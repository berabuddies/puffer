//! Real-LLM smoke test for `/recap`. Loads the user's actual puffer
//! config + provider registry + auth, seeds a tiny conversation into
//! the in-memory transcript, dispatches `/recap`, then prints whatever
//! ends up on the transcript afterward.
//!
//! Usage:
//!   cargo run --example recap_probe                # default provider/model
//!   PROBE_PROVIDER=hanbbq cargo run --example recap_probe
//!   PROBE_MODEL=gpt-5.5 cargo run --example recap_probe
//!
//! Expected: a final transcript line beginning with `※ recap: ` plus
//! 1-2 plain sentences. If the recap fails (no provider, network, etc.)
//! you'll see `Recap failed: …` instead — both are valid outcomes for
//! this probe; the bug we're hunting is "the slash command silently
//! does nothing."

use anyhow::{anyhow, Result};
use puffer_config::{ensure_workspace_dirs, load_config, ConfigPaths};
use puffer_core::{dispatch_command, supported_commands, AppState, MessageRole};
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
    let resources = load_resources(&paths, &puffer_core::runner_adapter::LocalToolRunner::new())?;

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

    let provider_id = std::env::var("PROBE_PROVIDER").unwrap_or_else(|_| {
        config
            .default_provider
            .clone()
            .unwrap_or_else(|| "hanbbq".to_string())
    });
    let model = resolve_model(&providers, &provider_id)?;

    eprintln!("[probe] provider={provider_id} model={model}");

    let session_store = SessionStore::from_paths(&paths)?;
    let session_record = session_store.create_session(cwd.clone())?;
    let session = SessionMetadata {
        id: session_record.id,
        display_name: Some("recap-probe".to_string()),
        generated_title: None,
        cwd: cwd.clone(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(config, cwd, session);
    state.current_provider = Some(provider_id);
    state.current_model = Some(model);

    // Seed a small synthetic exchange so the recap has something concrete
    // to summarize. The model never persists past the recap turn, so
    // these don't pollute anything.
    state.push_message(
        MessageRole::User,
        "Add a /recap command to puffer.".to_string(),
    );
    state.push_message(
        MessageRole::Assistant,
        "Added RecapConfig, recap.rs side-turn, slash command, and unit tests.".to_string(),
    );
    state.push_message(
        MessageRole::User,
        "Now wire the TUI auto-trigger.".to_string(),
    );
    state.push_message(
        MessageRole::Assistant,
        "Tracking last_user_input_at + enqueueing /recap when idle elapsed exceeds config.recap.idle_secs.".to_string(),
    );

    let before = state.transcript.len();
    eprintln!("[probe] transcript before dispatch: {} messages", before);

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut providers,
        &mut auth_store,
        &session_store,
        "/recap",
    )?;

    eprintln!(
        "[probe] transcript after dispatch: {} messages",
        state.transcript.len()
    );

    let last = state
        .transcript
        .last()
        .ok_or_else(|| anyhow!("transcript empty after dispatch — slash command did nothing"))?;

    println!("=== last transcript message ===");
    println!("role: {:?}", last.role);
    println!("text:\n{}", last.text);

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
