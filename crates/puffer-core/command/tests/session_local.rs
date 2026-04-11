use super::*;

#[test]
fn branch_forks_and_switches_current_session() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let original_id = session.id;
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    state.push_message(MessageRole::User, "Inspect the auth flow");
    session_store
        .append_event(
            original_id,
            TranscriptEvent::UserMessage {
                text: "Inspect the auth flow".to_string(),
            },
        )
        .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/branch drydock",
    )
    .unwrap();

    assert_ne!(state.session.id, original_id);
    assert_eq!(state.session.parent_session_id, Some(original_id));
    assert_eq!(
        state.session.display_name.as_deref(),
        Some("drydock (Branch)")
    );
    assert!(state.transcript.last().unwrap().text.contains("/resume"));
}

#[test]
fn branch_without_explicit_name_derives_branch_title_from_first_user_message() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let original_id = session.id;
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    state.push_message(MessageRole::User, "Inspect the auth flow");
    session_store
        .append_event(
            original_id,
            TranscriptEvent::UserMessage {
                text: "Inspect the auth flow".to_string(),
            },
        )
        .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/branch",
    )
    .unwrap();

    assert_ne!(state.session.id, original_id);
    assert_eq!(
        state.session.display_name.as_deref(),
        Some("Inspect the auth flow (Branch)")
    );
}

#[test]
fn branch_refuses_to_fork_empty_conversations() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let original_id = session.id;
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/branch",
    )
    .unwrap();

    assert_eq!(state.session.id, original_id);
    assert_eq!(state.session.parent_session_id, None);
    assert!(state
        .transcript
        .last()
        .unwrap()
        .text
        .contains("No conversation to branch"));
}

#[test]
fn memory_command_updates_session_metadata() {
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/memory note keep shipping",
    )
    .unwrap();
    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/memory tag add parity",
    )
    .unwrap();
    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/memory slug shipyard",
    )
    .unwrap();

    let record = session_store.load_session(state.session.id).unwrap();
    assert_eq!(state.session.note.as_deref(), Some("keep shipping"));
    assert_eq!(state.session.slug.as_deref(), Some("shipyard"));
    assert!(state.session.tags.iter().any(|tag| tag == "parity"));
    assert_eq!(record.metadata.note.as_deref(), Some("keep shipping"));
    assert_eq!(record.metadata.slug.as_deref(), Some("shipyard"));
    assert!(record.metadata.tags.iter().any(|tag| tag == "parity"));
}

#[test]
fn config_command_routes_theme_to_user_config() {
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
        "/config set theme harbor",
    )
    .unwrap();

    let config_text = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(config_text.contains("theme = \"harbor\""));
    assert!(!paths.workspace_config_file().exists());
}

#[test]
fn config_command_accepts_json_object_values_for_workspace_settings() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let _home = ScopedPufferHome::set(tempdir.path());
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        r#"/config set openai_headers {"x-test":"one","x-extra":"two"}"#,
    )
    .unwrap();

    let config_text = std::fs::read_to_string(paths.workspace_config_file()).unwrap();
    assert!(config_text.contains("x-test = \"one\""));
    assert!(config_text.contains("x-extra = \"two\""));
}

#[test]
fn config_command_lists_supported_keys() {
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/config list",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("status_line_command")
    ));
}
