//! In-process broadcast bus for subscriber events.
//!
//! Events are fanned out by topic. Subscribers (publishers) call
//! [`EventBus::publish`]. Consumers (the subscription router, the TUI
//! "live tail" pane, etc.) call [`EventBus::subscribe`] to get an
//! [`EventReceiver`] that sees every subsequent event whose topic matches.
//!
//! The bus uses bounded channels with drop-oldest semantics: if a consumer
//! falls behind it will see a `lagged` warning but will not block the
//! publisher. That matches the "telemetry-like" use case we want — no
//! subscriber ever waits on a slow consumer.

use crate::event::EventEnvelope;
use tokio::sync::broadcast;

/// Default capacity per-topic for the broadcast channel. Tuned for
/// telemetry-like traffic: enough to absorb brief consumer pauses, small
/// enough that we don't hold many unread events in memory.
const DEFAULT_CAPACITY: usize = 1024;

/// A simple topic-keyed broadcast bus. Internally holds one
/// `tokio::sync::broadcast::Sender<EventEnvelope>` and filters on the
/// receiver side — topics are low-cardinality (one per subscriber skill)
/// and this keeps the implementation tiny.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<EventEnvelope>,
}

/// A receiver yielded by [`EventBus::subscribe`]. Wraps a raw broadcast
/// receiver and optionally filters by topic prefix.
pub struct EventReceiver {
    rx: broadcast::Receiver<EventEnvelope>,
    topic_filter: Option<String>,
}

impl EventBus {
    /// Creates a new bus with the default per-channel capacity.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(DEFAULT_CAPACITY);
        Self { tx }
    }

    /// Publishes an envelope to all current subscribers. Returns the number
    /// of active receivers (useful for metrics; zero is not an error).
    pub fn publish(&self, envelope: EventEnvelope) -> usize {
        self.tx.send(envelope).unwrap_or(0)
    }

    /// Subscribes to all events. Use [`EventReceiver::with_topic`] to
    /// narrow by topic.
    pub fn subscribe(&self) -> EventReceiver {
        EventReceiver {
            rx: self.tx.subscribe(),
            topic_filter: None,
        }
    }

    /// Subscribes to events whose topic equals `topic`.
    pub fn subscribe_topic(&self, topic: impl Into<String>) -> EventReceiver {
        EventReceiver {
            rx: self.tx.subscribe(),
            topic_filter: Some(topic.into()),
        }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventReceiver {
    /// Narrows this receiver to only yield events whose topic equals `topic`.
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic_filter = Some(topic.into());
        self
    }

    /// Awaits the next matching envelope. Returns `None` when the sender
    /// has been dropped. Lagged messages are skipped with a `warn` log.
    pub async fn recv(&mut self) -> Option<EventEnvelope> {
        loop {
            match self.rx.recv().await {
                Ok(envelope) => {
                    if let Some(topic) = &self.topic_filter {
                        if envelope.event.topic != *topic {
                            continue;
                        }
                    }
                    return Some(envelope);
                }
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    tracing::warn!(dropped = count, "event bus lagged; dropped envelopes");
                    continue;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;

    fn envelope(topic: &str) -> EventEnvelope {
        EventEnvelope {
            envelope_id: "e1".into(),
            subscriber_id: "s1".into(),
            received_at_ms: 0,
            event: Event {
                topic: topic.into(),
                kind: "message".into(),
                control: false,
                dedup_key: None,
                text: String::new(),
                payload: serde_json::Value::Null,
            },
        }
    }

    #[tokio::test]
    async fn topic_filter_drops_unmatched() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe_topic("telegram-user");
        bus.publish(envelope("rss-hn"));
        bus.publish(envelope("telegram-user"));
        let got = rx.recv().await.unwrap();
        assert_eq!(got.event.topic, "telegram-user");
    }
}
