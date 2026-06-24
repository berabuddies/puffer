use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::Event;
use puffer_subscriptions::SelfMessageGate;
use serde_json::Value;

use super::monitor_task_ignore::monitor_tasks_path;

/// Gate that dispatches self/outgoing events to the monitor triage agent only
/// when the event's conversation has at least one OPEN (not completed, not
/// cancelled) monitor task in the monitor task store.
///
/// The conversation is identified by whichever connector-specific identity key
/// the event payload carries (`chat_id`, `channel_id`, `room_id`,
/// `conversation_id`, `thread_id`, `mailbox_id`, `project_id`, or WeChat's bare
/// `chat`). It is matched key-by-key against the same key stamped on a task's
/// metadata, so the gate is connector-agnostic: any connector that emits
/// `is_outgoing` plus one of these keys is supported with no further gate
/// changes.
///
/// On any error (missing file, parse failure, no identity key) returns `false`
/// (drop), which is the safe #569-preserving default.
pub(crate) struct MonitorSelfGate {
    paths: ConfigPaths,
}

impl MonitorSelfGate {
    pub(crate) fn new(paths: ConfigPaths) -> Self {
        Self { paths }
    }
}

impl SelfMessageGate for MonitorSelfGate {
    fn should_dispatch_self_message(&self, event: &Event) -> bool {
        // The event must carry at least one conversation-identity key, else we
        // cannot scope it to a conversation → drop (safe default).
        let payload_ids = conversation_ids(&event.payload);
        if payload_ids.is_empty() {
            return false;
        }
        let path = monitor_tasks_path(&self.paths);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return false;
        };
        let Ok(store) = serde_json::from_str::<Value>(&raw) else {
            return false;
        };
        store
            .get("tasks")
            .and_then(Value::as_array)
            .is_some_and(|tasks| {
                tasks.iter().any(|t| {
                    let open = !matches!(
                        t.get("status").and_then(Value::as_str),
                        Some("completed") | Some("cancelled")
                    );
                    open && task_shares_conversation(t, &payload_ids)
                })
            })
    }
}

/// Conversation-identity keys a connector may use to scope a monitor task to a
/// conversation. The triage protocol stamps one of these onto each task's
/// metadata, and the matching key appears in the connector's event payload.
/// (`chat` is WeChat's bare key.)
const CONVERSATION_ID_KEYS: &[&str] = &[
    "chat_id",
    "channel_id",
    "room_id",
    "conversation_id",
    "thread_id",
    "mailbox_id",
    "project_id",
    "chat",
];

/// Collect the `(key, normalised value)` conversation-identity pairs present in
/// a JSON object, scanning [`CONVERSATION_ID_KEYS`]. Applied to both the event
/// payload and a task's `metadata` so matching is key-by-key.
fn conversation_ids(obj: &Value) -> Vec<(&'static str, String)> {
    CONVERSATION_ID_KEYS
        .iter()
        .filter_map(|key| obj.get(*key).and_then(value_to_string).map(|v| (*key, v)))
        .collect()
}

/// Whether a task shares a conversation-identity key/value with the event
/// payload (same key, equal normalised value). Key-by-key matching avoids
/// cross-key collisions (a `chat_id` never matches a `channel_id`).
fn task_shares_conversation(task: &Value, payload_ids: &[(&'static str, String)]) -> bool {
    let Some(metadata) = task.get("metadata") else {
        return false;
    };
    let task_ids = conversation_ids(metadata);
    payload_ids
        .iter()
        .any(|(pk, pv)| task_ids.iter().any(|(tk, tv)| pk == tk && pv == tv))
}

/// Convert a `&Value` to a canonical string used for conversation-id comparison.
/// - `Value::String`: returned if non-empty after trimming.
/// - `Value::Number`: stringified via `to_string()`.
/// - Everything else: `None`.
fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => {
            let trimmed = s.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::ConfigPaths;
    use puffer_subscriber_runtime::EventEnvelope;
    use puffer_subscriptions::{
        process_envelope_result, ActionDispatcher, ActionResult, ActionSpec, Classifier,
        ClassifyDecision, WorkflowBindingSpec, WorkflowBindingStatus, WorkflowBindingStore,
    };
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn make_event(payload: Value) -> Event {
        Event {
            topic: "telegram-user".into(),
            kind: "message".into(),
            control: false,
            dedup_key: None,
            text: String::new(),
            payload,
        }
    }

    fn write_task_store(paths: &ConfigPaths, store: &Value) {
        let path = monitor_tasks_path(paths);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_string_pretty(store).unwrap()).unwrap();
    }

    #[test]
    fn gate_true_only_when_chat_has_open_monitor_task() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let gate = Arc::new(MonitorSelfGate::new(paths.clone()));

        // Write store with one open task for chat 42.
        write_task_store(
            &paths,
            &json!({
                "tasks": [{
                    "task_id": "t1",
                    "status": "pending",
                    "metadata": { "_monitor": true, "chat_id": 42 }
                }]
            }),
        );

        let event_42 = make_event(json!({ "chat_id": 42, "is_outgoing": true }));
        let event_99 = make_event(json!({ "chat_id": 99, "is_outgoing": true }));

        // Chat 42 has an open task → dispatch.
        assert!(gate.should_dispatch_self_message(&event_42));
        // Chat 99 has no task → drop.
        assert!(!gate.should_dispatch_self_message(&event_99));

        // Now mark the task completed → should become false for chat 42.
        write_task_store(
            &paths,
            &json!({
                "tasks": [{
                    "task_id": "t1",
                    "status": "completed",
                    "metadata": { "_monitor": true, "chat_id": 42 }
                }]
            }),
        );
        assert!(!gate.should_dispatch_self_message(&event_42));
    }

    #[test]
    fn gate_matches_chat_id_across_number_and_string_representation() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let gate = Arc::new(MonitorSelfGate::new(paths.clone()));

        // Store has numeric chat_id; event has string "42".
        write_task_store(
            &paths,
            &json!({
                "tasks": [{
                    "task_id": "t2",
                    "status": "pending",
                    "metadata": { "_monitor": true, "chat_id": 42 }
                }]
            }),
        );
        let event_str = make_event(json!({ "chat_id": "42", "is_outgoing": true }));
        assert!(gate.should_dispatch_self_message(&event_str));

        // Reverse: store has string "42"; event has numeric 42.
        write_task_store(
            &paths,
            &json!({
                "tasks": [{
                    "task_id": "t3",
                    "status": "pending",
                    "metadata": { "_monitor": true, "chat_id": "42" }
                }]
            }),
        );
        let event_num = make_event(json!({ "chat_id": 42, "is_outgoing": true }));
        assert!(gate.should_dispatch_self_message(&event_num));
    }

    #[test]
    fn gate_false_when_no_store_or_no_chat_id() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let gate = Arc::new(MonitorSelfGate::new(paths.clone()));

        // No store file at all → false.
        let event = make_event(json!({ "chat_id": 42, "is_outgoing": true }));
        assert!(!gate.should_dispatch_self_message(&event));

        // Write a valid store so the file exists, but event has no chat_id → false.
        write_task_store(
            &paths,
            &json!({
                "tasks": [{
                    "task_id": "t4",
                    "status": "pending",
                    "metadata": { "_monitor": true, "chat_id": 42 }
                }]
            }),
        );
        let event_no_chat_id = make_event(json!({ "is_outgoing": true }));
        assert!(!gate.should_dispatch_self_message(&event_no_chat_id));
    }

    #[test]
    fn gate_matches_non_chat_id_conversation_keys() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let gate = Arc::new(MonitorSelfGate::new(paths.clone()));

        // Email-style task keyed on thread_id; WeChat-style on the bare `chat`.
        write_task_store(
            &paths,
            &json!({
                "tasks": [
                    { "task_id": "mail", "status": "pending",
                      "metadata": { "_monitor": true, "thread_id": "abc-123" } },
                    { "task_id": "wechat", "status": "pending",
                      "metadata": { "_monitor": true, "chat": 7788 } }
                ]
            }),
        );

        // thread_id match (email).
        assert!(gate.should_dispatch_self_message(&make_event(
            json!({ "thread_id": "abc-123", "is_outgoing": true })
        )));
        // bare `chat` key (WeChat); numeric on both sides.
        assert!(gate.should_dispatch_self_message(&make_event(
            json!({ "chat": 7788, "is_outgoing": true })
        )));
        // A different thread_id → drop.
        assert!(!gate.should_dispatch_self_message(&make_event(
            json!({ "thread_id": "other", "is_outgoing": true })
        )));
    }

    #[test]
    fn gate_does_not_match_across_different_identity_keys() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let gate = Arc::new(MonitorSelfGate::new(paths.clone()));

        // Task keyed on channel_id = 42.
        write_task_store(
            &paths,
            &json!({
                "tasks": [{ "task_id": "c", "status": "pending",
                            "metadata": { "_monitor": true, "channel_id": 42 } }]
            }),
        );
        // Event carries chat_id = 42 (a DIFFERENT key) → must NOT match.
        assert!(!gate.should_dispatch_self_message(&make_event(
            json!({ "chat_id": 42, "is_outgoing": true })
        )));
        // Same key (channel_id) → matches.
        assert!(gate.should_dispatch_self_message(&make_event(
            json!({ "channel_id": 42, "is_outgoing": true })
        )));
    }

    // -------------------------------------------------------------------------
    // Integration Test A: real MonitorSelfGate wired through real process_envelope_result
    //
    // This is the high-value seam test: it proves that the gate + router
    // actually drop/allow outgoing envelopes end-to-end, without an LLM.
    // A PanicClassifier proves the self/outgoing path never reaches classify.
    // -------------------------------------------------------------------------

    /// Test double: records dispatch calls and reports success.
    struct CountingDispatcher {
        calls: Arc<AtomicUsize>,
    }
    impl ActionDispatcher for CountingDispatcher {
        fn dispatch(&self, _action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ActionResult::success("dispatched")
        }
    }

    /// Test double: panics if `classify` runs. Proves the self/outgoing path
    /// short-circuits the classifier (the #569 credit-burn guard).
    struct PanicClassifier;
    impl Classifier for PanicClassifier {
        fn classify(
            &self,
            _spec: &WorkflowBindingSpec,
            _event: &puffer_subscriber_runtime::Event,
        ) -> ClassifyDecision {
            panic!("classifier must not run for self/outgoing events");
        }
    }

    fn monitor_binding_spec() -> WorkflowBindingSpec {
        WorkflowBindingSpec {
            slug: "monitor-telegram-user".into(),
            description: "Monitor telegram-user for actionable tasks".into(),
            connection_slug: "telegram-user".into(),
            connector_slug: Some("telegram-login".into()),
            status: WorkflowBindingStatus::Enabled,
            filter: None,
            ignore_filters: Vec::new(),
            contact_ids: Vec::new(),
            classify_prompt: Some("classify".into()),
            classify_model: None,
            action: ActionSpec::TriageAgent {
                prompt: "triage".into(),
                model: None,
            },
            created_at_ms: 0,
        }
    }

    fn outgoing_envelope_for(chat_id: i64) -> EventEnvelope {
        EventEnvelope {
            envelope_id: format!("env-outgoing-{chat_id}"),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: puffer_subscriber_runtime::Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "done, just sent it".into(),
                payload: json!({ "is_outgoing": true, "chat_id": chat_id }),
            },
        }
    }

    /// Integration: outgoing envelope for chat 42 (open task) → dispatched once,
    /// classifier never called.
    #[test]
    fn integration_real_gate_allows_outgoing_when_chat_has_open_task() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());

        // Write monitor_tasks.json with one OPEN task for chat_id 42.
        write_task_store(
            &paths,
            &json!({
                "tasks": [{
                    "task_id": "monitor-1",
                    "status": "pending",
                    "metadata": { "_monitor": true, "chat_id": 42 }
                }]
            }),
        );

        let gate: Arc<dyn SelfMessageGate> = Arc::new(MonitorSelfGate::new(paths.clone()));
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(CountingDispatcher {
            calls: calls.clone(),
        });
        let classifier: Arc<dyn Classifier> = Arc::new(PanicClassifier);

        let store = WorkflowBindingStore::load(tempdir.path().join("bindings.json")).unwrap();
        store.create(monitor_binding_spec()).unwrap();

        let result = process_envelope_result(
            &outgoing_envelope_for(42),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
            &gate,
        );

        assert!(result.matched, "chat 42 has open task — must dispatch");
        assert_eq!(result.acted, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "dispatcher runs exactly once"
        );
        // PanicClassifier did not panic → classifier was bypassed for self event.
    }

    /// Integration: outgoing envelope for chat 99 (no open task) → dropped,
    /// dispatcher never called, classifier never called.
    #[test]
    fn integration_real_gate_drops_outgoing_when_chat_has_no_open_task() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());

        // Write monitor_tasks.json with an open task for chat 42, NOT chat 99.
        write_task_store(
            &paths,
            &json!({
                "tasks": [{
                    "task_id": "monitor-1",
                    "status": "pending",
                    "metadata": { "_monitor": true, "chat_id": 42 }
                }]
            }),
        );

        let gate: Arc<dyn SelfMessageGate> = Arc::new(MonitorSelfGate::new(paths.clone()));
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(CountingDispatcher {
            calls: calls.clone(),
        });
        let classifier: Arc<dyn Classifier> = Arc::new(PanicClassifier);

        let store = WorkflowBindingStore::load(tempdir.path().join("bindings.json")).unwrap();
        store.create(monitor_binding_spec()).unwrap();

        let result = process_envelope_result(
            &outgoing_envelope_for(99),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
            &gate,
        );

        assert!(
            !result.matched,
            "chat 99 has no open task — must not dispatch"
        );
        assert_eq!(result.acted, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "dispatcher must not run");
        // PanicClassifier did not panic → classifier was bypassed.
    }

    /// Integration: after marking the chat-42 task completed, the gate must
    /// drop the next outgoing envelope for that chat.
    #[test]
    fn integration_real_gate_drops_outgoing_after_task_completed() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());

        // Start with an OPEN task for chat 42.
        write_task_store(
            &paths,
            &json!({
                "tasks": [{
                    "task_id": "monitor-1",
                    "status": "pending",
                    "metadata": { "_monitor": true, "chat_id": 42 }
                }]
            }),
        );

        let gate: Arc<dyn SelfMessageGate> = Arc::new(MonitorSelfGate::new(paths.clone()));
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(CountingDispatcher {
            calls: calls.clone(),
        });
        let classifier: Arc<dyn Classifier> = Arc::new(PanicClassifier);

        let store = WorkflowBindingStore::load(tempdir.path().join("bindings.json")).unwrap();
        store.create(monitor_binding_spec()).unwrap();

        // First pass: open task → dispatched.
        let first = process_envelope_result(
            &outgoing_envelope_for(42),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
            &gate,
        );
        assert!(first.matched, "open task — first outgoing must dispatch");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Simulate task completion: writeback the store with status=completed.
        write_task_store(
            &paths,
            &json!({
                "tasks": [{
                    "task_id": "monitor-1",
                    "status": "completed",
                    "completed_via": "agent_report:incoming",
                    "metadata": { "_monitor": true, "chat_id": 42 }
                }]
            }),
        );

        // Second pass: completed task → gate must drop.
        let second = process_envelope_result(
            &outgoing_envelope_for(42),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
            &gate,
        );
        assert!(
            !second.matched,
            "task completed — subsequent outgoing must be dropped"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "dispatcher must not run after task completed"
        );
        // PanicClassifier never panicked — classifier bypassed on both passes.
    }
}
