use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

/// Read-only monitor task context for the workflow dashboard.
#[derive(Debug, Default)]
pub(super) struct MonitorTaskContext {
    pub(super) tasks: Vec<MonitorTaskRow>,
    pub(super) error: Option<String>,
}

impl MonitorTaskContext {
    /// Returns the number of non-ignored monitor tasks.
    pub(super) fn active_count(&self) -> usize {
        self.tasks.iter().filter(|task| !task.ignored).count()
    }
}

#[derive(Debug)]
pub(super) struct MonitorTaskRow {
    task_id: String,
    subject: String,
    description: String,
    status: String,
    monitor_connection: Option<String>,
    monitor_connector: Option<String>,
    monitor_memory_path: Option<String>,
    ignored: bool,
    actions: Vec<MonitorTaskAction>,
    possible_ignore_reasons: Vec<String>,
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
}

#[derive(Debug)]
struct MonitorTaskAction {
    name: String,
    prompt: String,
}

/// Loads connector monitor tasks for `/workflows` without failing the command.
pub(super) fn load_monitor_task_context(paths: &ConfigPaths) -> MonitorTaskContext {
    match load_monitor_tasks(paths) {
        Ok(tasks) => MonitorTaskContext { tasks, error: None },
        Err(error) => MonitorTaskContext {
            tasks: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

/// Renders monitor tasks and command hints for the workflow dashboard.
pub(super) fn write_monitor_tasks(out: &mut String, context: &MonitorTaskContext, query: &str) {
    out.push_str("\nMonitor tasks\n");
    out.push_str("filters: active | ignored | action | ignore | /tasks show | /tasks ignore\n");
    let tasks = context
        .tasks
        .iter()
        .filter(|task| super::matches_query(query, task.search_terms().iter().map(String::as_str)))
        .collect::<Vec<_>>();
    super::write_result_summary(
        out,
        tasks.len(),
        context.tasks.len(),
        "monitor tasks",
        query,
    );
    if context.tasks.is_empty() {
        out.push_str("- none recorded\n");
        return;
    }
    if tasks.is_empty() {
        out.push_str("- no matching monitor tasks\n");
        return;
    }
    for task in tasks.into_iter().take(20) {
        let action_summary = task.action_summary();
        let ignore_command = task.ignore_command();
        let _ = writeln!(
            out,
            "- {} [{}] connection={} connector={} subject={}{} show=/tasks show {} ignore={}",
            task.task_id,
            task.status_label(),
            task.monitor_connection.as_deref().unwrap_or("<unknown>"),
            task.monitor_connector.as_deref().unwrap_or("<unknown>"),
            super::first_line(&task.subject_or_description()),
            action_summary
                .as_deref()
                .map(|summary| format!(" actions={summary}"))
                .unwrap_or_default(),
            task.task_id,
            ignore_command
        );
    }
}

impl MonitorTaskRow {
    fn from_record(record: MonitorTaskSnapshotRecord) -> Self {
        let ignored = monitor_metadata_bool(&record.metadata, "ignored");
        Self {
            task_id: record.task_id,
            subject: record.subject,
            description: record.description,
            status: record.status,
            monitor_connection: monitor_metadata_string(
                &record.metadata,
                &["monitor_connection", "monitorConnection"],
                &["connection", "connection_slug", "connectionSlug"],
            ),
            monitor_connector: monitor_metadata_string(
                &record.metadata,
                &["monitor_connector", "monitorConnector"],
                &["connector", "connector_slug", "connectorSlug"],
            ),
            monitor_memory_path: monitor_metadata_string(
                &record.metadata,
                &["monitor_memory_path", "monitorMemoryPath"],
                &["memory_path", "memoryPath"],
            ),
            ignored,
            actions: monitor_actions(&record.metadata),
            possible_ignore_reasons: monitor_ignore_reasons(&record.metadata),
        }
    }

    fn search_terms(&self) -> Vec<String> {
        let mut terms = vec![
            "monitor task".to_string(),
            self.task_id.clone(),
            self.subject.clone(),
            self.description.clone(),
            self.status.clone(),
            self.status_label(),
            "/tasks".to_string(),
            format!("/tasks show {}", self.task_id),
            self.ignore_command(),
        ];
        terms.extend(
            [
                self.monitor_connection.clone(),
                self.monitor_connector.clone(),
                self.monitor_memory_path.clone(),
            ]
            .into_iter()
            .flatten(),
        );
        if self.ignored {
            terms.push("ignored".to_string());
        } else {
            terms.extend(["active", "open"].into_iter().map(str::to_string));
        }
        if !self.actions.is_empty() {
            terms.push("action".to_string());
            terms.push("actions".to_string());
            for action in &self.actions {
                terms.push(action.name.clone());
                terms.push(action.prompt.clone());
            }
        }
        if !self.possible_ignore_reasons.is_empty() {
            terms.push("ignore".to_string());
            terms.extend(self.possible_ignore_reasons.iter().cloned());
        }
        terms
    }

    fn status_label(&self) -> String {
        if self.ignored {
            "ignored".to_string()
        } else if self.status.trim().is_empty() {
            "pending".to_string()
        } else {
            self.status.clone()
        }
    }

    fn subject_or_description(&self) -> String {
        if self.subject.trim().is_empty() {
            self.description.clone()
        } else {
            self.subject.clone()
        }
    }

    fn action_summary(&self) -> Option<String> {
        if self.actions.is_empty() {
            return None;
        }
        let visible = self
            .actions
            .iter()
            .take(3)
            .map(|action| action.name.as_str())
            .collect::<Vec<_>>();
        let hidden = self.actions.len().saturating_sub(visible.len());
        let mut summary = visible.join(",");
        if hidden > 0 {
            let _ = write!(summary, ",+{hidden}");
        }
        Some(summary)
    }

    fn ignore_command(&self) -> String {
        self.possible_ignore_reasons
            .first()
            .map(|reason| format!("/tasks ignore {} {}", self.task_id, reason))
            .unwrap_or_else(|| format!("/tasks ignore {}", self.task_id))
    }
}

fn load_monitor_tasks(paths: &ConfigPaths) -> Result<Vec<MonitorTaskRow>> {
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
    Ok(store
        .tasks
        .into_iter()
        .map(MonitorTaskRow::from_record)
        .collect())
}

fn monitor_tasks_path(paths: &ConfigPaths) -> PathBuf {
    paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
        .join("monitor_tasks.json")
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

fn monitor_actions(metadata: &Map<String, Value>) -> Vec<MonitorTaskAction> {
    metadata_value_array(metadata, "actions")
        .map(|actions| {
            actions
                .iter()
                .filter_map(|action| {
                    let name = string_field(action, &["actionName", "name", "title"])?;
                    let prompt = string_field(action, &["actionPrompt", "prompt"])?;
                    Some(MonitorTaskAction { name, prompt })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn monitor_ignore_reasons(metadata: &Map<String, Value>) -> Vec<String> {
    metadata_value_array(metadata, "possibleIgnoreReasons")
        .or_else(|| metadata_value_array(metadata, "possible_ignore_reasons"))
        .map(|reasons| {
            reasons
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .map(ToOwned::to_owned)
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
