use super::*;
use crate::command_helpers::with_login_flow_handler;
use puffer_config::save_user_config;
use puffer_provider_registry::AuthMode;

fn provider(
    id: &str,
    api: &str,
    auth_modes: Vec<AuthMode>,
) -> puffer_provider_registry::ProviderDescriptor {
    puffer_provider_registry::ProviderDescriptor {
        id: id.to_string(),
        display_name: id.to_string(),
        base_url: "https://example.invalid".to_string(),
        default_api: api.to_string(),
        auth_modes,
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![puffer_provider_registry::ModelDescriptor {
            id: "model".to_string(),
            display_name: "model".to_string(),
            provider: id.to_string(),
            api: api.to_string(),
            context_window: 1000,
            max_output_tokens: 100,
            supports_reasoning: false,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    }
}

#[test]
fn login_command_runs_oauth_flow_for_oauth_capable_provider() {
    let tempdir = tempdir().unwrap();
    let _home_lock = lock_puffer_home();
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
    let mut providers = ProviderRegistry::new();
    providers.register(provider(
        "custom-openai",
        "openai-responses",
        vec![AuthMode::ApiKey, AuthMode::OAuth],
    ));

    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    with_login_flow_handler(
        move |request| {
            assert_eq!(request.provider_id, "custom-openai");
            assert_eq!(request.auth_path, auth_path);
            let mut written = AuthStore::default();
            written.set_api_key("custom-openai", "token");
            written.save(&request.auth_path)?;
            Ok(())
        },
        || {
            dispatch_command(
                &mut state,
                &supported_commands(),
                &LoadedResources::default(),
                &mut providers,
                &mut auth_store,
                &session_store,
                "/login custom-openai",
            )
            .unwrap();
        },
    );

    assert!(auth_store.has_auth("custom-openai"));

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text == "Completed login flow for custom-openai."
    ));
}

#[test]
fn login_command_reports_guidance_when_oauth_flow_fails() {
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
    let mut providers = ProviderRegistry::new();
    providers.register(provider(
        "custom-openai",
        "openai-responses",
        vec![AuthMode::ApiKey, AuthMode::OAuth],
    ));
    let mut auth_store = AuthStore::default();

    with_login_flow_handler(
        |_request| anyhow::bail!("browser launch blocked"),
        || {
            dispatch_command(
                &mut state,
                &supported_commands(),
                &LoadedResources::default(),
                &mut providers,
                &mut auth_store,
                &session_store,
                "/login custom-openai",
            )
            .unwrap();
        },
    );

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Login failed for custom-openai: browser launch blocked")
            && text.contains("Supported auth modes: api_key, oauth")
            && text.contains("API key: `puffer auth set-api-key custom-openai --stdin`")
            && text.contains("OAuth start bundle:")
    ));
}

#[test]
fn login_command_reports_session_ingress_support() {
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
    let mut providers = ProviderRegistry::new();
    providers.register(provider(
        "custom-anthropic",
        "anthropic-messages",
        vec![AuthMode::ApiKey, AuthMode::SessionIngress],
    ));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/login custom-anthropic",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("session_ingress")
            && text.contains("Session ingress: exported session-ingress credentials are supported.")
    ));
}

#[test]
fn login_command_reports_when_provider_has_no_auth_modes() {
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
    let mut providers = ProviderRegistry::new();
    providers.register(provider("ollama", "openai-completions", Vec::new()));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/login ollama",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text == "ollama does not require stored credentials."
    ));
}

#[test]
fn logout_command_removes_anthropic_credentials_and_clears_active_selection() {
    let tempdir = tempdir().unwrap();
    let _home_lock = lock_puffer_home();
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
    config.default_model = Some("anthropic/model".to_string());
    save_user_config(&paths, &config).unwrap();

    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/model".to_string());

    let mut providers = ProviderRegistry::new();
    providers.register(provider(
        "anthropic",
        "anthropic-messages",
        vec![AuthMode::ApiKey, AuthMode::OAuth],
    ));

    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("anthropic", "sk-ant");
    auth_store.save(&auth_path).unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut auth_store,
        &session_store,
        "/logout anthropic",
    )
    .unwrap();

    assert_eq!(state.current_provider, None);
    assert_eq!(state.current_model, None);
    assert_eq!(state.config.default_provider, None);
    assert_eq!(state.config.default_model, None);
    assert!(!auth_store.has_auth("anthropic"));
    assert!(!AuthStore::load(&auth_path).unwrap().has_auth("anthropic"));
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text == "Removed stored credentials for anthropic and cleared the active selection."
    ));
}

#[test]
fn logout_command_clears_selection_when_model_provider_matches_openai() {
    let tempdir = tempdir().unwrap();
    let _home_lock = lock_puffer_home();
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
    config.default_model = Some("openai/model".to_string());
    save_user_config(&paths, &config).unwrap();

    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("openai/model".to_string());

    let mut providers = ProviderRegistry::new();
    providers.register(provider(
        "openai",
        "openai-responses",
        vec![AuthMode::ApiKey, AuthMode::OAuth],
    ));

    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    auth_store.save(&auth_path).unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut auth_store,
        &session_store,
        "/logout openai",
    )
    .unwrap();

    assert_eq!(state.current_provider, None);
    assert_eq!(state.current_model, None);
    assert_eq!(state.config.default_provider, None);
    assert_eq!(state.config.default_model, None);
    assert!(!auth_store.has_auth("openai"));
    assert!(!AuthStore::load(&auth_path).unwrap().has_auth("openai"));
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text == "Removed stored credentials for openai and cleared the active selection."
    ));
}

#[test]
fn logout_command_defaults_to_current_provider_when_args_are_omitted() {
    let tempdir = tempdir().unwrap();
    let _home_lock = lock_puffer_home();
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
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/model".to_string());

    let mut providers = ProviderRegistry::new();
    providers.register(provider(
        "openai",
        "openai-responses",
        vec![AuthMode::ApiKey, AuthMode::OAuth],
    ));

    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    auth_store.save(&auth_path).unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &mut auth_store,
        &session_store,
        "/logout",
    )
    .unwrap();

    assert_eq!(state.current_provider, None);
    assert_eq!(state.current_model, None);
    assert!(!auth_store.has_auth("openai"));
    assert!(!AuthStore::load(&auth_path).unwrap().has_auth("openai"));
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text == "Removed stored credentials for openai and cleared the active selection."
    ));
}
