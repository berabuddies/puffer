use super::*;

#[test]
fn sandbox_command_reports_removal_without_writing_config() {
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
        "/sandbox read-only",
    )
    .unwrap();

    let sandbox_path = paths.workspace_config_dir.join("sandbox.toml");
    assert!(!sandbox_path.exists());
    assert!(state
        .transcript
        .last()
        .is_some_and(|message| message.text.contains("Sandbox mode has been removed")));
}
