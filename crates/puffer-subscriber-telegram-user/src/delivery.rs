//! Durable delivery cursor for Telegram message events.
//!
//! Telegram's session state tracks the MTProto update position, but Puffer
//! needs its own "last emitted to stdout" cursor so a restart can replay
//! messages that arrived while the subscriber was down instead of trusting
//! Telegram's live update delta alone.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use grammers_client::types::Message;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use crate::events::{build_message_event, emit, message_peer_metadata};
use crate::history_cache::TelegramHistoryCache;
use crate::notifications::NotificationMuteCache;
use crate::peer_cache::TelegramPeerCache;
use crate::state::SkillEnv;

const DELIVERY_SOURCE_LIVE: &str = "live";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct DeliveryCursor {
    #[serde(default)]
    initialized: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_user_id: Option<i64>,
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

    /// Clears stale delivery state when the connection authenticates as a different account.
    pub(crate) fn reset_for_account(&mut self, account_user_id: i64) -> bool {
        if self.account_user_id == Some(account_user_id) {
            return false;
        }
        self.account_user_id = Some(account_user_id);
        self.initialized = false;
        self.chats.clear();
        true
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
    if let Err(error) = record_message_history(env, message) {
        warn!(
            chat = %message.chat().id(),
            message_id = message.id(),
            %error,
            "failed to record Telegram message in bounded history cache"
        );
    }
    let notification_muted = notification_mutes.message_chat_muted(message);
    let notification_silent = message.silent();
    let is_outgoing = message.outgoing();
    if should_suppress_message(is_outgoing, notification_muted, notification_silent) {
        // Outgoing (self-sent) messages are recorded as seen but never emitted
        // into the triage pipeline: otherwise the user's own messages spin up a
        // triage turn (burning credits) and can be misread as incoming tasks.
        let stage = if is_outgoing {
            "suppressed_outgoing"
        } else {
            "suppressed"
        };
        append_message_diagnostic(
            env,
            stage,
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
            is_outgoing,
            notification_muted,
            notification_silent,
            "skipped Telegram message (outgoing or muted/silent)"
        );
        return Ok(false);
    }
    let peer_cache = TelegramPeerCache::load(env).unwrap_or_default();
    let event = build_message_event(
        &env.topic,
        message,
        Some(&peer_cache),
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

fn record_message_history(env: &SkillEnv, message: &Message) -> anyhow::Result<()> {
    // This read-modify-write path relies on the subscriber processing updates
    // serially. Startup hydration performs a merged final save so resume
    // backfill writes made through this path are not overwritten.
    let original = TelegramHistoryCache::load(env).unwrap_or_default();
    let mut cache = original.clone();
    cache.observe_message(message);
    cache.save_if_changed(env, &original)
}

fn message_notifications_suppressed(notification_muted: bool, notification_silent: bool) -> bool {
    notification_muted || notification_silent
}

/// Whether a message should be recorded as seen but NOT emitted into the triage
/// pipeline. Outgoing (self-sent) messages are always suppressed so the user's
/// own messages never trigger a triage turn (the #569 credit-burn bug); muted /
/// silent chats are suppressed per the user's notification settings.
fn should_suppress_message(
    is_outgoing: bool,
    notification_muted: bool,
    notification_silent: bool,
) -> bool {
    is_outgoing || message_notifications_suppressed(notification_muted, notification_silent)
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
    let peer_cache = TelegramPeerCache::load(env).unwrap_or_default();
    let peer = message_peer_metadata(message, Some(&peer_cache));
    let date_ms = i128::from(message.date().timestamp_millis());
    if peer.chat_is_bot || peer.sender_is_bot {
        return;
    }
    let record = json!({
        "at_ms": now_ms,
        "stage": stage,
        "delivery_source": delivery_source,
        "chat_id": chat.id(),
        "chat_kind": peer.chat_kind,
        "chat_title": peer.chat_title,
        "chat_username": peer.chat_username,
        "chat_is_bot": peer.chat_is_bot,
        "sender_id": peer.sender_id,
        "sender_username": peer.sender_username,
        "sender_name": peer.sender_name,
        "sender_is_bot": peer.sender_is_bot,
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
        assert_eq!(cursor.account_user_id, None);
        assert!(cursor.chats.is_empty());
    }

    #[test]
    fn delivery_cursor_resets_when_account_changes() {
        let mut cursor = DeliveryCursor {
            initialized: true,
            account_user_id: Some(111),
            chats: BTreeMap::from([("6156741935".to_string(), 5086)]),
        };

        assert!(cursor.reset_for_account(222));
        assert_eq!(cursor.account_user_id, Some(222));
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

    #[test]
    fn outgoing_messages_are_suppressed() {
        // #569: the user's own (outgoing) messages must be suppressed from the
        // triage pipeline even when the chat is not muted/silent.
        assert!(should_suppress_message(true, false, false));
        assert!(should_suppress_message(true, true, false));
        assert!(should_suppress_message(true, false, true));
        // Incoming messages still follow the notification-suppression rules.
        assert!(!should_suppress_message(false, false, false));
        assert!(should_suppress_message(false, true, false));
        assert!(should_suppress_message(false, false, true));
    }
}
