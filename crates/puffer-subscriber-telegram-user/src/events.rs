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

use crate::peer_cache::TelegramPeerCache;
use crate::polls::{poll_payload, poll_text};
use crate::reply::reply_header_payload;

/// Normalized Telegram peer fields for one message event.
#[derive(Debug, Clone)]
pub(crate) struct MessagePeerMetadata {
    /// Chat kind: `user`, `group`, or `channel`.
    pub(crate) chat_kind: &'static str,
    /// Human-readable chat title.
    pub(crate) chat_title: Option<String>,
    /// Chat public username, when any.
    pub(crate) chat_username: Option<String>,
    /// Whether the chat is a bot.
    pub(crate) chat_is_bot: bool,
    /// Stable sender user id, when Telegram exposes it.
    pub(crate) sender_id: Option<i64>,
    /// Sender public username, when any.
    pub(crate) sender_username: Option<String>,
    /// Sender display name.
    pub(crate) sender_name: Option<String>,
    /// Whether the sender is a bot.
    pub(crate) sender_is_bot: bool,
}

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
    peer_cache: Option<&TelegramPeerCache>,
    notification_muted: bool,
    delivery_source: &str,
    source_received_at_ms: Option<i128>,
) -> Event {
    let chat = message.chat();
    let peer = message_peer_metadata(message, peer_cache);
    let chat_id = chat.id();
    let message_id = message.id();
    let date_ms = message.date().timestamp_millis();
    let date = message.date().to_rfc3339();
    let is_outgoing = message.outgoing();
    let emit_at_ms = now_unix_millis();

    let kind = if matches!(chat, Chat::Channel(_)) {
        "channel_post"
    } else {
        "message"
    };

    let mut payload = serde_json::Map::new();
    payload.insert("chat_id".to_string(), json!(chat_id));
    payload.insert("chat_kind".to_string(), json!(peer.chat_kind));
    if let Some(title) = peer.chat_title.as_deref() {
        let group_channel_name = group_channel_name(&peer);
        payload.insert("chat_title".to_string(), json!(title));
        if let Some(name) = group_channel_name {
            payload.insert("group_channel_name".to_string(), json!(name));
        }
    }
    if let Some(username) = peer.chat_username {
        payload.insert("chat_username".to_string(), json!(username));
    }
    payload.insert("chat_is_bot".to_string(), json!(peer.chat_is_bot));
    if let Some(id) = peer.sender_id {
        payload.insert("sender_id".to_string(), json!(id));
    }
    if let Some(username) = peer.sender_username {
        payload.insert("sender_username".to_string(), json!(username));
    }
    if let Some(name) = peer.sender_name {
        payload.insert("sender_name".to_string(), json!(name));
    }
    payload.insert("sender_is_bot".to_string(), json!(peer.sender_is_bot));
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
    if let Some(media) = message.media() {
        payload.insert("media".to_string(), json!(message_media_label(&media)));
        if let Media::Poll(poll) = media {
            payload.insert("poll".to_string(), poll_payload(&poll));
        }
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

/// Builds normalized peer metadata and backfills sparse direct-user updates
/// from the durable peer cache when Telegram omits display fields.
pub(crate) fn message_peer_metadata(
    message: &Message,
    peer_cache: Option<&TelegramPeerCache>,
) -> MessagePeerMetadata {
    let chat = message.chat();
    let chat_id = chat.id();
    let (chat_kind, chat_title, chat_username) = describe_chat(&chat);
    let mut peer = MessagePeerMetadata {
        chat_kind,
        chat_title: nonempty_owned(chat_title),
        chat_username: nonempty_owned(chat_username),
        chat_is_bot: telegram_chat_is_bot(&chat),
        sender_id: None,
        sender_username: None,
        sender_name: None,
        sender_is_bot: false,
    };

    if let Some(sender) = message.sender() {
        peer.sender_id = Some(sender.id());
        peer.sender_username = nonempty_owned(sender.username().map(|value| value.to_string()));
        peer.sender_name = nonempty(chat_display_name(&sender));
        peer.sender_is_bot = telegram_chat_is_bot(&sender);
    }

    backfill_message_peer_metadata(&mut peer, chat_id, peer_cache, message.outgoing());

    peer
}

fn backfill_message_peer_metadata(
    peer: &mut MessagePeerMetadata,
    chat_id: i64,
    peer_cache: Option<&TelegramPeerCache>,
    is_outgoing: bool,
) {
    if peer.chat_title.is_none() {
        peer.chat_title = peer_cache.and_then(|cache| cache.title_for(peer.chat_kind, chat_id));
    }
    if peer.chat_username.is_none() {
        peer.chat_username =
            peer_cache.and_then(|cache| cache.username_for(peer.chat_kind, chat_id));
    }
    if peer.sender_name.is_none() {
        peer.sender_name = peer
            .sender_id
            .and_then(|id| peer_cache.and_then(|cache| cache.title_for("user", id)));
    }
    if peer.sender_username.is_none() {
        peer.sender_username = peer
            .sender_id
            .and_then(|id| peer_cache.and_then(|cache| cache.username_for("user", id)));
    }

    if peer.chat_kind == "user" {
        if !is_outgoing {
            if peer.sender_name.is_none() {
                peer.sender_name = peer.chat_title.clone();
            }
            if peer.sender_username.is_none() {
                peer.sender_username = peer.chat_username.clone();
            }
        }
        if peer.chat_title.is_none() {
            peer.chat_title = peer.sender_name.clone();
        }
        if peer.chat_username.is_none() {
            peer.chat_username = peer.sender_username.clone();
        }
    }
}

fn now_unix_millis() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i128
}

fn message_media_label(media: &Media) -> String {
    match media {
        Media::Photo(_) => "photo".to_string(),
        Media::Document(_) => "document".to_string(),
        Media::Sticker(_) => "sticker".to_string(),
        Media::Contact(_) => "contact".to_string(),
        Media::Poll(poll) => poll_text(poll),
        Media::Geo(_) => "location".to_string(),
        Media::Dice(dice) => format!("dice {}", dice.emoji()),
        Media::Venue(_) => "venue".to_string(),
        Media::GeoLive(_) => "live location".to_string(),
        Media::WebPage(_) => "web page".to_string(),
        _ => "media".to_string(),
    }
}

fn group_channel_name(peer: &MessagePeerMetadata) -> Option<&str> {
    if matches!(peer.chat_kind, "group" | "channel") {
        peer.chat_title.as_deref()
    } else {
        None
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

fn nonempty(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn nonempty_owned(value: Option<String>) -> Option<String> {
    value.and_then(nonempty)
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

#[cfg(test)]
mod tests {
    use super::{
        backfill_message_peer_metadata, group_channel_name, message_media_label,
        MessagePeerMetadata,
    };
    use crate::peer_cache::TelegramPeerCache;
    use grammers_client::types::{media::Photo, Media};
    use grammers_tl_types as tl;
    use serde_json::json;

    #[test]
    fn message_media_label_identifies_photos() {
        let photo = Media::Photo(Photo::from_raw(tl::types::PhotoEmpty { id: 42 }.into()));

        assert_eq!(message_media_label(&photo), "photo");
    }

    #[test]
    fn group_channel_name_skips_direct_chats() {
        let mut peer = MessagePeerMetadata {
            chat_kind: "user",
            chat_title: Some("smith john".to_string()),
            chat_username: None,
            chat_is_bot: false,
            sender_id: None,
            sender_username: None,
            sender_name: None,
            sender_is_bot: false,
        };

        assert_eq!(group_channel_name(&peer), None);

        peer.chat_kind = "group";
        peer.chat_title = Some("Puffer Internal".to_string());
        assert_eq!(group_channel_name(&peer), Some("Puffer Internal"));

        peer.chat_kind = "channel";
        peer.chat_title = Some("Puffer Updates".to_string());
        assert_eq!(group_channel_name(&peer), Some("Puffer Updates"));
    }

    #[test]
    fn direct_user_sender_name_is_backfilled_from_peer_cache() {
        let cache: TelegramPeerCache = serde_json::from_value(json!({
            "version": 1,
            "peers": [{
                "id": "6156741935",
                "numeric_id": 6156741935i64,
                "kind": "user",
                "title": "smith john",
                "usernames": ["johnsmith1847"],
                "updated_at_ms": 1
            }]
        }))
        .unwrap();
        let mut peer = MessagePeerMetadata {
            chat_kind: "user",
            chat_title: None,
            chat_username: None,
            chat_is_bot: false,
            sender_id: Some(6156741935i64),
            sender_username: None,
            sender_name: None,
            sender_is_bot: false,
        };

        backfill_message_peer_metadata(&mut peer, 6156741935i64, Some(&cache), false);

        assert_eq!(peer.sender_name.as_deref(), Some("smith john"));
        assert_eq!(peer.sender_username.as_deref(), Some("johnsmith1847"));
        assert_eq!(peer.chat_title.as_deref(), Some("smith john"));
        assert_eq!(peer.chat_username.as_deref(), Some("johnsmith1847"));
    }
}
