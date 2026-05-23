//! The event envelope subscribers emit on stdout.
//!
//! Subscribers write one line of JSON per event, following the [`Event`]
//! schema. The runtime reads these lines, tags them with a fresh envelope id
//! and the publishing subscriber id, and broadcasts on the event bus.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The raw payload a subscriber emits. All fields except `topic` are optional;
/// topic identifies the logical source (e.g. `"telegram-user"`,
/// `"rss-hn"`), usually the subscriber's manifest `topic`.
///
/// `kind` is a short, subscriber-specific tag (`"message"`, `"edit"`,
/// `"channel_post"`, …). Consumers match on `(topic, kind)` plus fields in
/// `payload`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Event {
    /// Logical source topic for routing. Must match a subscriber's
    /// manifest `topic`, or be prefixed with it.
    pub topic: String,
    /// Optional event kind (free-form per subscriber). Defaults to
    /// `"message"` when the subscriber omits it.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// True for command replies and lifecycle notices that should not be
    /// treated as workflow trigger events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub control: bool,
    /// Best-effort stable identity for de-duplication across restarts
    /// (e.g. Telegram `message_id@chat_id`). Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedup_key: Option<String>,
    /// Plain text content used for regex prefiltering and LLM classification.
    /// Subscribers populate this with the message body, email subject+body,
    /// RSS item title+summary, etc.
    #[serde(default)]
    pub text: String,
    /// Arbitrary structured fields the subscriber wants to attach
    /// (sender handle, chat title, urls, …). Actions read from this.
    #[serde(default)]
    pub payload: Value,
}

fn default_kind() -> String {
    "message".to_string()
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The envelope broadcast on the in-process bus. Wraps [`Event`] with
/// runtime-assigned identity.
#[derive(Debug, Clone)]
pub struct EventEnvelope {
    /// Runtime-assigned envelope id (v4 UUID); distinct from `dedup_key`.
    pub envelope_id: String,
    /// Subscriber manifest id that produced this event.
    pub subscriber_id: String,
    /// Milliseconds since UNIX epoch when the runtime received the line.
    pub received_at_ms: i128,
    /// The event payload.
    pub event: Event,
}
