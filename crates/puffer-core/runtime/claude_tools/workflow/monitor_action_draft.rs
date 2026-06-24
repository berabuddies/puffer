use crate::AppState;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

/// Saves a typed monitor pending action for a daemon-scoped action turn.
pub fn execute_monitor_action_draft(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    super::task_tools::execute_monitor_action_draft(state, cwd, input)
}
