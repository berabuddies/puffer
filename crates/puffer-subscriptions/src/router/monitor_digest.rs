//! Delayed monitor triage queue.

use super::{
    dispatch_matched_batch, is_monitor_binding, ActionDispatcher, ActionSpec, EnvelopeProcessResult,
};
use crate::history::WorkflowHistoryStore;
use crate::spec::WorkflowBindingSpec;
use puffer_subscriber_runtime::EventEnvelope;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Handle;

#[derive(Clone)]
pub(crate) struct MonitorDigestQueue {
    inner: Arc<Mutex<MonitorDigestInner>>,
    dispatcher: Arc<dyn ActionDispatcher>,
    history_store: Arc<WorkflowHistoryStore>,
    interval: Duration,
    handle: Handle,
}

#[derive(Default)]
struct MonitorDigestInner {
    pending: BTreeMap<String, MonitorDigestBucket>,
    scheduled: bool,
}

struct MonitorDigestBucket {
    spec: WorkflowBindingSpec,
    envelopes: Vec<EventEnvelope>,
}

impl MonitorDigestQueue {
    /// Creates a queue that flushes monitor triage events on `interval`.
    pub(crate) fn new(
        handle: Handle,
        dispatcher: Arc<dyn ActionDispatcher>,
        history_store: Arc<WorkflowHistoryStore>,
        interval: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MonitorDigestInner::default())),
            dispatcher,
            history_store,
            interval,
            handle,
        }
    }

    /// Returns whether this queue owns delayed dispatch for `spec`.
    pub(crate) fn handles(&self, spec: &WorkflowBindingSpec) -> bool {
        is_monitor_binding(spec) && matches!(spec.action, ActionSpec::TriageAgent { .. })
    }

    /// Adds one already-matched monitor event to the next digest.
    pub(crate) fn enqueue(&self, spec: &WorkflowBindingSpec, envelope: &EventEnvelope) {
        self.enqueue_owned(spec.clone(), envelope.clone());
    }

    /// Adds several already-matched monitor events to the next digest.
    pub(crate) fn enqueue_batch(&self, spec: &WorkflowBindingSpec, envelopes: &[&EventEnvelope]) {
        for envelope in envelopes {
            self.enqueue_owned(spec.clone(), (*envelope).clone());
        }
    }

    fn enqueue_owned(&self, spec: WorkflowBindingSpec, envelope: EventEnvelope) {
        let should_schedule = {
            let mut inner = self.inner.lock().unwrap();
            inner
                .pending
                .entry(spec.slug.clone())
                .and_modify(|bucket| bucket.envelopes.push(envelope.clone()))
                .or_insert_with(|| MonitorDigestBucket {
                    spec,
                    envelopes: vec![envelope],
                });
            if inner.scheduled {
                false
            } else {
                inner.scheduled = true;
                true
            }
        };
        if should_schedule {
            self.schedule_flush();
        }
    }

    fn schedule_flush(&self) {
        let queue = self.clone();
        self.handle.spawn(async move {
            tokio::time::sleep(queue.interval).await;
            queue.flush().await;
        });
    }

    /// Flushes every pending monitor digest immediately.
    pub(crate) async fn flush(&self) {
        let batches = {
            let mut inner = self.inner.lock().unwrap();
            inner.scheduled = false;
            std::mem::take(&mut inner.pending)
        };
        if batches.is_empty() {
            return;
        }
        let dispatcher = self.dispatcher.clone();
        let history_store = self.history_store.clone();
        let processed = tokio::task::spawn_blocking(move || {
            for (_, bucket) in batches {
                let envelopes = bucket.envelopes.iter().collect::<Vec<_>>();
                let mut result = EnvelopeProcessResult::default();
                dispatch_matched_batch(
                    &bucket.spec,
                    &envelopes,
                    Some(history_store.as_ref()),
                    &dispatcher,
                    None,
                    &mut result,
                );
            }
        })
        .await;
        if let Err(error) = processed {
            tracing::warn!(%error, "monitor digest flush task failed");
        }
    }
}
