//! Message-centric monitor trace storage.
//!
//! Workflow history is binding/run centric. This store keeps a compact
//! message-centric view so UI diagnostics can answer "which stage did this
//! Telegram message reach?" without reverse-engineering several run rows.

use crate::action::TriageDecision;
use puffer_subscriber_runtime::EventEnvelope;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

const DEFAULT_MAX_MESSAGES_PER_CONNECTION: usize = 1000;
const TRACE_TEXT_PREVIEW_CHARS: usize = 200;

#[derive(Debug, Error)]
pub enum MonitorTraceStoreError {
    #[error("monitor trace store io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("monitor trace store json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorTraceStatus {
    Received,
    Suppressed,
    Emitted,
    RouterSkipped,
    DigestWaiting,
    TriageRunning,
    TriagedNoTask,
    TaskCreated,
    TaskUpdated,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorTraceStageStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorTraceStage {
    pub id: String,
    pub status: MonitorTraceStageStatus,
    pub at_ms: i128,
    pub source: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_batch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_batch_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_batch_position: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<TriageDecision>,
}

impl MonitorTraceStage {
    pub fn completed(
        id: impl Into<String>,
        source: impl Into<String>,
        summary: impl Into<String>,
        at_ms: i128,
    ) -> Self {
        Self {
            id: id.into(),
            status: MonitorTraceStageStatus::Completed,
            at_ms,
            source: source.into(),
            summary: summary.into(),
            binding_slug: None,
            envelope_id: None,
            digest_batch_id: None,
            digest_batch_count: None,
            digest_batch_position: None,
            raw_source: None,
            decision: None,
        }
    }

    pub fn failed(
        id: impl Into<String>,
        source: impl Into<String>,
        summary: impl Into<String>,
        at_ms: i128,
    ) -> Self {
        Self {
            status: MonitorTraceStageStatus::Failed,
            ..Self::completed(id, source, summary, at_ms)
        }
    }

    pub fn running(
        id: impl Into<String>,
        source: impl Into<String>,
        summary: impl Into<String>,
        at_ms: i128,
    ) -> Self {
        Self {
            status: MonitorTraceStageStatus::Running,
            ..Self::completed(id, source, summary, at_ms)
        }
    }

    fn dedup_key(&self) -> StageDedupKey {
        StageDedupKey {
            id: self.id.clone(),
            binding_slug: self.binding_slug.clone(),
            envelope_id: self.envelope_id.clone(),
            digest_batch_id: self.digest_batch_id.clone(),
        }
    }

    pub fn with_binding(mut self, binding_slug: impl Into<String>) -> Self {
        self.binding_slug = Some(binding_slug.into());
        self
    }

    pub fn with_envelope(mut self, envelope_id: impl Into<String>) -> Self {
        self.envelope_id = Some(envelope_id.into());
        self
    }

    pub fn with_digest(
        mut self,
        batch_id: impl Into<String>,
        count: usize,
        position: usize,
    ) -> Self {
        self.digest_batch_id = Some(batch_id.into());
        self.digest_batch_count = Some(count);
        self.digest_batch_position = Some(position);
        self
    }

    pub fn with_decision(mut self, decision: TriageDecision) -> Self {
        self.decision = Some(decision);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StageDedupKey {
    id: String,
    binding_slug: Option<String>,
    envelope_id: Option<String>,
    digest_batch_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorTraceIdentity {
    pub message_key: String,
    pub connection_slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedup_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_date_ms: Option<i128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at_ms: Option<i128>,
}

impl MonitorTraceIdentity {
    pub fn from_envelope(
        connection_slug: &str,
        connector_slug: Option<&str>,
        envelope: &EventEnvelope,
    ) -> Self {
        let payload = &envelope.event.payload;
        let chat_id = payload_string(payload, "chat_id");
        let message_id = payload_string(payload, "message_id");
        let message_key = match (chat_id.as_deref(), message_id.as_deref()) {
            (Some(chat_id), Some(message_id)) => {
                format!("{connection_slug}:{chat_id}:{message_id}")
            }
            _ => envelope
                .event
                .dedup_key
                .as_ref()
                .map(|dedup_key| format!("{connection_slug}:{dedup_key}"))
                .unwrap_or_else(|| format!("{connection_slug}:{}", envelope.envelope_id)),
        };
        Self {
            message_key,
            connection_slug: connection_slug.to_string(),
            connector_slug: connector_slug.map(ToOwned::to_owned),
            topic: Some(envelope.event.topic.clone()),
            kind: Some(envelope.event.kind.clone()),
            chat_id,
            chat_title: payload_string(payload, "chat_title")
                .or_else(|| payload_string(payload, "group_channel_name")),
            sender_id: payload_string(payload, "sender_id"),
            sender_name: payload_string(payload, "sender_name")
                .or_else(|| payload_string(payload, "sender_username")),
            message_id,
            dedup_key: envelope.event.dedup_key.clone(),
            envelope_id: Some(envelope.envelope_id.clone()),
            text: trace_text_preview(&envelope.event.text),
            event_date_ms: payload_i128(payload, "date_ms"),
            received_at_ms: Some(envelope.received_at_ms),
        }
    }

    #[cfg(test)]
    fn telegram_for_test(connection_slug: &str, chat_id: &str, message_id: &str) -> Self {
        Self {
            message_key: format!("{connection_slug}:{chat_id}:{message_id}"),
            connection_slug: connection_slug.to_string(),
            connector_slug: Some("telegram-login".into()),
            topic: Some(connection_slug.to_string()),
            kind: Some("message".into()),
            chat_id: Some(chat_id.to_string()),
            chat_title: None,
            sender_id: None,
            sender_name: None,
            message_id: Some(message_id.to_string()),
            dedup_key: Some(format!("{chat_id}:{message_id}")),
            envelope_id: Some(format!("env-{message_id}")),
            text: Some(format!("message {message_id}")),
            event_date_ms: Some(message_id.parse::<i128>().unwrap_or_default()),
            received_at_ms: Some(message_id.parse::<i128>().unwrap_or_default()),
        }
    }
}

fn payload_string(payload: &Value, key: &str) -> Option<String> {
    let value = payload.get(key)?;
    match value {
        Value::String(value) => {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn payload_i128(payload: &Value, key: &str) -> Option<i128> {
    let value = payload.get(key)?;
    if let Some(value) = value.as_i64() {
        return Some(i128::from(value));
    }
    if let Some(value) = value.as_u64() {
        return Some(i128::from(value));
    }
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .map(|value| value as i128)
}

fn trace_text_preview(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let mut truncated = value
        .chars()
        .take(TRACE_TEXT_PREVIEW_CHARS)
        .collect::<String>();
    if value.chars().count() > TRACE_TEXT_PREVIEW_CHARS {
        truncated.push_str("...");
    }
    Some(truncated)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorTraceMessage {
    pub message_key: String,
    pub connection_slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedup_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_date_ms: Option<i128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at_ms: Option<i128>,
    pub latest_status: MonitorTraceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    #[serde(default)]
    pub stages: Vec<MonitorTraceStage>,
}

impl MonitorTraceMessage {
    fn new(identity: MonitorTraceIdentity) -> Self {
        Self {
            message_key: identity.message_key,
            connection_slug: identity.connection_slug,
            connector_slug: identity.connector_slug,
            topic: identity.topic,
            kind: identity.kind,
            chat_id: identity.chat_id,
            chat_title: identity.chat_title,
            sender_id: identity.sender_id,
            sender_name: identity.sender_name,
            message_id: identity.message_id,
            dedup_key: identity.dedup_key,
            envelope_id: identity.envelope_id,
            text: identity.text.as_deref().and_then(trace_text_preview),
            event_date_ms: identity.event_date_ms,
            received_at_ms: identity.received_at_ms,
            latest_status: MonitorTraceStatus::Received,
            terminal_reason: None,
            stages: Vec::new(),
        }
    }

    fn merge_identity(&mut self, identity: MonitorTraceIdentity) {
        self.connector_slug = self.connector_slug.take().or(identity.connector_slug);
        self.topic = self.topic.take().or(identity.topic);
        self.kind = self.kind.take().or(identity.kind);
        self.chat_id = self.chat_id.take().or(identity.chat_id);
        self.chat_title = self.chat_title.take().or(identity.chat_title);
        self.sender_id = self.sender_id.take().or(identity.sender_id);
        self.sender_name = self.sender_name.take().or(identity.sender_name);
        self.message_id = self.message_id.take().or(identity.message_id);
        self.dedup_key = self.dedup_key.take().or(identity.dedup_key);
        self.envelope_id = self.envelope_id.take().or(identity.envelope_id);
        self.text = self
            .text
            .take()
            .or(identity.text)
            .as_deref()
            .and_then(trace_text_preview);
        self.event_date_ms = self.event_date_ms.or(identity.event_date_ms);
        self.received_at_ms = self.received_at_ms.or(identity.received_at_ms);
    }

    fn last_activity_ms(&self) -> i128 {
        self.stages
            .iter()
            .map(|stage| stage.at_ms)
            .max()
            .or(self.received_at_ms)
            .or(self.event_date_ms)
            .unwrap_or_default()
    }

    fn recompute_status(&mut self) {
        self.latest_status = derive_status(&self.stages);
        self.terminal_reason = terminal_reason(&self.stages);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TraceFile {
    #[serde(default)]
    messages: Vec<MonitorTraceMessage>,
}

pub struct MonitorTraceStore {
    path: PathBuf,
    inner: Mutex<TraceFile>,
    max_messages_per_connection: Mutex<usize>,
}

impl MonitorTraceStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, MonitorTraceStoreError> {
        let path = path.into();
        let inner = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            if raw.trim().is_empty() {
                TraceFile::default()
            } else {
                serde_json::from_str(&raw)?
            }
        } else {
            TraceFile::default()
        };
        Ok(Self {
            path,
            inner: Mutex::new(inner),
            max_messages_per_connection: Mutex::new(DEFAULT_MAX_MESSAGES_PER_CONNECTION),
        })
    }

    pub fn record_stage(
        &self,
        identity: MonitorTraceIdentity,
        stage: MonitorTraceStage,
    ) -> Result<(), MonitorTraceStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let message = match guard
            .messages
            .iter_mut()
            .find(|message| message.message_key == identity.message_key)
        {
            Some(message) => {
                message.merge_identity(identity);
                message
            }
            None => {
                guard.messages.push(MonitorTraceMessage::new(identity));
                guard.messages.last_mut().unwrap()
            }
        };
        let stage_key = stage.dedup_key();
        match message
            .stages
            .iter()
            .position(|existing| existing.dedup_key() == stage_key)
        {
            Some(index) => message.stages[index] = stage,
            None => message.stages.push(stage),
        }
        message.stages.sort_by(|a, b| a.at_ms.cmp(&b.at_ms));
        message.text = message.text.as_deref().and_then(trace_text_preview);
        message.recompute_status();
        let max_per_connection = *self.max_messages_per_connection.lock().unwrap();
        enforce_retention(&mut guard.messages, max_per_connection);
        write_atomic(&self.path, &*guard)?;
        Ok(())
    }

    pub fn record_envelope_stage(
        &self,
        connection_slug: &str,
        connector_slug: Option<&str>,
        envelope: &EventEnvelope,
        stage: MonitorTraceStage,
    ) -> Result<(), MonitorTraceStoreError> {
        let identity =
            MonitorTraceIdentity::from_envelope(connection_slug, connector_slug, envelope);
        self.record_stage(identity, stage.with_envelope(envelope.envelope_id.clone()))
    }

    pub fn list_recent(
        &self,
        connection_slug: Option<&str>,
        limit: usize,
    ) -> Vec<MonitorTraceMessage> {
        if limit == 0 {
            return Vec::new();
        }
        let mut messages = self.inner.lock().unwrap().messages.clone();
        if let Some(connection_slug) = connection_slug {
            messages.retain(|message| message.connection_slug == connection_slug);
        }
        messages.sort_by(|a, b| b.last_activity_ms().cmp(&a.last_activity_ms()));
        messages.truncate(limit);
        messages
    }

    #[cfg(test)]
    fn set_max_messages_per_connection_for_test(&self, max_messages: usize) {
        *self.max_messages_per_connection.lock().unwrap() = max_messages;
    }
}

fn derive_status(stages: &[MonitorTraceStage]) -> MonitorTraceStatus {
    if has_stage(stages, "task_created") {
        return MonitorTraceStatus::TaskCreated;
    }
    if has_stage(stages, "task_updated") || has_stage(stages, "reply_sent") {
        return MonitorTraceStatus::TaskUpdated;
    }
    if stages
        .iter()
        .any(|stage| stage.status == MonitorTraceStageStatus::Failed)
        || has_stage(stages, "delivery_emit_failed")
    {
        return MonitorTraceStatus::Failed;
    }
    if has_stage(stages, "triage_completed") {
        return MonitorTraceStatus::TriagedNoTask;
    }
    if has_stage(stages, "triage_started") {
        return MonitorTraceStatus::TriageRunning;
    }
    if has_stage(stages, "router_digest_queued") {
        return MonitorTraceStatus::DigestWaiting;
    }
    if stages
        .iter()
        .any(|stage| matches_router_skip_stage(stage.id.as_str()))
    {
        return MonitorTraceStatus::RouterSkipped;
    }
    if has_stage(stages, "delivery_duplicate") || has_stage(stages, "delivery_suppressed") {
        return MonitorTraceStatus::Suppressed;
    }
    if has_stage(stages, "delivery_emitted") || has_stage(stages, "connector_stdout_received") {
        return MonitorTraceStatus::Emitted;
    }
    MonitorTraceStatus::Received
}

fn has_stage(stages: &[MonitorTraceStage], id: &str) -> bool {
    stages.iter().any(|stage| stage.id == id)
}

fn matches_router_skip_stage(id: &str) -> bool {
    matches!(
        id,
        "router_no_monitor_binding"
            | "router_binding_paused"
            | "router_dedup_seen"
            | "router_self_gate_skipped"
            | "router_muted_skip"
            | "router_ignore_filter"
            | "router_contact_filter_skip"
            | "router_filter_skip"
            | "router_classifier_skip"
    )
}

fn terminal_reason(stages: &[MonitorTraceStage]) -> Option<String> {
    stages
        .iter()
        .rev()
        .find(|stage| {
            stage.status == MonitorTraceStageStatus::Failed
                || matches_router_skip_stage(stage.id.as_str())
                || stage.id == "delivery_duplicate"
                || stage.id == "delivery_suppressed"
                || stage.id == "delivery_emit_failed"
        })
        .map(|stage| stage.summary.clone())
}

fn enforce_retention(messages: &mut Vec<MonitorTraceMessage>, max_per_connection: usize) {
    if max_per_connection == 0 {
        messages.clear();
        return;
    }
    let mut indexed = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            (
                index,
                message.connection_slug.clone(),
                message.last_activity_ms(),
            )
        })
        .collect::<Vec<_>>();
    indexed.sort_by(|a, b| b.2.cmp(&a.2));
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut keep_indexes = HashSet::new();
    for (index, connection_slug, _) in indexed {
        let count = counts.entry(connection_slug).or_default();
        if *count < max_per_connection {
            keep_indexes.insert(index);
            *count += 1;
        }
    }
    let mut next = Vec::with_capacity(keep_indexes.len());
    for (index, message) in messages.drain(..).enumerate() {
        if keep_indexes.contains(&index) {
            next.push(message);
        }
    }
    *messages = next;
}

fn write_atomic(path: &Path, value: &TraceFile) -> Result<(), MonitorTraceStoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let raw = serde_json::to_vec_pretty(value)?;
    std::fs::write(&tmp, raw)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn derives_digest_waiting_until_triage_completes() {
        let dir = tempdir().unwrap();
        let store = MonitorTraceStore::load(dir.path().join("monitor-trace.json")).unwrap();
        let identity = MonitorTraceIdentity {
            message_key: "telegram-user:42:7".into(),
            connection_slug: "telegram-user".into(),
            connector_slug: Some("telegram-login".into()),
            topic: Some("telegram-user".into()),
            kind: Some("message".into()),
            chat_id: Some("42".into()),
            chat_title: Some("Chat".into()),
            sender_id: Some("9".into()),
            sender_name: Some("Alice".into()),
            message_id: Some("7".into()),
            dedup_key: Some("42:7".into()),
            envelope_id: Some("env-1".into()),
            text: Some("please review".into()),
            event_date_ms: Some(1000),
            received_at_ms: Some(1200),
        };

        store
            .record_stage(
                identity.clone(),
                MonitorTraceStage::completed(
                    "router_digest_queued",
                    "subscription_router",
                    "Queued monitor event for digest triage.",
                    1300,
                ),
            )
            .unwrap();

        let message = store.list_recent(Some("telegram-user"), 10)[0].clone();
        assert_eq!(message.latest_status, MonitorTraceStatus::DigestWaiting);

        store
            .record_stage(
                identity,
                MonitorTraceStage::completed(
                    "triage_completed",
                    "triage_agent",
                    "No task required.",
                    2000,
                ),
            )
            .unwrap();

        let message = store.list_recent(Some("telegram-user"), 10)[0].clone();
        assert_eq!(message.latest_status, MonitorTraceStatus::TriagedNoTask);
    }

    #[test]
    fn task_stage_wins_over_completed_triage() {
        let dir = tempdir().unwrap();
        let store = MonitorTraceStore::load(dir.path().join("monitor-trace.json")).unwrap();
        let identity = MonitorTraceIdentity::telegram_for_test("telegram-user", "42", "7");

        store
            .record_stage(
                identity.clone(),
                MonitorTraceStage::completed(
                    "triage_completed",
                    "triage_agent",
                    "Created task.",
                    1000,
                ),
            )
            .unwrap();
        store
            .record_stage(
                identity,
                MonitorTraceStage::completed(
                    "task_created",
                    "monitor_task",
                    "Created task task-1.",
                    1100,
                ),
            )
            .unwrap();

        let message = store.list_recent(Some("telegram-user"), 10)[0].clone();
        assert_eq!(message.latest_status, MonitorTraceStatus::TaskCreated);
    }

    #[test]
    fn persisted_trace_text_is_bounded() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("monitor-trace.json");
        let store = MonitorTraceStore::load(&path).unwrap();
        let full_text = "private launch notes ".repeat(20);
        let mut identity = MonitorTraceIdentity::telegram_for_test("telegram-user", "42", "7");
        identity.text = Some(full_text.clone());

        store
            .record_stage(
                identity,
                MonitorTraceStage::completed(
                    "delivery_emitted",
                    "telegram_subscriber",
                    "emitted",
                    1000,
                ),
            )
            .unwrap();

        let raw = std::fs::read_to_string(path).unwrap();
        assert!(!raw.contains(&full_text));
        let message = store.list_recent(Some("telegram-user"), 10)[0].clone();
        let text = message.text.unwrap();
        assert!(text.chars().count() <= 203);
        assert!(text.ends_with("..."));
    }

    #[test]
    fn retains_newest_messages_per_connection() {
        let dir = tempdir().unwrap();
        let store = MonitorTraceStore::load(dir.path().join("monitor-trace.json")).unwrap();
        store.set_max_messages_per_connection_for_test(2);

        for message_id in ["1", "2", "3"] {
            let at_ms = 1000 + message_id.parse::<i128>().unwrap();
            store
                .record_stage(
                    MonitorTraceIdentity::telegram_for_test("telegram-user", "42", message_id),
                    MonitorTraceStage::completed(
                        "delivery_emitted",
                        "telegram_subscriber",
                        "emitted",
                        at_ms,
                    ),
                )
                .unwrap();
        }

        let keys = store
            .list_recent(Some("telegram-user"), 10)
            .into_iter()
            .map(|message| message.message_key)
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["telegram-user:42:3", "telegram-user:42:2"]);
    }

    #[test]
    fn identity_from_envelope_prefers_chat_and_message_id() {
        let envelope = puffer_subscriber_runtime::EventEnvelope {
            envelope_id: "env-1".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 1200,
            event: puffer_subscriber_runtime::Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: Some("42:7".into()),
                text: "hello".into(),
                payload: serde_json::json!({
                    "chat_id": 42,
                    "chat_title": "Chat",
                    "sender_id": 9,
                    "sender_name": "Alice",
                    "message_id": 7,
                    "date_ms": 1000
                }),
            },
        };

        let identity =
            MonitorTraceIdentity::from_envelope("telegram-user", Some("telegram-login"), &envelope);

        assert_eq!(identity.message_key, "telegram-user:42:7");
        assert_eq!(identity.connector_slug.as_deref(), Some("telegram-login"));
        assert_eq!(identity.chat_id.as_deref(), Some("42"));
        assert_eq!(identity.message_id.as_deref(), Some("7"));
        assert_eq!(identity.event_date_ms, Some(1000));
        assert_eq!(identity.received_at_ms, Some(1200));
    }

    #[test]
    fn identity_from_envelope_falls_back_to_dedup_then_envelope() {
        let mut envelope = puffer_subscriber_runtime::EventEnvelope {
            envelope_id: "env-1".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 1200,
            event: puffer_subscriber_runtime::Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: Some("dedup-only".into()),
                text: "hello".into(),
                payload: serde_json::json!({}),
            },
        };

        let identity = MonitorTraceIdentity::from_envelope("telegram-user", None, &envelope);
        assert_eq!(identity.message_key, "telegram-user:dedup-only");

        envelope.event.dedup_key = None;
        let identity = MonitorTraceIdentity::from_envelope("telegram-user", None, &envelope);
        assert_eq!(identity.message_key, "telegram-user:env-1");
    }
}
