//! Durable delivery cursor for Telegram message events.
//!
//! Telegram's session state tracks the MTProto update position, but Puffer
//! needs its own "last emitted to stdout" cursor so a restart can replay
//! messages that arrived while the subscriber was down instead of trusting
//! Telegram's live update delta alone.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use grammers_client::types::{Chat, Message};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::info;

use crate::events::{build_message_event, emit};
use crate::notifications::NotificationMuteCache;
use crate::state::SkillEnv;

const DELIVERY_SOURCE_LIVE: &str = "live";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct DeliveryCursor {
    #[serde(default)]
    initialized: bool,
    #[serde(default)]
    chats: BTreeMap<String, i32>,
}

impl DeliveryCursor {
    /// Loads the durable delivery cursor for this subscriber.
    pub(crate) fn load(env: &SkillEnv) -> anyhow::Result<Self> {
        let path = env.delivery_cursor_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
    }

    /// Saves the durable delivery cursor atomically.
    pub(crate) fn save(&self, env: &SkillEnv) -> anyhow::Result<()> {
        let path = env.delivery_cursor_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create delivery cursor parent {}", parent.display()))?;
        }
        let tmp = path.with_extension("tmp");
        let body = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
    }

    /// Returns whether the cursor has completed its initial dialog scan.
    pub(crate) fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Marks the cursor as initialized after the startup dialog scan completes.
    pub(crate) fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    /// Returns whether this message's chat has an existing cursor entry.
    pub(crate) fn has_chat(&self, message: &Message) -> bool {
        self.chats.contains_key(&message_chat_key(message))
    }

    /// Returns whether this message has not been delivered according to the cursor.
    pub(crate) fn is_new(&self, message: &Message) -> bool {
        message.id()
            > self
                .chats
                .get(&message_chat_key(message))
                .copied()
                .unwrap_or_default()
    }

    /// Records the message as seen without emitting it.
    pub(crate) fn record_seen(&mut self, message: &Message) {
        let key = message_chat_key(message);
        self.record_chat_id(key, message.id());
    }

    fn record_chat_id(&mut self, key: String, message_id: i32) {
        let current = self.chats.entry(key).or_default();
        *current = (*current).max(message_id);
    }
}

/// Emits a Telegram message if the delivery cursor has not seen it yet.
pub(crate) async fn emit_message_if_new(
    env: &SkillEnv,
    cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
    message: &Message,
    delivery_source: &str,
    source_received_at_ms: Option<i128>,
) -> anyhow::Result<bool> {
    if !cursor.is_new(message) {
        let notification_muted = notification_mutes.message_chat_muted(message);
        let notification_silent = message.silent();
        append_message_diagnostic(
            env,
            "duplicate",
            message,
            delivery_source,
            source_received_at_ms,
            notification_muted,
            notification_silent,
        );
        return Ok(false);
    }
    let notification_muted = notification_mutes.message_chat_muted(message);
    let notification_silent = message.silent();
    if message_notifications_suppressed(notification_muted, notification_silent) {
        append_message_diagnostic(
            env,
            "suppressed",
            message,
            delivery_source,
            source_received_at_ms,
            notification_muted,
            notification_silent,
        );
        cursor.record_seen(message);
        cursor.save(env)?;
        info!(
            chat = %message.chat().id(),
            message_id = message.id(),
            notification_muted,
            notification_silent,
            "skipped suppressed Telegram message"
        );
        return Ok(false);
    }
    let event = build_message_event(
        &env.topic,
        message,
        notification_muted,
        delivery_source,
        source_received_at_ms,
    );
    emit(&event)?;
    append_message_diagnostic(
        env,
        "emitted",
        message,
        delivery_source,
        source_received_at_ms,
        notification_muted,
        notification_silent,
    );
    cursor.record_seen(message);
    cursor.save(env)?;
    Ok(true)
}

/// Emits a live Telegram update if the delivery cursor has not seen it yet.
pub(crate) async fn emit_live_message_if_new(
    env: &SkillEnv,
    cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
    message: &Message,
    source_received_at_ms: Option<i128>,
) -> anyhow::Result<bool> {
    emit_message_if_new(
        env,
        cursor,
        notification_mutes,
        message,
        DELIVERY_SOURCE_LIVE,
        source_received_at_ms,
    )
    .await
}

fn message_notifications_suppressed(notification_muted: bool, notification_silent: bool) -> bool {
    notification_muted || notification_silent
}

fn message_chat_key(message: &Message) -> String {
    message.chat().id().to_string()
}

fn append_message_diagnostic(
    env: &SkillEnv,
    stage: &str,
    message: &Message,
    delivery_source: &str,
    source_received_at_ms: Option<i128>,
    notification_muted: bool,
    notification_silent: bool,
) {
    let path = env.state_dir.join("message-diagnostics.ndjson");
    let now_ms = now_unix_millis();
    let chat = message.chat();
    let (chat_kind, chat_title, chat_username) = describe_chat(&chat);
    let date_ms = i128::from(message.date().timestamp_millis());
    let (sender_id, sender_username, sender_name) = match message.sender() {
        Some(sender) => (
            Some(sender.id()),
            sender.username().map(|value| value.to_string()),
            Some(sender.name().to_string()),
        ),
        None => (None, None, None),
    };
    let record = json!({
        "at_ms": now_ms,
        "stage": stage,
        "delivery_source": delivery_source,
        "chat_id": chat.id(),
        "chat_kind": chat_kind,
        "chat_title": chat_title,
        "chat_username": chat_username,
        "sender_id": sender_id,
        "sender_username": sender_username,
        "sender_name": sender_name,
        "message_id": message.id(),
        "date_ms": date_ms,
        "source_received_at_ms": source_received_at_ms,
        "subscriber_receive_lag_ms": source_received_at_ms.map(|received_at_ms| received_at_ms - date_ms),
        "subscriber_emit_lag_ms": now_ms - date_ms,
        "notification_muted": notification_muted,
        "notification_silent": notification_silent,
        "suppressed": message_notifications_suppressed(notification_muted, notification_silent),
        "is_outgoing": message.outgoing(),
        "text_prefix": truncate_text(message.text(), 200),
    });
    crate::diagnostics::append_ndjson(&path, &record);
}

fn describe_chat(chat: &Chat) -> (&'static str, Option<String>, Option<String>) {
    match chat {
        Chat::User(_) => (
            "user",
            Some(chat.name().to_string()),
            chat.username().map(|value| value.to_string()),
        ),
        Chat::Group(_) => (
            "group",
            Some(chat.name().to_string()),
            chat.username().map(|value| value.to_string()),
        ),
        Chat::Channel(_) => (
            "channel",
            Some(chat.name().to_string()),
            chat.username().map(|value| value.to_string()),
        ),
    }
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        truncated.push_str("...");
    }
    truncated
}

fn now_unix_millis() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i128
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_cursor_defaults_to_uninitialized() {
        let cursor = DeliveryCursor::default();
        assert!(!cursor.initialized);
        assert!(cursor.chats.is_empty());
    }

    #[test]
    fn suppressed_notifications_are_not_emitted() {
        assert!(message_notifications_suppressed(true, false));
        assert!(message_notifications_suppressed(false, true));
        assert!(message_notifications_suppressed(true, true));
        assert!(!message_notifications_suppressed(false, false));
    }
}
