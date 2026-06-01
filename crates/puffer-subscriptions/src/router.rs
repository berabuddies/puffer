//! Workflow binding router — the loop that consumes connector events and
//! invokes matching workflow bindings.

use crate::action::{ActionDispatcher, BuiltinActionDispatcher};
use crate::classify::{Classifier, ClassifyDecision, NullClassifier};
use crate::history::{now_ms, WorkflowActionLog, WorkflowBindingRunStatus, WorkflowHistoryStore};
use crate::spec::{
    filter_matches, ActionSpec, FilterSpec, WorkflowBindingSpec, WorkflowBindingStatus,
};
use crate::store::WorkflowBindingStore;
use puffer_subscriber_runtime::{EventBus, EventEnvelope, EventReceiver};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::sync::Semaphore;
use tokio::task::{self, JoinHandle};

const MAX_CONCURRENT_EVENT_PROCESSORS: usize = 32;

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
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_EVENT_PROCESSORS));
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            maybe = rx.recv() => {
                let Some(envelope) = maybe else { break; };
                if envelope.event.control {
                    continue;
                }
                stats.events_seen.fetch_add(1, Ordering::Relaxed);
                spawn_envelope_processor(
                    envelope,
                    store.clone(),
                    history_store.clone(),
                    dispatcher.clone(),
                    classifier.clone(),
                    stats.clone(),
                    permits.clone(),
                );
            }
        }
    }
}

fn spawn_envelope_processor(
    envelope: EventEnvelope,
    store: Arc<WorkflowBindingStore>,
    history_store: Option<Arc<WorkflowHistoryStore>>,
    dispatcher: Arc<dyn ActionDispatcher>,
    classifier: Arc<dyn Classifier>,
    stats: Arc<RouterStats>,
    permits: Arc<Semaphore>,
) {
    task::spawn(async move {
        let _permit = match permits.acquire_owned().await {
            Ok(permit) => permit,
            Err(error) => {
                stats.events_failed.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(%error, "workflow binding processor semaphore closed");
                return;
            }
        };
        let result = process_envelope_blocking(
            envelope,
            store,
            history_store,
            dispatcher,
            classifier,
            stats.clone(),
        )
        .await;
        if result.matched {
            stats.events_matched.fetch_add(1, Ordering::Relaxed);
        }
    });
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
        if event_dedup_key_seen(history_store, &spec, envelope) {
            continue;
        }
        if monitor_binding_should_skip_event(&spec, &envelope.event.payload) {
            record_monitor_router_outcome(
                history_store,
                &spec,
                envelope,
                "monitor_muted_skip",
                "Muted or silent notification skipped before monitor triage.",
            );
            continue;
        }
        if ignore_filter_matches(&spec, &envelope.event.text, &envelope.event.payload) {
            record_monitor_router_outcome(
                history_store,
                &spec,
                envelope,
                "monitor_ignore_filter",
                "Matched an installed monitor ignore filter before triage.",
            );
            continue;
        }
        if !filter_matches(
            spec.filter.as_ref(),
            &envelope.event.text,
            &envelope.event.payload,
        ) {
            record_monitor_router_outcome(
                history_store,
                &spec,
                envelope,
                "monitor_filter_skip",
                "Did not match the monitor trigger filter.",
            );
            continue;
        }
        if spec.classify_prompt.is_some() {
            match classifier.classify(&spec, &envelope.event) {
                ClassifyDecision::Pass => {}
                ClassifyDecision::Reject | ClassifyDecision::Inconclusive => {
                    record_monitor_router_outcome(
                        history_store,
                        &spec,
                        envelope,
                        "monitor_classifier_skip",
                        "Classifier rejected the event before monitor triage.",
                    );
                    continue;
                }
            }
        }
        result.matched = true;
        let started_at_ms = now_ms();
        let started_history_idx = history_store.and_then(|history_store| {
            match history_store.append_action_started(&spec, envelope, &spec.action, started_at_ms)
            {
                Ok(run) => Some(run.idx),
                Err(error) => {
                    tracing::warn!(
                        workflow_binding = %spec.slug,
                        envelope = %envelope.envelope_id,
                        %error,
                        "failed to persist started workflow binding run history"
                    );
                    None
                }
            }
        });
        let action_result = dispatcher.dispatch(&spec.action, envelope);
        let ended_at_ms = now_ms();
        if let Some(history_store) = history_store {
            let persist_result = match started_history_idx {
                Some(idx) => match history_store.complete_action_result(
                    idx,
                    &spec.action,
                    &action_result,
                    started_at_ms,
                    ended_at_ms,
                ) {
                    Ok(Some(_)) => Ok(()),
                    Ok(None) => history_store
                        .append_action_result(
                            &spec,
                            envelope,
                            &spec.action,
                            &action_result,
                            started_at_ms,
                            ended_at_ms,
                        )
                        .map(|_| ()),
                    Err(error) => Err(error),
                },
                None => history_store
                    .append_action_result(
                        &spec,
                        envelope,
                        &spec.action,
                        &action_result,
                        started_at_ms,
                        ended_at_ms,
                    )
                    .map(|_| ()),
            };
            if let Err(error) = persist_result {
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

/// Processes a same-connection envelope batch and batches triage-agent
/// actions per matching binding.
pub fn process_envelope_batch_result(
    envelopes: &[EventEnvelope],
    store: &WorkflowBindingStore,
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    classifier: &Arc<dyn Classifier>,
    stats: Option<&RouterStats>,
) -> EnvelopeProcessResult {
    let mut result = EnvelopeProcessResult::default();
    let envelopes: Vec<&EventEnvelope> = envelopes
        .iter()
        .filter(|envelope| !envelope.event.control)
        .collect();
    if envelopes.is_empty() {
        return result;
    }
    for spec in store.list() {
        if spec.status == WorkflowBindingStatus::Paused {
            continue;
        }
        let mut triage_batch = Vec::new();
        for envelope in &envelopes {
            let Some(prefiltered) =
                prefilter_envelope_for_spec(&spec, envelope, history_store, classifier)
            else {
                continue;
            };
            result.matched = true;
            if matches!(spec.action, ActionSpec::TriageAgent { .. }) {
                triage_batch.push(prefiltered);
                continue;
            }
            dispatch_one_matched_envelope(
                &spec,
                prefiltered,
                history_store,
                dispatcher,
                stats,
                &mut result,
            );
        }
        if !triage_batch.is_empty() {
            dispatch_matched_batch(
                &spec,
                &triage_batch,
                history_store,
                dispatcher,
                stats,
                &mut result,
            );
        }
    }
    result
}

fn prefilter_envelope_for_spec<'a>(
    spec: &WorkflowBindingSpec,
    envelope: &'a EventEnvelope,
    history_store: Option<&WorkflowHistoryStore>,
    classifier: &Arc<dyn Classifier>,
) -> Option<&'a EventEnvelope> {
    let topic_matches = spec.connection_slug == envelope.event.topic
        || spec
            .connector_slug
            .as_deref()
            .is_some_and(|connector_slug| connector_slug == envelope.event.topic);
    if !topic_matches {
        return None;
    }
    if event_dedup_key_seen(history_store, spec, envelope) {
        return None;
    }
    if monitor_binding_should_skip_event(spec, &envelope.event.payload) {
        record_monitor_router_outcome(
            history_store,
            spec,
            envelope,
            "monitor_muted_skip",
            "Muted or silent notification skipped before monitor triage.",
        );
        return None;
    }
    if ignore_filter_matches(spec, &envelope.event.text, &envelope.event.payload) {
        record_monitor_router_outcome(
            history_store,
            spec,
            envelope,
            "monitor_ignore_filter",
            "Matched an installed monitor ignore filter before triage.",
        );
        return None;
    }
    if !filter_matches(
        spec.filter.as_ref(),
        &envelope.event.text,
        &envelope.event.payload,
    ) {
        record_monitor_router_outcome(
            history_store,
            spec,
            envelope,
            "monitor_filter_skip",
            "Did not match the monitor trigger filter.",
        );
        return None;
    }
    if spec.classify_prompt.is_some() {
        match classifier.classify(spec, &envelope.event) {
            ClassifyDecision::Pass => {}
            ClassifyDecision::Reject | ClassifyDecision::Inconclusive => {
                record_monitor_router_outcome(
                    history_store,
                    spec,
                    envelope,
                    "monitor_classifier_skip",
                    "Classifier rejected the event before monitor triage.",
                );
                return None;
            }
        }
    }
    Some(envelope)
}

fn dispatch_one_matched_envelope(
    spec: &WorkflowBindingSpec,
    envelope: &EventEnvelope,
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    stats: Option<&RouterStats>,
    result: &mut EnvelopeProcessResult,
) {
    let started_at_ms = now_ms();
    let started_history_idx = history_store.and_then(|history_store| {
        match history_store.append_action_started(spec, envelope, &spec.action, started_at_ms) {
            Ok(run) => Some(run.idx),
            Err(error) => {
                tracing::warn!(
                    workflow_binding = %spec.slug,
                    envelope = %envelope.envelope_id,
                    %error,
                    "failed to persist started workflow binding run history"
                );
                None
            }
        }
    });
    let action_result = dispatcher.dispatch(&spec.action, envelope);
    let ended_at_ms = now_ms();
    persist_action_result(
        spec,
        envelope,
        &action_result,
        started_at_ms,
        ended_at_ms,
        started_history_idx,
        history_store,
    );
    account_action_result(spec, envelope, &action_result, stats, result);
}

fn dispatch_matched_batch(
    spec: &WorkflowBindingSpec,
    envelopes: &[&EventEnvelope],
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    stats: Option<&RouterStats>,
    result: &mut EnvelopeProcessResult,
) {
    let started_at_ms = now_ms();
    let mut started_history = HashMap::new();
    if let Some(history_store) = history_store {
        for envelope in envelopes {
            match history_store.append_action_started(spec, envelope, &spec.action, started_at_ms) {
                Ok(run) => {
                    started_history.insert(envelope.envelope_id.clone(), run.idx);
                }
                Err(error) => {
                    tracing::warn!(
                        workflow_binding = %spec.slug,
                        envelope = %envelope.envelope_id,
                        %error,
                        "failed to persist started workflow binding run history"
                    );
                }
            }
        }
    }
    let batch: Vec<EventEnvelope> = envelopes
        .iter()
        .map(|envelope| (*envelope).clone())
        .collect();
    let action_result = dispatcher.dispatch_batch(&spec.action, &batch);
    let ended_at_ms = now_ms();
    for envelope in envelopes {
        let started_history_idx = started_history.get(&envelope.envelope_id).copied();
        persist_action_result(
            spec,
            envelope,
            &action_result,
            started_at_ms,
            ended_at_ms,
            started_history_idx,
            history_store,
        );
        account_action_result(spec, envelope, &action_result, stats, result);
    }
}

fn persist_action_result(
    spec: &WorkflowBindingSpec,
    envelope: &EventEnvelope,
    action_result: &crate::action::ActionResult,
    started_at_ms: i128,
    ended_at_ms: i128,
    started_history_idx: Option<u64>,
    history_store: Option<&WorkflowHistoryStore>,
) {
    if let Some(history_store) = history_store {
        let persist_result = match started_history_idx {
            Some(idx) => match history_store.complete_action_result(
                idx,
                &spec.action,
                action_result,
                started_at_ms,
                ended_at_ms,
            ) {
                Ok(Some(_)) => Ok(()),
                Ok(None) => history_store
                    .append_action_result(
                        spec,
                        envelope,
                        &spec.action,
                        action_result,
                        started_at_ms,
                        ended_at_ms,
                    )
                    .map(|_| ()),
                Err(error) => Err(error),
            },
            None => history_store
                .append_action_result(
                    spec,
                    envelope,
                    &spec.action,
                    action_result,
                    started_at_ms,
                    ended_at_ms,
                )
                .map(|_| ()),
        };
        if let Err(error) = persist_result {
            tracing::warn!(
                workflow_binding = %spec.slug,
                envelope = %envelope.envelope_id,
                %error,
                "failed to persist workflow binding run history"
            );
        }
    }
}

fn account_action_result(
    spec: &WorkflowBindingSpec,
    envelope: &EventEnvelope,
    action_result: &crate::action::ActionResult,
    stats: Option<&RouterStats>,
    result: &mut EnvelopeProcessResult,
) {
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

fn event_dedup_key_seen(
    history_store: Option<&WorkflowHistoryStore>,
    spec: &WorkflowBindingSpec,
    envelope: &EventEnvelope,
) -> bool {
    let Some(history_store) = history_store else {
        return false;
    };
    let Some(dedup_key) = envelope.event.dedup_key.as_deref() else {
        return false;
    };
    history_store.contains_dedup_key(&spec.slug, dedup_key)
}

fn monitor_binding_should_skip_event(spec: &WorkflowBindingSpec, payload: &Value) -> bool {
    if !is_monitor_binding(spec) {
        return false;
    }
    payload_bool(payload, "notification_muted") || payload_bool(payload, "notification_silent")
}

fn record_monitor_router_outcome(
    history_store: Option<&WorkflowHistoryStore>,
    spec: &WorkflowBindingSpec,
    envelope: &EventEnvelope,
    action: &str,
    summary: &str,
) {
    if !is_monitor_binding(spec) {
        return;
    }
    let Some(history_store) = history_store else {
        return;
    };
    let timestamp = now_ms();
    let log = WorkflowActionLog {
        action: action.to_string(),
        status: WorkflowBindingRunStatus::Completed,
        summary: summary.to_string(),
        started_at_ms: timestamp,
        ended_at_ms: timestamp,
        usage: None,
    };
    if let Err(error) = history_store.append_event_outcome(
        spec,
        envelope,
        log,
        WorkflowBindingRunStatus::Completed,
        timestamp,
        timestamp,
    ) {
        tracing::warn!(
            workflow_binding = %spec.slug,
            envelope = %envelope.envelope_id,
            %error,
            "failed to persist monitor router history"
        );
    }
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
include!("router_tests.rs");
