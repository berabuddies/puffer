//! Matching helpers for subscriber command terminal events.

use puffer_subscriber_runtime::{EventEnvelope, SubscriberCommand};
use serde_json::Value;

pub(crate) fn command_matches_terminal_event(
    command: &SubscriberCommand,
    envelope: &EventEnvelope,
) -> bool {
    match command {
        SubscriberCommand::SendMessage { peer, .. } => {
            payload_str_eq(&envelope.event.payload, "peer", peer)
        }
        SubscriberCommand::TelegramListPeers {
            query,
            peer_kind,
            limit,
        } => {
            payload_option_str_eq(&envelope.event.payload, "query", query.as_deref())
                && payload_option_str_eq(
                    &envelope.event.payload,
                    "peer_kind",
                    peer_kind.map(telegram_peer_kind_label),
                )
                && payload_option_usize_eq(&envelope.event.payload, "limit", *limit)
        }
        SubscriberCommand::TelegramSearchMessages { peer, query, .. } => {
            payload_str_eq(&envelope.event.payload, "peer", peer)
                && payload_str_eq(&envelope.event.payload, "query", query)
        }
        SubscriberCommand::Custom { op, args } if op == "telegram_act" => args
            .get("action")
            .and_then(Value::as_str)
            .map(|action| payload_str_eq(&envelope.event.payload, "action", action))
            .unwrap_or(true),
        _ => true,
    }
}

fn payload_str_eq(payload: &Value, key: &str, expected: &str) -> bool {
    payload.get(key).and_then(Value::as_str) == Some(expected)
}

fn payload_option_str_eq(payload: &Value, key: &str, expected: Option<&str>) -> bool {
    match expected {
        Some(expected) => payload_str_eq(payload, key, expected),
        None => true,
    }
}

fn payload_option_usize_eq(payload: &Value, key: &str, expected: Option<usize>) -> bool {
    match expected {
        Some(expected) => payload.get(key).and_then(Value::as_u64) == Some(expected as u64),
        None => true,
    }
}

fn telegram_peer_kind_label(kind: puffer_subscriber_runtime::TelegramPeerKind) -> &'static str {
    match kind {
        puffer_subscriber_runtime::TelegramPeerKind::User => "user",
        puffer_subscriber_runtime::TelegramPeerKind::Group => "group",
        puffer_subscriber_runtime::TelegramPeerKind::Channel => "channel",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_subscriber_runtime::{Event, TelegramPeerKind};
    use serde_json::json;

    fn envelope(kind: &str, payload: Value) -> EventEnvelope {
        EventEnvelope {
            envelope_id: "env".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: kind.into(),
                control: true,
                dedup_key: None,
                text: String::new(),
                payload,
            },
        }
    }

    #[test]
    fn send_message_terminal_events_match_peer() {
        let command = SubscriberCommand::SendMessage {
            peer: "@alice".into(),
            text: "hi".into(),
            reply_to: None,
            media: Vec::new(),
        };

        assert!(command_matches_terminal_event(
            &command,
            &envelope("send_complete", json!({"peer":"@alice"}))
        ));
        assert!(!command_matches_terminal_event(
            &command,
            &envelope("send_complete", json!({"peer":"@bob"}))
        ));
    }

    #[test]
    fn peer_list_terminal_events_match_query_shape() {
        let command = SubscriberCommand::TelegramListPeers {
            query: Some("karen".into()),
            peer_kind: Some(TelegramPeerKind::User),
            limit: Some(10),
        };

        assert!(command_matches_terminal_event(
            &command,
            &envelope(
                "peer_list",
                json!({"query":"karen","peer_kind":"user","limit":10})
            )
        ));
        assert!(!command_matches_terminal_event(
            &command,
            &envelope(
                "peer_list",
                json!({"query":"tony","peer_kind":"user","limit":10})
            )
        ));
    }
}
