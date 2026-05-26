//! Process-wide handle to the [`SubscriptionManager`].
//!
//! Subscription workflow tools (`SubscriptionCreate`, `SubscriptionList`, …)
//! reach into a manager that is set up once at puffer startup. The manager
//! owns the event bus, the subscription store, the supervised subscriber
//! children, and the router task. Storing it in a `OnceLock` avoids
//! threading another argument through `execute_workflow_tool`.

use anyhow::{anyhow, Result};
use puffer_subscriptions::SubscriptionManager;
use std::sync::{Arc, OnceLock};

static MANAGER: OnceLock<Arc<SubscriptionManager>> = OnceLock::new();

/// Installs the process-wide subscription manager. Called by `puffer-cli`
/// once during startup. Returns an error if a manager is already installed
/// (which would indicate a startup-time bug).
pub fn install(manager: Arc<SubscriptionManager>) -> Result<()> {
    MANAGER
        .set(manager)
        .map_err(|_| anyhow!("subscription manager already installed"))
}

/// Returns the installed subscription manager, or an error explaining
/// that the feature is not available in this build / startup mode.
pub fn manager() -> Result<Arc<SubscriptionManager>> {
    MANAGER
        .get()
        .cloned()
        .ok_or_else(|| anyhow!("subscription runtime is not running in this puffer process"))
}
