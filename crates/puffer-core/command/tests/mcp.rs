use super::*;
use puffer_resources::{LoadedItem, McpServerSpec, SourceInfo, SourceKind};

#[test]
fn mcp_command_creates_workspace_mcp_file() {
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
        "/mcp",
    )
    .unwrap();

    let mcp_path = paths
        .workspace_config_dir
        .join("resources/mcp_servers/workspace.yaml");
    assert!(mcp_path.exists());
}

#[test]
fn mcp_enable_and_disable_request_runtime_reload() {
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
        mcp_servers: vec![LoadedItem {
            value: McpServerSpec {
                id: "docs".to_string(),
                display_name: "Docs".to_string(),
                transport: "stdio".to_string(),
                endpoint: String::new(),
                target: "docs".to_string(),
                description: String::new(),
                headers: Default::default(),
                oauth: None,
            },
            source_info: SourceInfo {
                path: paths.builtin_resources_dir.join("mcp_servers/docs.yaml"),
                kind: SourceKind::Builtin,
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
        "/mcp disable docs",
    )
    .unwrap();

    let state_path = paths.workspace_config_dir.join("mcp_servers.toml");
    assert!(state.reload_resources_requested);
    let raw = std::fs::read_to_string(&state_path).unwrap();
    assert!(raw.contains("docs"));

    state.reload_resources_requested = false;
    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/mcp enable docs",
    )
    .unwrap();

    assert!(state.reload_resources_requested);
    let raw = std::fs::read_to_string(&state_path).unwrap();
    assert!(!raw.contains("docs"));
}
