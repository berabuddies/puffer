//! `CanvasState` workflow tool for reading user interaction state from Canvas.

use crate::AppState;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

/// Executes the `CanvasState` workflow tool.
pub fn execute_canvas_state(state: &AppState, cwd: &Path, input: Value) -> Result<String> {
    super::canvas::execute_canvas_state(state, cwd, input)
}
