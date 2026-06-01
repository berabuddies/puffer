use super::*;
use puffer_subscriber_runtime::Event;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tempfile::tempdir;

fn envelope(text: &str, payload: serde_json::Value) -> EventEnvelope {
    EventEnvelope {
        envelope_id: "env-1".into(),
        subscriber_id: "telegram-user".into(),
        received_at_ms: 0,
        event: Event {
            topic: "telegram-user".into(),
            kind: "message".into(),
            control: false,
            dedup_key: None,
            text: text.into(),
            payload,
        },
    }
}

#[test]
fn sqlite_insert_creates_table_and_inserts_row() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("x.db");
    let dispatcher = BuiltinActionDispatcher::with_storage_root(dir.path());
    let action = ActionSpec::SqliteInsert {
        path: "x.db".to_string(),
        table: "ioc_messages".into(),
    };
    let result = dispatcher.dispatch(&action, &envelope("hello", json!({"chat":"@x"})));
    assert!(result.success, "{}", result.summary);
    let conn = Connection::open(&db).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM ioc_messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn sqlite_insert_rejects_absolute_and_traversal_paths() {
    let dir = tempdir().unwrap();
    let dispatcher = BuiltinActionDispatcher::with_storage_root(dir.path());
    for path in [
        "/tmp/puffer-subscriptions.db",
        "~/puffer.db",
        "../puffer.db",
    ] {
        let action = ActionSpec::SqliteInsert {
            path: path.to_string(),
            table: "ioc_messages".into(),
        };
        let result = dispatcher.dispatch(&action, &envelope("hello", json!({})));
        assert!(!result.success, "{path} should be rejected");
    }
}

#[test]
fn file_append_text_preserves_shell_sensitive_message_text() {
    let dir = tempdir().unwrap();
    let dispatcher = BuiltinActionDispatcher::with_storage_root(dir.path());
    let action = ActionSpec::FileAppend {
        path: "msgs".to_string(),
        format: FileAppendFormat::Text,
    };
    let result = dispatcher.dispatch(
        &action,
        &envelope("McDonald's && $(rm -rf /)", json!({"chat":"@x"})),
    );

    assert!(result.success, "{}", result.summary);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("msgs")).unwrap(),
        "McDonald's && $(rm -rf /)\n"
    );
}

#[test]
fn file_append_jsonl_records_event_payload() {
    let dir = tempdir().unwrap();
    let dispatcher = BuiltinActionDispatcher::with_storage_root(dir.path());
    let action = ActionSpec::FileAppend {
        path: "msgs.jsonl".to_string(),
        format: FileAppendFormat::Jsonl,
    };
    let result = dispatcher.dispatch(
        &action,
        &envelope("hello", json!({"chat_id": 42, "is_outgoing": false})),
    );

    assert!(result.success, "{}", result.summary);
    let line = std::fs::read_to_string(dir.path().join("msgs.jsonl")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(parsed["text"], "hello");
    assert_eq!(parsed["payload"]["chat_id"], 42);
    assert_eq!(parsed["payload"]["is_outgoing"], false);
}

#[test]
fn file_append_accepts_absolute_tmp_path_at_runtime() {
    let dir = tempdir().unwrap();
    let dispatcher = BuiltinActionDispatcher::with_storage_root(dir.path());
    let path = format!("/tmp/puffer-file-append-test-{}", std::process::id());
    let _ = std::fs::remove_file(&path);
    let action = ActionSpec::FileAppend {
        path: path.clone(),
        format: FileAppendFormat::Text,
    };
    let result = dispatcher.dispatch(&action, &envelope("hello", json!({})));

    assert!(result.success, "{}", result.summary);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn file_append_rejects_absolute_paths_outside_tmp() {
    let dir = tempdir().unwrap();
    let dispatcher = BuiltinActionDispatcher::with_storage_root(dir.path());
    let action = ActionSpec::FileAppend {
        path: "/etc/puffer-msgs".to_string(),
        format: FileAppendFormat::Text,
    };
    let result = dispatcher.dispatch(&action, &envelope("hello", json!({})));

    assert!(!result.success);
    assert!(result.summary.contains("under /tmp"));
}

struct RecordingOutbound {
    calls: StdMutex<Vec<(String, String, String)>>,
}

impl Outbound for RecordingOutbound {
    fn send(&self, platform: &str, target: &str, text: &str) -> Result<String> {
        self.calls.lock().unwrap().push((
            platform.to_string(),
            target.to_string(),
            text.to_string(),
        ));
        Ok(format!("recorded {platform}:{target}"))
    }
}

#[test]
fn forward_message_calls_installed_outbound_with_rendered_template() {
    let dispatcher = BuiltinActionDispatcher::new();
    let outbound = Arc::new(RecordingOutbound {
        calls: StdMutex::new(Vec::new()),
    });
    dispatcher.set_outbound(outbound.clone());
    let action = ActionSpec::ForwardMessage {
        platform: "telegram".into(),
        target: "@hongyi_zhang".into(),
        template: Some("Zara: {{text}} ({{payload.subject}})".into()),
    };
    let result = dispatcher.dispatch(
        &action,
        &envelope("50% off jeans", json!({"subject": "Sale today"})),
    );
    assert!(result.success, "{}", result.summary);
    let calls = outbound.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "telegram");
    assert_eq!(calls[0].1, "@hongyi_zhang");
    assert_eq!(calls[0].2, "Zara: 50% off jeans (Sale today)");
}

#[test]
fn forward_message_without_outbound_reports_actionable_error() {
    let dispatcher = BuiltinActionDispatcher::new();
    let action = ActionSpec::ForwardMessage {
        platform: "telegram".into(),
        target: "@hongyi_zhang".into(),
        template: None,
    };
    let result = dispatcher.dispatch(&action, &envelope("hi", json!({})));
    assert!(!result.success);
    assert!(result.summary.contains("no outbound is installed"));
}

struct RecordingWorkflowRunner {
    calls: StdMutex<Vec<(String, serde_json::Value)>>,
}

impl WorkflowActionRunner for RecordingWorkflowRunner {
    fn run_workflow(&self, slug: &str, trigger: serde_json::Value) -> Result<WorkflowActionOutput> {
        self.calls.lock().unwrap().push((slug.to_string(), trigger));
        Ok(WorkflowActionOutput::new(format!("ran {slug}")))
    }
}

#[test]
fn run_workflow_dispatches_connection_trigger() {
    let dispatcher = BuiltinActionDispatcher::new();
    let runner = Arc::new(RecordingWorkflowRunner {
        calls: StdMutex::new(Vec::new()),
    });
    dispatcher.set_workflow_runner(runner.clone());
    let action = ActionSpec::RunWorkflow {
        slug: "daily-review".into(),
    };
    let result = dispatcher.dispatch(&action, &envelope("hello", json!({"chat":"@x"})));
    assert!(result.success, "{}", result.summary);
    let calls = runner.calls.lock().unwrap();
    assert_eq!(calls[0].0, "daily-review");
    assert_eq!(calls[0].1["type"], "connection");
    assert_eq!(calls[0].1["connection_id"], "telegram-user");
    assert_eq!(calls[0].1["receivedAt"], "1970-01-01T00:00:00Z");
    assert!(calls[0].1.get("received_at").is_none());
    assert!(calls[0].1.get("received_at_ms").is_none());
    assert_eq!(calls[0].1["text"], "hello");
}

struct RecordingTriageRunner {
    calls: StdMutex<Vec<(String, Option<String>, serde_json::Value)>>,
    batch_calls: StdMutex<Vec<(String, Option<String>, Vec<serde_json::Value>)>>,
}

impl WorkflowActionRunner for RecordingTriageRunner {
    fn run_workflow(
        &self,
        slug: &str,
        _trigger: serde_json::Value,
    ) -> Result<WorkflowActionOutput> {
        Ok(WorkflowActionOutput::new(format!("unused {slug}")))
    }

    fn triage_agent(
        &self,
        prompt: &str,
        model: Option<&str>,
        trigger: serde_json::Value,
    ) -> Result<WorkflowActionOutput> {
        self.calls.lock().unwrap().push((
            prompt.to_string(),
            model.map(ToOwned::to_owned),
            trigger,
        ));
        Ok(WorkflowActionOutput::with_usage(
            "triaged",
            Some(ActionUsage {
                input_tokens: 12,
                output_tokens: 3,
                cache_read_tokens: 2,
                cache_creation_tokens: 0,
            }),
        ))
    }

    fn triage_agent_batch(
        &self,
        prompt: &str,
        model: Option<&str>,
        triggers: Vec<serde_json::Value>,
    ) -> Result<WorkflowActionOutput> {
        self.batch_calls.lock().unwrap().push((
            prompt.to_string(),
            model.map(ToOwned::to_owned),
            triggers,
        ));
        Ok(WorkflowActionOutput::with_usage(
            "triaged batch",
            Some(ActionUsage {
                input_tokens: 20,
                output_tokens: 4,
                cache_read_tokens: 8,
                cache_creation_tokens: 1,
            }),
        ))
    }
}

#[test]
fn triage_agent_dispatches_rendered_prompt_and_connection_trigger() {
    let dispatcher = BuiltinActionDispatcher::new();
    let runner = Arc::new(RecordingTriageRunner {
        calls: StdMutex::new(Vec::new()),
        batch_calls: StdMutex::new(Vec::new()),
    });
    dispatcher.set_workflow_runner(runner.clone());
    let action = ActionSpec::TriageAgent {
        prompt: "Review {{text}} from {{payload.sender.name}}".into(),
        model: Some("fast-monitor".into()),
    };
    let result = dispatcher.dispatch(
        &action,
        &envelope("deploy?", json!({"sender": {"name": "Alice"}})),
    );

    assert!(result.success, "{}", result.summary);
    assert_eq!(result.summary, "triaged");
    assert_eq!(result.usage.map(|usage| usage.spent_tokens()), Some(13));
    let calls = runner.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "Review deploy? from Alice");
    assert_eq!(calls[0].1.as_deref(), Some("fast-monitor"));
    assert_eq!(calls[0].2["type"], "connection");
    assert_eq!(calls[0].2["connection_id"], "telegram-user");
    assert_eq!(calls[0].2["receivedAt"], "1970-01-01T00:00:00Z");
    assert!(calls[0].2.get("received_at").is_none());
    assert!(calls[0].2.get("received_at_ms").is_none());
    assert_eq!(calls[0].2["text"], "deploy?");
}

#[test]
fn triage_batch_dispatches_one_runner_call_with_all_triggers() {
    let dispatcher = BuiltinActionDispatcher::new();
    let runner = Arc::new(RecordingTriageRunner {
        calls: StdMutex::new(Vec::new()),
        batch_calls: StdMutex::new(Vec::new()),
    });
    dispatcher.set_workflow_runner(runner.clone());
    let action = ActionSpec::TriageAgent {
        prompt: "Review {{text}}".into(),
        model: Some("fast-monitor".into()),
    };
    let envelopes = vec![
        envelope("first", json!({"sender": {"name": "Alice"}})),
        envelope("second", json!({"sender": {"name": "Bob"}})),
    ];

    let result = dispatcher.dispatch_batch(&action, &envelopes);

    assert!(result.success, "{}", result.summary);
    assert_eq!(result.summary, "triaged batch");
    assert_eq!(result.usage.map(|usage| usage.spent_tokens()), Some(16));
    assert!(runner.calls.lock().unwrap().is_empty());
    let calls = runner.batch_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "Review first");
    assert_eq!(calls[0].1.as_deref(), Some("fast-monitor"));
    assert_eq!(calls[0].2.len(), 2);
    assert_eq!(calls[0].2[0]["text"], "first");
    assert_eq!(calls[0].2[1]["text"], "second");
}

struct RecordingConnectorExecutor {
    calls: StdMutex<Vec<(String, String, serde_json::Value)>>,
}

impl ConnectorActionExecutor for RecordingConnectorExecutor {
    fn run_connector_action(
        &self,
        connector_slug: &str,
        action: &str,
        input: serde_json::Value,
        _trigger: serde_json::Value,
    ) -> Result<String> {
        self.calls
            .lock()
            .unwrap()
            .push((connector_slug.to_string(), action.to_string(), input));
        Ok(format!("ran {connector_slug}.{action}"))
    }
}

#[test]
fn connector_act_uses_installed_executor() {
    let dispatcher = BuiltinActionDispatcher::new();
    let executor = Arc::new(RecordingConnectorExecutor {
        calls: StdMutex::new(Vec::new()),
    });
    dispatcher.set_connector_action_executor(executor.clone());
    let action = ActionSpec::ConnectorAct {
        connector_slug: "demo-connector".into(),
        action: "archive".into(),
        input: json!({"message":"saved {{text}}"}),
    };
    let result = dispatcher.dispatch(&action, &envelope("hello", json!({})));

    assert!(result.success, "{}", result.summary);
    let calls = executor.calls.lock().unwrap();
    assert_eq!(calls[0].0, "demo-connector");
    assert_eq!(calls[0].1, "archive");
    assert_eq!(calls[0].2, json!({"message":"saved hello"}));
}
