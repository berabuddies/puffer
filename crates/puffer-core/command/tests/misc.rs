use super::*;

#[test]
fn cost_command_reports_runtime_summary() {
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
    state.push_message(MessageRole::User, "review this");
    state.push_message(MessageRole::Assistant, "done");
    state.push_message(MessageRole::System, "Tool bash [ok]\ninput: printf hi\nhi");
    state.record_task("bash", "printf hi", true);

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/cost",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("assistant_messages=1")
            && text.contains("tool_invocations=1")
            && text.contains("recorded_tasks=1")
    ));
}

#[test]
fn reload_plugins_reports_resource_counts() {
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
    let resources = LoadedResources {
        plugins: vec![LoadedItem {
            value: puffer_resources::PluginSpec {
                id: "git".to_string(),
                display_name: "Git".to_string(),
                description: "Git helpers".to_string(),
                commands: Vec::new(),
                skills: Vec::new(),
                agents: Vec::new(),
                mcp_servers: Vec::new(),
                lsp_servers: Vec::new(),
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("plugins/git.yaml"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        skills: vec![LoadedItem {
            value: puffer_resources::SkillSpec {
                name: "reviewer".to_string(),
                description: "review".to_string(),
                content: "review".to_string(),
                disable_model_invocation: false,
                ..puffer_resources::SkillSpec::default()
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("skills/reviewer.md"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        mcp_servers: vec![LoadedItem {
            value: puffer_resources::McpServerSpec {
                id: "docs".to_string(),
                display_name: "Docs".to_string(),
                transport: "stdio".to_string(),
                endpoint: String::new(),
                target: "docs".to_string(),
                description: "docs".to_string(),
                headers: Default::default(),
                oauth: None,
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("mcp/docs.yaml"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        ..LoadedResources::default()
    };

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/reload-plugins",
    )
    .unwrap();

    assert!(state.reload_resources_requested);
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Reloading plugin changes from disk")
    ));
}
