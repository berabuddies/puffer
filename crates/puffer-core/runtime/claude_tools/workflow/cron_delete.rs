use crate::AppState;
use anyhow::{Context, Result};
use serde_json::json;
use serde_json::Value;
use std::path::Path;

use super::store::{crons_path, load_store, save_store, CronDeleteInput, CronStore};

/// Executes the Claude-compatible `CronDelete` workflow tool.
pub fn execute_cron_delete(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let _ = cwd;
    let parsed: CronDeleteInput =
        serde_json::from_value(input).context("invalid CronDelete input")?;
    let mut store = load_store::<CronStore>(&crons_path(state.session.cwd.as_path()))?;
    let before = store.jobs.len();
    store.jobs.retain(|job| job.id != parsed.id);
    save_store(&crons_path(state.session.cwd.as_path()), &store)?;
    Ok(serde_json::to_string_pretty(&json!({
        "deleted": before != store.jobs.len(),
        "id": parsed.id
    }))?)
}
