#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{
        ActionResult, BuiltinActionDispatcher, TriageDecision, TriageDecisionOutcome,
    };
    use crate::classify::{ClassifyDecision, NullClassifier};
    use crate::self_gate::{DropAllSelfGate, SelfMessageGate};
    use crate::spec::{ActionSpec, FileAppendFormat, TaggedFilterSpec, WorkflowBindingSpec};
    use puffer_subscriber_runtime::{Event, EventBus, EventEnvelope};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

    /// Drops all self messages (matches `DropAllSelfGate`) but is a local handle
    /// so tests can also flip to allow. Used as the default in self-gate tests.
    fn drop_all_gate() -> Arc<dyn SelfMessageGate> {
        Arc::new(DropAllSelfGate)
    }

    /// Test double: allows every self/outgoing message through the gate.
    struct AllowAllSelfGate;
    impl SelfMessageGate for AllowAllSelfGate {
        fn should_dispatch_self_message(&self, _event: &Event) -> bool {
            true
        }
    }

    /// Test double: panics if `classify` runs. Proves the self/outgoing path
    /// short-circuits the classifier (the #569 credit-burn guard).
    struct PanicClassifier;
    impl Classifier for PanicClassifier {
        fn classify(
            &self,
            _spec: &WorkflowBindingSpec,
            _event: &Event,
        ) -> ClassifyDecision {
            panic!("classifier must not run for self/outgoing events");
        }
    }

    /// Test double: records dispatch calls and reports success.
    struct CountingDispatcher {
        calls: Arc<AtomicUsize>,
    }
    impl ActionDispatcher for CountingDispatcher {
        fn dispatch(&self, _action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            ActionResult::success("dispatched")
        }
    }

    fn monitor_binding_with_classify_prompt() -> WorkflowBindingSpec {
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

    fn outgoing_envelope() -> EventEnvelope {
        EventEnvelope {
            envelope_id: "env-outgoing".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "done, sent it".into(),
                payload: serde_json::json!({"is_outgoing": true, "chat_id": 42}),
            },
        }
    }

    #[test]
    fn outgoing_event_with_dropping_gate_is_not_dispatched_or_classified() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store.create(monitor_binding_with_classify_prompt()).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(CountingDispatcher {
            calls: calls.clone(),
        });
        let classifier: Arc<dyn Classifier> = Arc::new(PanicClassifier);
        let gate = drop_all_gate();

        let result = process_envelope_result(
            &outgoing_envelope(),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
            &gate,
        );

        assert!(!result.matched, "dropped self message must not match");
        assert_eq!(result.acted, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 0, "dispatcher must not run");
    }

    #[test]
    fn outgoing_event_with_allowing_gate_is_dispatched_without_classify() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store.create(monitor_binding_with_classify_prompt()).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(CountingDispatcher {
            calls: calls.clone(),
        });
        let classifier: Arc<dyn Classifier> = Arc::new(PanicClassifier);
        let gate: Arc<dyn SelfMessageGate> = Arc::new(AllowAllSelfGate);

        let result = process_envelope_result(
            &outgoing_envelope(),
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
            &gate,
        );

        assert!(result.matched, "allowed self message must match");
        assert_eq!(result.acted, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "dispatcher runs exactly once"
        );
        // PanicClassifier never panicked => classify was bypassed for self event.
    }

    #[test]
    fn incoming_event_is_unaffected_by_gate() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        // No classify_prompt so the normal incoming path dispatches without an
        // LLM classify (and the NullClassifier would Pass anyway).
        let mut spec = monitor_binding_with_classify_prompt();
        spec.classify_prompt = None;
        store.create(spec).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(CountingDispatcher {
            calls: calls.clone(),
        });
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let gate = drop_all_gate();

        let incoming = EventEnvelope {
            envelope_id: "env-incoming".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "please file the report".into(),
                payload: serde_json::json!({"is_outgoing": false, "chat_id": 42}),
            },
        };

        let result = process_envelope_result(
            &incoming, &store, None, &dispatcher, &classifier, None, &gate,
        );

        assert!(result.matched, "incoming event still dispatches with drop gate");
        assert_eq!(result.acted, 1);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[test]
    fn batch_outgoing_event_with_dropping_gate_is_not_dispatched_or_classified() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store.create(monitor_binding_with_classify_prompt()).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(CountingDispatcher {
            calls: calls.clone(),
        });
        let classifier: Arc<dyn Classifier> = Arc::new(PanicClassifier);
        let gate = drop_all_gate();

        let result = process_envelope_batch_result(
            &[outgoing_envelope()],
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
            &gate,
        );

        assert!(!result.matched, "dropped self message must not match on batch path");
        assert_eq!(result.acted, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            0,
            "dispatcher must not run"
        );
    }

    #[test]
    fn batch_outgoing_event_with_allowing_gate_is_dispatched_without_classify() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store.create(monitor_binding_with_classify_prompt()).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(CountingDispatcher {
            calls: calls.clone(),
        });
        let classifier: Arc<dyn Classifier> = Arc::new(PanicClassifier);
        let gate: Arc<dyn SelfMessageGate> = Arc::new(AllowAllSelfGate);

        let result = process_envelope_batch_result(
            &[outgoing_envelope()],
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
            &gate,
        );

        assert!(result.matched, "allowed self message must match on batch path");
        assert_eq!(result.acted, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(
            calls.load(AtomicOrdering::SeqCst),
            1,
            "dispatcher runs exactly once"
        );
        // PanicClassifier never panicked => classify was bypassed for self event.
    }

    #[test]
    fn batch_incoming_event_is_unaffected_by_gate() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        // No classify_prompt so the normal incoming path dispatches without an
        // LLM classify (and the NullClassifier would Pass anyway).
        let mut spec = monitor_binding_with_classify_prompt();
        spec.classify_prompt = None;
        store.create(spec).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(CountingDispatcher {
            calls: calls.clone(),
        });
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let gate = drop_all_gate();

        let incoming = EventEnvelope {
            envelope_id: "env-incoming".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "please file the report".into(),
                payload: serde_json::json!({"is_outgoing": false, "chat_id": 42}),
            },
        };

        let result = process_envelope_batch_result(
            &[incoming],
            &store,
            None,
            &dispatcher,
            &classifier,
            None,
            &gate,
        );

        assert!(
            result.matched,
            "incoming event still dispatches through batch path with drop gate"
        );
        assert_eq!(result.acted, 1);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[test]
    fn batch_triage_decision_trace_is_written_only_to_matching_envelope() {
        struct DecisionBatchDispatcher;
        impl ActionDispatcher for DecisionBatchDispatcher {
            fn dispatch(&self, _action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
                ActionResult::failure("single dispatch should not run")
            }

            fn dispatch_batch(
                &self,
                _action: &ActionSpec,
                _envelopes: &[EventEnvelope],
            ) -> ActionResult {
                ActionResult::success("triaged").with_triage_decisions(vec![
                    TriageDecision {
                        envelope_id: "env-2".into(),
                        outcome: TriageDecisionOutcome::NoTask,
                        policy: Some("status_update_no_action".into()),
                        reason: "这是状态通知，没有请求你回复、决策或处理。".into(),
                    },
                ])
            }
        }

        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        let mut spec = monitor_binding_with_classify_prompt();
        spec.classify_prompt = None;
        store.create(spec).unwrap();
        let trace_store = crate::monitor_trace::MonitorTraceStore::load(
            dir.path().join("monitor-trace.json"),
        )
        .unwrap();
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(DecisionBatchDispatcher);
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let envelopes = vec![
            EventEnvelope {
                envelope_id: "env-1".into(),
                subscriber_id: "telegram-user".into(),
                received_at_ms: 1,
                event: Event {
                    topic: "telegram-user".into(),
                    kind: "message".into(),
                    control: false,
                    dedup_key: Some("42:1".into()),
                    text: "今天把工作总结汇报给我".into(),
                    payload: serde_json::json!({
                        "chat_id": 42,
                        "message_id": 1,
                        "sender_id": 7
                    }),
                },
            },
            EventEnvelope {
                envelope_id: "env-2".into(),
                subscriber_id: "telegram-user".into(),
                received_at_ms: 2,
                event: Event {
                    topic: "telegram-user".into(),
                    kind: "message".into(),
                    control: false,
                    dedup_key: Some("42:2".into()),
                    text: "预发环境昨晚重启过一次，现在正常。".into(),
                    payload: serde_json::json!({
                        "chat_id": 42,
                        "message_id": 2,
                        "sender_id": 7
                    }),
                },
            },
        ];

        let result = process_envelope_batch_result_with_monitor_digest(
            &envelopes,
            &store,
            None,
            &dispatcher,
            &classifier,
            &drop_all_gate(),
            None,
            Some(&trace_store),
            None,
        );

        assert!(result.matched);
        let trace = trace_store.list_recent(Some("telegram-user"), 10);
        let env1 = trace.iter().find(|message| message.envelope_id.as_deref() == Some("env-1")).unwrap();
        let env2 = trace.iter().find(|message| message.envelope_id.as_deref() == Some("env-2")).unwrap();
        assert!(env1.stages.iter().all(|stage| stage.id != "triage_decision"));
        let decision_stage = env2
            .stages
            .iter()
            .find(|stage| stage.id == "triage_decision")
            .expect("env-2 should carry its own triage decision");
        assert_eq!(
            decision_stage.decision.as_ref().map(|decision| decision.reason.as_str()),
            Some("这是状态通知，没有请求你回复、决策或处理。")
        );
    }

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
                contact_ids: Vec::new(),
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
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None, &drop_all_gate());

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
            &drop_all_gate(),
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
                contact_ids: Vec::new(),
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
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None, &drop_all_gate());

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
                contact_ids: Vec::new(),
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
                    text: "please review first".into(),
                    payload: serde_json::json!({"message":"please review first"}),
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
                    text: "please review second".into(),
                    payload: serde_json::json!({"message":"please review second"}),
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
            &drop_all_gate(),
        );

        assert!(result.matched);
        assert_eq!(result.acted, 2);
        assert_eq!(result.failed, 0);
        assert_eq!(
            dispatcher.batches.lock().unwrap().as_slice(),
            &[vec![
                "please review first".to_string(),
                "please review second".to_string()
            ]]
        );
    }

    #[test]
    fn monitor_debounce_key_uses_stable_conversation_scope() {
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
                contact_ids: Vec::new(),
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
            envelope_id: "env-1".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: Some("42:1".into()),
                text: "first".into(),
                payload: serde_json::json!({"chat_id": 42, "message":"first"}),
            },
        };

        assert_eq!(
            crate::router_debounce::monitor_debounce_key(&envelope, &store).as_deref(),
            Some("telegram-user:chat_id=42")
        );
    }

    #[test]
    fn monitor_debounce_key_skips_topics_with_immediate_workflows() {
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
                contact_ids: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::TriageAgent {
                    prompt: "triage".into(),
                    model: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        store
            .create(WorkflowBindingSpec {
                slug: "forward-telegram-user".into(),
                description: "Forward telegram messages".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: Some("telegram-login".into()),
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: Vec::new(),
                contact_ids: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::ForwardMessage {
                    platform: "telegram".into(),
                    target: "@ops".into(),
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
                dedup_key: Some("42:1".into()),
                text: "first".into(),
                payload: serde_json::json!({"chat_id": 42, "message":"first"}),
            },
        };

        assert_eq!(
            crate::router_debounce::monitor_debounce_key(&envelope, &store),
            None
        );
    }

    #[test]
    fn monitor_triage_dispatch_adds_runtime_language_guard_to_legacy_prompt() {
        struct PromptRecordingDispatcher {
            prompts: StdMutex<Vec<String>>,
        }

        impl ActionDispatcher for PromptRecordingDispatcher {
            fn dispatch(&self, action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
                match action {
                    ActionSpec::TriageAgent { prompt, .. } => {
                        self.prompts.lock().unwrap().push(prompt.clone());
                        ActionResult::success("triaged")
                    }
                    other => ActionResult::failure(format!("unexpected action: {other:?}")),
                }
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
                contact_ids: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::TriageAgent {
                    prompt: "legacy monitor prompt".into(),
                    model: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let dispatcher = Arc::new(PromptRecordingDispatcher {
            prompts: StdMutex::new(Vec::new()),
        });
        let dispatcher_trait: Arc<dyn ActionDispatcher> = dispatcher.clone();
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let envelope = EventEnvelope {
            envelope_id: "env-cn".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "请立即整理北京的国保单位名单".into(),
                payload: serde_json::json!({"message":"请立即整理北京的国保单位名单"}),
            },
        };

        let result =
            process_envelope_result(&envelope, &store, None, &dispatcher_trait, &classifier, None, &drop_all_gate());

        assert!(result.matched);
        assert_eq!(result.acted, 1);
        let prompts = dispatcher.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("legacy monitor prompt"));
        assert!(prompts[0].contains("Monitor source-language runtime guard"));
        assert!(prompts[0].contains("For Chinese source text"));
        assert!(prompts[0].contains("subject, description, actions[].actionPrompt"));
        assert!(prompts[0].contains("exactly as written in the current source event text"));
        assert!(prompts[0].contains("conversation_context"));
        assert!(prompts[0].contains("telegram_server_history_cache"));
        assert!(prompts[0].contains("subscriber_diagnostics"));
        assert!(prompts[0].contains("ambiguous short messages"));
        assert!(prompts[0].contains("Same chat/contact is not enough"));
        assert!(prompts[0].contains("replace or clear `metadata.actions`"));
        assert!(!MONITOR_RUNTIME_LANGUAGE_POLICY.contains("never change its status"));
        assert!(MONITOR_RUNTIME_LANGUAGE_POLICY.contains("status: completed"));
        assert!(MONITOR_RUNTIME_LANGUAGE_POLICY.contains("direction"));
        match store.get("monitor-telegram-user").unwrap().action {
            ActionSpec::TriageAgent { prompt, .. } => assert_eq!(prompt, "legacy monitor prompt"),
            _ => panic!("expected triage agent"),
        }
    }

    #[test]
    fn non_monitor_triage_dispatch_leaves_prompt_unchanged() {
        struct PromptRecordingDispatcher {
            prompts: StdMutex<Vec<String>>,
        }

        impl ActionDispatcher for PromptRecordingDispatcher {
            fn dispatch(&self, action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
                match action {
                    ActionSpec::TriageAgent { prompt, .. } => {
                        self.prompts.lock().unwrap().push(prompt.clone());
                        ActionResult::success("triaged")
                    }
                    other => ActionResult::failure(format!("unexpected action: {other:?}")),
                }
            }
        }

        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(WorkflowBindingSpec {
                slug: "custom-triage".into(),
                description: "Custom triage workflow".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: Some("telegram-login".into()),
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: Vec::new(),
                contact_ids: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::TriageAgent {
                    prompt: "custom prompt".into(),
                    model: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let dispatcher = Arc::new(PromptRecordingDispatcher {
            prompts: StdMutex::new(Vec::new()),
        });
        let dispatcher_trait: Arc<dyn ActionDispatcher> = dispatcher.clone();
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let envelope = EventEnvelope {
            envelope_id: "env-custom".into(),
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

        let result =
            process_envelope_result(&envelope, &store, None, &dispatcher_trait, &classifier, None, &drop_all_gate());

        assert!(result.matched);
        assert_eq!(result.acted, 1);
        assert_eq!(
            dispatcher.prompts.lock().unwrap().as_slice(),
            &["custom prompt".to_string()]
        );
    }

    #[test]
    fn monitor_triage_batch_adds_runtime_language_guard_to_legacy_prompt() {
        struct BatchPromptRecordingDispatcher {
            prompts: StdMutex<Vec<String>>,
        }

        impl ActionDispatcher for BatchPromptRecordingDispatcher {
            fn dispatch(&self, _action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
                ActionResult::failure("single dispatch should not run")
            }

            fn dispatch_batch(
                &self,
                action: &ActionSpec,
                _envelopes: &[EventEnvelope],
            ) -> ActionResult {
                match action {
                    ActionSpec::TriageAgent { prompt, .. } => {
                        self.prompts.lock().unwrap().push(prompt.clone());
                        ActionResult::success("batched triage")
                    }
                    other => ActionResult::failure(format!("unexpected action: {other:?}")),
                }
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
                contact_ids: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::TriageAgent {
                    prompt: "legacy batch prompt".into(),
                    model: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let dispatcher = Arc::new(BatchPromptRecordingDispatcher {
            prompts: StdMutex::new(Vec::new()),
        });
        let dispatcher_trait: Arc<dyn ActionDispatcher> = dispatcher.clone();
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let envelopes = vec![EventEnvelope {
            envelope_id: "env-cn".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: Some("chat:1".into()),
                text: "请立即整理北京的国保单位名单".into(),
                payload: serde_json::json!({"message":"请立即整理北京的国保单位名单"}),
            },
        }];

        let result = process_envelope_batch_result(
            &envelopes,
            &store,
            None,
            &dispatcher_trait,
            &classifier,
            None,
            &drop_all_gate(),
        );

        assert!(result.matched);
        assert_eq!(result.acted, 1);
        let prompts = dispatcher.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("legacy batch prompt"));
        assert!(prompts[0].contains("Monitor source-language runtime guard"));
        assert!(prompts[0].contains("Same chat/contact is not enough"));
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
                contact_ids: Vec::new(),
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
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None, &drop_all_gate());

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
                contact_ids: Vec::new(),
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
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None, &drop_all_gate());

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
                contact_ids: Vec::new(),
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
            &drop_all_gate(),
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
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None, &drop_all_gate());
        assert!(passed.matched);
        assert_eq!(passed.failed, 1);
    }

    #[test]
    fn contact_filters_suppress_unlisted_contacts() {
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
                description: "Monitor selected Telegram contacts".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: Some("telegram-login".into()),
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: Vec::new(),
                contact_ids: vec!["telegram@alice".into()],
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
        let unrelated = EventEnvelope {
            envelope_id: "env-bob".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "hello".into(),
                payload: serde_json::json!({
                    "chat_kind": "user",
                    "chat_username": "bob"
                }),
            },
        };
        let related = EventEnvelope {
            envelope_id: "env-alice".into(),
            event: Event {
                payload: serde_json::json!({
                    "chat_kind": "user",
                    "chat_username": "alice"
                }),
                ..unrelated.event.clone()
            },
            ..unrelated.clone()
        };

        let skipped =
            process_envelope_result(&unrelated, &store, None, &dispatcher, &classifier, None, &drop_all_gate());
        let passed =
            process_envelope_result(&related, &store, None, &dispatcher, &classifier, None, &drop_all_gate());

        assert!(!skipped.matched);
        assert_eq!(skipped.acted, 0);
        assert!(passed.matched);
        assert_eq!(passed.acted, 1);
    }

    #[test]
    fn trace_records_no_monitor_binding_for_unmatched_message() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        let trace_store = crate::monitor_trace::MonitorTraceStore::load(
            dir.path().join("monitor-trace.json"),
        )
        .unwrap();
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(BuiltinActionDispatcher::new());
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let envelope = EventEnvelope {
            envelope_id: "env-no-binding".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 100,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: Some("42:7".into()),
                text: "hello".into(),
                payload: serde_json::json!({
                    "chat_id": 42,
                    "message_id": 7,
                    "date_ms": 90
                }),
            },
        };

        let result = process_envelope_result_with_monitor_digest(
            &envelope,
            &store,
            None,
            &dispatcher,
            &classifier,
            &drop_all_gate(),
            None,
            Some(&trace_store),
            None,
        );

        assert!(!result.matched);
        let trace = trace_store.list_recent(Some("telegram-user"), 10);
        assert_eq!(trace.len(), 1);
        assert_eq!(
            trace[0].latest_status,
            crate::monitor_trace::MonitorTraceStatus::RouterSkipped
        );
        assert!(
            trace[0]
                .stages
                .iter()
                .any(|stage| stage.id == "router_no_monitor_binding")
        );
    }

    #[test]
    fn batch_trace_records_contact_filter_skip() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(WorkflowBindingSpec {
                slug: "monitor-telegram-user".into(),
                description: "Monitor selected Telegram contacts".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: Some("telegram-login".into()),
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: Vec::new(),
                contact_ids: vec!["telegram@alice".into()],
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::TriageAgent {
                    prompt: "triage".into(),
                    model: None,
                },
                created_at_ms: 0,
            })
            .unwrap();
        let trace_store = crate::monitor_trace::MonitorTraceStore::load(
            dir.path().join("monitor-trace.json"),
        )
        .unwrap();
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(BuiltinActionDispatcher::new());
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let envelope = EventEnvelope {
            envelope_id: "env-bob".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 100,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: Some("42:8".into()),
                text: "hello".into(),
                payload: serde_json::json!({
                    "chat_kind": "user",
                    "chat_username": "bob",
                    "chat_id": 42,
                    "message_id": 8
                }),
            },
        };

        let result = process_envelope_batch_result_with_monitor_digest(
            &[envelope],
            &store,
            None,
            &dispatcher,
            &classifier,
            &drop_all_gate(),
            None,
            Some(&trace_store),
            None,
        );

        assert!(!result.matched);
        let trace = trace_store.list_recent(Some("telegram-user"), 10);
        assert_eq!(trace.len(), 1);
        assert!(
            trace[0]
                .stages
                .iter()
                .any(|stage| stage.id == "router_contact_filter_skip")
        );
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
                contact_ids: Vec::new(),
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
        // A SUCCESSFUL action must dedup-suppress replays. (Previously this used
        // BuiltinActionDispatcher, whose ForwardMessage fails with no outbound
        // configured — which incidentally tested the buggy "failed run blocks
        // forever" behavior. Dedup-on-replay is about successfully-handled events.)
        struct OkDispatcher;
        impl ActionDispatcher for OkDispatcher {
            fn dispatch(&self, _action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
                ActionResult::success("forwarded")
            }
        }
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(OkDispatcher);
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
            &drop_all_gate(),
        );
        assert!(first.matched);
        assert_eq!(first.acted, 1);
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
            &drop_all_gate(),
        );

        assert!(!second.matched);
        assert_eq!(history_store.list_for("monitor-telegram-user").len(), 1);
    }

    // Root cause A fix (end-to-end through the router): a FAILED action must be
    // retried on the next delivery instead of being dedup-blocked forever.
    #[test]
    fn failed_action_is_retried_on_redelivery() {
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
                contact_ids: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::TriageAgent {
                    prompt: "triage".into(),
                    model: None,
                },
                created_at_ms: 0,
            })
            .unwrap();

        // Fails on the first delivery, succeeds on the second (e.g. model endpoint
        // recovered) — proving the message is re-triaged rather than lost.
        struct FlakyDispatcher {
            calls: Arc<AtomicUsize>,
        }
        impl ActionDispatcher for FlakyDispatcher {
            fn dispatch(&self, _action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
                let n = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                if n == 0 {
                    ActionResult::failure("triage_agent failed: endpoint down")
                } else {
                    ActionResult::success("created task")
                }
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher: Arc<dyn ActionDispatcher> = Arc::new(FlakyDispatcher {
            calls: calls.clone(),
        });
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
                text: "remind me to file the Q3 report".into(),
                payload: serde_json::json!({"chat_id": 1, "message_id": 2}),
            },
        };

        // First delivery: matched but failed.
        let first = process_envelope_result(
            &envelope,
            &store,
            Some(&history_store),
            &dispatcher,
            &classifier,
            None,
            &drop_all_gate(),
        );
        assert!(first.matched);
        assert_eq!(first.failed, 1, "first delivery fails");
        assert!(
            !history_store.contains_dedup_key("monitor-telegram-user", "5174182590:3263257"),
            "a failed run must NOT permanently dedup-block"
        );

        // Re-delivery of the same message: NOT skipped as dedup_seen; re-triaged and succeeds.
        let mut replay = envelope.clone();
        replay.envelope_id = "env-replay".into();
        let second = process_envelope_result(
            &replay,
            &store,
            Some(&history_store),
            &dispatcher,
            &classifier,
            None,
            &drop_all_gate(),
        );
        assert!(
            second.matched,
            "failed message must be retried, not dedup-skipped"
        );
        assert_eq!(second.acted, 1, "retry succeeds and creates the task");
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 2, "dispatched twice");
        // Now Completed → future replays are suppressed.
        assert!(history_store.contains_dedup_key("monitor-telegram-user", "5174182590:3263257"));
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
                contact_ids: Vec::new(),
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
            SubscriptionRouter::spawn(bus.clone(), Arc::new(store), None, dispatcher, classifier, drop_all_gate());

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
                contact_ids: Vec::new(),
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
            drop_all_gate(),
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
                contact_ids: Vec::new(),
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
            SubscriptionRouter::spawn(bus.clone(), Arc::new(store), None, dispatcher, classifier, drop_all_gate());

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
