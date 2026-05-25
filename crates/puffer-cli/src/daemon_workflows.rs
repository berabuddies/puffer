//! Workflow daemon RPC helpers.

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{
    connection_workflow_trigger_supported, connector_runtime_hints,
    connector_workflow_trigger_supported, suggested_connection_slug, ConnectionRecord,
    SubscriberManifestRoots,
};
use puffer_workflow::{RegisterOptions, WorkflowStore};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::fs;

/// Returns the workflow editor snapshot with connector catalog context.
pub(crate) fn handle_workflow_list(paths: &ConfigPaths) -> Result<Value> {
    let store = WorkflowStore::new(&paths.workspace_config_dir);
    let mut snapshot = serde_json::to_value(store.snapshot()?)?;
    add_connector_context(paths, &mut snapshot);
    add_monitor_task_context(paths, &mut snapshot);
    Ok(snapshot)
}

/// Saves one workflow definition and returns the refreshed editor snapshot.
pub(crate) fn handle_workflow_save(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let workflow = params
        .get("workflow")
        .or_else(|| params.get("definition"))
        .cloned()
        .context("missing workflow")?;
    let store = WorkflowStore::new(&paths.workspace_config_dir);
    store.register_json(workflow, RegisterOptions::default())?;
    let mut snapshot = serde_json::to_value(store.snapshot()?)?;
    add_connector_context(paths, &mut snapshot);
    add_monitor_task_context(paths, &mut snapshot);
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
                        "runtime_hints": connector_runtime_hints(&roots, &template),
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

fn add_monitor_task_context(paths: &ConfigPaths, snapshot: &mut Value) {
    let Some(object) = snapshot.as_object_mut() else {
        return;
    };
    match load_monitor_tasks(paths) {
        Ok(tasks) => {
            object.insert("monitor_tasks".to_string(), Value::Array(tasks));
            object.insert("monitor_task_error".to_string(), Value::Null);
        }
        Err(error) => {
            object.insert("monitor_tasks".to_string(), Value::Array(Vec::new()));
            object.insert(
                "monitor_task_error".to_string(),
                Value::String(error.to_string()),
            );
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct MonitorTaskStoreSnapshot {
    #[serde(default)]
    tasks: Vec<MonitorTaskSnapshotRecord>,
}

#[derive(Debug, Deserialize)]
struct MonitorTaskSnapshotRecord {
    #[serde(alias = "id", alias = "taskId")]
    task_id: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    metadata: Map<String, Value>,
    #[serde(default, alias = "startedAtMs")]
    started_at_ms: Option<u64>,
    #[serde(default, alias = "updatedAtMs")]
    updated_at_ms: Option<u64>,
}

fn load_monitor_tasks(paths: &ConfigPaths) -> Result<Vec<Value>> {
    let path = monitor_tasks_path(paths);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let store: MonitorTaskStoreSnapshot = serde_json::from_str(&raw)
        .with_context(|| format!("invalid monitor task store {}", path.display()))?;
    Ok(store.tasks.into_iter().map(monitor_task_json).collect())
}

fn monitor_tasks_path(paths: &ConfigPaths) -> std::path::PathBuf {
    paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
        .join("monitor_tasks.json")
}

fn monitor_task_json(task: MonitorTaskSnapshotRecord) -> Value {
    json!({
        "task_id": task.task_id,
        "subject": task.subject,
        "description": task.description,
        "status": task.status,
        "monitor_connection": monitor_metadata_string(
            &task.metadata,
            &["monitor_connection", "monitorConnection"],
            &["connection", "connection_slug", "connectionSlug"]
        ),
        "monitor_connector": monitor_metadata_string(
            &task.metadata,
            &["monitor_connector", "monitorConnector"],
            &["connector", "connector_slug", "connectorSlug"]
        ),
        "monitor_memory_path": monitor_metadata_string(
            &task.metadata,
            &["monitor_memory_path", "monitorMemoryPath"],
            &["memory_path", "memoryPath"]
        ),
        "ignored": monitor_metadata_bool(&task.metadata, "ignored"),
        "actions": monitor_actions(&task.metadata),
        "possible_ignore_reasons": monitor_ignore_reasons(&task.metadata),
        "started_at_ms": task.started_at_ms,
        "updated_at_ms": task.updated_at_ms,
    })
}

fn monitor_metadata_string(
    metadata: &Map<String, Value>,
    top_level_keys: &[&str],
    monitor_keys: &[&str],
) -> Option<String> {
    top_level_keys
        .iter()
        .find_map(|key| string_value(metadata.get(*key)))
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(Value::as_object)
                .and_then(|monitor| {
                    monitor_keys
                        .iter()
                        .find_map(|key| string_value(monitor.get(*key)))
                })
        })
}

fn monitor_metadata_bool(metadata: &Map<String, Value>, key: &str) -> bool {
    metadata.get(key).and_then(Value::as_bool).unwrap_or(false)
        || metadata
            .get("monitor")
            .and_then(Value::as_object)
            .and_then(|monitor| monitor.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn monitor_actions(metadata: &Map<String, Value>) -> Vec<Value> {
    metadata_value_array(metadata, "actions")
        .map(|actions| {
            actions
                .iter()
                .filter_map(|action| {
                    let name = string_field(action, &["actionName", "name", "title"])?;
                    let prompt = string_field(action, &["actionPrompt", "prompt"])?;
                    Some(json!({
                        "name": name,
                        "prompt": prompt,
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn monitor_ignore_reasons(metadata: &Map<String, Value>) -> Vec<Value> {
    metadata_value_array(metadata, "possibleIgnoreReasons")
        .or_else(|| metadata_value_array(metadata, "possible_ignore_reasons"))
        .map(|reasons| {
            reasons
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .map(|reason| Value::String(reason.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn metadata_value_array<'a>(metadata: &'a Map<String, Value>, key: &str) -> Option<&'a Vec<Value>> {
    metadata
        .get(key)
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(Value::as_object)
                .and_then(|monitor| monitor.get(key))
        })
        .and_then(Value::as_array)
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    keys.iter().find_map(|key| string_value(object.get(*key)))
}

fn string_value(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
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

    #[test]
    fn workflow_snapshot_includes_monitor_tasks() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let task_path = monitor_tasks_path(&paths);
        std::fs::create_dir_all(task_path.parent().unwrap()).unwrap();
        std::fs::write(
            &task_path,
            serde_json::to_string_pretty(&json!({
                "tasks": [
                    {
                        "task_id": "monitor-1",
                        "subject": "Answer Telegram support ping",
                        "description": "Alice asked whether the deployment is finished.",
                        "status": "pending",
                        "metadata": {
                            "_monitor": true,
                            "monitor_connection": "telegram-user",
                            "monitor_connector": "telegram-login",
                            "monitor_memory_path": "/tmp/telegram-user.md",
                            "actions": [
                                {
                                    "actionName": "Draft reply",
                                    "actionPrompt": "Draft a concise reply to Alice."
                                }
                            ],
                            "possibleIgnoreReasons": ["duplicate support ping"]
                        },
                        "started_at_ms": 10,
                        "updated_at_ms": 20
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let snapshot = handle_workflow_list(&paths).unwrap();
        let tasks = snapshot["monitor_tasks"].as_array().unwrap();

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task_id"], "monitor-1");
        assert_eq!(tasks[0]["monitor_connection"], "telegram-user");
        assert_eq!(tasks[0]["monitor_connector"], "telegram-login");
        assert_eq!(tasks[0]["monitor_memory_path"], "/tmp/telegram-user.md");
        assert_eq!(tasks[0]["actions"][0]["name"], "Draft reply");
        assert_eq!(
            tasks[0]["actions"][0]["prompt"],
            "Draft a concise reply to Alice."
        );
        assert_eq!(
            tasks[0]["possible_ignore_reasons"][0],
            "duplicate support ping"
        );
        assert_eq!(snapshot["monitor_task_error"], Value::Null);
    }

    #[test]
    fn workflow_snapshot_tolerates_missing_monitor_task_store() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());

        let snapshot = handle_workflow_list(&paths).unwrap();

        assert_eq!(snapshot["monitor_tasks"].as_array().unwrap().len(), 0);
        assert_eq!(snapshot["monitor_task_error"], Value::Null);
    }

    #[test]
    fn workflow_save_upserts_definition_and_returns_snapshot() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());

        let snapshot = handle_workflow_save(
            &paths,
            &json!({
                "workflow": {
                    "schema": "puffer.workflow.v1",
                    "slug": "saved-pipeline",
                    "enabled": true,
                    "trigger": {"type": "connection", "connection_slug": "telegram-user", "pattern": "hi"},
                    "pipeline": {
                        "name": "Saved pipeline",
                        "nodes": [
                            {"id": "reply", "type": "puffer", "prompt": "Draft a reply."}
                        ]
                    }
                }
            }),
        )
        .unwrap();

        assert_eq!(snapshot["workflows"].as_array().unwrap().len(), 1);
        assert_eq!(snapshot["workflows"][0]["slug"], "saved-pipeline");
        assert_eq!(
            snapshot["workflows"][0]["pipeline"]["name"],
            "Saved pipeline"
        );

        let listed = handle_workflow_list(&paths).unwrap();
        assert_eq!(listed["workflows"][0]["slug"], "saved-pipeline");
    }
}
