use super::daemon_contacts_telegram::{
    reply_index, telegram_contact_id, telegram_contact_name, TelegramDiagMessage,
};
use super::*;
use puffer_config::ConfigPaths;
use serde_json::{json, Value};
use std::collections::HashMap;

#[test]
fn save_contact_normalizes_ids() {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(temp.path());
    let result = handle_contacts_save(
        &paths,
        &json!({
            "name": "Alice",
            "description": "Project collaborator",
            "contact_ids": ["Google@Alice@Example.COM", "telegram@@alice"]
        }),
    )
    .unwrap();

    let contact = &result["contacts"].as_array().unwrap()[0];
    assert_eq!(contact["contact_ids"][0], "google@alice@example.com");
    assert_eq!(contact["contact_ids"][1], "telegram@alice");
}

#[test]
fn telegram_candidates_require_private_usernames_and_ignore_bots_groups() {
    assert_eq!(
        telegram_contact_id(
            &json!({
                "chat_kind": "user",
                "chat_username": "Alice"
            }),
            "user"
        ),
        Some("telegram@alice".to_string())
    );
    assert!(telegram_contact_id(
        &json!({
            "chat_kind": "user",
            "chat_id": 5229190700_i64,
            "chat_title": "Direct Numeric"
        }),
        "user"
    )
    .is_none());
    assert!(telegram_contact_id(
        &json!({
            "chat_kind": "group",
            "chat_id": -1,
            "sender_id": 42,
            "sender_is_bot": true
        }),
        "group"
    )
    .is_none());
    assert!(telegram_contact_id(
        &json!({
            "chat_kind": "user",
            "chat_username": "alertbot",
            "chat_is_bot": true
        }),
        "user"
    )
    .is_none());
    assert!(telegram_contact_id(
        &json!({
            "chat_kind": "group",
            "sender_username": "deploy"
        }),
        "group"
    )
    .is_none());
    assert!(telegram_contact_id(
        &json!({
            "chat_kind": "group",
            "chat_id": -1,
            "sender_id": 42,
            "sender_name": "Numeric Only"
        }),
        "group"
    )
    .is_none());
    assert!(telegram_contact_id(
        &json!({
            "chat_kind": "channel",
            "chat_id": -100,
            "sender_username": "news"
        }),
        "channel"
    )
    .is_none());
    assert!(telegram_contact_id(
        &json!({
            "chat_kind": "group",
            "sender_username": "deploybot",
            "sender_is_bot": true
        }),
        "group"
    )
    .is_none());
}

#[test]
fn telegram_personal_reply_requires_next_same_chat_message() {
    let first = read_test_message(json!({
        "stage": "emitted",
        "chat_kind": "user",
        "chat_id": 5,
        "chat_username": "alice",
        "chat_title": "Alice",
        "message_id": 1,
        "date_ms": 1_700_000_000_000_i64,
        "text_prefix": "first question"
    }));
    let second = read_test_message(json!({
        "stage": "emitted",
        "chat_kind": "user",
        "chat_id": 5,
        "chat_username": "alice",
        "chat_title": "Alice",
        "message_id": 2,
        "date_ms": 1_700_000_060_000_i64,
        "text_prefix": "second question"
    }));
    let outgoing = read_test_message(json!({
        "stage": "emitted",
        "chat_kind": "user",
        "chat_id": 5,
        "chat_username": "alice",
        "chat_title": "Alice",
        "message_id": 3,
        "date_ms": 1_700_000_120_000_i64,
        "is_outgoing": true,
        "text_prefix": "reply to the second question"
    }));
    let messages = vec![first, second, outgoing];
    let by_id = messages
        .iter()
        .enumerate()
        .map(|(index, message)| (message.message_id, index))
        .collect::<HashMap<_, _>>();

    assert_eq!(reply_index(&messages, &by_id, 0, &messages[0]), None);
    assert_eq!(reply_index(&messages, &by_id, 1, &messages[1]), Some(2));
}

#[test]
fn inference_sampling_keeps_top_contacts_per_prefix() {
    let mut candidates = Vec::new();
    for index in 0..35 {
        candidates.push(Candidate {
            id: format!("telegram@user{index}"),
            name: None,
            avatar: None,
            score: 100.0 - index as f64,
            context: Vec::new(),
        });
    }
    for index in 0..35 {
        candidates.push(Candidate {
            id: format!("google@user{index}@example.com"),
            name: None,
            avatar: None,
            score: 10.0 - index as f64,
            context: Vec::new(),
        });
    }

    let sampled = sample_inference_candidates(candidates, 30);
    let telegram = sampled
        .iter()
        .filter(|candidate| candidate.id.starts_with("telegram@"))
        .count();
    let google = sampled
        .iter()
        .filter(|candidate| candidate.id.starts_with("google@"))
        .count();

    assert_eq!(telegram, 30);
    assert_eq!(google, 30);
    assert!(sampled
        .iter()
        .any(|candidate| candidate.id == "google@user0@example.com"));
    assert!(!sampled
        .iter()
        .any(|candidate| candidate.id == "google@user30@example.com"));
}

fn read_test_message(payload: Value) -> TelegramDiagMessage {
    let chat_kind = payload
        .get("chat_kind")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    TelegramDiagMessage {
        contact_id: telegram_contact_id(&payload, &chat_kind).unwrap(),
        name: telegram_contact_name(&payload, &chat_kind),
        chat_id: payload.get("chat_id").and_then(Value::as_i64).unwrap_or(0),
        chat_kind,
        sender_username: payload
            .get("sender_username")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        is_outgoing: payload
            .get("is_outgoing")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        date_ms: payload.get("date_ms").and_then(Value::as_i64).unwrap() as i128,
        message_id: payload.get("message_id").and_then(Value::as_i64),
        reply_to_message_id: payload
            .pointer("/reply_to/message_id")
            .and_then(Value::as_i64),
        text: payload
            .get("text_prefix")
            .and_then(Value::as_str)
            .unwrap()
            .to_string(),
        payload,
    }
}
