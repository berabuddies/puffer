//! Task snapshot helpers for the desktop workflow/task screens.

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default)]
struct TaskStoreSnapshot {
    #[serde(default)]
    tasks: Vec<TaskSnapshotRecord>,
}

#[derive(Debug, Deserialize)]
struct TaskSnapshotRecord {
    #[serde(alias = "id", alias = "taskId")]
    task_id: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    description: String,
    #[serde(default, alias = "activeForm")]
    active_form: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    blocks: Vec<String>,
    #[serde(default, alias = "blockedBy")]
    blocked_by: Vec<String>,
    #[serde(default)]
    metadata: Map<String, Value>,
    #[serde(default, alias = "taskType")]
    task_type: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default, alias = "processId")]
    process_id: Option<u32>,
    #[serde(default, alias = "outputFile")]
    output_file: Option<String>,
    #[serde(default, rename = "receivedAt", alias = "received_at")]
    received_at: Option<String>,
    #[serde(default, rename = "expiresAt", alias = "expires_at")]
    expires_at: Option<String>,
    #[serde(default, alias = "startedAtMs")]
    started_at_ms: Option<u64>,
    #[serde(default, alias = "updatedAtMs")]
    updated_at_ms: Option<u64>,
    #[serde(default, alias = "exitCode")]
    exit_code: Option<i32>,
}

/// Adds normalized agent-created and monitor-created task rows to a workflow snapshot.
pub(crate) fn add_task_context(paths: &ConfigPaths, snapshot: &mut Value) {
    let Some(object) = snapshot.as_object_mut() else {
        return;
    };
    let mut tasks = Vec::new();
    let mut errors = Vec::new();
    let workflow = workflow_root(paths);
    append_task_file(
        &workflow.join("tasks.json"),
        "agent",
        "workspace",
        "workspace",
        &mut tasks,
        &mut errors,
    );
    append_scoped_agent_tasks(
        &workflow.join("sessions"),
        "session",
        &mut tasks,
        &mut errors,
    );
    append_scoped_agent_tasks(
        &workflow.join("team_tasks"),
        "team",
        &mut tasks,
        &mut errors,
    );
    append_task_file(
        &workflow.join("monitor_tasks.json"),
        "monitor",
        "monitor",
        "monitors",
        &mut tasks,
        &mut errors,
    );
    object.insert("tasks".to_string(), Value::Array(tasks));
    object.insert(
        "task_error".to_string(),
        if errors.is_empty() {
            Value::Null
        } else {
            Value::String(errors.join("; "))
        },
    );
}

fn append_scoped_agent_tasks(
    parent: &Path,
    scope_kind: &str,
    tasks: &mut Vec<Value>,
    errors: &mut Vec<String>,
) {
    match sorted_child_dirs(parent) {
        Ok(dirs) => {
            for dir in dirs {
                let Some(scope_id) = dir.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                let scope = format!("{scope_kind}:{scope_id}");
                let label = scope_label(scope_kind, scope_id);
                append_task_file(
                    &dir.join("tasks.json"),
                    "agent",
                    &scope,
                    &label,
                    tasks,
                    errors,
                );
            }
        }
        Err(error) => errors.push(error.to_string()),
    }
}

fn append_task_file(
    path: &Path,
    source: &str,
    scope: &str,
    scope_label: &str,
    tasks: &mut Vec<Value>,
    errors: &mut Vec<String>,
) {
    match load_task_file(path, source, scope, scope_label) {
        Ok(rows) => tasks.extend(rows),
        Err(error) => errors.push(error.to_string()),
    }
}

fn load_task_file(path: &Path, source: &str, scope: &str, scope_label: &str) -> Result<Vec<Value>> {
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let store: TaskStoreSnapshot = serde_json::from_str(&raw)
        .with_context(|| format!("invalid task store {}", path.display()))?;
    Ok(store
        .tasks
        .into_iter()
        .map(|task| task_json(task, source, scope, scope_label))
        .collect())
}

fn task_json(task: TaskSnapshotRecord, source: &str, scope: &str, scope_label: &str) -> Value {
    json!({
        "task_id": task.task_id,
        "subject": task.subject,
        "description": task.description,
        "active_form": task.active_form,
        "status": task.status,
        "source": source,
        "task_scope": scope,
        "task_scope_label": scope_label,
        "task_type": task.task_type.unwrap_or_else(|| "task".to_string()),
        "owner": task.owner,
        "blocks": task.blocks,
        "blocked_by": task.blocked_by,
        "command": task.command,
        "process_id": task.process_id,
        "output_file": task.output_file,
        "received_at": task.received_at,
        "expires_at": task.expires_at,
        "started_at_ms": task.started_at_ms,
        "updated_at_ms": task.updated_at_ms,
        "exit_code": task.exit_code,
        "ignored": metadata_bool(&task.metadata, "ignored"),
        "monitor_connection": metadata_string(
            &task.metadata,
            &["monitor_connection", "monitorConnection"],
            &["connection", "connection_slug", "connectionSlug"]
        ),
        "monitor_connector": metadata_string(
            &task.metadata,
            &["monitor_connector", "monitorConnector"],
            &["connector", "connector_slug", "connectorSlug"]
        ),
        "monitor_memory_path": metadata_string(
            &task.metadata,
            &["monitor_memory_path", "monitorMemoryPath"],
            &["memory_path", "memoryPath"]
        ),
        "actions": monitor_actions(&task.metadata),
        "possible_ignore_reasons": monitor_ignore_reasons(&task.metadata),
    })
}

fn workflow_root(paths: &ConfigPaths) -> PathBuf {
    paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
}

fn sorted_child_dirs(parent: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", parent.display()));
        }
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read {}", parent.display()))?;
        if entry
            .file_type()
            .with_context(|| format!("failed to stat {}", entry.path().display()))?
            .is_dir()
        {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn scope_label(scope_kind: &str, scope_id: &str) -> String {
    if scope_kind == "session" {
        let short = scope_id.get(..8).unwrap_or(scope_id);
        return format!("session {short}");
    }
    format!("{scope_kind} {scope_id}")
}

fn metadata_string(
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

fn metadata_bool(metadata: &Map<String, Value>, key: &str) -> bool {
    metadata
        .get(key)
        .and_then(Value::as_bool)
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(Value::as_object)
                .and_then(|monitor| monitor.get(key))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

fn monitor_actions(metadata: &Map<String, Value>) -> Vec<Value> {
    metadata
        .get("actions")
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(|monitor| monitor.get("actions"))
        })
        .and_then(Value::as_array)
        .map(|actions| {
            actions
                .iter()
                .filter_map(|action| {
                    let object = action.as_object()?;
                    let name = string_value(object.get("name"))
                        .or_else(|| string_value(object.get("actionName")))?;
                    let prompt = string_value(object.get("prompt"))
                        .or_else(|| string_value(object.get("actionPrompt")))?;
                    Some(json!({ "name": name, "prompt": prompt }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn monitor_ignore_reasons(metadata: &Map<String, Value>) -> Vec<Value> {
    metadata
        .get("possible_ignore_reasons")
        .or_else(|| metadata.get("possibleIgnoreReasons"))
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(|monitor| monitor.get("possible_ignore_reasons"))
        })
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(|monitor| monitor.get("possibleIgnoreReasons"))
        })
        .and_then(Value::as_array)
        .map(|reasons| {
            reasons
                .iter()
                .filter_map(|reason| string_value(Some(reason)))
                .map(Value::String)
                .collect()
        })
        .unwrap_or_default()
}

fn string_value(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::ConfigPaths;

    #[test]
    fn task_context_reads_agent_session_team_and_monitor_tasks() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let workflow = workflow_root(&paths);
        std::fs::create_dir_all(workflow.join("sessions/session-one")).unwrap();
        std::fs::create_dir_all(workflow.join("team_tasks/alpha")).unwrap();
        std::fs::write(
            workflow.join("sessions/session-one/tasks.json"),
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "task-1",
                    "subject": "Fix failing check",
                    "description": "Run the targeted test and patch the failure.",
                    "active_form": "Fixing failing check",
                    "status": "in_progress",
                    "metadata": {},
                    "started_at_ms": 10,
                    "updated_at_ms": 20
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            workflow.join("team_tasks/alpha/tasks.json"),
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "team-1",
                    "subject": "Review worker patch",
                    "description": "Review the delegated patch.",
                    "active_form": "Reviewing worker patch",
                    "status": "pending",
                    "metadata": {}
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            workflow.join("monitor_tasks.json"),
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "monitor-1",
                    "subject": "Handle support ping",
                    "description": "Alice asked for a deployment update.",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "actions": [{
                            "actionName": "Draft reply",
                            "actionPrompt": "Draft a concise reply."
                        }]
                    }
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let mut snapshot = json!({});
        add_task_context(&paths, &mut snapshot);
        let tasks = snapshot["tasks"].as_array().unwrap();

        assert_eq!(tasks.len(), 3);
        let session_task = tasks
            .iter()
            .find(|task| task["task_id"] == "task-1")
            .unwrap();
        assert_eq!(session_task["source"], "agent");
        assert_eq!(session_task["task_scope"], "session:session-one");
        let team_task = tasks
            .iter()
            .find(|task| task["task_id"] == "team-1")
            .unwrap();
        assert_eq!(team_task["task_scope"], "team:alpha");
        let monitor_task = tasks
            .iter()
            .find(|task| task["task_id"] == "monitor-1")
            .unwrap();
        assert_eq!(monitor_task["source"], "monitor");
        assert_eq!(monitor_task["monitor_connection"], "telegram-user");
        assert_eq!(monitor_task["actions"][0]["name"], "Draft reply");
        assert!(snapshot["task_error"].is_null());
    }
}
