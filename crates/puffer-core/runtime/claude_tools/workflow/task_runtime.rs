use super::store::{
    load_store, now_ms, process_is_running, save_store, tasks_path, StoredTask, TaskStore,
    TodoInputItem,
};
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Child;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const MAX_PROCESS_OUTPUT_CHARS: usize = 30_000;

/// Returns the persisted runtime-agent output path for a task id.
pub(super) fn runtime_agent_output_path(session_cwd: &Path, task_id: &str) -> std::path::PathBuf {
    ConfigPaths::discover(session_cwd)
        .workspace_config_dir
        .join("runtime")
        .join("agent_outputs")
        .join(format!("{task_id}.json"))
}

/// Loads the persisted runtime-agent output payload when it exists.
pub(super) fn read_runtime_agent_output(session_cwd: &Path, task_id: &str) -> Option<Value> {
    let raw = fs::read_to_string(runtime_agent_output_path(session_cwd, task_id)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Returns true when the runtime-agent output status is terminal.
pub(super) fn runtime_agent_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "stopped")
}

/// Waits for the persisted runtime-agent output to reach a terminal state.
pub(super) fn wait_for_runtime_agent_output(
    session_cwd: &Path,
    task_id: &str,
    timeout_ms: u64,
) -> (Option<Value>, bool) {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let payload = read_runtime_agent_output(session_cwd, task_id);
        if payload
            .as_ref()
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str)
            .is_some_and(runtime_agent_terminal_status)
        {
            return (payload, false);
        }
        if Instant::now() >= deadline {
            return (payload, true);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Validates the TodoWrite list shape and progress-state invariants.
pub(super) fn validate_todos(todos: &[TodoInputItem]) -> anyhow::Result<()> {
    if todos.is_empty() {
        anyhow::bail!("TodoWrite requires at least one todo item");
    }
    let mut in_progress = 0usize;
    for todo in todos {
        if todo.content.trim().is_empty() {
            anyhow::bail!("TodoWrite todo content cannot be empty");
        }
        if todo.active_form.trim().is_empty() {
            anyhow::bail!("TodoWrite activeForm cannot be empty");
        }
        match todo.status.as_str() {
            "pending" | "in_progress" | "completed" => {}
            other => anyhow::bail!("TodoWrite status `{other}` is not supported"),
        }
        if todo.status == "in_progress" {
            in_progress += 1;
        }
    }
    if in_progress > 1 {
        anyhow::bail!("TodoWrite supports at most one in_progress item at a time");
    }
    Ok(())
}

/// Captures child-process output together with timeout state.
pub(super) struct TimedProcessOutput {
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) timed_out: bool,
}

/// Returns true when the stored task status is terminal.
pub(super) fn terminal_task_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "stopped" | "deleted")
}

/// Reads persisted task output from disk or the in-memory cache.
pub(super) fn read_task_output(task: &StoredTask) -> Option<String> {
    task.output_file
        .as_deref()
        .and_then(|path| fs::read_to_string(path).ok())
        .filter(|text| !text.is_empty())
        .or_else(|| task.output.clone())
}

/// Refreshes one stored task from disk-backed process and output state.
pub(super) fn refresh_stored_task(
    store_cwd: &Path,
    session_id: &Uuid,
    task_id: &str,
) -> Result<Option<StoredTask>> {
    let tp = tasks_path(store_cwd, session_id);
    let mut store = load_store::<TaskStore>(&tp)?;
    let Some(task) = store.tasks.iter_mut().find(|task| task.task_id == task_id) else {
        return Ok(None);
    };

    let mut changed = false;
    if task.status == "running" {
        if let Some(process_id) = task.process_id {
            if !process_is_running(process_id) {
                task.process_id = None;
                task.status = "completed".to_string();
                task.output = read_task_output(task);
                task.updated_at_ms = Some(now_ms());
                changed = true;
            }
        }
    } else if task.output.is_none() {
        task.output = read_task_output(task);
        changed = task.output.is_some();
    }

    let output = task.clone();
    if changed {
        save_store(&tp, &store)?;
    }
    Ok(Some(output))
}

/// Waits until one stored task reaches a terminal state or the timeout elapses.
pub(super) fn wait_for_stored_task(
    store_cwd: &Path,
    session_id: &Uuid,
    task_id: &str,
    timeout_ms: u64,
) -> Result<(Option<StoredTask>, bool)> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let task = refresh_stored_task(store_cwd, session_id, task_id)?;
        if task
            .as_ref()
            .is_some_and(|task| terminal_task_status(&task.status))
        {
            return Ok((task, false));
        }
        if Instant::now() >= deadline {
            return Ok((task, true));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Waits for a child process to finish and captures its output with a timeout.
pub(super) fn wait_for_child_output(
    mut child: Child,
    timeout_ms: u64,
) -> Result<TimedProcessOutput> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if child
            .try_wait()
            .context("failed to poll child process")?
            .is_some()
        {
            let output = child
                .wait_with_output()
                .context("failed to collect child output")?;
            return Ok(TimedProcessOutput {
                stdout: truncate_process_output(
                    String::from_utf8_lossy(&output.stdout).to_string(),
                ),
                stderr: truncate_process_output(
                    String::from_utf8_lossy(&output.stderr).to_string(),
                ),
                timed_out: false,
            });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .context("failed to collect timed out child output")?;
            let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if !stderr.trim().is_empty() {
                stderr.push('\n');
            }
            stderr.push_str(&format!("command timed out after {timeout_ms}ms"));
            return Ok(TimedProcessOutput {
                stdout: truncate_process_output(
                    String::from_utf8_lossy(&output.stdout).to_string(),
                ),
                stderr: truncate_process_output(stderr),
                timed_out: true,
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn truncate_process_output(output: String) -> String {
    if output.chars().count() <= MAX_PROCESS_OUTPUT_CHARS {
        return output;
    }
    output.chars().take(MAX_PROCESS_OUTPUT_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::{truncate_process_output, MAX_PROCESS_OUTPUT_CHARS};

    #[test]
    fn truncate_process_output_leaves_short_text_unchanged() {
        let output = "ready".to_string();

        assert_eq!(truncate_process_output(output.clone()), output);
    }

    #[test]
    fn truncate_process_output_caps_long_text_at_reference_limit() {
        let output = "x".repeat(MAX_PROCESS_OUTPUT_CHARS + 128);
        let truncated = truncate_process_output(output);

        assert_eq!(truncated.chars().count(), MAX_PROCESS_OUTPUT_CHARS);
        assert!(truncated.chars().all(|ch| ch == 'x'));
    }
}
