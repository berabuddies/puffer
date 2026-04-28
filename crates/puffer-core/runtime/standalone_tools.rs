use crate::permissions::load_runtime_permission_context;
use crate::runtime::claude_tools::{self, ProviderToolContext};
use crate::runtime::hook_support::{run_tool_end_hooks, run_tool_start_hooks};
use crate::AppState;
use anyhow::{anyhow, bail, Result};
use puffer_config::PufferConfig;
use puffer_provider_openai::OpenAIRequestConfig;
use puffer_resources::LoadedResources;
use puffer_session_store::SessionMetadata;
use puffer_tools::{ToolExecutionResult, ToolRegistry};
use puffer_transport_anthropic::AnthropicRequestConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use uuid::Uuid;

/// Provider context for standalone Puffer tool execution.
#[derive(Debug, Clone)]
pub enum StandaloneProviderContext {
    None,
    OpenAi {
        request_config: OpenAIRequestConfig,
        model_id: String,
    },
    Anthropic {
        request_config: AnthropicRequestConfig,
        model_id: String,
    },
}

/// Normalized standalone tool execution result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StandaloneToolResult {
    pub tool_id: String,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub metadata: Value,
}

/// Executes an allowlisted Puffer runtime tool outside a conversation turn.
pub struct StandaloneToolExecutor {
    state: AppState,
    resources: LoadedResources,
    registry: ToolRegistry,
    provider_context: StandaloneProviderContext,
}

impl StandaloneToolExecutor {
    /// Creates a standalone executor with a synthetic session and loaded resources.
    pub fn new(config: PufferConfig, cwd: PathBuf, resources: LoadedResources) -> Self {
        let session = SessionMetadata {
            id: Uuid::new_v4(),
            display_name: Some("Burbot standalone tools".to_string()),
            generated_title: None,
            cwd: cwd.clone(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        let registry = ToolRegistry::from_resources(&resources);
        Self {
            state: AppState::new(config, cwd, session),
            resources,
            registry,
            provider_context: StandaloneProviderContext::None,
        }
    }

    /// Replaces the working directory allowlist used by file-search tools.
    pub fn with_working_dirs(mut self, working_dirs: Vec<PathBuf>) -> Self {
        self.state.working_dirs = working_dirs;
        self
    }

    /// Replaces the sandbox mode used by path-resolving Puffer tools.
    pub fn with_sandbox_mode(mut self, sandbox_mode: impl Into<String>) -> Self {
        self.state.sandbox_mode = sandbox_mode.into();
        self
    }

    /// Replaces the optional provider context used by provider-backed tools.
    pub fn with_provider_context(mut self, provider_context: StandaloneProviderContext) -> Self {
        self.provider_context = provider_context;
        self
    }

    /// Executes one allowlisted Puffer tool by id or alias.
    pub fn execute_tool(&mut self, tool_id: &str, input: Value) -> Result<StandaloneToolResult> {
        let cwd = self.state.cwd.clone();
        let definition = self
            .registry
            .definition(tool_id)
            .ok_or_else(|| anyhow!("unknown Puffer tool {tool_id}"))?;
        reject_disallowed_tool(
            definition.id.as_str(),
            definition.handler.as_str(),
            &input,
            tool_has_contract(&self.resources, definition.id.as_str()),
        )?;
        if !self.state.session_allow_all {
            let permission_context =
                load_runtime_permission_context(&cwd, &self.resources, &self.state)?;
            permission_context.enforce_tool_call(definition, &input)?;
        }
        let provider_context = match &self.provider_context {
            StandaloneProviderContext::None => ProviderToolContext::None,
            StandaloneProviderContext::OpenAi {
                request_config,
                model_id,
            } => ProviderToolContext::OpenAI {
                request_config,
                model_id,
                structured_output: None,
            },
            StandaloneProviderContext::Anthropic {
                request_config,
                model_id,
            } => ProviderToolContext::Anthropic {
                request_config,
                model_id,
                structured_output: None,
            },
        };
        let hook_input = input.clone();
        run_tool_start_hooks(&self.resources, &cwd, &definition.id, &hook_input);
        let result = claude_tools::execute_tool(
            &mut self.state,
            &self.resources,
            &self.registry,
            definition,
            &cwd,
            input,
            provider_context,
        )?;
        run_tool_end_hooks(
            &self.resources,
            &cwd,
            &definition.id,
            &hook_input,
            result.success,
            &result.output.stdout,
            &result.output.stderr,
        );
        Ok(standalone_result(result))
    }
}

fn reject_disallowed_tool(
    tool_id: &str,
    handler: &str,
    input: &Value,
    has_contract: bool,
) -> Result<()> {
    if tool_id == "Agent" || handler == "runtime:agent" {
        bail!("subagent tool execution is not available through standalone tools");
    }
    if handler.starts_with("runtime:workflow:") {
        bail!("workflow/AppState tool {tool_id} is not available through standalone tools");
    }
    if tool_id == "Browser" || handler == "runtime:browser" {
        bail!("browser session tools are not available through standalone tools");
    }
    if tool_id == "Bash"
        && input
            .get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        bail!("background Bash is not available through standalone tools");
    }
    if !has_contract {
        bail!("tool {tool_id} has no standalone execution contract");
    }
    Ok(())
}

fn tool_has_contract(resources: &LoadedResources, tool_id: &str) -> bool {
    resources
        .tools
        .iter()
        .any(|item| item.value.id == tool_id && item.value.contract.is_some())
}

fn standalone_result(result: ToolExecutionResult) -> StandaloneToolResult {
    StandaloneToolResult {
        tool_id: result.tool_id,
        success: result.success,
        stdout: result.output.stdout,
        stderr: result.output.stderr,
        metadata: result.output.metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_resources::{LoadedItem, SourceInfo, SourceKind, ToolSpec};
    use serde_json::json;

    fn resources_with_tool(tool: ToolSpec) -> LoadedResources {
        LoadedResources {
            tools: vec![LoadedItem {
                value: tool,
                source_info: SourceInfo {
                    path: PathBuf::from("tool.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        }
    }

    #[test]
    fn executes_allowlisted_sleep_tool() {
        let tool = ToolSpec {
            id: "Sleep".to_string(),
            name: "Sleep".to_string(),
            description: "Sleep briefly.".to_string(),
            handler: "runtime:sleep".to_string(),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "duration_ms": {"type": "integer"}
                },
                "required": ["duration_ms"]
            })),
            contract: Some(json!({"side_effect_class": "pure"})),
            ..ToolSpec::default()
        };
        let mut executor = StandaloneToolExecutor::new(
            PufferConfig::default(),
            std::env::current_dir().unwrap(),
            resources_with_tool(tool),
        );

        let result = executor
            .execute_tool("Sleep", json!({"duration_ms": 1, "reason": "test"}))
            .unwrap();

        assert!(result.success);
        assert!(result.stdout.contains("\"completed\": true"));
    }

    #[test]
    fn rejects_subagent_tool() {
        let tool = ToolSpec {
            id: "Agent".to_string(),
            name: "Agent".to_string(),
            description: "Spawn a subagent.".to_string(),
            handler: "runtime:agent".to_string(),
            ..ToolSpec::default()
        };
        let mut executor = StandaloneToolExecutor::new(
            PufferConfig::default(),
            std::env::current_dir().unwrap(),
            resources_with_tool(tool),
        );

        let error = executor.execute_tool("Agent", json!({})).unwrap_err();
        assert!(error.to_string().contains("subagent"));
    }
}
