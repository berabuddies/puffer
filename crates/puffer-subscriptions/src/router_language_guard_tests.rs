#[cfg(test)]
mod language_guard_tests {
    use super::*;
    use crate::action::ActionResult;
    use crate::classify::NullClassifier;
    use crate::self_gate::{DropAllSelfGate, SelfMessageGate};
    use crate::spec::{ActionSpec, WorkflowBindingSpec};
    use puffer_subscriber_runtime::{Event, EventEnvelope};
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

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

    fn drop_all_gate() -> Arc<dyn SelfMessageGate> {
        Arc::new(DropAllSelfGate)
    }

    fn triage_binding(slug: &str, description: &str, prompt: &str) -> WorkflowBindingSpec {
        WorkflowBindingSpec {
            slug: slug.into(),
            description: description.into(),
            connection_slug: "telegram-user".into(),
            connector_slug: Some("telegram-login".into()),
            status: WorkflowBindingStatus::Enabled,
            filter: None,
            ignore_filters: Vec::new(),
            contact_ids: Vec::new(),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::TriageAgent {
                prompt: prompt.into(),
                model: None,
            },
            created_at_ms: 0,
        }
    }

    fn chinese_event() -> EventEnvelope {
        EventEnvelope {
            envelope_id: "env-cn".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: "\u{8bf7}\u{7acb}\u{5373}\u{6574}\u{7406}\u{5317}\u{4eac}\u{7684}\u{56fd}\u{4fdd}\u{5355}\u{4f4d}\u{540d}\u{5355}".into(),
                payload: serde_json::json!({
                    "message": "\u{8bf7}\u{7acb}\u{5373}\u{6574}\u{7406}\u{5317}\u{4eac}\u{7684}\u{56fd}\u{4fdd}\u{5355}\u{4f4d}\u{540d}\u{5355}"
                }),
            },
        }
    }

    #[test]
    fn monitor_triage_dispatch_adds_runtime_language_guard_to_legacy_prompt() {
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(triage_binding(
                "monitor-telegram-user",
                "Monitor telegram-user for actionable tasks",
                "legacy monitor prompt",
            ))
            .unwrap();
        let dispatcher = Arc::new(PromptRecordingDispatcher {
            prompts: StdMutex::new(Vec::new()),
        });
        let dispatcher_trait: Arc<dyn ActionDispatcher> = dispatcher.clone();
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);

        let result = process_envelope_result(
            &chinese_event(),
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
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        store
            .create(triage_binding(
                "custom-triage",
                "Custom triage workflow",
                "custom prompt",
            ))
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

        let result = process_envelope_result(
            &envelope,
            &store,
            None,
            &dispatcher_trait,
            &classifier,
            None,
            &drop_all_gate(),
        );

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
            .create(triage_binding(
                "monitor-telegram-user",
                "Monitor telegram-user for actionable tasks",
                "legacy batch prompt",
            ))
            .unwrap();
        let dispatcher = Arc::new(BatchPromptRecordingDispatcher {
            prompts: StdMutex::new(Vec::new()),
        });
        let dispatcher_trait: Arc<dyn ActionDispatcher> = dispatcher.clone();
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let mut envelope = chinese_event();
        envelope.event.dedup_key = Some("chat:1".into());

        let result = process_envelope_batch_result(
            &[envelope],
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
}
