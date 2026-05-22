//! Runtime resolver for structured internal tool permission requests.

use super::local_tools::enrich_browser_permission_input;
use super::permission_prompt::{
    build_permission_prompt_request, prompt_for_permission, BrowserPermissionPromptSource,
    PermissionPromptAction,
};
use crate::permissions::acl::{append_allow_all_rule, append_allow_browser_rule};
use crate::permissions::browser_grants::{suggested_browser_grant_scope, BrowserGrantScopeKind};
use crate::permissions::browser_target::browser_permission_context_for_tool;
use crate::permissions::{
    load_runtime_permission_context_with_inputs, RuntimePermissionInputs, ToolPermissionBehavior,
};
use crate::tool_names::canonical_tool_name;
use crate::AppState;
use puffer_resources::LoadedResources;
use puffer_tools::internal_permissions::{
    InternalToolPermissionRequest, InternalToolPermissionResponse,
};
use puffer_tools::{ToolDefinition, ToolRegistry};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Resolves one structured permission request from a first-party internal tool.
pub(crate) fn resolve_internal_tool_permission(
    state: &mut AppState,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    cwd: &Path,
    request: InternalToolPermissionRequest,
) -> InternalToolPermissionResponse {
    match resolve_internal_tool_permission_result(state, resources, registry, cwd, request) {
        Ok(response) => response,
        Err(error) => InternalToolPermissionResponse::deny(error.to_string()),
    }
}

fn resolve_internal_tool_permission_result(
    state: &mut AppState,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    cwd: &Path,
    request: InternalToolPermissionRequest,
) -> anyhow::Result<InternalToolPermissionResponse> {
    match canonical_tool_name(&request.tool_id).as_str() {
        "browser" => resolve_browser_permission(state, resources, registry, cwd, request.input),
        other => Ok(InternalToolPermissionResponse::deny(format!(
            "unknown internal tool `{other}`"
        ))),
    }
}

fn resolve_browser_permission(
    state: &mut AppState,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    cwd: &Path,
    input: Value,
) -> anyhow::Result<InternalToolPermissionResponse> {
    let Some(definition) = browser_definition(registry) else {
        return Ok(InternalToolPermissionResponse::deny(
            "browser internal tool is not registered",
        ));
    };
    let current_session_id = state.browser_root_session_id();
    let input =
        enrich_browser_permission_input(cwd, &current_session_id, input.clone()).unwrap_or(input);
    let permission_context = load_runtime_permission_context_with_inputs(
        cwd,
        resources,
        state,
        RuntimePermissionInputs::default(),
    )?;
    let decision = permission_context.decision_for_tool_call(definition, &input);
    match decision.behavior {
        ToolPermissionBehavior::Allow => Ok(InternalToolPermissionResponse::allow()),
        ToolPermissionBehavior::Deny => Ok(InternalToolPermissionResponse::deny(
            decision
                .reason
                .unwrap_or_else(|| "browser permission denied".to_string()),
        )),
        ToolPermissionBehavior::Ask => {
            let current_session_id = permission_context
                .effective_profile()
                .current_session_id
                .clone();
            let workspace_roots = permission_context
                .effective_profile()
                .workspace_roots
                .clone();
            prompt_for_browser_permission(
                state,
                cwd,
                definition,
                &input,
                decision.reason.as_deref(),
                &current_session_id,
                &workspace_roots,
            )
        }
    }
}

fn prompt_for_browser_permission(
    state: &mut AppState,
    cwd: &Path,
    definition: &ToolDefinition,
    input: &Value,
    reason: Option<&str>,
    current_session_id: &str,
    workspace_roots: &[PathBuf],
) -> anyhow::Result<InternalToolPermissionResponse> {
    let mut request = build_permission_prompt_request(
        definition,
        input,
        reason,
        current_session_id,
        workspace_roots,
    );
    if let Some(browser) = request.browser.as_mut() {
        browser.source = BrowserPermissionPromptSource::BrowserInternalTool;
    }
    match prompt_for_permission(request) {
        PermissionPromptAction::AllowOnce => Ok(InternalToolPermissionResponse::allow()),
        PermissionPromptAction::AllowSession => {
            grant_browser_session_permission(
                state,
                cwd,
                definition,
                input,
                current_session_id,
                workspace_roots,
            )?;
            Ok(InternalToolPermissionResponse::allow())
        }
        PermissionPromptAction::AllowAllSession => {
            state.allow_browser_permission_for_tool_call(
                definition,
                input,
                browser_grant_scope(input, current_session_id, workspace_roots),
            );
            append_allow_all_rule(cwd)?;
            Ok(InternalToolPermissionResponse::allow())
        }
        PermissionPromptAction::Deny => Ok(InternalToolPermissionResponse::deny(
            "permission denied by user",
        )),
    }
}

fn grant_browser_session_permission(
    state: &mut AppState,
    cwd: &Path,
    definition: &ToolDefinition,
    input: &Value,
    current_session_id: &str,
    workspace_roots: &[PathBuf],
) -> anyhow::Result<()> {
    let scope = browser_grant_scope(input, current_session_id, workspace_roots);
    state.allow_browser_permission_for_tool_call(definition, input, scope);
    let context = browser_permission_context_for_tool(
        &definition.id,
        input,
        current_session_id,
        workspace_roots,
    );
    append_allow_browser_rule(cwd, &context)
}

fn browser_grant_scope(
    input: &Value,
    current_session_id: &str,
    workspace_roots: &[PathBuf],
) -> BrowserGrantScopeKind {
    let context =
        browser_permission_context_for_tool("Browser", input, current_session_id, workspace_roots);
    suggested_browser_grant_scope(&context)
}

fn browser_definition(registry: &ToolRegistry) -> Option<&ToolDefinition> {
    registry
        .internal_definition("Browser")
        .or_else(|| registry.internal_definition("browser"))
}
