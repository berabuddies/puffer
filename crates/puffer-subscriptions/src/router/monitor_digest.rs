//! Delayed monitor triage queue.

use super::{
    dispatch_matched_batch, is_monitor_binding, ActionDispatcher, ActionSpec, EnvelopeProcessResult,
};
use crate::history::WorkflowHistoryStore;
use crate::monitor_trace::{MonitorTraceStage, MonitorTraceStore};
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
    trace_store: Arc<MonitorTraceStore>,
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
        trace_store: Arc<MonitorTraceStore>,
        interval: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MonitorDigestInner::default())),
            dispatcher,
            history_store,
            trace_store,
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
        let trace_store = self.trace_store.clone();
        let processed = tokio::task::spawn_blocking(move || {
            for (_, bucket) in batches {
                let envelopes = bucket.envelopes.iter().collect::<Vec<_>>();
                let batch_id = digest_batch_id(&bucket.spec, &bucket.envelopes);
                let count = bucket.envelopes.len();
                for (index, envelope) in bucket.envelopes.iter().enumerate() {
                    let stage = MonitorTraceStage::completed(
                        "router_digest_flushed",
                        "monitor_digest",
                        "Monitor digest batch was flushed for triage.",
                        crate::history::now_ms(),
                    )
                    .with_binding(bucket.spec.slug.clone())
                    .with_digest(batch_id.clone(), count, index + 1);
                    if let Err(error) = trace_store.record_envelope_stage(
                        &bucket.spec.connection_slug,
                        bucket.spec.connector_slug.as_deref(),
                        envelope,
                        stage,
                    ) {
                        tracing::warn!(
                            workflow_binding = %bucket.spec.slug,
                            envelope = %envelope.envelope_id,
                            %error,
                            "failed to persist monitor digest trace stage"
                        );
                    }
                }
                let mut result = EnvelopeProcessResult::default();
                dispatch_matched_batch(
                    &bucket.spec,
                    &envelopes,
                    Some(history_store.as_ref()),
                    Some(trace_store.as_ref()),
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

fn digest_batch_id(spec: &WorkflowBindingSpec, envelopes: &[EventEnvelope]) -> String {
    let mut key = spec.slug.clone();
    for envelope in envelopes {
        key.push('|');
        key.push_str(&envelope.envelope_id);
    }
    format!("digest-{:x}", stable_hash(&key))
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
