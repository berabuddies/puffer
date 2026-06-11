use crate::AppState;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

/// Saves a draft reply for a daemon-scoped monitor action turn.
pub fn execute_monitor_reply_draft(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    super::task_tools::execute_monitor_reply_draft(state, cwd, input)
}
