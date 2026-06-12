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
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

/// Executes the live `TaskCreate` workflow tool.
pub(super) fn execute_task_create(
    state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: TaskCreateInput =
        serde_json::from_value(input).context("invalid TaskCreate input")?;
    validate_task_create_actions(&parsed)?;
    let received_at = parse_rfc3339_field(parsed.received_at.as_deref(), "receivedAt")?;
    let expires_at = parse_rfc3339_field(parsed.expires_at.as_deref(), "expiresAt")?;
    if let (Some((_, received)), Some((_, expires))) = (&received_at, &expires_at) {
        if expires <= received {
            bail!("TaskCreate expiresAt must be after receivedAt");
        }
    }
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
    if monitor_task {
        normalize_monitor_task_metadata(&mut metadata);
        validate_monitor_task_metadata(&metadata)?;
    }
    if monitor_task && received_at.is_none() {
        bail!("monitor TaskCreate requires receivedAt in RFC3339 format");
    }
    if monitor_task && expires_at.is_none() {
        bail!("monitor TaskCreate requires expiresAt in RFC3339 format");
    }
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
        received_at: received_at.map(|(value, _)| value),
        expires_at: expires_at.map(|(value, _)| value),
        started_at_ms: Some(now_ms()),
        created_at_ms: Some(now_ms()),
        updated_at_ms: Some(now_ms()),
        exit_code: None,
    };
    store.tasks.push(task.clone());
    save_store(&tp, &store)?;
    Ok(serde_json::to_string_pretty(&json!({
        "task": {
            "id": task.task_id,
            "subject": task.subject,
            "receivedAt": task.received_at,
            "expiresAt": task.expires_at,
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
            let source_context = monitor_source_context(&task.metadata);
            let completion_policy =
                monitor_completion_policy(&task.metadata, source_context.as_ref());
            json!({
                "id": task.task_id,
                "subject": task.subject,
                "description": task.description,
                "status": task.status,
                "blocks": task.blocks,
                "blockedBy": task.blocked_by,
                "receivedAt": task.received_at,
                "expiresAt": task.expires_at,
                "monitorConnection": metadata_string(&task.metadata, &["monitor_connection", "monitorConnection"]),
                "monitorConnector": metadata_string(&task.metadata, &["monitor_connector", "monitorConnector"]),
                "sourceContext": source_context.map(camel_case_source_context),
                "completionPolicy": completion_policy,
                "monitorActions": monitor_actions(&task.metadata),
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
                "receivedAt": task.received_at,
                "expiresAt": task.expires_at,
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
    let metadata_update = if let Some(metadata) = parsed.metadata.as_ref() {
        let updating_monitor_task = tp == monitor_tasks_path(&store_cwd)
            || is_monitor_task_metadata(&task.metadata)
            || is_monitor_task_metadata(metadata);
        if updating_monitor_task {
            validate_monitor_task_metadata(metadata)?;
            Some(sanitize_monitor_task_metadata_update(
                metadata,
                &task.metadata,
            )?)
        } else {
            Some(metadata.clone())
        }
    } else {
        None
    };
    if parsed.status.as_deref() == Some("completed")
        && monitor_task_is_human_gated(task)
        && !metadata_marks_monitor_ignored(metadata_update.as_ref())
    {
        bail!(
            "monitor task `{}` must be completed through its monitor action after human approval",
            parsed.task_id
        );
    }
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
    if let Some(metadata) = metadata_update {
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

#[derive(Debug, Deserialize)]
struct MonitorReplySendInput {
    #[serde(rename = "taskId")]
    task_id: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct MonitorReplyDraftInput {
    #[serde(rename = "taskId")]
    task_id: String,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MonitorReplyTarget {
    connector_slug: String,
    connection_slug: String,
    chat_id: String,
}

/// Sends a monitor-task reply to the task's recorded source target, then
/// completes the monitor task with a delivery receipt.
pub(super) fn execute_monitor_reply_send(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: MonitorReplySendInput =
        serde_json::from_value(input).context("invalid MonitorReplySend input")?;
    let message = parsed.message.trim();
    if message.is_empty() {
        bail!("MonitorReplySend message cannot be empty");
    }
    let store_cwd = state.session.cwd.clone();
    let task = load_monitor_task(store_cwd.as_path(), &parsed.task_id)?
        .ok_or_else(|| anyhow!("monitor task `{}` not found", parsed.task_id))?;
    if monitor_task_is_human_gated(&task) {
        append_monitor_reply_audit_to_store(
            store_cwd.as_path(),
            &parsed.task_id,
            "send_rejected",
            json!({"reason": "requires_human_approval"}),
        )?;
        bail!(
            "monitor task `{}` requires human approval; use the Bobo review flow",
            parsed.task_id
        );
    }
    if let Some(receipt) = monitor_reply_receipt(&task.metadata) {
        return Ok(serde_json::to_string_pretty(&json!({
            "success": true,
            "taskId": parsed.task_id,
            "alreadySent": true,
            "status": task.status,
            "receipt": receipt,
        }))?);
    }
    if monitor_reply_terminal_status(&task.status) {
        bail!(
            "monitor task `{}` is already {}; not sending monitor reply",
            parsed.task_id,
            task.status
        );
    }
    let target = monitor_reply_target(&task)?;
    let connector_input = monitor_reply_connector_act_input(&target, message);
    let raw = super::connector_tools::execute_connector_act(state, cwd, connector_input)?;
    let connector_output: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!(raw));
    let mut store = load_store::<TaskStore>(&monitor_tasks_path(store_cwd.as_path()))?;
    let Some(task) = store
        .tasks
        .iter_mut()
        .find(|task| task.task_id == parsed.task_id)
    else {
        bail!("monitor task `{}` not found after send", parsed.task_id);
    };
    append_monitor_action_receipt(task, &target, &connector_output)?;
    let previous_status = task.status.clone();
    task.status = "completed".to_string();
    task.process_id = None;
    task.updated_at_ms = Some(now_ms());
    save_store(&monitor_tasks_path(store_cwd.as_path()), &store)?;
    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "taskId": parsed.task_id,
        "statusChange": {
            "from": previous_status,
            "to": "completed",
        },
        "sentTo": {
            "type": "telegram_chat",
            "chatId": target.chat_id,
            "connectionSlug": target.connection_slug,
            "connectorSlug": target.connector_slug,
        },
        "connectorOutput": connector_output,
    }))?)
}

fn monitor_reply_terminal_status(status: &str) -> bool {
    terminal_task_status(status) || matches!(status, "cancelled" | "canceled")
}

/// Saves a draft reply for a daemon-validated monitor action turn.
pub(super) fn execute_monitor_reply_draft(
    state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: MonitorReplyDraftInput =
        serde_json::from_value(input).context("invalid MonitorReplyDraft input")?;
    let message = parsed.message.trim();
    if message.is_empty() {
        bail!("MonitorReplyDraft message cannot be empty");
    }
    let scope = state
        .monitor_reply_scope
        .clone()
        .ok_or_else(|| anyhow!("MonitorReplyDraft requires a monitor reply scope"))?;
    if parsed.task_id != scope.task_id {
        bail!(
            "MonitorReplyDraft taskId `{}` does not match scoped monitor task `{}`",
            parsed.task_id,
            scope.task_id
        );
    }

    let store_cwd = state.session.cwd.clone();
    let path = monitor_tasks_path(store_cwd.as_path());
    let mut store = load_store::<TaskStore>(&path)?;
    let Some(task) = store
        .tasks
        .iter_mut()
        .find(|task| task.task_id == parsed.task_id)
    else {
        bail!("monitor task `{}` not found", parsed.task_id);
    };
    // Reject only TERMINAL states, mirroring monitor_reply_send. `in_progress`
    // is a legitimate working state — the action agent drafting the reply (or
    // a triage TaskUpdate) may have marked the task active; requiring exactly
    // `pending` dead-ended the whole human-gated flow whenever that happened
    // (agentenv/monorepo#619 follow-up).
    if matches!(
        task.status.as_str(),
        "completed" | "cancelled" | "canceled" | "failed" | "stopped"
    ) {
        bail!(
            "MonitorReplyDraft expected an open monitor task `{}`, got terminal status `{}`",
            parsed.task_id,
            task.status
        );
    }
    if !monitor_task_is_human_gated(task) {
        bail!(
            "MonitorReplyDraft expected a human-gated monitor task `{}`",
            parsed.task_id
        );
    }
    let source_context = monitor_source_context(&task.metadata)
        .ok_or_else(|| anyhow!("monitor task `{}` has no source_context", parsed.task_id))?;
    // Validate the target from the server-owned source context before saving a
    // draft; the model never supplies recipient fields.
    let target = monitor_reply_target(task)?;
    let previous = task
        .metadata
        .get("pending_reply")
        .and_then(Value::as_object)
        .cloned();
    if let Some(previous_status) = previous
        .as_ref()
        .and_then(|draft| draft.get("status"))
        .and_then(Value::as_str)
    {
        if matches!(previous_status, "sending" | "sent") {
            bail!("cannot supersede monitor reply draft in `{previous_status}` state");
        }
    }
    let previous_version = previous
        .as_ref()
        .and_then(|draft| draft.get("version"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let now = now_rfc3339()?;
    let draft_id = format!("draft-{}-{}", parsed.task_id, now_ms());
    let version = previous_version + 1;
    if let Some(previous) = previous {
        append_monitor_reply_audit(
            task,
            "draft_superseded",
            json!({"previousStatus": previous.get("status").cloned()}),
        );
    }
    let source_hash = source_context_hash(&source_context)?;
    task.metadata.insert(
        "pending_reply".to_string(),
        json!({
            "id": draft_id,
            "created_by": "MonitorReplyDraft",
            "status": "draft_ready",
            "version": version,
            "agent_draft_text": message,
            "created_at": now,
            "updated_at": now,
            "session_id": scope.session_id,
            "turn_id": scope.turn_id,
            "source_context_snapshot": source_context,
            "source_context_hash": source_hash,
            "approved_message": Value::Null,
            "approved_by": Value::Null,
            "approved_at": Value::Null,
            "client_request_id": Value::Null,
            "send_attempt_id": Value::Null,
            "receipt": Value::Null,
            "error": Value::Null,
        }),
    );
    task.updated_at_ms = Some(now_ms());
    append_monitor_reply_audit(
        task,
        "draft_created",
        json!({
            "draft_id": draft_id,
            "version": version,
            "session_id": scope.session_id,
            "turn_id": scope.turn_id,
            "source_context_hash": source_hash,
            "chat_id": target.chat_id,
        }),
    );
    save_store(&path, &store)?;

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "taskId": parsed.task_id,
        "draft": {
            "id": draft_id,
            "status": "draft_ready",
            "version": version,
        }
    }))?)
}

fn parse_rfc3339_field(
    value: Option<&str>,
    field_name: &str,
) -> Result<Option<(String, OffsetDateTime)>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .with_context(|| format!("TaskCreate {field_name} must be an RFC3339 timestamp"))?;
    let normalized = parsed
        .format(&Rfc3339)
        .with_context(|| format!("failed to format TaskCreate {field_name}"))?;
    Ok(Some((normalized, parsed)))
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

fn validate_monitor_task_metadata(metadata: &Map<String, Value>) -> Result<()> {
    for key in [
        "monitor_ignore_filter",
        "monitorIgnoreFilter",
        "event_ignore_filter",
        "eventIgnoreFilter",
        "ignore_filter",
        "ignoreFilter",
        "ignore_filters",
        "ignoreFilters",
    ] {
        if metadata.contains_key(key) {
            bail!("monitor task metadata cannot include ignore filter field `{key}`");
        }
    }
    for key in [
        "pending_reply",
        "pendingReply",
        "monitor_reply_events",
        "monitorReplyEvents",
    ] {
        if metadata.contains_key(key) {
            bail!("monitor task metadata cannot include reserved field `{key}`");
        }
    }
    Ok(())
}

fn sanitize_monitor_task_metadata_update(
    metadata: &Map<String, Value>,
    existing: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let mut sanitized = Map::new();
    for (key, value) in metadata {
        if let Some(reserved_keys) = reserved_monitor_metadata_keys(key) {
            if reserved_monitor_value_unchanged(value, existing, reserved_keys) {
                continue;
            }
            bail!("reserved monitor metadata field `{key}` cannot be updated by TaskUpdate");
        }
        sanitized.insert(key.clone(), value.clone());
    }
    Ok(sanitized)
}

fn reserved_monitor_value_unchanged(
    value: &Value,
    existing: &Map<String, Value>,
    keys: &[&str],
) -> bool {
    let existing_value = keys.iter().find_map(|key| existing.get(*key));
    if value.is_null() {
        return existing_value.is_none();
    }
    existing_value.is_some_and(|existing_value| existing_value == value)
}

fn reserved_monitor_metadata_keys(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "source_context" | "sourceContext" => Some(&["source_context", "sourceContext"]),
        "completion_policy" | "completionPolicy" => {
            Some(&["completion_policy", "completionPolicy"])
        }
        "delivery_target" | "deliveryTarget" => Some(&["delivery_target", "deliveryTarget"]),
        "action_receipts" | "actionReceipts" => Some(&["action_receipts", "actionReceipts"]),
        "action_states" | "actionStates" => Some(&["action_states", "actionStates"]),
        "monitor_actions" | "monitorActions" => Some(&["monitor_actions", "monitorActions"]),
        "monitor_envelope_id" | "monitorEnvelopeId" => {
            Some(&["monitor_envelope_id", "monitorEnvelopeId"])
        }
        "monitor_connection" | "monitorConnection" => {
            Some(&["monitor_connection", "monitorConnection"])
        }
        "monitor_connector" | "monitorConnector" => {
            Some(&["monitor_connector", "monitorConnector"])
        }
        "chat_id" | "chatId" => Some(&["chat_id", "chatId"]),
        "chat_kind" | "chatKind" => Some(&["chat_kind", "chatKind"]),
        "sender_id" | "senderId" => Some(&["sender_id", "senderId"]),
        "sender_username" | "senderUsername" => Some(&["sender_username", "senderUsername"]),
        "pending_reply" | "pendingReply" => Some(&["pending_reply", "pendingReply"]),
        "monitor_reply_events" | "monitorReplyEvents" => {
            Some(&["monitor_reply_events", "monitorReplyEvents"])
        }
        "source_context_hash" | "sourceContextHash" => {
            Some(&["source_context_hash", "sourceContextHash"])
        }
        _ => None,
    }
}

fn normalize_monitor_task_metadata(metadata: &mut Map<String, Value>) {
    let source_context = derived_monitor_source_context(metadata);
    if let Some(source_context) = source_context {
        let default_completion_policy =
            default_monitor_completion_policy(metadata, Some(&source_context));
        metadata.insert("source_context".to_string(), source_context);
        if let Some(default_completion_policy) = default_completion_policy {
            metadata
                .entry("completion_policy".to_string())
                .or_insert(default_completion_policy);
        }
    } else {
        metadata.remove("source_context");
    }
}

fn monitor_source_context(metadata: &Map<String, Value>) -> Option<Value> {
    let context = metadata
        .get("source_context")
        .or_else(|| metadata.get("sourceContext"))
        .cloned()
        .or_else(|| derived_monitor_source_context(metadata));
    with_verbatim_source_text(metadata, context)
}

/// Surfaces the server-stamped verbatim event text (`metadata.source_text`,
/// written by the triage runner) as `source_context.text` when the stored or
/// derived context lacks one, so reply drafts and approval flows quote the
/// original wording rather than an LLM paraphrase (agentenv/monorepo#619).
fn with_verbatim_source_text(
    metadata: &Map<String, Value>,
    context: Option<Value>,
) -> Option<Value> {
    let mut context = context?;
    if let Some(object) = context.as_object_mut() {
        let has_text = object
            .get("text")
            .and_then(Value::as_str)
            .map_or(false, |value| !value.trim().is_empty());
        if !has_text {
            if let Some(text) = metadata
                .get("source_text")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                object.insert("text".to_string(), Value::String(text.to_string()));
            }
        }
        if object.get("message_id").and_then(Value::as_i64).is_none() {
            if let Some(message_id) = metadata.get("source_message_id").and_then(Value::as_i64) {
                object.insert("message_id".to_string(), Value::from(message_id));
            }
        }
    }
    Some(context)
}

fn derived_monitor_source_context(metadata: &Map<String, Value>) -> Option<Value> {
    let connector_slug = metadata_string(metadata, &["monitor_connector", "monitorConnector"])?;
    if !connector_slug.contains("telegram") {
        return None;
    }
    let chat_id = metadata_string(metadata, &["chat_id", "chatId"])?;
    let chat_kind = metadata_string(metadata, &["chat_kind", "chatKind"])
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "user".to_string());
    let (source_kind, summary_kind) = match chat_kind.as_str() {
        "group" | "supergroup" => ("telegram_group_message", "Telegram group message"),
        "channel" => ("telegram_channel_message", "Telegram channel message"),
        _ => ("telegram_direct_message", "Telegram direct message"),
    };
    let connection_slug = metadata_string(metadata, &["monitor_connection", "monitorConnection"]);
    let sender_id = metadata_string(metadata, &["sender_id", "senderId"]);
    let sender_username = metadata_string(metadata, &["sender_username", "senderUsername"]);
    let mut sender = Map::new();
    if let Some(sender_id) = sender_id {
        sender.insert("id".to_string(), Value::String(sender_id));
    }
    if let Some(sender_username) = sender_username {
        sender.insert("username".to_string(), Value::String(sender_username));
    }
    Some(json!({
        "kind": source_kind,
        "connection_slug": connection_slug,
        "connector_slug": connector_slug,
        "summary": format!("{summary_kind} from chat_id {chat_id}"),
        "delivery_target": {
            "type": "telegram_chat",
            "chat_id": chat_id,
            "chat_kind": chat_kind,
        },
        "sender": sender,
    }))
}

fn monitor_completion_policy(
    metadata: &Map<String, Value>,
    source_context: Option<&Value>,
) -> Option<Value> {
    metadata
        .get("completion_policy")
        .or_else(|| metadata.get("completionPolicy"))
        .cloned()
        .or_else(|| default_monitor_completion_policy(metadata, source_context))
}

fn default_monitor_completion_policy(
    metadata: &Map<String, Value>,
    source_context: Option<&Value>,
) -> Option<Value> {
    if !monitor_actions_require_reply(metadata) {
        return None;
    }
    source_context
        .and_then(source_context_delivery_target)
        .map(|_| human_gated_completion_policy())
}

fn source_context_delivery_target(context: &Value) -> Option<&Value> {
    context
        .get("delivery_target")
        .or_else(|| context.get("deliveryTarget"))
}

fn monitor_actions_require_reply(metadata: &Map<String, Value>) -> bool {
    monitor_actions(metadata).iter().any(|action| {
        let name = string_field(action, &["name"]).unwrap_or_default();
        let prompt = string_field(action, &["prompt"]).unwrap_or_default();
        let text = format!("{name}\n{prompt}").to_ascii_lowercase();
        [
            "reply",
            "respond",
            "send it back",
            "send back",
            "answer back",
            "message back",
        ]
        .iter()
        .any(|needle| text.contains(needle))
    })
}

fn monitor_task_is_human_gated(task: &StoredTask) -> bool {
    if !is_monitor_task_metadata(&task.metadata) {
        return false;
    }
    let source_context = monitor_source_context(&task.metadata);
    monitor_completion_policy(&task.metadata, source_context.as_ref())
        .as_ref()
        .is_some_and(completion_policy_requires_human_approval)
        || monitor_task_has_telegram_delivery_target(&task.metadata, source_context.as_ref())
}

fn completion_policy_mode(policy: &Value) -> Option<&str> {
    policy
        .as_str()
        .or_else(|| policy.get("mode").and_then(Value::as_str))
}

fn completion_policy_requires_human_approval(policy: &Value) -> bool {
    completion_policy_mode(policy)
        .is_some_and(|mode| matches!(mode, "draft_then_approve" | "send_to_source"))
        || policy
            .get("requires_human_approval")
            .or_else(|| policy.get("requiresHumanApproval"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn monitor_task_has_telegram_delivery_target(
    metadata: &Map<String, Value>,
    source_context: Option<&Value>,
) -> bool {
    source_context
        .and_then(|context| {
            let connector_slug = string_field(context, &["connector_slug", "connectorSlug"])
                .or_else(|| {
                    metadata_string(metadata, &["monitor_connector", "monitorConnector"])
                })?;
            connector_slug.contains("telegram").then_some(context)
        })
        .and_then(|context| {
            source_context_delivery_target(context)
                .and_then(|target| string_field(target, &["chat_id", "chatId"]))
        })
        .is_some()
        || metadata_string(metadata, &["monitor_connector", "monitorConnector"])
            .is_some_and(|connector| connector.contains("telegram"))
            && metadata_string(metadata, &["chat_id", "chatId"]).is_some()
}

fn human_gated_completion_policy() -> Value {
    json!({
        "mode": "draft_then_approve",
        "requires_human_approval": true,
        "requires_receipt": true,
    })
}

fn metadata_marks_monitor_ignored(metadata: Option<&Map<String, Value>>) -> bool {
    metadata
        .and_then(|metadata| metadata.get("ignored"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn monitor_reply_receipt(metadata: &Map<String, Value>) -> Option<Value> {
    metadata
        .get("action_receipts")
        .or_else(|| metadata.get("actionReceipts"))
        .and_then(Value::as_array)
        .and_then(|receipts| {
            receipts
                .iter()
                .find(|receipt| {
                    receipt
                        .get("kind")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind == "monitor_reply_send")
                })
                .cloned()
        })
}

fn monitor_reply_target(task: &StoredTask) -> Result<MonitorReplyTarget> {
    if !is_monitor_task_metadata(&task.metadata) {
        bail!("task `{}` is not a monitor task", task.task_id);
    }
    let source_context = monitor_source_context(&task.metadata)
        .ok_or_else(|| anyhow!("monitor task `{}` has no source_context", task.task_id))?;
    let Some(context) = source_context.as_object() else {
        bail!(
            "monitor task `{}` source_context is not an object",
            task.task_id
        );
    };
    let connector_slug = string_field_from_map(context, &["connector_slug", "connectorSlug"])
        .or_else(|| metadata_string(&task.metadata, &["monitor_connector", "monitorConnector"]))
        .ok_or_else(|| anyhow!("monitor task `{}` has no connector slug", task.task_id))?;
    if !connector_slug.contains("telegram") {
        bail!("MonitorReplySend currently supports Telegram monitor tasks only");
    }
    let connection_slug = string_field_from_map(context, &["connection_slug", "connectionSlug"])
        .or_else(|| metadata_string(&task.metadata, &["monitor_connection", "monitorConnection"]))
        .unwrap_or_else(|| connector_slug.clone());
    let delivery_target = context
        .get("delivery_target")
        .or_else(|| context.get("deliveryTarget"))
        .ok_or_else(|| anyhow!("monitor task `{}` has no delivery target", task.task_id))?;
    let Some(delivery_target) = delivery_target.as_object() else {
        bail!(
            "monitor task `{}` delivery target is not an object",
            task.task_id
        );
    };
    let target_type = string_field_from_map(delivery_target, &["type"]);
    if target_type.as_deref() != Some("telegram_chat") {
        bail!(
            "MonitorReplySend expected telegram_chat delivery target, got {}",
            target_type.unwrap_or_else(|| "<missing>".to_string())
        );
    }
    let chat_id = string_field_from_map(delivery_target, &["chat_id", "chatId"])
        .ok_or_else(|| anyhow!("monitor task `{}` has no Telegram chat_id", task.task_id))?;
    Ok(MonitorReplyTarget {
        connector_slug,
        connection_slug,
        chat_id,
    })
}

fn monitor_reply_connector_act_input(target: &MonitorReplyTarget, message: &str) -> Value {
    json!({
        "connector_slug": target.connector_slug,
        "connection_slug": target.connection_slug,
        "action": "send_message",
        "input": {
            "connection_slug": target.connection_slug,
            "connector_slug": target.connector_slug,
            "chat_id": target.chat_id,
            "message": message,
        }
    })
}

fn append_monitor_action_receipt(
    task: &mut StoredTask,
    target: &MonitorReplyTarget,
    connector_output: &Value,
) -> Result<()> {
    let sent_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .context("failed to format monitor reply receipt timestamp")?;
    let receipt = json!({
        "kind": "monitor_reply_send",
        "sent_at": sent_at,
        "connector_slug": target.connector_slug,
        "connection_slug": target.connection_slug,
        "delivery_target": {
            "type": "telegram_chat",
            "chat_id": target.chat_id,
        },
        "connector_action": "send_message",
        "connector_output": connector_output,
    });
    match task.metadata.get_mut("action_receipts") {
        Some(Value::Array(receipts)) => receipts.push(receipt),
        _ => {
            task.metadata
                .insert("action_receipts".to_string(), Value::Array(vec![receipt]));
        }
    }
    Ok(())
}

fn now_rfc3339() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .context("failed to format monitor reply timestamp")
}

fn source_context_hash(source_context: &Value) -> Result<String> {
    let raw = serde_json::to_vec(source_context).context("failed to encode source context")?;
    Ok(format!("{:x}", Sha256::digest(raw)))
}

fn append_monitor_reply_audit(task: &mut StoredTask, event: &str, details: Value) {
    let entry = json!({
        "event": event,
        "at": now_rfc3339().unwrap_or_else(|_| OffsetDateTime::now_utc().to_string()),
        "details": details,
    });
    match task.metadata.get_mut("monitor_reply_events") {
        Some(Value::Array(events)) => events.push(entry),
        _ => {
            task.metadata.insert(
                "monitor_reply_events".to_string(),
                Value::Array(vec![entry]),
            );
        }
    }
}

fn append_monitor_reply_audit_to_store(
    cwd: &Path,
    task_id: &str,
    event: &str,
    details: Value,
) -> Result<()> {
    let path = monitor_tasks_path(cwd);
    let mut store = load_store::<TaskStore>(&path)?;
    if let Some(task) = store.tasks.iter_mut().find(|task| task.task_id == task_id) {
        append_monitor_reply_audit(task, event, details);
        task.updated_at_ms = Some(now_ms());
        save_store(&path, &store)?;
    }
    Ok(())
}

fn monitor_actions(metadata: &Map<String, Value>) -> Vec<Value> {
    metadata
        .get("actions")
        .or_else(|| metadata.get("monitor_actions"))
        .or_else(|| metadata.get("monitorActions"))
        .and_then(Value::as_array)
        .map(|items| items.iter().map(camel_case_action).collect())
        .unwrap_or_default()
}

fn camel_case_action(value: &Value) -> Value {
    json!({
        "name": string_field(value, &["actionName", "name", "title"]),
        "prompt": string_field(value, &["actionPrompt", "prompt"]),
    })
}

fn camel_case_source_context(value: Value) -> Value {
    let Some(object) = value.as_object() else {
        return value;
    };
    let delivery_target = object
        .get("delivery_target")
        .or_else(|| object.get("deliveryTarget"))
        .map(camel_case_delivery_target)
        .unwrap_or(Value::Null);
    json!({
        "kind": string_field_from_map(object, &["kind"]),
        "connectionSlug": string_field_from_map(object, &["connection_slug", "connectionSlug"]),
        "connectorSlug": string_field_from_map(object, &["connector_slug", "connectorSlug"]),
        "summary": string_field_from_map(object, &["summary"]),
        "deliveryTarget": delivery_target,
        "sender": object.get("sender").cloned().unwrap_or(Value::Null),
    })
}

fn camel_case_delivery_target(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    json!({
        "type": string_field_from_map(object, &["type"]),
        "chatId": string_field_from_map(object, &["chat_id", "chatId"]),
        "chatKind": string_field_from_map(object, &["chat_kind", "chatKind"]),
    })
}

fn metadata_string(metadata: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| metadata.get(*key))
        .and_then(value_to_string)
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    value
        .as_object()
        .and_then(|object| string_field_from_map(object, keys))
}

fn string_field_from_map(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(value_to_string)
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
    use puffer_session_store::SessionStore;
    use tempfile::TempDir;

    fn make_state() -> (AppState, TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tmp.path());
        ensure_workspace_dirs(&paths).unwrap();
        let store = SessionStore::from_paths(&paths).unwrap();
        let session = store.create_session(tmp.path().to_path_buf()).unwrap();
        let state = AppState::new(PufferConfig::default(), tmp.path().to_path_buf(), session);
        (state, tmp)
    }

    fn create_telegram_monitor_task(state: &mut AppState, cwd: &Path) -> String {
        let raw = execute_task_create(
            state,
            cwd,
            json!({
                "subject": "Confirm P0/P1 risk before customer acceptance",
                "description": "Needs a reply in the source Telegram chat.",
                "receivedAt": "2026-06-10T13:00:00Z",
                "expiresAt": "2026-06-11T13:00:00Z",
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "telegram-user",
                    "monitor_connector": "telegram-login",
                    "chat_id": "8759047281",
                    "sender_id": "8759047281"
                },
                "actions": [
                    {
                        "actionName": "Reply",
                        "actionPrompt": "Research the answer and send it back."
                    }
                ]
            }),
        )
        .unwrap();
        serde_json::from_str::<Value>(&raw)
            .unwrap()
            .pointer("/task/id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string()
    }

    fn create_telegram_non_reply_monitor_task(state: &mut AppState, cwd: &Path) -> String {
        let raw = execute_task_create(
            state,
            cwd,
            json!({
                "subject": "Remember Telegram context",
                "description": "A Telegram message contains a useful deadline.",
                "receivedAt": "2026-06-10T13:00:00Z",
                "expiresAt": "2026-06-11T13:00:00Z",
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "telegram-user",
                    "monitor_connector": "telegram-login",
                    "chat_id": "8759047281",
                    "sender_id": "8759047281"
                },
                "actions": [
                    {
                        "actionName": "Add reminder",
                        "actionPrompt": "Create a reminder from the deadline."
                    }
                ]
            }),
        )
        .unwrap();
        serde_json::from_str::<Value>(&raw).unwrap()["task"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn monitor_task_metadata_rejects_ignore_filter_fields() {
        let metadata = serde_json::json!({
            "_monitor": true,
            "monitor_connection": "telegram-user",
            "ignore_filter": {"chat_id": "1", "sender_id": "2"}
        });
        let error = validate_monitor_task_metadata(metadata.as_object().unwrap())
            .expect_err("ignore filter metadata should be rejected");
        assert!(error
            .to_string()
            .contains("monitor task metadata cannot include ignore filter field"));
    }

    #[test]
    fn monitor_task_metadata_allows_identity_fields() {
        let metadata = serde_json::json!({
            "_monitor": true,
            "monitor_connection": "telegram-user",
            "chat_id": "1",
            "sender_id": "2"
        });
        validate_monitor_task_metadata(metadata.as_object().unwrap()).unwrap();
    }

    #[test]
    fn task_get_exposes_normalized_monitor_source_context() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());

        let raw = execute_task_get(&mut state, tmp.path(), json!({"taskId": task_id})).unwrap();
        let payload: Value = serde_json::from_str(&raw).unwrap();
        let task = payload.get("task").unwrap();

        assert_eq!(
            task.pointer("/sourceContext/kind").and_then(Value::as_str),
            Some("telegram_direct_message")
        );
        assert_eq!(
            task.pointer("/sourceContext/connectionSlug")
                .and_then(Value::as_str),
            Some("telegram-user")
        );
        assert_eq!(
            task.pointer("/sourceContext/deliveryTarget/chatId")
                .and_then(Value::as_str),
            Some("8759047281")
        );
        assert_eq!(
            task.pointer("/completionPolicy/mode")
                .and_then(Value::as_str),
            Some("draft_then_approve")
        );
        assert_eq!(
            task.pointer("/completionPolicy/requires_human_approval")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn task_get_exposes_group_monitor_source_context() {
        let (mut state, tmp) = make_state();
        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            json!({
                "subject": "Reply to group mention",
                "description": "A group chat mentioned me.",
                "receivedAt": "2026-06-10T13:00:00Z",
                "expiresAt": "2026-06-11T13:00:00Z",
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "telegram-user",
                    "monitor_connector": "telegram-login",
                    "chat_kind": "group",
                    "chat_id": "-10012345",
                    "sender_id": "8759047281"
                },
                "actions": [
                    {
                        "actionName": "Draft reply",
                        "actionPrompt": "Draft a concise reply to the group."
                    }
                ]
            }),
        )
        .unwrap();
        let task_id = serde_json::from_str::<Value>(&raw).unwrap()["task"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let raw = execute_task_get(&mut state, tmp.path(), json!({"taskId": task_id})).unwrap();
        let payload: Value = serde_json::from_str(&raw).unwrap();
        let task = payload.get("task").unwrap();

        assert_eq!(
            task.pointer("/sourceContext/kind").and_then(Value::as_str),
            Some("telegram_group_message")
        );
        assert_eq!(
            task.pointer("/sourceContext/deliveryTarget/chatKind")
                .and_then(Value::as_str),
            Some("group")
        );
    }

    #[test]
    fn task_update_rejects_reserved_monitor_metadata_changes() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());

        let error = execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "metadata": {
                    "source_context": {
                        "delivery_target": {"chat_id": "attacker"}
                    }
                }
            }),
        )
        .expect_err("agent must not be able to rewrite monitor source context");

        assert!(error
            .to_string()
            .contains("reserved monitor metadata field `source_context`"));
    }

    #[test]
    fn task_update_ignores_unchanged_reserved_monitor_metadata() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());

        let raw = execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "metadata": {
                    "chatId": "8759047281",
                    "senderId": "8759047281"
                }
            }),
        )
        .expect("unchanged reserved identity fields should be ignored");
        let payload: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(payload["success"], true);
        let task = load_monitor_task(tmp.path(), &task_id).unwrap().unwrap();
        assert!(task.metadata.get("chatId").is_none());
        assert!(task.metadata.get("senderId").is_none());
    }

    #[test]
    fn task_update_rejects_generic_completion_for_human_gated_monitor_task() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());

        let error = execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "status": "completed"
            }),
        )
        .expect_err("human-gated monitor tasks need approval before completion");

        assert!(error
            .to_string()
            .contains("must be completed through its monitor action"));
    }

    #[test]
    fn task_update_rejects_completion_for_telegram_delivery_target_without_policy() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_non_reply_monitor_task(&mut state, tmp.path());

        let error = execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "status": "completed"
            }),
        )
        .expect_err("Telegram delivery-target monitor tasks need approval before completion");

        assert!(error
            .to_string()
            .contains("must be completed through its monitor action"));
    }

    #[test]
    fn monitor_reply_send_uses_recorded_source_chat_only() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());
        let task = load_monitor_task(tmp.path(), &task_id).unwrap().unwrap();

        let target = monitor_reply_target(&task).unwrap();
        let input = monitor_reply_connector_act_input(&target, "Acknowledged.");

        assert_eq!(target.connection_slug, "telegram-user");
        assert_eq!(target.connector_slug, "telegram-login");
        assert_eq!(target.chat_id, "8759047281");
        assert_eq!(input["connection_slug"], "telegram-user");
        assert_eq!(input["connector_slug"], "telegram-login");
        assert_eq!(input["action"], "send_message");
        assert_eq!(input["input"]["chat_id"], "8759047281");
        assert_eq!(input["input"]["message"], "Acknowledged.");
        assert!(input["input"].get("to").is_none());
        assert!(input["input"].get("target").is_none());
    }

    #[test]
    fn monitor_reply_send_rejects_human_gated_monitor_tasks() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());

        let error = execute_monitor_reply_send(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "message": "Acknowledged."
            }),
        )
        .expect_err("human-gated monitor replies must not be sent by agent tools");

        assert!(error.to_string().contains("requires human approval"));
    }

    #[test]
    fn monitor_reply_send_rejects_telegram_delivery_target_without_policy() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_non_reply_monitor_task(&mut state, tmp.path());

        let error = execute_monitor_reply_send(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "message": "Acknowledged."
            }),
        )
        .expect_err("Telegram delivery-target tasks must not be sent by agent tools");

        assert!(error.to_string().contains("requires human approval"));
    }

    #[test]
    fn monitor_reply_draft_requires_matching_monitor_reply_scope() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());

        let error = execute_monitor_reply_draft(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "message": "Acknowledged."
            }),
        )
        .expect_err("draft tool must be scoped to a monitor action turn");

        assert!(error.to_string().contains("monitor reply scope"));
    }

    #[test]
    fn monitor_reply_draft_saves_server_owned_source_snapshot() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());
        state.set_monitor_reply_scope_for_turn(
            task_id.clone(),
            "session-1".to_string(),
            "turn-1".to_string(),
        );

        execute_monitor_reply_draft(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "message": "Acknowledged."
            }),
        )
        .unwrap();

        let task = load_monitor_task(tmp.path(), &task_id).unwrap().unwrap();
        let pending = task
            .metadata
            .get("pending_reply")
            .and_then(Value::as_object)
            .expect("draft metadata should be stored");

        assert_eq!(
            pending.get("status").and_then(Value::as_str),
            Some("draft_ready")
        );
        assert_eq!(
            pending.get("agent_draft_text").and_then(Value::as_str),
            Some("Acknowledged.")
        );
        assert_eq!(
            Value::Object(pending.clone())
                .pointer("/source_context_snapshot/delivery_target/chat_id")
                .and_then(Value::as_str),
            Some("8759047281")
        );
        assert!(pending.get("source_context_hash").is_some());
    }

    #[test]
    fn task_create_stamps_server_owned_created_at() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());

        let task = load_monitor_task(tmp.path(), &task_id).unwrap().unwrap();
        // created_at_ms is the stable creation stamp for latency stats —
        // updated_at_ms is clobbered by every TaskUpdate and started_at_ms
        // doubles as the in_progress transition stamp.
        assert!(task.created_at_ms.is_some());
        assert_eq!(task.created_at_ms, task.updated_at_ms);
    }

    #[test]
    fn monitor_reply_draft_allows_in_progress_task() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());
        execute_task_update(
            &mut state,
            tmp.path(),
            json!({ "taskId": task_id, "status": "in_progress" }),
        )
        .unwrap();
        assert_eq!(
            load_monitor_task(tmp.path(), &task_id)
                .unwrap()
                .unwrap()
                .status,
            "in_progress"
        );
        state.set_monitor_reply_scope_for_turn(
            task_id.clone(),
            "session-1".to_string(),
            "turn-1".to_string(),
        );

        execute_monitor_reply_draft(
            &mut state,
            tmp.path(),
            json!({ "taskId": task_id, "message": "排查结论稍后给出。" }),
        )
        .unwrap();

        let task = load_monitor_task(tmp.path(), &task_id).unwrap().unwrap();
        let pending = task
            .metadata
            .get("pending_reply")
            .and_then(Value::as_object)
            .expect("in_progress tasks must accept drafts");
        assert_eq!(
            pending.get("status").and_then(Value::as_str),
            Some("draft_ready")
        );
    }

    #[test]
    fn monitor_reply_draft_rejects_terminal_task() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());
        // TaskUpdate itself refuses agent-driven completion of human-gated
        // tasks, so build the terminal state directly in the store (the ignore
        // flow and reply-completion writeback land tasks here).
        let path = monitor_tasks_path(tmp.path());
        let mut store = load_store::<TaskStore>(&path).unwrap();
        store
            .tasks
            .iter_mut()
            .find(|task| task.task_id == task_id)
            .unwrap()
            .status = "completed".to_string();
        save_store(&path, &store).unwrap();
        state.set_monitor_reply_scope_for_turn(
            task_id.clone(),
            "session-1".to_string(),
            "turn-1".to_string(),
        );

        let error = execute_monitor_reply_draft(
            &mut state,
            tmp.path(),
            json!({ "taskId": task_id, "message": "Too late." }),
        )
        .expect_err("terminal tasks must not accept drafts");

        assert!(error.to_string().contains("terminal status"));
    }

    #[test]
    fn monitor_reply_send_rejects_tasks_without_stable_delivery_target() {
        let (mut state, tmp) = make_state();
        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            json!({
                "subject": "Reply to message without source",
                "description": "Missing Telegram chat id.",
                "receivedAt": "2026-06-10T13:00:00Z",
                "expiresAt": "2026-06-11T13:00:00Z",
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "telegram-user",
                    "monitor_connector": "telegram-login"
                }
            }),
        )
        .unwrap();
        let task_id = serde_json::from_str::<Value>(&raw)
            .unwrap()
            .pointer("/task/id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        let task = load_monitor_task(tmp.path(), &task_id).unwrap().unwrap();

        let error = monitor_reply_target(&task).expect_err("missing chat_id must be rejected");

        assert!(error.to_string().contains("has no source_context"));
    }
}
