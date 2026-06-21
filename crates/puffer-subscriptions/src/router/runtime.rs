//! Async router task runtime.

use super::{
    process_envelope_result_with_monitor_digest, EnvelopeProcessResult, MonitorDigestQueue,
};
use crate::action::{ActionDispatcher, BuiltinActionDispatcher};
use crate::classify::{Classifier, NullClassifier};
use crate::history::WorkflowHistoryStore;
use crate::self_gate::{DropAllSelfGate, SelfMessageGate};
use crate::store::WorkflowBindingStore;
use puffer_subscriber_runtime::{EventBus, EventEnvelope, EventReceiver};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{watch, Semaphore};
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
        gate: Arc<dyn SelfMessageGate>,
    ) -> Self {
        Self::spawn_with_monitor_digest(
            bus,
            store,
            history_store,
            dispatcher,
            classifier,
            gate,
            None,
        )
    }

    /// Spawns the router task with an optional delayed monitor digest queue.
    pub(crate) fn spawn_with_monitor_digest(
        bus: EventBus,
        store: Arc<WorkflowBindingStore>,
        history_store: Option<Arc<WorkflowHistoryStore>>,
        dispatcher: Arc<dyn ActionDispatcher>,
        classifier: Arc<dyn Classifier>,
        gate: Arc<dyn SelfMessageGate>,
        monitor_digest: Option<MonitorDigestQueue>,
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
                gate,
                monitor_digest,
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
            Arc::new(DropAllSelfGate),
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
    gate: Arc<dyn SelfMessageGate>,
    monitor_digest: Option<MonitorDigestQueue>,
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
                    gate.clone(),
                    monitor_digest.clone(),
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
    gate: Arc<dyn SelfMessageGate>,
    monitor_digest: Option<MonitorDigestQueue>,
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
            gate,
            monitor_digest,
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
    gate: Arc<dyn SelfMessageGate>,
    monitor_digest: Option<MonitorDigestQueue>,
    stats: Arc<RouterStats>,
) -> EnvelopeProcessResult {
    let stats_for_processing = stats.clone();
    match task::spawn_blocking(move || {
        process_envelope_result_with_monitor_digest(
            &envelope,
            &store,
            history_store.as_deref(),
            &dispatcher,
            &classifier,
            &gate,
            monitor_digest.as_ref(),
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
