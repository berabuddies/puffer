use super::agents::execute_agent_tool;
use super::claude_tools::{self, ProviderToolContext};
use super::hook_support::{run_tool_end_hooks, run_tool_start_hooks};
use super::permission_prompt::{
    build_permission_prompt_request, prompt_for_permission, PermissionPromptAction,
};
use super::remote_tool_executor::{maybe_execute_remote_tool_call, remote_tool_parallel_safe};
use super::structured_output_support::{
    requested_structured_output_definition_for_request, StructuredOutputConfig,
};
use super::RequestToolFilter;
use crate::permissions::{load_runtime_permission_context, ToolPermissionBehavior};
use crate::AppState;
use anyhow::{anyhow, Result};
use puffer_provider_openai::OpenAIRequestConfig;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_tools::{ToolExecutionResult, ToolOutput, ToolRegistry};
use puffer_transport_anthropic::AnthropicRequestConfig;
use serde_json::Value;
use std::path::Path;

/// Identifies which provider loop is currently executing a tool call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum ToolExecutionBackend<'a> {
    Anthropic {
        request_config: &'a AnthropicRequestConfig,
        structured_output: Option<&'a StructuredOutputConfig>,
    },
    OpenAi {
        request_config: &'a OpenAIRequestConfig,
        structured_output: Option<&'a StructuredOutputConfig>,
    },
}

/// Executes one tool call with access to the full conversation runtime context.
pub(super) fn execute_tool_call(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    registry: &ToolRegistry,
    model_id: &str,
    cwd: &Path,
    backend: ToolExecutionBackend<'_>,
    tool_filter: Option<&RequestToolFilter>,
    call_id: Option<&str>,
    tool_id: &str,
    input: Value,
) -> Result<ToolExecutionResult> {
    let structured_output = match backend {
        ToolExecutionBackend::Anthropic {
            structured_output, ..
        }
        | ToolExecutionBackend::OpenAi {
            structured_output, ..
        } => structured_output,
    };
    let definition = match registry.definition(tool_id) {
        Some(definition) => definition.clone(),
        None => requested_structured_output_definition_for_request(registry, structured_output)?
            .filter(|definition| definition.id == tool_id)
            .ok_or_else(|| anyhow!("unknown tool {tool_id}"))?,
    };
    if let Some(filter) = tool_filter {
        if !filter.allows_call(&definition, cwd, &input)? {
            return Ok(blocked_runtime_tool(
                tool_id,
                ToolPermissionBehavior::Deny,
                Some("slash command tool scope denied this tool call".to_string()),
            ));
        }
    }
    if !state.session_allow_all {
        let permission_context = load_runtime_permission_context(cwd, resources, state)?;
        let permission_decision = permission_context.decision_for_tool_call(&definition, &input);
        match permission_decision.behavior {
            ToolPermissionBehavior::Allow => {}
            ToolPermissionBehavior::Deny => {
                return Ok(blocked_runtime_tool(
                    tool_id,
                    ToolPermissionBehavior::Deny,
                    permission_decision.reason,
                ));
            }
            ToolPermissionBehavior::Ask => {
                match prompt_for_permission(build_permission_prompt_request(
                    &definition,
                    &input,
                    permission_decision.reason.as_deref(),
                )) {
                    PermissionPromptAction::AllowOnce => {}
                    PermissionPromptAction::AllowSession => {
                        state.allow_tool_for_session(&definition.id);
                    }
                    PermissionPromptAction::AllowAllSession => {
                        state.session_allow_all = true;
                    }
                    PermissionPromptAction::Deny => {
                        return Ok(blocked_runtime_tool(
                            tool_id,
                            ToolPermissionBehavior::Deny,
                            Some("permission denied by user".to_string()),
                        ));
                    }
                }
            }
        }
    }
    let provider_context = match backend {
        ToolExecutionBackend::Anthropic {
            request_config,
            structured_output,
        } => ProviderToolContext::Anthropic {
            request_config,
            model_id,
            structured_output,
        },
        ToolExecutionBackend::OpenAi {
            request_config,
            structured_output,
        } => ProviderToolContext::OpenAI {
            request_config,
            model_id,
            structured_output,
        },
    };
    let hook_input = input.clone();
    let remote_execution_context =
        claude_tools::build_remote_execution_context(tool_id, &provider_context, &input)?;
    run_tool_start_hooks(resources, cwd, tool_id, &hook_input);
    let result = if let Some(result) = maybe_execute_remote_tool_call(
        state,
        call_id,
        tool_id,
        input.clone(),
        remote_execution_context.as_ref(),
    )? {
        result
    } else if definition.handler == "runtime:agent" {
        let output = execute_agent_tool(state, resources, providers, auth_store, cwd, input)?;
        successful_runtime_tool(tool_id, output)
    } else {
        claude_tools::execute_tool(
            state,
            resources,
            registry,
            &definition,
            cwd,
            input,
            provider_context,
        )?
    };
    run_tool_end_hooks(
        resources,
        cwd,
        tool_id,
        &hook_input,
        result.success,
        &result.output.stdout,
        &result.output.stderr,
    );
    Ok(result)
}

fn successful_runtime_tool(tool_id: &str, stdout: String) -> ToolExecutionResult {
    ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: true,
        output: ToolOutput {
            stdout,
            stderr: String::new(),
            metadata: Value::Null,
        },
    }
}

/// Returns `true` when a tool can be executed without `&mut AppState`.
///
/// These tools perform pure IO (filesystem reads, HTTP requests, process spawning)
/// and don't read or write any mutable application state. This classification
/// enables parallel execution when the model requests multiple tool calls.
pub(super) fn is_parallel_safe_tool(state: &AppState, tool_id: &str) -> bool {
    matches!(
        tool_id,
        "Glob" | "Grep" | "WebFetch" | "WebSearch" | "ToolSearch" | "Skill" | "Bash"
    ) && remote_tool_parallel_safe(state, tool_id)
}

/// The result of pre-resolving permission for a tool call.
pub(super) enum PermissionOutcome {
    /// Tool execution is permitted.
    Allowed,
    /// Tool execution was denied; carry the pre-built denial result.
    Denied(ToolExecutionResult),
}

/// Pre-resolves permission for one tool call.
///
/// This is separated from `execute_tool_call` so that permissions can be
/// resolved serially (may prompt the user) before tools are dispatched in
/// parallel.
pub(super) fn resolve_tool_permission(
    state: &mut AppState,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    cwd: &Path,
    tool_id: &str,
    input: &Value,
    tool_filter: Option<&super::RequestToolFilter>,
) -> Result<PermissionOutcome> {
    let definition = match registry.definition(tool_id) {
        Some(d) => d.clone(),
        None => {
            return Ok(PermissionOutcome::Denied(blocked_runtime_tool(
                tool_id,
                ToolPermissionBehavior::Deny,
                Some(format!("unknown tool {tool_id}")),
            )));
        }
    };
    if let Some(filter) = tool_filter {
        if !filter.allows_call(&definition, cwd, input)? {
            return Ok(PermissionOutcome::Denied(blocked_runtime_tool(
                tool_id,
                ToolPermissionBehavior::Deny,
                Some("slash command tool scope denied this tool call".to_string()),
            )));
        }
    }
    if state.session_allow_all {
        return Ok(PermissionOutcome::Allowed);
    }
    let permission_context = load_runtime_permission_context(cwd, resources, state)?;
    let permission_decision = permission_context.decision_for_tool_call(&definition, input);
    match permission_decision.behavior {
        ToolPermissionBehavior::Allow => Ok(PermissionOutcome::Allowed),
        ToolPermissionBehavior::Deny => Ok(PermissionOutcome::Denied(blocked_runtime_tool(
            tool_id,
            ToolPermissionBehavior::Deny,
            permission_decision.reason,
        ))),
        ToolPermissionBehavior::Ask => {
            match prompt_for_permission(build_permission_prompt_request(
                &definition,
                input,
                permission_decision.reason.as_deref(),
            )) {
                PermissionPromptAction::AllowOnce => Ok(PermissionOutcome::Allowed),
                PermissionPromptAction::AllowSession => {
                    state.allow_tool_for_session(&definition.id);
                    Ok(PermissionOutcome::Allowed)
                }
                PermissionPromptAction::AllowAllSession => {
                    state.session_allow_all = true;
                    Ok(PermissionOutcome::Allowed)
                }
                PermissionPromptAction::Deny => {
                    Ok(PermissionOutcome::Denied(blocked_runtime_tool(
                        tool_id,
                        ToolPermissionBehavior::Deny,
                        Some("permission denied by user".to_string()),
                    )))
                }
            }
        }
    }
}

fn blocked_runtime_tool(
    tool_id: &str,
    behavior: ToolPermissionBehavior,
    reason: Option<String>,
) -> ToolExecutionResult {
    let prefix = match behavior {
        ToolPermissionBehavior::Allow => "Allowed",
        ToolPermissionBehavior::Ask => "Permission required",
        ToolPermissionBehavior::Deny => "Permission denied",
    };
    let stdout = reason
        .map(|reason| format!("{prefix}: {reason}"))
        .unwrap_or_else(|| prefix.to_string());
    ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: false,
        output: ToolOutput {
            stdout,
            stderr: String::new(),
            metadata: Value::Null,
        },
    }
}
