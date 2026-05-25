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
fn workflows_connectors_catalog_includes_jira_webhook_preset() {
    let text = connector_search_output("jira issue comment");

    assert!(text.contains("showing 1/16 connectors for query=\"jira issue comment\""));
    assert!(text.contains("jira-webhook"));
    assert!(text.contains("connect=/connect jira-webhook jira-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_stripe_webhook_preset() {
    let text = connector_search_output("stripe invoice payment");

    assert!(text.contains("showing 1/16 connectors for query=\"stripe invoice payment\""));
    assert!(text.contains("stripe-webhook"));
    assert!(text.contains("connect=/connect stripe-webhook stripe-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}
