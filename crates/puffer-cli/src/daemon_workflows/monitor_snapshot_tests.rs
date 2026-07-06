use super::*;
use puffer_subscriptions::{NewOutboundDraft, OutboundOrigin, OutboundStore, RecipientSource};

fn create_outbound_action(paths: &ConfigPaths, task_id: &str, message: &str) -> String {
    OutboundStore::load(paths.user_config_dir.join("outbound_actions.json"))
        .unwrap()
        .create_draft(NewOutboundDraft {
            connector_slug: "telegram-login".to_string(),
            connection_slug: "telegram-user".to_string(),
            action: "send_message".to_string(),
            input: json!({ "chat_id": "42", "message": message }),
            recipient_stable_id: "telegram:42".to_string(),
            recipient_source: RecipientSource::Stamped,
            message: message.to_string(),
            origin: OutboundOrigin {
                session_id: "session-1".to_string(),
                turn_id: Some("turn-1".to_string()),
                task_id: Some(task_id.to_string()),
            },
            ttl_ms: None,
        })
        .unwrap()
        .id
}

#[test]
fn workflow_snapshot_includes_monitor_tasks() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    let outbound_action_id =
        create_outbound_action(&paths, "monitor-1", "Deployment finished an hour ago.");
    let task_path = monitor_tasks_path(&paths);
    std::fs::create_dir_all(task_path.parent().unwrap()).unwrap();
    std::fs::write(
        &task_path,
        serde_json::to_string_pretty(&json!({
            "tasks": [
                {
                    "task_id": "monitor-1",
                    "subject": "Answer Telegram support ping",
                    "description": "Alice asked whether the deployment is finished.",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "monitor_memory_path": "/tmp/telegram-user.md",
                        "actions": [
                            {
                                "actionName": "Draft reply",
                                "actionPrompt": "Draft a concise reply to Alice."
                            }
                        ],
                        "possibleIgnoreReasons": ["duplicate support ping"],
                        "source_context": {
                            "connector": "telegram-login",
                            "chat_id": 42
                        },
                        "source_text": "回调失败率刚升到 18%，16:00 前给结论。",
                        "source_message_id": 6836,
                        "completion_policy": "human_gated_reply",
                        "outbound_action_id": outbound_action_id
                    },
                    "started_at_ms": 10,
                    "updated_at_ms": 20
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let snapshot = handle_workflow_list(&paths).unwrap();
    let tasks = snapshot["monitor_tasks"].as_array().unwrap();

    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["task_id"], "monitor-1");
    assert_eq!(tasks[0]["monitor_connection"], "telegram-user");
    assert_eq!(tasks[0]["monitor_connector"], "telegram-login");
    assert_eq!(tasks[0]["monitor_memory_path"], "/tmp/telegram-user.md");
    assert_eq!(tasks[0]["actions"][0]["name"], "Draft reply");
    assert_eq!(
        tasks[0]["actions"][0]["prompt"],
        "Draft a concise reply to Alice."
    );
    assert_eq!(
        tasks[0]["possible_ignore_reasons"][0],
        "duplicate support ping"
    );
    // The human-gated reply review state must surface on monitor_tasks[]
    // (the array bobo's Home renders), not only on the tasks[] snapshot.
    assert_eq!(tasks[0]["source_context"]["chat_id"], 42);
    // The server-stamped verbatim text rides along on source_context so
    // UIs can show the original message next to the LLM paraphrase.
    assert_eq!(
        tasks[0]["source_context"]["text"],
        "回调失败率刚升到 18%，16:00 前给结论。"
    );
    assert_eq!(tasks[0]["source_context"]["message_id"], 6836);
    assert_eq!(tasks[0]["completion_policy"], "human_gated_reply");
    assert_eq!(tasks[0]["outboundAction"]["id"], outbound_action_id);
    assert_eq!(tasks[0]["outboundAction"]["status"], "draft_ready");
    assert_eq!(tasks[0]["outboundAction"]["version"], 1);
    assert_eq!(
        tasks[0]["outboundAction"]["message"],
        "Deployment finished an hour ago."
    );
    assert_eq!(snapshot["monitor_task_error"], Value::Null);
}

#[test]
fn workflow_snapshot_enriches_monitor_sender_avatar_from_telegram_peer_cache() {
    let tempdir = tempfile::tempdir().unwrap();
    // Without the home override, user_config_dir resolves to the REAL
    // ~/.puffer and this test clobbers the developer's peer cache.
    let _home = puffer_config::set_puffer_home_override(tempdir.path());
    let paths = ConfigPaths::discover(tempdir.path());
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
            "peers": [{
                "id": "5229190700",
                "numeric_id": 5229190700_i64,
                "kind": "user",
                "title": "Helen",
                "username": "helen",
                "avatar": avatar,
                "is_bot": false,
                "updated_at_ms": 1_700_000_000_000_i64
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let task_path = monitor_tasks_path(&paths);
    std::fs::create_dir_all(task_path.parent().unwrap()).unwrap();
    std::fs::write(
        &task_path,
        serde_json::to_string_pretty(&json!({
            "tasks": [{
                "task_id": "monitor-avatar",
                "subject": "Reply to Helen",
                "description": "Helen asked for the shipping ETA.",
                "status": "pending",
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "telegram-user",
                    "monitor_connector": "telegram-login",
                    "chat_id": "5229190700",
                    "sender_id": "5229190700",
                    "sender_username": "helen"
                },
                "started_at_ms": 10,
                "updated_at_ms": 20
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let snapshot = handle_workflow_list(&paths).unwrap();
    let tasks = snapshot["monitor_tasks"].as_array().unwrap();

    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["source_context"]["sender"]["avatar_url"], avatar);
    // The display name rides along from the same peer-cache entry, so
    // accounts without an @username stop rendering as "Unknown sender".
    assert_eq!(tasks[0]["source_context"]["sender"]["name"], "Helen");
}

#[test]
fn workflow_snapshot_enriches_sender_name_without_username_or_avatar() {
    let tempdir = tempfile::tempdir().unwrap();
    let _home = puffer_config::set_puffer_home_override(tempdir.path());
    let paths = ConfigPaths::discover(tempdir.path());
    let account_dir = paths
        .user_config_dir
        .join("telegram-accounts")
        .join("telegram-user");
    std::fs::create_dir_all(&account_dir).unwrap();
    // Mirrors the reported case: a contact with a display name but no
    // @username and no profile photo (Telegram letter-avatar account).
    std::fs::write(
        account_dir.join("peer-cache.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "peers": [{
                "id": "8759047281",
                "numeric_id": 8759047281_i64,
                "kind": "user",
                "title": "博阿 杜",
                "first_name": "博阿",
                "last_name": "杜",
                "is_bot": false,
                "updated_at_ms": 1_700_000_000_000_i64
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let task_path = monitor_tasks_path(&paths);
    std::fs::create_dir_all(task_path.parent().unwrap()).unwrap();
    std::fs::write(
        &task_path,
        serde_json::to_string_pretty(&json!({
            "tasks": [{
                "task_id": "monitor-name",
                "subject": "调研国保单位清单",
                "description": "Telegram 联系人发来请求。",
                "status": "pending",
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "telegram-user",
                    "monitor_connector": "telegram-login",
                    "chat_id": "8759047281",
                    "sender_id": "8759047281"
                },
                "started_at_ms": 10,
                "updated_at_ms": 20
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let snapshot = handle_workflow_list(&paths).unwrap();
    let tasks = snapshot["monitor_tasks"].as_array().unwrap();

    assert_eq!(tasks[0]["source_context"]["sender"]["name"], "博阿 杜");
    assert!(tasks[0]["source_context"]["sender"]
        .get("avatar_url")
        .is_none());
}
