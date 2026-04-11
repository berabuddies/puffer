use super::*;
use puffer_provider_registry::{AuthMode, ModelDescriptor};

#[test]
fn status_command_reports_richer_session_and_resource_status() {
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
        session,
    );
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    state.remote_name = Some("dockyard".to_string());
    state.fast_mode = true;

    let mut providers = ProviderRegistry::new();
    providers.register(ProviderDescriptor {
        id: "openai".to_string(),
        display_name: "OpenAI".to_string(),
        base_url: "https://api.openai.com".to_string(),
        default_api: "openai-responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            provider: "openai".to_string(),
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            api: "openai-responses".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
        }],
    });
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai".to_string(), "sk-test".to_string());

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut auth_store,
        &session_store,
        "/status",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Status")
            && text.contains("Provider: OpenAI")
            && text.contains("Authentication: API key")
            && text.contains("Base URL: https://api.openai.com")
            && text.contains("Fast mode: true")
            && text.contains("Remote name: dockyard")
            && text.contains("Resource status")
    ));
}
