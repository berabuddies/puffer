use super::*;
use puffer_config::ConfigPaths;
use std::fs;

#[test]
fn monitor_task_create_sanitizes_source_and_judgment_language() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let created = crate::runtime::claude_tools::workflow::task_create::execute_task_create(
        &mut state,
        &cwd,
        json!({
            "subject": "Reply to Telegram request to buy something from a supermarket",
            "description": "Personal Telegram message asks: \"帮我买个东西路过超市的时候\". This is an actionable request to buy an item when passing a supermarket. Need a reply that clarifies what to buy, any budget/preferences, and when they expect it.",
            "metadata": {
                "_monitor": true,
                "monitor_connection": "telegram-user",
                "monitor_connector": "telegram-login",
                "monitor_memory_path": "/tmp/telegram-user.md"
            },
            "receivedAt": "2026-05-27T12:00:00Z",
            "expiresAt": "2026-05-28T12:00:00Z"
        }),
    )
    .unwrap();
    let created: Value = serde_json::from_str(&created).unwrap();
    let task_id = created["task"]["id"].as_str().unwrap();

    let task = crate::runtime::claude_tools::workflow::task_get::execute_task_get(
        &mut state,
        &cwd,
        json!({ "taskId": task_id }),
    )
    .unwrap();
    let task: Value = serde_json::from_str(&task).unwrap();
    let subject = task["task"]["subject"].as_str().unwrap();
    let description = task["task"]["description"].as_str().unwrap();

    assert_monitor_task_text_is_sanitized(subject, description);
    assert!(description.contains("帮我买个东西路过超市的时候"));
    assert!(!description.contains("what to buy"));
}

#[test]
fn monitor_task_update_sanitizes_source_and_judgment_language() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let created = crate::runtime::claude_tools::workflow::task_create::execute_task_create(
        &mut state,
        &cwd,
        json!({
            "subject": "问清要买什么",
            "description": "帮我买个东西路过超市的时候。问清楚要买什么。",
            "metadata": {
                "_monitor": true,
                "monitor_connection": "telegram-user",
                "monitor_connector": "telegram-login",
                "monitor_memory_path": "/tmp/telegram-user.md"
            },
            "receivedAt": "2026-05-27T12:00:00Z",
            "expiresAt": "2026-05-28T12:00:00Z"
        }),
    )
    .unwrap();
    let created: Value = serde_json::from_str(&created).unwrap();
    let task_id = created["task"]["id"].as_str().unwrap();

    crate::runtime::claude_tools::workflow::task_update::execute_task_update(
        &mut state,
        &cwd,
        json!({
            "taskId": task_id,
            "subject": "Reply to telegram request to buy something from a supermarket",
            "description": "Personal Telegram Message asks: \"帮我买个东西路过超市的时候\". This is an actionable request to buy an item when passing a supermarket. Need a reply that clarifies what to buy."
        }),
    )
    .unwrap();

    let task = crate::runtime::claude_tools::workflow::task_get::execute_task_get(
        &mut state,
        &cwd,
        json!({ "taskId": task_id }),
    )
    .unwrap();
    let task: Value = serde_json::from_str(&task).unwrap();
    let subject = task["task"]["subject"].as_str().unwrap();
    let description = task["task"]["description"].as_str().unwrap();

    assert_monitor_task_text_is_sanitized(subject, description);
    assert!(description.contains("帮我买个东西路过超市的时候"));
    assert!(!description.contains("what to buy"));
}

#[test]
fn monitor_task_get_backfills_legacy_source_and_judgment_language() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let monitor_tasks_path = ConfigPaths::discover(&cwd)
        .workspace_config_dir
        .join("runtime/claude_workflow/monitor_tasks.json");
    fs::create_dir_all(monitor_tasks_path.parent().unwrap()).unwrap();
    fs::write(
        &monitor_tasks_path,
        serde_json::to_string_pretty(&json!({
            "tasks": [{
                "task_id": "monitor-26",
                "subject": "Reply to Telegram request to buy something from a supermarket",
                "description": "Personal Telegram message asks: \"帮我买个东西路过超市的时候\". This is an actionable request to buy an item when passing a supermarket. Likely needs a response asking what to buy. Need a reply that clarifies what to buy.",
                "active_form": "Working",
                "status": "pending",
                "owner": null,
                "blocks": [],
                "blocked_by": [],
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "telegram-user",
                    "monitor_connector": "telegram-login",
                    "monitor_memory_path": "/tmp/telegram-user.md"
                },
                "output": null,
                "task_type": "task",
                "receivedAt": "2026-05-27T12:00:00Z",
                "expiresAt": "2026-05-28T12:00:00Z"
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let task = crate::runtime::claude_tools::workflow::task_get::execute_task_get(
        &mut state,
        &cwd,
        json!({ "taskId": "monitor-26" }),
    )
    .unwrap();
    let task: Value = serde_json::from_str(&task).unwrap();
    let subject = task["task"]["subject"].as_str().unwrap();
    let description = task["task"]["description"].as_str().unwrap();
    assert_monitor_task_text_is_sanitized(subject, description);
    assert!(description.contains("帮我买个东西路过超市的时候"));

    let persisted: Value =
        serde_json::from_str(&fs::read_to_string(&monitor_tasks_path).unwrap()).unwrap();
    let persisted_subject = persisted["tasks"][0]["subject"].as_str().unwrap();
    let persisted_description = persisted["tasks"][0]["description"].as_str().unwrap();
    assert_monitor_task_text_is_sanitized(persisted_subject, persisted_description);
}

fn assert_monitor_task_text_is_sanitized(subject: &str, description: &str) {
    for banned in [
        "telegram",
        "message asks",
        "message says",
        "actionable request",
        "need a reply",
        "sender is asking",
    ] {
        let lowered_subject = subject.to_ascii_lowercase();
        let lowered_description = description.to_ascii_lowercase();
        assert!(
            !lowered_subject.contains(banned),
            "subject contains {banned}: {subject}"
        );
        assert!(
            !lowered_description.contains(banned),
            "description contains {banned}: {description}"
        );
    }
    assert!(
        !description.to_ascii_lowercase().contains("likely "),
        "description contains likely judgment language: {description}"
    );
    assert!(
        !description.contains("\"."),
        "description contains leftover quote punctuation: {description}"
    );
}
