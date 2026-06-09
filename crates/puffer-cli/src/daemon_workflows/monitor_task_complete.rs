use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use super::monitor_task_ignore::{monitor_tasks_path, task_id_matches};

const DEFAULT_COMPLETED_VIA: &str = "reply";

#[derive(Debug, Deserialize)]
struct MonitorTaskCompleteParams {
    #[serde(alias = "taskId")]
    task_id: String,
    #[serde(default, alias = "completedVia")]
    completed_via: Option<String>,
}

/// Marks a monitor-created task completed (e.g. after the user replied to it
/// from Bobo) and returns the refreshed task snapshot.
///
/// This is the deterministic completion writeback that monitor tasks otherwise
/// lack: a Bobo chat session runs under a different cwd than the daemon root, so
/// the agent's per-session `TaskUpdate` tool resolves the wrong (empty) store
/// and silently fails (agentenv/monorepo#562). The daemon owns
/// `monitor_tasks.json`, so the completion is written here instead.
///
/// Unlike `task_monitor_ignore`, this writes no ignore memory and installs no
/// ignore filter — it only flips the task's lifecycle to completed and records
/// how it was completed (`completed_via`, default "reply").
///
/// Idempotent: a task already in a terminal state (completed/cancelled) is left
/// untouched, so a duplicate or stray complete can never re-open or mutate an
/// already-handled task (notably it will not clobber an ignore-completed task).
pub(crate) fn handle_monitor_task_complete(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: MonitorTaskCompleteParams =
        serde_json::from_value(params.clone()).context("invalid monitor task complete params")?;
    let task_id = non_empty(params.task_id.as_str()).context("missing task_id")?;
    let completed_via = params
        .completed_via
        .as_deref()
        .and_then(non_empty)
        .unwrap_or(DEFAULT_COMPLETED_VIA)
        .to_string();

    let path = monitor_tasks_path(paths);
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut store: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid monitor task store {}", path.display()))?;
    let tasks = store
        .get_mut("tasks")
        .and_then(Value::as_array_mut)
        .context("monitor task store missing tasks array")?;
    let task = tasks
        .iter_mut()
        .find(|task| task_id_matches(task, task_id))
        .with_context(|| format!("monitor task `{task_id}` not found"))?;
    let task_object = task
        .as_object_mut()
        .context("monitor task entry must be an object")?;

    if !is_terminal(task_object) {
        task_object.insert("status".to_string(), Value::String("completed".to_string()));
        task_object.insert(
            "completed_via".to_string(),
            Value::String(completed_via),
        );
        task_object.insert("updated_at_ms".to_string(), Value::from(now_ms()));
        fs::write(&path, serde_json::to_string_pretty(&store)?)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    super::handle_workflow_list(paths)
}

/// Whether the task is already in a terminal lifecycle state. Only `pending`
/// (or any non-terminal) tasks transition to completed here; terminal tasks are
/// no-ops so the writeback is idempotent.
fn is_terminal(task: &serde_json::Map<String, Value>) -> bool {
    matches!(
        task.get("status").and_then(Value::as_str),
        Some("completed") | Some("cancelled")
    )
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
#[path = "monitor_task_complete_tests.rs"]
mod tests;
