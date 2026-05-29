//! Workflow binding router — the loop that consumes connector events and
//! invokes matching workflow bindings.

use crate::action::{ActionDispatcher, BuiltinActionDispatcher};
use crate::classify::{Classifier, ClassifyDecision, NullClassifier};
use crate::history::{now_ms, WorkflowHistoryStore};
use crate::spec::{
    filter_matches, ActionSpec, FilterSpec, WorkflowBindingSpec, WorkflowBindingStatus,
};
use crate::store::WorkflowBindingStore;
use puffer_subscriber_runtime::{EventBus, EventEnvelope, EventReceiver};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::{self, JoinHandle};

/// Aggregate counters surfaced by workflow and connection status views.
#[derive(Debug, Default)]
pub struct RouterStats {
    /// Total events the router observed (regardless of match).
    pub events_seen: AtomicU64,
    /// Events that matched at least one subscription.
    pub events_matched: AtomicU64,
    /// Events that triggered a successful action.
    pub events_acted: AtomicU64,
    /// Events whose action failed.
    pub events_failed: AtomicU64,
}

impl RouterStats {
    fn snapshot(&self) -> [u64; 4] {
        [
            self.events_seen.load(Ordering::Relaxed),
            self.events_matched.load(Ordering::Relaxed),
            self.events_acted.load(Ordering::Relaxed),
            self.events_failed.load(Ordering::Relaxed),
        ]
    }

    /// Returns a `(seen, matched, acted, failed)` snapshot.
    pub fn snapshot_tuple(&self) -> (u64, u64, u64, u64) {
        let v = self.snapshot();
        (v[0], v[1], v[2], v[3])
    }
}

/// Router task wrapper. Holds the join handle and a shutdown trigger.
pub struct SubscriptionRouter {
    shutdown_tx: watch::Sender<bool>,
    join: Option<JoinHandle<()>>,
    stats: Arc<RouterStats>,
}

impl SubscriptionRouter {
    /// Spawns the router task. The `dispatcher` and `classifier` are
    /// shared across all events; `store` is consulted per-event so spec
    /// changes (create/pause/delete) take effect on the next event.
    pub fn spawn(
        bus: EventBus,
        store: Arc<WorkflowBindingStore>,
        history_store: Option<Arc<WorkflowHistoryStore>>,
        dispatcher: Arc<dyn ActionDispatcher>,
        classifier: Arc<dyn Classifier>,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let stats = Arc::new(RouterStats::default());
        let stats_for_task = stats.clone();
        let rx = bus.subscribe();
        let join = tokio::spawn(async move {
            run(
                rx,
                store,
                history_store,
                dispatcher,
                classifier,
                shutdown_rx,
                stats_for_task,
            )
            .await;
        });
        Self {
            shutdown_tx,
            join: Some(join),
            stats,
        }
    }

    /// Convenience constructor that uses [`BuiltinActionDispatcher`] and
    /// [`NullClassifier`].
    pub fn spawn_default(bus: EventBus, store: Arc<WorkflowBindingStore>) -> Self {
        Self::spawn(
            bus,
            store,
            None,
            Arc::new(BuiltinActionDispatcher::new()),
            Arc::new(NullClassifier),
        )
    }

    /// Returns the shared stats handle.
    pub fn stats(&self) -> Arc<RouterStats> {
        self.stats.clone()
    }

    /// Fires the shutdown signal and awaits the task.
    pub async fn shutdown(mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.join.take() {
            let _ = handle.await;
        }
    }
}

async fn run(
    mut rx: EventReceiver,
    store: Arc<WorkflowBindingStore>,
    history_store: Option<Arc<WorkflowHistoryStore>>,
    dispatcher: Arc<dyn ActionDispatcher>,
    classifier: Arc<dyn Classifier>,
    mut shutdown_rx: watch::Receiver<bool>,
    stats: Arc<RouterStats>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            maybe = rx.recv() => {
                let Some(envelope) = maybe else { break; };
                if envelope.event.control {
                    continue;
                }
                stats.events_seen.fetch_add(1, Ordering::Relaxed);
                let result = process_envelope_blocking(
                    envelope,
                    store.clone(),
                    history_store.clone(),
                    dispatcher.clone(),
                    classifier.clone(),
                    stats.clone(),
                )
                .await;
                if result.matched {
                    stats.events_matched.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

async fn process_envelope_blocking(
    envelope: EventEnvelope,
    store: Arc<WorkflowBindingStore>,
    history_store: Option<Arc<WorkflowHistoryStore>>,
    dispatcher: Arc<dyn ActionDispatcher>,
    classifier: Arc<dyn Classifier>,
    stats: Arc<RouterStats>,
) -> EnvelopeProcessResult {
    let stats_for_processing = stats.clone();
    match task::spawn_blocking(move || {
        process_envelope_result(
            &envelope,
            &store,
            history_store.as_deref(),
            &dispatcher,
            &classifier,
            Some(stats_for_processing.as_ref()),
        )
    })
    .await
    {
        Ok(result) => result,
        Err(error) => {
            stats.events_failed.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                %error,
                "workflow binding event processing task failed"
            );
            EnvelopeProcessResult::default()
        }
    }
}

/// Summary of processing one event envelope against workflow bindings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnvelopeProcessResult {
    /// Whether at least one enabled binding matched the envelope.
    pub matched: bool,
    /// Number of matched actions that completed successfully.
    pub acted: u64,
    /// Number of matched actions that failed.
    pub failed: u64,
}

/// Processes one event envelope against the current workflow bindings.
pub fn process_envelope(
    envelope: &EventEnvelope,
    store: &WorkflowBindingStore,
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    classifier: &Arc<dyn Classifier>,
    stats: Option<&RouterStats>,
) -> bool {
    process_envelope_result(
        envelope,
        store,
        history_store,
        dispatcher,
        classifier,
        stats,
    )
    .matched
}

/// Processes one event envelope and returns match/action/failure counts.
pub fn process_envelope_result(
    envelope: &EventEnvelope,
    store: &WorkflowBindingStore,
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    classifier: &Arc<dyn Classifier>,
    stats: Option<&RouterStats>,
) -> EnvelopeProcessResult {
    let mut result = EnvelopeProcessResult::default();
    if envelope.event.control {
        return result;
    }
    for spec in store.list() {
        if spec.status == WorkflowBindingStatus::Paused {
            continue;
        }
        let topic_matches = spec.connection_slug == envelope.event.topic
            || spec
                .connector_slug
                .as_deref()
                .is_some_and(|connector_slug| connector_slug == envelope.event.topic);
        if !topic_matches {
            continue;
        }
        if monitor_binding_should_skip_event(&spec, &envelope.event.payload) {
            continue;
        }
        if ignore_filter_matches(&spec, &envelope.event.text, &envelope.event.payload) {
            continue;
        }
        if !filter_matches(
            spec.filter.as_ref(),
            &envelope.event.text,
            &envelope.event.payload,
        ) {
            continue;
        }
        if spec.classify_prompt.is_some() {
            match classifier.classify(&spec, &envelope.event) {
                ClassifyDecision::Pass => {}
                ClassifyDecision::Reject | ClassifyDecision::Inconclusive => continue,
            }
        }
        result.matched = true;
        let started_at_ms = now_ms();
        let action_result = dispatcher.dispatch(&spec.action, envelope);
        let ended_at_ms = now_ms();
        if let Some(history_store) = history_store {
            if let Err(error) = history_store.append_action_result(
                &spec,
                envelope,
                &spec.action,
                &action_result,
                started_at_ms,
                ended_at_ms,
            ) {
                tracing::warn!(
                    workflow_binding = %spec.slug,
                    envelope = %envelope.envelope_id,
                    %error,
                    "failed to persist workflow binding run history"
                );
            }
        }
        if action_result.success {
            result.acted += 1;
            if let Some(stats) = stats {
                stats.events_acted.fetch_add(1, Ordering::Relaxed);
            }
            tracing::info!(
                workflow_binding = %spec.slug,
                envelope = %envelope.envelope_id,
                "{}",
                action_result.summary
            );
        } else {
            result.failed += 1;
            if let Some(stats) = stats {
                stats.events_failed.fetch_add(1, Ordering::Relaxed);
            }
            tracing::warn!(
                workflow_binding = %spec.slug,
                envelope = %envelope.envelope_id,
                "{}",
                action_result.summary
            );
        }
    }
    result
}

fn monitor_binding_should_skip_event(spec: &WorkflowBindingSpec, payload: &Value) -> bool {
    if !is_monitor_binding(spec) {
        return false;
    }
    payload_bool(payload, "notification_muted") || payload_bool(payload, "notification_silent")
}

fn ignore_filter_matches(spec: &WorkflowBindingSpec, text: &str, payload: &Value) -> bool {
    spec.ignore_filters
        .iter()
        .any(|filter| filter_matches(Some(filter), text, payload))
}

fn is_monitor_binding(spec: &WorkflowBindingSpec) -> bool {
    spec.slug.starts_with("monitor-")
        || (matches!(spec.action, ActionSpec::TriageAgent { .. })
            && spec.description.to_ascii_lowercase().contains("monitor"))
}

fn payload_bool(payload: &Value, key: &str) -> bool {
    payload.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Free-standing helper used by tests and by future explicit "test this
/// workflow binding" tooling. Returns whether the filter passes.
pub fn prefilter_passes(filter: Option<&FilterSpec>, text: &str) -> bool {
    filter_matches(filter, text, &serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{ActionResult, BuiltinActionDispatcher};
    use crate::classify::NullClassifier;
    use crate::spec::{ActionSpec, FileAppendFormat, TaggedFilterSpec, WorkflowBindingSpec};
    use puffer_subscriber_runtime::{Event, EventBus, EventEnvelope};
    use std::sync::Arc;
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

        let ignored =
            process_envelope_result(&envelope, &store, None, &dispatcher, &classifier, None);
        assert!(!ignored.matched);
        assert_eq!(ignored.acted, 0);

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
                ActionResult {
                    success: true,
                    summary: "runtime dropped".into(),
                }
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
}
