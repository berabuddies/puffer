//! Telegram reply metadata helpers.

use grammers_tl_types::enums as tl_enums;
use serde_json::{json, Map, Value};

const MAX_REPLY_LABEL_CHARS: usize = 160;

/// Converts a Telegram reply header into stable JSON metadata.
pub(crate) fn reply_header_payload(header: Option<tl_enums::MessageReplyHeader>) -> Option<Value> {
    match header? {
        tl_enums::MessageReplyHeader::Header(header) => Some(message_reply_header_payload(header)),
        tl_enums::MessageReplyHeader::MessageReplyStoryHeader(header) => Some(json!({
            "kind": "story",
            "peer": peer_payload(&header.peer),
            "story_id": header.story_id,
        })),
    }
}

/// Returns a compact human-readable label for a reply payload.
pub(crate) fn reply_to_label(reply_to: &Value) -> Option<String> {
    match value_string(reply_to, "kind")?.as_str() {
        "message" => message_reply_label(reply_to),
        "story" => {
            let story_id = reply_to.get("story_id").and_then(Value::as_i64);
            Some(match story_id {
                Some(id) => format!("reply to story #{id}"),
                None => "reply to story".to_string(),
            })
        }
        _ => None,
    }
}

fn message_reply_header_payload(header: grammers_tl_types::types::MessageReplyHeader) -> Value {
    let mut object = Map::new();
    object.insert("kind".to_string(), json!("message"));
    object.insert("message_id".to_string(), json!(header.reply_to_msg_id));
    object.insert(
        "thread_top_message_id".to_string(),
        json!(header.reply_to_top_id),
    );
    object.insert("is_scheduled".to_string(), json!(header.reply_to_scheduled));
    object.insert("is_forum_topic".to_string(), json!(header.forum_topic));
    object.insert("has_quote".to_string(), json!(header.quote));
    object.insert("quote_text".to_string(), json!(header.quote_text));
    object.insert("quote_offset".to_string(), json!(header.quote_offset));
    object.insert(
        "quote_entity_count".to_string(),
        json!(header.quote_entities.as_ref().map(Vec::len)),
    );
    object.insert(
        "peer".to_string(),
        header
            .reply_to_peer_id
            .as_ref()
            .map(peer_payload)
            .unwrap_or(Value::Null),
    );
    object.insert(
        "reply_from".to_string(),
        header
            .reply_from
            .as_ref()
            .map(forward_header_payload)
            .unwrap_or(Value::Null),
    );
    object.insert(
        "reply_media".to_string(),
        header
            .reply_media
            .as_ref()
            .map(message_media_payload)
            .unwrap_or(Value::Null),
    );
    Value::Object(object)
}

fn message_reply_label(reply_to: &Value) -> Option<String> {
    let message_id = reply_to
        .get("message_id")
        .and_then(Value::as_i64)
        .map(|id| format!("#{id}"));
    let label = match message_id {
        Some(id) => format!("reply to {id}"),
        None => "reply".to_string(),
    };
    let snippet = reply_snippet(reply_to);
    Some(match snippet {
        Some(snippet) => format!("{label}: {snippet}"),
        None => label,
    })
}

fn reply_snippet(reply_to: &Value) -> Option<String> {
    value_string(reply_to, "quote_text")
        .filter(|value| !value.is_empty())
        .or_else(|| {
            reply_to
                .get("resolved_message")
                .and_then(|resolved| value_string(resolved, "text"))
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            reply_to
                .get("resolved_message")
                .and_then(|resolved| value_string(resolved, "media"))
                .filter(|value| !value.is_empty())
        })
        .map(|value| truncate_label(&one_line(&value)))
        .filter(|value| !value.is_empty())
}

fn forward_header_payload(header: &tl_enums::MessageFwdHeader) -> Value {
    match header {
        tl_enums::MessageFwdHeader::Header(header) => json!({
            "imported": header.imported,
            "saved_out": header.saved_out,
            "from_peer": header.from_id.as_ref().map(peer_payload),
            "from_name": header.from_name,
            "date": header.date,
            "channel_post": header.channel_post,
            "post_author": header.post_author,
            "saved_from_peer": header.saved_from_peer.as_ref().map(peer_payload),
            "saved_from_message_id": header.saved_from_msg_id,
            "saved_from_id": header.saved_from_id.as_ref().map(peer_payload),
            "saved_from_name": header.saved_from_name,
            "saved_date": header.saved_date,
            "psa_type": header.psa_type,
        }),
    }
}

fn peer_payload(peer: &tl_enums::Peer) -> Value {
    match peer {
        tl_enums::Peer::User(peer) => json!({
            "kind": "user",
            "id": peer.user_id.to_string(),
            "numeric_id": peer.user_id,
        }),
        tl_enums::Peer::Chat(peer) => json!({
            "kind": "group",
            "id": peer.chat_id.to_string(),
            "numeric_id": peer.chat_id,
        }),
        tl_enums::Peer::Channel(peer) => json!({
            "kind": "channel",
            "id": peer.channel_id.to_string(),
            "numeric_id": peer.channel_id,
        }),
    }
}

fn message_media_payload(media: &tl_enums::MessageMedia) -> Value {
    json!({
        "kind": message_media_kind(media),
    })
}

fn message_media_kind(media: &tl_enums::MessageMedia) -> &'static str {
    match media {
        tl_enums::MessageMedia::Empty => "empty",
        tl_enums::MessageMedia::Photo(_) => "photo",
        tl_enums::MessageMedia::Geo(_) => "location",
        tl_enums::MessageMedia::Contact(_) => "contact",
        tl_enums::MessageMedia::Unsupported => "unsupported",
        tl_enums::MessageMedia::Document(_) => "document",
        tl_enums::MessageMedia::WebPage(_) => "web_page",
        tl_enums::MessageMedia::Venue(_) => "venue",
        tl_enums::MessageMedia::Game(_) => "game",
        tl_enums::MessageMedia::Invoice(_) => "invoice",
        tl_enums::MessageMedia::GeoLive(_) => "live_location",
        tl_enums::MessageMedia::Poll(_) => "poll",
        tl_enums::MessageMedia::Dice(_) => "dice",
        tl_enums::MessageMedia::Story(_) => "story",
        tl_enums::MessageMedia::Giveaway(_) => "giveaway",
        tl_enums::MessageMedia::GiveawayResults(_) => "giveaway_results",
        tl_enums::MessageMedia::PaidMedia(_) => "paid_media",
    }
}

fn value_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn one_line(value: &str) -> String {
    value.replace('\r', "\\r").replace('\n', "\\n")
}

fn truncate_label(value: &str) -> String {
    if value.chars().count() <= MAX_REPLY_LABEL_CHARS {
        return value.to_string();
    }
    let end = value
        .char_indices()
        .nth(MAX_REPLY_LABEL_CHARS)
        .map(|(idx, _)| idx)
        .unwrap_or(value.len());
    format!("{}...", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::reply_to_label;
    use serde_json::json;

    #[test]
    fn reply_label_prefers_quote_text() {
        let reply = json!({
            "kind": "message",
            "message_id": 42,
            "quote_text": "hello\nthere",
            "resolved_message": {
                "text": "fallback"
            }
        });

        assert_eq!(
            reply_to_label(&reply).as_deref(),
            Some("reply to #42: hello\\nthere")
        );
    }

    #[test]
    fn reply_label_uses_resolved_message_when_quote_is_absent() {
        let reply = json!({
            "kind": "message",
            "message_id": 42,
            "quote_text": null,
            "resolved_message": {
                "text": "resolved"
            }
        });

        assert_eq!(
            reply_to_label(&reply).as_deref(),
            Some("reply to #42: resolved")
        );
    }
}
