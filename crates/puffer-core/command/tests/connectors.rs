use super::*;

fn connector_search_output(query: &str) -> String {
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
        &format!("/workflows connectors {query}"),
    )
    .unwrap();

    state.transcript.last().unwrap().text.clone()
}

#[test]
fn workflows_connectors_catalog_lists_current_builtin_connectors() {
    let text = connector_search_output("");

    assert!(text.contains("showing 11/11 connectors"));
    assert!(text.contains("telegram-login"));
    assert!(text.contains("slack-login"));
    assert!(text.contains("email"));
}

#[test]
fn workflows_connectors_catalog_includes_serve_bots_as_non_triggers() {
    let text = connector_search_output("serve");

    assert!(text.contains("showing 3/11 connectors for query=\"serve\""));
    assert!(text.contains("telegram-bot"));
    assert!(text.contains("discord-bot"));
    assert!(text.contains("matrix-bot"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[auth,no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_has_no_http_ingress_presets() {
    let text = connector_search_output("github");

    assert!(text.contains("showing 0/11 connectors for query=\"github\""));
    assert!(!text.contains("github-"));
    assert!(!text.contains("runtime=serve"));
}
