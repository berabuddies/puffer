//! E2E probe for the PR #122 changes (codex-style default writable
//! roots, scratchpad ad removed). Runs against real provider config
//! **without** `--yolo` / `danger-full-access`, so the path gate is
//! actually exercised.
//!
//! Two subjects under test:
//!
//! 1. **`/tmp` writes work under default `workspace-write`.** Pre-PR,
//!    the model bounced off the gate; post-PR it should not.
//!
//! 2. **Subagent dispatch (Agent tool) works.** Verifies the path-gate
//!    relaxation does not regress nested subagent dispatch (the bug
//!    PR #119 was chasing, now resolved via a different mechanism).
//!
//! Usage:
//!
//!   cargo run --example tool_e2e_probe -p puffer-core
//!
//! Env knobs:
//!
//!   PROBE_PROVIDER  override provider (defaults to config.default_provider)
//!   PROBE_MODEL     override model
//!   PROBE_PROMPT_TMP    bypass prompt 1 (default: a /tmp Bash write)
//!   PROBE_PROMPT_AGENT  bypass prompt 2 (default: an Agent dispatch)

use anyhow::{anyhow, Result};
use puffer_config::{ensure_workspace_dirs, load_config, ConfigPaths};
use puffer_core::{execute_user_turn, AppState, MessageRole, TurnExecution};
use puffer_provider_registry::ProviderRegistry;
use puffer_resources::load_resources;
use puffer_session_store::{SessionMetadata, SessionStore};

fn main() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let paths = ConfigPaths::discover(&cwd);
    ensure_workspace_dirs(&paths)?;
    let config = load_config(&paths)?;
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = puffer_provider_registry::AuthStore::load(&auth_path)?;
    let resources =
        load_resources(&paths, &puffer_core::runner_adapter::LocalToolRunner::new())?;

    let mut providers = ProviderRegistry::new();
    for provider in &resources.providers {
        let descriptor = provider.value.clone().into_descriptor();
        providers.register_with_source(descriptor, provider.source_info.as_provider_source());
    }
    providers.apply_openai_base_url_override(config.openai_base_url.as_deref());
    if !config.openai_headers.is_empty() {
        providers.set_openai_headers(
            config
                .openai_headers
                .clone()
                .into_iter()
                .collect::<indexmap::IndexMap<_, _>>(),
        );
    }
    if !config.openai_query_params.is_empty() {
        providers.set_openai_query_params(
            config
                .openai_query_params
                .clone()
                .into_iter()
                .collect::<indexmap::IndexMap<_, _>>(),
        );
    }
    let _ = providers.discover_and_merge_all(&auth_store);

    let provider_id = std::env::var("PROBE_PROVIDER")
        .unwrap_or_else(|_| config.default_provider.clone().unwrap_or_else(|| "hanbbq".to_string()));
    let model = resolve_model(&providers, &provider_id)?;
    eprintln!("[probe] provider={provider_id} model={model}");
    eprintln!("[probe] sandbox_mode=workspace-write (DEFAULT — no --yolo, gate active)");
    eprintln!();

    // Subject 1: /tmp write via Bash
    let tmp_target = std::env::temp_dir().join(format!(
        "puffer_e2e_{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp_target);

    let prompt_tmp = std::env::var("PROBE_PROMPT_TMP").unwrap_or_else(|_| {
        format!(
            "Use the Write tool (not Bash) to create a file at {} containing exactly the text 'hello-from-e2e'. After creating it, read it back with the Read tool and report the content. Use full absolute paths.",
            tmp_target.display()
        )
    });

    eprintln!("=== Subject 1: /tmp write via Write+Read tool (NOT Bash, exercises the path gate) ===");
    eprintln!("[probe] target: {}", tmp_target.display());
    eprintln!("[probe] prompt: {prompt_tmp}");
    eprintln!();

    let turn1 = run_one_turn(
        &config,
        &paths,
        &resources,
        &mut providers,
        &mut auth_store,
        &cwd,
        &provider_id,
        &model,
        &prompt_tmp,
        "probe-tmp-write",
    )?;

    // Verify the file landed
    let landed = std::fs::read_to_string(&tmp_target).ok();
    eprintln!();
    match landed.as_deref() {
        Some(content) => {
            eprintln!(
                "[probe] ✓ file landed at {} ({} bytes)",
                tmp_target.display(),
                content.len()
            );
            eprintln!("[probe]   content head: {:?}", &content[..content.len().min(80)]);
        }
        None => eprintln!("[probe] ✗ file did NOT land at {}", tmp_target.display()),
    }
    let _ = std::fs::remove_file(&tmp_target);
    print_last_assistant(&turn1);

    eprintln!();
    eprintln!("=== Subject 2: Agent tool dispatch (subagent writes a file) ===");
    let sub_target = std::env::temp_dir().join(format!(
        "puffer_sub_{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&sub_target);

    let prompt_agent = std::env::var("PROBE_PROMPT_AGENT").unwrap_or_else(|_| {
        format!(
            "Use the Agent tool (Task) to dispatch a general-purpose subagent. Give it this concrete task: \
             'Use the Write tool to create the file {} containing exactly the text \"sub-ok\". \
              Report the absolute path in your final reply.' \
             After the subagent returns, confirm the file path back to me. Use absolute paths.",
            sub_target.display()
        )
    });
    eprintln!("[probe] sub_target: {}", sub_target.display());
    eprintln!("[probe] prompt: {prompt_agent}");
    eprintln!();

    let turn2 = run_one_turn(
        &config,
        &paths,
        &resources,
        &mut providers,
        &mut auth_store,
        &cwd,
        &provider_id,
        &model,
        &prompt_agent,
        "probe-agent-dispatch",
    )?;
    let sub_landed = std::fs::read_to_string(&sub_target).ok();
    match sub_landed.as_deref() {
        Some(content) => {
            eprintln!(
                "[probe] ✓ SUBAGENT wrote file at {} ({} bytes, content={:?})",
                sub_target.display(),
                content.len(),
                content.trim()
            );
        }
        None => eprintln!(
            "[probe] ✗ subagent did NOT land file at {}",
            sub_target.display()
        ),
    }
    let _ = std::fs::remove_file(&sub_target);
    print_last_assistant(&turn2);

    eprintln!();
    eprintln!("[probe] done. Check session.jsonl files under ~/.puffer/sessions/ for full traces.");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_one_turn(
    config: &puffer_config::PufferConfig,
    paths: &ConfigPaths,
    resources: &puffer_resources::LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut puffer_provider_registry::AuthStore,
    cwd: &std::path::Path,
    provider_id: &str,
    model: &str,
    prompt: &str,
    display_name: &str,
) -> Result<AppState> {
    let session_store = SessionStore::from_paths(paths)?;
    let session_record = session_store.create_session(cwd.to_path_buf())?;
    let session = SessionMetadata {
        id: session_record.id,
        display_name: Some(display_name.to_string()),
        generated_title: None,
        cwd: cwd.to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(config.clone(), cwd.to_path_buf(), session);
    state.current_provider = Some(provider_id.to_string());
    state.current_model = Some(model.to_string());
    // explicit: DO NOT touch state.sandbox_mode; leave it at the default
    // "workspace-write" so the path gate is genuinely active.
    eprintln!("[probe] session: {}", state.session.id);
    eprintln!("[probe] sandbox_mode={} working_dirs={:?}", state.sandbox_mode, state.working_dirs);

    let _turn = execute_user_turn(&mut state, resources, providers, auth_store, prompt)?;
    Ok(state)
}

fn print_last_assistant(state: &AppState) {
    if let Some(last) = state
        .transcript
        .iter()
        .rev()
        .find(|m| matches!(m.role, MessageRole::Assistant))
    {
        eprintln!("--- assistant reply ---");
        eprintln!("{}", last.text);
    }
}

fn resolve_model(providers: &ProviderRegistry, provider_id: &str) -> Result<String> {
    if let Ok(model) = std::env::var("PROBE_MODEL") {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            return Ok(if trimmed.contains('/') {
                trimmed.to_string()
            } else {
                format!("{provider_id}/{trimmed}")
            });
        }
    }
    let provider = providers
        .provider(provider_id)
        .ok_or_else(|| anyhow!("{provider_id} provider is not configured"))?;
    let model = provider
        .models
        .first()
        .ok_or_else(|| anyhow!("{provider_id} provider has no configured models"))?;
    Ok(format!("{provider_id}/{}", model.id))
}
