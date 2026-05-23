//! Event-envelope helpers.
//!
//! All outbound ndjson lines are built via [`emit`], which serializes an
//! [`Event`] to stdout followed by a newline and a flush. Diagnostics that
//! should NOT appear on stdout belong in `tracing`; stdout is reserved for
//! the runtime bus.

use std::io::Write as _;

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
/// handlers rely on (chat id, chat kind, sender id, date in ms, etc.).
pub fn build_message_event(topic: &str, message: &Message) -> Event {
    let chat = message.chat();
    let (chat_kind, chat_title, chat_username) = describe_chat(&chat);
    let chat_id = chat.id();
    let message_id = message.id();
    let date_ms = message.date().timestamp_millis();
    let is_outgoing = message.outgoing();

    let (sender_id, sender_username, sender_name) = match message.sender() {
        Some(s) => (
            Some(s.id()),
            s.username().map(|u| u.to_string()),
            Some(s.name().to_string()),
        ),
        None => (None, None, None),
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
    if let Some(id) = sender_id {
        payload.insert("sender_id".to_string(), json!(id));
    }
    if let Some(username) = sender_username {
        payload.insert("sender_username".to_string(), json!(username));
    }
    if let Some(name) = sender_name {
        payload.insert("sender_name".to_string(), json!(name));
    }
    payload.insert("message_id".to_string(), json!(message_id));
    payload.insert("date_ms".to_string(), json!(date_ms));
    payload.insert("is_outgoing".to_string(), json!(is_outgoing));
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

/// Returns `(chat_kind, chat_title, chat_username)` describing a chat.
///
/// `chat_kind` is always one of `"user"`, `"group"`, `"channel"`. For user
/// chats the "title" is the user's display name and "username" is their
/// `@handle` if any. For groups/channels the title is the chat title.
fn describe_chat(chat: &Chat) -> (&'static str, Option<String>, Option<String>) {
    match chat {
        Chat::User(_) => (
            "user",
            Some(chat.name().to_string()),
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
