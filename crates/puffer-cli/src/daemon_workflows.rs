//! Workflow daemon RPC helpers.

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{
    connection_workflow_trigger_supported, connector_workflow_trigger_supported,
    suggested_connection_slug, ConnectionRecord, SubscriberManifestRoots,
};
use puffer_workflow::WorkflowStore;
use serde_json::{json, Value};

/// Returns the workflow editor snapshot with connector catalog context.
pub(crate) fn handle_workflow_list(paths: &ConfigPaths) -> Result<Value> {
    let store = WorkflowStore::new(&paths.workspace_config_dir);
    let mut snapshot = serde_json::to_value(store.snapshot()?)?;
    add_connector_context(paths, &mut snapshot);
    Ok(snapshot)
}

/// Returns the persisted runs for one workflow slug.
pub(crate) fn handle_workflow_runs_list(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let slug = params
        .get("workflowSlug")
        .or_else(|| params.get("workflow_slug"))
        .and_then(Value::as_str)
        .context("missing workflowSlug")?;
    let store = WorkflowStore::new(&paths.workspace_config_dir);
    Ok(serde_json::to_value(store.list_runs_for(slug)?)?)
}

/// Returns one persisted workflow run by global index.
pub(crate) fn handle_workflow_run_show(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let idx = params
        .get("idx")
        .and_then(Value::as_u64)
        .context("missing idx")?;
    let store = WorkflowStore::new(&paths.workspace_config_dir);
    Ok(serde_json::to_value(store.get_run(idx)?)?)
}

fn add_connector_context(paths: &ConfigPaths, snapshot: &mut Value) {
    let Some(object) = snapshot.as_object_mut() else {
        return;
    };
    let (connectors, connections, connector_error) = match subscription_manager() {
        Ok(manager) => {
            let roots = subscriber_manifest_roots(paths);
            let refresh_error = manager
                .refresh_connection_consumers()
                .err()
                .map(|error| error.to_string());
            let connectors = manager
                .connector_store()
                .list_with_builtins()
                .into_iter()
                .map(|template| {
                    let action_slugs = template.actions.keys().cloned().collect::<Vec<_>>();
                    let slug = template.slug.clone();
                    let suggested_connection = suggested_connection_slug(&slug);
                    let connect_command = format!("/connect {slug} {suggested_connection}");
                    let can_trigger_workflow =
                        connector_workflow_trigger_supported(&roots, &template);
                    json!({
                        "connector_slug": slug,
                        "description": template.description,
                        "skill": template.skill,
                        "requires_auth": template.requires_auth,
                        "can_subscribe": template.can_subscribe,
                        "can_proxy_agent": template.can_proxy_agent,
                        "suggested_connection_slug": suggested_connection,
                        "connect_command": connect_command,
                        "can_trigger_workflow": can_trigger_workflow,
                        "action_slugs": action_slugs,
                    })
                })
                .collect::<Vec<_>>();
            let connections = manager
                .connection_store()
                .list()
                .into_iter()
                .map(|connection| {
                    let can_trigger_workflow = manager
                        .connector_store()
                        .get(&connection.connector_slug)
                        .is_some_and(|template| {
                            connection_workflow_trigger_supported(&roots, &connection, &template)
                        });
                    connection_snapshot_json(connection, can_trigger_workflow)
                })
                .collect::<Vec<_>>();
            (connectors, connections, refresh_error)
        }
        Err(error) => (Vec::new(), Vec::new(), Some(error.to_string())),
    };
    object.insert("connectors".to_string(), Value::Array(connectors));
    object.insert("connections".to_string(), Value::Array(connections));
    object.insert(
        "connector_error".to_string(),
        connector_error.map(Value::String).unwrap_or(Value::Null),
    );
}

fn connection_snapshot_json(connection: ConnectionRecord, can_trigger_workflow: bool) -> Value {
    let monitor_command = can_trigger_workflow.then(|| format!("/monitor {}", connection.slug));
    let connect_command = format!("/connect {} {}", connection.connector_slug, connection.slug);
    json!({
        "slug": connection.slug,
        "connector_slug": connection.connector_slug,
        "description": connection.description,
        "state": connection.state,
        "has_consumer": connection.has_consumer,
        "auth_failure_notified": connection.auth_failure_notified,
        "can_trigger_workflow": can_trigger_workflow,
        "connect_command": connect_command,
        "monitor_command": monitor_command,
    })
}

fn subscriber_manifest_roots(paths: &ConfigPaths) -> SubscriberManifestRoots {
    SubscriberManifestRoots::new(
        paths.workspace_config_dir.clone(),
        paths.user_config_dir.clone(),
        paths.builtin_resources_dir.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_ready_connection_snapshot_includes_monitor_command() {
        let connection =
            ConnectionRecord::authenticated("telegram-user", "telegram-login", "Personal Telegram");

        let snapshot = connection_snapshot_json(connection, true);

        assert_eq!(
            snapshot["connect_command"],
            "/connect telegram-login telegram-user"
        );
        assert_eq!(snapshot["monitor_command"], "/monitor telegram-user");
        assert_eq!(snapshot["can_trigger_workflow"], true);
    }

    #[test]
    fn non_trigger_connection_snapshot_omits_monitor_command() {
        let connection = ConnectionRecord::authenticated("slack-app", "slack-app", "Slack");

        let snapshot = connection_snapshot_json(connection, false);

        assert_eq!(snapshot["connect_command"], "/connect slack-app slack-app");
        assert!(snapshot["monitor_command"].is_null());
        assert_eq!(snapshot["can_trigger_workflow"], false);
    }
}
