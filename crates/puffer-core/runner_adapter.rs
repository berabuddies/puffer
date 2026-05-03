//! Pure dispatch from a [`puffer_runner_api::ToolRequest`] to the existing
//! claude-parity tool implementations living in `runtime::claude_tools`.
//!
//! This module is the seam used by `puffer-runner-local::LocalToolRunner`.
//! It contains no per-session mutable state — read-state staleness is the
//! dispatcher's job (see [`puffer_runner_api::check_read_freshness`]). Each
//! tool result carries the freshly-observed `mtime` for any path that the
//! caller's read-state map should track.

use anyhow::{anyhow, bail, Context, Result};
use puffer_runner_api::{ChunkSink, ReadStateUpdate, ToolRequest, ToolResult};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

use crate::runtime::claude_tools::{bash, edit, glob, grep, notebook_edit, read, web_fetch};

/// Tools the local runner is willing to execute through the
/// [`puffer_runner_api::ToolRunner::execute_tool`] surface.
///
/// Network search (`WebSearch`) currently requires a provider context that
/// hasn't been hoisted onto the trait yet, so it stays on the legacy path.
pub fn supported_runner_tools() -> &'static [&'static str] {
    &[
        "Bash",
        "Read",
        "Write",
        "Edit",
        "Glob",
        "Grep",
        "NotebookEdit",
        "WebFetch",
        "Sleep",
    ]
}

/// Returns true when [`execute_runner_tool`] knows how to handle `tool_id`.
pub fn is_runner_supported(tool_id: &str) -> bool {
    supported_runner_tools().contains(&tool_id)
}

/// Executes one tool request through the runner-shaped dispatcher.
///
/// This is the pure body of `LocalToolRunner::execute_tool`. The caller is
/// responsible for any pre-flight staleness checks; this function only
/// reports back which paths were touched so the caller can update its
/// read-state map.
pub fn execute_runner_tool(req: &ToolRequest, _sink: &mut dyn ChunkSink) -> Result<ToolResult> {
    let cwd = req.cwd.as_path();
    let working_dirs = req.working_dirs.as_slice();
    let allow_all_paths = req.allow_all_paths;
    let input = req.input.clone();

    match req.tool_id.as_str() {
        "Bash" => {
            let session_id = parse_session_id(req.session_id.as_deref())?;
            let execution = bash::execute_from_value(cwd, &session_id, input)?;
            let stdout = serde_json::to_string_pretty(&execution.output)
                .context("failed to serialize Bash output")?;
            Ok(plain_result(req.tool_id.as_str(), execution.success, stdout))
        }
        "Read" => {
            let stdout = read::execute_claude_read_tool(cwd, working_dirs, allow_all_paths, input.clone())?;
            let updates = read_update_from_input(&input)?;
            Ok(result_with_updates(req.tool_id.as_str(), true, stdout, updates))
        }
        "Write" => {
            let path = input_file_path(&input, "file_path")?;
            let stdout = run_claude_write(cwd, working_dirs, allow_all_paths, input.clone())?;
            let updates = match path.as_deref() {
                Some(path) => vec![ReadStateUpdate {
                    path: path.to_path_buf(),
                    timestamp_ms: file_timestamp_ms(path)?,
                    is_partial_view: false,
                }],
                None => Vec::new(),
            };
            Ok(result_with_updates(req.tool_id.as_str(), true, stdout, updates))
        }
        "Edit" => {
            let path = input_file_path(&input, "file_path")?;
            let stdout = edit::execute_claude_edit(cwd, working_dirs, allow_all_paths, input.clone())?;
            let updates = match path.as_deref() {
                Some(path) => vec![ReadStateUpdate {
                    path: path.to_path_buf(),
                    timestamp_ms: file_timestamp_ms(path)?,
                    is_partial_view: false,
                }],
                None => Vec::new(),
            };
            Ok(result_with_updates(req.tool_id.as_str(), true, stdout, updates))
        }
        "Glob" => {
            let stdout = glob::execute_claude_glob(cwd, working_dirs, allow_all_paths, input)?;
            Ok(plain_result(req.tool_id.as_str(), true, stdout))
        }
        "Grep" => {
            let stdout = grep::execute_claude_grep(cwd, working_dirs, allow_all_paths, input)?;
            Ok(plain_result(req.tool_id.as_str(), true, stdout))
        }
        "NotebookEdit" => {
            let path = input_file_path(&input, "notebook_path")?;
            let stdout =
                notebook_edit::execute_notebook_edit_tool(cwd, working_dirs, allow_all_paths, input.clone())?;
            let updates = match path.as_deref() {
                Some(path) if path.exists() => vec![ReadStateUpdate {
                    path: path.to_path_buf(),
                    timestamp_ms: file_timestamp_ms(path)?,
                    is_partial_view: false,
                }],
                _ => Vec::new(),
            };
            Ok(result_with_updates(req.tool_id.as_str(), true, stdout, updates))
        }
        "WebFetch" => {
            let stdout = serde_json::to_string_pretty(&web_fetch::execute_claude_web_fetch(input)?)
                .context("failed to serialize WebFetch output")?;
            Ok(plain_result(req.tool_id.as_str(), true, stdout))
        }
        "Sleep" => {
            let stdout = sleep_dispatch(input)?;
            Ok(plain_result(req.tool_id.as_str(), true, stdout))
        }
        other => bail!("tool `{other}` is not supported by the local runner"),
    }
}

/// Computes the `ReadStateUpdate` that the runtime should record after a
/// successful `Read` call. Mirrors the partial-view bookkeeping that used to
/// live inside `record_read_from_input`.
fn read_update_from_input(input: &Value) -> Result<Vec<ReadStateUpdate>> {
    let Some(path) = input_file_path(input, "file_path")? else {
        return Ok(Vec::new());
    };
    let timestamp_ms = file_timestamp_ms(&path)?;
    let offset = input.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let limit = input.get("limit").and_then(Value::as_u64);
    let line_count = std::fs::read_to_string(&path)
        .map(|content| content.lines().count() as u64)
        .unwrap_or(u64::MAX);
    let has_partial_offset = offset > 1;
    let has_restrictive_limit = limit.is_some_and(|l| {
        let effective_remaining = line_count.saturating_sub(offset);
        l < effective_remaining
    });
    let is_partial_view =
        has_partial_offset || has_restrictive_limit || pages_field_is_present(input);
    Ok(vec![ReadStateUpdate {
        path,
        timestamp_ms,
        is_partial_view,
    }])
}

fn pages_field_is_present(input: &Value) -> bool {
    match input.get("pages") {
        None | Some(Value::Null) => false,
        Some(Value::String(value)) => !value.trim().is_empty(),
        Some(_) => true,
    }
}

fn input_file_path(input: &Value, field: &str) -> Result<Option<std::path::PathBuf>> {
    Ok(input
        .get(field)
        .and_then(Value::as_str)
        .map(std::path::PathBuf::from))
}

fn file_timestamp_ms(path: &Path) -> Result<u128> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat file {}", path.display()))?;
    let modified = metadata
        .modified()
        .with_context(|| format!("failed to read mtime for {}", path.display()))?;
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .with_context(|| format!("mtime for {} predates UNIX_EPOCH", path.display()))?;
    Ok(duration.as_millis())
}

fn parse_session_id(raw: Option<&str>) -> Result<Uuid> {
    match raw {
        None => Ok(Uuid::nil()),
        Some(value) => Uuid::parse_str(value)
            .map_err(|e| anyhow!("invalid session_id `{value}`: {e}")),
    }
}

fn run_claude_write(
    cwd: &Path,
    working_dirs: &[std::path::PathBuf],
    allow_all_paths: bool,
    input: Value,
) -> Result<String> {
    use crate::runtime::claude_tools::write::{execute_claude_write_tool, ClaudeReadSnapshot};
    use std::collections::HashMap;
    let mut bypass: HashMap<std::path::PathBuf, ClaudeReadSnapshot> = HashMap::new();
    if let Some(path) = input_file_path(&input, "file_path")? {
        if path.exists() {
            bypass.insert(
                path.clone(),
                ClaudeReadSnapshot {
                    timestamp_ms: file_timestamp_ms(&path)?,
                    is_partial_view: false,
                },
            );
        }
    }
    execute_claude_write_tool(cwd, working_dirs, allow_all_paths, input, &mut bypass)
}

fn sleep_dispatch(input: Value) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct SleepInput {
        duration_ms: u64,
        #[serde(default)]
        reason: Option<String>,
    }
    const MAX_DURATION_MS: u64 = 300_000;
    let parsed: SleepInput = serde_json::from_value(input)?;
    if parsed.duration_ms == 0 {
        bail!("Sleep duration_ms must be greater than zero");
    }
    let duration_ms = parsed.duration_ms.min(MAX_DURATION_MS);
    std::thread::sleep(std::time::Duration::from_millis(duration_ms));
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "duration_ms": duration_ms,
        "completed": true,
        "reason": parsed.reason,
    }))?)
}

fn plain_result(tool_id: &str, success: bool, stdout: String) -> ToolResult {
    ToolResult {
        tool_id: tool_id.to_string(),
        success,
        stdout,
        stderr: String::new(),
        metadata: Value::Null,
        read_state_updates: Vec::new(),
    }
}

fn result_with_updates(
    tool_id: &str,
    success: bool,
    stdout: String,
    updates: Vec<ReadStateUpdate>,
) -> ToolResult {
    ToolResult {
        tool_id: tool_id.to_string(),
        success,
        stdout,
        stderr: String::new(),
        metadata: Value::Null,
        read_state_updates: updates,
    }
}
