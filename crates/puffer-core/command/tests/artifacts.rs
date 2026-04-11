use super::*;

#[test]
fn copy_selection_reaches_back_to_older_assistant_messages() {
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
    state.push_message(MessageRole::Assistant, "oldest");
    state.push_message(MessageRole::Assistant, "older");
    state.push_message(MessageRole::Assistant, "latest");

    let selection =
        crate::command_helpers::artifacts::select_copy_target(&state.transcript, "2").unwrap();

    assert_eq!(selection.text, "older");
    assert_eq!(selection.age, 1);
    assert_eq!(selection.total, 3);
}

#[test]
fn copy_command_reports_invalid_history_index() {
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
    state.push_message(MessageRole::Assistant, "latest");

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/copy 0",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Usage: /copy [N]")
    ));
}

#[test]
fn copy_picker_actions_list_full_response_and_code_blocks() {
    let tempdir = tempdir().unwrap();
    let session = puffer_session_store::SessionMetadata {
        id: uuid::Uuid::nil(),
        display_name: None,
        cwd: tempdir.path().to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    state.push_message(
        MessageRole::Assistant,
        "Intro\n```rs\nfn main() {}\n```\n```json\n{\"ok\":true}\n```",
    );

    let actions = crate::command_helpers::artifacts::render_copy_actions(&state, "")
        .unwrap()
        .expect("copy actions");

    assert_eq!(actions[0].label, "Full response");
    assert_eq!(actions[0].command, "/copy --full 0");
    assert_eq!(actions[1].label, "fn main() {}");
    assert_eq!(actions[1].description, "rs");
    assert_eq!(actions[1].command, "/copy --code 0 0");
    assert_eq!(actions[2].label, "{\"ok\":true}");
    assert_eq!(actions[2].description, "json");
    assert_eq!(actions[2].command, "/copy --code 0 1");
    assert_eq!(actions[3].label, "Always copy full response");
}

#[test]
fn copy_internal_commands_copy_code_blocks_and_persist_preference() {
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
    state.push_message(
        MessageRole::Assistant,
        "```rs\nfn main() {}\n```\n```txt\nhello\n```",
    );

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/copy --code 0 0",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("copy.rs")
    ));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/copy --always-full 0",
    )
    .unwrap();

    assert!(state.config.copy_full_response);
    let saved = std::fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(
        saved.contains("copy_full_response = true"),
        "saved config:\n{saved}"
    );
}

#[test]
fn export_command_writes_plain_text_transcript_to_txt_file() {
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
    state.push_message(MessageRole::User, "Review current diff");
    state.push_message(MessageRole::Assistant, "The diff is clean.");

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/export ship-notes.md",
    )
    .unwrap();

    let target = tempdir.path().join("ship-notes.txt");
    let contents = std::fs::read_to_string(&target).unwrap();
    assert!(contents.contains("Puffer Code Conversation Export"));
    assert!(contents.contains("## User"));
    assert!(contents.contains("Review current diff"));
    assert!(contents.contains("## Assistant"));
    assert!(contents.contains("The diff is clean."));
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Conversation exported to")
            && text.contains("ship-notes.txt")
    ));
}
