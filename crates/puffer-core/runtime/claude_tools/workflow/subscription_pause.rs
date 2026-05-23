//! `SubscriptionPause` workflow tool — toggles a subscription's status
//! between `enabled` and `paused`.

use crate::AppState;
use anyhow::{Context, Result};
use puffer_subscriptions::SubscriptionStatus;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

use super::subscription_globals;

#[derive(Debug, Deserialize)]
struct PauseInput {
    id: String,
    /// Defaults to `paused`. Set `false` to re-enable a paused subscription.
    #[serde(default = "default_paused")]
    paused: bool,
}

fn default_paused() -> bool {
    true
}

/// Executes `SubscriptionPause`. Returns the updated spec.
pub fn execute_subscription_pause(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: PauseInput =
        serde_json::from_value(input).context("invalid SubscriptionPause input")?;
    let manager = subscription_globals::manager()?;
    let status = if parsed.paused {
        SubscriptionStatus::Paused
    } else {
        SubscriptionStatus::Enabled
    };
    let updated = manager
        .store()
        .set_status(&parsed.id, status)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    manager.refresh_connection_consumers()?;
    Ok(serde_json::to_string_pretty(&updated)?)
}
