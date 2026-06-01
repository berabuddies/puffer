use super::agents::execute_agent_tool;
use super::auto_approval_review::{
    build_auto_permission_review_request, run_auto_permission_review, AutoPermissionReviewDecision,
};
use super::browser_auto_review::{
    build_browser_auto_review_request, run_browser_auto_review, BrowserAutoReviewRuntimeResult,
    BrowserAutoReviewSessionTargeting,
};
use super::claude_tools::{self, ProviderToolContext};
use super::filesystem_access::{ensure_filesystem_path_access, runtime_filesystem_policy};
use super::hook_support::{run_tool_end_hooks, run_tool_start_hooks};
use super::lambda_gate::merge_tool_metadata;
use super::lambda_tool::{
    commit_successful_lambda_skill_gate_call, lambda_skill_gate_scopes_tool_call,
    lambda_skill_skips_concrete_approval, prepare_lambda_host_call,
    reject_lambda_skill_gate_preflight, LAMBDA_HOST_CALL_TOOL_ID,
};
use super::local_tools::{
    enrich_browser_permission_input, read_current_tab_context, BrowserCurrentTabStatus,
};
use super::permission_prompt::{
    build_permission_prompt_request, prompt_for_permission, PermissionPromptAction,
};
use super::structured_output_support::{
    requested_structured_output_definition_for_request, StructuredOutputConfig,
};
use super::RequestToolFilter;
use crate::permissions::acl::{
    append_allow_all_rule, append_allow_bash_rule, append_allow_browser_rule,
};
use crate::permissions::browser_action::browser_permission_value_for_tool_call;
use crate::permissions::browser_grants::BrowserGrantScopeKind;
use crate::permissions::browser_target::browser_permission_context_for_tool;
use crate::permissions::{
    load_runtime_permission_context_with_inputs, FilesystemPermissionPolicy,
    RuntimePermissionInputs, ToolPermissionBehavior,
};
use crate::tool_names::canonical_tool_name;
use crate::AppState;
use anyhow::{anyhow, Result};
use puffer_provider_openai::OpenAIRequestConfig;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_tools::{ToolExecutionResult, ToolOutput, ToolRegistry};
use puffer_transport_anthropic::AnthropicRequestConfig;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const BROWSER_REVIEW_METADATA_KEY: &str = "__pufferBrowserReview";
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
    let gate_scopes_tool =
        lambda_skill_gate_scopes_tool_call(state, registry, cwd, tool_id, &input);
    let effective_tool_filter = active_tool_filter(state, tool_filter, gate_scopes_tool, tool_id);
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
    if definition.id == LAMBDA_HOST_CALL_TOOL_ID {
        return Ok(prepare_lambda_host_call(
            state,
            registry,
            cwd,
            effective_tool_filter.as_ref(),
            tool_id,
            input,
        ));
    }
    if let Some(denied) = reject_lambda_skill_gate_preflight(state, registry, cwd, tool_id, &input)
    {
        return Ok(denied);
    }
    if let Some(denied) = enforce_pentest_scope(state, tool_id, &input) {
        return Ok(denied);
    }
    let skip_verified_approval =
        lambda_skill_skips_concrete_approval(state, registry, tool_id, &input);
    let input = prepare_browser_permission_input(state, cwd, &definition, input)?;
    let permission_context = load_runtime_permission_context_with_inputs(
        cwd,
        resources,
        state,
        RuntimePermissionInputs {
            request_tool_filter: effective_tool_filter.clone(),
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
            if !skip_verified_approval {
                match resolve_ask_behavior(
                    state,
                    resources,
                    providers,
                    auth_store,
                    cwd,
                    effective_tool_filter.as_ref(),
                    &definition,
                    &input,
                    permission_decision.reason.as_deref(),
                    &permission_context.effective_profile().current_session_id,
                    &permission_context.effective_profile().workspace_roots,
                )? {
                    AskResolution::AllowOnce => {}
                    AskResolution::AllowSession => {}
                    AskResolution::Deny => {
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
    filesystem_policy = match ensure_filesystem_path_access(
        state,
        resources,
        providers,
        auth_store,
        cwd,
        &definition,
        &input,
        effective_tool_filter.as_ref(),
        filesystem_policy,
        skip_verified_approval,
    )? {
        Ok(policy) => policy,
        Err(denied) => return Ok(denied),
    };
    let proxy_config = state.config.network.proxy.clone();
    let provider_context = match backend {
        ToolExecutionBackend::Anthropic {
            request_config,
            structured_output,
        } => ProviderToolContext::Anthropic {
            request_config,
            model_id,
            proxy: &proxy_config,
            structured_output,
        },
        ToolExecutionBackend::OpenAi {
            request_config,
            structured_output,
        } => ProviderToolContext::OpenAI {
            request_config,
            model_id,
            proxy: &proxy_config,
            structured_output,
        },
    };
    let hook_input = input.clone();
    run_tool_start_hooks(resources, cwd, tool_id, &hook_input);
    let execution_input = if definition.handler == "runtime:agent" {
        input.clone()
    } else {
        super::secrets::expand_secret_placeholders(state, &input)?
    };
    let lambda_skill_target_snapshot = if tool_id == "Skill"
        && state
            .pending_lambda_host_call
            .as_ref()
            .is_some_and(|pending| pending.concrete_tool() == "Skill")
    {
        Some((
            state.lambda_gate.clone(),
            state.pending_lambda_host_call.clone(),
        ))
    } else {
        None
    };
    let execution_result: Result<ToolExecutionResult> = if definition.handler == "runtime:agent" {
        let output = execute_agent_tool(
            state,
            resources,
            providers,
            auth_store,
            cwd,
            execution_input,
        );
        output.map(|output| successful_runtime_tool(tool_id, output))
    } else {
        match execute_legacy_builtin_alias(&definition, cwd, &filesystem_policy, &execution_input) {
            Ok(Some(result)) => Ok(result),
            Ok(None) => claude_tools::execute_tool(
                state,
                resources,
                registry,
                &definition,
                cwd,
                &filesystem_policy,
                execution_input,
                provider_context,
            ),
            Err(error) => Err(error),
        }
    };
    let mut result = match execution_result {
        Ok(result) => result,
        Err(error) => {
            return Err(anyhow!(super::secrets::redact_known_secrets(
                state,
                &error.to_string()
            )));
        }
    };
    super::secrets::redact_tool_result(state, &mut result);
    if result.success {
        let post_skill_gate = lambda_skill_target_snapshot.as_ref().map(|_| {
            (
                state.lambda_gate.clone(),
                state.pending_lambda_host_call.clone(),
            )
        });
        if let Some((saved_gate, saved_pending)) = lambda_skill_target_snapshot {
            state.lambda_gate = saved_gate;
            state.pending_lambda_host_call = saved_pending;
        }
        let lambda_metadata =
            match commit_successful_lambda_skill_gate_call(state, tool_id, &result.output) {
                Ok(metadata) => metadata,
                Err(denied) => return Ok(denied),
            };
        if let Some((next_gate, next_pending)) = post_skill_gate {
            state.lambda_gate = next_gate;
            state.pending_lambda_host_call = next_pending;
        }
        if let Some(metadata) = lambda_metadata {
            merge_tool_metadata(&mut result.output.metadata, metadata);
        }
    }
    remember_browser_target(state, &definition, &input);
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

fn active_tool_filter(
    state: &AppState,
    tool_filter: Option<&RequestToolFilter>,
    gate_scopes_tool: bool,
    tool_id: &str,
) -> Option<RequestToolFilter> {
    if let Some(tool_filter) = tool_filter {
        return Some(tool_filter.clone());
    }
    if tool_id == "Skill" {
        return None;
    }
    if !gate_scopes_tool {
        return None;
    }
    state
        .lambda_gate
        .as_ref()
        .and_then(|gate| gate.request_tool_filter().cloned())
}

fn successful_runtime_tool(tool_id: &str, stdout: String) -> ToolExecutionResult {
    successful_runtime_tool_with_metadata(tool_id, stdout, Value::Null)
}

fn successful_runtime_tool_with_metadata(
    tool_id: &str,
    stdout: String,
    metadata: Value,
) -> ToolExecutionResult {
    ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: true,
        output: ToolOutput {
            stdout,
            stderr: String::new(),
            metadata,
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
        "Glob"
            | "Grep"
            | "WebFetch"
            | "WebSearch"
            | "ToolSearch"
            | "Skill"
            | "Bash"
            | "Agent"
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
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    registry: &ToolRegistry,
    cwd: &Path,
    tool_id: &str,
    input: &Value,
    tool_filter: Option<&super::RequestToolFilter>,
) -> Result<PermissionOutcome> {
    let gate_scopes_tool = lambda_skill_gate_scopes_tool_call(state, registry, cwd, tool_id, input);
    let effective_tool_filter = active_tool_filter(state, tool_filter, gate_scopes_tool, tool_id);
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
    let skip_verified_approval =
        lambda_skill_skips_concrete_approval(state, registry, tool_id, input);
    let input = prepare_browser_permission_input(state, cwd, &definition, input.clone())?;
    let permission_context = load_runtime_permission_context_with_inputs(
        cwd,
        resources,
        state,
        RuntimePermissionInputs {
            request_tool_filter: effective_tool_filter.clone(),
        },
    )?;
    let permission_decision = permission_context.decision_for_tool_call(&definition, &input);
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
            if skip_verified_approval {
                permission_context.derived_policy().filesystem().clone()
            } else {
                match resolve_ask_behavior(
                    state,
                    resources,
                    providers,
                    auth_store,
                    cwd,
                    effective_tool_filter.as_ref(),
                    &definition,
                    &input,
                    permission_decision.reason.as_deref(),
                    &permission_context.effective_profile().current_session_id,
                    &permission_context.effective_profile().workspace_roots,
                )? {
                    AskResolution::AllowOnce => {
                        permission_context.derived_policy().filesystem().clone()
                    }
                    AskResolution::AllowSession => {
                        remember_browser_target(state, &definition, &input);
                        runtime_filesystem_policy(
                            cwd,
                            resources,
                            state,
                            effective_tool_filter.as_ref(),
                        )?
                    }
                    AskResolution::Deny => {
                        return Ok(PermissionOutcome::Denied(blocked_runtime_tool(
                            tool_id,
                            ToolPermissionBehavior::Deny,
                            Some("permission denied by user".to_string()),
                        )));
                    }
                }
            }
        }
    };
    ensure_filesystem_path_access(
        state,
        resources,
        providers,
        auth_store,
        cwd,
        &definition,
        &input,
        effective_tool_filter.as_ref(),
        base_policy,
        skip_verified_approval,
    )
    .map(|outcome| match outcome {
        Ok(policy) => PermissionOutcome::Allowed(policy),
        Err(denied) => PermissionOutcome::Denied(denied),
    })
}

enum AskResolution {
    AllowOnce,
    AllowSession,
    Deny,
}

fn resolve_ask_behavior(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    cwd: &Path,
    _tool_filter: Option<&RequestToolFilter>,
    definition: &puffer_tools::ToolDefinition,
    input: &Value,
    reason: Option<&str>,
    current_session_id: &str,
    workspace_roots: &[std::path::PathBuf],
) -> Result<AskResolution> {
    let carries_browser_permission =
        browser_permission_value_for_tool_call(&definition.id, input).is_some();
    let browser_session_grant = carries_browser_permission.then(|| {
        browser_grant_scope_for_prompt_action(
            definition,
            input,
            current_session_id,
            workspace_roots,
        )
    });
    let prompt_request = build_permission_prompt_request(
        definition,
        input,
        reason,
        current_session_id,
        workspace_roots,
    );
    let auto_review_request = build_auto_permission_review_request(
        state,
        cwd,
        &prompt_request,
        input,
        current_session_id,
        workspace_roots,
    );
    match run_auto_permission_review(
        state,
        resources,
        providers,
        auth_store,
        &auto_review_request,
    )
    .decision
    {
        AutoPermissionReviewDecision::AllowOnce => return Ok(AskResolution::AllowOnce),
        AutoPermissionReviewDecision::AllowSession => {
            if carries_browser_permission {
                state.allow_browser_permission_for_tool_call(
                    definition,
                    input,
                    browser_session_grant.unwrap_or(BrowserGrantScopeKind::AllowOnce),
                );
            } else {
                state.allow_permission_for_tool_call(definition, input);
            }
            persist_acl_allow_for_tool_call(
                cwd,
                definition,
                input,
                current_session_id,
                workspace_roots,
            )?;
            return Ok(AskResolution::AllowSession);
        }
        AutoPermissionReviewDecision::Deny => return Ok(AskResolution::Deny),
        AutoPermissionReviewDecision::NeedsUser | AutoPermissionReviewDecision::Unavailable => {}
    }
    if let Some(browser) = prompt_request.browser.as_ref() {
        let resolved_root_session_id = input
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "current")
            .unwrap_or(current_session_id)
            .to_string();
        let session_targeting = if resolved_root_session_id == current_session_id {
            BrowserAutoReviewSessionTargeting::CurrentSession
        } else {
            BrowserAutoReviewSessionTargeting::ExplicitSession
        };
        let review_request = build_browser_auto_review_request(
            &definition.id,
            input,
            prompt_request.summary.clone(),
            prompt_request.reason.clone(),
            browser,
            resolved_root_session_id,
            session_targeting,
            browser_session_grant.unwrap_or(BrowserGrantScopeKind::AllowOnce),
        );
        match run_browser_auto_review(state, resources, providers, auth_store, &review_request) {
            BrowserAutoReviewRuntimeResult::AllowOnce => return Ok(AskResolution::AllowOnce),
            BrowserAutoReviewRuntimeResult::AllowSession => {
                state.allow_browser_permission_for_tool_call(
                    definition,
                    input,
                    browser_session_grant.unwrap_or(BrowserGrantScopeKind::AllowOnce),
                );
                persist_acl_allow_for_tool_call(
                    cwd,
                    definition,
                    input,
                    current_session_id,
                    workspace_roots,
                )?;
                return Ok(AskResolution::AllowSession);
            }
            BrowserAutoReviewRuntimeResult::Deny => return Ok(AskResolution::Deny),
            BrowserAutoReviewRuntimeResult::NeedsUser
            | BrowserAutoReviewRuntimeResult::Unavailable => {}
        }
    }
    match prompt_for_permission(prompt_request) {
        PermissionPromptAction::AllowOnce => Ok(AskResolution::AllowOnce),
        PermissionPromptAction::AllowSession => {
            if carries_browser_permission {
                state.allow_browser_permission_for_tool_call(
                    definition,
                    input,
                    browser_session_grant.unwrap_or(BrowserGrantScopeKind::AllowOnce),
                );
            } else {
                state.allow_permission_for_tool_call(definition, input);
            }
            persist_acl_allow_for_tool_call(
                cwd,
                definition,
                input,
                current_session_id,
                workspace_roots,
            )?;
            Ok(AskResolution::AllowSession)
        }
        PermissionPromptAction::AllowAllSession => {
            if carries_browser_permission {
                state.allow_browser_permission_for_tool_call(
                    definition,
                    input,
                    browser_session_grant.unwrap_or(BrowserGrantScopeKind::AllowOnce),
                );
            } else {
                state.grant_all_tools_for_session();
            }
            append_allow_all_rule(cwd)?;
            Ok(AskResolution::AllowSession)
        }
        PermissionPromptAction::Deny => Ok(AskResolution::Deny),
    }
}

fn persist_acl_allow_for_tool_call(
    cwd: &Path,
    definition: &puffer_tools::ToolDefinition,
    input: &Value,
    current_session_id: &str,
    workspace_roots: &[PathBuf],
) -> Result<()> {
    if browser_permission_value_for_tool_call(&definition.id, input).is_some() {
        let context = browser_permission_context_for_tool(
            &definition.id,
            input,
            current_session_id,
            workspace_roots,
        );
        return append_allow_browser_rule(cwd, &context);
    }
    if matches!(
        canonical_tool_name(&definition.id).as_str(),
        "bash" | "powershell"
    ) {
        if let Some(command) = input.get("command").and_then(Value::as_str) {
            append_allow_bash_rule(cwd, command)?;
        }
    }
    Ok(())
}

fn browser_grant_scope_for_prompt_action(
    definition: &puffer_tools::ToolDefinition,
    input: &Value,
    current_session_id: &str,
    workspace_roots: &[std::path::PathBuf],
) -> BrowserGrantScopeKind {
    let context = browser_permission_context_for_tool(
        &definition.id,
        input,
        current_session_id,
        workspace_roots,
    );
    crate::permissions::browser_grants::suggested_browser_grant_scope(&context)
}

fn prepare_browser_permission_input(
    state: &AppState,
    cwd: &Path,
    definition: &puffer_tools::ToolDefinition,
    input: Value,
) -> Result<Value> {
    if let Some(browser_input) = browser_permission_value_for_tool_call(&definition.id, &input) {
        let raw_action = browser_input
            .get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(str::to_ascii_lowercase);
        let explicit_requested_url = browser_input
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let browser_session_id = state.browser_root_session_id();
        let enriched = enrich_browser_permission_input(cwd, &browser_session_id, browser_input)?;
        let current_session_id = browser_session_id.to_string();
        let root_session_id = enriched
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "current")
            .map(ToString::to_string)
            .unwrap_or(current_session_id);
        let enriched = apply_browser_url_fallback(state, cwd, &root_session_id, enriched)?;
        let current_tab_url = if explicit_requested_url.is_some()
            && matches!(raw_action.as_deref(), Some("open" | "new"))
        {
            read_current_tab_context(cwd, &root_session_id)
                .ok()
                .and_then(|context| context.url)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("about:blank"))
        } else {
            None
        };
        let enriched = attach_browser_review_metadata(
            enriched,
            explicit_requested_url.as_deref(),
            current_tab_url.as_deref(),
        );
        if canonical_tool_name(&definition.id) == "browser" {
            return Ok(enriched);
        }
    }
    Ok(input)
}

fn attach_browser_review_metadata(
    input: Value,
    explicit_requested_url: Option<&str>,
    current_tab_url: Option<&str>,
) -> Value {
    let Some(payload) = input.as_object() else {
        return input;
    };
    let effective_url = payload
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let url_source = match (explicit_requested_url, effective_url.as_deref()) {
        (Some(requested), Some(effective)) if requested.eq_ignore_ascii_case(effective) => {
            "explicit"
        }
        (Some(_), Some(_)) => "current_tab",
        (Some(_), None) => "none",
        (None, Some(_)) => "current_tab",
        (None, None) => "none",
    };
    let mut enriched = payload.clone();
    enriched.insert(
        BROWSER_REVIEW_METADATA_KEY.to_string(),
        serde_json::json!({
            "urlSource": url_source,
            "requestedUrl": explicit_requested_url,
            "currentTabUrl": current_tab_url,
        }),
    );
    Value::Object(enriched)
}

fn apply_browser_url_fallback(
    state: &AppState,
    cwd: &Path,
    current_session_id: &str,
    input: Value,
) -> Result<Value> {
    let Some(payload) = input.as_object() else {
        return Ok(input);
    };
    if payload
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|url| !url.trim().is_empty() && !url.eq_ignore_ascii_case("about:blank"))
    {
        return Ok(input);
    }

    let root_session_id = payload
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "current")
        .unwrap_or(current_session_id);
    let tab_id = payload.get("tabId").and_then(Value::as_str).map(str::trim);
    if let Ok(context) = read_current_tab_context(cwd, root_session_id) {
        if matches!(context.status, BrowserCurrentTabStatus::Available) {
            if let Some(url) = context.url.as_deref() {
                if !url.trim().is_empty() && !url.eq_ignore_ascii_case("about:blank") {
                    let mut enriched = payload.clone();
                    enriched.insert("url".to_string(), Value::String(url.trim().to_string()));
                    return Ok(Value::Object(enriched));
                }
            }
        }
    }
    let remembered = tab_id
        .and_then(|tab_id| state.remembered_browser_url(root_session_id, Some(tab_id)))
        .or_else(|| state.remembered_browser_url(root_session_id, None));
    let Some(url) = remembered else {
        return Ok(input);
    };

    let mut enriched = payload.clone();
    enriched.insert("url".to_string(), Value::String(url.to_string()));
    Ok(Value::Object(enriched))
}

fn remember_browser_target(
    state: &mut AppState,
    definition: &puffer_tools::ToolDefinition,
    input: &Value,
) {
    let Some(payload) = browser_permission_value_for_tool_call(&definition.id, input) else {
        return;
    };
    let Some(payload) = payload.as_object() else {
        return;
    };
    let Some(url) = payload.get("url").and_then(Value::as_str) else {
        return;
    };
    let default_session_id = state.browser_root_session_id().to_string();
    let root_session_id = payload
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "current")
        .unwrap_or(default_session_id.as_str());
    let tab_id = payload.get("tabId").and_then(Value::as_str).map(str::trim);
    state.remember_browser_url(root_session_id, tab_id, url);
    state.remember_browser_url(root_session_id, None, url);
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
            let requested = Path::new(path);
            let file_path = if requested.is_absolute() {
                requested.to_path_buf()
            } else {
                cwd.join(requested)
            };
            mapped.insert(
                "file_path".to_string(),
                Value::String(file_path.to_string_lossy().into_owned()),
            );
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

/// Builds a failed tool result for a permission block.
pub(super) fn blocked_runtime_tool(
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

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn in_scope_matches_origin() {
        assert!(url_in_scope("http://target.local:32918/path?q=1", "http://target.local:32918"));
        assert!(url_in_scope("https://target.local:443/", "https://target.local:443"));
    }

    #[test]
    fn out_of_scope_origins_rejected() {
        let scope = "http://target.local:32918";
        assert!(!url_in_scope("http://127.0.0.1:32918/", scope));
        assert!(!url_in_scope("http://169.254.169.254/latest/meta-data/", scope));
        assert!(!url_in_scope("http://target.local:80/", scope), "wrong port");
        assert!(!url_in_scope("https://target.local:32918/", scope), "wrong scheme");
        assert!(!url_in_scope("http://attacker.example/", scope));
    }

    #[test]
    fn dangerous_schemes_always_rejected() {
        let scope = "http://target.local:32918";
        assert!(!url_in_scope("file:///etc/passwd", scope));
        assert!(!url_in_scope("javascript:alert(1)", scope));
        assert!(!url_in_scope("data:text/html,<script>", scope));
    }
}

/// Tools whose inputs carry outbound URLs that must respect pentest scope.
const PENTEST_SCOPED_TOOLS: &[&str] = &["WebFetch", "Browser"];

/// Denies a tool call when an in-scope origin is set and the call's URL points
/// outside it (loopback, link-local, file://, foreign hosts). Bash + nested Agent
/// dispatches are out of scope here — they go through their own filters.
pub(super) fn enforce_pentest_scope(
    state: &AppState,
    tool_id: &str,
    input: &Value,
) -> Option<ToolExecutionResult> {
    let scope = state.pentest_in_scope_origin.as_deref()?;
    let canonical = canonical_tool_name(tool_id);
    if !PENTEST_SCOPED_TOOLS.contains(&canonical.as_str()) {
        return None;
    }
    let url = input.get("url").and_then(Value::as_str)?;
    if url_in_scope(url, scope) {
        return None;
    }
    Some(blocked_runtime_tool(
        tool_id,
        ToolPermissionBehavior::Deny,
        Some(format!(
            "pentest scope: `{url}` is outside `{scope}` — set $PUFFER_PENTEST_TASK_SPEC to widen scope"
        )),
    ))
}

/// Compares a URL's `scheme://host:port` against an expected origin literal.
fn url_in_scope(url: &str, scope_origin: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let scheme = parsed.scheme();
    if scheme == "file" || scheme == "javascript" || scheme == "data" {
        return false;
    }
    let host = match parsed.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return false,
    };
    let port = parsed
        .port_or_known_default()
        .map(|p| format!(":{p}"))
        .unwrap_or_default();
    format!("{scheme}://{host}{port}") == scope_origin
}
