use super::*;
use puffer_provider_registry::AuthMode;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn provider(id: &str, models: &[&str]) -> puffer_provider_registry::ProviderDescriptor {
    puffer_provider_registry::ProviderDescriptor {
        id: id.to_string(),
        display_name: id.to_string(),
        base_url: "https://example.invalid".to_string(),
        default_api: "openai-responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: models
            .iter()
            .map(|model_id| puffer_provider_registry::ModelDescriptor {
                id: (*model_id).to_string(),
                display_name: (*model_id).to_string(),
                provider: id.to_string(),
                api: "openai-responses".to_string(),
                context_window: 1000,
                max_output_tokens: 100,
                supports_reasoning: false,
                compat: None,
                input: vec![puffer_provider_registry::Modality::Text],
                cost: None,
            })
            .collect(),
        chat_completions_path: None,
    }
}

fn provider_with_discovery(
    id: &str,
    base_url: String,
    models: &[&str],
) -> puffer_provider_registry::ProviderDescriptor {
    let mut provider = provider(id, models);
    provider.base_url = base_url;
    provider.discovery = Some(puffer_provider_registry::ModelDiscoveryConfig {
        path: "/v1/models".to_string(),
        response: puffer_provider_registry::ModelDiscoveryFormat::OpenAiModels,
        api: "openai-responses".to_string(),
        context_window: 272_000,
        max_output_tokens: 16_384,
        supports_reasoning: true,
        items_field: "data".to_string(),
        id_field: "id".to_string(),
        display_name_field: None,
        headers: Default::default(),
    });
    provider
}

fn spawn_model_server() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 4096];
        let _ = stream.read(&mut buffer).unwrap();
        let body = serde_json::json!({
            "data": [
                { "id": "gpt-5" },
                { "id": "gpt-4.1" },
                { "id": "gpt-4.1-mini" }
            ]
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    address
}

#[test]
fn model_command_lists_only_models_for_selected_provider() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let mut providers = ProviderRegistry::new();
    providers.register(provider("openai", &["gpt-5", "gpt-5-mini"]));
    providers.register(provider("groq", &["compound"]));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/model",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage { text, .. })
            if text.contains("Current model: openai/gpt-5")
                && text.contains("Available models for openai: gpt-5, gpt-5-mini")
                && !text.contains("compound")
    ));
}

#[test]
fn model_command_refreshes_active_provider_before_listing() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let address = spawn_model_server();
    let mut providers = ProviderRegistry::new();
    providers.register(provider_with_discovery(
        "openai",
        format!("http://{address}"),
        &["gpt-5", "gpt-5-mini"],
    ));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut auth_store,
        &session_store,
        "/model",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage { text, .. })
            if text.contains("Available models for openai: ")
                && text.contains("gpt-4.1")
                && text.contains("gpt-4.1-mini")
    ));
}

#[test]
fn model_command_allows_cross_provider_selection_when_selector_is_explicit() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let mut providers = ProviderRegistry::new();
    providers.register(provider("openai", &["gpt-5"]));
    providers.register(provider("groq", &["compound"]));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/model groq/compound",
    )
    .unwrap();

    assert_eq!(state.current_provider.as_deref(), Some("groq"));
    assert_eq!(state.current_model.as_deref(), Some("groq/compound"));
}

#[test]
fn model_command_persists_selected_model_to_user_config() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let mut providers = ProviderRegistry::new();
    providers.register(provider("openai", &["gpt-5", "gpt-5-mini"]));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/model gpt-5-mini",
    )
    .unwrap();

    let saved = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(saved.contains("default_provider = \"openai\""));
    assert!(saved.contains("default_model = \"openai/gpt-5-mini\""));
}

#[test]
fn model_command_supports_alias_matching_for_active_provider() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("anthropic".to_string());
    config.default_model = Some("anthropic/claude-sonnet-4-5".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
    let mut providers = ProviderRegistry::new();
    providers.register(provider(
        "anthropic",
        &["claude-sonnet-4-5", "claude-opus-4-1"],
    ));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/model opus",
    )
    .unwrap();

    assert_eq!(
        state.current_model.as_deref(),
        Some("anthropic/claude-opus-4-1")
    );
}

#[test]
fn model_command_help_lists_default_and_provider_selector_usage() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let state = AppState::new(PufferConfig::default(), workspace, session);
    let mut state = state;

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/model help",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage { text, .. })
            if text.contains("/model default") && text.contains("/model <provider/model>")
    ));
}

#[test]
fn model_command_default_restores_configured_default_model() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("groq".to_string());
    state.current_model = Some("groq/compound".to_string());

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/model default",
    )
    .unwrap();

    assert_eq!(state.current_provider.as_deref(), Some("openai"));
    assert_eq!(state.current_model.as_deref(), Some("openai/gpt-5"));
}

#[test]
fn effort_command_persists_openai_reasoning_levels_to_user_config() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let mut providers = ProviderRegistry::new();
    providers.register(provider("openai", &["gpt-5"]));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/effort xhigh",
    )
    .unwrap();

    assert_eq!(state.effort_level, "xhigh");
    assert_eq!(state.config.effort_level.as_deref(), Some("xhigh"));
    let saved = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(saved.contains("effort_level = \"xhigh\""));
}

#[test]
fn effort_command_rejects_provider_incompatible_values() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("openai".to_string());
    let mut providers = ProviderRegistry::new();
    providers.register(provider("openai", &["gpt-5"]));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/effort max",
    )
    .unwrap();

    assert_eq!(state.effort_level, "auto");
    assert_eq!(state.config.effort_level, None);
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage { text, .. })
            if text.contains("Valid options are: minimal, low, medium, high, xhigh")
    ));
}

#[test]
fn effort_command_auto_clears_persisted_setting() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("anthropic".to_string());
    config.effort_level = Some("high".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("anthropic".to_string());
    let mut providers = ProviderRegistry::new();
    providers.register(provider("anthropic", &["claude-sonnet-4-5"]));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/effort auto",
    )
    .unwrap();

    assert_eq!(state.effort_level, "auto");
    assert_eq!(state.config.effort_level, None);
    let saved = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(!saved.contains("effort_level"));
}

#[test]
fn fast_command_persists_preference_to_user_config() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut state = AppState::new(PufferConfig::default(), workspace, session);

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/fast on",
    )
    .unwrap();

    assert!(state.fast_mode);
    assert!(state.config.fast_mode);
    let saved = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(saved.contains("fast_mode = true"));
}

#[test]
fn model_command_normalizes_effort_when_switching_provider_families() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    state.effort_level = "xhigh".to_string();
    let mut providers = ProviderRegistry::new();
    providers.register(provider("openai", &["gpt-5"]));
    providers.register(provider("anthropic", &["claude-sonnet-4-5"]));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/model anthropic/claude-sonnet-4-5",
    )
    .unwrap();

    assert_eq!(state.current_provider.as_deref(), Some("anthropic"));
    assert_eq!(
        state.current_model.as_deref(),
        Some("anthropic/claude-sonnet-4-5")
    );
    assert_eq!(state.effort_level, "high");
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage { text, .. })
            if text.contains("Effort level adjusted from xhigh to high for anthropic.")
    ));
}

#[test]
fn effort_command_rejects_provider_specific_levels_for_anthropic() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("anthropic".to_string());
    config.default_model = Some("anthropic/claude-sonnet-4-5".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
    let mut providers = ProviderRegistry::new();
    providers.register(provider("anthropic", &["claude-sonnet-4-5"]));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/effort xhigh",
    )
    .unwrap();

    assert_eq!(state.effort_level, "auto");
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage { text, .. })
            if text.contains("Invalid effort level `xhigh`.")
                && text.contains("Valid options are: low, medium, high, max")
    ));
}

#[test]
fn effort_command_accepts_auto_mode() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let mut providers = ProviderRegistry::new();
    providers.register(provider("openai", &["gpt-5"]));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/effort auto",
    )
    .unwrap();

    assert_eq!(state.effort_level, "auto");
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage { text, .. })
            if text.contains("Effort level set to auto.")
                && text.contains("Current provider default: low")
    ));
}
