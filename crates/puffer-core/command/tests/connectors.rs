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

    assert!(text.contains("showing 1/23 connectors for query=\"jira issue comment\""));
    assert!(text.contains("jira-webhook"));
    assert!(text.contains("connect=/connect jira-webhook jira-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_asana_webhook_preset() {
    let text = connector_search_output("asana task project");

    assert!(text.contains("showing 1/23 connectors for query=\"asana task project\""));
    assert!(text.contains("asana-webhook"));
    assert!(text.contains("connect=/connect asana-webhook asana-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_alertmanager_webhook_preset() {
    let text = connector_search_output("prometheus alertmanager");

    assert!(text.contains("showing 1/23 connectors for query=\"prometheus alertmanager\""));
    assert!(text.contains("alertmanager-webhook"));
    assert!(text.contains("connect=/connect alertmanager-webhook alertmanager-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_grafana_webhook_preset() {
    let text = connector_search_output("grafana alerting");

    assert!(text.contains("showing 1/23 connectors for query=\"grafana alerting\""));
    assert!(text.contains("grafana-webhook"));
    assert!(text.contains("connect=/connect grafana-webhook grafana-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_stripe_webhook_preset() {
    let text = connector_search_output("stripe invoice payment");

    assert!(text.contains("showing 1/23 connectors for query=\"stripe invoice payment\""));
    assert!(text.contains("stripe-webhook"));
    assert!(text.contains("connect=/connect stripe-webhook stripe-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_trello_webhook_preset() {
    let text = connector_search_output("trello board card");

    assert!(text.contains("showing 1/23 connectors for query=\"trello board card\""));
    assert!(text.contains("trello-webhook"));
    assert!(text.contains("connect=/connect trello-webhook trello-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_shopify_webhook_preset() {
    let text = connector_search_output("shopify order product");

    assert!(text.contains("showing 1/23 connectors for query=\"shopify order product\""));
    assert!(text.contains("shopify-webhook"));
    assert!(text.contains("connect=/connect shopify-webhook shopify-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_pagerduty_webhook_preset() {
    let text = connector_search_output("pagerduty incident");

    assert!(text.contains("showing 1/23 connectors for query=\"pagerduty incident\""));
    assert!(text.contains("pagerduty-webhook"));
    assert!(text.contains("connect=/connect pagerduty-webhook pagerduty-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_sentry_webhook_preset() {
    let text = connector_search_output("sentry issue alert");

    assert!(text.contains("showing 1/23 connectors for query=\"sentry issue alert\""));
    assert!(text.contains("sentry-webhook"));
    assert!(text.contains("connect=/connect sentry-webhook sentry-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}
