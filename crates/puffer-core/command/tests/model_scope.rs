use super::*;
use puffer_provider_registry::AuthMode;
use std::sync::{Mutex, OnceLock};

fn puffer_home_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn provider(id: &str, models: &[&str]) -> puffer_provider_registry::ProviderDescriptor {
    puffer_provider_registry::ProviderDescriptor {
        id: id.to_string(),
        display_name: id.to_string(),
        base_url: "https://example.invalid".to_string(),
        default_api: "openai-responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: Default::default(),
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
            })
            .collect(),
    }
}

#[test]
fn model_command_lists_only_models_for_selected_provider() {
    let tempdir = tempdir().unwrap();
    let _lock = puffer_home_lock().lock().unwrap();
    let old_home = std::env::var_os("PUFFER_HOME");
    std::env::set_var("PUFFER_HOME", tempdir.path());
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, tempdir.path().to_path_buf(), session);
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
        &AuthStore::default(),
        &session_store,
        "/model",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage { text, .. })
            if text.contains("Available models for openai: gpt-5, gpt-5-mini")
                && !text.contains("compound")
    ));
    if let Some(value) = old_home {
        std::env::set_var("PUFFER_HOME", value);
    } else {
        std::env::remove_var("PUFFER_HOME");
    }
}

#[test]
fn model_command_rejects_cross_provider_selection() {
    let tempdir = tempdir().unwrap();
    let _lock = puffer_home_lock().lock().unwrap();
    let old_home = std::env::var_os("PUFFER_HOME");
    std::env::set_var("PUFFER_HOME", tempdir.path());
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, tempdir.path().to_path_buf(), session);
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
        &AuthStore::default(),
        &session_store,
        "/model groq/compound",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage { text, .. })
            if text.contains("/model only selects models for the active provider (openai).")
    ));
    if let Some(value) = old_home {
        std::env::set_var("PUFFER_HOME", value);
    } else {
        std::env::remove_var("PUFFER_HOME");
    }
}

#[test]
fn model_command_persists_selected_model_to_user_config() {
    let tempdir = tempdir().unwrap();
    let _lock = puffer_home_lock().lock().unwrap();
    let old_home = std::env::var_os("PUFFER_HOME");
    std::env::set_var("PUFFER_HOME", tempdir.path());
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    let mut state = AppState::new(config, tempdir.path().to_path_buf(), session);
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let mut providers = ProviderRegistry::new();
    providers.register(provider("openai", &["gpt-5", "gpt-5-mini"]));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut providers,
        &AuthStore::default(),
        &session_store,
        "/model gpt-5-mini",
    )
    .unwrap();

    let saved = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(saved.contains("default_provider = \"openai\""));
    assert!(saved.contains("default_model = \"openai/gpt-5-mini\""));
    if let Some(value) = old_home {
        std::env::set_var("PUFFER_HOME", value);
    } else {
        std::env::remove_var("PUFFER_HOME");
    }
}
