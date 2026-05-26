//! `SubscriptionDelete` workflow tool — removes a subscription spec.

use crate::AppState;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

use super::subscription_globals;

#[derive(Debug, Deserialize)]
struct DeleteInput {
    id: String,
}

/// Executes `SubscriptionDelete`. Returns a JSON acknowledgement.
pub fn execute_subscription_delete(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: DeleteInput =
        serde_json::from_value(input).context("invalid SubscriptionDelete input")?;
    let manager = subscription_globals::manager()?;
    manager
        .store()
        .delete(&parsed.id)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    manager.refresh_connection_consumers()?;
    Ok(json!({"deleted": parsed.id}).to_string())
}
