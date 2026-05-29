//! Workflow daemon RPC helpers.

mod binding_delete;
mod connection_delete;
mod monitor_create;
mod monitor_memory;
mod monitor_task_ignore;
mod planned;
mod task_snapshot;

pub(crate) use binding_delete::handle_workflow_binding_delete;
pub(crate) use connection_delete::handle_workflow_connection_delete;
pub(crate) use monitor_create::handle_monitor_create;
pub(crate) use monitor_memory::handle_monitor_memory_save;
pub(crate) use monitor_task_ignore::handle_monitor_task_ignore;

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{
    connection_subscriber_manifest, connection_workflow_trigger_supported, connector_runtime_hints,
    connector_workflow_trigger_supported, suggested_connection_slug, ActionSpec, ConnectionRecord,
    ConnectorTemplate, FilterSpec, SubscriberManifestRoots, TaggedFilterSpec, WorkflowBindingSpec,
    WorkflowBindingStatus,
};
use puffer_workflow::{RegisterOptions, WorkflowStore};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::fs;

/// Returns the workflow editor snapshot with connector catalog context.
pub(crate) fn handle_workflow_list(paths: &ConfigPaths) -> Result<Value> {
    let store = WorkflowStore::new(&paths.workspace_config_dir);
    let mut snapshot = serde_json::to_value(store.snapshot()?)?;
    let ignore_filter_sync_error =
        monitor_task_ignore::sync_monitor_ignore_filters_from_tasks(paths)
            .err()
            .map(|error| error.to_string());
    add_connector_context(paths, &mut snapshot);
    add_workflow_binding_context(paths, &mut snapshot);
    add_monitor_task_context(paths, &mut snapshot);
    monitor_memory::add_monitor_memory_context(paths, &mut snapshot);
    task_snapshot::add_task_context(paths, &mut snapshot);
    if let Some(object) = snapshot.as_object_mut() {
        object.insert(
            "monitor_ignore_filter_error".to_string(),
            ignore_filter_sync_error
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
    }
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
    add_workflow_binding_context(paths, &mut snapshot);
    add_monitor_task_context(paths, &mut snapshot);
    monitor_memory::add_monitor_memory_context(paths, &mut snapshot);
    task_snapshot::add_task_context(paths, &mut snapshot);
    Ok(snapshot)
}

/// Creates or updates a simple connection-triggered file append workflow binding.
pub(crate) fn handle_workflow_binding_create(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let parsed: WorkflowBindingCreateParams =
        serde_json::from_value(params.clone()).context("invalid workflow binding create params")?;
    let manager = subscription_manager()?;
    let (connection, connector_slug, template) = resolve_binding_trigger(
        paths,
        manager.as_ref(),
        &parsed.connection_slug,
        parsed.connector_slug.as_deref(),
    )?;
    let binding = file_append_binding_from_params(parsed, connector_slug)?;
    let enabled = binding.status == WorkflowBindingStatus::Enabled;
    let binding_slug = binding.slug.clone();
    let previous = manager.store().get(&binding_slug);
    manager.store().upsert(binding.clone())?;
    let setup_result = (|| -> Result<()> {
        if enabled {
            if let Some(connection) = connection.as_ref() {
                ensure_workflow_subscriber_started(manager.as_ref(), paths, connection, &template)?;
            }
        }
        manager.refresh_connection_consumers()?;
        Ok(())
    })();
    if let Err(error) = setup_result {
        let rollback_result = if let Some(previous) = previous {
            manager.store().upsert(previous).map(|_| ())
        } else {
            manager.store().delete(&binding_slug)
        };
        if let Err(rollback_error) = rollback_result {
            anyhow::bail!(
                "workflow binding `{binding_slug}` setup failed: {error:#}; rollback failed: {rollback_error:#}"
            );
        }
        manager.refresh_connection_consumers()?;
        return Err(error)
            .with_context(|| format!("workflow binding `{binding_slug}` setup failed"));
    }
    handle_workflow_list(paths)
}

/// Toggles a native workflow or subscription workflow binding.
pub(crate) fn handle_workflow_toggle(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let slug = params
        .get("slug")
        .and_then(Value::as_str)
        .context("missing slug")?;
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .context("missing enabled")?;
    let store = WorkflowStore::new(&paths.workspace_config_dir);
    if let Some(mut workflow) = store.get(slug)? {
        workflow.enabled = enabled;
        let workflow = store.upsert(workflow)?;
        if let Ok(manager) = subscription_manager() {
            let binding_slug = format!("workflow-{}", workflow.slug);
            if manager.store().get(&binding_slug).is_some() {
                manager
                    .store()
                    .set_status(&binding_slug, workflow_status(enabled))?;
                manager.refresh_connection_consumers()?;
            }
        }
        return handle_workflow_list(paths);
    }
    let manager = subscription_manager()?;
    let binding_slug = if manager.store().get(slug).is_some() {
        slug.to_string()
    } else {
        let native_slug = format!("workflow-{slug}");
        if manager.store().get(&native_slug).is_some() {
            native_slug
        } else {
            anyhow::bail!("workflow `{slug}` not found");
        }
    };
    manager
        .store()
        .set_status(&binding_slug, workflow_status(enabled))?;
    manager.refresh_connection_consumers()?;
    handle_workflow_list(paths)
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
    let filter_pattern = workflow_filter_pattern(binding.filter.as_ref());
    let ignore_filters =
        serde_json::to_value(&binding.ignore_filters).unwrap_or_else(|_| Value::Array(Vec::new()));
    let monitor = binding.slug.starts_with("monitor-")
        || (matches!(binding.action, ActionSpec::TriageAgent { .. })
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
        "action_path": action_path,
        "action_format": action_format,
        "filter_pattern": filter_pattern,
        "ignore_filters": ignore_filters,
        "monitor": monitor,
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

#[derive(Debug, Deserialize)]
struct WorkflowBindingCreateParams {
    #[serde(default)]
    slug: Option<String>,
    connection_slug: String,
    #[serde(default)]
    connector_slug: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default, alias = "path")]
    file_append_path: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
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

fn file_append_binding_from_params(
    params: WorkflowBindingCreateParams,
    connector_slug: String,
) -> Result<WorkflowBindingSpec> {
    let connection_slug = params.connection_slug.trim();
    if connection_slug.is_empty() {
        anyhow::bail!("connection_slug must not be empty");
    }
    let path = params
        .file_append_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("missing file_append_path")?
        .to_string();
    let slug = params
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| default_file_append_slug(connection_slug, &path));
    let pattern = params
        .pattern
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != ".*")
        .map(ToOwned::to_owned);
    let action = serde_json::from_value::<ActionSpec>(json!({
        "type": "file_append",
        "path": path,
        "format": "text",
    }))
    .context("build file_append action")?;
    Ok(WorkflowBindingSpec {
        slug,
        description: params.description.unwrap_or_else(|| {
            format!(
                "Append {connection_slug} messages to {}",
                workflow_action_path(&action).unwrap_or("")
            )
        }),
        connection_slug: connection_slug.to_string(),
        connector_slug: Some(connector_slug),
        status: workflow_status(params.enabled.unwrap_or(true)),
        filter: pattern.map(|pattern| {
            FilterSpec::Tagged(TaggedFilterSpec::Regex {
                pattern,
                case_insensitive: true,
            })
        }),
        ignore_filters: Vec::new(),
        classify_prompt: None,
        classify_model: None,
        action,
        created_at_ms: puffer_subscriptions::now_ms(),
    })
}

fn default_file_append_slug(connection_slug: &str, path: &str) -> String {
    let path_leaf = path
        .rsplit('/')
        .find(|part| !part.trim().is_empty())
        .unwrap_or("events");
    format!("append-{connection_slug}-{}", slug_fragment(path_leaf))
}

fn slug_fragment(value: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            slug.push(ch);
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "events".to_string()
    } else {
        slug
    }
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
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::TriageAgent {
                prompt: "triage events".to_string(),
                model: None,
            },
            created_at_ms: 42,
        };

        let value = workflow_binding_json(&paths, binding);

        assert_eq!(value["slug"], "monitor-telegram-user");
        assert_eq!(value["status"], "paused");
        assert_eq!(value["enabled"], false);
        assert_eq!(value["action_type"], "triage_agent");
        assert_eq!(value["monitor"], true);
        assert!(value["monitor_memory_path"]
            .as_str()
            .unwrap()
            .ends_with("runtime/monitors/telegram-user.md"));
    }

    #[test]
    fn file_append_binding_params_build_regex_filtered_spec() {
        let binding = file_append_binding_from_params(
            WorkflowBindingCreateParams {
                slug: Some("append-telegram-user-hi".to_string()),
                connection_slug: "telegram-user".to_string(),
                connector_slug: Some("telegram-login".to_string()),
                description: None,
                pattern: Some("hi".to_string()),
                file_append_path: Some("/tmp/hi".to_string()),
                enabled: Some(true),
            },
            "telegram-login".to_string(),
        )
        .unwrap();

        assert_eq!(binding.slug, "append-telegram-user-hi");
        assert_eq!(
            binding.description,
            "Append telegram-user messages to /tmp/hi"
        );
        assert_eq!(binding.connection_slug, "telegram-user");
        assert_eq!(binding.connector_slug.as_deref(), Some("telegram-login"));
        assert_eq!(binding.status, WorkflowBindingStatus::Enabled);
        assert!(matches!(
            binding.filter,
            Some(FilterSpec::Tagged(TaggedFilterSpec::Regex {
                ref pattern,
                case_insensitive: true
            })) if pattern == "hi"
        ));
        assert!(matches!(
            binding.action,
            ActionSpec::FileAppend {
                ref path,
                ..
            } if path == "/tmp/hi"
        ));
    }

    #[test]
    fn workflow_binding_json_includes_file_append_details() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let binding = file_append_binding_from_params(
            WorkflowBindingCreateParams {
                slug: None,
                connection_slug: "telegram-user".to_string(),
                connector_slug: None,
                description: Some("Capture hellos".to_string()),
                pattern: Some(".*".to_string()),
                file_append_path: Some("/tmp/hi".to_string()),
                enabled: Some(false),
            },
            "telegram-login".to_string(),
        )
        .unwrap();

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
