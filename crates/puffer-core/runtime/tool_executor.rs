use super::agents::execute_agent_tool;
use super::claude_tools::{self, ProviderToolContext};
use super::hook_support::{run_tool_end_hooks, run_tool_start_hooks};
use super::permission_prompt::{
    build_permission_prompt_request, prompt_for_permission, PermissionPromptAction,
    PermissionPromptRequest,
};
use super::structured_output_support::{
    requested_structured_output_definition_for_request, StructuredOutputConfig,
};
use super::RequestToolFilter;
use crate::permissions::{
    load_runtime_permission_context_with_inputs, FilesystemPermissionPolicy,
    RuntimePermissionInputs, ToolPermissionBehavior,
};
use crate::AppState;
use anyhow::{anyhow, Result};
use puffer_provider_openai::OpenAIRequestConfig;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_tools::{ToolExecutionResult, ToolOutput, ToolRegistry};
use puffer_transport_anthropic::AnthropicRequestConfig;
use serde_json::Value;
use std::fs;
use std::path::{Component, Path, PathBuf};

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
    let permission_context = load_runtime_permission_context_with_inputs(
        cwd,
        resources,
        state,
        RuntimePermissionInputs {
            request_tool_filter: tool_filter.cloned(),
        },
    )?;
    let mut filesystem_policy = permission_context.derived_policy().filesystem().clone();
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
                    state.allow_permission_for_tool_call(&definition, &input);
                }
                PermissionPromptAction::AllowAllSession => {
                    state.set_session_allow_all();
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
    filesystem_policy = match ensure_filesystem_path_access(
        state,
        resources,
        cwd,
        &definition,
        &input,
        tool_filter,
        filesystem_policy,
    )? {
        Ok(policy) => policy,
        Err(denied) => return Ok(denied),
    };
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
    run_tool_start_hooks(resources, cwd, tool_id, &hook_input);
    let result = if definition.handler == "runtime:agent" {
        let output = execute_agent_tool(state, resources, providers, auth_store, cwd, input)?;
        successful_runtime_tool(tool_id, output)
    } else if let Some(result) =
        execute_legacy_builtin_alias(&definition, cwd, &filesystem_policy, &input)?
    {
        result
    } else {
        claude_tools::execute_tool(
            state,
            resources,
            registry,
            &definition,
            cwd,
            &filesystem_policy,
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
pub(super) fn is_parallel_safe_tool(tool_id: &str) -> bool {
    matches!(
        tool_id,
        "Glob" | "Grep" | "WebFetch" | "WebSearch" | "ToolSearch" | "Skill" | "Bash"
    )
}

/// The result of pre-resolving permission for a tool call.
pub(super) enum PermissionOutcome {
    /// Tool execution is permitted.
    Allowed(FilesystemPermissionPolicy),
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
    let permission_context = load_runtime_permission_context_with_inputs(
        cwd,
        resources,
        state,
        RuntimePermissionInputs {
            request_tool_filter: tool_filter.cloned(),
        },
    )?;
    let permission_decision = permission_context.decision_for_tool_call(&definition, input);
    let base_policy = match permission_decision.behavior {
        ToolPermissionBehavior::Allow => permission_context.derived_policy().filesystem().clone(),
        ToolPermissionBehavior::Deny => {
            return Ok(PermissionOutcome::Denied(blocked_runtime_tool(
                tool_id,
                ToolPermissionBehavior::Deny,
                permission_decision.reason,
            )));
        }
        ToolPermissionBehavior::Ask => {
            match prompt_for_permission(build_permission_prompt_request(
                &definition,
                input,
                permission_decision.reason.as_deref(),
            )) {
                PermissionPromptAction::AllowOnce => {
                    permission_context.derived_policy().filesystem().clone()
                }
                PermissionPromptAction::AllowSession => {
                    state.allow_permission_for_tool_call(&definition, input);
                    runtime_filesystem_policy(cwd, resources, state, tool_filter)?
                }
                PermissionPromptAction::AllowAllSession => {
                    state.set_session_allow_all();
                    runtime_filesystem_policy(cwd, resources, state, tool_filter)?
                }
                PermissionPromptAction::Deny => {
                    return Ok(PermissionOutcome::Denied(blocked_runtime_tool(
                        tool_id,
                        ToolPermissionBehavior::Deny,
                        Some("permission denied by user".to_string()),
                    )));
                }
            }
        }
    };
    ensure_filesystem_path_access(
        state,
        resources,
        cwd,
        &definition,
        input,
        tool_filter,
        base_policy,
    )
    .map(|outcome| match outcome {
        Ok(policy) => PermissionOutcome::Allowed(policy),
        Err(denied) => PermissionOutcome::Denied(denied),
    })
}

/// Loads the effective filesystem policy for the current runtime permission state.
fn runtime_filesystem_policy(
    cwd: &Path,
    resources: &LoadedResources,
    state: &AppState,
    tool_filter: Option<&RequestToolFilter>,
) -> Result<FilesystemPermissionPolicy> {
    let permission_context = load_runtime_permission_context_with_inputs(
        cwd,
        resources,
        state,
        RuntimePermissionInputs {
            request_tool_filter: tool_filter.cloned(),
        },
    )?;
    Ok(permission_context.derived_policy().filesystem().clone())
}

fn ensure_filesystem_path_access(
    state: &mut AppState,
    resources: &LoadedResources,
    cwd: &Path,
    definition: &puffer_tools::ToolDefinition,
    input: &Value,
    tool_filter: Option<&RequestToolFilter>,
    mut policy: FilesystemPermissionPolicy,
) -> Result<std::result::Result<FilesystemPermissionPolicy, ToolExecutionResult>> {
    let Some(request) = filesystem_path_request(cwd, &definition.id, input) else {
        return Ok(Ok(policy));
    };
    if filesystem_policy_allows_path(cwd, &policy, &request.path) {
        return Ok(Ok(policy));
    }

    match prompt_for_permission(PermissionPromptRequest {
        tool_id: definition.id.clone(),
        summary: format!(
            "Allow {} to access {}",
            definition.id,
            request.grant_root.display()
        ),
        reason: Some(format!(
            "Path {} is outside the current working directories. Approve access to {} for this tool call.",
            request.path.display(),
            request.grant_root.display()
        )),
    }) {
        PermissionPromptAction::AllowOnce => {
            policy.workspace_roots.push(request.grant_root);
            Ok(Ok(policy))
        }
        PermissionPromptAction::AllowSession => {
            state.allow_path_for_session(request.grant_root);
            Ok(Ok(runtime_filesystem_policy(
                cwd,
                resources,
                state,
                tool_filter,
            )?))
        }
        PermissionPromptAction::AllowAllSession => {
            state.allow_path_for_session(request.grant_root);
            state.set_session_allow_all();
            Ok(Ok(runtime_filesystem_policy(
                cwd,
                resources,
                state,
                tool_filter,
            )?))
        }
        PermissionPromptAction::Deny => Ok(Err(blocked_runtime_tool(
            &definition.id,
            ToolPermissionBehavior::Deny,
            Some("permission denied by user".to_string()),
        ))),
    }
}

struct FilesystemPathRequest {
    path: PathBuf,
    grant_root: PathBuf,
}

fn filesystem_path_request(
    cwd: &Path,
    tool_id: &str,
    input: &Value,
) -> Option<FilesystemPathRequest> {
    let field = match tool_id {
        "Read" | "Write" | "Edit" => "file_path",
        "NotebookEdit" => "notebook_path",
        "Glob" | "Grep" => "path",
        "read_file" | "list_dir" | "search_text" => "path",
        "Agent" => "cwd",
        _ => return None,
    };
    let raw = input.get(field)?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    let path = normalize_permission_path(cwd, raw);
    let grant_root = grant_root_for_path(&path);
    Some(FilesystemPathRequest { path, grant_root })
}

fn execute_legacy_builtin_alias(
    definition: &puffer_tools::ToolDefinition,
    cwd: &Path,
    filesystem_policy: &FilesystemPermissionPolicy,
    input: &Value,
) -> Result<Option<ToolExecutionResult>> {
    match definition.id.as_str() {
        "read_file" => {
            let mut mapped = serde_json::Map::new();
            let Some(path) = input.get("path").and_then(Value::as_str) else {
                return Err(anyhow!("read_file requires path"));
            };
            mapped.insert("file_path".to_string(), Value::String(path.to_string()));
            if let Some(offset) = input.get("offset") {
                mapped.insert("offset".to_string(), offset.clone());
            }
            if let Some(limit) = input.get("limit") {
                mapped.insert("limit".to_string(), limit.clone());
            }
            let stdout = claude_tools::read::execute_claude_read_tool(
                cwd,
                &filesystem_policy.workspace_roots,
                &filesystem_policy.runner_policy(),
                Value::Object(mapped),
            )?;
            Ok(Some(successful_runtime_tool(&definition.id, stdout)))
        }
        "search_text" => {
            let Some(query) = input.get("query").and_then(Value::as_str) else {
                return Err(anyhow!("search_text requires query"));
            };
            let mut mapped = serde_json::Map::new();
            mapped.insert("pattern".to_string(), Value::String(query.to_string()));
            mapped.insert(
                "output_mode".to_string(),
                Value::String("content".to_string()),
            );
            if let Some(path) = input.get("path").and_then(Value::as_str) {
                mapped.insert("path".to_string(), Value::String(path.to_string()));
            }
            let stdout = claude_tools::grep::execute_claude_grep(
                cwd,
                &filesystem_policy.workspace_roots,
                &filesystem_policy.runner_policy(),
                Value::Object(mapped),
            )?;
            Ok(Some(successful_runtime_tool(&definition.id, stdout)))
        }
        "list_dir" => {
            let path = input
                .get("path")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(|path| {
                    crate::workspace_paths::resolve_path_for_filesystem_policy(
                        cwd,
                        &filesystem_policy.workspace_roots,
                        filesystem_policy.runner_policy().sandbox_mode,
                        Path::new(path),
                    )
                })
                .transpose()?
                .unwrap_or_else(|| cwd.to_path_buf());
            let mut entries = fs::read_dir(&path)
                .map_err(|error| anyhow!("failed to list directory {}: {error}", path.display()))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            entries.sort_by_key(|entry| entry.file_name());
            let stdout = entries
                .into_iter()
                .map(|entry| {
                    let suffix = entry
                        .file_type()
                        .map(|kind| if kind.is_dir() { "/" } else { "" })
                        .unwrap_or("");
                    format!("{}{}", entry.file_name().to_string_lossy(), suffix)
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(Some(successful_runtime_tool(&definition.id, stdout)))
        }
        _ => Ok(None),
    }
}

fn filesystem_policy_allows_path(
    cwd: &Path,
    policy: &FilesystemPermissionPolicy,
    path: &Path,
) -> bool {
    if policy.allow_all_paths() {
        return true;
    }
    crate::workspace_paths::workspace_roots(cwd, &policy.workspace_roots)
        .iter()
        .any(|root| path.starts_with(root))
}

fn grant_root_for_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }
    path.parent().unwrap_or(path).to_path_buf()
}

fn normalize_permission_path(cwd: &Path, raw_path: &str) -> PathBuf {
    let path = expand_tilde(raw_path).unwrap_or_else(|| PathBuf::from(raw_path));
    let joined = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    normalize_components(joined)
}

fn expand_tilde(raw_path: &str) -> Option<PathBuf> {
    if raw_path == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }
    raw_path
        .strip_prefix("~/")
        .or_else(|| raw_path.strip_prefix("~\\"))
        .and_then(|suffix| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(suffix)))
}

fn normalize_components(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
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
