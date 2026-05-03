//! LSP RPCs for the desktop Files pane.

use anyhow::{Context, Result};
use puffer_resources::load_resources;
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::daemon::DaemonState;

const LSP_OPERATIONS: &[&str] = &[
    "hover",
    "goToDefinition",
    "findReferences",
    "incomingCalls",
    "outgoingCalls",
];

/// Run the configured LSP server for a clicked file position.
pub(crate) fn handle_lsp_inspect(state: &DaemonState, params: &Value) -> Result<Value> {
    let raw = params
        .get("path")
        .and_then(Value::as_str)
        .context("missing path")?;
    let file_path = crate::daemon_files::validate_path(state, raw)?;
    let cwd = params
        .get("cwd")
        .and_then(Value::as_str)
        .and_then(|raw| canonical_allowed_dir(state, raw).ok())
        .unwrap_or_else(|| state.cwd_path().to_path_buf());
    let line = params.get("line").and_then(Value::as_u64).unwrap_or(0) as usize;
    let character = params.get("character").and_then(Value::as_u64).unwrap_or(0) as usize;

    let resources = load_resources(
        state.config_paths(),
        &puffer_runner_local::LocalToolRunner::new(),
    )?;
    let mut operations = serde_json::Map::new();
    for operation in LSP_OPERATIONS {
        let output = puffer_core::execute_lsp_query(
            &resources,
            &cwd,
            json!({
                "operation": operation,
                "filePath": file_path.display().to_string(),
                "line": line + 1,
                "character": character + 1,
            }),
        )?;
        let parsed = serde_json::from_str::<Value>(&output).unwrap_or_else(|_| {
            json!({
                "operation": operation,
                "filePath": file_path.display().to_string(),
                "result": output,
            })
        });
        let stop_after_error = parsed
            .get("result")
            .and_then(Value::as_str)
            .is_some_and(|result| {
                result.starts_with("Error performing ")
                    || result.starts_with("No LSP server available")
                    || result.starts_with("No LSP server installed")
            });
        operations.insert((*operation).to_string(), parsed);
        if stop_after_error {
            break;
        }
    }

    Ok(json!({
        "path": file_path.display().to_string(),
        "cwd": cwd.display().to_string(),
        "line": line,
        "character": character,
        "operations": operations,
    }))
}

fn canonical_allowed_dir(state: &DaemonState, raw: &str) -> Result<PathBuf> {
    let path = crate::daemon_files::validate_path(state, raw)?;
    if path.is_dir() {
        Ok(path)
    } else {
        Ok(path
            .parent()
            .unwrap_or_else(|| state.cwd_path())
            .to_path_buf())
    }
}
