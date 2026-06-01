#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{ActionResult, BuiltinActionDispatcher};
    use crate::classify::NullClassifier;
    use crate::spec::{ActionSpec, FileAppendFormat, TaggedFilterSpec, WorkflowBindingSpec};
    use puffer_subscriber_runtime::{Event, EventBus, EventEnvelope};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

    #[test]
    fn case_insensitive_regex_matches() {
        let filter = FilterSpec::Tagged(TaggedFilterSpec::Regex {
            pattern: r"\bIoC\b".into(),
            case_insensitive: true,
        });
        assert!(prefilter_passes(Some(&filter), "We saw an IOC today"));
        assert!(!prefilter_passes(
            Some(&filter),
            "We saw an Indicator today"
        ));
    }

    #[test]
    fn missing_filter_passes() {
        assert!(prefilter_passes(None, "anything"));
    }

    #[test]
    fn control_events_do_not_match_workflows() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(WorkflowBindingSpec {
                slug: "all-telegram".into(),
                description: "all telegram".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: None,
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::RunWorkflow {
                    slug: "downstream".into(),
                },
                created_at_ms: 0,
            })
            .unwrap();
        let envelope = EventEnvelope {
            envelope_id: "env-control".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "send_complete".into(),
                control: true,
                dedup_key: None,
                text: String::new(),
                payload: serde_json::json!({"peer":"@alice"}),
            },
        };
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(BuiltinActionDispatcher::new());
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);

        let result =
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None);

        assert!(!result.matched);
        assert_eq!(result.acted, 0);
        assert_eq!(result.failed, 0);

        let mut silent_envelope = envelope.clone();
        silent_envelope.envelope_id = "env-silent".into();
        silent_envelope.event.payload = serde_json::json!({
            "message": "quiet",
            "notification_silent": true
        });
        let result = process_envelope_result(
            &silent_envelope,
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
        );

        assert!(!result.matched);
        assert_eq!(result.acted, 0);
        assert_eq!(result.failed, 0);
    }

    #[test]
    fn process_result_reports_action_failures() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(WorkflowBindingSpec {
                slug: "notify".into(),
                description: "notify".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: None,
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::ForwardMessage {
                    platform: "telegram".into(),
                    target: "@alice".into(),
                    template: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let envelope = EventEnvelope {
            envelope_id: "env-1".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "hello".into(),
                payload: serde_json::json!({"message":"hello"}),
            },
        };
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(BuiltinActionDispatcher::new());
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);

        let result =
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None);

        assert!(result.matched);
        assert_eq!(result.acted, 0);
        assert_eq!(result.failed, 1);
    }

    #[test]
    fn batch_result_groups_triage_events_for_one_dispatch_call() {
        struct BatchRecordingDispatcher {
            batches: StdMutex<Vec<Vec<String>>>,
        }

        impl ActionDispatcher for BatchRecordingDispatcher {
            fn dispatch(&self, _action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
                ActionResult::failure("single dispatch should not run")
            }

            fn dispatch_batch(
                &self,
                _action: &ActionSpec,
                envelopes: &[EventEnvelope],
            ) -> ActionResult {
                self.batches.lock().unwrap().push(
                    envelopes
                        .iter()
                        .map(|envelope| envelope.event.text.clone())
                        .collect(),
                );
                ActionResult::success("batched triage")
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
                filter: None,
                ignore_filters: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::TriageAgent {
                    prompt: "triage".into(),
                    model: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let dispatcher = Arc::new(BatchRecordingDispatcher {
            batches: StdMutex::new(Vec::new()),
        });
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let envelopes = vec![
            EventEnvelope {
                envelope_id: "env-1".into(),
                subscriber_id: "telegram-user".into(),
                received_at_ms: 0,
                event: Event {
                    topic: "telegram-user".into(),
                    kind: "message".into(),
                    control: false,
                    dedup_key: Some("chat:1".into()),
                    text: "first".into(),
                    payload: serde_json::json!({"message":"first"}),
                },
            },
            EventEnvelope {
                envelope_id: "env-2".into(),
                subscriber_id: "telegram-user".into(),
                received_at_ms: 0,
                event: Event {
                    topic: "telegram-user".into(),
                    kind: "message".into(),
                    control: false,
                    dedup_key: Some("chat:2".into()),
                    text: "second".into(),
                    payload: serde_json::json!({"message":"second"}),
                },
            },
        ];
        let dispatcher_trait: Arc<dyn ActionDispatcher> = dispatcher.clone();

        let result = process_envelope_batch_result(
            &envelopes,
            &store,
            None,
            &dispatcher_trait,
            &classifier,
            None,
        );

        assert!(result.matched);
        assert_eq!(result.acted, 2);
        assert_eq!(result.failed, 0);
        assert_eq!(
            dispatcher.batches.lock().unwrap().as_slice(),
            &[vec!["first".to_string(), "second".to_string()]]
        );
    }

    #[test]
    fn monitor_bindings_skip_muted_notification_events() {
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
                ignore_filters: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::TriageAgent {
                    prompt: "triage".into(),
                    model: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let envelope = EventEnvelope {
            envelope_id: "env-muted".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "quiet".into(),
                payload: serde_json::json!({
                    "message": "quiet",
                    "notification_muted": true
                }),
            },
        };
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(BuiltinActionDispatcher::new());
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);

        let result =
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None);

        assert!(!result.matched);
        assert_eq!(result.acted, 0);
        assert_eq!(result.failed, 0);
    }

    #[test]
    fn non_monitor_bindings_still_receive_muted_notification_events() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(WorkflowBindingSpec {
                slug: "append-telegram".into(),
                description: "append telegram".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: Some("telegram-login".into()),
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::FileAppend {
                    path: "out.jsonl".into(),
                    format: FileAppendFormat::Jsonl,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let envelope = EventEnvelope {
            envelope_id: "env-muted".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "quiet".into(),
                payload: serde_json::json!({
                    "message": "quiet",
                    "notification_muted": true
                }),
            },
        };
        let dispatcher: Arc<dyn ActionDispatcher> =
            Arc::new(BuiltinActionDispatcher::with_storage_root(dir.path()));
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);

        let result =
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None);

        assert!(result.matched);
        assert_eq!(result.acted, 1);
        assert_eq!(result.failed, 0);
    }

    #[test]
    fn ignore_filters_suppress_matching_events_before_action() {
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
                ignore_filters: vec![FilterSpec::Json(serde_json::json!({
                    "chat_id": 2041550535_i64,
                    "sender_username": "FuzzlandInternalBot"
                }))],
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::TriageAgent {
                    prompt: "triage".into(),
                    model: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(BuiltinActionDispatcher::new());
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let history_store = WorkflowHistoryStore::load(dir.path().join("history.json")).unwrap();
        let mut envelope = EventEnvelope {
            envelope_id: "env-ignored".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "alert".into(),
                payload: serde_json::json!({
                    "chat_id": 2041550535_i64,
                    "sender_username": "FuzzlandInternalBot",
                    "message": "alert"
                }),
            },
        };

        let ignored = process_envelope_result(
            &envelope,
            &store,
            Some(&history_store),
            &dispatcher,
            &classifier,
            None,
        );
        assert!(!ignored.matched);
        assert_eq!(ignored.acted, 0);
        let ignored_history = history_store.list();
        assert_eq!(ignored_history.len(), 1);
        assert_eq!(
            ignored_history[0].action_log[0].action,
            "monitor_ignore_filter"
        );

        envelope.envelope_id = "env-other-sender".into();
        envelope.event.payload = serde_json::json!({
            "chat_id": 2041550535_i64,
            "sender_username": "Alice",
            "message": "alert"
        });
        let passed =
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None);
        assert!(passed.matched);
        assert_eq!(passed.failed, 1);
    }

    #[test]
    fn dedup_key_suppresses_replayed_events_for_same_binding() {
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
                ignore_filters: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::ForwardMessage {
                    platform: "telegram".into(),
                    target: "@alice".into(),
                    template: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(BuiltinActionDispatcher::new());
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let history_store = WorkflowHistoryStore::load(dir.path().join("history.json")).unwrap();
        let envelope = EventEnvelope {
            envelope_id: "env-original".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: Some("5174182590:3263257".into()),
                text: "alert".into(),
                payload: serde_json::json!({
                    "chat_id": 5174182590_i64,
                    "message_id": 3263257,
                    "message": "alert"
                }),
            },
        };

        let first = process_envelope_result(
            &envelope,
            &store,
            Some(&history_store),
            &dispatcher,
            &classifier,
            None,
        );
        assert!(first.matched);
        assert!(history_store.contains_dedup_key("monitor-telegram-user", "5174182590:3263257"));

        let mut replay = envelope.clone();
        replay.envelope_id = "env-replay".into();
        let second = process_envelope_result(
            &replay,
            &store,
            Some(&history_store),
            &dispatcher,
            &classifier,
            None,
        );

        assert!(!second.matched);
        assert_eq!(history_store.list_for("monitor-telegram-user").len(), 1);
    }

    #[tokio::test]
    async fn router_receives_event_published_immediately_after_spawn() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(WorkflowBindingSpec {
                slug: "append".into(),
                description: "append".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: None,
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::FileAppend {
                    path: "out.jsonl".into(),
                    format: FileAppendFormat::Jsonl,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let bus = EventBus::new();
        let dispatcher: Arc<dyn ActionDispatcher> =
            Arc::new(BuiltinActionDispatcher::with_storage_root(dir.path()));
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let router =
            SubscriptionRouter::spawn(bus.clone(), Arc::new(store), None, dispatcher, classifier);

        bus.publish(EventEnvelope {
            envelope_id: "env-race".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "hello".into(),
                payload: serde_json::json!({"message":"hello"}),
            },
        });

        let path = dir.path().join("out.jsonl");
        for _ in 0..50 {
            if path.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("hello"));
        router.shutdown().await;
    }

    #[tokio::test]
    async fn router_runs_actions_on_blocking_thread() {
        struct RuntimeDroppingDispatcher;

        impl ActionDispatcher for RuntimeDroppingDispatcher {
            fn dispatch(&self, _action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                drop(runtime);
                ActionResult::success("runtime dropped")
            }
        }

        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(WorkflowBindingSpec {
                slug: "triage".into(),
                description: "triage".into(),
                connection_slug: "email-live".into(),
                connector_slug: Some("email".into()),
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::ForwardMessage {
                    platform: "email".into(),
                    target: "ops@example.com".into(),
                    template: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let history =
            Arc::new(WorkflowHistoryStore::load(dir.path().join("history.json")).unwrap());
        let bus = EventBus::new();
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(RuntimeDroppingDispatcher);
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let router = SubscriptionRouter::spawn(
            bus.clone(),
            Arc::new(store),
            Some(history.clone()),
            dispatcher,
            classifier,
        );

        bus.publish(EventEnvelope {
            envelope_id: "env-runtime".into(),
            subscriber_id: "email".into(),
            received_at_ms: 0,
            event: Event {
                topic: "email".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "hello".into(),
                payload: serde_json::json!({"message":"hello"}),
            },
        });

        for _ in 0..50 {
            if router.stats().snapshot_tuple() == (1, 1, 1, 0) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(router.stats().snapshot_tuple(), (1, 1, 1, 0));
        assert_eq!(history.list_for("triage").len(), 1);
        router.shutdown().await;
    }

    #[tokio::test]
    async fn router_drains_events_while_actions_are_running() {
        struct SlowDispatcher {
            calls: Arc<AtomicUsize>,
        }

        impl ActionDispatcher for SlowDispatcher {
            fn dispatch(&self, _action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
                self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(300));
                ActionResult::success("slow action complete")
            }
        }

        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(WorkflowBindingSpec {
                slug: "triage".into(),
                description: "triage".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: None,
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::TriageAgent {
                    prompt: "triage".into(),
                    model: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let bus = EventBus::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(SlowDispatcher {
            calls: calls.clone(),
        });
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let router =
            SubscriptionRouter::spawn(bus.clone(), Arc::new(store), None, dispatcher, classifier);

        for index in 0..2 {
            bus.publish(EventEnvelope {
                envelope_id: format!("env-{index}"),
                subscriber_id: "telegram-user".into(),
                received_at_ms: 0,
                event: Event {
                    topic: "telegram-user".into(),
                    kind: "message".into(),
                    control: false,
                    dedup_key: Some(format!("chat:{index}")),
                    text: format!("hello {index}"),
                    payload: serde_json::json!({"message": format!("hello {index}")}),
                },
            });
        }

        for _ in 0..20 {
            if router.stats().snapshot_tuple().0 == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        assert_eq!(router.stats().snapshot_tuple().0, 2);
        router.shutdown().await;
    }
}
