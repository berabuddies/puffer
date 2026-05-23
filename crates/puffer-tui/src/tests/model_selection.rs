use super::*;
use puffer_core::{default_effort_level, supported_commands, ModelPreferenceFamily};

fn picker_commands() -> Vec<CommandSpec> {
    supported_commands()
}

fn temp_session_store(tempdir: &tempfile::TempDir) -> SessionStore {
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    SessionStore::from_paths(&paths).unwrap()
}

#[test]
fn onboarding_model_picker_enter_preserves_current_effort_selection() {
    let tempdir = tempdir().unwrap();
    let _home = crate::test_env::ScopedPufferHome::new("model-selection-onboarding");
    let session_store = temp_session_store(&tempdir);
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    auth_store.set_api_key("openai".to_string(), "token".to_string());
    let auth_path = ConfigPaths::discover(tempdir.path())
        .user_config_dir
        .join("auth.json");
    let commands = picker_commands();
    let mut tui = TuiState::default();
    tui.overlay = Some(OverlayState::ModelPicker {
        provider_id: "openai".to_string(),
        entries: vec![
            ModelPickerEntry {
                selector: "gpt-5".to_string(),
                description: "GPT-5".to_string(),
                command: None,
            },
            ModelPickerEntry {
                selector: "gpt-5-mini".to_string(),
                description: "GPT-5 Mini".to_string(),
                command: None,
            },
        ],
        selection: 0,
        onboarding: true,
    });

    handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &commands,
        &mut tui,
        true,
    )
    .unwrap();

    match &tui.overlay {
        Some(OverlayState::EffortPicker {
            provider_id,
            model_id,
            entries,
            selection,
            ..
        }) => {
            assert_eq!(provider_id, "openai");
            assert_eq!(model_id, "gpt-5");
            assert_eq!(
                entries[*selection].selector,
                default_effort_level(ModelPreferenceFamily::OpenAi)
            );
            assert!(entries.iter().any(|entry| entry.selector == "xhigh"));
            assert!(entries.iter().any(|entry| entry.selector == "minimal"));
        }
        other => panic!("expected effort picker, got {other:?}"),
    }
}

#[test]
fn slash_command_model_picker_enter_applies_selected_model_immediately() {
    let tempdir = tempdir().unwrap();
    let _home = crate::test_env::ScopedPufferHome::new("model-selection-slash");
    let session_store = temp_session_store(&tempdir);
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    auth_store.set_api_key("openai".to_string(), "token".to_string());
    let auth_path = ConfigPaths::discover(tempdir.path())
        .user_config_dir
        .join("auth.json");
    let commands = picker_commands();
    let mut tui = TuiState::default();
    tui.overlay = Some(OverlayState::ModelPicker {
        provider_id: "openai".to_string(),
        entries: vec![ModelPickerEntry {
            selector: "gpt-5".to_string(),
            description: "GPT-5".to_string(),
            command: None,
        }],
        selection: 0,
        onboarding: false,
    });

    handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &commands,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(tui.overlay.is_none(), "overlay={:?}", tui.overlay);
    assert_eq!(state.current_provider.as_deref(), Some("openai"));
    assert_eq!(state.current_model.as_deref(), Some("openai/gpt-5"));
}

#[test]
fn model_picker_unmatched_query_does_not_apply_stale_selection() {
    let tempdir = tempdir().unwrap();
    let _home = crate::test_env::ScopedPufferHome::new("model-selection-unmatched-query");
    let session_store = temp_session_store(&tempdir);
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    auth_store.set_api_key("openai".to_string(), "token".to_string());
    let auth_path = ConfigPaths::discover(tempdir.path())
        .user_config_dir
        .join("auth.json");
    let commands = picker_commands();
    let mut tui = TuiState::default();
    tui.overlay = Some(OverlayState::ModelPicker {
        provider_id: "openai".to_string(),
        entries: vec![
            ModelPickerEntry {
                selector: "gpt-5".to_string(),
                description: "GPT-5".to_string(),
                command: None,
            },
            ModelPickerEntry {
                selector: "gpt-5-mini".to_string(),
                description: "GPT-5 Mini".to_string(),
                command: None,
            },
        ],
        selection: 0,
        onboarding: false,
    });

    for ch in "zzzz".chars() {
        handle_key(
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            &mut state,
            &mut resources,
            &mut providers,
            &mut auth_store,
            &auth_path,
            &session_store,
            &commands,
            &mut tui,
            true,
        )
        .unwrap();
    }
    handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &commands,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(matches!(
        tui.overlay,
        Some(OverlayState::ModelPicker { .. })
    ));
    assert_eq!(state.current_model.as_deref(), None);
    assert_eq!(
        tui.status_hint.as_ref().map(|(hint, _)| hint.as_str()),
        Some("No matching picker item.")
    );
}

#[test]
fn effort_picker_enter_opens_fast_mode_picker_with_provider_defaults() {
    let tempdir = tempdir().unwrap();
    let _home = crate::test_env::ScopedPufferHome::new("model-selection-effort");
    let session_store = temp_session_store(&tempdir);
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    auth_store.set_api_key("openai".to_string(), "token".to_string());
    let auth_path = ConfigPaths::discover(tempdir.path())
        .user_config_dir
        .join("auth.json");
    let commands = picker_commands();
    let mut tui = TuiState::default();
    tui.overlay = Some(OverlayState::EffortPicker {
        provider_id: "anthropic".to_string(),
        model_id: "claude-sonnet-4-5".to_string(),
        entries: vec![
            ModelPickerEntry {
                selector: "low".to_string(),
                description: "low".to_string(),
                command: None,
            },
            ModelPickerEntry {
                selector: "high".to_string(),
                description: "high".to_string(),
                command: None,
            },
        ],
        selection: 1,
        onboarding: true,
    });

    handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &commands,
        &mut tui,
        true,
    )
    .unwrap();

    match &tui.overlay {
        Some(OverlayState::FastModePicker {
            provider_id,
            model_id,
            effort,
            entries,
            selection,
            ..
        }) => {
            assert_eq!(provider_id, "anthropic");
            assert_eq!(model_id, "claude-sonnet-4-5");
            assert_eq!(effort, "high");
            assert_eq!(entries[*selection].selector, "off");
        }
        other => panic!("expected fast mode picker, got {other:?}"),
    }
}

#[test]
fn fast_mode_picker_enter_applies_model_effort_and_fast_mode() {
    let tempdir = tempdir().unwrap();
    let _home = crate::test_env::ScopedPufferHome::new("model-selection-fast");
    let session_store = temp_session_store(&tempdir);
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    auth_store.set_api_key("openai".to_string(), "token".to_string());
    let auth_path = ConfigPaths::discover(tempdir.path())
        .user_config_dir
        .join("auth.json");
    let commands = picker_commands();
    let mut tui = TuiState::default();
    tui.overlay = Some(OverlayState::FastModePicker {
        provider_id: "openai".to_string(),
        model_id: "gpt-5".to_string(),
        effort: "high".to_string(),
        entries: vec![
            ModelPickerEntry {
                selector: "on".to_string(),
                description: "fast".to_string(),
                command: None,
            },
            ModelPickerEntry {
                selector: "off".to_string(),
                description: "normal".to_string(),
                command: None,
            },
        ],
        selection: 0,
        onboarding: true,
    });

    handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &commands,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(tui.overlay.is_none(), "overlay={:?}", tui.overlay);
    assert_eq!(state.current_provider.as_deref(), Some("openai"));
    assert_eq!(state.current_model.as_deref(), Some("openai/gpt-5"));
    assert_eq!(state.config.default_model.as_deref(), Some("openai/gpt-5"));
    assert_eq!(state.effort_level, "high");
    assert!(state.fast_mode);
}
