use super::store::{
    agents_path, append_agent_message, ensure_safe_identifier, load_store, monitor_tasks_path,
    next_monitor_task_id, next_task_id, now_ms, save_store, tasks_path, team_lead_agent_id,
    terminate_process, wait_for_process_exit, AgentStore, StoredTask, TaskCreateInput, TaskIdInput,
    TaskOutputInput, TaskStopInput, TaskStore, TaskUpdateInput,
};
use super::task_runtime::{
    read_runtime_agent_output, read_task_output, refresh_stored_task, runtime_agent_output_path,
    runtime_agent_terminal_status, terminal_task_status, wait_for_runtime_agent_output,
    wait_for_stored_task,
};
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use serde_json::{Map, Value};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

/// Executes the live `TaskCreate` workflow tool.
pub(super) fn execute_task_create(
    state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: TaskCreateInput =
        serde_json::from_value(input).context("invalid TaskCreate input")?;
    validate_task_create_actions(&parsed)?;
    let mut metadata = parsed.metadata.unwrap_or_default();
    if !parsed.actions.is_empty() {
        metadata.insert("actions".to_string(), json!(parsed.actions));
    }
    if !parsed.possible_ignore_reasons.is_empty() {
        metadata.insert(
            "possibleIgnoreReasons".to_string(),
            json!(parsed.possible_ignore_reasons),
        );
    }
    let monitor_task = is_monitor_task_metadata(&metadata);
    let tp = if monitor_task {
        monitor_tasks_path(state.session.cwd.as_path())
    } else {
        tasks_path(state.session.cwd.as_path(), &state.session.id)
    };
    let mut store = load_store::<TaskStore>(&tp)?;
    let task = StoredTask {
        task_id: if monitor_task {
            next_monitor_task_id(&store.tasks)
        } else {
            next_task_id(&store.tasks)
        },
        subject: parsed.subject,
        description: parsed.description,
        active_form: parsed.active_form.unwrap_or_else(|| "Working".to_string()),
        status: "pending".to_string(),
        owner: None,
        blocks: Vec::new(),
        blocked_by: Vec::new(),
        metadata,
        output: None,
        task_type: Some("task".to_string()),
        command: None,
        process_id: None,
        output_file: None,
        started_at_ms: Some(now_ms()),
        updated_at_ms: Some(now_ms()),
        exit_code: None,
    };
    store.tasks.push(task.clone());
    save_store(&tp, &store)?;
    Ok(serde_json::to_string_pretty(&json!({
        "task": {
            "id": task.task_id,
            "subject": task.subject,
        }
    }))?)
}

/// Executes the live `TaskGet` workflow tool.
pub(super) fn execute_task_get(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: TaskIdInput = serde_json::from_value(input).context("invalid TaskGet input")?;
    let mut task = refresh_stored_task(
        state.session.cwd.as_path(),
        &state.session.id,
        &parsed.task_id,
    )?;
    if task.is_none() {
        task = load_monitor_task(state.session.cwd.as_path(), &parsed.task_id)?;
    }
    Ok(serde_json::to_string_pretty(&json!({
        "task": task.map(|task| {
            json!({
                "id": task.task_id,
                "subject": task.subject,
                "description": task.description,
                "status": task.status,
                "blocks": task.blocks,
                "blockedBy": task.blocked_by,
            })
        })
    }))?)
}

/// Executes the live `TaskList` workflow tool.
pub(super) fn execute_task_list(
    state: &mut AppState,
    _cwd: &Path,
    _input: Value,
) -> Result<String> {
    let store_cwd = state.session.cwd.as_path();
    let sid = &state.session.id;
    let tp = tasks_path(store_cwd, sid);
    let mut store = load_store::<TaskStore>(&tp)?;
    let monitor_tp = monitor_tasks_path(store_cwd);
    let mut monitor_store = load_store::<TaskStore>(&monitor_tp)?;
    let mut changed = false;
    for task in &mut store.tasks {
        let previous = task.clone();
        if let Some(updated) = refresh_stored_task(store_cwd, sid, &task.task_id)? {
            *task = updated;
            changed |= *task != previous;
        }
    }
    if changed {
        save_store(&tp, &store)?;
    }
    let mut monitor_changed = false;
    for task in &mut monitor_store.tasks {
        let previous = task.clone();
        if task.output.is_none() {
            task.output = read_task_output(task);
            monitor_changed |= task.output.is_some();
        }
        monitor_changed |= *task != previous;
    }
    if monitor_changed {
        save_store(&monitor_tp, &monitor_store)?;
    }
    let resolved = store
        .tasks
        .iter()
        .filter(|task| task.status == "completed")
        .map(|task| task.task_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let tasks = store
        .tasks
        .iter()
        .chain(monitor_store.tasks.iter())
        .filter(|task| {
            !task
                .metadata
                .get("_internal")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .map(|task| {
            json!({
                "id": task.task_id,
                "subject": task.subject,
                "status": task.status,
                "owner": task.owner,
                "blockedBy": task
                    .blocked_by
                    .iter()
                    .filter(|task_id| !resolved.contains(task_id.as_str()))
                    .cloned()
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&json!({ "tasks": tasks }))?)
}

/// Executes the live `TaskUpdate` workflow tool.
pub(super) fn execute_task_update(
    state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: TaskUpdateInput =
        serde_json::from_value(input).context("invalid TaskUpdate input")?;
    let store_cwd = state.session.cwd.clone();
    let tp = task_update_store_path(&store_cwd, &state.session.id, &parsed.task_id)?;
    let mut store = load_store::<TaskStore>(&tp)?;
    let Some(index) = store
        .tasks
        .iter()
        .position(|task| task.task_id == parsed.task_id)
    else {
        return Ok(serde_json::to_string_pretty(&json!({
            "success": false,
            "taskId": parsed.task_id,
            "updatedFields": [],
            "error": "Task not found",
        }))?);
    };
    let task_id = parsed.task_id.clone();
    let previous_status = store.tasks[index].status.clone();
    if parsed.status.as_deref() == Some("deleted") {
        store.tasks.remove(index);
        save_store(&tp, &store)?;
        return Ok(serde_json::to_string_pretty(&json!({
            "success": true,
            "taskId": task_id,
            "updatedFields": ["deleted"],
            "statusChange": {
                "from": previous_status,
                "to": "deleted",
            }
        }))?);
    }

    let task = &mut store.tasks[index];
    let mut updated_fields = Vec::new();
    let mut status_change = None;
    if let Some(subject) = parsed.subject.filter(|subject| *subject != task.subject) {
        task.subject = subject;
        updated_fields.push("subject");
    }
    if let Some(description) = parsed
        .description
        .filter(|description| *description != task.description)
    {
        task.description = description;
        updated_fields.push("description");
    }
    if let Some(active_form) = parsed
        .active_form
        .filter(|active_form| *active_form != task.active_form)
    {
        task.active_form = active_form;
        updated_fields.push("activeForm");
    }
    if let Some(owner) = parsed
        .owner
        .filter(|owner| task.owner.as_deref() != Some(owner.as_str()))
    {
        task.owner = Some(owner);
        updated_fields.push("owner");
    }
    if let Some(status) = parsed.status.filter(|status| *status != task.status) {
        task.status = status;
        if task.status == "in_progress" && task.started_at_ms.is_none() {
            task.started_at_ms = Some(now_ms());
        }
        if matches!(task.status.as_str(), "completed" | "failed" | "stopped") {
            task.process_id = None;
        }
        status_change = Some(json!({
            "from": previous_status,
            "to": task.status,
        }));
        updated_fields.push("status");
    }
    // Auto-set owner when transitioning to in_progress without an explicit owner.
    if task.status == "in_progress" && task.owner.is_none() {
        if let Some(ref team_name) = state.active_team_name {
            task.owner = Some(team_lead_agent_id(team_name));
            if !updated_fields.contains(&"owner") {
                updated_fields.push("owner");
            }
        }
    }
    let mut added_blocks = false;
    for block in parsed.add_blocks {
        if !task.blocks.iter().any(|existing| existing == &block) {
            task.blocks.push(block);
            added_blocks = true;
        }
    }
    if added_blocks {
        updated_fields.push("blocks");
    }
    let mut added_blocked_by = false;
    for blocked_by in parsed.add_blocked_by {
        if !task
            .blocked_by
            .iter()
            .any(|existing| existing == &blocked_by)
        {
            task.blocked_by.push(blocked_by);
            added_blocked_by = true;
        }
    }
    if added_blocked_by {
        updated_fields.push("blockedBy");
    }
    if let Some(metadata) = parsed.metadata {
        let before = task.metadata.clone();
        for (key, value) in metadata {
            if value.is_null() {
                task.metadata.remove(&key);
            } else {
                task.metadata.insert(key, value);
            }
        }
        if task.metadata != before {
            updated_fields.push("metadata");
        }
    }
    task.updated_at_ms = Some(now_ms());
    save_store(&tp, &store)?;
    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "taskId": task_id,
        "updatedFields": updated_fields,
        "statusChange": status_change,
    }))?)
}

fn validate_task_create_actions(parsed: &TaskCreateInput) -> Result<()> {
    for action in &parsed.actions {
        if action.action_name.trim().is_empty() {
            bail!("TaskCreate actionName cannot be empty");
        }
        if action.action_prompt.trim().is_empty() {
            bail!("TaskCreate actionPrompt cannot be empty");
        }
    }
    for reason in &parsed.possible_ignore_reasons {
        if reason.trim().is_empty() {
            bail!("TaskCreate possibleIgnoreReasons cannot contain empty values");
        }
    }
    Ok(())
}

fn is_monitor_task_metadata(metadata: &Map<String, Value>) -> bool {
    metadata
        .get("_monitor")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || metadata.contains_key("monitor_connection")
        || metadata.contains_key("monitorConnection")
}

fn load_monitor_task(cwd: &Path, task_id: &str) -> Result<Option<StoredTask>> {
    let store = load_store::<TaskStore>(&monitor_tasks_path(cwd))?;
    Ok(store.tasks.into_iter().find(|task| task.task_id == task_id))
}

fn task_update_store_path(
    cwd: &Path,
    session_id: &uuid::Uuid,
    task_id: &str,
) -> Result<std::path::PathBuf> {
    let session_path = tasks_path(cwd, session_id);
    let session_store = load_store::<TaskStore>(&session_path)?;
    if session_store
        .tasks
        .iter()
        .any(|task| task.task_id == task_id)
    {
        return Ok(session_path);
    }
    let monitor_path = monitor_tasks_path(cwd);
    let monitor_store = load_store::<TaskStore>(&monitor_path)?;
    if monitor_store
        .tasks
        .iter()
        .any(|task| task.task_id == task_id)
    {
        return Ok(monitor_path);
    }
    Ok(session_path)
}

/// Executes the live `TaskStop` workflow tool.
pub(super) fn execute_task_stop(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: TaskStopInput = serde_json::from_value(input).context("invalid TaskStop input")?;
    let target = parsed
        .task_id
        .or(parsed.shell_id)
        .ok_or_else(|| anyhow!("TaskStop requires task_id or shell_id"))?;
    ensure_safe_identifier(&target, "task_id")?;

    let store_cwd = state.session.cwd.as_path();
    let tp = tasks_path(store_cwd, &state.session.id);
    let mut tasks = load_store::<TaskStore>(&tp)?;
    if let Some(task) = tasks.tasks.iter_mut().find(|task| task.task_id == target) {
        if task.process_id.is_none() && task.command.is_none() && task.output_file.is_none() {
            bail!("task `{target}` is not a running background task");
        }
        if terminal_task_status(&task.status) {
            bail!("task `{target}` is not running (status: {})", task.status);
        }
        if let Some(process_id) = task.process_id {
            terminate_process(process_id)?;
            let _ = wait_for_process_exit(process_id, 1_000);
            task.process_id = None;
        }
        if let Some(output) = read_task_output(task) {
            task.output = Some(output);
        }
        task.status = "stopped".to_string();
        if task.output.as_deref().unwrap_or_default().trim().is_empty() {
            task.output = Some("Stopped by TaskStop.".to_string());
        }
        let task_id = task.task_id.clone();
        let task_type = task.task_type.clone().unwrap_or_else(|| "task".to_string());
        let command = task.command.clone();
        save_store(&tp, &tasks)?;
        return Ok(serde_json::to_string_pretty(&json!({
            "message": format!("Successfully stopped task: {task_id}"),
            "task_id": task_id,
            "task_type": task_type,
            "command": command,
        }))?);
    }

    let mut agents = load_store::<AgentStore>(&agents_path(store_cwd))?;
    if let Some(agent) = agents
        .agents
        .iter_mut()
        .find(|agent| agent.agent_id == target)
    {
        if terminal_task_status(&agent.status) {
            bail!("task `{target}` is not running (status: {})", agent.status);
        }
        agent.status = "stopped".to_string();
        append_agent_message(
            Path::new(&agent.output_file),
            &json!("Stopped by TaskStop."),
        )?;
        let output = json!({
            "message": format!("Successfully stopped task: {target}"),
            "task_id": target,
            "task_type": "agent",
            "status": agent.status,
            "output_file": agent.output_file,
            "command": agent.prompt,
        });
        save_store(&agents_path(store_cwd), &agents)?;
        return Ok(serde_json::to_string_pretty(&output)?);
    }

    bail!("unknown task `{}`", target)
}

/// Executes the live `TaskOutput` workflow tool.
pub(super) fn execute_task_output(
    state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: TaskOutputInput =
        serde_json::from_value(input).context("invalid TaskOutput input")?;
    ensure_safe_identifier(&parsed.task_id, "task_id")?;
    let store_cwd = state.session.cwd.as_path();
    let sid = &state.session.id;
    let block = parsed.block.unwrap_or(true);
    let timeout = parsed.timeout.unwrap_or(30_000);
    let (task, timed_out) = if block {
        wait_for_stored_task(store_cwd, sid, &parsed.task_id, timeout)?
    } else {
        (refresh_stored_task(store_cwd, sid, &parsed.task_id)?, false)
    };
    if let Some(task) = task {
        let mut task_payload = json!({
            "task_id": task.task_id,
            "task_type": task.task_type,
            "status": task.status,
            "description": task.description,
            "output": read_task_output(&task),
        });
        if let Some(exit_code) = task.exit_code {
            task_payload["exitCode"] = json!(exit_code);
        }
        if let Some(command) = task.command {
            task_payload["command"] = json!(command);
        }
        if let Some(output_file) = task.output_file {
            task_payload["outputFile"] = json!(output_file);
        }
        return task_output_response(
            if timed_out {
                "timeout"
            } else if terminal_task_status(
                task_payload
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("running"),
            ) {
                "success"
            } else {
                "not_ready"
            },
            task_payload,
            None,
            block,
            timeout,
        );
    }
    let agents = load_store::<AgentStore>(&agents_path(store_cwd))?;
    if let Some(agent) = agents
        .agents
        .iter()
        .find(|agent| agent.agent_id == parsed.task_id)
    {
        let mut status = agent.status.clone();
        let deadline = Instant::now() + Duration::from_millis(timeout);
        while block && !terminal_task_status(&status) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            status = load_store::<AgentStore>(&agents_path(store_cwd))?
                .agents
                .into_iter()
                .find(|candidate| candidate.agent_id == parsed.task_id)
                .map(|candidate| candidate.status)
                .unwrap_or(status);
        }
        let output = fs::read_to_string(&agent.output_file).unwrap_or_default();
        let task_payload = json!({
            "task_id": agent.agent_id,
            "task_type": "agent",
            "status": status,
            "description": agent.description,
            "output": output.clone(),
            "prompt": agent.prompt,
            "result": output,
            "outputFile": agent.output_file,
        });
        return task_output_response(
            if terminal_task_status(
                task_payload
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("running"),
            ) {
                "success"
            } else if block {
                "timeout"
            } else {
                "not_ready"
            },
            task_payload,
            None,
            block,
            timeout,
        );
    }

    let (agent_payload, timed_out) = if block {
        wait_for_runtime_agent_output(store_cwd, &parsed.task_id, timeout)
    } else {
        (read_runtime_agent_output(store_cwd, &parsed.task_id), false)
    };
    if let Some(agent_payload) = agent_payload {
        let status = agent_payload
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("running");
        let output = agent_payload
            .get("result")
            .and_then(Value::as_str)
            .or_else(|| agent_payload.get("error").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string_pretty(&agent_payload).unwrap_or_default());
        let mut task_payload = json!({
            "task_id": parsed.task_id,
            "task_type": "agent",
            "status": status,
            "description": agent_payload
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            "output": output,
        });
        if let Some(prompt) = agent_payload.get("prompt").and_then(Value::as_str) {
            task_payload["prompt"] = json!(prompt);
        }
        if let Some(result) = agent_payload.get("result").and_then(Value::as_str) {
            task_payload["result"] = json!(result);
        }
        if let Some(error) = agent_payload.get("error").and_then(Value::as_str) {
            task_payload["error"] = json!(error);
        }
        task_payload["outputFile"] = json!(runtime_agent_output_path(store_cwd, &parsed.task_id)
            .display()
            .to_string());
        return task_output_response(
            if timed_out {
                "timeout"
            } else if runtime_agent_terminal_status(status) {
                "success"
            } else {
                "not_ready"
            },
            task_payload,
            Some(
                runtime_agent_output_path(store_cwd, &parsed.task_id)
                    .display()
                    .to_string(),
            ),
            block,
            timeout,
        );
    }

    bail!("unknown task `{}`", parsed.task_id)
}

pub(crate) fn task_output_response(
    retrieval_status: &str,
    mut task: Value,
    output_file: Option<String>,
    _block: bool,
    _timeout: u64,
) -> Result<String> {
    if task.get("outputFile").is_none() {
        if let Some(output_file) = output_file {
            task["outputFile"] = json!(output_file);
        }
    }
    Ok(serde_json::to_string_pretty(&json!({
        "retrieval_status": retrieval_status,
        "task": task,
    }))?)
}
