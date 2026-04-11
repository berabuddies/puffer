use super::*;

#[test]
fn tasks_command_reports_recorded_runtime_tasks() {
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
    state.record_task("bash", "printf hi", true);

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/tasks",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("bash") && text.contains("completed")
    ));
}

#[test]
fn tasks_command_reports_workflow_tasks_and_todos() {
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

    let cwd = state.cwd.clone();
    crate::runtime::claude_tools::workflow::task_create::execute_task_create(
        &mut state,
        &cwd,
        serde_json::json!({
            "subject": "Audit slash command parity",
            "description": "Check missing task surfaces"
        }),
    )
    .unwrap();
    crate::runtime::claude_tools::workflow::todo_write::execute_todo_write(
        &mut state,
        &cwd,
        serde_json::json!({
            "todos": [
                {
                    "content": "Wire /tasks to workflow state",
                    "status": "in_progress",
                    "activeForm": "Wiring /tasks to workflow state"
                }
            ]
        }),
    )
    .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/tasks",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Task list:")
            && text.contains("Audit slash command parity")
            && text.contains("Todos:")
            && text.contains("Wire /tasks to workflow state")
    ));
}

#[test]
fn tasks_command_reports_background_agents_and_teams() {
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

    let cwd = state.cwd.clone();
    crate::runtime::claude_tools::workflow::agent::execute_agent(
        &mut state,
        &cwd,
        serde_json::json!({
            "description": "Review pending changes",
            "prompt": "Inspect the branch",
            "name": "reviewer",
            "subagent_type": "general-purpose",
            "run_in_background": true
        }),
    )
    .unwrap();
    crate::runtime::claude_tools::workflow::team_create::execute_team_create(
        &mut state,
        &cwd,
        serde_json::json!({
            "team_name": "alpha",
            "description": "Review team",
            "agent_type": "general-purpose"
        }),
    )
    .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/tasks agents",
    )
    .unwrap();
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Background agents:") && text.contains("reviewer")
    ));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/tasks teams",
    )
    .unwrap();
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Teams:") && text.contains("alpha")
    ));
}

#[test]
fn task_actions_do_not_offer_stop_for_plain_workflow_tasks() {
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

    let cwd = state.cwd.clone();
    crate::runtime::claude_tools::workflow::task_create::execute_task_create(
        &mut state,
        &cwd,
        serde_json::json!({
            "subject": "Audit parity",
            "description": "Inspect command differences"
        }),
    )
    .unwrap();

    let actions = crate::render_task_actions(&mut state).unwrap();
    assert!(!actions
        .iter()
        .any(|entry| entry.command.starts_with("/tasks stop ")));
}

#[test]
fn tasks_command_can_show_task_details_after_claude_style_tool_refactor() {
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

    let cwd = state.cwd.clone();
    crate::runtime::claude_tools::workflow::task_create::execute_task_create(
        &mut state,
        &cwd,
        serde_json::json!({
            "subject": "Audit slash command parity",
            "description": "Check the task detail view"
        }),
    )
    .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/tasks show task-1",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Task task-1")
            && text.contains("subject=Audit slash command parity")
            && text.contains("description=Check the task detail view")
    ));
}
