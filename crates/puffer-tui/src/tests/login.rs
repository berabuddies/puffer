use super::*;

fn openai_only_providers() -> ProviderRegistry {
    let mut providers = ProviderRegistry::default();
    providers.register(ProviderDescriptor {
        id: "openai".to_string(),
        display_name: "OpenAI".to_string(),
        base_url: "https://api.openai.com".to_string(),
        default_api: "openai-responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            provider: "openai".to_string(),
            api: "openai-responses".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    });
    providers
}

#[test]
fn login_picker_selection_is_auth_only() {
    let tempdir = tempdir().unwrap();
    let _home = crate::test_env::ScopedPufferHome::new("login-picker-auth-only");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("anthropic".to_string());
    config.default_model = Some("anthropic/claude-sonnet-4-5".to_string());
    save_user_config(&paths, &config).unwrap();
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = sample_auth_store();
    let mut providers = sample_providers();
    let mut resources = sample_resources();
    let mut tui = TuiState::default();

    assert!(try_open_overlay(
        &state,
        &resources,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/login",
    )
    .unwrap());
    match tui.overlay.as_mut() {
        Some(OverlayState::LoginPicker { entries, selection }) => {
            *selection = entries
                .iter()
                .position(|entry| entry.selector == "openai")
                .expect("openai login entry");
        }
        _ => panic!("login picker"),
    }

    handle_overlay_key(
        KeyEvent::from(KeyCode::Enter),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();

    assert_eq!(state.current_provider.as_deref(), Some("anthropic"));
    assert_eq!(
        state.current_model.as_deref(),
        Some("anthropic/claude-sonnet-4-5")
    );
    match tui.overlay.as_mut() {
        Some(OverlayState::AuthPicker {
            provider_id,
            entries,
            selection,
            onboarding,
        }) => {
            assert_eq!(provider_id, "openai");
            assert!(!*onboarding);
            *selection = entries
                .iter()
                .position(|entry| entry.label == "api-key")
                .expect("api key action");
        }
        _ => panic!("auth picker"),
    }

    handle_overlay_key(
        KeyEvent::from(KeyCode::Enter),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();

    assert_eq!(state.current_provider.as_deref(), Some("anthropic"));
    assert_eq!(
        state.current_model.as_deref(),
        Some("anthropic/claude-sonnet-4-5")
    );
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::ApiKeyPrompt {
            ref provider_id,
            onboarding: false,
            ..
        }) if provider_id == "openai"
    ));

    for ch in "sk-openai".chars() {
        handle_overlay_key(
            KeyEvent::from(KeyCode::Char(ch)),
            &mut state,
            &mut resources,
            &mut providers,
            &mut auth_store,
            &auth_path,
            &session_store,
            &mut tui,
            true,
        )
        .unwrap();
    }
    handle_overlay_key(
        KeyEvent::from(KeyCode::Enter),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();

    assert_eq!(state.current_provider.as_deref(), Some("anthropic"));
    assert_eq!(
        state.current_model.as_deref(),
        Some("anthropic/claude-sonnet-4-5")
    );
    let saved = puffer_config::load_config(&paths).unwrap();
    assert_eq!(saved.default_provider.as_deref(), Some("anthropic"));
    assert_eq!(
        saved.default_model.as_deref(),
        Some("anthropic/claude-sonnet-4-5")
    );
    assert!(auth_store.has_auth("openai"));
    assert!(tui.overlay.is_none());
}

#[test]
fn login_picker_filter_cursor_movement_edits_query() {
    let tempdir = tempdir().unwrap();
    let _home = crate::test_env::ScopedPufferHome::new("login-picker-filter-cursor");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut state = AppState::new(PufferConfig::default(), workspace, session);
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut providers = openai_only_providers();
    let mut resources = sample_resources();
    let mut tui = TuiState::default();

    assert!(try_open_overlay(
        &state,
        &resources,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/login",
    )
    .unwrap());

    for ch in "opnai".chars() {
        handle_overlay_key(
            KeyEvent::from(KeyCode::Char(ch)),
            &mut state,
            &mut resources,
            &mut providers,
            &mut auth_store,
            &auth_path,
            &session_store,
            &mut tui,
            true,
        )
        .unwrap();
    }
    for _ in 0..3 {
        handle_overlay_key(
            KeyEvent::from(KeyCode::Left),
            &mut state,
            &mut resources,
            &mut providers,
            &mut auth_store,
            &auth_path,
            &session_store,
            &mut tui,
            true,
        )
        .unwrap();
    }
    handle_overlay_key(
        KeyEvent::from(KeyCode::Char('e')),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();

    assert_eq!(tui.input, "openai");

    handle_overlay_key(
        KeyEvent::from(KeyCode::Enter),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(matches!(
        tui.overlay,
        Some(OverlayState::AuthPicker {
            ref provider_id,
            ..
        }) if provider_id == "openai"
    ));
}

#[test]
fn stale_provider_prompt_opens_provider_picker() {
    let tempdir = tempdir().unwrap();
    let _home = crate::test_env::ScopedPufferHome::new("stale-provider-picker");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    save_user_config(&paths, &PufferConfig::default()).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut state = AppState::new(PufferConfig::default(), workspace, session);
    state.current_provider = Some("removed-provider".to_string());
    state.current_model = Some("removed-provider/missing-model".to_string());
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut resources = sample_resources();
    let mut providers = openai_only_providers();
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "hello with stale provider".to_string(),
        true,
    )
    .unwrap();

    assert!(!tui.has_pending_submit());
    assert_eq!(
        tui.deferred_prompt.as_deref(),
        Some("hello with stale provider")
    );
    assert!(state.transcript.is_empty());
    match tui.overlay {
        Some(OverlayState::ProviderPicker {
            entries,
            onboarding: true,
            ..
        }) => {
            assert!(entries.iter().any(|entry| entry.selector == "openai"));
            assert!(!entries
                .iter()
                .any(|entry| entry.selector == "removed-provider"));
        }
        other => panic!("expected provider picker, got {other:?}"),
    }
}

#[test]
fn canceling_onboarding_clears_deferred_prompt() {
    let tempdir = tempdir().unwrap();
    let _home = crate::test_env::ScopedPufferHome::new("cancel-onboarding-deferred-prompt");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    save_user_config(&paths, &PufferConfig::default()).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut state = AppState::new(PufferConfig::default(), workspace, session);
    state.current_provider = Some("removed-provider".to_string());
    state.current_model = Some("removed-provider/missing-model".to_string());
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut resources = sample_resources();
    let mut providers = openai_only_providers();
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "hello with stale provider".to_string(),
        true,
    )
    .unwrap();

    assert_eq!(
        tui.deferred_prompt.as_deref(),
        Some("hello with stale provider")
    );
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::ProviderPicker {
            onboarding: true,
            ..
        })
    ));

    handle_overlay_key(
        KeyEvent::from(KeyCode::Esc),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::ThemePicker { .. })
    ));

    handle_overlay_key(
        KeyEvent::from(KeyCode::Esc),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(tui.overlay.is_none());
    assert!(tui.deferred_prompt.is_none());
    submit_queued_prompt_if_ready(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();
    assert!(!tui.has_pending_submit());
    assert!(state.transcript.is_empty());
}
