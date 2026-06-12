//! Self-report completion lane of the subscription pipeline.
//!
//! A subscriber's *outgoing* (self-sent) messages must never enter the triage
//! pipeline (that is the #569 credit-burn guard). They are instead routed to a
//! [`SelfReportHandler`], which inspects the message for evidence that the user
//! just completed an open monitor task in the same conversation.
//!
//! Like [`crate::Classifier`], the trait is defined here so the router signature
//! stays in this crate, while the provider-backed implementation lives in
//! `puffer-cli` (where the task store and Anthropic key are already in scope).

use puffer_subscriber_runtime::EventEnvelope;

/// Distinct event kind carried by outgoing (self-sent) messages. The manager
/// routes these to the self-report handler and never to the triage router.
pub const SELF_MESSAGE_KIND: &str = "message_self";

/// Whether an envelope is a self-sent (outgoing) message that must bypass triage
/// and be offered to the self-report handler instead. Matches either the
/// dedicated [`SELF_MESSAGE_KIND`] or a truthy `is_outgoing` payload flag, so the
/// guard holds even if a subscriber only sets the payload field.
pub fn is_self_message(envelope: &EventEnvelope) -> bool {
    if envelope.event.kind == SELF_MESSAGE_KIND {
        return true;
    }
    envelope
        .event
        .payload
        .get("is_outgoing")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

/// Handles a self-sent message that may indicate the user completed an open
/// monitor task. Implementations must be cheap when the conversation has no open
/// task (no LLM call) to honour the token-efficiency requirement.
pub trait SelfReportHandler: Send + Sync {
    /// Inspects the outgoing message and completes a matching monitor task if
    /// one is found. Best-effort: failures are swallowed so a self-report never
    /// breaks event processing.
    fn handle(&self, envelope: &EventEnvelope);
}

/// [`SelfReportHandler`] whose behaviour is delegated to a closure. Lets the
/// provider-backed implementation live in `puffer-cli` without dragging the LLM
/// transport or task store into this crate (mirrors [`crate::RemoteClassifier`]).
pub struct RemoteSelfReportHandler {
    callable: Box<dyn Fn(&EventEnvelope) + Send + Sync>,
}

impl RemoteSelfReportHandler {
    /// Wraps a closure as a [`SelfReportHandler`]. The closure is invoked once
    /// per outgoing self-message.
    pub fn new<F>(callable: F) -> Self
    where
        F: Fn(&EventEnvelope) + Send + Sync + 'static,
    {
        Self {
            callable: Box::new(callable),
        }
    }
}

impl SelfReportHandler for RemoteSelfReportHandler {
    fn handle(&self, envelope: &EventEnvelope) {
        (self.callable)(envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_subscriber_runtime::Event;
    use serde_json::json;

    fn envelope(kind: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope {
            envelope_id: "env".into(),
            subscriber_id: "sub".into(),
            received_at_ms: 0,
            event: Event {
                topic: "telegram-user".into(),
                kind: kind.into(),
                control: false,
                dedup_key: None,
                text: "done".into(),
                payload,
            },
        }
    }

    #[test]
    fn self_message_detected_by_kind() {
        assert!(is_self_message(&envelope(SELF_MESSAGE_KIND, json!({}))));
    }

    #[test]
    fn self_message_detected_by_payload_flag() {
        assert!(is_self_message(&envelope(
            "message",
            json!({ "is_outgoing": true })
        )));
    }

    #[test]
    fn incoming_message_is_not_self() {
        assert!(!is_self_message(&envelope(
            "message",
            json!({ "is_outgoing": false })
        )));
        assert!(!is_self_message(&envelope("message", json!({}))));
    }

    #[test]
    fn remote_handler_invokes_closure() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_closure = calls.clone();
        let handler = RemoteSelfReportHandler::new(move |_env| {
            calls_for_closure.fetch_add(1, Ordering::Relaxed);
        });
        handler.handle(&envelope(SELF_MESSAGE_KIND, json!({})));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}
