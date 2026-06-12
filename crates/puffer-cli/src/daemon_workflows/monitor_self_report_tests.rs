use super::*;

use puffer_subscriber_runtime::Event;
use serde_json::json;

/// A judge whose behaviour is fixed by the test. When `panic_on_call` is set it
/// blows up if invoked, proving the cheap gate short-circuited before any LLM
/// call would have happened.
struct StubJudge {
    result: Option<String>,
    panic_on_call: bool,
}

impl StubJudge {
    fn never() -> Self {
        Self {
            result: None,
            panic_on_call: true,
        }
    }

    fn returning(task_id: Option<&str>) -> Self {
        Self {
            result: task_id.map(ToOwned::to_owned),
            panic_on_call: false,
        }
    }
}

impl CompletionJudge for StubJudge {
    fn judge(
        &self,
        _model: Option<&str>,
        _tasks: &[OpenTask],
        _message: &str,
        _hints: &[String],
    ) -> Option<String> {
        if self.panic_on_call {
            panic!("judge must not be invoked when the conversation has no open task");
        }
        self.result.clone()
    }
}

fn self_envelope(chat_id: i64, text: &str) -> EventEnvelope {
    EventEnvelope {
        envelope_id: "env".into(),
        subscriber_id: "telegram-user".into(),
        received_at_ms: 0,
        event: Event {
            topic: "telegram-user".into(),
            kind: puffer_subscriptions::SELF_MESSAGE_KIND.into(),
            control: false,
            dedup_key: None,
            text: text.into(),
            payload: json!({ "chat_id": chat_id, "is_outgoing": true }),
        },
    }
}

fn write_tasks(paths: &ConfigPaths, tasks: Value) {
    let path = monitor_tasks_path(paths);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_string_pretty(&json!({ "tasks": tasks })).unwrap())
        .unwrap();
}

fn task_status(paths: &ConfigPaths, task_id: &str) -> Option<String> {
    let raw = std::fs::read_to_string(monitor_tasks_path(paths)).unwrap();
    let store: Value = serde_json::from_str(&raw).unwrap();
    store
        .get("tasks")?
        .as_array()?
        .iter()
        .find(|task| task.get("task_id").and_then(Value::as_str) == Some(task_id))
        .and_then(|task| task.get("status").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn monitor_task(task_id: &str, chat_id: i64, status: &str) -> Value {
    json!({
        "task_id": task_id,
        "subject": format!("Task {task_id}"),
        "description": "An open monitor task.",
        "status": status,
        "metadata": {
            "_monitor": true,
            "monitor_connection": "telegram-user",
            "chat_id": chat_id,
        },
    })
}

/// A judge that records the hints it was handed, so a test can assert that
/// per-monitor configured phrases were merged in alongside the defaults.
struct CapturingJudge {
    hints: std::sync::Mutex<Vec<String>>,
    result: Option<String>,
}

impl CompletionJudge for CapturingJudge {
    fn judge(
        &self,
        _model: Option<&str>,
        _tasks: &[OpenTask],
        _message: &str,
        hints: &[String],
    ) -> Option<String> {
        *self.hints.lock().unwrap() = hints.to_vec();
        self.result.clone()
    }
}

#[test]
fn configured_phrases_merge_into_hints_alongside_defaults() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    write_tasks(&paths, json!([monitor_task("monitor-1", 42, "pending")]));
    super::monitor_completion_phrases::handle_monitor_completion_phrase_add(
        &paths,
        &json!({ "connectionSlug": "telegram-user", "phrase": "快递拿了" }),
    )
    .unwrap();

    let judge = CapturingJudge {
        hints: std::sync::Mutex::new(Vec::new()),
        result: None,
    };
    process_self_report(&paths, &self_envelope(42, "快递拿了"), &judge);

    let hints = judge.hints.lock().unwrap();
    assert!(hints.iter().any(|h| h == "快递拿了"), "configured phrase merged");
    assert!(hints.iter().any(|h| h == "done"), "default phrase retained");
}

#[test]
fn no_open_task_in_conversation_spends_zero_tokens() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    // The conversation 42 has a task, but the self-message arrives in chat 99.
    write_tasks(&paths, json!([monitor_task("monitor-1", 42, "pending")]));

    // The judge panics if invoked; reaching it would fail the test.
    process_self_report(&paths, &self_envelope(99, "搞定了"), &StubJudge::never());

    assert_eq!(task_status(&paths, "monitor-1").as_deref(), Some("pending"));
}

#[test]
fn missing_task_store_spends_zero_tokens() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());

    process_self_report(&paths, &self_envelope(42, "done"), &StubJudge::never());
}

#[test]
fn completed_and_ignored_tasks_do_not_reach_the_judge() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    let mut ignored = monitor_task("monitor-ignored", 42, "pending");
    ignored["metadata"]["ignored"] = json!(true);
    write_tasks(
        &paths,
        json!([monitor_task("monitor-done", 42, "completed"), ignored]),
    );

    // Only terminal/ignored tasks exist for this chat, so the gate is empty.
    process_self_report(&paths, &self_envelope(42, "done"), &StubJudge::never());
}

#[test]
fn single_open_task_yes_marks_completed_via_self_report() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    write_tasks(&paths, json!([monitor_task("monitor-1", 42, "pending")]));

    process_self_report(
        &paths,
        &self_envelope(42, "搞定了"),
        &StubJudge::returning(Some("monitor-1")),
    );

    assert_eq!(task_status(&paths, "monitor-1").as_deref(), Some("completed"));
    let raw = std::fs::read_to_string(monitor_tasks_path(&paths)).unwrap();
    assert!(raw.contains("self_report"));
}

#[test]
fn single_open_task_no_leaves_task_untouched() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    write_tasks(&paths, json!([monitor_task("monitor-1", 42, "pending")]));

    process_self_report(
        &paths,
        &self_envelope(42, "随便聊聊"),
        &StubJudge::returning(None),
    );

    assert_eq!(task_status(&paths, "monitor-1").as_deref(), Some("pending"));
}

#[test]
fn multiple_open_tasks_only_the_selected_one_completes() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    write_tasks(
        &paths,
        json!([
            monitor_task("monitor-1", 42, "pending"),
            monitor_task("monitor-2", 42, "in_progress"),
        ]),
    );

    process_self_report(
        &paths,
        &self_envelope(42, "快递拿了"),
        &StubJudge::returning(Some("monitor-2")),
    );

    assert_eq!(task_status(&paths, "monitor-1").as_deref(), Some("pending"));
    assert_eq!(task_status(&paths, "monitor-2").as_deref(), Some("completed"));
}

#[test]
fn judge_result_outside_open_set_is_ignored() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    write_tasks(&paths, json!([monitor_task("monitor-1", 42, "pending")]));

    // The judge returns an id the gate never offered; nothing must be completed.
    process_self_report(
        &paths,
        &self_envelope(42, "done"),
        &StubJudge::returning(Some("monitor-999")),
    );

    assert_eq!(task_status(&paths, "monitor-1").as_deref(), Some("pending"));
}

#[test]
fn empty_self_message_spends_zero_tokens() {
    let tempdir = tempfile::tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    write_tasks(&paths, json!([monitor_task("monitor-1", 42, "pending")]));

    process_self_report(&paths, &self_envelope(42, "   "), &StubJudge::never());

    assert_eq!(task_status(&paths, "monitor-1").as_deref(), Some("pending"));
}

#[test]
fn chat_id_matches_across_string_and_number_shapes() {
    let task = json!({
        "task_id": "monitor-1",
        "metadata": { "chat_id": "42" },
    });
    assert_eq!(task_chat_id(&task).as_deref(), Some("42"));

    let payload = json!({ "chat_id": 42 });
    assert_eq!(extract_chat_id(&payload).as_deref(), Some("42"));
}

#[test]
fn task_index_parsing_tolerates_surrounding_text() {
    assert_eq!(parse_task_index("2"), Some(2));
    assert_eq!(parse_task_index("Task 3 is done"), Some(3));
    assert_eq!(parse_task_index("none"), None);
    assert_eq!(parse_task_index("0"), Some(0));
}
