//! E2E probe for PTY (pseudo-terminal) support and WriteStdin tool.
//!
//! Sends a prompt to a real LLM model asking it to:
//! 1. Start a Python REPL using Bash with `tty: true`
//! 2. Interact with it via WriteStdin (compute 2+2, import sys, etc.)
//! 3. Exit the REPL cleanly
//!
//! Usage:
//!   cargo run --example pty_e2e_probe -p puffer-core
//!
//! Env:
//!   PROBE_PROVIDER  override provider (default: hanbbq)
//!   PROBE_MODEL     override model

use anyhow::{anyhow, Result};
use puffer_config::{ensure_workspace_dirs, load_config, ConfigPaths};
use puffer_core::{execute_user_turn, AppState, MessageRole};
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
    eprintln!("[pty-probe] provider={provider_id} model={model}");

    let prompt = r#"You have two tools available: Bash (with a "tty" boolean parameter) and WriteStdin (with "process_id" and "input" parameters).

Your task: demonstrate interactive PTY by doing the following steps IN ORDER:

1. Use the Bash tool with tty: true and command: "python3" to start a Python REPL.
   This will return a process_id and initial output (the Python prompt).

2. Use the WriteStdin tool with the process_id from step 1 and input: "2 + 2\n"
   to send "2 + 2" to the Python REPL. Report the output (should show 4).

3. Use WriteStdin again with input: "import sys; print(sys.version)\n"
   to get the Python version.

4. Use WriteStdin with input: "exit()\n" to cleanly exit the REPL.

After each step, report what output you received. At the end, summarize whether the PTY interaction worked correctly."#;

    eprintln!("[pty-probe] prompt:\n{prompt}\n");

    let session_store = SessionStore::from_paths(&paths)?;
    let session_record = session_store.create_session(cwd.to_path_buf())?;
    let session = SessionMetadata {
        id: session_record.id,
        display_name: Some("pty-e2e-probe".to_string()),
        generated_title: None,
        cwd: cwd.to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(config, cwd, session);
    state.current_provider = Some(provider_id.to_string());
    state.current_model = Some(model.clone());
    state.sandbox_mode = "danger-full-access".to_string();

    eprintln!(
        "[pty-probe] session={} sandbox_mode={}",
        state.session.id, state.sandbox_mode
    );
    eprintln!();

    let turn = execute_user_turn(&mut state, &resources, &mut providers, &mut auth_store, prompt)?;
    eprintln!();
    eprintln!(
        "[pty-probe] turn completed: {} tool invocations",
        turn.tool_invocations.len()
    );
    for (i, inv) in turn.tool_invocations.iter().enumerate() {
        eprintln!("  [{i}] tool={} input={}", inv.tool_id, inv.input);
        eprintln!("      output={}", &inv.output[..inv.output.len().min(500)]);
    }

    eprintln!();
    eprintln!("--- full transcript ---");
    for msg in &state.transcript {
        let role_str = match msg.role {
            MessageRole::User => "USER",
            MessageRole::Assistant => "ASST",
            _ => "????",
        };
        let text_preview = &msg.text[..msg.text.len().min(800)];
        eprintln!("[{role_str}] {text_preview}");
        eprintln!();
    }

    {
        let mut store = state.process_store.lock().unwrap();
        let exited = store.drain_exited();
        eprintln!();
        eprintln!("[pty-probe] ProcessStore: {} processes drained (exited)", exited.len());
        for (pid, cmd, code) in &exited {
            eprintln!("  pid={pid} cmd={cmd:?} exit_code={code:?}");
        }
    }

    eprintln!("[pty-probe] done.");
    Ok(())
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
