use super::auto_approval_review::{
    build_auto_permission_review_request, run_auto_permission_review, AutoPermissionReviewDecision,
};
use super::permission_prompt::{
    prompt_for_permission, PermissionPromptAction, PermissionPromptRequest,
};
use super::tool_executor::blocked_runtime_tool;
use super::RequestToolFilter;
use crate::permissions::acl::{
    append_allow_all_rule, append_allow_path_rule, AclDecision, FilesystemAccessKind,
    FilesystemAclPathKind, ProjectPermissionAcl,
};
use crate::permissions::{
    load_runtime_permission_context_with_inputs, FilesystemPermissionPolicy,
    RuntimePermissionInputs, ToolPermissionBehavior,
};
use crate::AppState;
use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_tools::{ToolDefinition, ToolExecutionResult};
use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Ensures a tool call has access to its concrete filesystem path argument.
pub(super) fn ensure_filesystem_path_access(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    cwd: &Path,
    definition: &ToolDefinition,
    input: &Value,
    tool_filter: Option<&RequestToolFilter>,
    mut policy: FilesystemPermissionPolicy,
    skip_prompt: bool,
) -> Result<std::result::Result<FilesystemPermissionPolicy, ToolExecutionResult>> {
    let Some(request) = filesystem_path_request(cwd, &definition.id, input) else {
        return Ok(Ok(policy));
    };
    if policy.allow_all_paths() {
        return Ok(Ok(policy));
    }
    let acl = ProjectPermissionAcl::load(cwd)?;
    if let Some(decision) = acl.decision_for_path(request.access, &request.path) {
        match decision {
            AclDecision::Allow(_) => {
                policy.workspace_roots.push(request.grant_root);
                return Ok(Ok(policy));
            }
            AclDecision::Deny(reason) => {
                return Ok(Err(blocked_runtime_tool(
                    &definition.id,
                    ToolPermissionBehavior::Deny,
                    Some(reason),
                )));
            }
        }
    }
    if filesystem_policy_allows_path(cwd, &policy, &request.path) {
        return Ok(Ok(policy));
    }
    if skip_prompt {
        policy.workspace_roots.push(request.grant_root);
        return Ok(Ok(policy));
    }

    let prompt_request = PermissionPromptRequest {
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
        browser: None,
        review: None,
    };
    match auto_review_filesystem_request(
        state,
        resources,
        providers,
        auth_store,
        cwd,
        input,
        &prompt_request,
        &request,
        &mut policy,
        tool_filter,
    )? {
        Some(outcome) => return Ok(outcome),
        None => {}
    }

    match prompt_for_permission(prompt_request) {
        PermissionPromptAction::AllowOnce => {
            policy.workspace_roots.push(request.grant_root);
            Ok(Ok(policy))
        }
        PermissionPromptAction::AllowSession => {
            append_allow_path_rule(cwd, request.access, &request.path, request.path_kind)?;
            state.allow_path_for_session(request.grant_root);
            Ok(Ok(runtime_filesystem_policy(
                cwd,
                resources,
                state,
                tool_filter,
            )?))
        }
        PermissionPromptAction::AllowAllSession => {
            append_allow_all_rule(cwd)?;
            state.allow_path_for_session(request.grant_root);
            state.grant_all_tools_for_session();
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

fn auto_review_filesystem_request(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    cwd: &Path,
    input: &Value,
    prompt_request: &PermissionPromptRequest,
    request: &FilesystemPathRequest,
    policy: &mut FilesystemPermissionPolicy,
    tool_filter: Option<&RequestToolFilter>,
) -> Result<Option<std::result::Result<FilesystemPermissionPolicy, ToolExecutionResult>>> {
    let review_input = json!({
        "tool_input": input,
        "path": request.path.display().to_string(),
        "grant_root": request.grant_root.display().to_string(),
        "access": format!("{:?}", request.access),
        "path_kind": format!("{:?}", request.path_kind),
    });
    let current_session_id = state.session.id.to_string();
    let auto_review_request = build_auto_permission_review_request(
        state,
        cwd,
        prompt_request,
        &review_input,
        &current_session_id,
        &policy.workspace_roots,
    );
    let decision = run_auto_permission_review(
        state,
        resources,
        providers,
        auth_store,
        &auto_review_request,
    )
    .decision;
    match decision {
        AutoPermissionReviewDecision::AllowOnce => {
            policy.workspace_roots.push(request.grant_root.clone());
            Ok(Some(Ok(policy.clone())))
        }
        AutoPermissionReviewDecision::AllowSession => {
            append_allow_path_rule(cwd, request.access, &request.path, request.path_kind)?;
            state.allow_path_for_session(request.grant_root.clone());
            Ok(Some(Ok(runtime_filesystem_policy(
                cwd,
                resources,
                state,
                tool_filter,
            )?)))
        }
        AutoPermissionReviewDecision::Deny => Ok(Some(Err(blocked_runtime_tool(
            &prompt_request.tool_id,
            ToolPermissionBehavior::Deny,
            Some("permission denied by user".to_string()),
        )))),
        AutoPermissionReviewDecision::NeedsUser | AutoPermissionReviewDecision::Unavailable => {
            Ok(None)
        }
    }
}

/// Loads the effective filesystem policy for the current runtime permission state.
pub(super) fn runtime_filesystem_policy(
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

struct FilesystemPathRequest {
    path: PathBuf,
    grant_root: PathBuf,
    access: FilesystemAccessKind,
    path_kind: FilesystemAclPathKind,
}

fn filesystem_path_request(
    cwd: &Path,
    tool_id: &str,
    input: &Value,
) -> Option<FilesystemPathRequest> {
    let (field, access) = match tool_id {
        "Read" => ("file_path", FilesystemAccessKind::Read),
        "Write" | "Edit" => ("file_path", FilesystemAccessKind::Write),
        "NotebookEdit" => ("notebook_path", FilesystemAccessKind::Write),
        "Glob" | "Grep" => ("path", FilesystemAccessKind::Read),
        "read_file" | "list_dir" | "search_text" => ("path", FilesystemAccessKind::Read),
        "Agent" => ("cwd", FilesystemAccessKind::Write),
        _ => return None,
    };
    let raw = input.get(field)?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    let path = normalize_permission_path(cwd, raw);
    let grant_root = grant_root_for_path(&path);
    let path_kind = if path.is_dir() {
        FilesystemAclPathKind::Dir
    } else {
        FilesystemAclPathKind::File
    };
    Some(FilesystemPathRequest {
        path,
        grant_root,
        access,
        path_kind,
    })
}

fn filesystem_policy_allows_path(
    cwd: &Path,
    policy: &FilesystemPermissionPolicy,
    path: &Path,
) -> bool {
    if policy.allow_all_paths() {
        return true;
    }
    let normalized_path = normalize_components(path.to_path_buf());
    let canonical_path = canonicalize_existing_prefix(path);
    crate::workspace_paths::workspace_roots(cwd, &policy.workspace_roots)
        .iter()
        .any(|root| {
            let normalized_root = normalize_components(root.clone());
            let canonical_root = canonicalize_existing_prefix(root);
            normalized_path.starts_with(&normalized_root)
                || normalized_path.starts_with(&canonical_root)
                || canonical_path.starts_with(&normalized_root)
                || canonical_path.starts_with(&canonical_root)
        })
}

fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return normalize_components(canonical);
    }
    let mut suffix = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = fs::canonicalize(current) {
            let mut resolved = canonical;
            for component in suffix.iter().rev() {
                resolved.push(component);
            }
            return normalize_components(resolved);
        }
        let Some(parent) = current.parent() else {
            return normalize_components(path.to_path_buf());
        };
        if let Some(name) = current.file_name() {
            suffix.push(name.to_os_string());
        }
        current = parent;
    }
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
