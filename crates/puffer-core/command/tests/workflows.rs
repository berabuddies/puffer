use super::*;

#[test]
fn workflows_new_accepts_planned_connection_and_pattern() {
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
        "/workflows new Hi-Archive telegram-user hi",
    )
    .unwrap();

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("slug: hi-archive"));
    assert!(text.contains("trigger: connection:telegram-user pattern=hi"));
    assert!(text.contains("connect: /connect telegram-login telegram-user"));
    let workflows = puffer_workflow::WorkflowStore::new(&paths.workspace_config_dir)
        .list()
        .unwrap();
    assert_eq!(workflows.len(), 1);
    assert!(matches!(
        &workflows[0].trigger,
        puffer_workflow::TriggerSpec::Connection {
            connection_slug,
            pattern: Some(pattern),
            ..
        } if connection_slug == "telegram-user" && pattern == "hi"
    ));
}
