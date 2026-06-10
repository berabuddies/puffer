//! Event-envelope helpers.
//!
//! All outbound ndjson lines are built via [`emit`], which serializes an
//! [`Event`] to stdout followed by a newline and a flush. Diagnostics that
//! should NOT appear on stdout belong in `tracing`; stdout is reserved for
//! the runtime bus.

use std::io::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use grammers_client::types::{Chat, Media, Message};
use puffer_subscriber_runtime::Event;
use serde_json::{json, Value};

use crate::polls::{poll_payload, poll_text};
use crate::reply::reply_header_payload;

/// Writes one [`Event`] to stdout as a single JSON line and flushes.
///
/// Stdout is the subscriber runtime's bus channel, so writes must not be
/// interleaved with any other output. The helper takes a lock on stdout for
/// the duration of the write to make the line atomic with respect to other
/// threads in the same process.
pub fn emit(event: &Event) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer(&mut handle, event).context("serialize event to stdout")?;
    handle.write_all(b"\n").context("write newline to stdout")?;
    handle.flush().context("flush stdout")?;
    Ok(())
}

/// Emits a control-flow event (login state changes, errors, etc.) on the
/// subscriber's configured topic.
pub fn emit_control(topic: &str, kind: &str, payload: Value) -> anyhow::Result<()> {
    let event = Event {
        topic: topic.to_string(),
        kind: kind.to_string(),
        control: true,
        dedup_key: None,
        text: String::new(),
        payload,
    };
    emit(&event)
}

/// Builds the `message`-kind event for a new Telegram message update.
///
/// `kind` is `"channel_post"` when the message came from a broadcast channel
/// and `"message"` otherwise. The payload contains routing fields action
/// handlers rely on (chat id, chat kind, sender id, date in ms, delivery
/// source, etc.).
pub fn build_message_event(
    topic: &str,
    message: &Message,
    notification_muted: bool,
    delivery_source: &str,
    source_received_at_ms: Option<i128>,
) -> Event {
    let chat = message.chat();
    let (chat_kind, chat_title, chat_username) = describe_chat(&chat);
    let chat_is_bot = telegram_chat_is_bot(&chat);
    let chat_id = chat.id();
    let message_id = message.id();
    let date_ms = message.date().timestamp_millis();
    let date = message.date().to_rfc3339();
    let is_outgoing = message.outgoing();
    let emit_at_ms = now_unix_millis();

    let (sender_id, sender_username, sender_name, sender_is_bot) = match message.sender() {
        Some(s) => (
            Some(s.id()),
            s.username().map(|u| u.to_string()),
            Some(chat_display_name(&s)),
            telegram_chat_is_bot(&s),
        ),
        None => (None, None, None, false),
    };

    let kind = if matches!(chat, Chat::Channel(_)) {
        "channel_post"
    } else {
        "message"
    };

    let mut payload = serde_json::Map::new();
    payload.insert("chat_id".to_string(), json!(chat_id));
    payload.insert("chat_kind".to_string(), json!(chat_kind));
    if let Some(title) = chat_title {
        payload.insert("chat_title".to_string(), json!(title));
    }
    if let Some(username) = chat_username {
        payload.insert("chat_username".to_string(), json!(username));
    }
    payload.insert("chat_is_bot".to_string(), json!(chat_is_bot));
    if let Some(id) = sender_id {
        payload.insert("sender_id".to_string(), json!(id));
    }
    if let Some(username) = sender_username {
        payload.insert("sender_username".to_string(), json!(username));
    }
    if let Some(name) = sender_name {
        payload.insert("sender_name".to_string(), json!(name));
    }
    payload.insert("sender_is_bot".to_string(), json!(sender_is_bot));
    payload.insert("message_id".to_string(), json!(message_id));
    payload.insert("date".to_string(), json!(date));
    payload.insert("date_ms".to_string(), json!(date_ms));
    payload.insert("delivery_source".to_string(), json!(delivery_source));
    payload.insert("subscriber_emit_at_ms".to_string(), json!(emit_at_ms));
    payload.insert(
        "subscriber_emit_lag_ms".to_string(),
        json!(emit_at_ms - i128::from(date_ms)),
    );
    if let Some(received_at_ms) = source_received_at_ms {
        payload.insert(
            "subscriber_received_at_ms".to_string(),
            json!(received_at_ms),
        );
        payload.insert(
            "subscriber_receive_lag_ms".to_string(),
            json!(received_at_ms - i128::from(date_ms)),
        );
        payload.insert(
            "subscriber_queue_lag_ms".to_string(),
            json!(emit_at_ms.saturating_sub(received_at_ms)),
        );
    }
    payload.insert("is_outgoing".to_string(), json!(is_outgoing));
    payload.insert("notification_muted".to_string(), json!(notification_muted));
    payload.insert("notification_silent".to_string(), json!(message.silent()));
    if let Some(reply_to) = reply_header_payload(message.reply_header()) {
        payload.insert("reply_to".to_string(), reply_to);
    }
    if let Some(reply_count) = message.reply_count() {
        payload.insert("reply_count".to_string(), json!(reply_count));
    }
    if let Some(Media::Poll(poll)) = message.media() {
        payload.insert("media".to_string(), json!(poll_text(&poll)));
        payload.insert("poll".to_string(), poll_payload(&poll));
    }

    Event {
        topic: topic.to_string(),
        kind: kind.to_string(),
        control: false,
        dedup_key: Some(format!("{chat_id}:{message_id}")),
        text: message.text().to_string(),
        payload: Value::Object(payload),
    }
}

fn now_unix_millis() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i128
}

/// Returns `(chat_kind, chat_title, chat_username)` describing a chat.
///
/// `chat_kind` is always one of `"user"`, `"group"`, `"channel"`. For user
/// chats the "title" is the user's display name and "username" is their
/// `@handle` if any. For groups/channels the title is the chat title.
fn describe_chat(chat: &Chat) -> (&'static str, Option<String>, Option<String>) {
    match chat {
        Chat::User(_) => (
            "user",
            Some(chat_display_name(chat)),
            chat.username().map(|u| u.to_string()),
        ),
        Chat::Group(_) => (
            "group",
            Some(chat.name().to_string()),
            chat.username().map(|u| u.to_string()),
        ),
        Chat::Channel(_) => (
            "channel",
            Some(chat.name().to_string()),
            chat.username().map(|u| u.to_string()),
        ),
    }
}

fn chat_display_name(chat: &Chat) -> String {
    match chat {
        Chat::User(user) => user.full_name(),
        Chat::Group(_) | Chat::Channel(_) => chat.name().to_string(),
    }
}

fn telegram_chat_is_bot(chat: &Chat) -> bool {
    matches!(chat, Chat::User(user) if user.raw.bot)
        || chat
            .username()
            .is_some_and(telegram_username_looks_like_bot)
}

fn telegram_username_looks_like_bot(username: &str) -> bool {
    username.to_ascii_lowercase().ends_with("bot")
}
