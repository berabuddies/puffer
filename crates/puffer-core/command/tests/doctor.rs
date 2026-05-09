use super::*;
use puffer_provider_registry::{AuthMode, ModelDescriptor, OAuthCredential};

fn provider(id: &str, auth_modes: Vec<AuthMode>) -> ProviderDescriptor {
    ProviderDescriptor {
        id: id.to_string(),
        display_name: id.to_ascii_uppercase(),
        base_url: format!("https://{}.example.test", id),
        default_api: if id == "anthropic" {
            "anthropic-messages".to_string()
        } else {
            "openai-responses".to_string()
        },
        auth_modes,
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            id: if id == "anthropic" {
                "claude-sonnet-4-5".to_string()
            } else {
                "gpt-5".to_string()
            },
            display_name: "primary".to_string(),
            provider: id.to_string(),
            api: if id == "anthropic" {
                "anthropic-messages".to_string()
            } else {
                "openai-responses".to_string()
            },
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    }
}

#[test]
fn doctor_command_emits_rich_diagnostics_report() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session.clone(),
    );
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    providers.register(provider("anthropic", vec![AuthMode::ApiKey]));
    providers.register(provider("openai", vec![AuthMode::ApiKey, AuthMode::OAuth]));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("anthropic", "sk-ant");
    auth_store.set_oauth(
        "openai",
        OAuthCredential {
            email: Some("dev@example.com".to_string()),
            ..OAuthCredential::default()
        },
    );

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut providers,
        &mut auth_store,
        &session_store,
        "/doctor",
    )
    .unwrap();

    let message = state
        .transcript
        .last()
        .map(|message| message.text.clone())
        .unwrap();
    assert!(message.contains("Puffer doctor"));
    assert!(message.contains("Runtime:"));
    assert!(message.contains("Workspace:"));
    assert!(message.contains("Providers:"));
    assert!(message.contains("- anthropic [builtin] auth=api-key"));
    assert!(message.contains("- openai [builtin] auth=oauth"));
    assert!(message.contains("Resources:"));
    assert!(message.contains("Dependencies:"));
    assert!(message.contains("Warnings:"));
}
