use super::{handle_monitor_task_complete, monitor_tasks_path};
use puffer_config::ConfigPaths;
use serde_json::{json, Value};
use std::fs;

fn write_store(paths: &ConfigPaths, store: Value) {
    let path = monitor_tasks_path(paths);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, serde_json::to_string_pretty(&store).unwrap()).unwrap();
}

fn read_store(paths: &ConfigPaths) -> Value {
    serde_json::from_str(&fs::read_to_string(monitor_tasks_path(paths)).unwrap()).unwrap()
}

#[test]
fn complete_marks_pending_task_completed_via_reply() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    write_store(
        &paths,
        json!({
            "tasks": [{
                "task_id": "monitor-1",
                "subject": "Telegram request: buy a burger",
                "description": "reply please",
                "status": "pending",
                "metadata": {"_monitor": true, "monitor_connection": "telegram-user"},
                "started_at_ms": 10
            }]
        }),
    );

    let snapshot =
        handle_monitor_task_complete(&paths, &json!({"task_id": "monitor-1"})).unwrap();
    let tasks = snapshot["tasks"].as_array().unwrap();
    assert_eq!(tasks[0]["task_id"], "monitor-1");
    assert_eq!(tasks[0]["status"], "completed");

    let stored = read_store(&paths);
    assert_eq!(stored["tasks"][0]["status"], "completed");
    assert_eq!(stored["tasks"][0]["completed_via"], "reply");
    // No ignore side effects: completion is not an ignore.
    assert_eq!(stored["tasks"][0]["metadata"]["ignored"], Value::Null);
    assert_eq!(stored["tasks"][0]["metadata"]["ignore_reason"], Value::Null);
}

#[test]
fn complete_accepts_task_id_alias_and_explicit_completed_via() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    write_store(
        &paths,
        json!({
            "tasks": [{
                "task_id": "monitor-7",
                "subject": "s",
                "description": "d",
                "status": "pending",
                "metadata": {"_monitor": true}
            }]
        }),
    );

    handle_monitor_task_complete(&paths, &json!({"taskId": "monitor-7", "completed_via": "manual"}))
        .unwrap();

    let stored = read_store(&paths);
    assert_eq!(stored["tasks"][0]["status"], "completed");
    assert_eq!(stored["tasks"][0]["completed_via"], "manual");
}

#[test]
fn complete_is_idempotent_no_op_on_already_terminal() {
    // Backstop for agentenv/monorepo#561/#562 review (dev-4 Finding 2): a
    // duplicate or stray complete on an already-terminal task must be a harmless
    // no-op — it must not re-open the task and must not clobber the markers of a
    // task that was completed via ignore.
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    write_store(
        &paths,
        json!({
            "tasks": [{
                "task_id": "monitor-1",
                "subject": "already ignored",
                "description": "d",
                "status": "completed",
                "metadata": {"_monitor": true, "ignored": true, "ignore_reason": "user ignored"},
                "updated_at_ms": 42
            }]
        }),
    );

    handle_monitor_task_complete(&paths, &json!({"task_id": "monitor-1"})).unwrap();

    let stored = read_store(&paths);
    // Unchanged: still completed, ignore markers intact, no completed_via added,
    // updated_at_ms untouched.
    assert_eq!(stored["tasks"][0]["status"], "completed");
    assert_eq!(stored["tasks"][0]["metadata"]["ignored"], true);
    assert_eq!(stored["tasks"][0]["metadata"]["ignore_reason"], "user ignored");
    assert_eq!(stored["tasks"][0]["completed_via"], Value::Null);
    assert_eq!(stored["tasks"][0]["updated_at_ms"], 42);
}

#[test]
fn complete_unknown_task_id_errors() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    write_store(&paths, json!({"tasks": []}));

    let result = handle_monitor_task_complete(&paths, &json!({"task_id": "monitor-404"}));
    assert!(result.is_err());
}

#[test]
fn complete_missing_task_id_errors() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    write_store(&paths, json!({"tasks": []}));

    let result = handle_monitor_task_complete(&paths, &json!({"task_id": "   "}));
    assert!(result.is_err());
}
