use super::claude_tools::{
    bash, edit, glob, grep, notebook_edit, read, web_fetch, write, BashOutputStream,
};
use anyhow::{anyhow, bail, Context, Result};
use puffer_remote_tools::{
    RemoteToolCapabilities, RemoteToolChunk, RemoteToolChunkStream, RemoteToolExecutionContext,
    RemoteToolRequest, RemoteWebSearchRequest,
};
use puffer_tools::{ToolExecutionResult, ToolOutput};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

const REMOTE_TOOL_IDS: &[&str] = &[
    "Bash",
    "Read",
    "Write",
    "Edit",
    "Glob",
    "Grep",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "Sleep",
];

/// Describes the capabilities surfaced by the remote tool-runner service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteToolRunnerCapabilities {
    pub supported_tools: Vec<String>,
    pub streams_stdout_stderr: bool,
}

/// Returns the tool ids currently supported by the remote tool-runner.
pub fn remote_tool_runner_supported_tools() -> &'static [&'static str] {
    REMOTE_TOOL_IDS
}

/// Returns the capabilities surfaced by the remote tool-runner service.
pub fn remote_tool_runner_capabilities() -> RemoteToolRunnerCapabilities {
    RemoteToolRunnerCapabilities {
        supported_tools: REMOTE_TOOL_IDS
            .iter()
            .map(|tool| (*tool).to_string())
            .collect(),
        streams_stdout_stderr: true,
    }
}

/// Converts the local capability view into the shared DTO exposed over gRPC.
pub fn remote_tool_capabilities_dto() -> RemoteToolCapabilities {
    let capabilities = remote_tool_runner_capabilities();
    RemoteToolCapabilities {
        version: env!("CARGO_PKG_VERSION").to_string(),
        supported_tools: capabilities.supported_tools,
        streams_stdout_stderr: capabilities.streams_stdout_stderr,
    }
}

/// Executes one tool request using the same Claude-style tool implementations as the local runtime.
pub fn execute_remote_tool<F>(
    request: &RemoteToolRequest,
    mut on_chunk: F,
) -> Result<ToolExecutionResult>
where
    F: FnMut(RemoteToolChunk),
{
    let input: Value = serde_json::from_str(&request.input_json)
        .with_context(|| format!("invalid tool input JSON for {}", request.tool_id))?;
    let cwd = PathBuf::from(&request.cwd);
    let working_dirs = if request.working_dirs.is_empty() {
        vec![cwd.clone()]
    } else {
        request
            .working_dirs
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    };
    let allow_all_paths = request.sandbox_mode.trim() == "danger-full-access";
    match request.tool_id.as_str() {
        "Bash" => execute_remote_bash(&cwd, &request.session_id, input, &mut on_chunk),
        "Read" => execute_remote_stdout_tool(
            &request.tool_id,
            read::execute_claude_read_tool(&cwd, &working_dirs, allow_all_paths, input)?,
        ),
        "Write" => execute_remote_stdout_tool(
            &request.tool_id,
            write::execute_claude_write_tool_unchecked(
                &cwd,
                &working_dirs,
                allow_all_paths,
                input,
            )?,
        ),
        "Edit" => execute_remote_stdout_tool(
            &request.tool_id,
            edit::execute_claude_edit(&cwd, &working_dirs, allow_all_paths, input)?,
        ),
        "Glob" => execute_remote_stdout_tool(
            &request.tool_id,
            glob::execute_claude_glob(&cwd, &working_dirs, allow_all_paths, input)?,
        ),
        "Grep" => execute_remote_stdout_tool(
            &request.tool_id,
            grep::execute_claude_grep(&cwd, &working_dirs, allow_all_paths, input)?,
        ),
        "NotebookEdit" => execute_remote_stdout_tool(
            &request.tool_id,
            notebook_edit::execute_notebook_edit_tool(&cwd, &working_dirs, allow_all_paths, input)?,
        ),
        "WebFetch" => execute_remote_stdout_tool(
            &request.tool_id,
            serde_json::to_string_pretty(&web_fetch::execute_claude_web_fetch(input)?)
                .context("failed to serialize WebFetch output")?,
        ),
        "WebSearch" => execute_remote_stdout_tool(
            &request.tool_id,
            super::claude_tools::web_search::execute_remote_web_search(
                &remote_web_search_request(request)?,
            )?,
        ),
        "Sleep" => execute_remote_stdout_tool(&request.tool_id, execute_remote_sleep(input)?),
        other => bail!("tool `{other}` is not supported by the remote runner"),
    }
}

fn execute_remote_stdout_tool(tool_id: &str, stdout: String) -> Result<ToolExecutionResult> {
    Ok(ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: true,
        output: ToolOutput {
            stdout,
            stderr: String::new(),
            metadata: Value::Null,
        },
    })
}

fn execute_remote_bash<F>(
    cwd: &Path,
    session_id: &str,
    input: Value,
    on_chunk: &mut F,
) -> Result<ToolExecutionResult>
where
    F: FnMut(RemoteToolChunk),
{
    let session_id = Uuid::parse_str(session_id).unwrap_or_else(|_| Uuid::nil());
    let execution = bash::execute_streaming_from_value(cwd, &session_id, input, |stream, text| {
        on_chunk(RemoteToolChunk {
            stream: match stream {
                BashOutputStream::Stdout => RemoteToolChunkStream::Stdout,
                BashOutputStream::Stderr => RemoteToolChunkStream::Stderr,
            },
            text,
        });
    })?;
    let stdout = serde_json::to_string_pretty(&execution.output)
        .context("failed to serialize Bash output")?;
    Ok(ToolExecutionResult {
        tool_id: "Bash".to_string(),
        success: execution.success,
        output: ToolOutput {
            stdout,
            stderr: String::new(),
            metadata: Value::Null,
        },
    })
}

fn execute_remote_sleep(input: Value) -> Result<String> {
    let duration_ms = input
        .get("duration_ms")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("Sleep duration_ms must be an integer"))?;
    if duration_ms == 0 {
        bail!("Sleep duration_ms must be greater than zero");
    }
    let capped = duration_ms.min(300_000);
    std::thread::sleep(Duration::from_millis(capped));
    Ok(serde_json::to_string_pretty(&json!({
        "duration_ms": capped,
        "completed": true,
        "reason": input.get("reason").cloned().unwrap_or(Value::Null),
    }))?)
}

fn remote_web_search_request(request: &RemoteToolRequest) -> Result<RemoteWebSearchRequest> {
    let raw = request
        .execution_context_json
        .as_deref()
        .ok_or_else(|| anyhow!("WebSearch remote execution context is missing"))?;
    let context: RemoteToolExecutionContext =
        serde_json::from_str(raw).context("invalid WebSearch remote execution context")?;
    match context {
        RemoteToolExecutionContext::WebSearch(request) => Ok(request),
    }
}
