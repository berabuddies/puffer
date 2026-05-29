//! LLM classification step of the subscription pipeline.
//!
//! Today the trait is wired but the only implementation is
//! [`NullClassifier`], which always returns `Pass`. The real
//! provider-backed classifier is a planned follow-up; introducing the trait
//! now keeps the router signature stable so the upgrade is purely additive.

use crate::spec::SubscriptionSpec;
use puffer_subscriber_runtime::Event;

/// Result of a classify call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifyDecision {
    /// Event passes the judge, action should run.
    Pass,
    /// Event is rejected; action should be skipped.
    Reject,
    /// Classifier could not decide (e.g. provider error). The router treats
    /// `Inconclusive` as `Reject` to be safe.
    Inconclusive,
}

/// Classifier trait. The router calls [`Classifier::classify`] only when
/// the spec's `classify_prompt` is set.
pub trait Classifier: Send + Sync {
    /// Returns the classify decision for the given event under the
    /// supplied prompt.
    fn classify(&self, spec: &SubscriptionSpec, event: &Event) -> ClassifyDecision;
}

/// No-op classifier — every event passes. Used when no provider-backed
/// classifier has been configured yet.
pub struct NullClassifier;

impl Classifier for NullClassifier {
    fn classify(&self, _spec: &SubscriptionSpec, _event: &Event) -> ClassifyDecision {
        ClassifyDecision::Pass
    }
}

/// Classifier whose decision is delegated to a closure. Lets the
/// provider-backed implementation live in `puffer-cli` (where the
/// `AuthStore` and HTTP client are already in scope) without dragging
/// the LLM transport stack into this crate.
///
/// The closure receives the spec's `classify_prompt`, an optional
/// `classify_model` hint (`<provider>/<model>`), and the event text.
/// Implementations should return `Pass` / `Reject` from a yes/no
/// answer, and `Inconclusive` on transient errors so the router can
/// skip the action without hard-failing.
pub struct RemoteClassifier {
    callable: Box<dyn Fn(&str, Option<&str>, &str) -> ClassifyDecision + Send + Sync>,
}

impl RemoteClassifier {
    /// Wraps a closure as a [`Classifier`]. The closure is invoked once
    /// per event whose subscription has a `classify_prompt`.
    pub fn new<F>(callable: F) -> Self
    where
        F: Fn(&str, Option<&str>, &str) -> ClassifyDecision + Send + Sync + 'static,
    {
        Self {
            callable: Box::new(callable),
        }
    }
}

impl Classifier for RemoteClassifier {
    fn classify(&self, spec: &SubscriptionSpec, event: &Event) -> ClassifyDecision {
        let Some(prompt) = spec.classify_prompt.as_deref() else {
            return ClassifyDecision::Pass;
        };
        (self.callable)(prompt, spec.classify_model.as_deref(), &event.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ActionSpec, SubscriptionStatus};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn spec_with_prompt(prompt: &str) -> SubscriptionSpec {
        SubscriptionSpec {
            slug: "x".into(),
            description: String::new(),
            connection_slug: "telegram-user".into(),
            connector_slug: None,
            status: SubscriptionStatus::Enabled,
            filter: None,
            ignore_filters: Vec::new(),
            classify_prompt: Some(prompt.into()),
            classify_model: Some("anthropic/claude-haiku-4-5".into()),
            action: ActionSpec::SqliteInsert {
                path: "/tmp/x.db".into(),
                table: "t".into(),
            },
            created_at_ms: 0,
        }
    }

    fn event(text: &str) -> Event {
        Event {
            topic: "telegram-user".into(),
            kind: "message".into(),
            control: false,
            dedup_key: None,
            text: text.into(),
            payload: serde_json::Value::Null,
        }
    }

    #[test]
    fn remote_classifier_calls_closure_with_prompt_and_text() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_closure = calls.clone();
        let classifier = RemoteClassifier::new(move |prompt, model, text| {
            calls_for_closure.fetch_add(1, Ordering::Relaxed);
            assert_eq!(prompt, "Is this about IoC?");
            assert_eq!(model, Some("anthropic/claude-haiku-4-5"));
            if text.contains("IoC") {
                ClassifyDecision::Pass
            } else {
                ClassifyDecision::Reject
            }
        });
        let spec = spec_with_prompt("Is this about IoC?");
        assert_eq!(
            classifier.classify(&spec, &event("we found an IoC today")),
            ClassifyDecision::Pass
        );
        assert_eq!(
            classifier.classify(&spec, &event("just saying hi")),
            ClassifyDecision::Reject
        );
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn remote_classifier_passes_when_no_prompt_set() {
        let mut spec = spec_with_prompt("ignored");
        spec.classify_prompt = None;
        let classifier = RemoteClassifier::new(|_, _, _| {
            panic!("closure should not run when classify_prompt is None")
        });
        assert_eq!(
            classifier.classify(&spec, &event("x")),
            ClassifyDecision::Pass
        );
    }
}
