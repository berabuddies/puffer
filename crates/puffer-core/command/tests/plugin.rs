use super::*;
use puffer_resources::{LoadedItem, PluginSpec, SourceInfo, SourceKind};
use std::fs;

#[test]
fn plugin_command_creates_workspace_plugin_file() {
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
        "/plugin",
    )
    .unwrap();

    let plugin_path = paths
        .workspace_config_dir
        .join("resources/plugins/workspace.yaml");
    assert!(plugin_path.exists());
}

#[test]
fn plugin_disable_and_enable_commands_toggle_workspace_override() {
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
    let mut resources = LoadedResources::default();
    resources.plugins.push(LoadedItem {
        value: PluginSpec {
            id: "docs".to_string(),
            display_name: "Docs".to_string(),
            description: "Builtin docs helpers".to_string(),
            commands: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            mcp_servers: Vec::new(),
            lsp_servers: Vec::new(),
        },
        source_info: SourceInfo {
            path: paths.builtin_resources_dir.join("plugins/docs.yaml"),
            kind: SourceKind::Builtin,
        },
    });

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin disable docs",
    )
    .unwrap();

    let workspace_override = paths
        .workspace_config_dir
        .join("resources/plugins/docs.yaml");
    assert!(workspace_override.exists());
    assert!(state.reload_resources_requested);
    let disabled: PluginSpec =
        serde_yaml::from_str(&std::fs::read_to_string(&workspace_override).unwrap()).unwrap();
    assert!(disabled.description.contains("Disabled plugin placeholder"));
    assert!(disabled.commands.is_empty());

    state.reload_resources_requested = false;
    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin enable docs",
    )
    .unwrap();

    assert!(!workspace_override.exists());
    assert!(state.reload_resources_requested);
}

#[test]
fn plugin_validate_reports_duplicate_entries() {
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
    let mut resources = LoadedResources::default();
    resources.plugins.push(LoadedItem {
        value: PluginSpec {
            id: "docs".to_string(),
            display_name: "Docs".to_string(),
            description: "Builtin docs helpers".to_string(),
            commands: vec![
                puffer_resources::PluginCommandSpec {
                    name: "search".to_string(),
                    description: String::new(),
                },
                puffer_resources::PluginCommandSpec {
                    name: "search".to_string(),
                    description: String::new(),
                },
            ],
            skills: vec!["reviewer".to_string(), "reviewer".to_string()],
            agents: Vec::new(),
            mcp_servers: Vec::new(),
            lsp_servers: Vec::new(),
        },
        source_info: SourceInfo {
            path: paths.builtin_resources_dir.join("plugins/docs.yaml"),
            kind: SourceKind::Builtin,
        },
    });

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin validate docs",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("duplicate command `search`") && text.contains("duplicate skill `reviewer`")
    ));
}

#[test]
fn plugin_errors_filters_resource_diagnostics() {
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
    let mut resources = LoadedResources::default();
    resources
        .diagnostics
        .push("workspace plugin `docs` from /tmp/plugins/docs.yaml overrides builtin resource from /builtin/plugins/docs.yaml".to_string());
    resources
        .diagnostics
        .push("workspace prompt `review` from /tmp/prompts/review.yaml overrides builtin resource from /builtin/prompts/review.yaml".to_string());

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin errors",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("errors=1") && text.contains("plugin `docs`") && !text.contains("prompt `review`")
    ));
}

#[test]
fn plugin_marketplace_lists_builtin_plugins() {
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
    let mut resources = LoadedResources::default();
    let builtin_path = paths.builtin_resources_dir.join("plugins/docs.yaml");
    fs::create_dir_all(builtin_path.parent().unwrap()).unwrap();
    fs::write(
        &builtin_path,
        "id: docs\ndisplay_name: Docs\ndescription: Builtin docs helpers\n",
    )
    .unwrap();
    resources.plugins.push(LoadedItem {
        value: PluginSpec {
            id: "docs".to_string(),
            display_name: "Docs".to_string(),
            description: "Builtin docs helpers".to_string(),
            commands: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            mcp_servers: Vec::new(),
            lsp_servers: Vec::new(),
        },
        source_info: SourceInfo {
            path: builtin_path,
            kind: SourceKind::Builtin,
        },
    });

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin marketplace",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Plugin marketplace") && text.contains("docs") && text.contains("Builtin docs helpers")
    ));
}

fn plugin_validate_accepts_manifest_file_path() {
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
    let manifest_path = tempdir.path().join("resources/plugins/docs.yaml");
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    fs::write(
        &manifest_path,
        "id: docs\ndisplay_name: Docs\ndescription: Builtin docs helpers\n",
    )
    .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin validate resources/plugins/docs.yaml",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("validated_manifests=1")
            && text.contains("- docs [ok]")
            && text.contains("resources/plugins/docs.yaml")
    ));
}

#[test]
fn plugin_validate_accepts_directory_path() {
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
    let manifest_path = tempdir.path().join("bundle/resources/plugins/docs.yaml");
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    fs::write(
        &manifest_path,
        "id: docs\ndisplay_name: Docs\ndescription: Builtin docs helpers\n",
    )
    .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin validate bundle",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("validated_manifests=1")
            && text.contains("target=")
            && text.contains("bundle/resources/plugins/docs.yaml")
    ));
}

#[test]
fn plugin_validate_reports_manifest_parse_errors_for_paths() {
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
    let manifest_path = tempdir.path().join("resources/plugins/bad.yaml");
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    fs::write(&manifest_path, "id: [broken\n").unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin validate resources/plugins/bad.yaml",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("- bad [issues]")
            && text.contains("failed to parse manifest")
    ));
}

#[test]
fn plugin_install_update_and_uninstall_manage_workspace_copy() {
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
    let mut resources = LoadedResources::default();
    let builtin_path = paths.builtin_resources_dir.join("plugins/docs.yaml");
    fs::create_dir_all(builtin_path.parent().unwrap()).unwrap();
    fs::write(
        &builtin_path,
        "id: docs\ndisplay_name: Docs\ndescription: Builtin docs helpers\n",
    )
    .unwrap();
    resources.plugins.push(LoadedItem {
        value: PluginSpec {
            id: "docs".to_string(),
            display_name: "Docs".to_string(),
            description: "Builtin docs helpers".to_string(),
            commands: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            mcp_servers: Vec::new(),
            lsp_servers: Vec::new(),
        },
        source_info: SourceInfo {
            path: builtin_path.clone(),
            kind: SourceKind::Builtin,
        },
    });

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin install docs",
    )
    .unwrap();

    let workspace_copy = paths
        .workspace_config_dir
        .join("resources/plugins/docs.yaml");
    assert!(workspace_copy.exists());
    assert!(state.reload_resources_requested);
    assert!(fs::read_to_string(&workspace_copy)
        .unwrap()
        .contains("Builtin docs helpers"));

    state.reload_resources_requested = false;
    fs::write(
        &builtin_path,
        "id: docs\ndisplay_name: Docs\ndescription: Refreshed builtin docs helpers\n",
    )
    .unwrap();
    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin update docs",
    )
    .unwrap();

    assert!(state.reload_resources_requested);
    assert!(fs::read_to_string(&workspace_copy)
        .unwrap()
        .contains("Refreshed builtin docs helpers"));

    state.reload_resources_requested = false;
    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin uninstall docs",
    )
    .unwrap();

    assert!(!workspace_copy.exists());
    assert!(state.reload_resources_requested);
}

#[test]
fn plugin_marketplace_add_install_update_and_remove_custom_marketplace() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut state = AppState::new(PufferConfig::default(), workspace.clone(), session);
    let resources = LoadedResources::default();
    let marketplace_path = workspace.join("vendor/extras/marketplace.yaml");
    fs::create_dir_all(marketplace_path.parent().unwrap()).unwrap();
    fs::write(
        &marketplace_path,
        "name: extras\nowner:\n  name: Example Inc\nmetadata:\n  description: Shared team plugins\nplugins:\n  - name: docs-helper\n    display_name: Docs Helper\n    description: Initial marketplace docs helper\n    commands:\n      - name: docs-search\n        description: Search docs\n",
    )
    .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        &format!("/plugin marketplace add {}", marketplace_path.display()),
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Added plugin marketplace `extras`")
    ));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin marketplace",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("custom_marketplaces=1")
            && text.contains("docs-helper@extras")
            && text.contains("Shared team plugins")
    ));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin install docs-helper@extras",
    )
    .unwrap();

    let workspace_copy = paths
        .workspace_config_dir
        .join("resources/plugins/docs-helper.yaml");
    assert!(workspace_copy.exists());
    assert!(fs::read_to_string(&workspace_copy)
        .unwrap()
        .contains("Initial marketplace docs helper"));

    fs::write(
        &marketplace_path,
        "name: extras\nowner:\n  name: Example Inc\nmetadata:\n  description: Shared team plugins\nplugins:\n  - name: docs-helper\n    display_name: Docs Helper\n    description: Refreshed marketplace docs helper\n    commands:\n      - name: docs-search\n        description: Search docs\n      - name: docs-index\n        description: Rebuild docs index\n",
    )
    .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin marketplace update extras",
    )
    .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin update docs-helper@extras",
    )
    .unwrap();

    let updated = fs::read_to_string(&workspace_copy).unwrap();
    assert!(updated.contains("Refreshed marketplace docs helper"));
    assert!(updated.contains("docs-index"));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin marketplace remove extras",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Removed plugin marketplace `extras`")
    ));
}

#[test]
fn plugin_marketplace_install_reads_external_manifest_source() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut state = AppState::new(PufferConfig::default(), workspace.clone(), session);
    let resources = LoadedResources::default();
    let marketplace_dir = workspace.join("vendor/external");
    let marketplace_path = marketplace_dir.join("marketplace.yaml");
    let plugin_path = marketplace_dir.join("plugins/docs-helper.yaml");
    fs::create_dir_all(plugin_path.parent().unwrap()).unwrap();
    fs::write(
        &marketplace_path,
        "name: external\nowner:\n  name: Example Inc\nplugins:\n  - name: docs-helper\n    description: External manifest source\n    source: ./plugins/docs-helper.yaml\n",
    )
    .unwrap();
    fs::write(
        &plugin_path,
        "id: docs-helper\ndisplay_name: Docs Helper\ndescription: Loaded from external plugin manifest\nskills:\n  - reviewer\n",
    )
    .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        &format!("/plugin marketplace add {}", marketplace_path.display()),
    )
    .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plugin install docs-helper@external",
    )
    .unwrap();

    let workspace_copy = paths
        .workspace_config_dir
        .join("resources/plugins/docs-helper.yaml");
    let installed = fs::read_to_string(&workspace_copy).unwrap();
    assert!(installed.contains("Loaded from external plugin manifest"));
    assert!(installed.contains("reviewer"));
}
