use super::daemon_contacts_telegram::{
    reply_index, score_telegram_window, telegram_contact_id, telegram_contact_name,
    TelegramDiagMessage,
};
use super::*;
use grammers_session::Session;
use puffer_config::ConfigPaths;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

#[test]
fn telegram_candidates_require_private_users_and_ignore_bots_groups() {
    assert_eq!(
        telegram_contact_id(
            &json!({
                "chat_kind": "user",
                "chat_id": 5,
                "chat_username": "Alice"
            }),
            "user"
        ),
        Some("telegram-user-id@5".to_string())
    );
    assert_eq!(
        telegram_contact_id(
            &json!({
                "chat_kind": "user",
                "chat_id": 5229190700_i64,
                "chat_title": "Direct Numeric"
            }),
            "user"
        ),
        Some("telegram-user-id@5229190700".to_string())
    );
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
    assert!(telegram_contact_id(
        &json!({
            "chat_kind": "user",
            "chat_username": "alertbot"
        }),
        "user"
    )
    .is_none());
    assert!(telegram_contact_id(
        &json!({
            "chat_kind": "group",
            "sender_username": "deploybot"
        }),
        "group"
    )
    .is_none());
}

#[test]
fn telegram_candidates_ignore_self_chat_messages() {
    let self_message = read_test_message(json!({
        "stage": "emitted",
        "chat_kind": "user",
        "chat_id": 42,
        "chat_username": "me",
        "chat_title": "Me",
        "sender_id": 42,
        "sender_username": "me",
        "sender_name": "Me",
        "message_id": 1,
        "date_ms": 1_700_000_000_000_i64,
        "is_self_contact": true,
        "text_prefix": "private note to myself"
    }));

    let ranked = score_telegram_window(&[self_message]);

    assert!(ranked.is_empty());
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
fn telegram_candidates_prefer_full_names() {
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
        "chat_title": "Alice Smith",
        "message_id": 2,
        "date_ms": 1_700_000_060_000_i64,
        "text_prefix": "second question"
    }));

    let ranked = score_telegram_window(&[first, second]);

    assert_eq!(
        ranked
            .get("telegram-user-id@5")
            .and_then(|candidate| candidate.name.as_deref()),
        Some("Alice Smith")
    );
}

#[test]
fn telegram_context_marks_recent_messages_and_interaction_replies() {
    let mut messages = Vec::new();
    let base_ms = 1_700_000_000_000_i64;
    for index in 0..12 {
        let incoming_id = (index * 2 + 1) as i64;
        let reply_id = (index * 2 + 2) as i64;
        messages.push(read_test_message(json!({
            "stage": "emitted",
            "chat_kind": "user",
            "chat_id": 5,
            "chat_username": "alice",
            "chat_title": "Alice Smith",
            "message_id": incoming_id,
            "date_ms": base_ms + (index as i64 * 120_000),
            "text_prefix": format!("question {index} about the launch plan")
        })));
        messages.push(read_test_message(json!({
            "stage": "emitted",
            "chat_kind": "user",
            "chat_id": 5,
            "chat_username": "alice",
            "chat_title": "Alice Smith",
            "message_id": reply_id,
            "date_ms": base_ms + (index as i64 * 120_000) + 60_000,
            "is_outgoing": true,
            "text_prefix": format!("answer {index} about the launch plan")
        })));
    }

    let ranked = score_telegram_window(&messages);
    let context = &ranked.get("telegram-user-id@5").unwrap().context;

    assert_eq!(context.len(), 40);
    assert_eq!(
        context
            .iter()
            .filter(|item| item.kind == "telegram_recent_message")
            .count(),
        20
    );
    assert_eq!(
        context
            .iter()
            .filter(|item| item.kind == "telegram_interaction_contact_message")
            .count(),
        10
    );
    assert_eq!(
        context
            .iter()
            .filter(|item| item.kind == "telegram_interaction_user_reply")
            .count(),
        10
    );
    assert!(context
        .iter()
        .all(|item| item.text.contains("[dest: Alice Smith]")));
    assert!(context.iter().any(|item| {
        item.kind == "telegram_interaction_user_reply"
            && item.text.contains("[from: user]")
            && item.text.contains("answer 11")
    }));
    assert_eq!(
        context[0].payload.pointer("/destination/label"),
        Some(&json!("Alice Smith"))
    );
    assert_eq!(
        context[0].payload.pointer("/context_section"),
        Some(&json!("recent"))
    );
}

#[test]
fn telegram_context_keeps_twenty_recent_messages_when_available() {
    let mut messages = Vec::new();
    let base_ms = 1_700_000_000_000_i64;
    for index in 0..25 {
        messages.push(read_test_message(json!({
            "stage": "emitted",
            "chat_kind": "user",
            "chat_id": 5,
            "chat_username": "alice",
            "chat_title": "Alice Smith",
            "message_id": index + 1,
            "date_ms": base_ms + (index as i64 * 60_000),
            "is_outgoing": true,
            "text_prefix": format!("recent context message {index} about the launch plan")
        })));
    }

    let ranked = score_telegram_window(&messages);
    let context = &ranked.get("telegram-user-id@5").unwrap().context;

    assert_eq!(
        context
            .iter()
            .filter(|item| item.kind == "telegram_recent_message")
            .count(),
        MIN_CONTEXT_MESSAGES_PER_CANDIDATE
    );
    assert!(context
        .iter()
        .any(|item| item.text.contains("recent context message 24")));
    assert!(!context
        .iter()
        .any(|item| item.text.contains("recent context message 4")));
}

#[test]
fn inference_sampling_keeps_top_contacts_per_prefix() {
    let mut candidates = Vec::new();
    for index in 0..35 {
        candidates.push(Candidate {
            id: format!("telegram@user{index}"),
            name: None,
            public_username: None,
            avatar: None,
            score: 100.0 - index as f64,
            last_message_at_ms: None,
            context: Vec::new(),
        });
    }
    for index in 0..35 {
        candidates.push(Candidate {
            id: format!("google@user{index}@example.com"),
            name: None,
            public_username: None,
            avatar: None,
            score: 10.0 - index as f64,
            last_message_at_ms: None,
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

#[test]
fn inference_sampling_excludes_bot_candidates() {
    let candidates = vec![
        Candidate {
            id: "telegram@alertbot".to_string(),
            name: Some("Alert Bot".to_string()),
            public_username: None,
            avatar: None,
            score: 100.0,
            last_message_at_ms: None,
            context: Vec::new(),
        },
        Candidate {
            id: "google@support-bot@example.com".to_string(),
            name: Some("Support Bot".to_string()),
            public_username: None,
            avatar: None,
            score: 90.0,
            last_message_at_ms: None,
            context: Vec::new(),
        },
        Candidate {
            id: "telegram@alice".to_string(),
            name: Some("Alice".to_string()),
            public_username: None,
            avatar: None,
            score: 80.0,
            last_message_at_ms: None,
            context: Vec::new(),
        },
    ];

    let sampled = sample_inference_candidates(candidates, 30);

    assert_eq!(sampled.len(), 1);
    assert_eq!(sampled[0].id, "telegram@alice");
}

#[test]
fn inference_context_avoids_live_connector_commands() {
    let options = CandidateContextOptions::inference();

    assert!(!options.uses_connector_commands());
    assert_eq!(
        options.limit_for_id("telegram@alice"),
        INFERENCE_CONTEXT_LIMIT
    );
    assert_eq!(
        options.limit_for_id("google@alice@example.com"),
        INFERENCE_CONTEXT_LIMIT
    );
    assert_eq!(options.payload(&json!({"message": "hello"})), Value::Null);
}

#[test]
fn contact_list_and_search_reject_malformed_params() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());

    let list_error = handle_contacts_list(&paths, &json!({ "limit": "many" })).unwrap_err();
    let search_error = handle_contacts_search(&paths, &json!({ "query": 42 })).unwrap_err();

    assert!(list_error
        .to_string()
        .contains("invalid contact list params"));
    assert!(search_error
        .to_string()
        .contains("invalid contact search params"));
}

#[test]
fn telegram_direct_payload_name_prefers_chat_title() {
    assert_eq!(
        name_from_payload(&json!({
            "chat_kind": "user",
            "chat_title": "REAL Yuki",
            "sender_name": "C",
            "is_outgoing": true
        }))
        .as_deref(),
        Some("REAL Yuki")
    );
    assert_eq!(
        name_from_payload(&json!({
            "chat_kind": "group",
            "chat_title": "Team",
            "sender_name": "Alice"
        }))
        .as_deref(),
        Some("Alice")
    );
}

#[test]
fn history_payload_name_cleans_email_headers() {
    assert_eq!(
        name_from_payload(&json!({
            "from": "Alice Example <Alice@Example.COM>"
        }))
        .as_deref(),
        Some("Alice Example")
    );
    assert_eq!(
        name_from_payload(&json!({
            "from": "alice@example.com",
            "event": {"title": "Launch Review"}
        })),
        None
    );
}

#[test]
fn contacts_list_returns_lightweight_candidate_previews() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let diagnostics = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user")
        .join("message-diagnostics.ndjson");
    std::fs::create_dir_all(diagnostics.parent().unwrap()).unwrap();
    let messages = [
        json!({
            "stage": "emitted",
            "chat_kind": "user",
            "chat_id": 5,
            "chat_username": "alice_smith",
            "chat_title": "Alice",
            "message_id": 1,
            "date_ms": 1_700_000_000_000_i64,
            "text_prefix": "could you review the contacts loading issue"
        }),
        json!({
            "stage": "emitted",
            "chat_kind": "user",
            "chat_id": 5,
            "chat_username": "alice_smith",
            "chat_title": "Alice",
            "message_id": 2,
            "date_ms": 1_700_000_060_000_i64,
            "is_outgoing": true,
            "text_prefix": "yes I will review the contacts loading issue"
        }),
        json!({
            "stage": "emitted",
            "chat_kind": "user",
            "chat_id": 6,
            "chat_username": "alertbot",
            "chat_title": "Alert Bot",
            "chat_is_bot": true,
            "message_id": 3,
            "date_ms": 1_700_000_120_000_i64,
            "text_prefix": "automated alert for deployment"
        }),
    ]
    .into_iter()
    .map(|message| serde_json::to_string(&message).unwrap())
    .collect::<Vec<_>>()
    .join("\n");
    std::fs::write(diagnostics, messages).unwrap();

    let result = handle_contacts_list(&paths, &json!({ "limit": 10 })).unwrap();
    let candidates = result["candidates"].as_array().unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate["id"] == "telegram-user-id@5")
        .unwrap();

    assert!(candidate.get("context").is_none(), "{candidate:#}");
    assert!(candidate.get("avatar").is_none(), "{candidate:#}");
    assert_eq!(candidate["name"], "Alice (@alice_smith)");
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["id"] == "telegram@alertbot"));
}

#[test]
fn contacts_list_prefers_telegram_peer_cache_names() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let avatar = "data:image/jpeg;base64,ZmFrZS1hdmF0YXI=";
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();
    let diagnostics = account_dir.join("message-diagnostics.ndjson");
    let messages = [
        json!({
            "stage": "emitted",
            "chat_kind": "user",
            "chat_id": 37253512_i64,
            "chat_username": "tohsakar_in",
            "chat_title": "Rin",
            "message_id": 1,
            "date_ms": 1_700_000_000_000_i64,
            "text_prefix": "can you check the snapshot storage"
        }),
        json!({
            "stage": "emitted",
            "chat_kind": "user",
            "chat_id": 37253512_i64,
            "chat_username": "tohsakar_in",
            "chat_title": "Rin",
            "message_id": 2,
            "date_ms": 1_700_000_060_000_i64,
            "is_outgoing": true,
            "text_prefix": "yes I can check the snapshot storage"
        }),
    ]
    .into_iter()
    .map(|message| serde_json::to_string(&message).unwrap())
    .collect::<Vec<_>>()
    .join("\n");
    std::fs::write(diagnostics, messages).unwrap();
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": [{
                "id": "37253512",
                "numeric_id": 37253512_i64,
                "kind": "user",
                "title": "Rin Tohsaka",
                "username": "tohsakar_in",
                "first_name": "Rin",
                "avatar": avatar,
                "is_bot": false,
                "updated_at_ms": 1_700_000_000_000_i64
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let result = handle_contacts_list(&paths, &json!({ "limit": 10 })).unwrap();
    let candidates = result["candidates"].as_array().unwrap();
    let candidate = candidates
        .iter()
        .find(|candidate| candidate["id"] == "telegram-user-id@37253512")
        .unwrap();

    assert_eq!(candidate["name"], "Rin Tohsaka");
    assert_eq!(candidate["display"]["label"], "Rin Tohsaka");
    assert_eq!(candidate["display"]["name"], "Rin Tohsaka");
    assert_eq!(candidate["display"]["public_username"], "tohsakar_in");
    assert_eq!(candidate["display"]["username_label"], "@tohsakar_in");
    assert!(
        !candidate["display"].to_string().contains("37253512"),
        "{candidate:#}"
    );
    assert!(candidate.get("avatar").is_none(), "{candidate:#}");

    let search = handle_contacts_search(&paths, &json!({ "query": "rin", "limit": 10 })).unwrap();
    let search_candidate = search["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["id"] == "telegram-user-id@37253512")
        .unwrap();
    assert!(
        search_candidate.get("avatar").is_none(),
        "{search_candidate:#}"
    );

    let saved = handle_contacts_save(
        &paths,
        &json!({
            "name": "Rin",
            "description": "Technical collaborator.",
            "contact_ids": ["telegram-user-id@37253512"]
        }),
    )
    .unwrap();
    let saved_contact = saved["contacts"].as_array().unwrap().first().unwrap();
    assert_eq!(saved_contact["avatar"], avatar);
}

#[test]
fn contacts_list_uses_telegram_peer_cache_without_message_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": [
                {
                    "id": "37253512",
                    "numeric_id": 37253512_i64,
                    "kind": "user",
                    "title": "Rin Tohsaka",
                    "username": "tohsakar_in",
                    "first_name": "Rin",
                    "is_bot": false,
                    "updated_at_ms": 1_700_000_000_000_i64
                },
                {
                    "id": "5229190700",
                    "numeric_id": 5229190700_i64,
                    "kind": "user",
                    "title": "Numeric Direct",
                    "first_name": "Numeric",
                    "is_bot": false,
                    "updated_at_ms": 1_700_000_000_000_i64
                },
                {
                    "id": "9",
                    "numeric_id": 9_i64,
                    "kind": "user",
                    "title": "Build Bot",
                    "username": "buildbot",
                    "is_bot": true,
                    "updated_at_ms": 1_700_000_000_000_i64
                },
                {
                    "id": "-100123",
                    "numeric_id": -100123_i64,
                    "kind": "channel",
                    "title": "Announcements",
                    "username": "announcements",
                    "is_bot": false,
                    "updated_at_ms": 1_700_000_000_000_i64
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let result = handle_contacts_list(&paths, &json!({ "limit": 10 })).unwrap();
    let candidates = result["candidates"].as_array().unwrap();

    assert!(candidates
        .iter()
        .any(|candidate| candidate["id"] == "telegram-user-id@37253512"
            && candidate["name"] == "Rin Tohsaka"));
    assert!(candidates
        .iter()
        .any(|candidate| candidate["id"] == "telegram-user-id@5229190700"
            && candidate["name"] == "Numeric Direct"));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["id"] == "telegram@buildbot"));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["id"] == "telegram@announcements"));
}

#[test]
fn contacts_list_filters_current_telegram_user_from_peer_cache() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();

    let session = Session::new();
    session.set_user(42, 2, false);
    let session_path = account_dir.join("telegram.session");
    std::fs::File::create(&session_path).unwrap();
    session.save_to_file(&session_path).unwrap();

    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": [
                {
                    "id": "42",
                    "numeric_id": 42_i64,
                    "kind": "user",
                    "title": "Me Myself",
                    "username": "me",
                    "first_name": "Me",
                    "is_bot": false,
                    "updated_at_ms": 1_700_000_000_000_i64
                },
                {
                    "id": "43",
                    "numeric_id": 43_i64,
                    "kind": "user",
                    "title": "Alice",
                    "username": "alice",
                    "first_name": "Alice",
                    "is_bot": false,
                    "updated_at_ms": 1_700_000_000_000_i64
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let result = handle_contacts_list(&paths, &json!({ "limit": 10 })).unwrap();
    let candidates = result["candidates"].as_array().unwrap();

    assert!(!candidates
        .iter()
        .any(|candidate| candidate["id"] == "telegram-user-id@42"));
    assert!(candidates.iter().any(|candidate| {
        candidate["id"] == "telegram-user-id@43" && candidate["name"] == "Alice"
    }));
}

fn telegram_peer_cache_user(id: i64, title: &str, last_message_at_ms: i64) -> Value {
    json!({
        "id": id.to_string(),
        "numeric_id": id,
        "kind": "user",
        "title": title,
        "first_name": title,
        "is_bot": false,
        "last_message_at_ms": last_message_at_ms,
        "updated_at_ms": last_message_at_ms
    })
}

fn read_recent_dialog_cache_marker(account_dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(account_dir.join("recent-dialog-cache.json")).unwrap())
        .unwrap()
}

#[test]
fn contacts_list_ignores_stale_delivery_cursor_for_recent_readiness() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();
    std::fs::write(
        account_dir.join("delivery-cursor.json"),
        serde_json::to_vec_pretty(&json!({ "initialized": true })).unwrap(),
    )
    .unwrap();
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": (1..=5)
                .map(|id| telegram_peer_cache_user(
                    id,
                    &format!("Partial Contact {id}"),
                    1_700_000_000_000_i64 + id,
                ))
                .collect::<Vec<_>>()
        }))
        .unwrap(),
    )
    .unwrap();

    let cold = handle_contacts_list(
        &paths,
        &json!({ "connector": "telegram", "sort": "recent", "limit": 50 }),
    )
    .unwrap();

    assert_eq!(cold["ready"], false);
    assert_eq!(cold["candidate_count"], 5);

    super::daemon_contacts_telegram::write_test_recent_dialog_cache_marker(&account_dir, 18, 50)
        .unwrap();
    let partial_marker = read_recent_dialog_cache_marker(&account_dir);
    assert_eq!(partial_marker["ready"], true);
    assert_eq!(partial_marker["direct_users_seen"], 18);
    assert_eq!(partial_marker["target"], 50);
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": (1..=18)
                .map(|id| telegram_peer_cache_user(
                    id,
                    &format!("Partial Contact {id}"),
                    1_700_000_000_000_i64 + id,
                ))
                .collect::<Vec<_>>()
        }))
        .unwrap(),
    )
    .unwrap();

    let partial = handle_contacts_list(
        &paths,
        &json!({ "connector": "telegram", "sort": "recent", "limit": 50 }),
    )
    .unwrap();

    assert_eq!(partial["ready"], false);
    assert_eq!(partial["candidate_count"], 18);

    super::daemon_contacts_telegram::write_test_recent_dialog_cache_marker(&account_dir, 50, 50)
        .unwrap();
    let complete_marker = read_recent_dialog_cache_marker(&account_dir);
    assert_eq!(complete_marker["ready"], true);
    assert_eq!(complete_marker["direct_users_seen"], 50);
    assert_eq!(complete_marker["target"], 50);
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": (1..=50)
                .map(|id| telegram_peer_cache_user(
                    id,
                    &format!("Complete Contact {id}"),
                    1_700_000_000_000_i64 + id,
                ))
                .collect::<Vec<_>>()
        }))
        .unwrap(),
    )
    .unwrap();

    let complete = handle_contacts_list(
        &paths,
        &json!({ "connector": "telegram", "sort": "recent", "limit": 50 }),
    )
    .unwrap();

    assert_eq!(complete["ready"], true);
    assert_eq!(complete["candidate_count"], 50);
}

#[test]
fn contacts_list_rehydrates_recent_cache_when_marker_claims_partial_target() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();
    std::fs::write(account_dir.join("telegram.session"), b"fake-session").unwrap();
    super::daemon_contacts_telegram::write_test_recent_dialog_cache_marker(&account_dir, 18, 50)
        .unwrap();
    let partial_marker = read_recent_dialog_cache_marker(&account_dir);
    assert_eq!(partial_marker["ready"], true);
    assert_eq!(partial_marker["direct_users_seen"], 18);
    assert_eq!(partial_marker["target"], 50);
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": (1..=18)
                .map(|id| telegram_peer_cache_user(
                    id,
                    &format!("Partial Contact {id}"),
                    1_700_000_000_000_i64 + id,
                ))
                .collect::<Vec<_>>()
        }))
        .unwrap(),
    )
    .unwrap();

    let calls = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let _guard = super::daemon_contacts_telegram::install_test_telegram_peer_cache_hydrator({
        let calls = std::sync::Arc::clone(&calls);
        move |_paths, account_dir| {
            *calls.lock().unwrap() += 1;
            super::daemon_contacts_telegram::write_test_recent_dialog_cache_marker(
                account_dir,
                50,
                50,
            )
            .unwrap();
            std::fs::write(
                account_dir.join("peer-cache.json"),
                serde_json::to_vec_pretty(&json!({
                    "version": 1,
                    "peers": (1..=50)
                        .map(|id| telegram_peer_cache_user(
                            id,
                            &format!("Complete Contact {id}"),
                            1_700_000_000_000_i64 + id,
                        ))
                        .collect::<Vec<_>>()
                }))
                .unwrap(),
            )
            .unwrap();
            Ok(())
        }
    });

    let result = handle_contacts_list(
        &paths,
        &json!({ "connector": "telegram", "sort": "recent", "limit": 50 }),
    )
    .unwrap();

    assert_eq!(*calls.lock().unwrap(), 1);
    assert_eq!(result["ready"], true);
    assert_eq!(result["candidate_count"], 50);
}

#[test]
fn contacts_list_repairs_recent_cache_when_service_user_consumed_target() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();
    std::fs::write(
        account_dir.join("recent-dialog-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "ready": true,
            "direct_users_seen": 5,
            "target": 5
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": [
                telegram_peer_cache_user(777000, "Telegram", 1_700_000_005_000_i64),
                telegram_peer_cache_user(1, "Alice", 1_700_000_004_000_i64),
                telegram_peer_cache_user(2, "Bob", 1_700_000_003_000_i64),
                telegram_peer_cache_user(3, "Cara", 1_700_000_002_000_i64),
                telegram_peer_cache_user(4, "Dina", 1_700_000_001_000_i64)
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let calls = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let _guard = super::daemon_contacts_telegram::install_test_telegram_peer_cache_hydrator({
        let calls = std::sync::Arc::clone(&calls);
        move |_paths, account_dir| {
            *calls.lock().unwrap() += 1;
            std::fs::write(
                account_dir.join("peer-cache.json"),
                serde_json::to_vec_pretty(&json!({
                    "version": 1,
                    "peers": [
                        telegram_peer_cache_user(777000, "Telegram", 1_700_000_006_000_i64),
                        telegram_peer_cache_user(1, "Alice", 1_700_000_005_000_i64),
                        telegram_peer_cache_user(2, "Bob", 1_700_000_004_000_i64),
                        telegram_peer_cache_user(3, "Cara", 1_700_000_003_000_i64),
                        telegram_peer_cache_user(4, "Dina", 1_700_000_002_000_i64),
                        telegram_peer_cache_user(5, "Eli", 1_700_000_001_000_i64)
                    ]
                }))
                .unwrap(),
            )?;
            Ok(())
        }
    });

    let result = handle_contacts_list(
        &paths,
        &json!({ "connector": "telegram", "sort": "recent", "limit": 5 }),
    )
    .unwrap();
    let candidates = result["candidates"].as_array().unwrap();

    assert_eq!(*calls.lock().unwrap(), 1);
    assert_eq!(candidates.len(), 5);
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["id"] == "telegram-user-id@777000"));
    assert!(candidates
        .iter()
        .any(|candidate| candidate["id"] == "telegram-user-id@5" && candidate["name"] == "Eli"));
}

#[test]
fn contacts_refresh_forces_telegram_peer_cache_hydration_when_cache_is_non_empty() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": [{
                "id": "1",
                "numeric_id": 1_i64,
                "kind": "user",
                "title": "Old Contact",
                "username": "old_contact",
                "is_bot": false,
                "updated_at_ms": 1_700_000_000_000_i64
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let _guard = super::daemon_contacts_telegram::install_test_telegram_peer_cache_hydrator({
        let calls = std::sync::Arc::clone(&calls);
        move |_paths, account_dir| {
            calls.lock().unwrap().push(account_dir.to_path_buf());
            std::fs::write(
                account_dir.join("peer-cache.json"),
                serde_json::to_vec_pretty(&json!({
                    "version": 1,
                    "peers": [{
                        "id": "2",
                        "numeric_id": 2_i64,
                        "kind": "user",
                        "title": "Fresh Contact",
                        "username": "fresh_contact",
                        "is_bot": false,
                        "updated_at_ms": 1_700_000_100_000_i64
                    }]
                }))
                .unwrap(),
            )?;
            Ok(())
        }
    });

    let result = handle_contacts_refresh(&paths, &json!({ "limit": 10 })).unwrap();
    let candidates = result["candidates"].as_array().unwrap();

    assert_eq!(calls.lock().unwrap().len(), 1);
    assert!(candidates
        .iter()
        .any(|candidate| candidate["id"] == "telegram-user-id@2"
            && candidate["name"] == "Fresh Contact"));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["id"] == "telegram-user-id@1"));
}

#[test]
fn contacts_list_returns_cursor_pagination_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": [
                telegram_peer_cache_user(1, "Alice", 1_700_000_001_000_i64),
                telegram_peer_cache_user(2, "Bob", 1_700_000_002_000_i64),
                telegram_peer_cache_user(3, "Cara", 1_700_000_003_000_i64)
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let first = handle_contacts_list(&paths, &json!({ "limit": 2 })).unwrap();
    let first_candidates = first["candidates"].as_array().unwrap();

    assert_eq!(first["limit"], 2);
    assert_eq!(first["returned_count"], 2);
    assert_eq!(first["candidate_count"], 3);
    assert_eq!(first["has_more"], true);
    let cursor = first["next_cursor"].as_str().unwrap();
    assert!(!cursor.is_empty());

    let second = handle_contacts_list(&paths, &json!({ "limit": 2, "cursor": cursor })).unwrap();
    let second_candidates = second["candidates"].as_array().unwrap();

    assert_eq!(second["returned_count"], 1);
    assert_eq!(second["candidate_count"], 3);
    assert_eq!(second["has_more"], false);
    assert!(second["next_cursor"].is_null());
    assert!(first_candidates
        .iter()
        .all(|candidate| candidate["id"] != second_candidates[0]["id"]));
}

#[test]
fn contacts_cursor_pagination_survives_new_candidate_ahead_of_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": [
                telegram_peer_cache_user(2, "Bob", 1_700_000_002_000_i64),
                telegram_peer_cache_user(3, "Cara", 1_700_000_003_000_i64),
                telegram_peer_cache_user(4, "Dina", 1_700_000_004_000_i64)
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let first = handle_contacts_list(&paths, &json!({ "limit": 2 })).unwrap();
    let first_candidates = first["candidates"].as_array().unwrap();
    assert_eq!(first_candidates[0]["id"], "telegram-user-id@2");
    assert_eq!(first_candidates[1]["id"], "telegram-user-id@3");
    let cursor = first["next_cursor"].as_str().unwrap();

    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": [
                telegram_peer_cache_user(1, "New Contact", 1_700_000_001_000_i64),
                telegram_peer_cache_user(2, "Bob", 1_700_000_002_000_i64),
                telegram_peer_cache_user(3, "Cara", 1_700_000_003_000_i64),
                telegram_peer_cache_user(4, "Dina", 1_700_000_004_000_i64)
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let second = handle_contacts_list(&paths, &json!({ "limit": 2, "cursor": cursor })).unwrap();
    let second_candidates = second["candidates"].as_array().unwrap();

    assert_eq!(second_candidates.len(), 1);
    assert_eq!(second_candidates[0]["id"], "telegram-user-id@4");
}

#[test]
fn telegram_peer_cache_skips_malformed_entries() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let avatar = "data:image/jpeg;base64,ZmFrZS1hdmF0YXI=";
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": [
                {
                    "kind": "user",
                    "title": "Bad Entry",
                    "username": "bad_entry",
                    "is_bot": "not-a-bool"
                },
                {
                    "kind": "user",
                    "title": "Yu Feng",
                    "username": "yufeng_us",
                    "first_name": "Yu",
                    "last_name": "Feng",
                    "avatar": avatar,
                    "is_bot": false
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let names = read_telegram_peer_names(&paths);
    let avatars = read_telegram_peer_avatars(&paths);

    assert_eq!(
        names.get("telegram@yufeng_us").map(String::as_str),
        Some("Yu Feng")
    );
    assert_eq!(
        avatars.get("telegram@yufeng_us").map(String::as_str),
        Some(avatar)
    );
    assert!(!names.contains_key("telegram@bad_entry"));
    assert!(!avatars.contains_key("telegram@bad_entry"));
}

#[test]
fn history_candidate_name_prefers_telegram_peer_cache_names() {
    let mut peer_names = HashMap::new();
    peer_names.insert("telegram@yufeng_us".to_string(), "Yu Feng".to_string());
    let payload = json!({
        "chat_kind": "group",
        "sender_username": "yufeng_us",
        "sender_name": "Yu"
    });

    let name = history_candidate_name("telegram@yufeng_us", Some(&payload), &peer_names);

    assert_eq!(name.as_deref(), Some("Yu Feng"));
}

#[test]
fn history_candidate_uses_telegram_peer_cache_avatar() {
    let avatar = "data:image/jpeg;base64,ZmFrZS1hdmF0YXI=";
    let mut peer_names = HashMap::new();
    peer_names.insert("telegram@yufeng_us".to_string(), "Yu Feng".to_string());
    let mut peer_avatars = HashMap::new();
    peer_avatars.insert("telegram@yufeng_us".to_string(), avatar.to_string());
    let run = puffer_subscriptions::WorkflowBindingRun {
        idx: 0,
        run_id: "run".to_string(),
        workflow_slug: "monitor-telegram-user".to_string(),
        trigger_info: json!({
            "received_at_ms": 1_700_000_000_000_i64,
            "text": "Yu Feng shared the deployment notes",
            "payload": {
                "chat_kind": "user",
                "chat_username": "yufeng_us",
                "chat_title": "Yu"
            }
        }),
        action_summary: Value::Null,
        action_log: Vec::new(),
        status: puffer_subscriptions::WorkflowBindingRunStatus::Completed,
        started_at_ms: 1_700_000_000_000,
        ended_at_ms: 1_700_000_000_100,
    };
    let mut by_id = HashMap::new();

    merge_history_candidate_run(
        &run,
        &mut by_id,
        CandidateContextOptions::none(),
        &peer_names,
        &peer_avatars,
    );

    let candidate = by_id.get("telegram@yufeng_us").unwrap();
    assert_eq!(candidate.name.as_deref(), Some("Yu Feng"));
    assert_eq!(candidate.avatar.as_deref(), Some(avatar));
    assert!(candidate.context.is_empty());
}

#[test]
fn history_context_text_labels_telegram_group_payloads() {
    let payload = json!({
        "chat_kind": "group",
        "chat_id": -100,
        "chat_title": "OKX DEX API Solana Self Host",
        "sender_username": "ricky3211",
        "sender_name": "Ricky",
        "is_outgoing": false
    });

    let text = history_context_text(Some(&payload), "cuLimit will be optimized this week");

    assert_eq!(
        text,
        "[history][dest: OKX DEX API Solana Self Host][from: Ricky] cuLimit will be optimized this week"
    );
}

#[test]
fn contacts_context_returns_marked_telegram_destination_context() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();
    let diagnostics = account_dir.join("message-diagnostics.ndjson");
    let messages = [
        json!({
            "stage": "emitted",
            "chat_kind": "user",
            "chat_id": 5,
            "chat_username": "alice",
            "chat_title": "Alice Smith",
            "message_id": 1,
            "date_ms": 1_700_000_000_000_i64,
            "text_prefix": "can you review the contact context"
        }),
        json!({
            "stage": "emitted",
            "chat_kind": "user",
            "chat_id": 5,
            "chat_username": "alice",
            "chat_title": "Alice Smith",
            "message_id": 2,
            "date_ms": 1_700_000_060_000_i64,
            "is_outgoing": true,
            "text_prefix": "yes I can review the contact context"
        }),
    ]
    .into_iter()
    .map(|message| serde_json::to_string(&message).unwrap())
    .collect::<Vec<_>>()
    .join("\n");
    std::fs::write(diagnostics, messages).unwrap();

    let result = handle_contacts_context(
        &paths,
        &json!({ "contact_ids": ["telegram-user-id@5"], "limit": 30 }),
    )
    .unwrap();
    let context = result["context"].as_array().unwrap();

    assert!(context.iter().any(|item| {
        item["kind"] == "telegram_recent_message"
            && item["text"]
                .as_str()
                .is_some_and(|text| text.contains("[dest: Alice Smith]"))
    }));
    assert!(context
        .iter()
        .any(|item| item["kind"] == "telegram_interaction_user_reply"));
    assert_eq!(
        context[0].pointer("/payload/destination/label"),
        Some(&json!("Alice Smith"))
    );
}

#[test]
fn contacts_context_rejects_empty_or_invalid_contact_ids() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_config_paths(temp.path());

    for params in [
        json!({ "contact_ids": [] }),
        json!({ "contact_ids": ["not-a-contact"] }),
    ] {
        let error = handle_contacts_context(&paths, &params).unwrap_err();
        assert!(error
            .to_string()
            .contains("contact context requires at least one valid contact id"));
    }
}

#[test]
fn contact_infer_prompt_omits_selection_guidance_block() {
    let prompt = contact_infer_system_prompt();

    assert!(!prompt.contains("Selection guidance:"));
    assert!(!prompt.contains("Strong contacts:"));
    assert!(!prompt.contains("Weak or spam-like contacts:"));
    assert!(prompt.contains("natural summary of the user's relationship"));
    assert!(!prompt.contains("This contact matters because"));
    assert!(!prompt.contains("This is not spam"));
    assert!(!prompt.contains("Return zero tool calls if no candidate is clearly worth saving."));
}

fn read_test_message(payload: Value) -> TelegramDiagMessage {
    let chat_kind = payload
        .get("chat_kind")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let name = telegram_contact_name(&payload, &chat_kind);
    let destination_name = if chat_kind == "user" {
        name.clone()
            .or_else(|| payload_string(&payload, "chat_title"))
            .or_else(|| payload_string(&payload, "chat_username").map(|value| format!("@{value}")))
    } else {
        payload_string(&payload, "chat_title")
            .or_else(|| payload_string(&payload, "chat_username").map(|value| format!("@{value}")))
    };
    TelegramDiagMessage {
        contact_id: telegram_contact_id(&payload, &chat_kind).unwrap(),
        name,
        public_username: payload_string(&payload, "chat_username"),
        avatar: None,
        destination_name,
        destination_username: payload_string(&payload, "chat_username"),
        chat_id: payload.get("chat_id").and_then(Value::as_i64).unwrap_or(0),
        chat_kind,
        sender_username: payload
            .get("sender_username")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        sender_name: payload_string(&payload, "sender_name"),
        is_outgoing: payload
            .get("is_outgoing")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_self_contact: payload
            .get("is_self_contact")
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

fn payload_string(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn test_config_paths(root: &Path) -> ConfigPaths {
    ConfigPaths {
        workspace_root: root.to_path_buf(),
        workspace_config_dir: root.join(".puffer"),
        user_config_dir: root.join("user-puffer"),
        builtin_resources_dir: root.join("resources"),
    }
}
