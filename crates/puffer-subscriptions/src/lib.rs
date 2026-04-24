//! Routes events from the subscriber bus through user-defined subscriptions
//! (regex prefilter → optional LLM classify → one action) and persists
//! subscription specs to disk.
//!
//! The unit of configuration is [`SubscriptionSpec`]. The agent creates and
//! manages specs via workflow tools (`SubscriptionCreate`, `SubscriptionList`,
//! `SubscriptionPause`, `SubscriptionDelete`); the runtime never asks users to
//! hand-author the file.
//!
//! Specs live as a JSON document at `~/.puffer/subscriptions.json`. The
//! [`SubscriptionStore`] guards reads/writes with a mutex; mutations are
//! atomic via temp-file rename.
//!
//! The router consumes events from a [`puffer_subscriber_runtime::EventBus`]
//! and dispatches matched events to an [`ActionDispatcher`]. The default
//! dispatcher implements the two MVP actions: `sqlite_insert` and
//! `forward_message`. `forward_message` is a placeholder until a connector
//! egress trait is exposed; it logs the intended forward and returns.

mod action;
mod classify;
mod manager;
mod router;
mod spec;
mod store;

pub use action::{
    install_outbound, ActionDispatcher, ActionResult, BuiltinActionDispatcher, Outbound,
};
pub use classify::{ClassifyDecision, Classifier, NullClassifier, RemoteClassifier};
pub use manager::{SubscriptionManager, SubscriptionManagerBuilder};
pub use router::{RouterStats, SubscriptionRouter};
pub use spec::{ActionSpec, PrefilterSpec, SubscriptionSpec, SubscriptionStatus};
pub use store::{SubscriptionStore, SubscriptionStoreError};
