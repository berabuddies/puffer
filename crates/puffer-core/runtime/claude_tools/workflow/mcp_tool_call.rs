use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_runner_api::{ChunkSink, McpResult, NullChunkSink, RunnerError, ToolRunner};
use puffer_tools::mcp_qualify::{qualify_tools, McpToolKey};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpToolCallInput {
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    args: Option<Value>,
    #[serde(default, rename = "qualifiedToolName", alias = "qualified_tool_name")]
    qualified_tool_name: Option<String>,
    #[serde(default, rename = "argsJson", alias = "args_json")]
    args_json: Option<String>,
    #[serde(default)]
    registry: Option<Value>,
}

#[derive(Debug)]
struct ResolvedMcpCall {
    server: String,
    tool: String,
    args: Value,
}

/// Executes one declared MCP tool call for verified Lambda skill contracts.
pub fn execute_mcp_tool_call(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: McpToolCallInput =
        serde_json::from_value(input).context("invalid McpToolCall input")?;
    let resolved = resolve_mcp_call(state.tool_runner.as_ref(), parsed)?;
    let mut sink = NullChunkSink;
    let result = call_mcp_tool(
        state.tool_runner.as_ref(),
        &resolved.server,
        &resolved.tool,
        resolved.args,
        &mut sink,
    )?;
    format_mcp_result(result)
}

fn resolve_mcp_call(runner: &dyn ToolRunner, input: McpToolCallInput) -> Result<ResolvedMcpCall> {
    if let Some(qualified) = optional_string(input.qualified_tool_name) {
        if input.server.is_some() || input.tool.is_some() || input.args.is_some() {
            bail!("McpToolCall must use either server/tool/args or qualifiedToolName/argsJson");
        }
        if input.registry.is_none() {
            bail!("McpToolCall qualifiedToolName requires the registry proof argument");
        }
        let args_json = required_optional_string(input.args_json, "argsJson")?;
        let args = serde_json::from_str::<Value>(&args_json)
            .context("McpToolCall argsJson must be valid JSON")?;
        let (server, tool) = resolve_qualified_mcp_tool(runner, &qualified)?;
        return Ok(ResolvedMcpCall { server, tool, args });
    }

    if input.args_json.is_some() || input.registry.is_some() {
        bail!("McpToolCall argsJson/registry are only valid with qualifiedToolName");
    }
    let server = required_optional_string(input.server, "server")?;
    let tool = required_optional_string(input.tool, "tool")?;
    let args = input.args.unwrap_or_else(|| json!({}));
    Ok(ResolvedMcpCall { server, tool, args })
}

fn required_optional_string(value: Option<String>, field: &str) -> Result<String> {
    optional_string(value).with_context(|| format!("McpToolCall `{field}` is required"))
}

fn optional_string(value: Option<String>) -> Option<String> {
    let trimmed = value?.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn resolve_qualified_mcp_tool(
    runner: &dyn ToolRunner,
    requested: &str,
) -> Result<(String, String)> {
    if requested.trim().is_empty() {
        bail!("McpToolCall qualifiedToolName is required");
    }
    let mut keys = Vec::new();
    for server in runner.list_mcp_servers().map_err(map_runner_error)? {
        let tools = runner
            .list_mcp_tools(&server.id)
            .map_err(map_runner_error)
            .with_context(|| format!("list MCP tools for `{}`", server.id))?;
        keys.extend(tools.into_iter().map(|tool| McpToolKey {
            server: server.id.clone(),
            tool: tool.name,
        }));
    }

    let qualified = qualify_tools(keys.clone());
    let mut matches: Vec<(String, String)> = Vec::new();
    for (key, qualified) in keys.iter().zip(qualified.iter()) {
        let puffer_name = qualified.qualified_name.as_str();
        let hermes_name = hermes_mcp_name(&key.server, &key.tool);
        if requested == puffer_name || requested == hermes_name {
            let pair = (key.server.clone(), key.tool.clone());
            if !matches.contains(&pair) {
                matches.push(pair);
            }
        }
    }

    match matches.as_slice() {
        [(server, tool)] => Ok((server.clone(), tool.clone())),
        [] => bail!("MCP qualified tool `{requested}` was not found in the live registry"),
        _ => bail!("MCP qualified tool `{requested}` is ambiguous in the live registry"),
    }
}

fn hermes_mcp_name(server: &str, tool: &str) -> String {
    format!(
        "mcp_{}_{}",
        hermes_component(server),
        hermes_component(tool)
    )
}

fn hermes_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len().max(1));
    for ch in input.chars() {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
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
    use puffer_runner_api::{
        DirEntry, McpPrompt, McpPromptContent, McpResourceContent, McpResourceRecord,
        McpServerInfo, McpTool, RunnerCapabilities, RunnerPing, ToolRequest, ToolResult,
    };
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    #[derive(Debug)]
    struct FakeRunner {
        tools: Vec<(String, String)>,
    }

    impl FakeRunner {
        fn new(tools: &[(&str, &str)]) -> Self {
            Self {
                tools: tools
                    .iter()
                    .map(|(server, tool)| (server.to_string(), tool.to_string()))
                    .collect(),
            }
        }
    }

    impl ToolRunner for FakeRunner {
        fn ping(&self) -> Result<RunnerPing, RunnerError> {
            Ok(RunnerPing {
                version: "test".to_string(),
                uptime: Duration::ZERO,
            })
        }

        fn capabilities(&self) -> RunnerCapabilities {
            RunnerCapabilities::default()
        }

        fn execute_tool(
            &self,
            _: ToolRequest,
            _: &mut dyn ChunkSink,
        ) -> Result<ToolResult, RunnerError> {
            Err(RunnerError::Unsupported("not used".to_string()))
        }

        fn read_file(&self, _: &Path) -> Result<Vec<u8>, RunnerError> {
            Err(RunnerError::Unsupported("not used".to_string()))
        }

        fn list_dir(&self, _: &Path) -> Result<Vec<DirEntry>, RunnerError> {
            Err(RunnerError::Unsupported("not used".to_string()))
        }

        fn glob(&self, _: &Path, _: &str) -> Result<Vec<PathBuf>, RunnerError> {
            Err(RunnerError::Unsupported("not used".to_string()))
        }

        fn list_mcp_servers(&self) -> Result<Vec<McpServerInfo>, RunnerError> {
            let mut servers = self
                .tools
                .iter()
                .map(|(server, _)| server.clone())
                .collect::<Vec<_>>();
            servers.sort();
            servers.dedup();
            Ok(servers
                .into_iter()
                .map(|server| McpServerInfo {
                    id: server.clone(),
                    display_name: server,
                    transport: "stdio".to_string(),
                    target: "test".to_string(),
                    description: String::new(),
                })
                .collect())
        }

        fn list_mcp_tools(&self, server: &str) -> Result<Vec<McpTool>, RunnerError> {
            Ok(self
                .tools
                .iter()
                .filter(|(candidate, _)| candidate == server)
                .map(|(_, tool)| McpTool {
                    name: tool.clone(),
                    description: None,
                    input_schema: None,
                })
                .collect())
        }

        fn call_mcp_tool(
            &self,
            server: &str,
            tool: &str,
            args: Value,
            _: &mut dyn ChunkSink,
        ) -> Result<McpResult, RunnerError> {
            Ok(McpResult {
                server: server.to_string(),
                tool: tool.to_string(),
                success: true,
                stdout: String::new(),
                stderr: String::new(),
                metadata: json!({ "args": args }),
            })
        }

        fn list_mcp_resources(
            &self,
            _: Option<&str>,
        ) -> Result<Vec<McpResourceRecord>, RunnerError> {
            Err(RunnerError::Unsupported("not used".to_string()))
        }

        fn read_mcp_resource(&self, _: &str, _: &str) -> Result<McpResourceContent, RunnerError> {
            Err(RunnerError::Unsupported("not used".to_string()))
        }

        fn list_mcp_prompts(&self, _: &str) -> Result<Vec<McpPrompt>, RunnerError> {
            Err(RunnerError::Unsupported("not used".to_string()))
        }

        fn get_mcp_prompt(
            &self,
            _: &str,
            _: &str,
            _: Value,
        ) -> Result<McpPromptContent, RunnerError> {
            Err(RunnerError::Unsupported("not used".to_string()))
        }
    }

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

    #[test]
    fn resolves_puffer_qualified_mcp_tool_names() {
        let runner = FakeRunner::new(&[("my-server", "do.thing")]);
        let (server, tool) = resolve_qualified_mcp_tool(&runner, "mcp__my-server__do_thing")
            .expect("qualified Puffer MCP name should resolve");

        assert_eq!(server, "my-server");
        assert_eq!(tool, "do.thing");
    }

    #[test]
    fn resolves_hermes_qualified_mcp_tool_names() {
        let runner = FakeRunner::new(&[("my-server", "do.thing")]);
        let (server, tool) = resolve_qualified_mcp_tool(&runner, "mcp_my_server_do_thing")
            .expect("qualified Hermes MCP name should resolve");

        assert_eq!(server, "my-server");
        assert_eq!(tool, "do.thing");
    }

    #[test]
    fn rejects_ambiguous_hermes_qualified_mcp_tool_names() {
        let runner = FakeRunner::new(&[("a_b", "c"), ("a", "b_c")]);
        let error = resolve_qualified_mcp_tool(&runner, "mcp_a_b_c")
            .expect_err("ambiguous normalized MCP names must fail closed");

        assert!(format!("{error:#}").contains("ambiguous"));
    }

    #[test]
    fn qualified_mcp_call_requires_registry_proof() {
        let runner = FakeRunner::new(&[("agentmail", "send_message")]);
        let error = resolve_mcp_call(
            &runner,
            McpToolCallInput {
                server: None,
                tool: None,
                args: None,
                qualified_tool_name: Some("mcp_agentmail_send_message".to_string()),
                args_json: Some("{}".to_string()),
                registry: None,
            },
        )
        .expect_err("qualified MCP bridge must carry registry proof");

        assert!(format!("{error:#}").contains("registry proof"));
    }

    #[test]
    fn parses_qualified_mcp_call_args_json() {
        let runner = FakeRunner::new(&[("agentmail", "send_message")]);
        let resolved = resolve_mcp_call(
            &runner,
            McpToolCallInput {
                server: None,
                tool: None,
                args: None,
                qualified_tool_name: Some("mcp_agentmail_send_message".to_string()),
                args_json: Some(r#"{"subject":"hello"}"#.to_string()),
                registry: Some(json!({"kind": "registry"})),
            },
        )
        .expect("qualified MCP bridge should parse argsJson");

        assert_eq!(resolved.server, "agentmail");
        assert_eq!(resolved.tool, "send_message");
        assert_eq!(resolved.args, json!({"subject": "hello"}));
    }
}
