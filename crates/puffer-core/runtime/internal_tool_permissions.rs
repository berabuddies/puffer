//! Runtime resolver for structured internal tool permission and execution requests.

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
use puffer_media::ExactMediaDiscoveryCache;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_tools::internal_permissions::{
    InternalToolExecutionRequest, InternalToolExecutionResponse, InternalToolPermissionRequest,
    InternalToolPermissionResponse,
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

/// Executes one first-party internal tool request inside the parent runtime.
pub(crate) fn execute_internal_tool_request(
    state: &mut AppState,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    discovery_cache: &ExactMediaDiscoveryCache,
    cwd: &Path,
    request: InternalToolExecutionRequest,
) -> InternalToolExecutionResponse {
    match execute_internal_tool_request_result(
        state,
        resources,
        registry,
        providers,
        auth_store,
        discovery_cache,
        cwd,
        request,
    ) {
        Ok(output) => InternalToolExecutionResponse::success(output),
        Err(error) => {
            let reason = format!("{error:#}");
            if let Some(diagnostic) = puffer_media::media_failure_diagnostic(&error) {
                let fallback_reason = reason.clone();
                let value = serde_json::to_value(diagnostic)
                    .unwrap_or_else(|_| serde_json::json!({ "error": fallback_reason }));
                InternalToolExecutionResponse::failure_with_diagnostic(reason, value)
            } else {
                InternalToolExecutionResponse::failure(reason)
            }
        }
    }
}

fn execute_internal_tool_request_result(
    state: &mut AppState,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    discovery_cache: &ExactMediaDiscoveryCache,
    cwd: &Path,
    request: InternalToolExecutionRequest,
) -> anyhow::Result<String> {
    let permission = resolve_internal_tool_permission(
        state,
        resources,
        registry,
        cwd,
        InternalToolPermissionRequest {
            tool_id: request.tool_id.clone(),
            input: redacted_internal_permission_input(&request.tool_id, request.input.clone()),
        },
    );
    if !permission.is_allowed() {
        anyhow::bail!(
            "{} denied: {}",
            request.tool_id,
            permission
                .reason
                .unwrap_or_else(|| "permission denied".to_string())
        );
    }
    let workflow_tool = match canonical_tool_name(&request.tool_id).as_str() {
        "email" => "Email",
        "requestuserbrowseraction" => "requestuserbrowseraction",
        "telegram" => "Telegram",
        "imagegeneration" => {
            return crate::runtime::claude_tools::workflow::image_generation::execute_image_generation(
                state,
                cwd,
                request.input,
                Some(crate::runtime::claude_tools::workflow::image_generation::ImageGenerationMediaContext {
                    providers,
                    auth_store,
                    discovery_cache,
                }),
            );
        }
        "videogeneration" => {
            return crate::runtime::claude_tools::workflow::video_generation::execute_video_generation(
                state,
                cwd,
                request.input,
                Some(crate::runtime::claude_tools::workflow::video_generation::VideoGenerationMediaContext {
                    providers,
                    auth_store,
                    discovery_cache,
                }),
            );
        }
        other => anyhow::bail!("unknown internal executable tool `{other}`"),
    };
    crate::runtime::claude_tools::execute_workflow_tool(
        state,
        resources,
        cwd,
        workflow_tool,
        request.input,
        None,
    )
}

fn redacted_internal_permission_input(tool_id: &str, mut input: Value) -> Value {
    match canonical_tool_name(tool_id).as_str() {
        "email" => {
            if let Some(object) = input.as_object_mut() {
                if object.contains_key("password") {
                    object.insert(
                        "password".to_string(),
                        Value::String("<redacted>".to_string()),
                    );
                }
            }
        }
        "telegram" => {
            if let Some(object) = input.as_object_mut() {
                match object.get("action").and_then(Value::as_str) {
                    Some("import_desktop") => {
                        if object.get("passcode").is_some_and(|value| !value.is_null()) {
                            object.insert(
                                "passcode".to_string(),
                                Value::String("<redacted>".to_string()),
                            );
                        }
                    }
                    Some("login_submit_code") => {
                        object.insert("code".to_string(), Value::String("<redacted>".to_string()));
                    }
                    Some("login_submit_password") => {
                        object.insert(
                            "password".to_string(),
                            Value::String("<redacted>".to_string()),
                        );
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    input
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
        "email"
        | "imagegeneration"
        | "requestuserbrowseraction"
        | "telegram"
        | "videogeneration" => {
            resolve_generic_internal_permission(state, resources, registry, cwd, request)
        }
        other => Ok(InternalToolPermissionResponse::deny(format!(
            "unknown internal tool `{other}`"
        ))),
    }
}

fn resolve_generic_internal_permission(
    state: &mut AppState,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    cwd: &Path,
    request: InternalToolPermissionRequest,
) -> anyhow::Result<InternalToolPermissionResponse> {
    let Some(definition) = registry.internal_definition(&request.tool_id) else {
        return Ok(InternalToolPermissionResponse::deny(format!(
            "{} internal tool is not registered",
            request.tool_id
        )));
    };
    let permission_context = load_runtime_permission_context_with_inputs(
        cwd,
        resources,
        state,
        RuntimePermissionInputs::default(),
    )?;
    let decision = permission_context.decision_for_tool_call(definition, &request.input);
    match decision.behavior {
        ToolPermissionBehavior::Allow => Ok(InternalToolPermissionResponse::allow()),
        ToolPermissionBehavior::Deny => Ok(InternalToolPermissionResponse::deny(
            decision.reason.unwrap_or_else(|| {
                format!("{} permission denied", definition.id.to_ascii_lowercase())
            }),
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
            prompt_for_generic_internal_permission(
                state,
                cwd,
                definition,
                &request.input,
                decision.reason.as_deref(),
                &current_session_id,
                &workspace_roots,
            )
        }
    }
}

fn prompt_for_generic_internal_permission(
    state: &mut AppState,
    cwd: &Path,
    definition: &ToolDefinition,
    input: &Value,
    reason: Option<&str>,
    current_session_id: &str,
    workspace_roots: &[PathBuf],
) -> anyhow::Result<InternalToolPermissionResponse> {
    let request = build_permission_prompt_request(
        definition,
        input,
        reason,
        current_session_id,
        workspace_roots,
    );
    match prompt_for_permission(request) {
        PermissionPromptAction::AllowOnce => Ok(InternalToolPermissionResponse::allow()),
        PermissionPromptAction::AllowSession => {
            state.allow_permission_for_tool_call(definition, input);
            Ok(InternalToolPermissionResponse::allow())
        }
        PermissionPromptAction::AllowAllSession => {
            state.grant_all_tools_for_session();
            append_allow_all_rule(cwd)?;
            Ok(InternalToolPermissionResponse::allow())
        }
        PermissionPromptAction::Deny => Ok(InternalToolPermissionResponse::deny(
            "permission denied by user",
        )),
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

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::MediaGenerationConfig;
    use puffer_provider_registry::{
        AuthMode, AuthStore, Axis, AxisRole, ControlKind, MediaExecutionDescriptor,
        MediaExecutionKind, MediaKindDescriptor, MediaModelDescriptor, MediaOperation,
        ModelDescriptor, ProviderDescriptor, ProviderMediaDescriptor, ProviderRegistry, Variant,
        Variants, WireType,
    };
    use puffer_resources::{LoadedItem, SourceInfo, SourceKind, ToolSpec};
    use puffer_session_store::SessionMetadata;
    use serde_json::json;
    use std::collections::BTreeMap;
    use tempfile::tempdir;
    use uuid::Uuid;

    #[test]
    fn image_internal_execution_receives_media_context() {
        let dir = tempdir().unwrap();
        let resources = media_internal_resources();
        let registry = ToolRegistry::from_resources(&resources);
        let providers = image_provider_registry();
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("exact-provider", "sk-test");
        let discovery_cache = puffer_media::ExactMediaDiscoveryCache::empty();
        let mut state = media_state(dir.path());

        let response = execute_internal_tool_request(
            &mut state,
            &resources,
            &registry,
            &providers,
            &auth_store,
            &discovery_cache,
            dir.path(),
            InternalToolExecutionRequest {
                tool_id: "image-generation".to_string(),
                input: json!({"prompt": "draw a ship", "count": 1}),
            },
        );

        assert!(!response.success);
        let reason = response.reason.unwrap_or_default();
        assert!(
            reason.contains("stale-image-model"),
            "expected the stale model to be rejected, got: {reason}"
        );
        assert!(!reason.contains("media runtime is not configured"));
        assert!(!reason.contains("unknown internal tool"));
    }

    fn media_state(cwd: &Path) -> AppState {
        let mut config = puffer_config::PufferConfig::default();
        config.media.image = Some(MediaGenerationConfig {
            provider_id: "exact-provider".to_string(),
            logical_model_id: "stale-image-model".to_string(),
            selections: BTreeMap::from([
                ("size".to_string(), "1024x1024".to_string()),
                ("quality".to_string(), "auto".to_string()),
                ("output_format".to_string(), "png".to_string()),
            ]),
        });
        AppState::new(
            config,
            cwd.to_path_buf(),
            SessionMetadata {
                id: Uuid::new_v4(),
                display_name: None,
                generated_title: None,
                cwd: cwd.to_path_buf(),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        )
    }

    fn media_internal_resources() -> LoadedResources {
        LoadedResources {
            internal_tools: vec![LoadedItem {
                value: ToolSpec {
                    id: "ImageGeneration".to_string(),
                    name: "ImageGeneration".to_string(),
                    description: "Generate images".to_string(),
                    handler: "runtime:workflow:image_generation".to_string(),
                    aliases: vec!["image-generation".to_string(), "imagegen".to_string()],
                    approval_policy: Some("auto".to_string()),
                    sandbox_policy: Some("network".to_string()),
                    ..ToolSpec::default()
                },
                source_info: SourceInfo {
                    path: PathBuf::from("internal_tools/image_generation.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        }
    }

    fn image_provider_registry() -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        registry.register(ProviderDescriptor {
            id: "exact-provider".to_string(),
            display_name: "Exact Provider".to_string(),
            base_url: "http://127.0.0.1:9".to_string(),
            default_api: "openai-responses".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: Default::default(),
            query_params: Default::default(),
            chat_completions_path: None,
            discovery: None,
            media: Some(ProviderMediaDescriptor {
                image: Some(MediaKindDescriptor {
                    discovery: None,
                    execution: Some(MediaExecutionDescriptor {
                        adapter: MediaExecutionKind::ImagesJson,
                        base_url: None,
                        path: "/custom/images".to_string(),
                        batch: Default::default(),
                        prompt_format: Default::default(),
                    }),
                    models: vec![MediaModelDescriptor {
                        id: "exact-image-model".to_string(),
                        display_name: Some("Exact Image Model".to_string()),
                        max_outputs: None,
                        execution: None,
                        operations: vec![MediaOperation::Generate],
                        axes: vec![
                            Axis {
                                id: "size".to_string(),
                                label: "Size".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec!["1024x1024".to_string()],
                                    default: "1024x1024".to_string(),
                                },
                                request_field: Some("size".to_string()),
                                wire_type: WireType::String,
                            },
                            Axis {
                                id: "quality".to_string(),
                                label: "Quality".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec!["auto".to_string()],
                                    default: "auto".to_string(),
                                },
                                request_field: Some("quality".to_string()),
                                wire_type: WireType::String,
                            },
                            Axis {
                                id: "output_format".to_string(),
                                label: "Output format".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec!["png".to_string()],
                                    default: "png".to_string(),
                                },
                                request_field: Some("output_format".to_string()),
                                wire_type: WireType::String,
                            },
                        ],
                        variants: Variants::Single(Variant {
                            model_id: "exact-image-model".to_string(),
                            base_params: ::std::collections::BTreeMap::new(),
                        }),
                        media_map: None,
                    }],
                }),
                video: None,
            }),
            models: Vec::<ModelDescriptor>::new(),
        });
        registry
    }
}
