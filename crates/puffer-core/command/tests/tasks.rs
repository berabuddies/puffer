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
    assert!(!actions
        .iter()
        .any(|entry| entry.command.starts_with("/tasks ignore ")));
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

#[test]
fn monitor_tasks_offer_actions_and_ignore_choices() {
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
    let memory_path = paths
        .workspace_config_dir
        .join("runtime")
        .join("monitors")
        .join("telegram-user.md");

    let cwd = state.cwd.clone();
    crate::runtime::claude_tools::workflow::task_create::execute_task_create(
        &mut state,
        &cwd,
        serde_json::json!({
            "subject": "Answer Telegram support ping",
            "description": "Alice asked whether the deployment is finished.",
            "actions": [
                {
                    "actionName": "Draft reply",
                    "actionPrompt": "Draft a concise reply to Alice with the deployment status."
                }
            ],
            "possibleIgnoreReasons": ["duplicate support ping"],
            "metadata": {
                "_monitor": true,
                "monitor_connection": "telegram-user",
                "monitor_connector": "telegram-login",
                "monitor_memory_path": memory_path.display().to_string()
            }
        }),
    )
    .unwrap();

    let actions = crate::render_task_actions(&mut state).unwrap();
    assert!(actions
        .iter()
        .any(|entry| entry.command == "/tasks ignore monitor-1 duplicate support ping"));
    assert!(actions.iter().any(|entry| {
        entry
            .command
            .starts_with("Act on monitored task monitor-1:")
            && entry.command.contains("Draft a concise reply to Alice")
    }));

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
        }) if text.contains("Answer Telegram support ping")
            && text.contains("[Ignore]")
            && text.contains("[Ignore: duplicate support ping]")
            && text.contains("[Draft reply]")
    ));
}

#[test]
fn tasks_ignore_marks_monitor_task_and_hides_actions() {
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
    let memory_path = paths
        .workspace_config_dir
        .join("runtime")
        .join("monitors")
        .join("telegram-user.md");

    let cwd = state.cwd.clone();
    crate::runtime::claude_tools::workflow::task_create::execute_task_create(
        &mut state,
        &cwd,
        serde_json::json!({
            "subject": "Ignore noisy Telegram ping",
            "description": "A duplicate deployment reminder appeared.",
            "actions": [
                {
                    "actionName": "Open chat",
                    "actionPrompt": "Open the chat and inspect the latest reminder."
                }
            ],
            "possibleIgnoreReasons": ["duplicate reminder"],
            "metadata": {
                "_monitor": true,
                "monitor_connection": "telegram-user",
                "monitor_connector": "telegram-login",
                "monitor_memory_path": memory_path.display().to_string()
            }
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
        "/tasks ignore monitor-1 duplicate reminder",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Ignored task")
            && text.contains("task_id=monitor-1")
            && text.contains("reason=duplicate reminder")
    ));
    let memory = std::fs::read_to_string(&memory_path).unwrap();
    assert!(memory.contains("Ignored Task: monitor-1"));
    assert!(memory.contains("Reason: duplicate reminder"));
    let monitor_tasks_path = paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
        .join("monitor_tasks.json");
    let monitor_store: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(monitor_tasks_path).unwrap()).unwrap();
    assert_eq!(
        monitor_store["tasks"][0]["metadata"]["ignored"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        monitor_store["tasks"][0]["metadata"]["ignore_reason"],
        serde_json::Value::String("duplicate reminder".to_string())
    );
    let actions = crate::render_task_actions(&mut state).unwrap();
    assert!(!actions
        .iter()
        .any(|entry| entry.command.contains("monitor-1")));

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
        }) if !text.contains("Ignore noisy Telegram ping")
            && text.contains("structured_tasks=0")
    ));
}
