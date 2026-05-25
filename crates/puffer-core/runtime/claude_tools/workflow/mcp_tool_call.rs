use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_runner_api::{ChunkSink, McpResult, NullChunkSink, RunnerError, ToolRunner};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpToolCallInput {
    server: String,
    tool: String,
    #[serde(default)]
    args: Option<Value>,
}

/// Executes one declared MCP tool call for verified Lambda skill contracts.
pub fn execute_mcp_tool_call(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: McpToolCallInput =
        serde_json::from_value(input).context("invalid McpToolCall input")?;
    let server = required(&parsed.server, "server")?;
    let tool = required(&parsed.tool, "tool")?;
    let args = parsed.args.unwrap_or_else(|| json!({}));
    let mut sink = NullChunkSink;
    let result = call_mcp_tool(state.tool_runner.as_ref(), server, tool, args, &mut sink)?;
    format_mcp_result(result)
}

fn required<'a>(value: &'a str, field: &str) -> Result<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("McpToolCall `{field}` is required");
    }
    Ok(trimmed)
}

fn call_mcp_tool(
    runner: &dyn ToolRunner,
    server: &str,
    tool: &str,
    args: Value,
    sink: &mut dyn ChunkSink,
) -> Result<McpResult> {
    runner
        .call_mcp_tool(server, tool, args, sink)
        .map_err(map_runner_error)
}

fn map_runner_error(error: RunnerError) -> anyhow::Error {
    match error {
        RunnerError::NotFound(message) => anyhow!("MCP tool call target not found: {message}"),
        RunnerError::PermissionDenied(message) => anyhow!("MCP tool call denied: {message}"),
        RunnerError::Unsupported(message) => anyhow!("unsupported MCP tool call: {message}"),
        RunnerError::InvalidArgument(message) => anyhow!("invalid MCP tool call: {message}"),
        RunnerError::Transport(message) => anyhow!("MCP tool call transport error: {message}"),
        RunnerError::Mcp(message) => anyhow!("MCP tool call failed: {message}"),
        RunnerError::OAuthRequired {
            server_id,
            authorization_url,
        } => {
            let suffix = authorization_url
                .map(|url| format!("; authorize at {url}"))
                .unwrap_or_default();
            anyhow!("MCP tool call requires OAuth for server `{server_id}`{suffix}")
        }
        RunnerError::Execution(message) => anyhow!("MCP tool call failed: {message}"),
        RunnerError::Other(message) => anyhow!("MCP tool call error: {message}"),
    }
}

fn format_mcp_result(result: McpResult) -> Result<String> {
    if !result.success {
        let mut message = format!(
            "MCP tool {}/{} reported failure",
            result.server, result.tool
        );
        if !result.stderr.trim().is_empty() {
            message.push_str(&format!(": {}", result.stderr));
        } else if !result.stdout.trim().is_empty() {
            message.push_str(&format!(": {}", result.stdout));
        }
        bail!(message);
    }
    if !result.metadata.is_null() {
        Ok(serde_json::to_string_pretty(&result.metadata)?)
    } else if !result.stdout.is_empty() {
        Ok(result.stdout)
    } else {
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_metadata_result() {
        let output = format_mcp_result(McpResult {
            server: "agentmail".to_string(),
            tool: "list_inboxes".to_string(),
            success: true,
            stdout: String::new(),
            stderr: String::new(),
            metadata: json!({"inboxes": [{"id": "in_1"}]}),
        })
        .unwrap();

        assert!(output.contains("\"inboxes\""));
    }

    #[test]
    fn reports_mcp_failure() {
        let error = format_mcp_result(McpResult {
            server: "agentmail".to_string(),
            tool: "get_thread".to_string(),
            success: false,
            stdout: String::new(),
            stderr: "thread not found".to_string(),
            metadata: Value::Null,
        })
        .expect_err("MCP failure must fail closed");

        assert!(format!("{error:#}").contains("thread not found"));
    }
}
