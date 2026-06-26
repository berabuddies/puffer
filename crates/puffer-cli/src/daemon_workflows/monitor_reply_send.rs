use anyhow::{anyhow, bail, Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::{Event, EventEnvelope};
use puffer_subscriptions::{ActionDispatcher, ActionSpec, BuiltinActionDispatcher};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use uuid::Uuid;

use super::handle_workflow_list;
use super::monitor_task_ignore::{monitor_tasks_path, task_id_matches};
use crate::subscriptions::send_authorization_for_send_message_input_with_source;

#[derive(Debug, Deserialize)]
struct MonitorReplySendParams {
    #[serde(alias = "taskId")]
    task_id: String,
    #[serde(alias = "draftId")]
    draft_id: String,
    version: u64,
    message: String,
    #[serde(alias = "clientRequestId")]
    client_request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplySendTarget {
    pub connector_slug: String,
    pub connection_slug: String,
    pub chat_id: String,
    pub version: u64,
    /// Source message to thread the reply onto (Telegram `reply_to`).
    /// Stamped server-side from the trigger envelope; `None` for tasks that
    /// predate the stamping or when the reply fallback stripped it.
    pub reply_to_message_id: Option<i32>,
}

pub(crate) trait MonitorReplySender: Send + Sync {
    fn send_monitor_reply(&self, target: &ReplySendTarget, message: &str) -> Result<Value>;
}

static SEND_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

struct DispatcherMonitorReplySender;

impl MonitorReplySender for DispatcherMonitorReplySender {
    fn send_monitor_reply(&self, target: &ReplySendTarget, message: &str) -> Result<Value> {
        let mut input = json!({
            "connection_slug": target.connection_slug,
            "connector_slug": target.connector_slug,
            "chat_id": target.chat_id,
            "message": message,
        });
        if let Some(reply_to) = target.reply_to_message_id {
            input["reply_to"] = json!(reply_to);
        }
        input["_send_authorization"] =
            json!(send_authorization_for_send_message_input_with_source(
                "monitor-reply",
                &format!(
                    "monitor-reply:{}:{}",
                    target.chat_id,
                    target.reply_to_message_id.unwrap_or_default()
                ),
                target.version,
                "send_message",
                &input,
                message,
                &format!("monitor-reply:{}", Uuid::new_v4()),
            )?);
        let envelope = synthetic_envelope(&target.connection_slug, &input);
        let dispatcher = BuiltinActionDispatcher::new();
        let result = dispatcher.dispatch(
            &ActionSpec::ConnectorAct {
                connector_slug: target.connector_slug.clone(),
                action: "send_message".to_string(),
                input,
            },
            &envelope,
        );
        if !result.success {
            bail!("{}", result.summary);
        }
        Ok(json!({
            "success": true,
            "summary": result.summary,
        }))
    }
}

pub(crate) fn handle_monitor_reply_send(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    handle_monitor_reply_send_with_sender(paths, params, &DispatcherMonitorReplySender)
}

pub(crate) fn handle_monitor_reply_send_with_sender(
    paths: &ConfigPaths,
    params: &Value,
    sender: &dyn MonitorReplySender,
) -> Result<Value> {
    let params: MonitorReplySendParams =
        serde_json::from_value(params.clone()).context("invalid monitor reply send params")?;
    let task_id = non_empty(params.task_id.as_str())
        .context("missing task_id")?
        .to_string();
    let draft_id = non_empty(params.draft_id.as_str())
        .context("missing draft_id")?
        .to_string();
    let message = non_empty(params.message.as_str())
        .context("missing message")?
        .to_string();
    let client_request_id = non_empty(params.client_request_id.as_str())
        .context("missing client_request_id")?
        .to_string();

    let path = monitor_tasks_path(paths);
    let send_lock = monitor_send_lock(&path, &task_id);
    let _send_guard = send_lock.lock().unwrap();
    let mut store = read_monitor_store(&path)?;
    let task = find_task_mut(&mut store, &task_id)?;
    let terminal_status = task.get("status").and_then(Value::as_str);
    if matches!(terminal_status, Some("completed")) && is_human_approved_sent_task(task) {
        return snapshot_with_send_status(paths, "already_sent");
    }
    if matches!(terminal_status, Some("completed") | Some("cancelled")) {
        bail!("terminal_not_sent");
    }

    let metadata = task
        .get_mut("metadata")
        .and_then(Value::as_object_mut)
        .context("monitor task missing metadata")?;
    if !is_human_gated_reply_task(metadata) {
        bail!("task `{task_id}` is not a human-gated monitor reply task");
    }
    let pending_snapshot = metadata
        .get("pending_reply")
        .and_then(Value::as_object)
        .context("monitor task has no pending reply draft")?;
    validate_draft_identity(pending_snapshot, &draft_id, params.version)?;
    validate_pending_reply_provenance(metadata, pending_snapshot)?;
    let pending = metadata
        .get_mut("pending_reply")
        .and_then(Value::as_object_mut)
        .context("monitor task has no pending reply draft")?;
    match pending.get("status").and_then(Value::as_str) {
        Some("sent") => return snapshot_with_send_status(paths, "already_sent"),
        Some("sending") => {
            pending.insert(
                "status".to_string(),
                Value::String("send_uncertain".to_string()),
            );
            pending.insert(
                "error".to_string(),
                Value::String(
                    "previous send attempt was interrupted before receipt confirmation".to_string(),
                ),
            );
            append_audit(
                metadata,
                "send_uncertain",
                json!({
                    "task_id": task_id,
                    "draft_id": draft_id,
                    "version": params.version,
                    "client_request_id": client_request_id,
                    "reason": "stale_sending_state",
                }),
            );
            set_task_updated(task, now_ms());
            fs::write(&path, serde_json::to_string_pretty(&store)?)
                .with_context(|| format!("failed to write {}", path.display()))?;
            bail!("duplicate_risk_ack_required");
        }
        Some("send_uncertain") => bail!("duplicate_risk_ack_required"),
        Some("draft_ready" | "send_failed") => {}
        Some(other) => bail!("draft state `{other}` cannot be sent"),
        None => bail!("pending reply draft missing status"),
    }
    let target = reply_target_from_pending(pending, params.version)?;
    let attempt_id = Uuid::new_v4().to_string();
    let now = now_ms();
    pending.insert("status".to_string(), Value::String("sending".to_string()));
    pending.insert(
        "approved_message".to_string(),
        Value::String(message.clone()),
    );
    pending.insert(
        "approved_by".to_string(),
        Value::String("human".to_string()),
    );
    pending.insert("approved_at".to_string(), Value::from(now));
    pending.insert(
        "client_request_id".to_string(),
        Value::String(client_request_id.clone()),
    );
    pending.insert(
        "send_attempt_id".to_string(),
        Value::String(attempt_id.clone()),
    );
    pending.insert("error".to_string(), Value::Null);
    append_audit(
        metadata,
        "send_started",
        json!({
            "task_id": task_id,
            "draft_id": draft_id,
            "version": params.version,
            "client_request_id": client_request_id,
            "attempt_id": attempt_id,
        }),
    );
    set_task_updated(task, now);
    fs::write(&path, serde_json::to_string_pretty(&store)?)
        .with_context(|| format!("failed to write {}", path.display()))?;

    let send_result = sender.send_monitor_reply(&target, &message);
    // Reply threading is best-effort garnish: when Telegram rejects the
    // reply target (source message deleted since triage), deliver the
    // approved text as a plain message rather than failing the send.
    let send_result = match send_result {
        Err(error) if target.reply_to_message_id.is_some() && is_reply_target_error(&error) => {
            let mut plain = target.clone();
            plain.reply_to_message_id = None;
            sender.send_monitor_reply(&plain, &message)
        }
        other => other,
    };
    let mut store = read_monitor_store(&path)?;
    let task = find_task_mut(&mut store, &task_id)?;
    let metadata = task
        .get_mut("metadata")
        .and_then(Value::as_object_mut)
        .context("monitor task missing metadata after send")?;
    let pending = metadata
        .get_mut("pending_reply")
        .and_then(Value::as_object_mut)
        .context("monitor task missing pending reply after send")?;
    match send_result {
        Ok(receipt) => {
            pending.insert("status".to_string(), Value::String("sent".to_string()));
            pending.insert("receipt".to_string(), receipt.clone());
            append_audit(
                metadata,
                "send_succeeded",
                json!({
                    "task_id": task_id,
                    "draft_id": draft_id,
                    "version": params.version,
                    "client_request_id": client_request_id,
                    "attempt_id": attempt_id,
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
                    Value::String("human_approved_reply".to_string()),
                );
            set_task_updated(task, now_ms());
            fs::write(&path, serde_json::to_string_pretty(&store)?)
                .with_context(|| format!("failed to write {}", path.display()))?;
            snapshot_with_send_status(paths, "sent")
        }
        Err(error) => {
            pending.insert(
                "status".to_string(),
                Value::String("send_uncertain".to_string()),
            );
            pending.insert("error".to_string(), Value::String(format!("{error:#}")));
            append_audit(
                metadata,
                "send_uncertain",
                json!({
                    "task_id": task_id,
                    "draft_id": draft_id,
                    "version": params.version,
                    "client_request_id": client_request_id,
                    "attempt_id": attempt_id,
                    "error": format!("{error:#}"),
                }),
            );
            set_task_updated(task, now_ms());
            fs::write(&path, serde_json::to_string_pretty(&store)?)
                .with_context(|| format!("failed to write {}", path.display()))?;
            Err(anyhow!("send_uncertain: {error:#}"))
        }
    }
}

fn monitor_send_lock(path: &Path, task_id: &str) -> Arc<Mutex<()>> {
    let key = format!("{}::{task_id}", path.display());
    let locks = SEND_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().unwrap();
    locks
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn read_monitor_store(path: &std::path::Path) -> Result<Value> {
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

fn validate_draft_identity(
    pending: &Map<String, Value>,
    draft_id: &str,
    version: u64,
) -> Result<()> {
    if pending.get("id").and_then(Value::as_str) != Some(draft_id) {
        bail!("draft_id mismatch");
    }
    if pending.get("version").and_then(Value::as_u64) != Some(version) {
        bail!("draft version mismatch");
    }
    Ok(())
}

fn is_human_approved_sent_task(task: &Value) -> bool {
    task.pointer("/metadata/pending_reply/status")
        .and_then(Value::as_str)
        == Some("sent")
        || task.get("completed_via").and_then(Value::as_str) == Some("human_approved_reply")
}

fn is_human_gated_reply_task(metadata: &Map<String, Value>) -> bool {
    is_human_gated_policy(metadata) || metadata_has_telegram_delivery_target(metadata)
}

fn is_human_gated_policy(metadata: &Map<String, Value>) -> bool {
    let policy = metadata
        .get("completion_policy")
        .or_else(|| metadata.get("completionPolicy"));
    let mode = policy.and_then(|policy| {
        policy
            .get("mode")
            .and_then(Value::as_str)
            .or_else(|| policy.as_str())
    });
    matches!(mode, Some("draft_then_approve" | "send_to_source"))
        || policy
            .and_then(|policy| {
                policy
                    .get("requires_human_approval")
                    .or_else(|| policy.get("requiresHumanApproval"))
                    .and_then(Value::as_bool)
            })
            .unwrap_or(false)
}

fn metadata_has_telegram_delivery_target(metadata: &Map<String, Value>) -> bool {
    metadata_source_context(metadata)
        .and_then(|source| {
            let connector_slug = source
                .get("connector_slug")
                .or_else(|| source.get("connectorSlug"))
                .and_then(Value::as_str)?;
            connector_slug.contains("telegram").then_some(source)
        })
        .and_then(|source| {
            source
                .get("delivery_target")
                .or_else(|| source.get("deliveryTarget"))
                .and_then(|target| target.get("chat_id").or_else(|| target.get("chatId")))
                .and_then(value_to_string)
        })
        .is_some()
}

fn validate_pending_reply_provenance(
    metadata: &Map<String, Value>,
    pending: &Map<String, Value>,
) -> Result<()> {
    if pending.get("created_by").and_then(Value::as_str) != Some("MonitorReplyDraft") {
        bail!("pending reply draft was not created by MonitorReplyDraft");
    }
    let snapshot = pending
        .get("source_context_snapshot")
        .or_else(|| pending.get("sourceContextSnapshot"))
        .context("pending reply missing source context snapshot")?;
    let pending_hash = pending
        .get("source_context_hash")
        .or_else(|| pending.get("sourceContextHash"))
        .and_then(Value::as_str)
        .context("pending reply missing source context hash")?;
    let snapshot_hash = source_context_hash(snapshot)?;
    if pending_hash != snapshot_hash {
        bail!("pending reply source context hash mismatch");
    }
    let current_source =
        metadata_source_context(metadata).context("monitor task missing current source context")?;
    let current_hash = source_context_hash(&current_source)?;
    if pending_hash != current_hash {
        bail!("pending reply source context no longer matches monitor task source context");
    }
    Ok(())
}

fn metadata_source_context(metadata: &Map<String, Value>) -> Option<Value> {
    let context = metadata
        .get("source_context")
        .or_else(|| metadata.get("sourceContext"))
        .cloned()
        .or_else(|| derived_metadata_source_context(metadata));
    // Apply the same server-stamped field merge the draft snapshot got
    // (task_tools::monitor_source_context), so the provenance hash compares
    // like with like when the context was derived rather than stored.
    super::task_snapshot::with_verbatim_source_text(metadata, context)
}

fn derived_metadata_source_context(metadata: &Map<String, Value>) -> Option<Value> {
    let connector_slug = metadata_string(metadata, &["monitor_connector", "monitorConnector"])?;
    if !connector_slug.contains("telegram") {
        return None;
    }
    let chat_id = metadata_string(metadata, &["chat_id", "chatId"])?;
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
        "kind": "telegram_direct_message",
        "connection_slug": connection_slug,
        "connector_slug": connector_slug,
        "summary": format!("Telegram direct message from chat_id {chat_id}"),
        "delivery_target": {
            "type": "telegram_chat",
            "chat_id": chat_id,
        },
        "sender": sender,
    }))
}

fn metadata_string(metadata: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| metadata.get(*key))
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

/// Telegram rejects replies whose target message no longer exists (e.g.
/// MSG_ID_INVALID / REPLY_MESSAGE_ID_INVALID classes).
fn is_reply_target_error(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_ascii_uppercase();
    text.contains("MSG_ID") || text.contains("REPLY")
}

fn source_context_hash(source_context: &Value) -> Result<String> {
    let raw = serde_json::to_vec(source_context).context("failed to encode source context")?;
    let digest = Sha256::digest(raw);
    Ok(format!("{digest:x}"))
}

fn reply_target_from_pending(
    pending: &Map<String, Value>,
    version: u64,
) -> Result<ReplySendTarget> {
    let source = pending
        .get("source_context_snapshot")
        .or_else(|| pending.get("sourceContextSnapshot"))
        .context("pending reply missing source context snapshot")?;
    // Best-effort reply threading: thread onto the human-reviewed snapshot's
    // source message when it fits Telegram's i32 message-id space.
    let reply_to_message_id = source
        .get("message_id")
        .or_else(|| source.get("messageId"))
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    let connector_slug = source
        .get("connector_slug")
        .or_else(|| source.get("connectorSlug"))
        .and_then(Value::as_str)
        .context("source context missing connector_slug")?
        .to_string();
    let connection_slug = source
        .get("connection_slug")
        .or_else(|| source.get("connectionSlug"))
        .and_then(Value::as_str)
        .unwrap_or(&connector_slug)
        .to_string();
    let target = source
        .get("delivery_target")
        .or_else(|| source.get("deliveryTarget"))
        .context("source context missing delivery target")?;
    let chat_id = target
        .get("chat_id")
        .or_else(|| target.get("chatId"))
        .and_then(value_to_string)
        .context("source context delivery target missing chat_id")?
        .to_string();
    Ok(ReplySendTarget {
        connector_slug,
        connection_slug,
        chat_id,
        version,
        reply_to_message_id,
    })
}

fn append_audit(metadata: &mut Map<String, Value>, event: &str, details: Value) {
    let entry = json!({
        "event": event,
        "at_ms": now_ms(),
        "details": details,
    });
    match metadata.get_mut("monitor_reply_events") {
        Some(Value::Array(events)) => events.push(entry),
        _ => {
            metadata.insert(
                "monitor_reply_events".to_string(),
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

fn snapshot_with_send_status(paths: &ConfigPaths, status: &str) -> Result<Value> {
    let mut snapshot = handle_workflow_list(paths)?;
    if let Some(object) = snapshot.as_object_mut() {
        object.insert(
            "monitorReplySend".to_string(),
            json!({
                "status": status,
            }),
        );
    }
    Ok(snapshot)
}

fn synthetic_envelope(topic: &str, payload: &Value) -> EventEnvelope {
    EventEnvelope {
        envelope_id: format!(
            "monitor-reply-send-{}",
            OffsetDateTime::now_utc().unix_timestamp_nanos()
        ),
        subscriber_id: topic.to_string(),
        received_at_ms: OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000,
        event: Event {
            topic: topic.to_string(),
            kind: "connector_action".to_string(),
            control: false,
            dedup_key: None,
            text: payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            payload: payload.clone(),
        },
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use puffer_config::ConfigPaths;
    use serde_json::{json, Value};
    use std::fs;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    use super::{
        handle_monitor_reply_send_with_sender, source_context_hash, MonitorReplySender,
        ReplySendTarget,
    };
    use crate::daemon_workflows::monitor_task_ignore::monitor_tasks_path;

    #[derive(Default)]
    struct RecordingSender {
        calls: Arc<Mutex<Vec<(ReplySendTarget, String)>>>,
    }

    impl MonitorReplySender for RecordingSender {
        fn send_monitor_reply(
            &self,
            target: &ReplySendTarget,
            message: &str,
        ) -> anyhow::Result<Value> {
            self.calls
                .lock()
                .unwrap()
                .push((target.clone(), message.to_string()));
            Ok(json!({
                "message_id": "telegram-message-1",
                "summary": "sent"
            }))
        }
    }

    struct ReplyRejectingSender {
        calls: Arc<Mutex<Vec<(ReplySendTarget, String)>>>,
    }

    impl MonitorReplySender for ReplyRejectingSender {
        fn send_monitor_reply(
            &self,
            target: &ReplySendTarget,
            message: &str,
        ) -> anyhow::Result<Value> {
            self.calls
                .lock()
                .unwrap()
                .push((target.clone(), message.to_string()));
            if target.reply_to_message_id.is_some() {
                anyhow::bail!("send via telegram-user failed: rpc error 400: MSG_ID_INVALID")
            }
            Ok(json!({
                "message_id": "telegram-message-1",
                "summary": "sent"
            }))
        }
    }

    struct FailingSender;

    impl MonitorReplySender for FailingSender {
        fn send_monitor_reply(
            &self,
            _target: &ReplySendTarget,
            _message: &str,
        ) -> anyhow::Result<Value> {
            anyhow::bail!("transport disconnected")
        }
    }

    #[derive(Default)]
    struct SlowRecordingSender {
        calls: Arc<Mutex<Vec<(ReplySendTarget, String)>>>,
    }

    impl MonitorReplySender for SlowRecordingSender {
        fn send_monitor_reply(
            &self,
            target: &ReplySendTarget,
            message: &str,
        ) -> anyhow::Result<Value> {
            thread::sleep(Duration::from_millis(50));
            self.calls
                .lock()
                .unwrap()
                .push((target.clone(), message.to_string()));
            Ok(json!({
                "message_id": "telegram-message-1",
                "summary": "sent"
            }))
        }
    }

    fn write_store(paths: &ConfigPaths, store: Value) {
        let path = monitor_tasks_path(paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_string_pretty(&store).unwrap()).unwrap();
    }

    fn read_store(paths: &ConfigPaths) -> Value {
        serde_json::from_str(&fs::read_to_string(monitor_tasks_path(paths)).unwrap()).unwrap()
    }

    fn telegram_source_context() -> Value {
        telegram_source_context_with_chat_id("8759047281")
    }

    fn telegram_source_context_with_chat_id(chat_id: &str) -> Value {
        json!({
            "kind": "telegram_direct_message",
            "connection_slug": "telegram-user",
            "connector_slug": "telegram-login",
            "summary": format!("Telegram direct message from chat_id {chat_id}"),
            "delivery_target": {
                "type": "telegram_chat",
                "chat_id": chat_id
            },
            "sender": {}
        })
    }

    fn telegram_source_context_with_sender() -> Value {
        json!({
            "kind": "telegram_direct_message",
            "connection_slug": "telegram-user",
            "connector_slug": "telegram-login",
            "summary": "Telegram direct message from chat_id 8759047281",
            "delivery_target": {
                "type": "telegram_chat",
                "chat_id": "8759047281"
            },
            "sender": {
                "id": "8759047281",
                "username": "alice"
            }
        })
    }

    fn numeric_telegram_source_context() -> Value {
        json!({
            "kind": "telegram_direct_message",
            "connection_slug": "telegram-user",
            "connector_slug": "telegram-login",
            "summary": "Telegram direct message from chat_id 5229190700",
            "delivery_target": {
                "type": "telegram_chat",
                "chat_id": 5229190700_i64
            },
            "sender": {}
        })
    }

    fn telegram_source_context_with_message_id() -> Value {
        json!({
            "kind": "telegram_direct_message",
            "connection_slug": "telegram-user",
            "connector_slug": "telegram-login",
            "summary": "Telegram direct message from chat_id 8759047281",
            "delivery_target": {
                "type": "telegram_chat",
                "chat_id": "8759047281"
            },
            "sender": {},
            "text": "请排查 api-prod 服务 500 错误",
            "message_id": 6836
        })
    }

    fn task_with_draft() -> Value {
        task_with_draft_from_source_context(telegram_source_context())
    }

    fn task_with_draft_from_source_context(source_context: Value) -> Value {
        let source_hash = source_context_hash(&source_context).unwrap();
        json!({
            "task_id": "monitor-1",
            "subject": "Confirm P0/P1 risk",
            "description": "Needs Telegram reply.",
            "status": "pending",
            "metadata": {
                "_monitor": true,
                "monitor_connector": "telegram-login",
                "monitor_connection": "telegram-user",
                "source_context": source_context,
                "completion_policy": {
                    "mode": "draft_then_approve",
                    "requires_receipt": true,
                    "requires_human_approval": true
                },
                "pending_reply": {
                    "id": "draft-1",
                    "created_by": "MonitorReplyDraft",
                    "status": "draft_ready",
                    "version": 2,
                    "agent_draft_text": "Draft text",
                    "source_context_snapshot": source_context.clone(),
                    "source_context_hash": source_hash
                }
            }
        })
    }

    fn legacy_task_with_draft_from_source_context(source_context: Value, chat_id: Value) -> Value {
        let source_hash = source_context_hash(&source_context).unwrap();
        json!({
            "task_id": "monitor-1",
            "subject": "Confirm P0/P1 risk",
            "description": "Needs Telegram reply.",
            "status": "pending",
            "metadata": {
                "_monitor": true,
                "monitor_connector": "telegram-login",
                "monitor_connection": "telegram-user",
                "chat_id": chat_id,
                "pending_reply": {
                    "id": "draft-1",
                    "created_by": "MonitorReplyDraft",
                    "status": "draft_ready",
                    "version": 2,
                    "agent_draft_text": "Draft text",
                    "source_context_snapshot": source_context.clone(),
                    "source_context_hash": source_hash
                }
            }
        })
    }

    #[test]
    fn send_threads_reply_onto_snapshot_source_message() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        write_store(
            &paths,
            json!({"tasks": [task_with_draft_from_source_context(
                telegram_source_context_with_message_id()
            )]}),
        );
        let sender = RecordingSender::default();

        handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Draft text",
                "client_request_id": "req-1",
            }),
            &sender,
        )
        .unwrap();

        let calls = sender.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.reply_to_message_id, Some(6836));
    }

    #[test]
    fn send_falls_back_to_plain_message_when_reply_target_rejected() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        write_store(
            &paths,
            json!({"tasks": [task_with_draft_from_source_context(
                telegram_source_context_with_message_id()
            )]}),
        );
        let sender = ReplyRejectingSender {
            calls: Arc::new(Mutex::new(Vec::new())),
        };

        handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Draft text",
                "client_request_id": "req-1",
            }),
            &sender,
        )
        .unwrap();

        // First attempt threads the reply; the deleted-source rejection is
        // retried once as a plain message so delivery still succeeds.
        let calls = sender.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0.reply_to_message_id, Some(6836));
        assert_eq!(calls[1].0.reply_to_message_id, None);
        let store = read_store(&paths);
        assert_eq!(store["tasks"][0]["status"], "completed");
        assert_eq!(
            store["tasks"][0]["metadata"]["pending_reply"]["status"],
            "sent"
        );
    }

    #[test]
    fn send_without_source_message_id_stays_plain() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        write_store(&paths, json!({"tasks": [task_with_draft()]}));
        let sender = RecordingSender::default();

        handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Draft text",
                "client_request_id": "req-1",
            }),
            &sender,
        )
        .unwrap();

        let calls = sender.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.reply_to_message_id, None);
    }

    #[test]
    fn human_approved_send_uses_snapshot_target_and_completes_task() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        write_store(&paths, json!({"tasks": [task_with_draft()]}));
        let sender = RecordingSender::default();

        handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Approved text",
                "client_request_id": "request-1"
            }),
            &sender,
        )
        .unwrap();

        let calls = sender.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.connector_slug, "telegram-login");
        assert_eq!(calls[0].0.connection_slug, "telegram-user");
        assert_eq!(calls[0].0.chat_id, "8759047281");
        assert_eq!(calls[0].1, "Approved text");

        let stored = read_store(&paths);
        let task = &stored["tasks"][0];
        assert_eq!(task["status"], "completed");
        assert_eq!(task["completed_via"], "human_approved_reply");
        assert_eq!(task["metadata"]["pending_reply"]["status"], "sent");
        assert_eq!(
            task["metadata"]["pending_reply"]["approved_message"],
            "Approved text"
        );
        assert!(task["metadata"]["pending_reply"]["receipt"].is_object());
    }

    #[test]
    fn human_approved_send_accepts_numeric_snapshot_chat_id() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        write_store(
            &paths,
            json!({"tasks": [task_with_draft_from_source_context(numeric_telegram_source_context())]}),
        );
        let sender = RecordingSender::default();

        handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Approved text",
                "client_request_id": "request-1"
            }),
            &sender,
        )
        .unwrap();

        let calls = sender.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.chat_id, "5229190700");
    }

    #[test]
    fn human_approved_send_accepts_legacy_task_without_completion_policy() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        write_store(
            &paths,
            json!({"tasks": [legacy_task_with_draft_from_source_context(
                telegram_source_context(),
                json!("8759047281")
            )]}),
        );
        let sender = RecordingSender::default();

        handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Approved text",
                "client_request_id": "request-1"
            }),
            &sender,
        )
        .unwrap();

        let calls = sender.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.chat_id, "8759047281");
        let stored = read_store(&paths);
        assert_eq!(stored["tasks"][0]["status"], "completed");
        assert_eq!(stored["tasks"][0]["completed_via"], "human_approved_reply");
    }

    #[test]
    fn human_approved_send_accepts_legacy_task_with_sender_identity() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let mut task = legacy_task_with_draft_from_source_context(
            telegram_source_context_with_sender(),
            json!("8759047281"),
        );
        task["metadata"]["sender_id"] = json!("8759047281");
        task["metadata"]["sender_username"] = json!("alice");
        write_store(&paths, json!({"tasks": [task]}));
        let sender = RecordingSender::default();

        handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Approved text",
                "client_request_id": "request-1"
            }),
            &sender,
        )
        .unwrap();

        let calls = sender.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.chat_id, "8759047281");
    }

    #[test]
    fn human_approved_send_accepts_numeric_legacy_task_without_completion_policy() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        write_store(
            &paths,
            json!({"tasks": [legacy_task_with_draft_from_source_context(
                telegram_source_context_with_chat_id("5229190700"),
                json!(5229190700_i64)
            )]}),
        );
        let sender = RecordingSender::default();

        handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Approved text",
                "client_request_id": "request-1"
            }),
            &sender,
        )
        .unwrap();

        let calls = sender.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.chat_id, "5229190700");
    }

    #[test]
    fn duplicate_client_request_does_not_send_twice_after_success() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        write_store(&paths, json!({"tasks": [task_with_draft()]}));
        let sender = RecordingSender::default();
        let params = json!({
            "task_id": "monitor-1",
            "draft_id": "draft-1",
            "version": 2,
            "message": "Approved text",
            "client_request_id": "request-1"
        });

        handle_monitor_reply_send_with_sender(&paths, &params, &sender).unwrap();
        let result = handle_monitor_reply_send_with_sender(&paths, &params, &sender).unwrap();

        assert_eq!(
            result["monitorReplySend"]["status"].as_str(),
            Some("already_sent")
        );
        assert_eq!(sender.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn send_uncertain_requires_duplicate_risk_ack_instead_of_resending() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let mut task = task_with_draft();
        task["metadata"]["pending_reply"]["status"] = json!("send_uncertain");
        write_store(&paths, json!({"tasks": [task]}));
        let sender = RecordingSender::default();

        let error = handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Approved text",
                "client_request_id": "request-1"
            }),
            &sender,
        )
        .expect_err("send_uncertain must not use normal send");

        assert!(error.to_string().contains("duplicate_risk_ack_required"));
        assert_eq!(sender.calls.lock().unwrap().len(), 0);
    }

    #[test]
    fn stale_sending_state_is_restored_to_uncertain() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let mut task = task_with_draft();
        task["metadata"]["pending_reply"]["status"] = json!("sending");
        write_store(&paths, json!({"tasks": [task]}));
        let sender = RecordingSender::default();

        let error = handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Approved text",
                "client_request_id": "request-1"
            }),
            &sender,
        )
        .expect_err("stale sending state must not resend");

        assert!(error.to_string().contains("duplicate_risk_ack_required"));
        assert_eq!(sender.calls.lock().unwrap().len(), 0);
        let stored = read_store(&paths);
        assert_eq!(
            stored["tasks"][0]["metadata"]["pending_reply"]["status"],
            "send_uncertain"
        );
    }

    #[test]
    fn forged_pending_reply_without_draft_provenance_is_rejected() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let mut task = task_with_draft();
        task["metadata"]["pending_reply"]
            .as_object_mut()
            .unwrap()
            .remove("created_by");
        write_store(&paths, json!({"tasks": [task]}));
        let sender = RecordingSender::default();

        let error = handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Approved text",
                "client_request_id": "request-1"
            }),
            &sender,
        )
        .expect_err("send must reject pending replies not created by MonitorReplyDraft");

        assert!(error
            .to_string()
            .contains("was not created by MonitorReplyDraft"));
        assert_eq!(sender.calls.lock().unwrap().len(), 0);
    }

    #[test]
    fn failed_send_is_marked_uncertain_and_blocks_normal_retry() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        write_store(&paths, json!({"tasks": [task_with_draft()]}));

        let error = handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Approved text",
                "client_request_id": "request-1"
            }),
            &FailingSender,
        )
        .expect_err("ambiguous transport failures must not be normal retryable failures");

        assert!(error.to_string().contains("send_uncertain"));
        let stored = read_store(&paths);
        assert_eq!(
            stored["tasks"][0]["metadata"]["pending_reply"]["status"],
            "send_uncertain"
        );

        let sender = RecordingSender::default();
        let retry = handle_monitor_reply_send_with_sender(
            &paths,
            &json!({
                "task_id": "monitor-1",
                "draft_id": "draft-1",
                "version": 2,
                "message": "Approved text",
                "client_request_id": "request-2"
            }),
            &sender,
        )
        .expect_err("uncertain send needs explicit duplicate-risk acknowledgement");
        assert!(retry.to_string().contains("duplicate_risk_ack_required"));
        assert_eq!(sender.calls.lock().unwrap().len(), 0);
    }

    #[test]
    fn concurrent_send_requests_do_not_double_send() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        write_store(&paths, json!({"tasks": [task_with_draft()]}));
        let sender = Arc::new(SlowRecordingSender::default());
        let barrier = Arc::new(Barrier::new(2));

        let mut handles = Vec::new();
        for request_id in ["request-1", "request-2"] {
            let paths = paths.clone();
            let sender = sender.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                handle_monitor_reply_send_with_sender(
                    &paths,
                    &json!({
                        "task_id": "monitor-1",
                        "draft_id": "draft-1",
                        "version": 2,
                        "message": "Approved text",
                        "client_request_id": request_id
                    }),
                    sender.as_ref(),
                )
            }));
        }

        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        assert!(results.iter().all(Result::is_ok));
        assert_eq!(sender.calls.lock().unwrap().len(), 1);
        let stored = read_store(&paths);
        assert_eq!(stored["tasks"][0]["status"], "completed");
        assert_eq!(
            stored["tasks"][0]["metadata"]["pending_reply"]["status"],
            "sent"
        );
    }
}
