use anyhow::{anyhow, Result};
use puffer_config::{ensure_workspace_dirs, load_config, ConfigPaths};
use puffer_core::{execute_user_turn_with_structured_output, AppState, StructuredOutputConfig};
use puffer_provider_registry::ProviderRegistry;
use puffer_resources::load_resources;
use puffer_session_store::SessionMetadata;
use uuid::Uuid;

fn main() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let paths = ConfigPaths::discover(&cwd);
    ensure_workspace_dirs(&paths)?;
    let config = load_config(&paths)?;
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = puffer_provider_registry::AuthStore::load(&auth_path)?;
    let mut resources =
        load_resources(&paths, &puffer_core::runner_adapter::LocalToolRunner::new())?;

    let mut providers = ProviderRegistry::new();
    for provider in &resources.providers {
        let mut descriptor = provider.value.clone().into_descriptor();
        if std::env::var("PROBE_FORCE_CHAT_COMPLETIONS")
            .ok()
            .as_deref()
            == Some("1")
            && descriptor.id == "openai"
        {
            descriptor.default_api = "openai-completions".to_string();
            for model in &mut descriptor.models {
                model.api = "openai-completions".to_string();
            }
        }
        providers.register_with_source(descriptor, provider.source_info.as_provider_source());
    }
    providers.apply_openai_base_url_override(config.openai_base_url.as_deref());
    if let Ok(base_url) = std::env::var("PROBE_OPENAI_BASE_URL") {
        let trimmed = base_url.trim().to_string();
        if !trimmed.is_empty() {
            providers.apply_openai_base_url_override(Some(&trimmed));
        }
    }
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
    if std::env::var("PROBE_KEEP_TOOLS").ok().as_deref() != Some("1") {
        resources.tools.clear();
    }
    if let Some(tool_ids) = probe_tool_ids() {
        resources
            .tools
            .retain(|tool| tool_ids.contains(&tool.value.id));
    }
    if std::env::var("PROBE_LIST_TOOLS").ok().as_deref() == Some("1") {
        for tool in &resources.tools {
            eprintln!("{}", tool.value.id);
        }
    }

    let provider_id = probe_provider_id();
    let session = SessionMetadata {
        id: Uuid::new_v4(),
        display_name: Some("structured-output-probe".to_string()),
        generated_title: None,
        cwd: cwd.clone(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(config, cwd.clone(), session);
    state.current_provider = Some(provider_id.clone());
    state.current_model = Some(resolve_model(&providers, &provider_id)?);

    let schema = StructuredOutputConfig::new(
        "probe_result",
        serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"],
            "additionalProperties": false
        }),
    );
    let prompt = std::env::var("PROBE_PROMPT").unwrap_or_else(|_| {
        "Reply with a short answer describing what provider/model you are, and nothing else."
            .to_string()
    });

    let turn = if std::env::var("PROBE_DISABLE_STRUCTURED_OUTPUT")
        .ok()
        .as_deref()
        == Some("1")
    {
        puffer_core::execute_user_turn(
            &mut state,
            &resources,
            &providers,
            &mut auth_store,
            &prompt,
        )?
    } else {
        execute_user_turn_with_structured_output(
            &mut state,
            &resources,
            &providers,
            &mut auth_store,
            &prompt,
            &schema,
        )?
    };

    println!("{}", turn.assistant_text);
    Ok(())
}

fn resolve_model(providers: &ProviderRegistry, provider_id: &str) -> Result<String> {
    if let Ok(model) = std::env::var("PROBE_MODEL") {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            if trimmed.contains('/') {
                return Ok(trimmed.to_string());
            }
            return Ok(format!("{provider_id}/{trimmed}"));
        }
    }

    let provider = providers
        .provider(provider_id)
        .ok_or_else(|| anyhow!("{provider_id} provider is not configured"))?;
    let preferred_model_id = match provider_id {
        "openai" => Some("gpt-5"),
        "anthropic" => Some("claude-sonnet-4-5"),
        _ => None,
    };
    let model = preferred_model_id
        .and_then(|id| provider.models.iter().find(|model| model.id == id))
        .or_else(|| provider.models.first())
        .ok_or_else(|| anyhow!("{provider_id} provider has no configured models"))?;
    Ok(format!("{provider_id}/{}", model.id))
}

fn probe_provider_id() -> String {
    std::env::var("PROBE_PROVIDER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "openai".to_string())
}

fn probe_tool_ids() -> Option<std::collections::BTreeSet<String>> {
    let raw = std::env::var("PROBE_TOOL_IDS").ok()?;
    let ids = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    (!ids.is_empty()).then_some(ids)
}
