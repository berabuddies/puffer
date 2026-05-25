use super::store::{
    ensure_safe_identifier, load_store, save_store, tasks_path, terminate_process,
    wait_for_process_exit, StoredTask, TaskStore,
};
use super::task_runtime::{read_task_output, refresh_stored_task, terminal_task_status};
use crate::AppState;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

const DEFAULT_WAIT_MS: u64 = 1_000;
const MAX_WAIT_MS: u64 = 30_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProcessControlAction {
    List,
    Poll,
    Log,
    Kill,
    InterruptOrKill,
}

#[derive(Debug, Deserialize)]
struct ProcessControlInput {
    action: ProcessControlAction,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default)]
    block: Option<bool>,
    #[serde(default)]
    timeout: Option<u64>,
}

/// Executes the typed process bridge used by verified Lambda skill contracts.
pub fn execute_process_control(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: ProcessControlInput =
        serde_json::from_value(input).context("invalid ProcessControl input")?;
    match parsed.action {
        ProcessControlAction::List => process_list(state),
        ProcessControlAction::Poll => {
            let session_id = required_session_id(&parsed)?;
            process_poll(
                state,
                &session_id,
                parsed.block.unwrap_or(false),
                parsed.timeout,
            )
        }
        ProcessControlAction::Log => {
            let session_id = required_session_id(&parsed)?;
            process_log(state, &session_id)
        }
        ProcessControlAction::Kill => {
            let session_id = required_session_id(&parsed)?;
            process_kill(state, &session_id, false)
        }
        ProcessControlAction::InterruptOrKill => {
            let session_id = required_session_id(&parsed)?;
            process_kill(state, &session_id, true)
        }
    }
}

fn required_session_id(input: &ProcessControlInput) -> Result<String> {
    input
        .session_id
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("ProcessControl action requires sessionId"))
}

fn process_list(state: &mut AppState) -> Result<String> {
    let processes = state
        .process_store
        .lock()
        .unwrap()
        .snapshots()
        .into_iter()
        .map(|entry| {
            json!({
                "sessionId": entry.process_id.to_string(),
                "kind": "interactive_process",
                "command": entry.command,
                "tty": entry.tty,
                "status": if entry.exited { "exited" } else { "running" },
                "exitCode": entry.exit_code,
                "outputBytes": entry.output_bytes,
            })
        })
        .collect::<Vec<_>>();

    let tasks =
        load_store::<TaskStore>(&tasks_path(state.session.cwd.as_path(), &state.session.id))
            .unwrap_or_default()
            .tasks
            .into_iter()
            .filter(|task| task.process_id.is_some() || task.output_file.is_some())
            .map(task_summary)
            .collect::<Vec<_>>();

    Ok(serde_json::to_string_pretty(&json!({
        "processes": processes,
        "tasks": tasks,
    }))?)
}

fn process_poll(
    state: &mut AppState,
    session_id: &str,
    block: bool,
    timeout: Option<u64>,
) -> Result<String> {
    if let Some(process_id) = parse_process_id(session_id) {
        let timeout = timeout.unwrap_or(DEFAULT_WAIT_MS).min(MAX_WAIT_MS);
        if block {
            wait_for_process_state(state, process_id, timeout);
        }
        let guard = state.process_store.lock().unwrap();
        let Some(entry) = guard.peek(process_id) else {
            bail!("process `{session_id}` not found");
        };
        return Ok(serde_json::to_string_pretty(&json!({
            "sessionId": session_id,
            "kind": "interactive_process",
            "status": if entry.has_exited() { "exited" } else { "running" },
            "exitCode": entry.exit_code(),
            "outputBytes": entry.total_output_bytes(),
            "command": entry.command,
            "tty": entry.tty,
        }))?);
    }

    let task = refresh_required_task(state, session_id)?;
    Ok(serde_json::to_string_pretty(&json!({
        "sessionId": session_id,
        "kind": "background_task",
        "status": task.status,
        "exitCode": task.exit_code,
        "outputFile": task.output_file,
        "command": task.command,
    }))?)
}

fn process_log(state: &mut AppState, session_id: &str) -> Result<String> {
    if let Some(process_id) = parse_process_id(session_id) {
        let guard = state.process_store.lock().unwrap();
        let Some(entry) = guard.peek(process_id) else {
            bail!("process `{session_id}` not found");
        };
        let output = String::from_utf8_lossy(&entry.collect_output()).to_string();
        return Ok(serde_json::to_string_pretty(&json!({
            "sessionId": session_id,
            "kind": "interactive_process",
            "status": if entry.has_exited() { "exited" } else { "running" },
            "exitCode": entry.exit_code(),
            "output": output,
        }))?);
    }

    let task = refresh_required_task(state, session_id)?;
    Ok(serde_json::to_string_pretty(&json!({
        "sessionId": session_id,
        "kind": "background_task",
        "status": task.status,
        "exitCode": task.exit_code,
        "outputFile": task.output_file,
        "output": read_task_output(&task),
    }))?)
}

fn process_kill(state: &mut AppState, session_id: &str, interrupt_first: bool) -> Result<String> {
    if let Some(process_id) = parse_process_id(session_id) {
        let mut guard = state.process_store.lock().unwrap();
        let Some(mut entry) = guard.remove(process_id) else {
            bail!("process `{session_id}` not found");
        };
        if interrupt_first {
            let _ = entry.write_stdin(b"\x03");
            std::thread::sleep(Duration::from_millis(250));
        }
        entry.terminate();
        return Ok(serde_json::to_string_pretty(&json!({
            "sessionId": session_id,
            "kind": "interactive_process",
            "status": if interrupt_first { "interrupted" } else { "killed" },
        }))?);
    }

    stop_required_task(state, session_id)
}

fn refresh_required_task(state: &mut AppState, session_id: &str) -> Result<StoredTask> {
    ensure_safe_identifier(session_id, "sessionId")?;
    refresh_stored_task(state.session.cwd.as_path(), &state.session.id, session_id)?
        .ok_or_else(|| anyhow::anyhow!("background task `{session_id}` not found"))
}

fn stop_required_task(state: &mut AppState, session_id: &str) -> Result<String> {
    ensure_safe_identifier(session_id, "sessionId")?;
    let store_cwd = state.session.cwd.as_path();
    let tp = tasks_path(store_cwd, &state.session.id);
    let mut store = load_store::<TaskStore>(&tp)?;
    let Some(task) = store
        .tasks
        .iter_mut()
        .find(|task| task.task_id == session_id)
    else {
        bail!("background task `{session_id}` not found");
    };
    if terminal_task_status(&task.status) {
        bail!(
            "background task `{session_id}` is not running (status: {})",
            task.status
        );
    }
    if let Some(process_id) = task.process_id {
        terminate_process(process_id)?;
        let _ = wait_for_process_exit(process_id, 1_000);
        task.process_id = None;
    }
    task.output = Some(read_task_output(task).unwrap_or_default());
    task.status = "stopped".to_string();
    task.updated_at_ms = Some(super::store::now_ms());
    let output = json!({
        "sessionId": session_id,
        "kind": "background_task",
        "status": task.status,
        "outputFile": task.output_file,
        "command": task.command,
    });
    save_store(&tp, &store)?;
    Ok(serde_json::to_string_pretty(&output)?)
}

fn parse_process_id(session_id: &str) -> Option<i32> {
    session_id.parse::<i32>().ok().filter(|id| *id >= 1000)
}

fn wait_for_process_state(state: &mut AppState, process_id: i32, timeout_ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let exited = state
            .process_store
            .lock()
            .unwrap()
            .peek(process_id)
            .map(|entry| entry.has_exited())
            .unwrap_or(true);
        if exited {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn task_summary(task: StoredTask) -> Value {
    json!({
        "sessionId": task.task_id,
        "kind": task.task_type.unwrap_or_else(|| "task".to_string()),
        "status": task.status,
        "command": task.command,
        "outputFile": task.output_file,
        "exitCode": task.exit_code,
    })
}
