use crate::AppState;
use anyhow::Result;
use puffer_runner_api::{McpServerInfo, McpTool, RunnerError};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpStatusInput {
    #[serde(default = "default_include_tools")]
    include_tools: bool,
}

/// Returns the MCP servers and tool counts visible to Puffer's runner.
pub fn execute_mcp_status(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: McpStatusInput = serde_json::from_value(input)?;
    let servers = match state.tool_runner.list_mcp_servers() {
        Ok(servers) => servers,
        Err(error) => {
            return Ok(serde_json::to_string_pretty(&json!({
                "ok": false,
                "servers": [],
                "errors": [format_runner_error(error)]
            }))?);
        }
    };

    let mut tool_lists = Vec::new();
    let mut errors = Vec::new();
    for server in &servers {
        match state.tool_runner.list_mcp_tools(&server.id) {
            Ok(tools) => tool_lists.push((server.id.clone(), tools)),
            Err(error) => errors.push(json!({
                "server": server.id,
                "error": format_runner_error(error)
            })),
        }
    }

    Ok(serde_json::to_string_pretty(&format_status(
        servers,
        tool_lists,
        errors,
        parsed.include_tools,
    ))?)
}

fn default_include_tools() -> bool {
    true
}

fn format_status(
    servers: Vec<McpServerInfo>,
    tool_lists: Vec<(String, Vec<McpTool>)>,
    errors: Vec<Value>,
    include_tools: bool,
) -> Value {
    let server_entries = servers
        .into_iter()
        .map(|server| {
            let tools = tool_lists
                .iter()
                .find(|(id, _)| id == &server.id)
                .map(|(_, tools)| tools.as_slice())
                .unwrap_or(&[]);
            let mut entry = json!({
                "id": server.id,
                "displayName": server.display_name,
                "transport": server.transport,
                "target": server.target,
                "description": server.description,
                "toolCount": tools.len(),
            });
            if include_tools {
                entry["tools"] = json!(tools
                    .iter()
                    .map(|tool| json!({
                        "name": tool.name,
                        "description": tool.description,
                    }))
                    .collect::<Vec<_>>());
            }
            entry
        })
        .collect::<Vec<_>>();
    json!({
        "ok": errors.is_empty(),
        "serverCount": server_entries.len(),
        "servers": server_entries,
        "errors": errors,
    })
}

fn format_runner_error(error: RunnerError) -> String {
    match error {
        RunnerError::NotFound(message) => format!("not found: {message}"),
        RunnerError::PermissionDenied(message) => format!("permission denied: {message}"),
        RunnerError::Unsupported(message) => format!("unsupported: {message}"),
        RunnerError::InvalidArgument(message) => format!("invalid argument: {message}"),
        RunnerError::Mcp(message) => format!("mcp error: {message}"),
        RunnerError::Transport(message) => format!("transport error: {message}"),
        RunnerError::OAuthRequired { server_id, .. } => {
            format!("oauth required for server `{server_id}`")
        }
        RunnerError::Execution(message) => format!("execution error: {message}"),
        RunnerError::Other(message) => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_server_tool_counts() {
        let status = format_status(
            vec![McpServerInfo {
                id: "filesystem".to_string(),
                display_name: "Filesystem".to_string(),
                transport: "stdio".to_string(),
                target: "mcp-filesystem".to_string(),
                description: "local files".to_string(),
            }],
            vec![(
                "filesystem".to_string(),
                vec![McpTool {
                    name: "read_file".to_string(),
                    description: Some("Read a file".to_string()),
                    input_schema: None,
                }],
            )],
            Vec::new(),
            true,
        );

        assert_eq!(status["ok"], json!(true));
        assert_eq!(status["serverCount"], json!(1));
        assert_eq!(status["servers"][0]["toolCount"], json!(1));
        assert_eq!(status["servers"][0]["tools"][0]["name"], json!("read_file"));
    }
}
