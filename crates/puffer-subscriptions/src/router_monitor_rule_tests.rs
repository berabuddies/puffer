#[cfg(test)]
mod monitor_rule_tests {
    use super::*;
    use crate::action::{ActionResult, BuiltinActionDispatcher};
    use crate::classify::NullClassifier;
    use crate::spec::{ActionSpec, TaggedFilterSpec, WorkflowBindingSpec};
    use crate::{compile_event_field_rule, EventFieldRule, EventOperator, EventSchema};
    use puffer_subscriber_runtime::{Event, EventEnvelope};
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

    #[test]
    fn keyword_ignore_filter_suppresses_matching_text() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(WorkflowBindingSpec {
                slug: "monitor-telegram-user".into(),
                description: "Monitor telegram-user for actionable tasks".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: Some("telegram-login".into()),
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: vec![FilterSpec::Tagged(TaggedFilterSpec::Regex {
                    pattern: regex::escape("作业"),
                    case_insensitive: true,
                })],
                contact_ids: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::RunWorkflow {
                    slug: "downstream".into(),
                },
                created_at_ms: 0,
            })
            .unwrap();
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(BuiltinActionDispatcher::new());
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let history_store = WorkflowHistoryStore::load(dir.path().join("history.json")).unwrap();
        let matching = EventEnvelope {
            envelope_id: "env-matching-keyword".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "今天作业很多".into(),
                payload: serde_json::json!({"chat_id": 7_i64}),
            },
        };
        let non_matching = EventEnvelope {
            envelope_id: "env-non-matching-keyword".into(),
            event: Event {
                text: "今天正常消息".into(),
                ..matching.event.clone()
            },
            ..matching.clone()
        };

        let ignored = process_envelope_result(
            &matching,
            &store,
            Some(&history_store),
            &dispatcher,
            &classifier,
            None,
        );
        let passed =
            process_envelope_result(&non_matching, &store, None, &dispatcher, &classifier, None);

        assert!(!ignored.matched);
        assert_eq!(ignored.acted, 0);
        assert_eq!(
            history_store.list()[0].action_log[0].action,
            "monitor_ignore_filter"
        );
        assert!(passed.matched);
    }

    #[test]
    fn include_filter_skips_events_that_do_not_match_before_action() {
        struct OkDispatcher;

        impl ActionDispatcher for OkDispatcher {
            fn dispatch(&self, _action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
                ActionResult::success("ok")
            }
        }

        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(WorkflowBindingSpec {
                slug: "monitor-telegram-user".into(),
                description: "Monitor telegram-user for actionable tasks".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: Some("telegram-login".into()),
                status: WorkflowBindingStatus::Enabled,
                filter: Some(FilterSpec::Tagged(TaggedFilterSpec::Regex {
                    pattern: regex::escape("review"),
                    case_insensitive: true,
                })),
                ignore_filters: Vec::new(),
                contact_ids: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::RunWorkflow {
                    slug: "downstream".into(),
                },
                created_at_ms: 0,
            })
            .unwrap();
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(OkDispatcher);
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let base = EventEnvelope {
            envelope_id: "env-skip".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "hello".into(),
                payload: serde_json::json!({"chat_id": 7_i64}),
            },
        };
        let matching = EventEnvelope {
            envelope_id: "env-pass".into(),
            event: Event {
                text: "please review this".into(),
                ..base.event.clone()
            },
            ..base.clone()
        };

        let skipped = process_envelope_result(&base, &store, None, &dispatcher, &classifier, None);
        let passed =
            process_envelope_result(&matching, &store, None, &dispatcher, &classifier, None);

        assert!(!skipped.matched);
        assert_eq!(skipped.acted, 0);
        assert!(passed.matched);
        assert_eq!(passed.acted, 1);
    }

    struct RecordingDispatcher {
        topics: StdMutex<Vec<String>>,
    }

    impl ActionDispatcher for RecordingDispatcher {
        fn dispatch(&self, _action: &ActionSpec, envelope: &EventEnvelope) -> ActionResult {
            self.topics
                .lock()
                .unwrap()
                .push(envelope.event.topic.clone());
            ActionResult::success("triaged")
        }
    }

    fn recording_dispatcher() -> (Arc<RecordingDispatcher>, Arc<dyn ActionDispatcher>) {
        let dispatcher = Arc::new(RecordingDispatcher {
            topics: StdMutex::new(Vec::new()),
        });
        let erased: Arc<dyn ActionDispatcher> = dispatcher.clone();
        (dispatcher, erased)
    }

    fn bundled_schema(slug: &str) -> EventSchema {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root");
        crate::load_event_schema_from_dir(&root.join("resources").join("subscribers").join(slug))
            .unwrap()
            .unwrap_or_else(|| panic!("missing bundled event schema for {slug}"))
    }

    fn field_filter(
        schema: &EventSchema,
        field: &str,
        operator: EventOperator,
        value: Option<serde_json::Value>,
    ) -> FilterSpec {
        compile_event_field_rule(
            schema,
            &EventFieldRule {
                field: field.to_string(),
                operator,
                value,
            },
        )
        .unwrap()
    }

    fn keyword_contains(value: &str) -> FilterSpec {
        FilterSpec::Tagged(TaggedFilterSpec::Regex {
            pattern: regex::escape(value),
            case_insensitive: true,
        })
    }

    fn triage_binding(
        connection_slug: &str,
        connector_slug: &str,
        filter: Option<FilterSpec>,
        ignore_filters: Vec<FilterSpec>,
    ) -> WorkflowBindingSpec {
        WorkflowBindingSpec {
            slug: format!("monitor-{connection_slug}"),
            description: format!("Monitor {connection_slug} for actionable tasks"),
            connection_slug: connection_slug.to_string(),
            connector_slug: Some(connector_slug.to_string()),
            status: WorkflowBindingStatus::Enabled,
            filter,
            ignore_filters,
            contact_ids: Vec::new(),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::TriageAgent {
                prompt: format!("triage {connection_slug}"),
                model: None,
            },
            created_at_ms: 0,
        }
    }

    fn event(connection_slug: &str, text: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope {
            envelope_id: format!("env-{connection_slug}-{}", text.len()),
            subscriber_id: connection_slug.to_string(),
            received_at_ms: 0,
            event: Event {
                topic: connection_slug.to_string(),
                kind: "message".to_string(),
                control: false,
                dedup_key: None,
                text: text.to_string(),
                payload,
            },
        }
    }

    #[test]
    fn monitor_filter_matrix_routes_synthetic_events() {
        let telegram = bundled_schema("telegram-user");
        let gmail = bundled_schema("gmail-browser");
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);

        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(triage_binding(
                "matrix-text",
                "telegram-login",
                Some(keyword_contains("invoice")),
                Vec::new(),
            ))
            .unwrap();
        let (recording, dispatcher) = recording_dispatcher();
        let matched = process_envelope_result(
            &event(
                "matrix-text",
                "please review invoice",
                serde_json::json!({"chat_kind": "user"}),
            ),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
        );
        let missed = process_envelope_result(
            &event(
                "matrix-text",
                "please review roadmap",
                serde_json::json!({"chat_kind": "user"}),
            ),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
        );
        assert!(matched.matched);
        assert_eq!(matched.acted, 1);
        assert!(!missed.matched);
        assert_eq!(missed.acted, 0);

        let group_filter = field_filter(
            &telegram,
            "chat_kind",
            EventOperator::Equals,
            Some(serde_json::json!("group")),
        );
        store
            .create(triage_binding(
                "matrix-skip-group",
                "telegram-login",
                None,
                vec![group_filter.clone()],
            ))
            .unwrap();
        let skipped = process_envelope_result(
            &event(
                "matrix-skip-group",
                "hello",
                serde_json::json!({"chat_kind": "group"}),
            ),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
        );
        let passed = process_envelope_result(
            &event(
                "matrix-skip-group",
                "hello",
                serde_json::json!({"chat_kind": "user"}),
            ),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
        );
        assert!(!skipped.matched);
        assert_eq!(skipped.acted, 0);
        assert!(passed.matched);
        assert_eq!(passed.acted, 1);

        let subject_invoice = field_filter(
            &gmail,
            "message.subject",
            EventOperator::Contains,
            Some(serde_json::json!("invoice")),
        );
        store
            .create(triage_binding(
                "matrix-gmail",
                "gmail-browser",
                Some(subject_invoice.clone()),
                Vec::new(),
            ))
            .unwrap();
        let gmail_passed = process_envelope_result(
            &event(
                "matrix-gmail",
                "snippet",
                serde_json::json!({"message": {"subject": "invoice due"}}),
            ),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
        );
        assert!(gmail_passed.matched);
        assert_eq!(gmail_passed.acted, 1);

        store
            .create(triage_binding(
                "matrix-include-exclude",
                "telegram-login",
                Some(keyword_contains("invoice")),
                vec![group_filter.clone()],
            ))
            .unwrap();
        let exclude_wins = process_envelope_result(
            &event(
                "matrix-include-exclude",
                "invoice is ready",
                serde_json::json!({"chat_kind": "group"}),
            ),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
        );
        assert!(!exclude_wins.matched);
        assert_eq!(exclude_wins.acted, 0);

        store
            .create(triage_binding(
                "matrix-telegram-isolated",
                "telegram-login",
                None,
                vec![group_filter],
            ))
            .unwrap();
        let gmail_with_telegram_payload = process_envelope_result(
            &event(
                "matrix-gmail",
                "snippet",
                serde_json::json!({
                    "chat_kind": "group",
                    "message": {"subject": "invoice due"}
                }),
            ),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
        );
        assert!(gmail_with_telegram_payload.matched);
        assert_eq!(gmail_with_telegram_payload.acted, 1);

        assert_eq!(
            recording.topics.lock().unwrap().as_slice(),
            &[
                "matrix-text".to_string(),
                "matrix-skip-group".to_string(),
                "matrix-gmail".to_string(),
                "matrix-gmail".to_string()
            ]
        );
    }

    #[test]
    fn deleting_rule_removes_suppression_for_same_event() {
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        let mut binding = triage_binding(
            "matrix-delete",
            "telegram-login",
            None,
            vec![keyword_contains("noise")],
        );
        store.create(binding.clone()).unwrap();
        let (_recording, dispatcher) = recording_dispatcher();
        let envelope = event(
            "matrix-delete",
            "noise",
            serde_json::json!({"chat_kind": "user"}),
        );

        let suppressed =
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None);
        assert!(!suppressed.matched);
        assert_eq!(suppressed.acted, 0);

        binding.ignore_filters.clear();
        store.upsert(binding).unwrap();

        let passed =
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None);
        assert!(passed.matched);
        assert_eq!(passed.acted, 1);
    }
}
