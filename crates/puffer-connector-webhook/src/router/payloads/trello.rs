use axum::http::HeaderMap;
use puffer_connector_core::InboundMessage;
use serde_json::Value;

use super::super::{header_value, number_or_string, pointer_string, snippet, string_field};

/// Converts a Trello webhook payload into an inbound Puffer message.
pub(super) fn trello_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    if !trello_payload_shape(headers, payload) {
        return None;
    }

    let action = payload.get("action")?;
    let action_type = string_field(action, "type").unwrap_or("action");
    let actor = trello_actor(action);
    let board = trello_board(payload);
    let subject = trello_subject(payload);
    let conversation_id = trello_conversation_id(payload, subject.as_ref(), action_type);
    let text = trello_message(action_type, actor, board.as_ref(), subject.as_ref(), action);

    Some(InboundMessage {
        conversation_id,
        user_id: Some(actor.to_string()),
        text,
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    })
}

fn trello_payload_shape(headers: &HeaderMap, payload: &Value) -> bool {
    header_value(headers, "x-trello-webhook").is_some()
        || payload.get("action").is_some_and(|action| {
            string_field(action, "type").is_some()
                && (action.get("data").is_some() || payload.get("model").is_some())
        })
}

#[derive(Clone)]
struct TrelloRef {
    kind: &'static str,
    conversation_kind: &'static str,
    id: Option<String>,
    name: Option<String>,
    url: Option<String>,
}

fn trello_subject(payload: &Value) -> Option<TrelloRef> {
    let data = payload.pointer("/action/data");
    data.and_then(|data| trello_ref(data.get("card"), "card", "card"))
        .or_else(|| data.and_then(|data| trello_ref(data.get("list"), "list", "list")))
        .or_else(|| data.and_then(|data| trello_ref(data.get("board"), "board", "board")))
        .or_else(|| {
            data.and_then(|data| {
                trello_ref(data.get("organization"), "organization", "organization")
            })
        })
        .or_else(|| trello_ref(payload.get("model"), "model", "model"))
}

fn trello_board(payload: &Value) -> Option<TrelloRef> {
    payload
        .pointer("/action/data/board")
        .and_then(|value| trello_ref(Some(value), "board", "board"))
}

fn trello_ref(
    value: Option<&Value>,
    kind: &'static str,
    conversation_kind: &'static str,
) -> Option<TrelloRef> {
    let value = value?;
    Some(TrelloRef {
        kind,
        conversation_kind,
        id: string_field(value, "id")
            .or_else(|| string_field(value, "idShort"))
            .map(str::to_string)
            .or_else(|| value.get("idShort").and_then(number_or_string)),
        name: string_field(value, "name").map(str::to_string),
        url: string_field(value, "url")
            .or_else(|| string_field(value, "shortUrl"))
            .map(str::to_string),
    })
}

fn trello_conversation_id(
    payload: &Value,
    subject: Option<&TrelloRef>,
    action_type: &str,
) -> String {
    if let Some(subject) = subject {
        if let Some(id) = &subject.id {
            return format!(
                "trello:{}:{}",
                subject.conversation_kind,
                normalize_trello_part(id)
            );
        }
    }
    let fallback = payload
        .pointer("/action/id")
        .and_then(Value::as_str)
        .or_else(|| string_field(payload, "webhookId"))
        .unwrap_or(action_type);
    format!("trello:event:{}", normalize_trello_part(fallback))
}

fn trello_message(
    action_type: &str,
    actor: &str,
    board: Option<&TrelloRef>,
    subject: Option<&TrelloRef>,
    action: &Value,
) -> String {
    let mut lines = vec![format!("Trello {action_type}")];
    lines.push(format!("Actor: {actor}"));
    if let Some(board) = board {
        lines.push(format!("Board: {}", trello_label(board)));
    }
    if let Some(subject) = subject {
        lines.push(format!(
            "Subject: {} {}",
            subject.kind,
            trello_label(subject)
        ));
        if let Some(url) = &subject.url {
            lines.push(format!("URL: {url}"));
        }
    }
    if let Some(list) = action
        .pointer("/data/list")
        .and_then(|value| trello_ref(Some(value), "list", "list"))
    {
        lines.push(format!("List: {}", trello_label(&list)));
    }
    if let Some(date) = string_field(action, "date") {
        lines.push(format!("Date: {date}"));
    }
    if let Some(text) = pointer_string(action, "/data/text").map(snippet) {
        lines.push(String::new());
        lines.push(text);
    }
    lines.join("\n")
}

fn trello_actor(action: &Value) -> &str {
    pointer_string(action, "/memberCreator/fullName")
        .or_else(|| pointer_string(action, "/memberCreator/username"))
        .or_else(|| pointer_string(action, "/memberCreator/id"))
        .unwrap_or("trello")
}

fn trello_label(value: &TrelloRef) -> String {
    match (&value.id, &value.name) {
        (Some(id), Some(name)) => format!("{} {}", normalize_trello_part(id), snippet(name)),
        (Some(id), None) => normalize_trello_part(id),
        (None, Some(name)) => snippet(name),
        (None, None) => "unknown".to_string(),
    }
}

fn normalize_trello_part(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace(':', "_")
        .replace(' ', "_")
        .replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trello_card_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("x-trello-webhook", "signature".parse().unwrap());
        let payload = serde_json::json!({
            "action": {
                "id": "action-1",
                "type": "updateCard",
                "date": "2026-05-25T10:30:00.000Z",
                "memberCreator": {
                    "id": "member-1",
                    "username": "tony",
                    "fullName": "Tony"
                },
                "data": {
                    "board": {
                        "id": "board-1",
                        "name": "Puffer"
                    },
                    "list": {
                        "id": "list-1",
                        "name": "Doing"
                    },
                    "card": {
                        "id": "card-1",
                        "idShort": 42,
                        "name": "Improve workflow UX",
                        "shortUrl": "https://trello.com/c/abc123"
                    }
                }
            },
            "model": {
                "id": "board-1",
                "name": "Puffer"
            }
        });

        let inbound = trello_inbound(&headers, &payload).expect("trello inbound");

        assert_eq!(inbound.conversation_id, "trello:card:card_1");
        assert_eq!(inbound.user_id.as_deref(), Some("Tony"));
        assert!(inbound.text.contains("Trello updateCard"));
        assert!(inbound.text.contains("Board: board_1 Puffer"));
        assert!(inbound
            .text
            .contains("Subject: card card_1 Improve workflow UX"));
        assert!(inbound.text.contains("URL: https://trello.com/c/abc123"));
        assert!(inbound.text.contains("List: list_1 Doing"));
    }

    #[test]
    fn trello_shape_requires_action_data_or_header() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "action": {
                "type": "updateCard"
            }
        });

        assert!(trello_inbound(&headers, &payload).is_none());
    }

    #[test]
    fn trello_model_payload_uses_model_as_fallback_subject() {
        let headers = HeaderMap::new();
        let payload = serde_json::json!({
            "action": {
                "id": "action-2",
                "type": "updateBoard",
                "memberCreator": {
                    "id": "member-2",
                    "username": "ava"
                }
            },
            "model": {
                "id": "board-2",
                "name": "Roadmap"
            }
        });

        let inbound = trello_inbound(&headers, &payload).expect("trello inbound");

        assert_eq!(inbound.conversation_id, "trello:model:board_2");
        assert!(inbound.text.contains("Subject: model board_2 Roadmap"));
        assert!(!inbound.text.contains("Board:"));
    }
}
