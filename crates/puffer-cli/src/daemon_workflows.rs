//! Workflow daemon RPC helpers.

mod binding_create;
mod binding_delete;
#[cfg(test)]
mod binding_snapshot_tests;
mod connection_delete;
mod monitor_create;
mod monitor_history;
mod monitor_ignore_result;
mod monitor_memory;
mod monitor_rules;
mod monitor_self_gate;
#[cfg(test)]
mod monitor_snapshot_tests;
mod monitor_task_complete;
mod monitor_task_ignore;
mod monitor_trace;
mod outbound_action;
mod planned;
mod runtime;
mod snapshot_json;
mod task_snapshot;
mod telegram_diagnostics;
#[cfg(test)]
mod telegram_diagnostics_export_tests;

pub(crate) use binding_create::handle_workflow_binding_create;
pub(crate) use binding_delete::handle_workflow_binding_delete;
pub(crate) use connection_delete::handle_workflow_connection_delete;
pub(crate) use monitor_create::handle_monitor_create;
pub(crate) use monitor_history::handle_monitor_history_list;
pub(crate) use monitor_memory::handle_monitor_memory_save;
pub(crate) use monitor_rules::{handle_monitor_rule_add, handle_monitor_rule_delete};
pub(crate) use monitor_self_gate::MonitorSelfGate;
pub(crate) use monitor_task_complete::handle_monitor_task_complete;
pub(crate) use monitor_task_ignore::handle_monitor_task_ignore;
pub(crate) use monitor_trace::handle_monitor_trace_list;
pub(crate) use outbound_action::{
    create_automation_connector_action_draft, handle_automation_pending_action_get,
    handle_automation_pending_action_list, handle_automation_pending_action_reject,
    handle_outbound_action_cancel, handle_outbound_action_execute, handle_outbound_action_status,
    AutomationConnectorActionDraftParams, CreatedAutomationConnectorActionDraft,
};
pub(crate) use runtime::{
    handle_workflow_create, handle_workflow_deploy, handle_workflow_execute,
    handle_workflow_execute_in_memory, handle_workflow_get_execution,
    handle_workflow_list_executions, handle_workflow_node_definition,
    handle_workflow_node_definitions, handle_workflow_undeploy, handle_workflow_update,
};
pub(crate) use telegram_diagnostics::handle_telegram_diagnostics_export;

use anyhow::{Context, Result};
use puffer_config::{load_config, ConfigPaths};
use puffer_core::subscription_manager;
use puffer_subscriptions::{
    connection_subscriber_manifest, connection_workflow_trigger_supported, connector_runtime_hints,
    connector_workflow_trigger_supported, suggested_connection_slug, ActionSpec, ConnectionRecord,
    ConnectorTemplate, FilterSpec, SubscriberManifestRoots, TaggedFilterSpec, WorkflowBindingSpec,
    WorkflowBindingStatus,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use snapshot_json::connection_snapshot_json;
use std::collections::HashMap;
use std::fs;

/// Returns the workflow runtime snapshot with connector catalog context.
pub(crate) fn handle_workflow_list(paths: &ConfigPaths) -> Result<Value> {
    handle_workflow_list_with_runtime(paths, true)
}

pub(crate) fn handle_workflow_list_with_runtime(
    paths: &ConfigPaths,
    include_workflows: bool,
) -> Result<Value> {
    let (workflows, workflow_error) = if include_workflows {
        match runtime_workflows(paths) {
            Ok(workflows) => (workflows, None),
            Err(error) => {
                let detail = format!("{error:#}");
                tracing::warn!(error = %detail, "workflow runtime list failed");
                (
                    Vec::new(),
                    Some(
                        crate::daemon_workflow_runtime::public_workflow_runtime_error_message(
                            &error,
                        ),
                    ),
                )
            }
        }
    } else {
        (Vec::new(), None)
    };
    let mut snapshot = json!({
        "workflows": workflows,
        "runs": [],
        "workflow_error": workflow_error,
    });
    add_connector_context(paths, &mut snapshot);
    add_workflow_binding_context(paths, &mut snapshot);
    add_monitor_task_context(paths, &mut snapshot);
    monitor_memory::add_monitor_memory_context(paths, &mut snapshot);
    task_snapshot::add_task_context(paths, &mut snapshot);
    if let Some(object) = snapshot.as_object_mut() {
        object.insert("monitor_ignore_filter_error".to_string(), Value::Null);
    }
    Ok(snapshot)
}

/// Toggles a workflow binding.
pub(crate) fn handle_workflow_toggle(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let slug = params
        .get("slug")
        .and_then(Value::as_str)
        .context("missing slug")?;
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .context("missing enabled")?;
    let manager = subscription_manager()?;
    let binding_slug = if manager.store().get(slug).is_some() {
        slug.to_string()
    } else {
        anyhow::bail!("workflow binding `{slug}` not found");
    };
    manager
        .store()
        .set_status(&binding_slug, workflow_status(enabled))?;
    manager.refresh_connection_consumers()?;
    handle_workflow_list(paths)
}

fn runtime_workflows(paths: &ConfigPaths) -> Result<Vec<puffer_workflow::WorkflowRuntimeWorkflow>> {
    let config = load_config(paths).context("load workflow backend config")?;
    let client = crate::daemon_workflow_runtime::workflow_runtime_client(paths, &config)
        .context("create workflow runtime client")?;
    client
        .list_workflows()
        .context("list workflows from configured runtime")
}

fn add_connector_context(paths: &ConfigPaths, snapshot: &mut Value) {
    let Some(object) = snapshot.as_object_mut() else {
        return;
    };
    let (connectors, connections, connector_error) = match subscription_manager() {
        Ok(manager) => {
            let roots = subscriber_manifest_roots(paths);
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
                    let schema = monitor_rules::connection_monitor_rule_schema_json(
                        paths,
                        manager.as_ref(),
                        &connection,
                    );
                    let can_trigger_workflow = manager
                        .connector_store()
                        .get(&connection.connector_slug)
                        .is_some_and(|template| {
                            connection_workflow_trigger_supported(&roots, &connection, &template)
                        });
                    connection_snapshot_json(connection, can_trigger_workflow, schema)
                })
                .collect::<Vec<_>>();
            (connectors, connections, None)
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

fn add_workflow_binding_context(paths: &ConfigPaths, snapshot: &mut Value) {
    let Some(object) = snapshot.as_object_mut() else {
        return;
    };
    match subscription_manager() {
        Ok(manager) => {
            let bindings = manager
                .store()
                .list()
                .into_iter()
                .map(|binding| workflow_binding_json(paths, binding))
                .collect::<Vec<_>>();
            object.insert("workflow_bindings".to_string(), Value::Array(bindings));
            object.insert("workflow_binding_error".to_string(), Value::Null);
        }
        Err(error) => {
            object.insert("workflow_bindings".to_string(), Value::Array(Vec::new()));
            object.insert(
                "workflow_binding_error".to_string(),
                Value::String(error.to_string()),
            );
        }
    }
}

fn workflow_binding_json(paths: &ConfigPaths, binding: WorkflowBindingSpec) -> Value {
    let action_type = workflow_action_type(&binding.action);
    let action_path = workflow_action_path(&binding.action);
    let action_format = workflow_action_format(&binding.action);
    let action = serde_json::to_value(&binding.action).unwrap_or(Value::Null);
    let model = workflow_action_model(&binding.action).map(ToOwned::to_owned);
    let filter_pattern = workflow_filter_pattern(binding.filter.as_ref());
    let ignore_filters =
        serde_json::to_value(&binding.ignore_filters).unwrap_or(Value::Array(vec![]));
    let monitor = binding.slug.starts_with("monitor-")
        || (matches!(&binding.action, ActionSpec::TriageAgent { .. })
            && binding.description.to_ascii_lowercase().contains("monitor"));
    let monitor_memory_path = monitor.then(|| {
        paths
            .workspace_config_dir
            .join("runtime")
            .join("monitors")
            .join(format!("{}.md", binding.connection_slug))
            .display()
            .to_string()
    });
    json!({
        "slug": binding.slug,
        "description": binding.description,
        "connection_slug": binding.connection_slug,
        "connector_slug": binding.connector_slug,
        "status": workflow_status_label(binding.status),
        "enabled": binding.status == WorkflowBindingStatus::Enabled,
        "action_type": action_type,
        "action": action,
        "action_path": action_path,
        "action_format": action_format,
        "model": model,
        "include_filter": serde_json::to_value(&binding.filter).unwrap_or(Value::Null),
        "include_filters": monitor_rules::include_filters_json(binding.filter.as_ref()),
        "filter_pattern": filter_pattern,
        "ignore_filters": ignore_filters,
        "contact_ids": binding.contact_ids.clone(),
        "monitor": monitor,
        "monitor_rule_schema": monitor_rules::binding_monitor_rule_schema_json(paths, &binding),
        "monitor_memory_path": monitor_memory_path,
        "created_at_ms": binding.created_at_ms,
    })
}

fn workflow_action_type(action: &ActionSpec) -> &'static str {
    match action {
        ActionSpec::SqliteInsert { .. } => "sqlite_insert",
        ActionSpec::FileAppend { .. } => "file_append",
        ActionSpec::ForwardMessage { .. } => "forward_message",
        ActionSpec::RunWorkflow { .. } => "run_workflow",
        ActionSpec::RunAutomation { .. } => "run_automation",
        ActionSpec::ConnectorAct { .. } => "connector_act",
        ActionSpec::ToolCall { .. } => "tool_call",
        ActionSpec::TriageAgent { .. } => "triage_agent",
        ActionSpec::Graph { .. } => "graph",
        ActionSpec::Unknown => "unknown",
    }
}

fn workflow_action_path(action: &ActionSpec) -> Option<&str> {
    match action {
        ActionSpec::SqliteInsert { path, .. } | ActionSpec::FileAppend { path, .. } => {
            Some(path.as_str())
        }
        _ => None,
    }
}

fn workflow_action_format(action: &ActionSpec) -> Option<String> {
    match action {
        ActionSpec::FileAppend { format, .. } => serde_json::to_value(format)
            .ok()
            .and_then(|value| value.as_str().map(ToOwned::to_owned)),
        _ => None,
    }
}

fn workflow_action_model(action: &ActionSpec) -> Option<&str> {
    match action {
        ActionSpec::TriageAgent { model, .. } => model.as_deref(),
        _ => None,
    }
}

fn workflow_filter_pattern(filter: Option<&FilterSpec>) -> Option<&str> {
    match filter {
        Some(FilterSpec::Tagged(TaggedFilterSpec::Regex { pattern, .. })) => Some(pattern.as_str()),
        _ => None,
    }
}

fn workflow_status(enabled: bool) -> WorkflowBindingStatus {
    if enabled {
        WorkflowBindingStatus::Enabled
    } else {
        WorkflowBindingStatus::Paused
    }
}

fn workflow_status_label(status: WorkflowBindingStatus) -> &'static str {
    match status {
        WorkflowBindingStatus::Enabled => "enabled",
        WorkflowBindingStatus::Paused => "paused",
    }
}

fn resolve_binding_trigger(
    paths: &ConfigPaths,
    manager: &puffer_subscriptions::SubscriptionManager,
    connection_slug: &str,
    connector_slug: Option<&str>,
) -> Result<(Option<ConnectionRecord>, String, ConnectorTemplate)> {
    let roots = subscriber_manifest_roots(paths);
    if let Some(connection) = manager.connection_store().get(connection_slug) {
        if let Some(connector_slug) = connector_slug {
            if connector_slug != connection.connector_slug {
                anyhow::bail!(
                    "connection `{}` uses connector `{}`, not `{connector_slug}`",
                    connection.slug,
                    connection.connector_slug
                );
            }
        }
        let template = manager
            .connector_store()
            .get(&connection.connector_slug)
            .ok_or_else(|| {
                anyhow::anyhow!("connector `{}` not found", connection.connector_slug)
            })?;
        if !connection_workflow_trigger_supported(&roots, &connection, &template) {
            anyhow::bail!(
                "connector `{}` cannot produce workflow trigger events",
                connection.connector_slug
            );
        }
        return Ok((
            Some(connection.clone()),
            connection.connector_slug.clone(),
            template,
        ));
    }

    let connectors = manager.connector_store().list_with_builtins();
    let (connector_slug, template) = planned::resolve_planned_binding_template(
        &roots,
        &connectors,
        connection_slug,
        connector_slug,
    )?;
    Ok((None, connector_slug, template))
}

fn connector_stream_supported(template: &ConnectorTemplate) -> bool {
    template.can_subscribe && template.command_argv().is_some()
}

fn ensure_workflow_subscriber_started(
    manager: &puffer_subscriptions::SubscriptionManager,
    paths: &ConfigPaths,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> Result<()> {
    if connector_stream_supported(template) {
        return Ok(());
    }
    if let Some(manifest) =
        connection_subscriber_manifest(&subscriber_manifest_roots(paths), connection, template)?
    {
        manager.start_subscriber(manifest)?;
    }
    Ok(())
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
    let telegram_peer_avatars = crate::daemon_contacts::cached_telegram_peer_avatars(paths);
    let telegram_peer_names = crate::daemon_contacts::cached_telegram_peer_names(paths);
    let mut errors = Vec::new();
    // Degrade gracefully like `task_snapshot::add_task_context`: a failure to
    // load the outbound store must not blank the whole monitor feed. Carry the
    // error, render tasks with a null `outboundAction`.
    let outbound_store = task_snapshot::outbound_store(paths, &mut errors);
    for error in &errors {
        eprintln!("monitor feed: outbound action store unavailable: {error}");
    }
    Ok(store
        .tasks
        .into_iter()
        .map(|task| {
            monitor_task_json(
                task,
                &telegram_peer_avatars,
                &telegram_peer_names,
                outbound_store.as_ref(),
            )
        })
        .collect())
}

fn monitor_tasks_path(paths: &ConfigPaths) -> std::path::PathBuf {
    paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
        .join("monitor_tasks.json")
}

fn monitor_task_json(
    task: MonitorTaskSnapshotRecord,
    telegram_peer_avatars: &HashMap<String, String>,
    telegram_peer_names: &HashMap<String, String>,
    outbound_store: Option<&puffer_subscriptions::OutboundStore>,
) -> Value {
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
        "actions": monitor_actions_for_status(&task.metadata, &task.status),
        "possible_ignore_reasons": monitor_ignore_reasons(&task.metadata),
        "monitor": task_snapshot::metadata_value(
            &task.metadata,
            &["monitor"],
            &[]
        ),
        "source_context": task_snapshot::monitor_source_context_with_sender_identity(
            &task.metadata,
            telegram_peer_avatars,
            telegram_peer_names
        ),
        "source_messages": task_snapshot::monitor_source_messages(&task.metadata),
        "completion_policy": task_snapshot::monitor_completion_policy(&task.metadata),
        "source_state": task_snapshot::metadata_value(
            &task.metadata,
            &["source_state", "sourceState"],
            &["source_state", "sourceState"]
        ),
        "monitor_task_gate": task_snapshot::metadata_value(
            &task.metadata,
            &["monitor_task_gate", "monitorTaskGate"],
            &["task_gate", "taskGate"]
        ),
        "outboundAction": task_snapshot::outbound_action_for_metadata(outbound_store, &task.metadata),
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

fn monitor_actions_for_status(metadata: &Map<String, Value>, status: &str) -> Vec<Value> {
    if monitor_task_status_is_terminal(status) {
        return Vec::new();
    }
    monitor_actions(metadata)
}

fn monitor_task_status_is_terminal(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "completed" | "cancelled" | "deleted" | "ignored" | "stopped"
    )
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
    use puffer_subscriptions::{
        ConnectionState, NewOutboundDraft, OutboundOrigin, OutboundStore, RecipientSource,
    };

    fn create_outbound_action(paths: &ConfigPaths, task_id: &str, message: &str) -> String {
        OutboundStore::load(paths.user_config_dir.join("outbound_actions.json"))
            .unwrap()
            .create_draft(NewOutboundDraft {
                connector_slug: "telegram-login".to_string(),
                connection_slug: "telegram-user".to_string(),
                action: "send_message".to_string(),
                input: json!({ "chat_id": "42", "message": message }),
                recipient_stable_id: "telegram:42".to_string(),
                recipient_source: RecipientSource::Stamped,
                message: message.to_string(),
                origin: OutboundOrigin {
                    session_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    task_id: Some(task_id.to_string()),
                },
                ttl_ms: None,
            })
            .unwrap()
            .id
    }

    #[test]
    fn trigger_ready_connection_snapshot_includes_monitor_command() {
        let connection =
            ConnectionRecord::authenticated("telegram-user", "telegram-login", "Personal Telegram");

        let snapshot = connection_snapshot_json(connection, true, None);

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

        let snapshot = connection_snapshot_json(connection, false, None);

        assert_eq!(snapshot["connect_command"], "/connect slack-app slack-app");
        assert!(snapshot["monitor_command"].is_null());
        assert_eq!(snapshot["can_trigger_workflow"], false);
    }

    #[test]
    fn connection_snapshot_includes_health_when_present() {
        let mut connection =
            ConnectionRecord::authenticated("telegram-user", "telegram-login", "Personal Telegram");
        connection.state = ConnectionState::Degraded;
        connection.health = Some(puffer_subscriptions::ConnectionHealth {
            status: puffer_subscriptions::ConnectionHealthStatus::Retrying,
            reason: Some("connect_failed".into()),
            detail: Some("read 0 bytes".into()),
            updated_at_ms: 1_700_000_000_000,
            next_retry_at_ms: Some(1_700_000_010_000),
        });

        let snapshot = connection_snapshot_json(connection, true, None);

        assert_eq!(snapshot["state"], "degraded");
        assert_eq!(snapshot["health"]["status"], "retrying");
        assert_eq!(snapshot["health"]["reason"], "connect_failed");
        assert_eq!(snapshot["health"]["detail"], "read 0 bytes");
        assert_eq!(snapshot["health"]["updated_at_ms"], 1_700_000_000_000_i64);
        assert_eq!(
            snapshot["health"]["next_retry_at_ms"],
            1_700_000_010_000_i64
        );
    }

    #[test]
    fn workflow_snapshot_includes_monitor_tasks() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let outbound_action_id =
            create_outbound_action(&paths, "monitor-1", "Deployment finished an hour ago.");
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
                            "possibleIgnoreReasons": ["duplicate support ping"],
                            "source_context": {
                                "connector": "telegram-login",
                                "chat_id": 42
                            },
                            "source_text": "回调失败率刚升到 18%，16:00 前给结论。",
                            "source_message_id": 6836,
                            "completion_policy": "human_gated_reply",
                            "outbound_action_id": outbound_action_id,
                            "source_state": {
                                "telegram": {
                                    "read": true,
                                    "label": "已读"
                                }
                            },
                            "monitor_task_gate": {
                                "decision": "create_read",
                                "read": true,
                                "replied": false
                            }
                        },
                        "started_at_ms": 10,
                        "updated_at_ms": 20
                    },
                    {
                        "task_id": "monitor-sent",
                        "subject": "Answer Telegram follow-up",
                        "description": "Alice asked for the next update.",
                        "status": "completed",
                        "metadata": {
                            "_monitor": true,
                            "monitor_connection": "telegram-user",
                            "monitor_connector": "telegram-login",
                            "actions": [
                                {
                                    "actionName": "Send",
                                    "actionPrompt": "Send the approved Telegram reply."
                                }
                            ]
                        },
                        "started_at_ms": 30,
                        "updated_at_ms": 40
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let snapshot = handle_workflow_list(&paths).unwrap();
        let tasks = snapshot["monitor_tasks"].as_array().unwrap();

        assert_eq!(tasks.len(), 2);
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
        // The human-gated reply review state must surface on monitor_tasks[]
        // (the array bobo's Home renders), not only on the tasks[] snapshot.
        assert_eq!(tasks[0]["source_context"]["chat_id"], 42);
        // The server-stamped verbatim text rides along on source_context so
        // UIs can show the original message next to the LLM paraphrase.
        assert_eq!(
            tasks[0]["source_context"]["text"],
            "回调失败率刚升到 18%，16:00 前给结论。"
        );
        assert_eq!(tasks[0]["source_context"]["message_id"], 6836);
        assert_eq!(tasks[0]["completion_policy"], "human_gated_reply");
        assert_eq!(tasks[0]["source_state"]["telegram"]["read"], true);
        assert_eq!(tasks[0]["source_state"]["telegram"]["label"], "已读");
        assert_eq!(tasks[0]["monitor_task_gate"]["decision"], "create_read");
        assert_eq!(tasks[0]["outboundAction"]["id"], outbound_action_id);
        assert_eq!(tasks[0]["outboundAction"]["status"], "draft_ready");
        assert_eq!(tasks[0]["outboundAction"]["version"], 1);
        assert_eq!(
            tasks[0]["outboundAction"]["message"],
            "Deployment finished an hour ago."
        );
        assert_eq!(tasks[1]["task_id"], "monitor-sent");
        assert!(tasks[1]["outboundAction"].is_null());
        assert!(tasks[1]["actions"].as_array().unwrap().is_empty());
        assert_eq!(snapshot["monitor_task_error"], Value::Null);
    }

    #[test]
    fn workflow_snapshot_monitor_tasks_include_telegram_source_messages() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let task_path = monitor_tasks_path(&paths);
        std::fs::create_dir_all(task_path.parent().unwrap()).unwrap();
        std::fs::write(
            &task_path,
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "monitor-world-cup",
                    "subject": "回复世界杯今晚赛程",
                    "description": "联系人发来今晚赛程请求。",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "source_text": "还有世界杯今晚的赛程",
                        "source_message_id": 53970,
                        "source_context": {
                            "kind": "telegram_direct_message",
                            "sender": { "name": "博阿 杜" },
                            "context_messages": [
                                { "from": "them", "direction": "incoming", "text": "在吗", "message_id": 53961 },
                                { "from": "me", "direction": "outgoing", "text": "在", "message_id": 53962 },
                                { "from": "me", "direction": "outgoing", "text": "今天怎么样", "message_id": 53963 },
                                { "from": "them", "direction": "incoming", "text": "还行", "message_id": 53964 },
                                { "from": "them", "direction": "incoming", "text": "跟我说下NVDA最近的财报情况", "message_id": 53965 },
                                { "from": "me", "direction": "outgoing", "text": "汇报下明天杭州的天气", "message_id": 53967 },
                                { "from": "me", "direction": "outgoing", "text": "还有世界杯今晚的赛程", "message_id": 53968 },
                                { "from": "them", "direction": "incoming", "text": "汇报下明天杭州的天气", "message_id": 53969 }
                            ]
                        },
                        "source_messages": [
                            { "from": "me", "direction": "outgoing", "text": "在", "message_id": 53962 },
                            { "from": "me", "direction": "outgoing", "text": "今天怎么样", "message_id": 53963 },
                            { "from": "them", "direction": "incoming", "text": "还行", "message_id": 53964 },
                            { "from": "them", "direction": "incoming", "text": "跟我说下NVDA最近的财报情况", "message_id": 53965 },
                            { "from": "me", "direction": "outgoing", "text": "汇报下明天杭州的天气", "message_id": 53967 },
                            { "from": "me", "direction": "outgoing", "text": "还有世界杯今晚的赛程", "message_id": 53968 },
                            { "from": "them", "direction": "incoming", "text": "汇报下明天杭州的天气", "message_id": 53969 },
                            { "from": "them", "direction": "incoming", "text": "还有世界杯今晚的赛程", "message_id": 53970 }
                        ],
                        "monitor": {
                            "schema_version": 2,
                            "kind": "telegram.reply",
                            "source": {
                                "connector_slug": "telegram-login",
                                "connection_slug": "telegram-user",
                                "message_id": 53970,
                                "sender_name": "博阿 杜",
                                "text": "还有世界杯今晚的赛程"
                            },
                            "action": { "type": "telegram_reply_draft" }
                        }
                    },
                    "started_at_ms": 10,
                    "updated_at_ms": 20
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let snapshot = handle_workflow_list(&paths).unwrap();
        let tasks = snapshot["monitor_tasks"].as_array().unwrap();
        let source_messages = tasks[0]["source_messages"].as_array().unwrap();

        assert_eq!(source_messages.len(), 8);
        assert_eq!(source_messages[0]["text"], "在");
        assert_eq!(source_messages[0]["direction"], "outgoing");
        assert_eq!(source_messages[7]["text"], "还有世界杯今晚的赛程");
        assert_eq!(source_messages[7]["message_id"], 53970);
    }

    #[test]
    fn workflow_snapshot_uses_typed_monitor_context_for_gmail_tasks() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let task_path = monitor_tasks_path(&paths);
        std::fs::create_dir_all(task_path.parent().unwrap()).unwrap();
        std::fs::write(
            &task_path,
            serde_json::to_string_pretty(&json!({
                "tasks": [
                    {
                        "task_id": "monitor-gmail-1",
                        "subject": "Confirm next week's meeting",
                        "description": "Reply to the Gmail thread with available times.",
                        "status": "pending",
                        "metadata": {
                            "_monitor": true,
                            "monitor_connection": "gmail-browser",
                            "monitor_connector": "gmail-browser",
                            "monitor_memory_path": "/tmp/gmail-browser.md",
                            "source_context": {
                                "kind": "telegram_direct_message",
                                "delivery_target": {
                                    "type": "telegram_chat",
                                    "chat_id": "999"
                                }
                            },
                            "monitor": {
                                "schema_version": 2,
                                "kind": "gmail.reply",
                                "source_hash": "sha256:b8e1bc99df97a47171b03fd10a708fb4c8220f8ae5cbe59e5c6ce4005cc847b2",
                                "source": {
                                    "connector_slug": "gmail-browser",
                                    "connection_slug": "gmail-browser",
                                    "account": "winterfell0614@gmail.com",
                                    "thread_id": "thread-123",
                                    "message_id": "message-123",
                                    "from": {
                                        "name": "Fu Xiangyu",
                                        "email": "fuxiangyu@example.com"
                                    }
                                },
                                "action": {
                                    "type": "gmail_reply_draft",
                                    "approval": "draft_then_create_gmail_draft"
                                }
                            }
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
        let monitor_tasks = snapshot["monitor_tasks"].as_array().unwrap();
        let task_rows = snapshot["tasks"].as_array().unwrap();
        let task_row = task_rows
            .iter()
            .find(|task| task["task_id"] == "monitor-gmail-1")
            .expect("monitor task row");

        assert_eq!(monitor_tasks[0]["monitor"]["kind"], "gmail.reply");
        assert_eq!(monitor_tasks[0]["source_context"]["kind"], "gmail_message");
        assert_eq!(
            monitor_tasks[0]["source_context"]["delivery_target"]["type"],
            "gmail_thread"
        );
        assert_eq!(
            monitor_tasks[0]["source_context"]["sender"]["email"],
            "fuxiangyu@example.com"
        );
        assert_eq!(task_row["monitor"]["kind"], "gmail.reply");
        assert_eq!(task_row["source_context"]["kind"], "gmail_message");
        assert!(task_row["outboundAction"].is_null());
    }

    #[test]
    fn workflow_snapshot_enriches_monitor_sender_avatar_from_telegram_peer_cache() {
        let tempdir = tempfile::tempdir().unwrap();
        // Without the home override, user_config_dir resolves to the REAL
        // ~/.puffer and this test clobbers the developer's peer cache.
        let _home = puffer_config::set_puffer_home_override(tempdir.path());
        let paths = ConfigPaths::discover(tempdir.path());
        let avatar = "data:image/jpeg;base64,ZmFrZS1hdmF0YXI=";
        let account_dir = paths
            .user_config_dir
            .join("telegram-accounts")
            .join("telegram-user");
        std::fs::create_dir_all(&account_dir).unwrap();
        std::fs::write(
            account_dir.join("peer-cache.json"),
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "peers": [{
                    "id": "5229190700",
                    "numeric_id": 5229190700_i64,
                    "kind": "user",
                    "title": "Helen",
                    "username": "helen",
                    "avatar": avatar,
                    "is_bot": false,
                    "updated_at_ms": 1_700_000_000_000_i64
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let task_path = monitor_tasks_path(&paths);
        std::fs::create_dir_all(task_path.parent().unwrap()).unwrap();
        std::fs::write(
            &task_path,
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "monitor-avatar",
                    "subject": "Reply to Helen",
                    "description": "Helen asked for the shipping ETA.",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "chat_id": "5229190700",
                        "sender_id": "5229190700",
                        "sender_username": "helen"
                    },
                    "started_at_ms": 10,
                    "updated_at_ms": 20
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let snapshot = handle_workflow_list(&paths).unwrap();
        let tasks = snapshot["monitor_tasks"].as_array().unwrap();

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["source_context"]["sender"]["avatar_url"], avatar);
        // The display name rides along from the same peer-cache entry, so
        // accounts without an @username stop rendering as "Unknown sender".
        assert_eq!(tasks[0]["source_context"]["sender"]["name"], "Helen");
    }

    #[test]
    fn workflow_snapshot_enriches_sender_name_without_username_or_avatar() {
        let tempdir = tempfile::tempdir().unwrap();
        let _home = puffer_config::set_puffer_home_override(tempdir.path());
        let paths = ConfigPaths::discover(tempdir.path());
        let account_dir = paths
            .user_config_dir
            .join("telegram-accounts")
            .join("telegram-user");
        std::fs::create_dir_all(&account_dir).unwrap();
        // Mirrors the reported case: a contact with a display name but no
        // @username and no profile photo (Telegram letter-avatar account).
        std::fs::write(
            account_dir.join("peer-cache.json"),
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "peers": [{
                    "id": "8759047281",
                    "numeric_id": 8759047281_i64,
                    "kind": "user",
                    "title": "博阿 杜",
                    "first_name": "博阿",
                    "last_name": "杜",
                    "is_bot": false,
                    "updated_at_ms": 1_700_000_000_000_i64
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let task_path = monitor_tasks_path(&paths);
        std::fs::create_dir_all(task_path.parent().unwrap()).unwrap();
        std::fs::write(
            &task_path,
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "monitor-name",
                    "subject": "调研国保单位清单",
                    "description": "Telegram 联系人发来请求。",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "chat_id": "8759047281",
                        "sender_id": "8759047281"
                    },
                    "started_at_ms": 10,
                    "updated_at_ms": 20
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let snapshot = handle_workflow_list(&paths).unwrap();
        let tasks = snapshot["monitor_tasks"].as_array().unwrap();

        assert_eq!(tasks[0]["source_context"]["sender"]["name"], "博阿 杜");
        assert!(tasks[0]["source_context"]["sender"]
            .get("avatar_url")
            .is_none());
    }

    #[test]
    fn workflow_binding_json_marks_monitor_bindings() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let binding = WorkflowBindingSpec {
            slug: "monitor-telegram-user".to_string(),
            description: "Monitor telegram-user for actionable tasks".to_string(),
            connection_slug: "telegram-user".to_string(),
            connector_slug: Some("telegram-login".to_string()),
            status: WorkflowBindingStatus::Paused,
            filter: None,
            ignore_filters: Vec::new(),
            contact_ids: Vec::new(),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::TriageAgent {
                prompt: "triage events".to_string(),
                model: Some("openai/gpt-5.4".to_string()),
            },
            created_at_ms: 42,
        };

        let value = workflow_binding_json(&paths, binding);

        assert_eq!(value["slug"], "monitor-telegram-user");
        assert_eq!(value["status"], "paused");
        assert_eq!(value["enabled"], false);
        assert_eq!(value["action_type"], "triage_agent");
        assert_eq!(value["model"], "openai/gpt-5.4");
        assert_eq!(value["monitor"], true);
        assert!(value["monitor_memory_path"]
            .as_str()
            .unwrap()
            .ends_with("runtime/monitors/telegram-user.md"));
    }

    #[test]
    fn workflow_binding_json_includes_file_append_details() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let binding = WorkflowBindingSpec {
            slug: "append-telegram-user-hi".to_string(),
            description: "Capture hellos".to_string(),
            connection_slug: "telegram-user".to_string(),
            connector_slug: Some("telegram-login".to_string()),
            status: WorkflowBindingStatus::Paused,
            filter: None,
            ignore_filters: Vec::new(),
            contact_ids: Vec::new(),
            classify_prompt: None,
            classify_model: None,
            action: serde_json::from_value(json!({
                "type": "file_append",
                "path": "/tmp/hi",
                "format": "text"
            }))
            .unwrap(),
            created_at_ms: 42,
        };

        let value = workflow_binding_json(&paths, binding);

        assert_eq!(value["slug"], "append-telegram-user-hi");
        assert_eq!(value["description"], "Capture hellos");
        assert_eq!(value["status"], "paused");
        assert_eq!(value["action_type"], "file_append");
        assert_eq!(value["action_path"], "/tmp/hi");
        assert_eq!(value["action_format"], "text");
        assert!(value["filter_pattern"].is_null());
        assert_eq!(value["monitor"], false);
    }

    #[test]
    fn workflow_snapshot_tolerates_missing_monitor_task_store() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());

        let snapshot = handle_workflow_list(&paths).unwrap();

        assert_eq!(snapshot["monitor_tasks"].as_array().unwrap().len(), 0);
        assert_eq!(snapshot["monitor_task_error"], Value::Null);
    }
}
