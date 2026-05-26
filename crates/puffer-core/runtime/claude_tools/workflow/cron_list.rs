use crate::AppState;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

use super::store::{crons_path, load_store, CronStore};

/// Executes the Claude-compatible `CronList` workflow tool.
pub fn execute_cron_list(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let _ = cwd;
    let _ = input;
    let store = load_store::<CronStore>(&crons_path(state.session.cwd.as_path()))?;
    Ok(serde_json::to_string_pretty(&store.jobs)?)
}
