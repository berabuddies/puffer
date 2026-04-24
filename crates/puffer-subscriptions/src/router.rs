//! Subscription router — the loop that consumes events from the bus and
//! invokes matching subscriptions.

use crate::action::{ActionDispatcher, BuiltinActionDispatcher};
use crate::classify::{ClassifyDecision, Classifier, NullClassifier};
use crate::spec::{PrefilterSpec, SubscriptionStatus};
use crate::store::SubscriptionStore;
use puffer_subscriber_runtime::{EventBus, EventEnvelope};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Aggregate counters surfaced by `/subscriptions status`.
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
        store: Arc<SubscriptionStore>,
        dispatcher: Arc<dyn ActionDispatcher>,
        classifier: Arc<dyn Classifier>,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let stats = Arc::new(RouterStats::default());
        let stats_for_task = stats.clone();
        let join = tokio::spawn(async move {
            run(bus, store, dispatcher, classifier, shutdown_rx, stats_for_task).await;
        });
        Self {
            shutdown_tx,
            join: Some(join),
            stats,
        }
    }

    /// Convenience constructor that uses [`BuiltinActionDispatcher`] and
    /// [`NullClassifier`].
    pub fn spawn_default(bus: EventBus, store: Arc<SubscriptionStore>) -> Self {
        Self::spawn(
            bus,
            store,
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
    bus: EventBus,
    store: Arc<SubscriptionStore>,
    dispatcher: Arc<dyn ActionDispatcher>,
    classifier: Arc<dyn Classifier>,
    mut shutdown_rx: watch::Receiver<bool>,
    stats: Arc<RouterStats>,
) {
    let mut rx = bus.subscribe();
    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            maybe = rx.recv() => {
                let Some(envelope) = maybe else { break; };
                stats.events_seen.fetch_add(1, Ordering::Relaxed);
                let any_match = process(&envelope, &store, &dispatcher, &classifier, &stats);
                if any_match {
                    stats.events_matched.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

fn process(
    envelope: &EventEnvelope,
    store: &SubscriptionStore,
    dispatcher: &Arc<dyn ActionDispatcher>,
    classifier: &Arc<dyn Classifier>,
    stats: &RouterStats,
) -> bool {
    let mut any_match = false;
    for spec in store.list() {
        if spec.status == SubscriptionStatus::Paused {
            continue;
        }
        if spec.source_topic != envelope.event.topic {
            continue;
        }
        if let Some(filter) = &spec.prefilter {
            if !regex_passes(filter, &envelope.event.text) {
                continue;
            }
        }
        if spec.classify_prompt.is_some() {
            match classifier.classify(&spec, &envelope.event) {
                ClassifyDecision::Pass => {}
                ClassifyDecision::Reject | ClassifyDecision::Inconclusive => continue,
            }
        }
        any_match = true;
        let result = dispatcher.dispatch(&spec.action, envelope);
        if result.success {
            stats.events_acted.fetch_add(1, Ordering::Relaxed);
            tracing::info!(
                subscription = %spec.id,
                envelope = %envelope.envelope_id,
                "{}",
                result.summary
            );
        } else {
            stats.events_failed.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                subscription = %spec.id,
                envelope = %envelope.envelope_id,
                "{}",
                result.summary
            );
        }
    }
    any_match
}

fn regex_passes(filter: &PrefilterSpec, text: &str) -> bool {
    match filter {
        PrefilterSpec::Regex {
            pattern,
            case_insensitive,
        } => {
            let mut builder = regex::RegexBuilder::new(pattern);
            builder.case_insensitive(*case_insensitive);
            match builder.build() {
                Ok(re) => re.is_match(text),
                Err(error) => {
                    tracing::warn!(%error, "regex prefilter failed to compile; rejecting event");
                    false
                }
            }
        }
    }
}

/// Free-standing helper used by tests and by future explicit "test this
/// subscription" tooling. Returns whether the spec's prefilter passes
/// against `text`.
pub fn prefilter_passes(filter: Option<&PrefilterSpec>, text: &str) -> bool {
    match filter {
        Some(filter) => regex_passes(filter, text),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_insensitive_regex_matches() {
        let filter = PrefilterSpec::Regex {
            pattern: r"\bIoC\b".into(),
            case_insensitive: true,
        };
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
}
