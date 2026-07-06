use super::monitor_contract::{
    display_source_context, monitor_contract_hash, parse_monitor_contract, MonitorContract,
    MonitorTaskKind, MONITOR_SCHEMA_VERSION,
};
use super::store::{
    agents_path, append_agent_message, ensure_safe_identifier, load_store, monitor_tasks_path,
    next_monitor_task_id, next_task_id, now_ms, save_store, tasks_path, team_lead_agent_id,
    terminate_process, wait_for_process_exit, AgentStore, StoredTask, TaskCreateInput, TaskIdInput,
    TaskOutputInput, TaskStopInput, TaskStore, TaskUpdateInput,
};
use super::task_runtime::{
    is_subagent_context, read_runtime_agent_output, read_task_output, refresh_stored_task,
    runtime_agent_output_path, runtime_agent_terminal_status,
    should_emit_verification_nudge_for_tasks, terminal_task_status, wait_for_runtime_agent_output,
    wait_for_stored_task, VERIFICATION_NUDGE,
};
use crate::{AppState, MonitorSourceStampContext, MonitorTaskCreateGateContext};
use anyhow::{anyhow, bail, Context, Result};
use puffer_subscriptions::{MonitorTraceIdentity, MonitorTraceStage, MonitorTraceStore};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

const MONITOR_SOURCE_MESSAGES_LIMIT: usize = 8;

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
    let mut metadata = parsed.metadata.clone().unwrap_or_default();
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
    if monitor_task && received_at.is_none() {
        bail!("monitor TaskCreate requires receivedAt in RFC3339 format");
    }
    if monitor_task && expires_at.is_none() {
        bail!("monitor TaskCreate requires expiresAt in RFC3339 format");
    }
    if monitor_task {
        stamp_monitor_task_metadata_from_current_sources(state, &parsed, &mut metadata)?;
        validate_monitor_task_metadata(&metadata)?;
        if let Some(skip) = monitor_task_source_scope_skip(state, &metadata) {
            return Ok(serde_json::to_string_pretty(&skip)?);
        }
        normalize_monitor_task_metadata(&mut metadata)?;
        validate_monitor_task_metadata(&metadata)?;
    }
    if let Some(gate) = apply_monitor_task_create_gate(state, &mut metadata) {
        record_monitor_task_create_gate_trace(&gate);
        if gate.decision == MonitorTaskCreateGateDecision::SkipHandled {
            return Ok(serde_json::to_string_pretty(&json!({
                "success": true,
                "skipped": true,
                "reason": "handled_in_telegram",
                "gate": gate.gate,
            }))?);
        }
    }
    let tp = if monitor_task {
        monitor_tasks_path(state.session.cwd.as_path())
    } else {
        tasks_path(state.session.cwd.as_path(), &state.session.id)
    };
    let mut store = load_store::<TaskStore>(&tp)?;
    if monitor_task {
        if let Some(skip) = duplicate_monitor_task_skip(&store.tasks, &metadata, &parsed.subject) {
            return Ok(serde_json::to_string_pretty(&skip)?);
        }
    }
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
        completed_via: None,
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
            let mut item = json!({
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
            });
            if let Some(source) = compact_monitor_source(task) {
                item["monitorSource"] = source;
            }
            item
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
        && !metadata_marks_monitor_ignored(metadata_update.as_ref())
    {
        if monitor_task_is_typed_executable(task) {
            bail!(
                "typed monitor task `{}` must be completed through its monitor action",
                parsed.task_id
            );
        }
        if monitor_task_requires_human_approval(task) && !state.monitor_triage_turn {
            bail!(
                "monitor task `{}` must be completed through its monitor action after human approval",
                parsed.task_id
            );
        }
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
    // Stamp completed_via on monitor tasks when THIS call transitioned the task
    // into completed.  Checking status_change (Some with to=="completed")) prevents
    // clobbering an existing completed_via when a subsequent update only touches
    // metadata or other fields on an already-completed task.
    // This mirrors the daemon's human-approval path (handle_monitor_task_complete)
    // which also records completed_via as a top-level field.
    if status_change.as_ref().and_then(|sc| sc["to"].as_str()) == Some("completed")
        && (tp == monitor_tasks_path(&store_cwd) || is_monitor_task_metadata(&task.metadata))
    {
        let via = parsed
            .metadata
            .as_ref()
            .and_then(|m| m.get("completed_via"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("agent_report")
            .to_string();
        task.completed_via = Some(via);
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
    let verification_nudge_needed = !is_subagent_context(state)
        && status_change
            .as_ref()
            .and_then(|change| change.get("to"))
            .and_then(Value::as_str)
            == Some("completed")
        && should_emit_verification_nudge_for_tasks(&store.tasks);
    save_store(&tp, &store)?;
    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "taskId": task_id,
        "updatedFields": updated_fields,
        "statusChange": status_change,
        "verificationNudgeNeeded": verification_nudge_needed,
        "note": verification_nudge_needed.then_some(VERIFICATION_NUDGE),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitorTaskCreateGateDecision {
    SkipHandled,
    CreateRead,
    CreateUnknown,
}

impl MonitorTaskCreateGateDecision {
    fn slug(self) -> &'static str {
        match self {
            Self::SkipHandled => "skip_handled",
            Self::CreateRead => "create_read",
            Self::CreateUnknown => "create_unknown",
        }
    }
}

#[derive(Debug, Clone)]
struct MonitorTaskCreateGateOutcome {
    decision: MonitorTaskCreateGateDecision,
    context: MonitorTaskCreateGateContext,
    gate: Value,
}

#[derive(Debug, Clone)]
struct TelegramActivityEvaluation {
    read: bool,
    replied: bool,
    basis: Vec<&'static str>,
    read_inbox_max_id: Option<i64>,
    activity_updated_at_ms: Option<i64>,
    chat_updated_at_ms: Option<i64>,
    error: Option<String>,
}

fn monitor_task_source_scope_skip(
    state: &AppState,
    metadata: &Map<String, Value>,
) -> Option<Value> {
    if state.monitor_task_create_gate_contexts.is_empty()
        || !metadata_is_direct_telegram_monitor(state, metadata)
    {
        return None;
    }
    let envelope_id = metadata_string(metadata, &["monitor_envelope_id", "monitorEnvelopeId"]);
    let Some(envelope_id) = envelope_id.as_deref() else {
        return Some(monitor_task_skip_payload(
            "untrusted_monitor_source",
            None,
            Some(json!({
                "source": "monitor_source_scope",
                "decision": "skip_untrusted_source",
                "reason": "missing_monitor_envelope_id",
                "allowed_envelope_ids": current_gate_envelope_ids(state, metadata),
            })),
        ));
    };
    if !current_gate_context_matches(state, metadata, envelope_id) {
        return Some(monitor_task_skip_payload(
            "untrusted_monitor_source",
            None,
            Some(json!({
                "source": "monitor_source_scope",
                "decision": "skip_untrusted_source",
                "reason": "monitor_envelope_id_not_in_current_batch",
                "envelope_id": envelope_id,
                "allowed_envelope_ids": current_gate_envelope_ids(state, metadata),
            })),
        ));
    }
    for envelope_id in monitor_envelope_ids(metadata) {
        if !current_gate_context_matches(state, metadata, &envelope_id) {
            return Some(monitor_task_skip_payload(
                "untrusted_monitor_source",
                None,
                Some(json!({
                    "source": "monitor_source_scope",
                    "decision": "skip_untrusted_source",
                    "reason": "monitor_envelope_ids_contains_non_current_source",
                    "envelope_id": envelope_id,
                    "allowed_envelope_ids": current_gate_envelope_ids(state, metadata),
                })),
            ));
        }
    }
    None
}

fn metadata_is_direct_telegram_monitor(state: &AppState, metadata: &Map<String, Value>) -> bool {
    let connection_slug = metadata_string(metadata, &["monitor_connection", "monitorConnection"]);
    let connector_slug = metadata_string(metadata, &["monitor_connector", "monitorConnector"]);
    let is_telegram = connection_slug
        .as_deref()
        .is_some_and(|connection| connection.contains("telegram"))
        || connector_slug
            .as_deref()
            .is_some_and(|connector| connector.contains("telegram"));
    if !is_telegram {
        return false;
    }
    let chat_kind = metadata_string(metadata, &["chat_kind", "chatKind"]).or_else(|| {
        metadata
            .get("source_context")
            .or_else(|| metadata.get("sourceContext"))
            .and_then(|context| string_field(context, &["kind"]))
            .map(|kind| {
                if kind == "telegram_direct_message" {
                    "user".to_string()
                } else {
                    kind
                }
            })
    });
    if let Some(chat_kind) = chat_kind {
        return is_direct_telegram_chat_kind(&chat_kind);
    }
    let chat_id = metadata_i64(metadata, &["chat_id", "chatId"]);
    state
        .monitor_task_create_gate_contexts
        .iter()
        .any(|context| {
            connection_slug
                .as_deref()
                .is_none_or(|connection| connection == context.connection_slug)
                && connector_matches(context.connector_slug.as_deref(), connector_slug.as_deref())
                && chat_id.is_some_and(|chat_id| chat_id == context.chat_id)
                && is_direct_telegram_chat_kind(&context.chat_kind)
        })
}

fn current_gate_context_matches(
    state: &AppState,
    metadata: &Map<String, Value>,
    envelope_id: &str,
) -> bool {
    let connection_slug = metadata_string(metadata, &["monitor_connection", "monitorConnection"]);
    let connector_slug = metadata_string(metadata, &["monitor_connector", "monitorConnector"]);
    let chat_id = metadata_i64(metadata, &["chat_id", "chatId"]);
    state
        .monitor_task_create_gate_contexts
        .iter()
        .any(|context| {
            context.envelope_id == envelope_id
                && connection_slug
                    .as_deref()
                    .is_none_or(|connection| connection == context.connection_slug)
                && connector_matches(context.connector_slug.as_deref(), connector_slug.as_deref())
                && chat_id.is_none_or(|chat_id| chat_id == context.chat_id)
                && is_direct_telegram_chat_kind(&context.chat_kind)
        })
}

fn current_gate_envelope_ids(state: &AppState, metadata: &Map<String, Value>) -> Vec<String> {
    let connection_slug = metadata_string(metadata, &["monitor_connection", "monitorConnection"]);
    let connector_slug = metadata_string(metadata, &["monitor_connector", "monitorConnector"]);
    let chat_id = metadata_i64(metadata, &["chat_id", "chatId"]);
    state
        .monitor_task_create_gate_contexts
        .iter()
        .filter(|context| {
            connection_slug
                .as_deref()
                .is_none_or(|connection| connection == context.connection_slug)
                && connector_matches(context.connector_slug.as_deref(), connector_slug.as_deref())
                && chat_id.is_none_or(|chat_id| chat_id == context.chat_id)
        })
        .map(|context| context.envelope_id.clone())
        .collect()
}

fn apply_monitor_task_create_gate(
    state: &AppState,
    metadata: &mut Map<String, Value>,
) -> Option<MonitorTaskCreateGateOutcome> {
    let context = monitor_task_create_gate_context(state, metadata)?;
    let evaluation = evaluate_telegram_activity(&context);
    let decision = if evaluation.replied {
        MonitorTaskCreateGateDecision::SkipHandled
    } else if evaluation.read {
        MonitorTaskCreateGateDecision::CreateRead
    } else {
        MonitorTaskCreateGateDecision::CreateUnknown
    };
    let gate = monitor_task_create_gate_json(&context, decision, &evaluation);
    metadata.insert("monitor_task_gate".to_string(), gate.clone());
    if evaluation.read && !evaluation.replied {
        metadata.insert(
            "source_state".to_string(),
            json!({
                "telegram": {
                    "read": true,
                    "replied": false,
                    "decision": decision.slug(),
                    "label": "已读",
                }
            }),
        );
    }
    Some(MonitorTaskCreateGateOutcome {
        decision,
        context,
        gate,
    })
}

fn duplicate_monitor_task_skip(
    tasks: &[StoredTask],
    metadata: &Map<String, Value>,
    subject: &str,
) -> Option<Value> {
    let candidate_subject = normalize_monitor_subject(subject);
    let candidate_envelopes = monitor_envelope_ids(metadata);
    let candidate_sources = monitor_source_message_ids(metadata);
    let candidate_senders = monitor_sender_ids(metadata);
    for task in tasks {
        if !same_monitor_task_scope(metadata, &task.metadata) {
            continue;
        }

        // Message identity is the ONLY status-independent duplicate signal: the same
        // Telegram (chat_id, message_id) re-delivered by a reconnect/replay must never
        // spawn a second task, even after the first was completed or ignored
        // (agentenv/monorepo#625). Content and subject are deliberately NOT used as a
        // cross-status signal — two *distinct* messages with identical text are
        // distinct events and may each open their own task.
        let existing_sources = monitor_source_message_ids(&task.metadata);
        if !candidate_sources.is_empty()
            && !existing_sources.is_empty()
            && !candidate_sources.is_disjoint(&existing_sources)
        {
            return Some(monitor_task_skip_payload(
                "duplicate_source",
                Some(task.task_id.as_str()),
                None,
            ));
        }

        // The envelope and subject legs apply to OPEN tasks only (unchanged #432
        // behavior): an in-flight re-delivery of the same envelope, or a still-open
        // task with the same subject, is collapsed while it is actionable.
        if terminal_task_status(&task.status)
            || metadata_marks_monitor_ignored(Some(&task.metadata))
        {
            continue;
        }
        let existing_envelopes = monitor_envelope_ids(&task.metadata);
        if !candidate_envelopes.is_disjoint(&existing_envelopes) {
            return Some(monitor_task_skip_payload(
                "duplicate_source",
                Some(task.task_id.as_str()),
                None,
            ));
        }
        if let Some(candidate_subject) = candidate_subject.as_deref() {
            if normalize_monitor_subject(&task.subject).as_deref() == Some(candidate_subject) {
                // Same normalized subject, but distinct KNOWN senders are
                // distinct requests (e.g. two people in a group asking the
                // same thing) — each keeps its own task (agentenv/monorepo#655).
                // The candidate folds only when every sender it is known to
                // carry (one for single-source tasks, several for a
                // mixed-sender burst) is already represented by the existing
                // task. If either side has no known sender, keep the
                // historical subject-only fold (DMs are 1:1; sender adds
                // nothing there).
                let existing_senders = monitor_sender_ids(&task.metadata);
                let senders_differ = !candidate_senders.is_empty()
                    && !existing_senders.is_empty()
                    && !candidate_senders
                        .iter()
                        .all(|sender| existing_senders.contains(sender));
                if !senders_differ {
                    return Some(monitor_task_skip_payload(
                        "duplicate_monitor_task",
                        Some(task.task_id.as_str()),
                        None,
                    ));
                }
            }
        }
    }
    None
}

fn monitor_task_skip_payload(
    reason: &str,
    existing_task_id: Option<&str>,
    gate: Option<Value>,
) -> Value {
    let mut payload = json!({
        "success": true,
        "skipped": true,
        "reason": reason,
    });
    if let Some(existing_task_id) = existing_task_id {
        payload["existingTaskId"] = Value::String(existing_task_id.to_string());
    }
    if let Some(gate) = gate {
        payload["gate"] = gate;
    }
    payload
}

fn compact_monitor_source(task: &StoredTask) -> Option<Value> {
    if !is_monitor_task_metadata(&task.metadata) {
        return None;
    }
    let mut source = Map::new();
    if let Some(value) =
        metadata_string(&task.metadata, &["monitor_connection", "monitorConnection"])
    {
        source.insert("connectionSlug".to_string(), Value::String(value));
    }
    if let Some(value) = metadata_string(&task.metadata, &["monitor_connector", "monitorConnector"])
    {
        source.insert("connectorSlug".to_string(), Value::String(value));
    }
    if let Some(value) = metadata_i64(&task.metadata, &["chat_id", "chatId"]) {
        source.insert("chatId".to_string(), Value::from(value));
    }
    if let Some(value) = metadata_string(&task.metadata, &["chat_kind", "chatKind"]) {
        source.insert("chatKind".to_string(), Value::String(value));
    }
    if let Some(value) = metadata_string(
        &task.metadata,
        &["monitor_envelope_id", "monitorEnvelopeId"],
    ) {
        source.insert("envelopeId".to_string(), Value::String(value));
    }
    let mut envelope_ids = monitor_envelope_ids(&task.metadata)
        .into_iter()
        .collect::<Vec<_>>();
    envelope_ids.sort();
    if envelope_ids.len() > 1 {
        source.insert("envelopeIds".to_string(), json!(envelope_ids));
    }
    let mut source_message_ids = monitor_source_message_ids(&task.metadata)
        .into_iter()
        .collect::<Vec<_>>();
    source_message_ids.sort();
    if let Some(value) = source_message_ids.first() {
        source.insert("sourceMessageId".to_string(), Value::from(*value));
    }
    if let Some(value) = metadata_string(&task.metadata, &["source_text", "sourceText"])
        .and_then(|value| normalize_source_snippet(&value, 160))
    {
        source.insert("sourceTextSnippet".to_string(), Value::String(value));
    }
    (!source.is_empty()).then_some(Value::Object(source))
}

fn normalize_source_snippet(value: &str, max_chars: usize) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    Some(normalized.chars().take(max_chars).collect())
}

fn same_monitor_task_scope(left: &Map<String, Value>, right: &Map<String, Value>) -> bool {
    let Some(left_connection) = metadata_string(left, &["monitor_connection", "monitorConnection"])
    else {
        return false;
    };
    if metadata_string(right, &["monitor_connection", "monitorConnection"]).as_deref()
        != Some(left_connection.as_str())
    {
        return false;
    }
    let left_connector = metadata_string(left, &["monitor_connector", "monitorConnector"]);
    let right_connector = metadata_string(right, &["monitor_connector", "monitorConnector"]);
    if left_connector.is_some()
        && right_connector.is_some()
        && left_connector.as_deref() != right_connector.as_deref()
    {
        return false;
    }
    let Some(left_chat_id) = metadata_i64(left, &["chat_id", "chatId"]) else {
        return false;
    };
    metadata_i64(right, &["chat_id", "chatId"]) == Some(left_chat_id)
}

fn monitor_envelope_ids(metadata: &Map<String, Value>) -> HashSet<String> {
    let mut envelope_ids = HashSet::new();
    for key in ["monitor_envelope_ids", "monitorEnvelopeIds"] {
        if let Some(items) = metadata.get(key).and_then(Value::as_array) {
            for item in items {
                if let Some(value) = value_to_string(item) {
                    envelope_ids.insert(value);
                }
            }
        }
    }
    if let Some(value) = metadata_string(metadata, &["monitor_envelope_id", "monitorEnvelopeId"]) {
        envelope_ids.insert(value);
    }
    envelope_ids
}

fn monitor_source_message_ids(metadata: &Map<String, Value>) -> HashSet<i64> {
    let mut ids = HashSet::new();
    if let Some(id) = metadata_i64(metadata, &["source_message_id", "sourceMessageId"]) {
        ids.insert(id);
    }
    // Plural form stamped onto consolidated multi-envelope (generic.review) tasks.
    for key in ["source_message_ids", "sourceMessageIds"] {
        if let Some(items) = metadata.get(key).and_then(Value::as_array) {
            for item in items {
                if let Some(id) = value_i64(item) {
                    ids.insert(id);
                }
            }
        }
    }
    for key in ["monitor_task_gate", "monitorTaskGate"] {
        if let Some(gate) = metadata.get(key) {
            if let Some(id) = value_i64_field(gate, &["source_message_id", "sourceMessageId"]) {
                ids.insert(id);
            }
        }
    }
    for key in ["source_context", "sourceContext"] {
        if let Some(context) = metadata.get(key) {
            if let Some(id) = value_i64_field(context, &["message_id", "messageId"]) {
                ids.insert(id);
            }
        }
    }
    ids
}

/// Sender identity for dedup: top-level `sender_id` (stamped on bursts and by
/// the triage runner), falling back to the typed contract's `source.sender_id`
/// (single-source tasks). Normalized to a string because the id arrives as a
/// JSON number from the telegram subscriber but as a string in stamped forms.
fn monitor_sender_id(metadata: &Map<String, Value>) -> Option<String> {
    for key in ["sender_id", "senderId"] {
        if let Some(value) = metadata.get(key).and_then(value_to_string) {
            return Some(value);
        }
    }
    parse_monitor_contract(metadata)
        .ok()
        .flatten()
        .and_then(|contract| string_field_from_map(&contract.source, &["sender_id", "senderId"]))
}

/// All distinct sender identities a task is known to carry: the plural
/// `sender_ids` stamp of a mixed-sender burst, or the single
/// [`monitor_sender_id`]. Empty when no sender is known.
fn monitor_sender_ids(metadata: &Map<String, Value>) -> Vec<String> {
    if let Some(values) = metadata
        .get("sender_ids")
        .or_else(|| metadata.get("senderIds"))
        .and_then(Value::as_array)
    {
        let ids = values
            .iter()
            .filter_map(value_to_string)
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            return ids;
        }
    }
    monitor_sender_id(metadata).into_iter().collect()
}

fn normalize_monitor_subject(subject: &str) -> Option<String> {
    let normalized = subject
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

fn monitor_task_create_gate_context(
    state: &AppState,
    metadata: &Map<String, Value>,
) -> Option<MonitorTaskCreateGateContext> {
    let envelope_id = metadata_string(metadata, &["monitor_envelope_id", "monitorEnvelopeId"])?;
    let connection_slug = metadata_string(metadata, &["monitor_connection", "monitorConnection"])?;
    let connector_slug = metadata_string(metadata, &["monitor_connector", "monitorConnector"]);
    if !connection_slug.contains("telegram")
        && !connector_slug
            .as_deref()
            .is_some_and(|connector| connector.contains("telegram"))
    {
        return None;
    }
    let chat_kind = metadata_string(metadata, &["chat_kind", "chatKind"])
        .or_else(|| {
            metadata
                .get("source_context")
                .or_else(|| metadata.get("sourceContext"))
                .and_then(|context| string_field(context, &["kind"]))
                .map(|kind| {
                    if kind == "telegram_direct_message" {
                        "user".to_string()
                    } else {
                        kind
                    }
                })
        })
        .unwrap_or_else(|| "user".to_string());
    if !is_direct_telegram_chat_kind(&chat_kind) {
        return None;
    }
    let chat_id = metadata_i64(metadata, &["chat_id", "chatId"]);
    state
        .monitor_task_create_gate_contexts
        .iter()
        .find(|context| {
            context.envelope_id == envelope_id
                && context.connection_slug == connection_slug
                && connector_matches(context.connector_slug.as_deref(), connector_slug.as_deref())
                && chat_id.is_none_or(|chat_id| chat_id == context.chat_id)
                && is_direct_telegram_chat_kind(&context.chat_kind)
        })
        .cloned()
}

fn connector_matches(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn is_direct_telegram_chat_kind(chat_kind: &str) -> bool {
    matches!(
        chat_kind.trim().to_ascii_lowercase().as_str(),
        "user" | "private" | "direct" | "telegram_direct_message"
    )
}

fn evaluate_telegram_activity(
    context: &MonitorTaskCreateGateContext,
) -> TelegramActivityEvaluation {
    let raw = match fs::read_to_string(&context.activity_state_path) {
        Ok(raw) => raw,
        Err(error) => {
            return TelegramActivityEvaluation {
                read: false,
                replied: false,
                basis: vec!["activity_state_unavailable"],
                read_inbox_max_id: None,
                activity_updated_at_ms: None,
                chat_updated_at_ms: None,
                error: Some(error.to_string()),
            };
        }
    };
    let state: Value = match serde_json::from_str(&raw) {
        Ok(state) => state,
        Err(error) => {
            return TelegramActivityEvaluation {
                read: false,
                replied: false,
                basis: vec!["activity_state_parse_failed"],
                read_inbox_max_id: None,
                activity_updated_at_ms: None,
                chat_updated_at_ms: None,
                error: Some(error.to_string()),
            };
        }
    };
    let activity_updated_at_ms = value_i64_field(&state, &["updated_at_ms", "updatedAtMs"]);
    let Some(chat) = state
        .get("chats")
        .and_then(Value::as_array)
        .and_then(|chats| {
            chats.iter().find(|chat| {
                value_i64_field(chat, &["chat_id", "chatId"]) == Some(context.chat_id)
                    && value_string_field(chat, &["chat_kind", "chatKind"])
                        .as_deref()
                        .map(is_direct_telegram_chat_kind)
                        .unwrap_or(true)
            })
        })
    else {
        return TelegramActivityEvaluation {
            read: false,
            replied: false,
            basis: vec!["chat_state_missing"],
            read_inbox_max_id: None,
            activity_updated_at_ms,
            chat_updated_at_ms: None,
            error: None,
        };
    };

    let read_inbox_max_id = value_i64_field(chat, &["read_inbox_max_id", "readInboxMaxId"]);
    let read = read_inbox_max_id.is_some_and(|max_id| max_id >= context.source_message_id);
    let chat_updated_at_ms = value_i64_field(chat, &["updated_at_ms", "updatedAtMs"]);
    let agent_sent_ids = chat
        .get("agent_sent_message_ids")
        .or_else(|| chat.get("agentSentMessageIds"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(value_i64)
        .collect::<HashSet<_>>();
    let replied = chat
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages.iter().any(|message| {
                let message_id = value_i64_field(message, &["message_id", "messageId"]);
                let agent_originated =
                    value_bool_field(message, &["agent_originated", "agentOriginated"])
                        || message_id.is_some_and(|id| agent_sent_ids.contains(&id));
                value_bool_field(message, &["is_outgoing", "isOutgoing"])
                    && value_i64_field(
                        message,
                        &[
                            "reply_to_message_id",
                            "replyToMessageId",
                            "reply_to",
                            "replyTo",
                        ],
                    ) == Some(context.source_message_id)
                    && !agent_originated
            })
        })
        .unwrap_or(false);
    let mut basis = Vec::new();
    if replied {
        basis.push("outgoing_reply_to_source_message_id");
    } else if read {
        basis.push("read_inbox_max_id");
    } else {
        basis.push("no_local_read_or_reply_match");
    }
    TelegramActivityEvaluation {
        read,
        replied,
        basis,
        read_inbox_max_id,
        activity_updated_at_ms,
        chat_updated_at_ms,
        error: None,
    }
}

fn monitor_task_create_gate_json(
    context: &MonitorTaskCreateGateContext,
    decision: MonitorTaskCreateGateDecision,
    evaluation: &TelegramActivityEvaluation,
) -> Value {
    json!({
        "source": "telegram_local_activity",
        "decision": decision.slug(),
        "read": evaluation.read,
        "replied": evaluation.replied,
        "basis": evaluation.basis.clone(),
        "connection_slug": context.connection_slug,
        "connector_slug": context.connector_slug,
        "envelope_id": context.envelope_id,
        "chat_id": context.chat_id,
        "chat_kind": context.chat_kind,
        "source_message_id": context.source_message_id,
        "source_date_ms": context.source_date_ms,
        "read_inbox_max_id": evaluation.read_inbox_max_id,
        "activity_updated_at_ms": evaluation.activity_updated_at_ms,
        "chat_updated_at_ms": evaluation.chat_updated_at_ms,
        "activity_state_staleness_ms": evaluation
            .chat_updated_at_ms
            .or(evaluation.activity_updated_at_ms)
            .map(|updated| i64::try_from(now_ms()).unwrap_or(i64::MAX).saturating_sub(updated)),
        "error": evaluation.error.clone(),
    })
}

fn record_monitor_task_create_gate_trace(outcome: &MonitorTaskCreateGateOutcome) {
    let Some(path) = outcome.context.monitor_trace_path.as_ref() else {
        return;
    };
    let Ok(store) = MonitorTraceStore::load(path) else {
        return;
    };
    let identity = MonitorTraceIdentity {
        message_key: format!(
            "{}:{}:{}",
            outcome.context.connection_slug,
            outcome.context.chat_id,
            outcome.context.source_message_id
        ),
        connection_slug: outcome.context.connection_slug.clone(),
        connector_slug: outcome.context.connector_slug.clone(),
        topic: Some(outcome.context.connection_slug.clone()),
        kind: Some("message".to_string()),
        chat_id: Some(outcome.context.chat_id.to_string()),
        chat_title: None,
        sender_id: None,
        sender_name: None,
        message_id: Some(outcome.context.source_message_id.to_string()),
        dedup_key: Some(format!(
            "{}:{}",
            outcome.context.chat_id, outcome.context.source_message_id
        )),
        envelope_id: Some(outcome.context.envelope_id.clone()),
        text: None,
        event_date_ms: outcome.context.source_date_ms.map(i128::from),
        received_at_ms: None,
    };
    let mut stage = MonitorTraceStage::completed(
        "task_create_gate",
        "TaskCreate",
        format!(
            "TaskCreate Telegram read/reply gate decision: {}.",
            outcome.decision.slug()
        ),
        i128::from(now_ms()),
    )
    .with_envelope(outcome.context.envelope_id.clone());
    stage.raw_source = serde_json::to_string(&outcome.gate).ok();
    let _ = store.record_stage(identity, stage);
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
    for key in ["monitor_reply_events", "monitorReplyEvents"] {
        if metadata.contains_key(key) {
            bail!("monitor task metadata cannot include reserved field `{key}`");
        }
    }
    parse_monitor_contract(metadata)?;
    Ok(())
}

fn stamp_monitor_task_metadata_from_current_sources(
    state: &AppState,
    parsed: &TaskCreateInput,
    metadata: &mut Map<String, Value>,
) -> Result<()> {
    if state.monitor_source_stamp_contexts.is_empty() {
        return Ok(());
    }
    reject_llm_written_typed_monitor_fields(metadata)?;
    let selected = selected_monitor_source_stamps(state, parsed, metadata)?;
    if selected.is_empty() {
        bail!("monitor TaskCreate could not be bound to a current workflow trigger");
    }
    if selected.len() > 1 {
        let contract = generic_review_contract_from_stamps(&selected);
        apply_stamped_monitor_contract(metadata, contract)?;
        metadata.insert(
            "monitor_envelope_ids".to_string(),
            Value::Array(
                selected
                    .iter()
                    .map(|context| Value::String(context.envelope_id.clone()))
                    .collect(),
            ),
        );
        // The generic.review contract drops per-message source identity, which would
        // leave a consolidated burst task un-scope-matchable and un-deduplicable.
        // Restore the shared scalar identity so message-identity dedup still applies
        // (agentenv/monorepo#625).
        stamp_shared_monitor_source_identity(metadata, &selected);
        return Ok(());
    }

    let stamp = selected[0].clone();
    let contract = typed_contract_from_stamp(&stamp);
    metadata.insert(
        "monitor_envelope_id".to_string(),
        Value::String(stamp.envelope_id.clone()),
    );
    if let Some(text) = stamp.text.as_ref().filter(|value| !value.trim().is_empty()) {
        metadata.insert("source_text".to_string(), Value::String(text.clone()));
    }
    if let Some(message_id) = source_message_id_from_stamp(&stamp) {
        metadata.insert("source_message_id".to_string(), Value::from(message_id));
    }
    if let Some(source_messages) = source_messages_from_stamp(&stamp) {
        metadata.insert("source_messages".to_string(), source_messages);
    }
    apply_stamped_monitor_contract(metadata, contract)?;
    Ok(())
}

/// For a consolidated multi-envelope (generic.review) monitor task, copy the
/// source identity that is shared across all contributing envelopes to top-level
/// metadata, so scope-matching and message-identity dedup behave the same as for
/// single-source tasks. A same-conversation burst is bucketed by chat in the
/// digest, so `chat_id`/`sender_id` are normally uniform; only fields that are
/// uniform across the batch are stamped, and the per-message ids are collected
/// into a plural `source_message_ids` array. Content/subject are deliberately not
/// involved (agentenv/monorepo#625 is message-identity only).
fn stamp_shared_monitor_source_identity(
    metadata: &mut Map<String, Value>,
    stamps: &[MonitorSourceStampContext],
) {
    if let Some(chat_id) = uniform_stamp(stamps, &["chat_id", "chatId"], value_i64_field) {
        metadata.insert("chat_id".to_string(), Value::from(chat_id));
    }
    if let Some(sender_id) = uniform_stamp(stamps, &["sender_id", "senderId"], value_i64_field) {
        metadata.insert("sender_id".to_string(), Value::from(sender_id));
    } else {
        // Mixed senders in one burst: stamp the distinct set so the
        // subject-dedup leg can tell a known multi-sender burst apart from a
        // sender-less record instead of folding it into one person's task
        // (agentenv/monorepo#655).
        let mut sender_ids = stamps
            .iter()
            .filter_map(|stamp| value_i64_field(&stamp.payload, &["sender_id", "senderId"]))
            .collect::<Vec<_>>();
        sender_ids.sort_unstable();
        sender_ids.dedup();
        if sender_ids.len() >= 2 {
            metadata.insert(
                "sender_ids".to_string(),
                Value::Array(sender_ids.into_iter().map(Value::from).collect()),
            );
        }
    }
    let source_message_ids = sorted_source_message_ids(stamps);
    if !source_message_ids.is_empty() {
        metadata.insert(
            "source_message_ids".to_string(),
            Value::Array(source_message_ids.into_iter().map(Value::from).collect()),
        );
    }
}

/// Returns the extracted value for `keys` iff every stamp's payload has it and
/// they all agree; otherwise `None` (the field is not uniform across the batch,
/// so there is no single authoritative value to stamp — e.g. a genuinely
/// cross-chat review). `extract` is `value_i64_field`/`value_string_field`.
fn uniform_stamp<T: PartialEq>(
    stamps: &[MonitorSourceStampContext],
    keys: &[&str],
    extract: impl Fn(&Value, &[&str]) -> Option<T>,
) -> Option<T> {
    let mut agreed: Option<T> = None;
    for stamp in stamps {
        let value = extract(&stamp.payload, keys)?;
        match &agreed {
            None => agreed = Some(value),
            Some(existing) if *existing == value => {}
            Some(_) => return None,
        }
    }
    agreed
}

fn reject_llm_written_typed_monitor_fields(metadata: &Map<String, Value>) -> Result<()> {
    for key in [
        "monitor",
        "source_context",
        "sourceContext",
        "completion_policy",
        "completionPolicy",
        "delivery_target",
        "deliveryTarget",
        "source_context_hash",
        "sourceContextHash",
    ] {
        if metadata.contains_key(key) {
            bail!(
                "monitor TaskCreate metadata field `{key}` is server-owned; use sourceEnvelopeId"
            );
        }
    }
    Ok(())
}

fn selected_monitor_source_stamps(
    state: &AppState,
    parsed: &TaskCreateInput,
    metadata: &Map<String, Value>,
) -> Result<Vec<MonitorSourceStampContext>> {
    let mut ids = parsed
        .source_envelope_ids
        .iter()
        .filter_map(|id| non_empty_str(id).map(ToString::to_string))
        .collect::<Vec<_>>();
    if let Some(id) = parsed.source_envelope_id.as_deref().and_then(non_empty_str) {
        ids.push(id.to_string());
    }
    // Transitional compatibility: older prompts wrote the selector under
    // metadata.monitor_envelope_id. It is validated against the current batch
    // and then overwritten as server-owned metadata.
    if ids.is_empty() {
        if let Some(id) = metadata_string(metadata, &["monitor_envelope_id", "monitorEnvelopeId"]) {
            ids.push(id);
        }
    }
    if ids.is_empty() && state.monitor_source_stamp_contexts.len() == 1 {
        ids.push(state.monitor_source_stamp_contexts[0].envelope_id.clone());
    }
    if ids.is_empty() {
        bail!("monitor TaskCreate requires sourceEnvelopeId for multi-trigger batches");
    }
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(context) = state
            .monitor_source_stamp_contexts
            .iter()
            .find(|context| context.envelope_id == id)
        else {
            bail!("sourceEnvelopeId `{id}` is not in the current workflow trigger batch");
        };
        selected.push(context.clone());
    }
    Ok(selected)
}

fn apply_stamped_monitor_contract(
    metadata: &mut Map<String, Value>,
    contract: MonitorContract,
) -> Result<()> {
    let mut monitor = Map::new();
    monitor.insert(
        "schema_version".to_string(),
        Value::from(MONITOR_SCHEMA_VERSION),
    );
    monitor.insert(
        "kind".to_string(),
        Value::String(contract.kind.as_str().to_string()),
    );
    monitor.insert("source".to_string(), Value::Object(contract.source.clone()));
    monitor.insert("action".to_string(), Value::Object(contract.action.clone()));
    metadata.insert("monitor".to_string(), Value::Object(monitor));
    stamp_legacy_monitor_scope_fields(metadata, &contract);
    normalize_monitor_task_metadata(metadata)?;
    Ok(())
}

fn stamp_legacy_monitor_scope_fields(
    metadata: &mut Map<String, Value>,
    contract: &MonitorContract,
) {
    stamp_source_string_as_metadata(
        metadata,
        &contract.source,
        "monitor_connection",
        &["connection_slug", "connectionSlug"],
    );
    stamp_source_string_as_metadata(
        metadata,
        &contract.source,
        "monitor_connector",
        &["connector_slug", "connectorSlug"],
    );
    for key in [
        "chat_id",
        "chat_kind",
        "sender_id",
        "sender_username",
        "thread_id",
        "account",
        "account_id",
        "calendar_id",
        "event_id",
    ] {
        stamp_source_value_as_metadata(metadata, &contract.source, key, &[key]);
    }
    if !metadata.contains_key("from_email") {
        if let Some(email) = contract
            .source
            .get("from")
            .and_then(Value::as_object)
            .and_then(|from| from.get("email"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|email| !email.is_empty())
        {
            metadata.insert("from_email".to_string(), Value::String(email.to_string()));
        }
    }
}

fn stamp_source_string_as_metadata(
    metadata: &mut Map<String, Value>,
    source: &Map<String, Value>,
    metadata_key: &str,
    source_keys: &[&str],
) {
    if let Some(value) = value_string_field(&Value::Object(source.clone()), source_keys) {
        metadata.insert(metadata_key.to_string(), Value::String(value));
    }
}

fn stamp_source_value_as_metadata(
    metadata: &mut Map<String, Value>,
    source: &Map<String, Value>,
    metadata_key: &str,
    source_keys: &[&str],
) {
    let Some(value) = source_keys.iter().find_map(|key| source.get(*key)) else {
        return;
    };
    if value.is_null() {
        return;
    }
    if value.as_str().is_some_and(|value| value.trim().is_empty()) {
        return;
    }
    metadata.insert(metadata_key.to_string(), value.clone());
}

fn typed_contract_from_stamp(stamp: &MonitorSourceStampContext) -> MonitorContract {
    let connector_slug = stamp.connector_slug.as_deref().unwrap_or_default();
    if connector_slug.contains("telegram") || stamp.connection_slug.contains("telegram") {
        return telegram_contract_from_stamp(stamp);
    }
    if connector_slug == "gmail-browser" || stamp.connection_slug.contains("gmail") {
        return gmail_contract_from_stamp(stamp);
    }
    if connector_slug == "gcal-browser"
        || connector_slug.contains("calendar")
        || stamp.connection_slug.contains("gcal")
        || stamp.connection_slug.contains("calendar")
    {
        return calendar_contract_from_stamp(stamp);
    }
    generic_review_contract_from_stamps(&[stamp.clone()])
}

fn telegram_contract_from_stamp(stamp: &MonitorSourceStampContext) -> MonitorContract {
    let mut source = Map::new();
    let connector_slug = stamp
        .connector_slug
        .clone()
        .unwrap_or_else(|| "telegram-login".to_string());
    source.insert("connector_slug".to_string(), Value::String(connector_slug));
    source.insert(
        "connection_slug".to_string(),
        Value::String(stamp.connection_slug.clone()),
    );
    let payload = &stamp.payload;
    copy_value_field(payload, &mut source, "chat_id", &["chat_id", "chatId"]);
    copy_string_field(
        payload,
        &mut source,
        "chat_kind",
        &["chat_kind", "chatKind"],
    );
    copy_value_field(
        payload,
        &mut source,
        "message_id",
        &["message_id", "messageId"],
    );
    copy_value_field(
        payload,
        &mut source,
        "sender_id",
        &["sender_id", "senderId"],
    );
    copy_string_field(
        payload,
        &mut source,
        "sender_username",
        &["sender_username", "senderUsername", "username"],
    );
    copy_string_field(
        payload,
        &mut source,
        "sender_name",
        &["sender_name", "senderName", "sender"],
    );
    source.insert(
        "envelope_id".to_string(),
        Value::String(stamp.envelope_id.clone()),
    );
    if let Some(text) = stamp.text.as_ref() {
        source.insert("text".to_string(), Value::String(text.clone()));
    }
    MonitorContract {
        schema_version: MONITOR_SCHEMA_VERSION,
        kind: MonitorTaskKind::TelegramReply,
        source,
        action: map_from_pairs(&[
            ("type", "telegram_reply_draft"),
            ("approval", "draft_then_send"),
        ]),
        source_hash: None,
    }
}

fn gmail_contract_from_stamp(stamp: &MonitorSourceStampContext) -> MonitorContract {
    let payload = &stamp.payload;
    let message = payload.get("message").unwrap_or(payload);
    let mut source = Map::new();
    source.insert(
        "connector_slug".to_string(),
        Value::String(
            stamp
                .connector_slug
                .clone()
                .unwrap_or_else(|| "gmail-browser".to_string()),
        ),
    );
    source.insert(
        "connection_slug".to_string(),
        Value::String(stamp.connection_slug.clone()),
    );
    copy_string_field(
        payload,
        &mut source,
        "account",
        &["account", "account_id", "accountId", "accountEmail"],
    );
    copy_string_field(
        message,
        &mut source,
        "thread_id",
        &["threadId", "thread_id", "thread_id_hex"],
    );
    copy_string_field(
        message,
        &mut source,
        "message_id",
        &["id", "message_id", "messageId"],
    );
    copy_string_field(message, &mut source, "subject", &["subject", "title"]);
    copy_string_field(
        message,
        &mut source,
        "url",
        &["url", "html_link", "htmlLink"],
    );
    let mut from = Map::new();
    if let Some(name) = value_string_field(message, &["sender", "senderName", "fromName", "name"])
        .or_else(|| value_string_field(payload, &["sender", "senderName", "fromName", "name"]))
    {
        from.insert("name".to_string(), Value::String(name));
    }
    if let Some(email) = value_string_field(
        message,
        &[
            "fromEmail",
            "from_email",
            "senderEmail",
            "sender_email",
            "email",
        ],
    )
    .or_else(|| {
        value_string_field(
            payload,
            &[
                "fromEmail",
                "from_email",
                "senderEmail",
                "sender_email",
                "email",
            ],
        )
    }) {
        from.insert("email".to_string(), Value::String(email));
    }
    if !from.is_empty() {
        source.insert("from".to_string(), Value::Object(from));
    }
    source.insert(
        "envelope_id".to_string(),
        Value::String(stamp.envelope_id.clone()),
    );
    if let Some(text) = stamp
        .text
        .as_ref()
        .cloned()
        .or_else(|| value_string_field(message, &["snippet", "text", "body"]))
    {
        source.insert("text".to_string(), Value::String(text));
    }
    let is_executable = required_gmail_source_present(&source);
    let action = if is_executable {
        map_from_pairs(&[
            ("type", "gmail_reply_draft"),
            ("approval", "draft_then_create_gmail_draft"),
        ])
    } else {
        map_from_pairs(&[("type", "review_only")])
    };
    MonitorContract {
        schema_version: MONITOR_SCHEMA_VERSION,
        kind: if is_executable {
            MonitorTaskKind::GmailReply
        } else {
            MonitorTaskKind::GenericReview
        },
        source,
        action,
        source_hash: None,
    }
}

fn calendar_contract_from_stamp(stamp: &MonitorSourceStampContext) -> MonitorContract {
    let payload = &stamp.payload;
    let event = payload.get("event").unwrap_or(payload);
    let mut source = Map::new();
    source.insert(
        "connector_slug".to_string(),
        Value::String(
            stamp
                .connector_slug
                .clone()
                .unwrap_or_else(|| "gcal-browser".to_string()),
        ),
    );
    source.insert(
        "connection_slug".to_string(),
        Value::String(stamp.connection_slug.clone()),
    );
    copy_string_field(
        payload,
        &mut source,
        "account",
        &["account", "account_id", "accountId", "accountEmail"],
    );
    copy_string_field(
        event,
        &mut source,
        "calendar_id",
        &["calendar_id", "calendarId", "calendar"],
    );
    copy_string_field(
        event,
        &mut source,
        "event_id",
        &["event_id", "eventId", "id"],
    );
    copy_string_field(
        event,
        &mut source,
        "html_link",
        &["html_link", "htmlLink", "url"],
    );
    copy_string_field(event, &mut source, "summary", &["summary", "title"]);
    copy_string_field(
        event,
        &mut source,
        "organizer_email",
        &["organizer_email", "organizerEmail", "organizer"],
    );
    copy_string_field(
        event,
        &mut source,
        "start",
        &["start", "start_time", "startTime"],
    );
    copy_string_field(event, &mut source, "end", &["end", "end_time", "endTime"]);
    source.insert(
        "envelope_id".to_string(),
        Value::String(stamp.envelope_id.clone()),
    );
    if let Some(text) = stamp.text.as_ref() {
        source.insert("text".to_string(), Value::String(text.clone()));
    }
    let is_executable = string_field_from_map(&source, &["event_id", "eventId"]).is_some();
    let mut action = if is_executable {
        map_from_pairs(&[
            ("type", "calendar_rsvp"),
            ("approval", "confirm_then_execute"),
        ])
    } else {
        map_from_pairs(&[("type", "review_only")])
    };
    if is_executable {
        action.insert("allowed_responses".to_string(), json!(["accept", "deny"]));
    }
    MonitorContract {
        schema_version: MONITOR_SCHEMA_VERSION,
        kind: if is_executable {
            MonitorTaskKind::CalendarRsvp
        } else {
            MonitorTaskKind::GenericReview
        },
        source,
        action,
        source_hash: None,
    }
}

fn generic_review_contract_from_stamps(stamps: &[MonitorSourceStampContext]) -> MonitorContract {
    let mut source = Map::new();
    if let Some(first) = stamps.first() {
        source.insert(
            "connector_slug".to_string(),
            Value::String(
                first
                    .connector_slug
                    .clone()
                    .unwrap_or_else(|| first.connection_slug.clone()),
            ),
        );
        source.insert(
            "connection_slug".to_string(),
            Value::String(first.connection_slug.clone()),
        );
    }
    // Uniform chat identity keeps a same-chat burst deliverable: the reply
    // layer resolves its chat-level target from the contract source, exactly
    // like a single-source telegram task (agentenv/monorepo#761, #722).
    if let Some(chat_id) = uniform_stamp(stamps, &["chat_id", "chatId"], value_i64_field) {
        source.insert("chat_id".to_string(), Value::from(chat_id));
        if let Some(chat_kind) =
            uniform_stamp(stamps, &["chat_kind", "chatKind"], value_string_field)
        {
            source.insert("chat_kind".to_string(), Value::String(chat_kind));
        }
    }
    let source_message_ids = sorted_source_message_ids(stamps);
    if !source_message_ids.is_empty() {
        source.insert(
            "source_message_ids".to_string(),
            Value::Array(source_message_ids.into_iter().map(Value::from).collect()),
        );
    }
    source.insert(
        "envelope_ids".to_string(),
        Value::Array(
            stamps
                .iter()
                .map(|stamp| Value::String(stamp.envelope_id.clone()))
                .collect(),
        ),
    );
    let text = stamps
        .iter()
        .filter_map(|stamp| stamp.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !text.trim().is_empty() {
        source.insert("text".to_string(), Value::String(text));
    }
    source.insert(
        "summary".to_string(),
        Value::String("Monitor item".to_string()),
    );
    MonitorContract {
        schema_version: MONITOR_SCHEMA_VERSION,
        kind: MonitorTaskKind::GenericReview,
        source,
        action: map_from_pairs(&[("type", "review_only")]),
        source_hash: None,
    }
}

fn required_gmail_source_present(source: &Map<String, Value>) -> bool {
    string_field_from_map(source, &["thread_id", "threadId"]).is_some()
}

/// Distinct per-message ids across a stamp batch, sorted for stable output.
fn sorted_source_message_ids(stamps: &[MonitorSourceStampContext]) -> Vec<i64> {
    let mut ids = stamps
        .iter()
        .filter_map(source_message_id_from_stamp)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn source_message_id_from_stamp(stamp: &MonitorSourceStampContext) -> Option<i64> {
    let payload = &stamp.payload;
    value_i64_field(payload, &["message_id", "messageId"]).or_else(|| {
        payload
            .get("message")
            .and_then(|message| value_i64_field(message, &["id", "message_id", "messageId"]))
    })
}

fn source_messages_from_stamp(stamp: &MonitorSourceStampContext) -> Option<Value> {
    if !stamp
        .connector_slug
        .as_deref()
        .is_some_and(|slug| slug.contains("telegram"))
        && !stamp.connection_slug.contains("telegram")
    {
        return None;
    }

    let mut messages = stamp
        .payload
        .get("conversation_context")
        .and_then(|context| context.get("messages"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(current) = current_telegram_source_message_from_stamp(stamp) {
        messages.push(current);
    }
    if messages.is_empty() {
        return None;
    }
    let start = messages.len().saturating_sub(MONITOR_SOURCE_MESSAGES_LIMIT);
    Some(Value::Array(messages.into_iter().skip(start).collect()))
}

fn current_telegram_source_message_from_stamp(stamp: &MonitorSourceStampContext) -> Option<Value> {
    let text = stamp
        .text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let payload = &stamp.payload;
    let is_outgoing = value_bool_field(
        payload,
        &["is_outgoing", "isOutgoing", "outgoing", "from_me", "fromMe"],
    ) || value_string_field(payload, &["direction"])
        .is_some_and(|direction| direction.eq_ignore_ascii_case("outgoing"));
    let direction = if is_outgoing { "outgoing" } else { "incoming" };
    let from = if is_outgoing { "me" } else { "them" };
    let sender_label = value_string_field(payload, &["sender_name", "senderName", "sender"])
        .or_else(|| value_string_field(payload, &["chat_title", "chatTitle"]))
        .unwrap_or_else(|| {
            if is_outgoing {
                "me".to_string()
            } else {
                "sender".to_string()
            }
        });
    let sender_username =
        value_string_field(payload, &["sender_username", "senderUsername", "username"]);
    let chat_id = payload
        .get("chat_id")
        .or_else(|| payload.get("chatId"))
        .cloned()
        .unwrap_or(Value::Null);
    let chat_title = value_string_field(payload, &["chat_title", "chatTitle"]);
    let message_id = source_message_id_from_stamp(stamp);
    let date_ms = value_i64_field(
        payload,
        &["date_ms", "dateMs", "event_date_ms", "eventDateMs"],
    );

    Some(json!({
        "from": from,
        "direction": direction,
        "sender": {
            "label": sender_label,
            "username": sender_username,
            "is_user": is_outgoing,
        },
        "chat": {
            "id": chat_id,
            "title": chat_title,
        },
        "message_id": message_id,
        "date_ms": date_ms,
        "ts": date_ms,
        "reply_to": payload.get("reply_to").cloned().unwrap_or(Value::Null),
        "text": text,
        "is_outgoing": is_outgoing,
    }))
}

fn copy_value_field(source: &Value, target: &mut Map<String, Value>, out_key: &str, keys: &[&str]) {
    if let Some(value) = keys.iter().find_map(|key| source.get(*key)).cloned() {
        target.insert(out_key.to_string(), value);
    }
}

fn copy_string_field(
    source: &Value,
    target: &mut Map<String, Value>,
    out_key: &str,
    keys: &[&str],
) {
    if let Some(value) = value_string_field(source, keys) {
        target.insert(out_key.to_string(), Value::String(value));
    }
}

fn map_from_pairs(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
        .collect()
}

fn non_empty_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
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
        "monitor" => Some(&["monitor"]),
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
        "monitor_reply_events" | "monitorReplyEvents" => {
            Some(&["monitor_reply_events", "monitorReplyEvents"])
        }
        "source_context_hash" | "sourceContextHash" => {
            Some(&["source_context_hash", "sourceContextHash"])
        }
        _ => None,
    }
}

fn normalize_monitor_task_metadata(metadata: &mut Map<String, Value>) -> Result<()> {
    if let Some(contract) = parse_monitor_contract(metadata)? {
        let source_context = display_source_context(&contract);
        let default_completion_policy =
            default_monitor_completion_policy(metadata, Some(&source_context));
        metadata.insert("source_context".to_string(), source_context);
        if let Some(default_completion_policy) = default_completion_policy {
            metadata
                .entry("completion_policy".to_string())
                .or_insert(default_completion_policy);
        }
        let source_hash = monitor_contract_hash(&contract)?;
        if let Some(monitor) = metadata.get_mut("monitor").and_then(Value::as_object_mut) {
            monitor.insert("source_hash".to_string(), Value::String(source_hash));
        }
        return Ok(());
    }
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
    Ok(())
}

fn monitor_source_context(metadata: &Map<String, Value>) -> Option<Value> {
    if let Some(contract) = parse_monitor_contract(metadata).ok().flatten() {
        return with_verbatim_source_text(metadata, Some(display_source_context(&contract)));
    }
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

fn monitor_task_requires_human_approval(task: &StoredTask) -> bool {
    if !is_monitor_task_metadata(&task.metadata) {
        return false;
    }
    let source_context = monitor_source_context(&task.metadata);
    monitor_completion_policy(&task.metadata, source_context.as_ref())
        .as_ref()
        .is_some_and(completion_policy_requires_human_approval)
}

fn monitor_task_is_typed_executable(task: &StoredTask) -> bool {
    parse_monitor_contract(&task.metadata)
        .ok()
        .flatten()
        .is_some_and(|contract| contract.kind.as_str() != "generic.review")
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

fn metadata_i64(metadata: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| metadata.get(*key))
        .and_then(value_i64)
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

fn value_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    value
        .as_object()
        .and_then(|object| string_field_from_map(object, keys))
}

fn value_i64_field(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(value_i64)
}

fn value_bool_field(value: &Value, keys: &[&str]) -> bool {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn value_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok())),
        Value::String(value) => value.trim().parse::<i64>().ok(),
        _ => None,
    }
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

    #[test]
    fn task_create_stamps_typed_gmail_monitor_context_and_hash() {
        let (mut state, tmp) = make_state();

        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            json!({
                "subject": "Confirm next week's meeting",
                "description": "Reply to the Gmail thread with available times.",
                "receivedAt": "2026-06-10T13:00:00Z",
                "expiresAt": "2026-06-11T13:00:00Z",
                "actions": [{
                    "actionName": "Draft email",
                    "actionPrompt": "Draft a reply to the Gmail sender."
                }],
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
                        },
                        "source_hash": "sha256:stale"
                    }
                }
            }),
        )
        .unwrap();
        let payload: Value = serde_json::from_str(&raw).unwrap();
        let task_id = payload.pointer("/task/id").and_then(Value::as_str).unwrap();

        let task = load_monitor_task(tmp.path(), task_id).unwrap().unwrap();
        let metadata = Value::Object(task.metadata.clone());
        assert_eq!(
            metadata.pointer("/source_context/kind"),
            Some(&json!("gmail_message"))
        );
        assert_eq!(
            metadata.pointer("/source_context/delivery_target/type"),
            Some(&json!("gmail_thread"))
        );
        assert_eq!(
            metadata.pointer("/source_context/delivery_target/thread_id"),
            Some(&json!("thread-123"))
        );
        assert_eq!(
            metadata.pointer("/source_context/sender/email"),
            Some(&json!("fuxiangyu@example.com"))
        );
        assert_eq!(
            metadata.pointer("/monitor/source_hash"),
            Some(&json!(
                "sha256:b8e1bc99df97a47171b03fd10a708fb4c8220f8ae5cbe59e5c6ce4005cc847b2"
            ))
        );
        assert_eq!(
            metadata.pointer("/completion_policy/mode"),
            Some(&json!("draft_then_approve"))
        );
    }

    #[test]
    fn task_create_stamps_gmail_monitor_from_current_source_envelope() {
        let (mut state, tmp) = make_state();
        state.set_monitor_source_stamp_contexts(vec![crate::MonitorSourceStampContext {
            envelope_id: "env-gmail".to_string(),
            connection_slug: "gmail-browser".to_string(),
            connector_slug: Some("gmail-browser".to_string()),
            received_at_ms: None,
            text: Some("Fu Xiangyu\nConfirm meeting\nWhat time works?".to_string()),
            payload: json!({
                "account": "winterfell0614@gmail.com",
                "message": {
                    "id": "message-123",
                    "threadId": "thread-123",
                    "subject": "Confirm meeting",
                    "sender": "Fu Xiangyu",
                    "fromEmail": "fuxiangyu@example.com",
                    "url": "https://mail.google.com/mail/u/0/#inbox/thread-123"
                }
            }),
        }]);

        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            json!({
                "subject": "Reply with meeting availability",
                "description": "The sender asks what time works.",
                "receivedAt": "2026-06-10T13:00:00Z",
                "expiresAt": "2026-06-11T13:00:00Z",
                "sourceEnvelopeId": "env-gmail",
                "actions": [{
                    "actionName": "Draft email",
                    "actionPrompt": "Draft a reply to the sender."
                }],
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "gmail-browser",
                    "monitor_connector": "gmail-browser",
                    "monitor_memory_path": "/tmp/gmail-browser.md",
                    "thread_id": "thread-123",
                    "from_email": "fuxiangyu@example.com"
                }
            }),
        )
        .unwrap();
        let payload: Value = serde_json::from_str(&raw).unwrap();
        let task_id = payload.pointer("/task/id").and_then(Value::as_str).unwrap();

        let task = load_monitor_task(tmp.path(), task_id).unwrap().unwrap();
        let metadata = Value::Object(task.metadata);
        assert_eq!(
            metadata.pointer("/monitor/kind"),
            Some(&json!("gmail.reply"))
        );
        assert_eq!(
            metadata.pointer("/monitor/source/thread_id"),
            Some(&json!("thread-123"))
        );
        assert_eq!(
            metadata.pointer("/monitor/source/from/email"),
            Some(&json!("fuxiangyu@example.com"))
        );
        assert_eq!(
            metadata.pointer("/source_context/delivery_target/thread_id"),
            Some(&json!("thread-123"))
        );
        assert_eq!(
            metadata.pointer("/monitor_envelope_id"),
            Some(&json!("env-gmail"))
        );
    }

    #[test]
    fn task_create_rejects_llm_written_typed_delivery_fields_when_source_stamped() {
        let (mut state, tmp) = make_state();
        state.set_monitor_source_stamp_contexts(vec![crate::MonitorSourceStampContext {
            envelope_id: "env-gmail".to_string(),
            connection_slug: "gmail-browser".to_string(),
            connector_slug: Some("gmail-browser".to_string()),
            received_at_ms: None,
            text: Some("hello".to_string()),
            payload: json!({
                "account": "winterfell0614@gmail.com",
                "message": {
                    "id": "message-123",
                    "threadId": "thread-123",
                    "fromEmail": "fuxiangyu@example.com"
                }
            }),
        }]);

        let error = execute_task_create(
            &mut state,
            tmp.path(),
            json!({
                "subject": "Reply with meeting availability",
                "description": "The sender asks what time works.",
                "receivedAt": "2026-06-10T13:00:00Z",
                "expiresAt": "2026-06-11T13:00:00Z",
                "sourceEnvelopeId": "env-gmail",
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "gmail-browser",
                    "monitor_connector": "gmail-browser",
                    "monitor_memory_path": "/tmp/gmail-browser.md",
                    "source_context": {
                        "delivery_target": {
                            "type": "telegram_chat",
                            "chat_id": "999"
                        }
                    }
                }
            }),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("metadata field `source_context` is server-owned"));
    }

    #[test]
    fn task_create_stamps_calendar_contract_from_current_source_envelope() {
        let (mut state, tmp) = make_state();
        state.set_monitor_source_stamp_contexts(vec![crate::MonitorSourceStampContext {
            envelope_id: "env-calendar".to_string(),
            connection_slug: "gcal-browser".to_string(),
            connector_slug: Some("gcal-browser".to_string()),
            received_at_ms: None,
            text: Some("Project sync invitation".to_string()),
            payload: json!({
                "account": "winterfell0614@gmail.com",
                "event": {
                    "id": "event-123",
                    "calendar_id": "primary",
                    "summary": "Project sync",
                    "organizer_email": "organizer@example.com",
                    "start": "2026-06-25T10:00:00+08:00",
                    "end": "2026-06-25T10:30:00+08:00",
                    "htmlLink": "https://calendar.google.com/calendar/event?eid=event-123"
                }
            }),
        }]);

        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            json!({
                "subject": "Review Project sync invitation",
                "description": "Decide whether to accept the Project sync invitation.",
                "receivedAt": "2026-06-10T13:00:00Z",
                "expiresAt": "2026-06-11T13:00:00Z",
                "sourceEnvelopeId": "env-calendar",
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "gcal-browser",
                    "monitor_connector": "gcal-browser",
                    "monitor_memory_path": "/tmp/gcal-browser.md"
                }
            }),
        )
        .unwrap();
        let payload: Value = serde_json::from_str(&raw).unwrap();
        let task_id = payload.pointer("/task/id").and_then(Value::as_str).unwrap();

        let task = load_monitor_task(tmp.path(), task_id).unwrap().unwrap();
        let metadata = Value::Object(task.metadata);
        assert_eq!(
            metadata.pointer("/monitor/kind"),
            Some(&json!("calendar.rsvp"))
        );
        assert_eq!(
            metadata.pointer("/source_context/delivery_target/event_id"),
            Some(&json!("event-123"))
        );
    }

    #[test]
    fn task_create_stamps_telegram_legacy_scope_fields_from_current_source_envelope() {
        let (mut state, tmp) = make_state();
        state.set_monitor_source_stamp_contexts(vec![crate::MonitorSourceStampContext {
            envelope_id: "env-6836".to_string(),
            connection_slug: "telegram-user".to_string(),
            connector_slug: Some("telegram-login".to_string()),
            received_at_ms: None,
            text: Some("What's the latest on WLF?".to_string()),
            payload: json!({
                "chat_id": 42,
                "chat_kind": "user",
                "message_id": 6836,
                "date_ms": 1_009,
                "reply_to": {
                    "kind": "message",
                    "message_id": 6835
                },
                "sender_id": "8759047281",
                "sender_username": "alice",
                "sender_name": "Alice",
                "conversation_context": {
                    "kind": "telegram_prior_messages",
                    "scope": "same_chat_before_current_message",
                    "messages": [
                        { "from": "them", "direction": "incoming", "text": "prior 1", "date_ms": 1_001 },
                        { "from": "me", "direction": "outgoing", "text": "在", "date_ms": 1_002, "reply_to": { "kind": "message", "message_id": 6801 } },
                        { "from": "them", "direction": "incoming", "text": "prior 3", "date_ms": 1_003 },
                        { "from": "me", "direction": "outgoing", "text": "prior 4", "date_ms": 1_004 },
                        { "from": "them", "direction": "incoming", "text": "prior 5", "date_ms": 1_005 },
                        { "from": "them", "direction": "incoming", "text": "prior 6", "date_ms": 1_006 },
                        { "from": "them", "direction": "incoming", "text": "prior 7", "date_ms": 1_007 },
                        { "from": "them", "direction": "incoming", "text": "prior 8", "date_ms": 1_008 }
                    ]
                }
            }),
        }]);
        configure_telegram_gate(
            &mut state,
            &tmp,
            "env-6836",
            activity_state(Vec::new(), None),
        );

        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            json!({
                "subject": "Reply to Alice about WLF latest",
                "description": "Alice asked for the latest WLF update.",
                "receivedAt": "2026-06-10T13:00:00Z",
                "expiresAt": "2026-06-11T13:00:00Z",
                "sourceEnvelopeId": "env-6836",
                "metadata": {
                    "_monitor": true,
                    "monitor_memory_path": "/tmp/telegram-user.md"
                }
            }),
        )
        .unwrap();
        let payload: Value = serde_json::from_str(&raw).unwrap();
        let task_id = payload.pointer("/task/id").and_then(Value::as_str).unwrap();

        let task = load_monitor_task(tmp.path(), task_id).unwrap().unwrap();
        let metadata = Value::Object(task.metadata);
        assert_eq!(
            metadata.pointer("/monitor_connection"),
            Some(&json!("telegram-user"))
        );
        assert_eq!(
            metadata.pointer("/monitor_connector"),
            Some(&json!("telegram-login"))
        );
        assert_eq!(metadata.pointer("/chat_id"), Some(&json!(42)));
        assert_eq!(metadata.pointer("/chat_kind"), Some(&json!("user")));
        assert_eq!(metadata.pointer("/sender_id"), Some(&json!("8759047281")));
        assert_eq!(
            metadata.pointer("/monitor_task_gate/decision"),
            Some(&json!("create_unknown"))
        );
        let source_messages = metadata
            .pointer("/source_messages")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(source_messages.len(), 8);
        assert_eq!(source_messages[0]["text"], "在");
        assert_eq!(source_messages[0]["direction"], "outgoing");
        assert_eq!(source_messages[0]["reply_to"]["message_id"], 6801);
        assert_eq!(source_messages[7]["text"], "What's the latest on WLF?");
        assert_eq!(source_messages[7]["direction"], "incoming");
        assert_eq!(source_messages[7]["sender"]["username"], "alice");
        assert_eq!(source_messages[7]["chat"]["id"], 42);
        assert_eq!(source_messages[7]["message_id"], 6836);
        assert_eq!(source_messages[7]["reply_to"]["message_id"], 6835);
    }

    #[test]
    fn task_update_cannot_directly_complete_typed_gmail_monitor_task() {
        let (mut state, tmp) = make_state();
        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            json!({
                "subject": "Confirm next week's meeting",
                "description": "Reply to the Gmail thread with available times.",
                "receivedAt": "2026-06-10T13:00:00Z",
                "expiresAt": "2026-06-11T13:00:00Z",
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "gmail-browser",
                    "monitor_connector": "gmail-browser",
                    "monitor_memory_path": "/tmp/gmail-browser.md",
                    "monitor": {
                        "schema_version": 2,
                        "kind": "gmail.reply",
                        "source": {
                            "connector_slug": "gmail-browser",
                            "connection_slug": "gmail-browser",
                            "thread_id": "thread-123",
                            "message_id": "message-123"
                        },
                        "action": {
                            "type": "gmail_reply_draft"
                        }
                    }
                }
            }),
        )
        .unwrap();
        let payload: Value = serde_json::from_str(&raw).unwrap();
        let task_id = payload.pointer("/task/id").and_then(Value::as_str).unwrap();

        let error = execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "status": "completed"
            }),
        )
        .unwrap_err();

        assert!(error.to_string().contains(
            "typed monitor task `monitor-1` must be completed through its monitor action"
        ));
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

    fn telegram_monitor_task_input(envelope_id: &str) -> Value {
        json!({
            "subject": "Reply to Chaofan about launch risk",
            "description": "Chaofan asked whether the launch risk is still P1.",
            "receivedAt": "2026-06-10T13:00:00Z",
            "expiresAt": "2026-06-11T13:00:00Z",
            "metadata": {
                "_monitor": true,
                "monitor_connection": "telegram-user",
                "monitor_connector": "telegram-login",
                "monitor_envelope_id": envelope_id,
                "chat_id": "42",
                "chat_kind": "user",
                "sender_id": "42"
            },
            "actions": [
                {
                    "actionName": "Reply",
                    "actionPrompt": "Research the answer and draft a reply."
                }
            ]
        })
    }

    fn configure_telegram_gate(
        state: &mut AppState,
        tmp: &TempDir,
        envelope_id: &str,
        activity: Value,
    ) {
        configure_telegram_gate_with_source_message(
            state,
            tmp,
            envelope_id,
            6836,
            Some(1_000),
            activity,
        );
    }

    fn configure_telegram_gate_with_source_message(
        state: &mut AppState,
        tmp: &TempDir,
        envelope_id: &str,
        source_message_id: i64,
        source_date_ms: Option<i64>,
        activity: Value,
    ) {
        let activity_path = tmp.path().join("telegram-activity-state.json");
        std::fs::write(
            &activity_path,
            serde_json::to_vec_pretty(&activity).unwrap(),
        )
        .unwrap();
        state.set_monitor_task_create_gate_contexts(vec![crate::MonitorTaskCreateGateContext {
            envelope_id: envelope_id.to_string(),
            connection_slug: "telegram-user".to_string(),
            connector_slug: Some("telegram-login".to_string()),
            chat_id: 42,
            chat_kind: "user".to_string(),
            source_message_id,
            source_date_ms,
            activity_state_path: activity_path,
            monitor_trace_path: None,
        }]);
    }

    fn activity_state(messages: Vec<Value>, read_inbox_max_id: Option<i64>) -> Value {
        json!({
            "version": 1,
            "source": "telegram_subscriber_activity",
            "updated_at_ms": 1_500,
            "chats": [
                {
                    "chat_id": 42,
                    "chat_kind": "user",
                    "updated_at_ms": 1_500,
                    "read_inbox_max_id": read_inbox_max_id,
                    "agent_sent_message_ids": [9001],
                    "messages": messages
                }
            ]
        })
    }

    #[test]
    fn task_create_skips_telegram_monitor_when_exact_human_reply_seen() {
        let (mut state, tmp) = make_state();
        configure_telegram_gate(
            &mut state,
            &tmp,
            "env-6836",
            activity_state(
                vec![json!({
                    "message_id": 7001,
                    "date_ms": 1_200,
                    "is_outgoing": true,
                    "reply_to_message_id": 6836
                })],
                None,
            ),
        );

        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            telegram_monitor_task_input("env-6836"),
        )
        .expect("skip should be a success-shaped TaskCreate result");
        let payload: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(payload["success"], true);
        assert_eq!(payload["skipped"], true);
        assert_eq!(payload["reason"], "handled_in_telegram");
        assert_eq!(
            payload.pointer("/gate/decision").and_then(Value::as_str),
            Some("skip_handled")
        );
        let store = load_store::<TaskStore>(&monitor_tasks_path(tmp.path())).unwrap();
        assert!(
            store.tasks.is_empty(),
            "skipped monitor task must not be written"
        );
    }

    #[test]
    fn task_create_does_not_skip_for_unrelated_later_outgoing() {
        let (mut state, tmp) = make_state();
        configure_telegram_gate(
            &mut state,
            &tmp,
            "env-6836",
            activity_state(
                vec![json!({
                    "message_id": 7001,
                    "date_ms": 1_200,
                    "is_outgoing": true
                })],
                None,
            ),
        );

        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            telegram_monitor_task_input("env-6836"),
        )
        .unwrap();
        let payload: Value = serde_json::from_str(&raw).unwrap();
        let task_id = payload.pointer("/task/id").and_then(Value::as_str).unwrap();

        let task = load_monitor_task(tmp.path(), task_id).unwrap().unwrap();
        let metadata = Value::Object(task.metadata.clone());
        assert_eq!(
            metadata
                .pointer("/monitor_task_gate/decision")
                .and_then(Value::as_str),
            Some("create_unknown")
        );
        assert_eq!(
            metadata
                .pointer("/monitor_task_gate/replied")
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn task_create_does_not_skip_for_agent_originated_exact_reply() {
        let (mut state, tmp) = make_state();
        configure_telegram_gate(
            &mut state,
            &tmp,
            "env-6836",
            activity_state(
                vec![json!({
                    "message_id": 9001,
                    "date_ms": 1_200,
                    "is_outgoing": true,
                    "reply_to_message_id": 6836
                })],
                None,
            ),
        );

        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            telegram_monitor_task_input("env-6836"),
        )
        .unwrap();
        let payload: Value = serde_json::from_str(&raw).unwrap();
        let task_id = payload.pointer("/task/id").and_then(Value::as_str).unwrap();

        let task = load_monitor_task(tmp.path(), task_id).unwrap().unwrap();
        let metadata = Value::Object(task.metadata.clone());
        assert_eq!(
            metadata
                .pointer("/monitor_task_gate/decision")
                .and_then(Value::as_str),
            Some("create_unknown")
        );
        assert_eq!(
            metadata
                .pointer("/monitor_task_gate/replied")
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn task_create_marks_telegram_monitor_read_only() {
        let (mut state, tmp) = make_state();
        configure_telegram_gate(
            &mut state,
            &tmp,
            "env-6836",
            activity_state(Vec::new(), Some(6836)),
        );

        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            telegram_monitor_task_input("env-6836"),
        )
        .unwrap();
        let payload: Value = serde_json::from_str(&raw).unwrap();
        let task_id = payload.pointer("/task/id").and_then(Value::as_str).unwrap();

        let task = load_monitor_task(tmp.path(), task_id).unwrap().unwrap();
        let metadata = Value::Object(task.metadata.clone());
        assert_eq!(
            metadata
                .pointer("/source_state/telegram/read")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            metadata
                .pointer("/monitor_task_gate/decision")
                .and_then(Value::as_str),
            Some("create_read")
        );
        assert_eq!(
            metadata
                .pointer("/monitor_task_gate/basis")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>(),
            vec!["read_inbox_max_id"]
        );
    }

    #[test]
    fn task_create_skips_telegram_monitor_when_envelope_is_not_current_batch_source() {
        let (mut state, tmp) = make_state();
        configure_telegram_gate_with_source_message(
            &mut state,
            &tmp,
            "env-current",
            6837,
            Some(2_000),
            activity_state(Vec::new(), None),
        );

        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            telegram_monitor_task_input("env-prior-context"),
        )
        .expect("untrusted source should be a success-shaped skip");
        let payload: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(payload["success"], true);
        assert_eq!(payload["skipped"], true);
        assert_eq!(payload["reason"], "untrusted_monitor_source");
        let store = load_store::<TaskStore>(&monitor_tasks_path(tmp.path())).unwrap();
        assert!(
            store.tasks.is_empty(),
            "monitor task from an envelope outside the current batch must not be written"
        );
    }

    #[test]
    fn task_create_does_not_apply_direct_source_scope_without_direct_chat_kind() {
        let (mut state, tmp) = make_state();
        configure_telegram_gate_with_source_message(
            &mut state,
            &tmp,
            "env-current",
            6837,
            Some(2_000),
            activity_state(Vec::new(), None),
        );
        let mut input = telegram_monitor_task_input("env-prior-context");
        input
            .get_mut("metadata")
            .and_then(Value::as_object_mut)
            .unwrap()
            .remove("chat_kind");
        input
            .get_mut("metadata")
            .and_then(Value::as_object_mut)
            .unwrap()
            .insert(
                "chat_id".to_string(),
                Value::String("-10012345".to_string()),
            );

        let raw = execute_task_create(&mut state, tmp.path(), input)
            .expect("source scope should only apply to explicit direct Telegram metadata");
        let payload: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(
            payload.pointer("/task/id").and_then(Value::as_str),
            Some("monitor-1")
        );
        let store = load_store::<TaskStore>(&monitor_tasks_path(tmp.path())).unwrap();
        assert_eq!(store.tasks.len(), 1);
    }

    #[test]
    fn task_create_skips_duplicate_telegram_monitor_source() {
        let (mut state, tmp) = make_state();
        configure_telegram_gate(
            &mut state,
            &tmp,
            "env-6836",
            activity_state(Vec::new(), None),
        );
        let first_raw = execute_task_create(
            &mut state,
            tmp.path(),
            telegram_monitor_task_input("env-6836"),
        )
        .unwrap();
        let first_task_id = serde_json::from_str::<Value>(&first_raw).unwrap()["task"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let second_raw = execute_task_create(
            &mut state,
            tmp.path(),
            telegram_monitor_task_input("env-6836"),
        )
        .expect("duplicate source should be a success-shaped skip");
        let second_payload: Value = serde_json::from_str(&second_raw).unwrap();

        assert_eq!(second_payload["success"], true);
        assert_eq!(second_payload["skipped"], true);
        assert_eq!(second_payload["reason"], "duplicate_source");
        assert_eq!(
            second_payload["existingTaskId"].as_str(),
            Some(first_task_id.as_str())
        );
        let store = load_store::<TaskStore>(&monitor_tasks_path(tmp.path())).unwrap();
        assert_eq!(store.tasks.len(), 1);
    }

    #[test]
    fn task_create_skips_duplicate_open_telegram_monitor_subject_in_same_chat() {
        let (mut state, tmp) = make_state();
        configure_telegram_gate(
            &mut state,
            &tmp,
            "env-6836",
            activity_state(Vec::new(), None),
        );
        let first_raw = execute_task_create(
            &mut state,
            tmp.path(),
            telegram_monitor_task_input("env-6836"),
        )
        .unwrap();
        let first_task_id = serde_json::from_str::<Value>(&first_raw).unwrap()["task"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        configure_telegram_gate_with_source_message(
            &mut state,
            &tmp,
            "env-6837",
            6837,
            Some(2_000),
            activity_state(Vec::new(), None),
        );
        let second_raw = execute_task_create(
            &mut state,
            tmp.path(),
            telegram_monitor_task_input("env-6837"),
        )
        .expect("same-chat duplicate subject should be a success-shaped skip");
        let second_payload: Value = serde_json::from_str(&second_raw).unwrap();

        assert_eq!(second_payload["success"], true);
        assert_eq!(second_payload["skipped"], true);
        assert_eq!(second_payload["reason"], "duplicate_monitor_task");
        assert_eq!(
            second_payload["existingTaskId"].as_str(),
            Some(first_task_id.as_str())
        );
        let store = load_store::<TaskStore>(&monitor_tasks_path(tmp.path())).unwrap();
        assert_eq!(store.tasks.len(), 1);
    }

    // ---- agentenv/monorepo#625 regression coverage ----
    // Direct, deterministic exercises of `duplicate_monitor_task_skip` (no LLM).
    // The aligned scope is MESSAGE-IDENTITY ONLY: the same Telegram (chat_id,
    // message_id) re-delivered must never spawn a second task (even after the first
    // is completed/ignored), but two *distinct* messages with identical text are
    // distinct events and may each open their own task.

    fn issue625_existing(
        subject: &str,
        status: &str,
        source_message_id: Option<i64>,
        ignored: bool,
    ) -> StoredTask {
        let mut meta = serde_json::Map::new();
        meta.insert("_monitor".into(), json!(true));
        meta.insert("monitor_connection".into(), json!("telegram-user"));
        meta.insert("monitor_connector".into(), json!("telegram-login"));
        meta.insert("chat_id".into(), json!(8_689_648_954i64));
        meta.insert("monitor_envelope_id".into(), json!("env-existing"));
        if let Some(id) = source_message_id {
            meta.insert("source_message_id".into(), json!(id));
        }
        if ignored {
            meta.insert("ignored".into(), json!(true));
        }
        serde_json::from_value(json!({
            "task_id": "monitor-existing",
            "subject": subject,
            "description": "",
            "active_form": "",
            "status": status,
            "owner": null,
            "blocks": [],
            "blocked_by": [],
            "metadata": Value::Object(meta),
            "output": null,
        }))
        .expect("construct existing StoredTask")
    }

    fn issue625_candidate(
        subject: &str,
        source_message_id: Option<i64>,
    ) -> (Map<String, Value>, String) {
        let mut meta = serde_json::Map::new();
        meta.insert("_monitor".into(), json!(true));
        meta.insert("monitor_connection".into(), json!("telegram-user"));
        meta.insert("monitor_connector".into(), json!("telegram-login"));
        meta.insert("chat_id".into(), json!(8_689_648_954i64));
        // Fresh per-delivery envelope id, distinct from the existing task's.
        meta.insert("monitor_envelope_id".into(), json!("env-redelivery"));
        if let Some(id) = source_message_id {
            meta.insert("source_message_id".into(), json!(id));
        }
        (meta, subject.to_string())
    }

    fn issue655_with_sender(
        base: (Map<String, Value>, String),
        sender_id: i64,
    ) -> (Map<String, Value>, String) {
        let (mut meta, subject) = base;
        meta.insert("sender_id".into(), json!(sender_id));
        (meta, subject)
    }

    fn issue655_existing_with_sender(subject: &str, sender_id: i64) -> StoredTask {
        let mut task = issue625_existing(subject, "pending", None, false);
        task.metadata.insert("sender_id".into(), json!(sender_id));
        task
    }

    #[test]
    fn issue_655_same_subject_different_senders_do_not_fold() {
        // Two different people asking the same thing in one group are two
        // distinct requests — the subject leg must not collapse them
        // (agentenv/monorepo#655).
        let original = issue655_existing_with_sender(
            "Telegram: road test registration needs immediate decision",
            42,
        );
        let (candidate, subject) = issue655_with_sender(
            issue625_candidate(
                "Telegram: road test registration needs immediate decision",
                None,
            ),
            43,
        );
        assert!(
            duplicate_monitor_task_skip(&[original], &candidate, &subject).is_none(),
            "different senders with the same subject must each keep a task"
        );
    }

    #[test]
    fn issue_655_same_subject_same_sender_still_folds() {
        let original = issue655_existing_with_sender(
            "Telegram: road test registration needs immediate decision",
            42,
        );
        let (candidate, subject) = issue655_with_sender(
            issue625_candidate(
                "Telegram: road test registration needs immediate decision",
                None,
            ),
            42,
        );
        let v = duplicate_monitor_task_skip(&[original], &candidate, &subject)
            .expect("same sender + same open subject still folds (#432)");
        assert_eq!(v["reason"], "duplicate_monitor_task");
    }

    #[test]
    fn issue_655_unknown_sender_keeps_historical_fold() {
        // If either side lacks a sender (DMs, older records), behavior is
        // unchanged: subject-only fold.
        let original = issue625_existing(
            "Telegram: road test registration needs immediate decision",
            "pending",
            None,
            false,
        );
        let (candidate, subject) = issue655_with_sender(
            issue625_candidate(
                "Telegram: road test registration needs immediate decision",
                None,
            ),
            43,
        );
        let v = duplicate_monitor_task_skip(&[original], &candidate, &subject)
            .expect("unknown sender on one side keeps the historical fold");
        assert_eq!(v["reason"], "duplicate_monitor_task");
    }

    #[test]
    fn issue_655_mixed_sender_burst_stamps_sender_ids() {
        // A burst whose stamps disagree on sender_id has no single sender to
        // stamp, but the distinct set is recorded so dedup can tell a known
        // multi-sender burst apart from a sender-less record.
        let stamps = vec![
            issue625_stamp("env-a", 8_689_648_954, 42, 6090),
            issue625_stamp("env-b", 8_689_648_954, 43, 6091),
        ];
        let mut metadata = serde_json::Map::new();
        stamp_shared_monitor_source_identity(&mut metadata, &stamps);
        assert_eq!(metadata.get("sender_id"), None);
        assert_eq!(metadata.get("sender_ids"), Some(&json!([42, 43])));
    }

    #[test]
    fn issue_655_multi_sender_burst_does_not_fold_into_single_sender_task() {
        // A consolidated burst carrying messages from Alice AND Bob must not
        // be absorbed by Alice's pre-existing same-subject task — that would
        // silently drop Bob's request (agentenv/monorepo#655).
        let original = issue655_existing_with_sender(
            "Telegram: road test registration needs immediate decision",
            42,
        );
        let (mut candidate, subject) = issue625_candidate(
            "Telegram: road test registration needs immediate decision",
            None,
        );
        candidate.insert("sender_ids".into(), json!([42, 43]));
        assert!(
            duplicate_monitor_task_skip(&[original], &candidate, &subject).is_none(),
            "a multi-sender burst must keep its own task"
        );
    }

    #[test]
    fn issue_655_single_sender_folds_into_burst_that_contains_them() {
        // The converse: one more message from a sender already represented by
        // an open multi-sender burst task is still a duplicate.
        let mut original = issue625_existing(
            "Telegram: road test registration needs immediate decision",
            "pending",
            None,
            false,
        );
        original
            .metadata
            .insert("sender_ids".into(), json!([42, 43]));
        let (candidate, subject) = issue655_with_sender(
            issue625_candidate(
                "Telegram: road test registration needs immediate decision",
                None,
            ),
            42,
        );
        let v = duplicate_monitor_task_skip(&[original], &candidate, &subject)
            .expect("a sender already in the burst still folds");
        assert_eq!(v["reason"], "duplicate_monitor_task");
    }

    #[test]
    fn issue_625_same_message_id_dedups_across_completion() {
        // The #625 fix: a reconnect/replay re-delivers the EXACT same Telegram message
        // (same chat_id+message_id) after the original task was completed. Message
        // identity must suppress it regardless of status or any subject drift.
        let original = issue625_existing(
            "Telegram: road test registration needs immediate decision",
            "completed",
            Some(6080),
            false,
        );
        let (candidate, subject) = issue625_candidate(
            "Telegram: a re-paraphrased subject for the same message",
            Some(6080),
        );
        let v = duplicate_monitor_task_skip(&[original], &candidate, &subject)
            .expect("same message id must dedup across completion");
        assert_eq!(v["reason"], "duplicate_source");
        assert_eq!(v["existingTaskId"].as_str(), Some("monitor-existing"));
    }

    #[test]
    fn issue_625_distinct_messages_same_text_create_separate_tasks() {
        // The aligned non-goal: a DIFFERENT message (different message_id) with the
        // same text, after the original task closed, is a distinct event and must NOT
        // be deduped — it opens its own task. (This is the monitor-3 -> monitor-4 case
        // observed in bobo, which is correct behavior under message-identity dedup.)
        let original = issue625_existing(
            "Telegram: road test registration needs immediate decision",
            "completed",
            Some(6080),
            false,
        );
        let (candidate, subject) = issue625_candidate(
            "Telegram: road test registration needs immediate decision",
            Some(6081),
        );
        assert!(
            duplicate_monitor_task_skip(&[original], &candidate, &subject).is_none(),
            "a distinct message with identical text must open its own task"
        );
    }

    #[test]
    fn issue_625_open_task_same_subject_still_dedups() {
        // Preserve #432: a still-OPEN task with the same subject in the same chat is
        // collapsed (avoids piling up near-identical open items while actionable).
        let original = issue625_existing(
            "Telegram: road test registration needs immediate decision",
            "pending",
            None,
            false,
        );
        let (candidate, subject) = issue625_candidate(
            "Telegram: road test registration needs immediate decision",
            Some(6081),
        );
        let v = duplicate_monitor_task_skip(&[original], &candidate, &subject)
            .expect("same subject against an open task should dedup (#432)");
        assert_eq!(v["reason"], "duplicate_monitor_task");
    }

    #[test]
    fn issue_625_different_subject_same_chat_is_not_a_duplicate() {
        // Same chat/sender is not enough (router.rs source-isolation policy): a new,
        // unrelated question must still create its own task.
        let original = issue625_existing(
            "Telegram: road test registration needs immediate decision",
            "pending",
            None,
            false,
        );
        let (candidate, subject) =
            issue625_candidate("Telegram: a brand new unrelated question about lunch", None);
        assert!(
            duplicate_monitor_task_skip(&[original], &candidate, &subject).is_none(),
            "a different subject in the same chat is not a duplicate"
        );
    }

    // ---- option (a): generic.review (consolidated multi-envelope) identity ----

    fn issue625_stamp(
        envelope_id: &str,
        chat_id: i64,
        sender_id: i64,
        message_id: i64,
    ) -> crate::MonitorSourceStampContext {
        crate::MonitorSourceStampContext {
            envelope_id: envelope_id.to_string(),
            connection_slug: "telegram-user".to_string(),
            connector_slug: Some("telegram-login".to_string()),
            received_at_ms: None,
            text: Some("burst message".to_string()),
            payload: json!({ "chat_id": chat_id, "sender_id": sender_id, "message_id": message_id }),
        }
    }

    #[test]
    fn issue_625_monitor_source_message_ids_reads_plural() {
        let mut meta = serde_json::Map::new();
        meta.insert("source_message_ids".into(), json!([6090, 6091]));
        let ids = monitor_source_message_ids(&meta);
        assert!(ids.contains(&6090) && ids.contains(&6091));
    }

    #[test]
    fn issue_625_same_chat_batch_stamps_shared_identity() {
        // A same-conversation burst -> uniform chat_id/sender_id stamped to top level,
        // plus the per-message ids as a plural array.
        let stamps = vec![
            issue625_stamp("env-a", 8_689_648_954, 8_689_648_954, 6090),
            issue625_stamp("env-b", 8_689_648_954, 8_689_648_954, 6091),
        ];
        let mut meta = serde_json::Map::new();
        stamp_shared_monitor_source_identity(&mut meta, &stamps);
        assert_eq!(metadata_i64(&meta, &["chat_id"]), Some(8_689_648_954));
        assert_eq!(metadata_i64(&meta, &["sender_id"]), Some(8_689_648_954));
        let ids = monitor_source_message_ids(&meta);
        assert!(ids.contains(&6090) && ids.contains(&6091));
    }

    #[test]
    fn issue_625_cross_chat_batch_does_not_stamp_chat_id() {
        // A genuinely cross-chat review: chat_id is not uniform, so it must NOT be
        // stamped (the task is not scoped to a single chat), but message ids are kept.
        let stamps = vec![
            issue625_stamp("env-a", 111, 111, 6090),
            issue625_stamp("env-b", 222, 222, 6091),
        ];
        let mut meta = serde_json::Map::new();
        stamp_shared_monitor_source_identity(&mut meta, &stamps);
        assert_eq!(metadata_i64(&meta, &["chat_id"]), None);
        assert_eq!(metadata_i64(&meta, &["sender_id"]), None);
        assert!(monitor_source_message_ids(&meta).contains(&6090));
    }

    #[test]
    fn issue_761_generic_review_contract_carries_uniform_chat_identity() {
        // A same-chat burst must keep its chat identity inside the contract
        // source so the source_context can render a delivery target
        // (agentenv/monorepo#761). It is stamped verbatim as an i64, exactly
        // like the single-source telegram contract; the renderer's
        // string_field coerces numeric ids.
        let stamps = vec![
            issue625_stamp("env-a", 8_689_648_954, 42, 6090),
            issue625_stamp("env-b", 8_689_648_954, 42, 6091),
        ];
        let contract = generic_review_contract_from_stamps(&stamps);
        assert_eq!(
            contract.source.get("chat_id"),
            Some(&json!(8_689_648_954i64)),
            "chat_id is stamped verbatim as an i64"
        );
        assert_eq!(
            contract.source.get("source_message_ids"),
            Some(&json!([6090, 6091]))
        );
    }

    #[test]
    fn issue_761_cross_chat_contract_has_no_chat_identity() {
        // Non-uniform chat_id -> no single chat to deliver to -> no identity
        // stamped; the task stays review-only.
        let stamps = vec![
            issue625_stamp("env-a", 111, 42, 6090),
            issue625_stamp("env-b", 222, 42, 6091),
        ];
        let contract = generic_review_contract_from_stamps(&stamps);
        assert_eq!(contract.source.get("chat_id"), None);
        assert_eq!(
            contract.source.get("source_message_ids"),
            Some(&json!([6090, 6091])),
            "per-message ids are kept for dedup/audit even cross-chat"
        );
    }

    #[test]
    fn issue_625_generic_review_task_dedups_by_message_id() {
        // The goal of option (a): a consolidated generic.review task now carries
        // chat_id + source_message_ids, so a re-delivery of one of its messages dedups
        // (which previously slipped through because the identity was stripped).
        let mut meta = serde_json::Map::new();
        meta.insert("_monitor".into(), json!(true));
        meta.insert("monitor_connection".into(), json!("telegram-user"));
        meta.insert("monitor_connector".into(), json!("telegram-login"));
        meta.insert("chat_id".into(), json!(8_689_648_954i64));
        meta.insert("source_message_ids".into(), json!([6090, 6091]));
        let existing: StoredTask = serde_json::from_value(json!({
            "task_id": "monitor-existing",
            "subject": "Telegram: consolidated burst",
            "description": "",
            "active_form": "",
            "status": "completed",
            "owner": null,
            "blocks": [],
            "blocked_by": [],
            "metadata": Value::Object(meta),
            "output": null,
        }))
        .expect("construct generic.review StoredTask");
        // A re-delivery of message 6090 (same chat) as a fresh single-source candidate.
        let (candidate, subject) =
            issue625_candidate("Telegram: a re-paraphrased subject", Some(6090));
        let v = duplicate_monitor_task_skip(&[existing], &candidate, &subject)
            .expect("re-delivery of a message already in a generic.review task should dedup");
        assert_eq!(v["reason"], "duplicate_source");
    }

    #[test]
    fn task_list_exposes_compact_monitor_source_refs() {
        let (mut state, tmp) = make_state();
        configure_telegram_gate(
            &mut state,
            &tmp,
            "env-6836",
            activity_state(Vec::new(), None),
        );
        let raw = execute_task_create(
            &mut state,
            tmp.path(),
            telegram_monitor_task_input("env-6836"),
        )
        .unwrap();
        let task_id = serde_json::from_str::<Value>(&raw).unwrap()["task"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let store_path = monitor_tasks_path(tmp.path());
        let mut store = load_store::<TaskStore>(&store_path).unwrap();
        store.tasks[0].metadata.insert(
            "source_text".to_string(),
            Value::String("线上支付回调失败率刚升到 18%，请在 16:00 前给结论。".to_string()),
        );
        save_store(&store_path, &store).unwrap();

        let raw = execute_task_list(&mut state, tmp.path(), json!({})).unwrap();
        let payload: Value = serde_json::from_str(&raw).unwrap();
        let task = payload["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"].as_str() == Some(task_id.as_str()))
            .unwrap();

        assert_eq!(
            task.pointer("/monitorSource/connectionSlug")
                .and_then(Value::as_str),
            Some("telegram-user")
        );
        assert_eq!(
            task.pointer("/monitorSource/connectorSlug")
                .and_then(Value::as_str),
            Some("telegram-login")
        );
        assert_eq!(
            task.pointer("/monitorSource/chatId")
                .and_then(Value::as_i64),
            Some(42)
        );
        assert_eq!(
            task.pointer("/monitorSource/envelopeId")
                .and_then(Value::as_str),
            Some("env-6836")
        );
        assert_eq!(
            task.pointer("/monitorSource/sourceMessageId")
                .and_then(Value::as_i64),
            Some(6836)
        );
        assert_eq!(
            task.pointer("/monitorSource/sourceTextSnippet")
                .and_then(Value::as_str),
            Some("线上支付回调失败率刚升到 18%，请在 16:00 前给结论。")
        );
        assert!(task.pointer("/monitorSource/conversationContext").is_none());
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
    fn task_update_rejects_generic_completion_for_completion_policy_monitor_task() {
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
        .expect_err("completion-policy monitor tasks need approval before completion");

        assert!(error
            .to_string()
            .contains("must be completed through its monitor action"));
    }

    #[test]
    fn task_update_allows_completion_for_telegram_delivery_target_without_policy() {
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_non_reply_monitor_task(&mut state, tmp.path());

        execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "status": "completed"
            }),
        )
        .expect("delivery target alone is no longer a completion gate");

        let task = load_monitor_task(tmp.path(), &task_id).unwrap().unwrap();
        assert_eq!(task.status, "completed");
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

    /// Creates a plain monitor task without a completion policy that requires
    /// human approval, so `TaskUpdate` is allowed to complete it directly.
    fn create_plain_monitor_task(state: &mut AppState, cwd: &Path) -> String {
        let raw = execute_task_create(
            state,
            cwd,
            json!({
                "subject": "Log check for anomalies",
                "description": "Scan logs for error spikes.",
                "receivedAt": "2026-06-10T13:00:00Z",
                "expiresAt": "2026-06-11T13:00:00Z",
                "metadata": {
                    "_monitor": true,
                    "chat_id": "42"
                }
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

    #[test]
    fn task_update_stamps_completed_via_on_monitor_completion() {
        // GIVEN a non-human-gated monitor task in the monitor store
        let (mut state, tmp) = make_state();
        let task_id = create_plain_monitor_task(&mut state, tmp.path());

        // Confirm the task is NOT gated by completion policy before we proceed
        let task = load_monitor_task(tmp.path(), &task_id).unwrap().unwrap();
        assert!(
            !monitor_task_requires_human_approval(&task),
            "test setup: task should not be gated by completion policy"
        );

        // WHEN execute_task_update completes it with completed_via in metadata
        execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "status": "completed",
                "metadata": { "completed_via": "agent_report:outgoing" }
            }),
        )
        .unwrap();

        // THEN the persisted monitor task records completed_via at top level
        let path = monitor_tasks_path(tmp.path());
        let store_raw = std::fs::read_to_string(&path).unwrap();
        let store: Value = serde_json::from_str(&store_raw).unwrap();
        let task_json = store["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["task_id"].as_str() == Some(&task_id))
            .expect("task must be in monitor store");

        assert_eq!(task_json["status"], "completed");
        assert_eq!(task_json["completed_via"], "agent_report:outgoing");
    }

    #[test]
    fn task_update_stamps_completed_via_default_when_not_in_metadata() {
        // GIVEN a non-human-gated monitor task
        let (mut state, tmp) = make_state();
        let task_id = create_plain_monitor_task(&mut state, tmp.path());

        // WHEN execute_task_update completes it WITHOUT completed_via in metadata
        execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "status": "completed"
            }),
        )
        .unwrap();

        // THEN the persisted task records the default "agent_report" value
        let path = monitor_tasks_path(tmp.path());
        let store_raw = std::fs::read_to_string(&path).unwrap();
        let store: Value = serde_json::from_str(&store_raw).unwrap();
        let task_json = store["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["task_id"].as_str() == Some(&task_id))
            .expect("task must be in monitor store");

        assert_eq!(task_json["status"], "completed");
        assert_eq!(task_json["completed_via"], "agent_report");
    }

    #[test]
    fn triage_turn_completes_completion_policy_monitor_task() {
        // GIVEN a monitor task whose completion policy requires approval.
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());

        // Confirm it IS gated by completion policy.
        let task = load_monitor_task(tmp.path(), &task_id).unwrap().unwrap();
        assert!(
            monitor_task_requires_human_approval(&task),
            "test setup: task should be gated by completion policy"
        );

        // AND a state that is running inside a monitor triage turn
        state.monitor_triage_turn = true;

        // WHEN execute_task_update completes it
        execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "status": "completed"
            }),
        )
        .expect("triage turn must be allowed to complete a policy-gated monitor task");

        // THEN it persists status=completed with completed_via stamped
        let path = monitor_tasks_path(tmp.path());
        let store_raw = std::fs::read_to_string(&path).unwrap();
        let store: Value = serde_json::from_str(&store_raw).unwrap();
        let task_json = store["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["task_id"].as_str() == Some(&task_id))
            .expect("task must be in monitor store");

        assert_eq!(task_json["status"], "completed");
        assert_eq!(task_json["completed_via"], "agent_report");
    }

    #[test]
    fn task_update_still_refuses_completion_policy_monitor_completion() {
        // GIVEN a monitor task whose completion policy requires approval.
        let (mut state, tmp) = make_state();
        let task_id = create_telegram_monitor_task(&mut state, tmp.path());

        // AND a NON-triage caller (default monitor_triage_turn = false)
        assert!(
            !state.monitor_triage_turn,
            "test setup: a non-triage caller must keep monitor_triage_turn = false"
        );

        // Confirm it IS gated by completion policy.
        let task = load_monitor_task(tmp.path(), &task_id).unwrap().unwrap();
        assert!(
            monitor_task_requires_human_approval(&task),
            "test setup: task should be gated by completion policy"
        );

        // WHEN execute_task_update tries status: completed
        let error = execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "status": "completed"
            }),
        )
        .expect_err("policy-gated monitor tasks must not be completable by agent");

        // THEN it errors with the expected message
        assert!(
            error
                .to_string()
                .contains("must be completed through its monitor action"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn task_update_stamps_completed_via_incoming_label_on_monitor_completion() {
        // Test B (integration coverage Task 7): prove the incoming-label variant
        // of completed_via is persisted correctly.  The existing sibling test
        // `task_update_stamps_completed_via_on_monitor_completion` covers the
        // outgoing label ("agent_report:outgoing"); this test exercises the
        // symmetrical incoming path so both labels are regression-protected.

        // GIVEN a non-human-gated monitor task
        let (mut state, tmp) = make_state();
        let task_id = create_plain_monitor_task(&mut state, tmp.path());

        // WHEN the agent completes it with the incoming direction label
        execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "status": "completed",
                "metadata": { "completed_via": "agent_report:incoming" }
            }),
        )
        .unwrap();

        // THEN the persisted monitor task records completed_via = "agent_report:incoming"
        let path = monitor_tasks_path(tmp.path());
        let store_raw = std::fs::read_to_string(&path).unwrap();
        let store: serde_json::Value = serde_json::from_str(&store_raw).unwrap();
        let task_json = store["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["task_id"].as_str() == Some(&task_id))
            .expect("task must be in monitor store");

        assert_eq!(task_json["status"], "completed");
        assert_eq!(
            task_json["completed_via"], "agent_report:incoming",
            "incoming-direction completion must record the incoming label"
        );
    }

    #[test]
    fn task_update_does_not_restamp_completed_via_on_metadata_only_update() {
        // GIVEN a monitor task that was already completed with completed_via = "reply"
        // (set directly in the store, as the daemon's reply-completion path would do)
        let (mut state, tmp) = make_state();
        let task_id = create_plain_monitor_task(&mut state, tmp.path());

        // Directly stamp completed status + completed_via in the store (bypass TaskUpdate)
        let path = monitor_tasks_path(tmp.path());
        {
            let mut store = load_store::<TaskStore>(&path).unwrap();
            let task = store
                .tasks
                .iter_mut()
                .find(|t| t.task_id == task_id)
                .unwrap();
            task.status = "completed".to_string();
            task.completed_via = Some("reply".to_string());
            save_store(&path, &store).unwrap();
        }

        // WHEN execute_task_update is called with only a metadata content change
        // (no status field — task is already completed)
        execute_task_update(
            &mut state,
            tmp.path(),
            json!({
                "taskId": task_id,
                "metadata": { "some_label": "extra-context" }
            }),
        )
        .unwrap();

        // THEN completed_via is NOT clobbered — it remains "reply"
        let store_raw = std::fs::read_to_string(&path).unwrap();
        let store: Value = serde_json::from_str(&store_raw).unwrap();
        let task_json = store["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["task_id"].as_str() == Some(&task_id))
            .expect("task must be in monitor store");

        assert_eq!(task_json["status"], "completed");
        assert_eq!(
            task_json["completed_via"], "reply",
            "completed_via must not be clobbered by a metadata-only update on an already-completed task"
        );
    }
}
