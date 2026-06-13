#[cfg(test)]
mod monitor_rule_tests {
    use super::*;
    use crate::action::{ActionResult, BuiltinActionDispatcher};
    use crate::classify::NullClassifier;
    use crate::spec::{ActionSpec, TaggedFilterSpec, WorkflowBindingSpec};
    use crate::{
        compile_event_field_rule, EventField, EventFieldRule, EventFieldType, EventOperator,
        EventSchema,
    };
    use puffer_subscriber_runtime::{Event, EventEnvelope};
    use serde_json::Value;
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

    fn bundled_connector_schema(slug: &str) -> EventSchema {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root");
        crate::load_event_schema_from_dir(&root.join("resources").join("connectors").join(slug))
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

    #[derive(Clone, Copy)]
    enum MatrixMode {
        Include,
        Skip,
    }

    #[derive(Clone)]
    struct MatrixCase {
        connector_slug: &'static str,
        schema_slug: &'static str,
        label: String,
        filter: FilterSpec,
        matching_text: String,
        matching_payload: Value,
        nonmatching_text: String,
        nonmatching_payload: Value,
    }

    fn schemas_for_ui_matrix() -> Vec<(&'static str, &'static str, EventSchema, usize)> {
        vec![
            (
                "telegram-login",
                "telegram-user",
                bundled_schema("telegram-user"),
                25,
            ),
            (
                "gmail-browser",
                "gmail-browser",
                bundled_schema("gmail-browser"),
                19,
            ),
            (
                "gcal-browser",
                "gcal-browser",
                bundled_schema("gcal-browser"),
                6,
            ),
            ("email", "email", bundled_schema("email"), 19),
            (
                "telegram-bot",
                "telegram-bot",
                bundled_connector_schema("telegram-bot"),
                7,
            ),
            (
                "lark-login",
                "lark-login",
                bundled_connector_schema("lark-login"),
                15,
            ),
            (
                "lark-bot",
                "lark-bot",
                bundled_connector_schema("lark-bot"),
                15,
            ),
        ]
    }

    fn keyword_matrix_cases(
        connector_slug: &'static str,
        schema_slug: &'static str,
    ) -> Vec<MatrixCase> {
        vec![
            MatrixCase {
                connector_slug,
                schema_slug,
                label: "Message text contains".to_string(),
                filter: FilterSpec::Tagged(TaggedFilterSpec::Regex {
                    pattern: regex::escape("invoice"),
                    case_insensitive: true,
                }),
                matching_text: "please review invoice".to_string(),
                matching_payload: serde_json::json!({}),
                nonmatching_text: "please review roadmap".to_string(),
                nonmatching_payload: serde_json::json!({}),
            },
            MatrixCase {
                connector_slug,
                schema_slug,
                label: "Message text equals".to_string(),
                filter: FilterSpec::Tagged(TaggedFilterSpec::Regex {
                    pattern: format!("^(?:{})$", regex::escape("invoice")),
                    case_insensitive: true,
                }),
                matching_text: "invoice".to_string(),
                matching_payload: serde_json::json!({}),
                nonmatching_text: "invoice due".to_string(),
                nonmatching_payload: serde_json::json!({}),
            },
            MatrixCase {
                connector_slug,
                schema_slug,
                label: "Message text matches".to_string(),
                filter: FilterSpec::Tagged(TaggedFilterSpec::Regex {
                    pattern: "invoice|receipt".to_string(),
                    case_insensitive: true,
                }),
                matching_text: "receipt attached".to_string(),
                matching_payload: serde_json::json!({}),
                nonmatching_text: "status update".to_string(),
                nonmatching_payload: serde_json::json!({}),
            },
        ]
    }

    fn field_matrix_cases(
        connector_slug: &'static str,
        schema_slug: &'static str,
        schema: &EventSchema,
    ) -> Vec<MatrixCase> {
        let mut cases = Vec::new();
        for field in &schema.fields {
            for operator in &field.operators {
                for value in matrix_rule_values(field, *operator) {
                    let (matching_payload, nonmatching_payload) =
                        matrix_payloads(field, *operator, value.as_ref());
                    cases.push(MatrixCase {
                        connector_slug,
                        schema_slug,
                        label: format!("{} {:?} {:?}", field.path, operator, value),
                        filter: field_filter(schema, &field.path, *operator, value),
                        matching_text: "source text".to_string(),
                        matching_payload,
                        nonmatching_text: "source text".to_string(),
                        nonmatching_payload,
                    });
                }
            }
        }
        cases
    }

    fn matrix_rule_values(field: &EventField, operator: EventOperator) -> Vec<Option<Value>> {
        if operator == EventOperator::Exists {
            return vec![None];
        }
        if !field.values.is_empty() {
            return field
                .values
                .iter()
                .map(|value| Some(value.value.clone()))
                .collect();
        }
        match (field.field_type, operator) {
            (EventFieldType::String, EventOperator::Contains) => {
                vec![Some(serde_json::json!("needle"))]
            }
            (EventFieldType::String, EventOperator::Equals) => {
                vec![Some(serde_json::json!("exact-value"))]
            }
            (EventFieldType::String, EventOperator::Matches) => {
                vec![Some(serde_json::json!("^match-[0-9]+$"))]
            }
            (EventFieldType::Number, EventOperator::Equals) => vec![Some(serde_json::json!(7))],
            (EventFieldType::Boolean, EventOperator::Equals) => {
                vec![Some(serde_json::json!(true)), Some(serde_json::json!(false))]
            }
            (EventFieldType::Enum, EventOperator::Equals) => {
                panic!("enum field `{}` must declare UI values", field.path)
            }
            _ => panic!(
                "unsupported matrix value for `{}` {:?} {:?}",
                field.path, field.field_type, operator
            ),
        }
    }

    fn matrix_payloads(
        field: &EventField,
        operator: EventOperator,
        value: Option<&Value>,
    ) -> (Value, Value) {
        if operator == EventOperator::Exists {
            return (
                payload_with_path(&field.path, serde_json::json!("present")),
                serde_json::json!({}),
            );
        }
        let value = value.expect("non-exists matrix rule value");
        match operator {
            EventOperator::Contains => (
                payload_with_path(
                    &field.path,
                    serde_json::json!(format!(
                        "prefix {} suffix",
                        value.as_str().expect("contains value")
                    )),
                ),
                payload_with_path(&field.path, serde_json::json!("prefix haystack suffix")),
            ),
            EventOperator::Equals => (
                payload_with_path(&field.path, value.clone()),
                payload_with_path(&field.path, nonmatching_equals_value(field, value)),
            ),
            EventOperator::Matches => (
                payload_with_path(&field.path, serde_json::json!("match-42")),
                payload_with_path(&field.path, serde_json::json!("no-match")),
            ),
            EventOperator::Exists => unreachable!(),
        }
    }

    fn nonmatching_equals_value(field: &EventField, value: &Value) -> Value {
        match value {
            Value::Bool(value) => serde_json::json!(!value),
            Value::Number(value) => {
                if value.as_i64() == Some(7) {
                    serde_json::json!(8)
                } else {
                    serde_json::json!(7)
                }
            }
            Value::String(value) => {
                if let Some(other) = field.values.iter().find_map(|candidate| {
                    let candidate = candidate.value.as_str()?;
                    (candidate != value).then(|| candidate.to_string())
                }) {
                    serde_json::json!(other)
                } else {
                    serde_json::json!(format!("{value}-other"))
                }
            }
            other => panic!("unsupported matrix equals value: {other}"),
        }
    }

    fn payload_with_path(path: &str, value: Value) -> Value {
        fn insert_path(current: &mut Value, parts: &[&str], value: Value) {
            if parts.len() == 1 {
                current
                    .as_object_mut()
                    .expect("payload object")
                    .insert(parts[0].to_string(), value);
                return;
            }
            let next = current
                .as_object_mut()
                .expect("payload object")
                .entry(parts[0])
                .or_insert_with(|| serde_json::json!({}));
            insert_path(next, &parts[1..], value);
        }

        let mut payload = serde_json::json!({});
        let parts = path.split('.').collect::<Vec<_>>();
        insert_path(&mut payload, &parts, value);
        payload
    }

    fn monitor_hard_skips_payload(payload: &Value) -> bool {
        payload
            .get("notification_muted")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || payload
                .get("notification_silent")
                .and_then(Value::as_bool)
                .unwrap_or(false)
    }

    fn expected_matrix_match(mode: MatrixMode, filter_matched: bool, payload: &Value) -> bool {
        if monitor_hard_skips_payload(payload) {
            return false;
        }
        match mode {
            MatrixMode::Include => filter_matched,
            MatrixMode::Skip => !filter_matched,
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
    fn monitor_filter_ui_template_matrix_routes_every_schema_condition_and_mode() {
        let classifier: Arc<dyn Classifier> = Arc::new(NullClassifier);
        let dir = tempdir().unwrap();
        let store = WorkflowBindingStore::load(dir.path().join("bindings.json")).unwrap();
        let (recording, dispatcher) = recording_dispatcher();
        let mut per_connector = std::collections::BTreeMap::new();
        let mut cases = Vec::new();

        for (connector_slug, schema_slug, schema, expected_count) in schemas_for_ui_matrix() {
            let mut schema_cases = keyword_matrix_cases(connector_slug, schema_slug);
            schema_cases.extend(field_matrix_cases(connector_slug, schema_slug, &schema));
            assert_eq!(
                schema_cases.len(),
                expected_count,
                "{schema_slug} UI template count changed"
            );
            per_connector.insert(schema_slug, schema_cases.len());
            cases.extend(schema_cases);
        }

        assert_eq!(per_connector["telegram-user"], 25);
        assert_eq!(per_connector["gmail-browser"], 19);
        assert_eq!(per_connector["gcal-browser"], 6);
        assert_eq!(per_connector["email"], 19);
        assert_eq!(per_connector["telegram-bot"], 7);
        assert_eq!(per_connector["lark-login"], 15);
        assert_eq!(per_connector["lark-bot"], 15);
        assert_eq!(cases.len(), 106);

        let mut mode_template_count = 0;
        let mut expected_actions = 0;
        for (idx, case) in cases.iter().enumerate() {
            for mode in [MatrixMode::Include, MatrixMode::Skip] {
                mode_template_count += 1;
                let mode_name = match mode {
                    MatrixMode::Include => "include",
                    MatrixMode::Skip => "skip",
                };
                let connection_slug = format!("matrix-{idx}-{mode_name}");
                let (include_filter, ignore_filters) = match mode {
                    MatrixMode::Include => (Some(case.filter.clone()), Vec::new()),
                    MatrixMode::Skip => (None, vec![case.filter.clone()]),
                };
                store
                    .create(triage_binding(
                        &connection_slug,
                        case.connector_slug,
                        include_filter,
                        ignore_filters,
                    ))
                    .unwrap();

                let matching = process_envelope_result(
                    &event(
                        &connection_slug,
                        &case.matching_text,
                        case.matching_payload.clone(),
                    ),
                    &store,
                    None,
                    &dispatcher,
                    &classifier,
                    None,
                );
                let nonmatching = process_envelope_result(
                    &event(
                        &connection_slug,
                        &case.nonmatching_text,
                        case.nonmatching_payload.clone(),
                    ),
                    &store,
                    None,
                    &dispatcher,
                    &classifier,
                    None,
                );

                let expected_matching =
                    expected_matrix_match(mode, true, &case.matching_payload);
                let expected_nonmatching =
                    expected_matrix_match(mode, false, &case.nonmatching_payload);
                expected_actions += usize::from(expected_matching);
                expected_actions += usize::from(expected_nonmatching);

                assert_eq!(
                    matching.matched,
                    expected_matching,
                    "{} {} {mode_name} matching event route mismatch",
                    case.schema_slug,
                    case.label
                );
                assert_eq!(matching.acted, u64::from(expected_matching));
                assert_eq!(
                    nonmatching.matched,
                    expected_nonmatching,
                    "{} {} {mode_name} nonmatching event route mismatch",
                    case.schema_slug,
                    case.label
                );
                assert_eq!(nonmatching.acted, u64::from(expected_nonmatching));
            }
        }

        assert_eq!(mode_template_count, 212);
        assert_eq!(expected_actions, 208);
        assert_eq!(recording.topics.lock().unwrap().len(), expected_actions);
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
