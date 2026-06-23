//! Per-conversation debounce for monitor triage events.

use crate::action::ActionDispatcher;
use crate::classify::Classifier;
use crate::history::WorkflowHistoryStore;
use crate::router::{process_envelope_batch_result, RouterStats};
use crate::self_gate::SelfMessageGate;
use crate::spec::{ActionSpec, WorkflowBindingStatus};
use crate::store::WorkflowBindingStore;
use puffer_subscriber_runtime::EventEnvelope;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task;

pub(crate) const MONITOR_DEBOUNCE_DELAY: Duration = Duration::from_secs(45);

#[derive(Debug, Default)]
struct PendingBatch {
    generation: u64,
    envelopes: Vec<EventEnvelope>,
}

/// Shared debounce queue for live monitor events.
#[derive(Clone, Debug, Default)]
pub(crate) struct MonitorDebounce {
    pending: Arc<Mutex<HashMap<String, PendingBatch>>>,
}

impl MonitorDebounce {
    /// Creates an empty debounce queue.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Queues an envelope when monitor triage should be delayed.
    pub(crate) fn enqueue(
        &self,
        envelope: EventEnvelope,
        store: Arc<WorkflowBindingStore>,
        history_store: Option<Arc<WorkflowHistoryStore>>,
        dispatcher: Arc<dyn ActionDispatcher>,
        classifier: Arc<dyn Classifier>,
        gate: Arc<dyn SelfMessageGate>,
        stats: Arc<RouterStats>,
        permits: Arc<Semaphore>,
    ) -> bool {
        let Some(key) = monitor_debounce_key(&envelope, &store) else {
            return false;
        };
        let generation = {
            let mut pending = self.pending.lock().unwrap();
            let batch = pending.entry(key.clone()).or_default();
            batch.generation = batch.generation.saturating_add(1);
            batch.envelopes.push(envelope);
            batch.generation
        };
        let debounce = self.clone();
        task::spawn(async move {
            tokio::time::sleep(MONITOR_DEBOUNCE_DELAY).await;
            debounce
                .flush_if_current(
                    key,
                    generation,
                    store,
                    history_store,
                    dispatcher,
                    classifier,
                    gate,
                    stats,
                    permits,
                )
                .await;
        });
        true
    }

    async fn flush_if_current(
        &self,
        key: String,
        generation: u64,
        store: Arc<WorkflowBindingStore>,
        history_store: Option<Arc<WorkflowHistoryStore>>,
        dispatcher: Arc<dyn ActionDispatcher>,
        classifier: Arc<dyn Classifier>,
        gate: Arc<dyn SelfMessageGate>,
        stats: Arc<RouterStats>,
        permits: Arc<Semaphore>,
    ) {
        let envelopes = {
            let mut pending = self.pending.lock().unwrap();
            let Some(batch) = pending.get(&key) else {
                return;
            };
            if batch.generation != generation {
                return;
            }
            pending.remove(&key).unwrap().envelopes
        };
        task::spawn(async move {
            let _permit = match permits.acquire_owned().await {
                Ok(permit) => permit,
                Err(error) => {
                    stats
                        .events_failed
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(%error, "workflow debounce processor semaphore closed");
                    return;
                }
            };
            let stats_for_processing = stats.clone();
            let processed = task::spawn_blocking(move || {
                process_envelope_batch_result(
                    &envelopes,
                    &store,
                    history_store.as_deref(),
                    &dispatcher,
                    &classifier,
                    Some(stats_for_processing.as_ref()),
                    &gate,
                )
            })
            .await;
            match processed {
                Ok(result) if result.matched => {
                    stats
                        .events_matched
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(_) => {}
                Err(error) => {
                    stats
                        .events_failed
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(%error, "workflow debounce processing task failed");
                }
            }
        });
    }
}

/// Returns the stable same-conversation debounce key for a monitor event.
pub(crate) fn monitor_debounce_key(
    envelope: &EventEnvelope,
    store: &WorkflowBindingStore,
) -> Option<String> {
    if !store_has_debounceable_monitor(store, envelope) {
        return None;
    }
    conversation_scope(&envelope.event.payload)
        .map(|scope| format!("{}:{scope}", envelope.event.topic))
}

fn store_has_debounceable_monitor(store: &WorkflowBindingStore, envelope: &EventEnvelope) -> bool {
    let mut has_monitor = false;
    for spec in store.list() {
        if spec.status == WorkflowBindingStatus::Paused || !topic_matches(&spec, envelope) {
            continue;
        }
        if !matches!(spec.action, ActionSpec::TriageAgent { .. }) {
            return false;
        }
        if spec.slug.starts_with("monitor-") {
            has_monitor = true;
        }
    }
    has_monitor
}

fn topic_matches(spec: &crate::spec::WorkflowBindingSpec, envelope: &EventEnvelope) -> bool {
    spec.connection_slug == envelope.event.topic
        || spec
            .connector_slug
            .as_deref()
            .is_some_and(|connector_slug| connector_slug == envelope.event.topic)
}

pub(crate) fn conversation_scope(payload: &Value) -> Option<String> {
    let object = payload.as_object()?;
    for key in [
        "thread_id",
        "threadId",
        "conversation_id",
        "conversationId",
        "chat_id",
        "chatId",
        "channel_id",
        "channelId",
        "room_id",
        "roomId",
    ] {
        if let Some(value) = object.get(key).and_then(stable_scalar) {
            return Some(format!("{key}={value}"));
        }
    }
    None
}

fn stable_scalar(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
        .or_else(|| value.as_u64().map(|value| value.to_string()))
}
