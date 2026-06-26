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
        SubscriberCommand::TelegramListMessages {
            peer,
            before_id,
            sender,
            ..
        } => {
            payload_str_eq(&envelope.event.payload, "peer", peer)
                && payload_option_i32_eq(&envelope.event.payload, "before_id", *before_id)
                && payload_optional_str_eq(
                    &envelope.event.payload,
                    "sender_filter",
                    sender.as_deref(),
                )
        }
        SubscriberCommand::Custom { op, args } if op == "telegram_act" => args
            .get("action")
            .and_then(Value::as_str)
            .map(|action| payload_str_eq(&envelope.event.payload, "action", action))
            .unwrap_or(true),
        SubscriberCommand::Custom { op, args }
            if op == "email_act" || op == "gmail_browser_act" || op == "gcal_browser_act" =>
        {
            args.get("action")
                .and_then(Value::as_str)
                .map(|action| payload_str_eq(&envelope.event.payload, "action", action))
                .unwrap_or(true)
        }
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

fn payload_optional_str_eq(payload: &Value, key: &str, expected: Option<&str>) -> bool {
    match expected {
        Some(expected) => payload_str_eq(payload, key, expected),
        None => payload
            .get(key)
            .map(|value| value.is_null() || value.as_str() == Some(""))
            .unwrap_or(true),
    }
}

fn payload_option_usize_eq(payload: &Value, key: &str, expected: Option<usize>) -> bool {
    match expected {
        Some(expected) => payload.get(key).and_then(Value::as_u64) == Some(expected as u64),
        None => true,
    }
}

fn payload_option_i32_eq(payload: &Value, key: &str, expected: Option<i32>) -> bool {
    match expected {
        Some(expected) => payload.get(key).and_then(Value::as_i64) == Some(expected as i64),
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
    use puffer_subscriber_runtime::{Event, SendAuthorization, TelegramPeerKind};
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
            authorization: test_send_authorization("@alice", "hi"),
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

    fn test_send_authorization(peer: &str, text: &str) -> SendAuthorization {
        SendAuthorization {
            source: "test".into(),
            draft_id: "draft-test".into(),
            version: 1,
            action: "send_message".into(),
            recipient_stable_id: peer.into(),
            content_hash: format!("test:{text}"),
            client_request_id: "request-test".into(),
        }
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

    #[test]
    fn message_list_terminal_events_match_peer_and_cursor() {
        let command = SubscriberCommand::TelegramListMessages {
            peer: "477843728".into(),
            limit: Some(20),
            before_id: Some(325),
            sender: Some("Tony".into()),
            scan_limit: Some(1_000),
            succinct: true,
        };

        assert!(command_matches_terminal_event(
            &command,
            &envelope(
                "message_list",
                json!({
                    "peer":"477843728",
                    "before_id":325,
                    "sender_filter":"Tony",
                    "scan_limit":500
                })
            )
        ));
        assert!(!command_matches_terminal_event(
            &command,
            &envelope(
                "message_list",
                json!({
                    "peer":"477843728",
                    "before_id":326,
                    "sender_filter":"Tony",
                    "scan_limit":200
                })
            )
        ));
        assert!(!command_matches_terminal_event(
            &command,
            &envelope(
                "message_list",
                json!({
                    "peer":"477843728",
                    "before_id":325,
                    "sender_filter":"Karen",
                    "scan_limit":200
                })
            )
        ));
    }

    #[test]
    fn unfiltered_message_list_does_not_match_sender_filtered_event() {
        let command = SubscriberCommand::TelegramListMessages {
            peer: "477843728".into(),
            limit: Some(20),
            before_id: Some(325),
            sender: None,
            scan_limit: None,
            succinct: true,
        };

        assert!(command_matches_terminal_event(
            &command,
            &envelope(
                "message_list",
                json!({"peer":"477843728","before_id":325,"sender_filter":null})
            )
        ));
        assert!(!command_matches_terminal_event(
            &command,
            &envelope(
                "message_list",
                json!({"peer":"477843728","before_id":325,"sender_filter":"Tony"})
            )
        ));
    }

    #[test]
    fn custom_email_actions_match_action_terminal_events() {
        let command = SubscriberCommand::Custom {
            op: "email_act".into(),
            args: json!({"action":"list_emails"}),
        };

        assert!(command_matches_terminal_event(
            &command,
            &envelope(
                "email_action_complete",
                json!({"action":"list_emails","summary":"ok"})
            )
        ));
        assert!(!command_matches_terminal_event(
            &command,
            &envelope(
                "email_action_complete",
                json!({"action":"delete","summary":"ok"})
            )
        ));
    }
}
