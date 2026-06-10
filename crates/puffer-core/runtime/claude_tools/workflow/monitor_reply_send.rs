use crate::AppState;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

/// Executes the monitor-specific reply send and completion tool.
pub fn execute_monitor_reply_send(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    super::task_tools::execute_monitor_reply_send(state, cwd, input)
}
