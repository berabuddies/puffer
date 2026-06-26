use anyhow::{anyhow, bail, Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::installed_connector_action_executor;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use time::OffsetDateTime;

use super::handle_workflow_list;
use super::monitor_task_ignore::{monitor_tasks_path, task_id_matches};
use crate::subscriptions::send_authorization_for_send_message_input_with_source;

#[derive(Debug, Deserialize)]
struct MonitorActionExecuteParams {
    #[serde(alias = "taskId")]
    task_id: String,
    #[serde(alias = "actionId")]
    action_id: String,
    version: u64,
    #[serde(default, alias = "approvedMessage")]
    approved_message: Option<String>,
    #[serde(default, alias = "approvedResponse")]
    approved_response: Option<String>,
    #[serde(alias = "clientRequestId")]
    client_request_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MonitorConnectorAction {
    pub connector_slug: String,
    pub connection_slug: String,
    pub action: String,
    pub input: Value,
    pub idempotency_key: String,
    pub version: u64,
}

pub(crate) trait MonitorActionExecutor: Send + Sync {
    fn execute_monitor_action(&self, action: &MonitorConnectorAction) -> Result<Value>;
}

struct DispatcherMonitorActionExecutor;

impl MonitorActionExecutor for DispatcherMonitorActionExecutor {
    fn execute_monitor_action(&self, action: &MonitorConnectorAction) -> Result<Value> {
        let executor = installed_connector_action_executor()
            .context("connector action executor is not installed")?;
        let summary = executor.run_connector_action(
            &action.connector_slug,
            &action.action,
            action.input.clone(),
            action_trigger_payload(action)?,
        )?;
        Ok(json!({
            "success": true,
            "summary": summary,
            "connector_slug": action.connector_slug,
            "connection_slug": action.connection_slug,
            "action": action.action,
        }))
    }
}

static ACTION_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

pub(crate) fn handle_monitor_action_execute(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    handle_monitor_action_execute_with_executor(paths, params, &DispatcherMonitorActionExecutor)
}

pub(crate) fn handle_monitor_action_execute_with_executor(
    paths: &ConfigPaths,
    params: &Value,
    executor: &dyn MonitorActionExecutor,
) -> Result<Value> {
    let params: MonitorActionExecuteParams =
        serde_json::from_value(params.clone()).context("invalid monitor action params")?;
    let task_id = non_empty(params.task_id.as_str())
        .context("missing task_id")?
        .to_string();
    let action_id = non_empty(params.action_id.as_str())
        .context("missing action_id")?
        .to_string();
    let client_request_id = non_empty(params.client_request_id.as_str())
        .context("missing client_request_id")?
        .to_string();

    let path = monitor_tasks_path(paths);
    let action_lock = monitor_action_lock(&path, &task_id);
    let _guard = action_lock.lock().unwrap();
    let mut store = read_monitor_store(&path)?;
    let task = find_task_mut(&mut store, &task_id)?;
    let terminal_status = task
        .get("status")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let completed_via = task
        .get("completed_via")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let metadata = task
        .get_mut("metadata")
        .and_then(Value::as_object_mut)
        .context("monitor task missing metadata")?;
    let pending_snapshot = metadata
        .get("pending_action")
        .and_then(Value::as_object)
        .context("monitor task has no pending action")?;
    validate_pending_action_identity(pending_snapshot, &action_id, params.version)?;
    validate_pending_action_provenance(metadata, pending_snapshot)?;
    let connector_action =
        connector_action_for_pending(metadata, pending_snapshot, &params, &task_id, &action_id)
            .with_context(|| {
                format!("monitor task `{task_id}` pending action cannot be executed")
            })?;
    let pending_status = pending_snapshot
        .get("status")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    if matches!(terminal_status.as_deref(), Some("completed"))
        && completed_action_matches(
            completed_via.as_deref(),
            pending_status.as_deref(),
            &connector_action.action,
        )
    {
        return snapshot_with_action_status(
            paths,
            already_status_for_action(&connector_action.action),
        );
    }
    if matches!(
        terminal_status.as_deref(),
        Some("completed") | Some("cancelled")
    ) {
        bail!("terminal_monitor_task");
    }
    let pending = metadata
        .get_mut("pending_action")
        .and_then(Value::as_object_mut)
        .context("monitor task has no pending action")?;
    match pending.get("status").and_then(Value::as_str) {
        Some(status) if status == success_status_for_action(&connector_action.action) => {
            return snapshot_with_action_status(
                paths,
                already_status_for_action(&connector_action.action),
            );
        }
        Some(status) if status == pending_status_for_action(&connector_action.action) => {
            pending.insert(
                "status".to_string(),
                Value::String(uncertain_status_for_action(&connector_action.action).to_string()),
            );
            pending.insert(
                "error".to_string(),
                Value::String(
                    "previous action attempt was interrupted before receipt confirmation"
                        .to_string(),
                ),
            );
            append_audit(
                metadata,
                "typed_action_uncertain",
                json!({
                    "task_id": task_id,
                    "action_id": action_id,
                    "version": params.version,
                    "client_request_id": client_request_id,
                    "connector_action": connector_action.action,
                    "reason": "stale_in_flight_state",
                }),
            );
            set_task_updated(task, now_ms());
            fs::write(&path, serde_json::to_string_pretty(&store)?)
                .with_context(|| format!("failed to write {}", path.display()))?;
            bail!("duplicate_risk_ack_required");
        }
        Some(status) if status == uncertain_status_for_action(&connector_action.action) => {
            bail!("duplicate_risk_ack_required");
        }
        Some(status) if executable_pending_status(&connector_action.action, status) => {}
        Some(other) => bail!("action state `{other}` cannot be executed"),
        None => bail!("pending action missing status"),
    }
    let starting_status = pending_status_for_action(&connector_action.action);
    pending.insert(
        "status".to_string(),
        Value::String(starting_status.to_string()),
    );
    pending.insert(
        "client_request_id".to_string(),
        Value::String(client_request_id.clone()),
    );
    pending.insert("error".to_string(), Value::Null);
    if let Some(message) = params
        .approved_message
        .as_deref()
        .and_then(non_empty)
        .map(ToString::to_string)
    {
        pending.insert("approved_message".to_string(), Value::String(message));
    }
    if let Some(response) = params
        .approved_response
        .as_deref()
        .and_then(non_empty)
        .map(ToString::to_string)
    {
        pending.insert("approved_response".to_string(), Value::String(response));
    }
    pending.insert(
        "approved_by".to_string(),
        Value::String("human".to_string()),
    );
    pending.insert("approved_at".to_string(), Value::from(now_ms()));
    append_audit(
        metadata,
        "typed_action_started",
        json!({
            "task_id": task_id,
            "action_id": action_id,
            "version": params.version,
            "client_request_id": client_request_id,
            "connector_action": connector_action.action,
        }),
    );
    set_task_updated(task, now_ms());
    fs::write(&path, serde_json::to_string_pretty(&store)?)
        .with_context(|| format!("failed to write {}", path.display()))?;

    let result = execute_monitor_connector_action(executor, &connector_action);
    let mut store = read_monitor_store(&path)?;
    let task = find_task_mut(&mut store, &task_id)?;
    let metadata = task
        .get_mut("metadata")
        .and_then(Value::as_object_mut)
        .context("monitor task missing metadata after action")?;
    let pending = metadata
        .get_mut("pending_action")
        .and_then(Value::as_object_mut)
        .context("monitor task missing pending action after action")?;
    match result {
        Ok(receipt) => {
            pending.insert(
                "status".to_string(),
                Value::String(success_status_for_action(&connector_action.action).to_string()),
            );
            pending.insert("receipt".to_string(), receipt.clone());
            append_audit(
                metadata,
                "typed_action_succeeded",
                json!({
                    "task_id": task_id,
                    "action_id": action_id,
                    "version": params.version,
                    "receipt": receipt,
                }),
            );
            task.as_object_mut()
                .context("monitor task entry must be an object")?
                .insert("status".to_string(), Value::String("completed".to_string()));
            task.as_object_mut()
                .context("monitor task entry must be an object")?
                .insert(
                    "completed_via".to_string(),
                    Value::String(completed_via_for_action(&connector_action.action).to_string()),
                );
            set_task_updated(task, now_ms());
            fs::write(&path, serde_json::to_string_pretty(&store)?)
                .with_context(|| format!("failed to write {}", path.display()))?;
            snapshot_with_action_status(paths, "completed")
        }
        Err(error) => {
            pending.insert(
                "status".to_string(),
                Value::String(failure_status_for_action(&connector_action.action).to_string()),
            );
            pending.insert("error".to_string(), Value::String(format!("{error:#}")));
            append_audit(
                metadata,
                "typed_action_failed",
                json!({
                    "task_id": task_id,
                    "action_id": action_id,
                    "version": params.version,
                    "error": format!("{error:#}"),
                }),
            );
            set_task_updated(task, now_ms());
            fs::write(&path, serde_json::to_string_pretty(&store)?)
                .with_context(|| format!("failed to write {}", path.display()))?;
            Err(anyhow!("monitor_action_failed: {error:#}"))
        }
    }
}

fn connector_action_for_pending(
    metadata: &Map<String, Value>,
    pending: &Map<String, Value>,
    params: &MonitorActionExecuteParams,
    task_id: &str,
    action_id: &str,
) -> Result<MonitorConnectorAction> {
    let monitor = metadata
        .get("monitor")
        .and_then(Value::as_object)
        .context("monitor task missing typed monitor contract")?;
    let kind = monitor
        .get("kind")
        .and_then(Value::as_str)
        .context("monitor.kind missing")?;
    let source = monitor
        .get("source")
        .and_then(Value::as_object)
        .context("monitor.source missing")?;
    match kind {
        "telegram.reply" => {
            let message = params
                .approved_message
                .as_deref()
                .and_then(non_empty)
                .map(ToString::to_string)
                .or_else(|| {
                    string_field(pending, &["agent_draft_text", "agentDraftText"])
                        .map(str::to_string)
                })
                .context("Telegram pending action requires approved_message or agent draft text")?;
            let chat_id = scalar_field_string(source, &["chat_id", "chatId"])
                .context("Telegram pending action requires chat_id")?;
            let mut input = json!({
                "connection_slug": string_field(source, &["connection_slug", "connectionSlug"]),
                "connector_slug": string_field(source, &["connector_slug", "connectorSlug"]),
                "chat_id": chat_id,
                "message": message,
            });
            if let Some(reply_to) = scalar_field_value(source, &["message_id", "messageId"]) {
                input["reply_to"] = reply_to;
            }
            Ok(MonitorConnectorAction {
                connector_slug: string_field(source, &["connector_slug", "connectorSlug"])
                    .unwrap_or("telegram-login")
                    .to_string(),
                connection_slug: string_field(source, &["connection_slug", "connectionSlug"])
                    .unwrap_or("telegram-user")
                    .to_string(),
                action: "send_message".to_string(),
                input,
                idempotency_key: action_idempotency_key(task_id, action_id, params),
                version: params.version,
            })
        }
        "gmail.reply" => {
            let message = params
                .approved_message
                .as_deref()
                .and_then(non_empty)
                .map(ToString::to_string)
                .or_else(|| {
                    string_field(pending, &["agent_draft_text", "agentDraftText"])
                        .map(str::to_string)
                })
                .context("Gmail pending action requires approved_message or agent draft text")?;
            let thread_id = string_field(
                source,
                &["thread_id", "threadId", "message_id", "messageId"],
            )
            .context("Gmail pending action requires thread_id or message_id")?;
            Ok(MonitorConnectorAction {
                connector_slug: string_field(source, &["connector_slug", "connectorSlug"])
                    .unwrap_or("gmail-browser")
                    .to_string(),
                connection_slug: string_field(source, &["connection_slug", "connectionSlug"])
                    .unwrap_or("gmail-browser")
                    .to_string(),
                action: "draft_reply".to_string(),
                input: json!({
                    "connection_slug": string_field(source, &["connection_slug", "connectionSlug"]),
                    "connector_slug": string_field(source, &["connector_slug", "connectorSlug"]),
                    "account": string_field(source, &["account", "account_id", "accountId"]),
                    "thread_id": thread_id,
                    "url": string_field(source, &["url"]),
                    "body": message,
                }),
                idempotency_key: action_idempotency_key(task_id, action_id, params),
                version: params.version,
            })
        }
        "calendar.rsvp" => {
            let response = params
                .approved_response
                .as_deref()
                .and_then(non_empty)
                .context("Calendar pending action requires approved_response")?;
            if !matches!(response, "accept" | "deny") {
                bail!("unsupported Calendar RSVP response `{response}`");
            }
            Ok(MonitorConnectorAction {
                connector_slug: string_field(source, &["connector_slug", "connectorSlug"])
                    .unwrap_or("gcal-browser")
                    .to_string(),
                connection_slug: string_field(source, &["connection_slug", "connectionSlug"])
                    .unwrap_or("gcal-browser")
                    .to_string(),
                action: response.to_string(),
                input: json!({
                    "connection_slug": string_field(source, &["connection_slug", "connectionSlug"]),
                    "connector_slug": string_field(source, &["connector_slug", "connectorSlug"]),
                    "account": string_field(source, &["account", "account_id", "accountId"]),
                    "event_id": string_field(source, &["event_id", "eventId"]),
                    "calendar_id": string_field(source, &["calendar_id", "calendarId"]),
                    "url": string_field(source, &["html_link", "htmlLink", "url"]),
                    "title": string_field(source, &["summary", "title"]),
                }),
                idempotency_key: action_idempotency_key(task_id, action_id, params),
                version: params.version,
            })
        }
        other => bail!("typed monitor action execution does not support `{other}`"),
    }
}

fn validate_pending_action_identity(
    pending: &Map<String, Value>,
    action_id: &str,
    version: u64,
) -> Result<()> {
    if pending.get("id").and_then(Value::as_str) != Some(action_id) {
        bail!("action_id mismatch");
    }
    if pending.get("version").and_then(Value::as_u64) != Some(version) {
        bail!("action version mismatch");
    }
    Ok(())
}

fn validate_pending_action_provenance(
    metadata: &Map<String, Value>,
    pending: &Map<String, Value>,
) -> Result<()> {
    let monitor_hash = metadata
        .get("monitor")
        .and_then(Value::as_object)
        .and_then(|monitor| monitor.get("source_hash"))
        .and_then(Value::as_str)
        .context("monitor.source_hash missing")?;
    let pending_hash = pending
        .get("monitor_hash")
        .or_else(|| pending.get("monitorHash"))
        .and_then(Value::as_str)
        .context("pending action missing monitor_hash")?;
    if monitor_hash != pending_hash {
        bail!("pending action monitor hash mismatch");
    }
    Ok(())
}

fn action_idempotency_key(
    task_id: &str,
    action_id: &str,
    params: &MonitorActionExecuteParams,
) -> String {
    format!(
        "monitor-action-execute:{task_id}:{action_id}:{}",
        params.client_request_id
    )
}

fn completed_action_matches(
    completed_via: Option<&str>,
    pending_status: Option<&str>,
    action: &str,
) -> bool {
    let completed_via_matches = completed_via == Some(completed_via_for_action(action));
    let pending_status_matches = pending_status == Some(success_status_for_action(action));
    completed_via_matches || pending_status_matches
}

fn executable_pending_status(action: &str, status: &str) -> bool {
    match action {
        "send_message" => matches!(status, "draft_ready" | "send_failed"),
        "draft_reply" => matches!(status, "draft_ready" | "draft_failed"),
        "accept" | "deny" => matches!(status, "awaiting_confirmation" | "failed"),
        _ => matches!(status, "draft_ready" | "failed"),
    }
}

fn pending_status_for_action(action: &str) -> &'static str {
    match action {
        "draft_reply" => "creating_draft",
        "send_message" => "sending",
        "accept" | "deny" => "executing",
        _ => "executing",
    }
}

fn success_status_for_action(action: &str) -> &'static str {
    match action {
        "draft_reply" => "draft_created",
        "send_message" => "sent",
        "accept" | "deny" => "completed",
        _ => "completed",
    }
}

fn uncertain_status_for_action(action: &str) -> &'static str {
    match action {
        "draft_reply" => "draft_uncertain",
        "send_message" => "send_uncertain",
        "accept" | "deny" => "action_uncertain",
        _ => "action_uncertain",
    }
}

fn already_status_for_action(action: &str) -> &'static str {
    match action {
        "draft_reply" => "already_created",
        "send_message" => "already_sent",
        "accept" | "deny" => "already_completed",
        _ => "already_completed",
    }
}

fn failure_status_for_action(action: &str) -> &'static str {
    match action {
        "draft_reply" => "draft_failed",
        "send_message" => "send_failed",
        "accept" | "deny" => "failed",
        _ => "failed",
    }
}

fn completed_via_for_action(action: &str) -> &'static str {
    match action {
        "draft_reply" => "human_approved_gmail_draft",
        "send_message" => "human_approved_telegram_reply",
        "accept" | "deny" => "human_approved_calendar_rsvp",
        _ => "human_approved_monitor_action",
    }
}

fn execute_monitor_connector_action(
    executor: &dyn MonitorActionExecutor,
    action: &MonitorConnectorAction,
) -> Result<Value> {
    match executor.execute_monitor_action(action) {
        Err(error) if action.action == "send_message" && telegram_reply_target_error(&error) => {
            let Some(input) = action.input.as_object() else {
                return Err(error);
            };
            if !input.contains_key("reply_to") && !input.contains_key("reply_to_message_id") {
                return Err(error);
            }
            let mut plain = action.clone();
            if let Some(input) = plain.input.as_object_mut() {
                input.remove("reply_to");
                input.remove("reply_to_message_id");
            }
            executor.execute_monitor_action(&plain)
        }
        other => other,
    }
}

fn telegram_reply_target_error(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_ascii_uppercase();
    [
        "MSG_ID_INVALID",
        "MESSAGE_ID_INVALID",
        "REPLY_MESSAGE_ID_INVALID",
        "REPLY_TO_MSG_ID_INVALID",
        "REPLY_TO_MESSAGE_ID_INVALID",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn monitor_action_lock(path: &Path, task_id: &str) -> Arc<Mutex<()>> {
    let key = format!("{}::{task_id}", path.display());
    let locks = ACTION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().unwrap();
    locks
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn read_monitor_store(path: &Path) -> Result<Value> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("invalid monitor task store {}", path.display()))
}

fn find_task_mut<'a>(store: &'a mut Value, task_id: &str) -> Result<&'a mut Value> {
    store
        .get_mut("tasks")
        .and_then(Value::as_array_mut)
        .context("monitor task store missing tasks array")?
        .iter_mut()
        .find(|task| task_id_matches(task, task_id))
        .ok_or_else(|| anyhow!("monitor task `{task_id}` not found"))
}

fn append_audit(metadata: &mut Map<String, Value>, event: &str, details: Value) {
    let entry = json!({
        "event": event,
        "at_ms": now_ms(),
        "details": details,
    });
    match metadata.get_mut("monitor_action_events") {
        Some(Value::Array(events)) => events.push(entry),
        _ => {
            metadata.insert(
                "monitor_action_events".to_string(),
                Value::Array(vec![entry]),
            );
        }
    }
}

fn set_task_updated(task: &mut Value, now: u64) {
    if let Some(object) = task.as_object_mut() {
        object.insert("updated_at_ms".to_string(), Value::from(now));
    }
}

fn snapshot_with_action_status(paths: &ConfigPaths, status: &str) -> Result<Value> {
    let mut snapshot = handle_workflow_list(paths)?;
    if let Some(object) = snapshot.as_object_mut() {
        object.insert(
            "monitorActionExecute".to_string(),
            json!({
                "status": status,
            }),
        );
    }
    Ok(snapshot)
}

fn action_trigger_payload(action: &MonitorConnectorAction) -> Result<Value> {
    let mut trigger = json!({
        "type": "monitor_action_execute",
        "envelope_id": action.idempotency_key,
        "connection_id": action.connection_slug,
        "receivedAt": OffsetDateTime::now_utc().to_string(),
        "topic": action.connection_slug,
        "kind": "connector_action",
        "dedup_key": action.idempotency_key,
        "text": "",
        "payload": action.input,
    });
    if action.action == "send_message" {
        let input = action
            .input
            .as_object()
            .context("send_message monitor action input must be an object")?;
        let message = string_field(input, &["message", "text"])
            .context("send_message monitor action requires message")?;
        trigger["send_authorization"] =
            json!(send_authorization_for_send_message_input_with_source(
                "monitor-action",
                &action.idempotency_key,
                action.version,
                "send_message",
                &action.input,
                message,
                &action.idempotency_key,
            )?);
    }
    Ok(trigger)
}

fn now_ms() -> u64 {
    OffsetDateTime::now_utc().unix_timestamp_nanos() as u64 / 1_000_000
}

fn string_field<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn scalar_field_string(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::String(value) => non_empty(value).map(ToString::to_string),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
}

fn scalar_field_value(object: &Map<String, Value>, keys: &[&str]) -> Option<Value> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::String(value) => non_empty(value).map(|value| Value::String(value.to_string())),
            Value::Number(_) => Some(value.clone()),
            _ => None,
        })
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::ConfigPaths;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingExecutor {
        actions: Arc<Mutex<Vec<MonitorConnectorAction>>>,
    }

    impl MonitorActionExecutor for RecordingExecutor {
        fn execute_monitor_action(&self, action: &MonitorConnectorAction) -> Result<Value> {
            self.actions.lock().unwrap().push(action.clone());
            Ok(json!({"ok": true, "action": action.action}))
        }
    }

    struct SequenceExecutor {
        actions: Arc<Mutex<Vec<MonitorConnectorAction>>>,
        results: Mutex<VecDeque<Result<Value, String>>>,
    }

    impl SequenceExecutor {
        fn new(results: Vec<Result<Value, String>>) -> Self {
            Self {
                actions: Arc::new(Mutex::new(Vec::new())),
                results: Mutex::new(VecDeque::from(results)),
            }
        }
    }

    impl MonitorActionExecutor for SequenceExecutor {
        fn execute_monitor_action(&self, action: &MonitorConnectorAction) -> Result<Value> {
            self.actions.lock().unwrap().push(action.clone());
            match self.results.lock().unwrap().pop_front() {
                Some(Ok(value)) => Ok(value),
                Some(Err(error)) => bail!("{error}"),
                None => Ok(json!({"ok": true, "action": action.action})),
            }
        }
    }

    fn write_store(paths: &ConfigPaths, store: Value) {
        let path = monitor_tasks_path(paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&store).unwrap()).unwrap();
    }

    fn telegram_action_store(task_status: &str, pending_status: &str) -> Value {
        telegram_action_store_with_hashes(task_status, pending_status, "sha256:test", "sha256:test")
    }

    fn telegram_action_store_with_hashes(
        task_status: &str,
        pending_status: &str,
        monitor_hash: &str,
        pending_hash: &str,
    ) -> Value {
        json!({
            "tasks": [{
                "task_id": "monitor-telegram",
                "subject": "Reply",
                "status": task_status,
                "completed_via": if task_status == "completed" {
                    json!("human_approved_telegram_reply")
                } else {
                    Value::Null
                },
                "metadata": {
                    "_monitor": true,
                    "monitor": {
                        "schema_version": 2,
                        "kind": "telegram.reply",
                        "source_hash": monitor_hash,
                        "source": {
                            "connector_slug": "telegram-login",
                            "connection_slug": "telegram-user",
                            "chat_id": 42,
                            "message_id": 6836
                        },
                        "action": {"type": "telegram_reply_draft"}
                    },
                    "pending_action": {
                        "id": "telegram-action-1",
                        "type": "telegram_reply_draft_intent",
                        "status": pending_status,
                        "version": 4,
                        "agent_draft_text": "Draft from the agent.",
                        "monitor_hash": pending_hash
                    }
                }
            }]
        })
    }

    #[test]
    fn gmail_pending_action_creates_reply_draft_and_completes_task() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let monitor_hash = "sha256:test";
        write_store(
            &paths,
            json!({
                "tasks": [{
                    "task_id": "monitor-1",
                    "subject": "Reply",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor": {
                            "schema_version": 2,
                            "kind": "gmail.reply",
                            "source_hash": monitor_hash,
                            "source": {
                                "connector_slug": "gmail-browser",
                                "connection_slug": "gmail-browser",
                                "account": "me@example.com",
                                "thread_id": "thread-1"
                            },
                            "action": {"type": "gmail_reply_draft"}
                        },
                        "pending_action": {
                            "id": "action-1",
                            "type": "gmail_reply_draft_intent",
                            "status": "draft_ready",
                            "version": 1,
                            "agent_draft_text": "Works for me.",
                            "monitor_hash": monitor_hash
                        }
                    }
                }]
            }),
        );
        let executor = RecordingExecutor::default();

        handle_monitor_action_execute_with_executor(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "action_id": "action-1",
                "version": 1,
                "approved_message": "Works for me.",
                "client_request_id": "client-1"
            }),
            &executor,
        )
        .unwrap();

        let actions = executor.actions.lock().unwrap();
        assert_eq!(actions[0].connector_slug, "gmail-browser");
        assert_eq!(actions[0].action, "draft_reply");
        assert_eq!(actions[0].input["thread_id"], "thread-1");
        assert_eq!(actions[0].input["body"], "Works for me.");
        let store = read_monitor_store(&monitor_tasks_path(&paths)).unwrap();
        assert_eq!(store["tasks"][0]["status"], "completed");
        assert_eq!(
            store["tasks"][0]["completed_via"],
            "human_approved_gmail_draft"
        );
        assert_eq!(
            store["tasks"][0]["metadata"]["pending_action"]["status"],
            "draft_created"
        );
    }

    #[test]
    fn telegram_pending_action_sends_message_and_completes_task() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let monitor_hash = "sha256:test";
        write_store(
            &paths,
            json!({
                "tasks": [{
                    "task_id": "monitor-telegram",
                    "subject": "Reply",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor": {
                            "schema_version": 2,
                            "kind": "telegram.reply",
                            "source_hash": monitor_hash,
                            "source": {
                                "connector_slug": "telegram-login",
                                "connection_slug": "telegram-user",
                                "chat_id": 42,
                                "message_id": 6836
                            },
                            "action": {"type": "telegram_reply_draft"}
                        },
                        "pending_action": {
                            "id": "telegram-action-1",
                            "type": "telegram_reply_draft_intent",
                            "status": "draft_ready",
                            "version": 4,
                            "agent_draft_text": "Draft from the agent.",
                            "monitor_hash": monitor_hash
                        }
                    }
                }]
            }),
        );
        let executor = RecordingExecutor::default();

        handle_monitor_action_execute_with_executor(
            &paths,
            &json!({
                "task_id": "monitor-telegram",
                "action_id": "telegram-action-1",
                "version": 4,
                "approved_message": "Approved Telegram reply.",
                "client_request_id": "client-1"
            }),
            &executor,
        )
        .unwrap();

        let actions = executor.actions.lock().unwrap();
        assert_eq!(actions[0].connector_slug, "telegram-login");
        assert_eq!(actions[0].connection_slug, "telegram-user");
        assert_eq!(actions[0].action, "send_message");
        assert_eq!(actions[0].input["chat_id"], "42");
        assert_eq!(actions[0].input["message"], "Approved Telegram reply.");
        assert_eq!(actions[0].input["reply_to"], 6836);
        let store = read_monitor_store(&monitor_tasks_path(&paths)).unwrap();
        assert_eq!(store["tasks"][0]["status"], "completed");
        assert_eq!(
            store["tasks"][0]["completed_via"],
            "human_approved_telegram_reply"
        );
        assert_eq!(
            store["tasks"][0]["metadata"]["pending_action"]["status"],
            "sent"
        );
    }

    #[test]
    fn completed_sent_telegram_action_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        write_store(&paths, telegram_action_store("completed", "sent"));
        let executor = RecordingExecutor::default();

        let snapshot = handle_monitor_action_execute_with_executor(
            &paths,
            &json!({
                "task_id": "monitor-telegram",
                "action_id": "telegram-action-1",
                "version": 4,
                "approved_message": "Approved Telegram reply.",
                "client_request_id": "client-1"
            }),
            &executor,
        )
        .unwrap();

        assert_eq!(
            snapshot.pointer("/monitorActionExecute/status"),
            Some(&json!("already_sent"))
        );
        assert!(executor.actions.lock().unwrap().is_empty());
    }

    #[test]
    fn stale_sending_telegram_action_marks_uncertain_without_resending() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        write_store(&paths, telegram_action_store("pending", "sending"));
        let executor = RecordingExecutor::default();

        let error = handle_monitor_action_execute_with_executor(
            &paths,
            &json!({
                "task_id": "monitor-telegram",
                "action_id": "telegram-action-1",
                "version": 4,
                "approved_message": "Approved Telegram reply.",
                "client_request_id": "client-2"
            }),
            &executor,
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("duplicate_risk_ack_required"));
        assert!(executor.actions.lock().unwrap().is_empty());
        let store = read_monitor_store(&monitor_tasks_path(&paths)).unwrap();
        assert_eq!(
            store["tasks"][0]["metadata"]["pending_action"]["status"],
            "send_uncertain"
        );
        assert_eq!(
            store["tasks"][0]["metadata"]["monitor_action_events"][0]["event"],
            "typed_action_uncertain"
        );
    }

    #[test]
    fn telegram_action_rejects_identity_and_provenance_mismatch_before_execute() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        write_store(&paths, telegram_action_store("pending", "draft_ready"));
        let executor = RecordingExecutor::default();

        let error = handle_monitor_action_execute_with_executor(
            &paths,
            &json!({
                "task_id": "monitor-telegram",
                "action_id": "wrong-action",
                "version": 4,
                "approved_message": "Approved Telegram reply.",
                "client_request_id": "client-1"
            }),
            &executor,
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("action_id mismatch"));
        assert!(executor.actions.lock().unwrap().is_empty());

        write_store(
            &paths,
            telegram_action_store_with_hashes("pending", "draft_ready", "sha256:new", "sha256:old"),
        );
        let error = handle_monitor_action_execute_with_executor(
            &paths,
            &json!({
                "task_id": "monitor-telegram",
                "action_id": "telegram-action-1",
                "version": 4,
                "approved_message": "Approved Telegram reply.",
                "client_request_id": "client-1"
            }),
            &executor,
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("monitor hash mismatch"));
        assert!(executor.actions.lock().unwrap().is_empty());
    }

    #[test]
    fn telegram_reply_target_fallback_only_retries_known_message_id_errors() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        write_store(&paths, telegram_action_store("pending", "draft_ready"));
        let executor = SequenceExecutor::new(vec![
            Err("RPCError 400: MSG_ID_INVALID".to_string()),
            Ok(json!({"ok": true})),
        ]);

        handle_monitor_action_execute_with_executor(
            &paths,
            &json!({
                "task_id": "monitor-telegram",
                "action_id": "telegram-action-1",
                "version": 4,
                "approved_message": "Approved Telegram reply.",
                "client_request_id": "client-1"
            }),
            &executor,
        )
        .unwrap();

        let actions = executor.actions.lock().unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].input["reply_to"], 6836);
        assert!(actions[1].input.get("reply_to").is_none());
        drop(actions);

        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        write_store(&paths, telegram_action_store("pending", "draft_ready"));
        let executor = SequenceExecutor::new(vec![Err("reply quota exceeded".to_string())]);

        let error = handle_monitor_action_execute_with_executor(
            &paths,
            &json!({
                "task_id": "monitor-telegram",
                "action_id": "telegram-action-1",
                "version": 4,
                "approved_message": "Approved Telegram reply.",
                "client_request_id": "client-1"
            }),
            &executor,
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("monitor_action_failed"));
        assert_eq!(executor.actions.lock().unwrap().len(), 1);
        let store = read_monitor_store(&monitor_tasks_path(&paths)).unwrap();
        assert_eq!(
            store["tasks"][0]["metadata"]["pending_action"]["status"],
            "send_failed"
        );
    }

    #[test]
    fn telegram_pending_action_requires_chat_id() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let monitor_hash = "sha256:test";
        write_store(
            &paths,
            json!({
                "tasks": [{
                    "task_id": "monitor-telegram",
                    "subject": "Reply",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor": {
                            "schema_version": 2,
                            "kind": "telegram.reply",
                            "source_hash": monitor_hash,
                            "source": {
                                "connector_slug": "telegram-login",
                                "connection_slug": "telegram-user"
                            },
                            "action": {"type": "telegram_reply_draft"}
                        },
                        "pending_action": {
                            "id": "telegram-action-1",
                            "type": "telegram_reply_draft_intent",
                            "status": "draft_ready",
                            "version": 1,
                            "agent_draft_text": "Draft from the agent.",
                            "monitor_hash": monitor_hash
                        }
                    }
                }]
            }),
        );
        let executor = RecordingExecutor::default();

        let error = handle_monitor_action_execute_with_executor(
            &paths,
            &json!({
                "task_id": "monitor-telegram",
                "action_id": "telegram-action-1",
                "version": 1,
                "approved_message": "Approved Telegram reply.",
                "client_request_id": "client-1"
            }),
            &executor,
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("requires chat_id"));
        assert!(executor.actions.lock().unwrap().is_empty());
    }

    #[test]
    fn gmail_pending_action_requires_thread_id_for_reply_draft() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let monitor_hash = "sha256:test";
        write_store(
            &paths,
            json!({
                "tasks": [{
                    "task_id": "monitor-1",
                    "subject": "Reply",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor": {
                            "schema_version": 2,
                            "kind": "gmail.reply",
                            "source_hash": monitor_hash,
                            "source": {
                                "connector_slug": "gmail-browser",
                                "connection_slug": "gmail-browser",
                                "account": "me@example.com"
                            },
                            "action": {"type": "gmail_reply_draft"}
                        },
                        "pending_action": {
                            "id": "action-1",
                            "type": "gmail_reply_draft_intent",
                            "status": "draft_ready",
                            "version": 1,
                            "agent_draft_text": "Works for me.",
                            "monitor_hash": monitor_hash
                        }
                    }
                }]
            }),
        );
        let executor = RecordingExecutor::default();

        let error = handle_monitor_action_execute_with_executor(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "action_id": "action-1",
                "version": 1,
                "approved_message": "Works for me.",
                "client_request_id": "client-1"
            }),
            &executor,
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("requires thread_id or message_id"));
        assert!(executor.actions.lock().unwrap().is_empty());
    }

    #[test]
    fn calendar_pending_action_executes_approved_response() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let monitor_hash = "sha256:test";
        write_store(
            &paths,
            json!({
                "tasks": [{
                    "task_id": "monitor-1",
                    "subject": "RSVP",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor": {
                            "schema_version": 2,
                            "kind": "calendar.rsvp",
                            "source_hash": monitor_hash,
                            "source": {
                                "connector_slug": "gcal-browser",
                                "connection_slug": "gcal-browser",
                                "account": "me@example.com",
                                "event_id": "event-1"
                            },
                            "action": {"type": "calendar_rsvp"}
                        },
                        "pending_action": {
                            "id": "action-1",
                            "type": "calendar_rsvp",
                            "status": "awaiting_confirmation",
                            "version": 1,
                            "monitor_hash": monitor_hash
                        }
                    }
                }]
            }),
        );
        let executor = RecordingExecutor::default();

        handle_monitor_action_execute_with_executor(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "action_id": "action-1",
                "version": 1,
                "approved_response": "accept",
                "client_request_id": "client-1"
            }),
            &executor,
        )
        .unwrap();

        let actions = executor.actions.lock().unwrap();
        assert_eq!(actions[0].connector_slug, "gcal-browser");
        assert_eq!(actions[0].action, "accept");
        assert_eq!(actions[0].input["event_id"], "event-1");
        let store = read_monitor_store(&monitor_tasks_path(&paths)).unwrap();
        assert_eq!(store["tasks"][0]["status"], "completed");
        assert_eq!(
            store["tasks"][0]["metadata"]["pending_action"]["status"],
            "completed"
        );
    }
}
