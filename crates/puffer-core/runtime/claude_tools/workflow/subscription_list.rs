//! `SubscriptionList` workflow tool — returns every installed subscription
//! plus the router's running counters.

use crate::AppState;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

use super::subscription_globals;

/// Executes `SubscriptionList`. Returns a JSON object with `subscriptions`
/// (the full spec list) and `running_subscribers` (subscriber ids
/// currently supervised in this process).
pub fn execute_subscription_list(
    _state: &mut AppState,
    _cwd: &Path,
    _input: Value,
) -> Result<String> {
    let manager = subscription_globals::manager()?;
    let subscriptions = manager.store().list();
    let running = manager.subscriber_ids();
    let body = json!({
        "subscriptions": subscriptions,
        "running_subscribers": running,
    });
    Ok(serde_json::to_string_pretty(&body)?)
}
