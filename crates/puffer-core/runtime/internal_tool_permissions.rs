//! Runtime resolver for structured internal tool permission and execution requests.

use super::local_tools::enrich_browser_permission_input;
use super::permission_prompt::{
    build_permission_prompt_request, prompt_for_permission, BrowserPermissionPromptSource,
    PermissionPromptAction,
};
use super::tool_executor::PermissionOutcome;
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

/// Snapshot of the three media tool permission decisions resolved before parallel dispatch.
#[derive(Clone, Copy)]
pub(crate) struct MediaPermissionSnapshot {
    pub image: ToolPermissionBehavior,
    pub video: ToolPermissionBehavior,
    pub capabilities: ToolPermissionBehavior,
}

impl MediaPermissionSnapshot {
    /// Promotes each media kind the batch demands to `Allow` when `authorize`
    /// approves it. `authorize(tool_id)` is the injected permission check
    /// (`true` = approved). Pure aside from the injected closure.
    fn authorize_media_batch(&mut self, commands: &[&str], mut authorize: impl FnMut(&str) -> bool) {
        let demand = batch_media_demand(commands, self);
        if demand.is_empty() {
            return;
        }
        if demand.image && authorize("image-generation") {
            self.image = ToolPermissionBehavior::Allow;
        }
        if demand.video && authorize("video-generation") {
            self.video = ToolPermissionBehavior::Allow;
        }
    }
}

/// Snapshot of the subscriber-tool permission decisions resolved before parallel
/// dispatch. Subscriber tools (telegram/email) execute via the process-global
/// `subscription_globals::manager()`, so unlike media they need no borrowed
/// `AppState` data captured into the context — only the pre-resolved gate.
#[derive(Clone, Copy)]
pub(crate) struct SubscriberPermissionSnapshot {
    pub telegram: ToolPermissionBehavior,
    pub email: ToolPermissionBehavior,
}

/// Shared-ref context for executing media + subscriber tools from parallel
/// workers (no &mut AppState).
pub(crate) struct MediaCapabilityContext<'a> {
    pub permissions: MediaPermissionSnapshot,
    pub subscriber: SubscriberPermissionSnapshot,
    pub image_settings: Option<&'a puffer_config::MediaGenerationConfig>,
    pub video_settings: Option<&'a puffer_config::MediaGenerationConfig>,
    pub providers: &'a ProviderRegistry,
    pub auth_store: &'a AuthStore,
    pub discovery_cache: &'a ExactMediaDiscoveryCache,
    pub process_store: Option<
        &'a std::sync::Arc<std::sync::Mutex<crate::runtime::process_store::ProcessStore>>,
    >,
}

/// Owned media-capability data captured from `AppState` before a parallel batch
/// enters `thread::scope`. Borrow it into a [`MediaCapabilityContext`] per call
/// site with [`Self::context`]; the owned value must outlive the scope.
pub(crate) struct MediaCapabilitySnapshot {
    permissions: MediaPermissionSnapshot,
    subscriber: SubscriberPermissionSnapshot,
    image_settings: Option<puffer_config::MediaGenerationConfig>,
    video_settings: Option<puffer_config::MediaGenerationConfig>,
    discovery_cache: ExactMediaDiscoveryCache,
    process_store: std::sync::Arc<std::sync::Mutex<crate::runtime::process_store::ProcessStore>>,
}

impl MediaCapabilitySnapshot {
    /// Captures the media-capability inputs from `state` once. Reads only shared
    /// state (config, discovery cache, process store) plus the permission policy,
    /// so it never needs `&mut AppState`.
    pub(crate) fn capture(
        cwd: &Path,
        resources: &LoadedResources,
        state: &AppState,
        registry: &ToolRegistry,
    ) -> anyhow::Result<Self> {
        let perm_ctx = crate::permissions::load_runtime_permission_context_with_inputs(
            cwd,
            resources,
            state,
            crate::permissions::RuntimePermissionInputs::default(),
        )?;
        Ok(Self {
            permissions: resolve_media_permission_snapshot(&perm_ctx, registry),
            subscriber: resolve_subscriber_permission_snapshot(&perm_ctx, registry),
            image_settings: state.config.media.image.clone(),
            video_settings: state.config.media.video.clone(),
            discovery_cache: state
                .exact_media_discovery_cache
                .clone()
                .unwrap_or_else(ExactMediaDiscoveryCache::empty),
            process_store: state.process_store.clone(),
        })
    }

    /// Borrows this snapshot, together with the shared provider/auth registries,
    /// into a context the parallel workers share.
    pub(crate) fn context<'a>(
        &'a self,
        providers: &'a ProviderRegistry,
        auth_store: &'a AuthStore,
    ) -> MediaCapabilityContext<'a> {
        MediaCapabilityContext {
            permissions: self.permissions,
            subscriber: self.subscriber,
            image_settings: self.image_settings.as_ref(),
            video_settings: self.video_settings.as_ref(),
            providers,
            auth_store,
            discovery_cache: &self.discovery_cache,
            process_store: Some(&self.process_store),
        }
    }

    /// Prompts once per demanded media kind (reusing `resolve_internal_tool_permission`,
    /// which loads the permission context, prompts on `Ask`, and persists the session
    /// grant on "Always allow"), upgrading this batch's snapshot on approval. Runs on the
    /// main thread before `thread::scope`; the `&mut AppState` borrow ends before workers
    /// capture shared refs.
    pub(crate) fn authorize_media_batch(
        &mut self,
        state: &mut AppState,
        resources: &LoadedResources,
        registry: &ToolRegistry,
        cwd: &Path,
        commands: &[&str],
    ) {
        self.permissions.authorize_media_batch(commands, |tool_id| {
            resolve_internal_tool_permission(
                state,
                resources,
                registry,
                cwd,
                InternalToolPermissionRequest {
                    tool_id: tool_id.to_string(),
                    input: Value::Null,
                },
            )
            .is_allowed()
        });
    }
}

/// Runs the parallel-batch media authorization gate for a backend whose tool
/// calls expose their Bash command via `bash_command` (which returns `None` for
/// non-Bash calls). Collects the command of every *permitted* Bash call, then
/// prompts once per demanded-and-`Ask` media kind, upgrading `snapshot` in place.
/// Backend-agnostic: each backend supplies only the per-call command extractor,
/// so the gate flow and the `&mut AppState` borrow contract live in one place.
pub(in crate::runtime) fn authorize_parallel_media_batch<T>(
    snapshot: &mut MediaCapabilitySnapshot,
    state: &mut AppState,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    cwd: &Path,
    tool_calls: &[T],
    permissions: &[PermissionOutcome],
    bash_command: impl Fn(&T) -> Option<String>,
) {
    let commands: Vec<String> = tool_calls
        .iter()
        .zip(permissions)
        .filter(|(_, outcome)| matches!(outcome, PermissionOutcome::Allowed(_)))
        .filter_map(|(call, _)| bash_command(call))
        .collect();
    let command_refs: Vec<&str> = commands.iter().map(String::as_str).collect();
    snapshot.authorize_media_batch(state, resources, registry, cwd, &command_refs);
}

/// Resolves the three media tool permission decisions once, before parallel dispatch.
pub(crate) fn resolve_media_permission_snapshot(
    perm_ctx: &crate::permissions::RuntimePermissionContext,
    registry: &ToolRegistry,
) -> MediaPermissionSnapshot {
    let behavior = |tool_id: &str| {
        registry
            .internal_definition(tool_id)
            .map(|def| {
                perm_ctx
                    .decision_for_tool_call(def, &serde_json::Value::Null)
                    .behavior
            })
            .unwrap_or(ToolPermissionBehavior::Deny)
    };
    MediaPermissionSnapshot {
        image: behavior("image-generation"),
        video: behavior("video-generation"),
        // media-capabilities is always allowed (see resolve_internal_tool_permission_result)
        capabilities: ToolPermissionBehavior::Allow,
    }
}

/// Executes a media internal tool with shared references only (no &mut AppState),
/// for use from parallel workers. A non-media request fails with an instruction to
/// run it as a single command; a media request whose kind was not approved for this
/// batch fails with guidance to approve the prompt (see [`run_media_tool`]).
pub(crate) fn execute_media_internal_tool(
    ctx: &MediaCapabilityContext<'_>,
    cwd: &std::path::Path,
    request: InternalToolExecutionRequest,
) -> InternalToolExecutionResponse {
    use crate::runtime::claude_tools::workflow::{image_generation, media_capabilities, video_generation};
    let canonical = canonical_tool_name(&request.tool_id);
    match canonical.as_str() {
        "imagegeneration" => run_media_tool(ctx.permissions.image, &canonical, || {
            image_generation::execute_image_generation(
                ctx.image_settings,
                cwd,
                request.input,
                Some(image_generation::ImageGenerationMediaContext {
                    providers: ctx.providers,
                    auth_store: ctx.auth_store,
                    discovery_cache: ctx.discovery_cache,
                }),
            )
        }),
        "videogeneration" => run_media_tool(ctx.permissions.video, &canonical, || {
            video_generation::execute_video_generation(
                ctx.video_settings,
                cwd,
                request.input,
                Some(video_generation::VideoGenerationMediaContext {
                    providers: ctx.providers,
                    auth_store: ctx.auth_store,
                    discovery_cache: ctx.discovery_cache,
                }),
            )
        }),
        "mediacapabilities" => run_media_tool(ctx.permissions.capabilities, &canonical, || {
            media_capabilities::execute_media_capabilities(
                ctx.providers,
                ctx.auth_store,
                ctx.discovery_cache,
                request.input,
            )
        }),
        // telegram/email reach the subscriber runtime through the process-global
        // `subscription_globals::manager()`, so they can execute from a parallel
        // worker too — gated on their pre-resolved snapshot.
        _ => execute_subscriber_internal_tool(ctx.subscriber, cwd, request),
    }
}

/// Resolves the subscriber tool permission decisions once, before parallel dispatch.
/// Mirrors [`resolve_media_permission_snapshot`] and reuses the same `perm_ctx`, so
/// it adds no extra permission-file read.
pub(crate) fn resolve_subscriber_permission_snapshot(
    perm_ctx: &crate::permissions::RuntimePermissionContext,
    registry: &ToolRegistry,
) -> SubscriberPermissionSnapshot {
    let behavior = |tool_id: &str| {
        registry
            .internal_definition(tool_id)
            .map(|def| {
                perm_ctx
                    .decision_for_tool_call(def, &serde_json::Value::Null)
                    .behavior
            })
            .unwrap_or(ToolPermissionBehavior::Deny)
    };
    SubscriberPermissionSnapshot {
        telegram: behavior("telegram"),
        email: behavior("email"),
    }
}

/// Executes a subscriber internal tool (telegram/email) with no `&mut AppState`, for
/// use from parallel workers. Each reaches the subscriber runtime through the
/// process-global `subscription_globals::manager()`, so only the pre-resolved
/// permission gate is needed here. A tool not `Allow`ed for this batch fails with
/// guidance to run it as a single command (where the serial path can prompt on
/// `Ask`); a non-subscriber tool fails with the same parallel-batch guidance as media.
pub(crate) fn execute_subscriber_internal_tool(
    permissions: SubscriberPermissionSnapshot,
    cwd: &std::path::Path,
    request: InternalToolExecutionRequest,
) -> InternalToolExecutionResponse {
    use crate::runtime::claude_tools::workflow::{email_configure, telegram_login};
    match canonical_tool_name(&request.tool_id).as_str() {
        "telegram" => run_subscriber_tool(permissions.telegram, "Telegram", || {
            telegram_login::execute_telegram(cwd, request.input)
        }),
        "email" => run_subscriber_tool(permissions.email, "Email", || {
            email_configure::execute_email(cwd, request.input)
        }),
        other => InternalToolExecutionResponse::failure(format!(
            "internal tool `{other}` is not available in a parallel batch; run it as a single command"
        )),
    }
}

/// Runs a subscriber tool only when its pre-resolved permission is `Allow`. When the
/// tool was not approved for this batch, short-circuits with guidance to run it as a
/// single command so the serial path can prompt (mirrors [`run_media_tool`]).
fn run_subscriber_tool(
    behavior: ToolPermissionBehavior,
    label: &str,
    run: impl FnOnce() -> anyhow::Result<String>,
) -> InternalToolExecutionResponse {
    if !matches!(behavior, ToolPermissionBehavior::Allow) {
        return InternalToolExecutionResponse::failure(format!(
            "{label} was not approved for this batch; run it as a single command"
        ));
    }
    internal_tool_execution_response(run())
}

/// Runs a media tool only when its pre-resolved permission is `Allow`, mapping
/// the result (and any diagnostic) into a wire response. When the kind was not
/// approved for this batch, short-circuits with guidance to approve the one-shot
/// prompt (or enable "Always allow").
fn run_media_tool(
    behavior: ToolPermissionBehavior,
    canonical: &str,
    run: impl FnOnce() -> anyhow::Result<String>,
) -> InternalToolExecutionResponse {
    if !matches!(behavior, ToolPermissionBehavior::Allow) {
        let label = match canonical {
            "imagegeneration" => "Image generation",
            "videogeneration" => "Video generation",
            other => other,
        };
        return InternalToolExecutionResponse::failure(format!(
            "{label} was not approved for this batch; \
             approve it once when prompted, or enable Always allow"
        ));
    }
    internal_tool_execution_response(run())
}

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
    internal_tool_execution_response(execute_internal_tool_request_result(
        state,
        resources,
        registry,
        providers,
        auth_store,
        discovery_cache,
        cwd,
        request,
    ))
}

/// Maps an internal media tool result into a wire response, preserving media
/// failure diagnostics. Shared by the serial and parallel execution paths.
fn internal_tool_execution_response(
    result: anyhow::Result<String>,
) -> InternalToolExecutionResponse {
    match result {
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
                state.config.media.image.as_ref(),
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
                state.config.media.video.as_ref(),
                cwd,
                request.input,
                Some(crate::runtime::claude_tools::workflow::video_generation::VideoGenerationMediaContext {
                    providers,
                    auth_store,
                    discovery_cache,
                }),
            );
        }
        "mediacapabilities" => {
            return crate::runtime::claude_tools::workflow::media_capabilities::execute_media_capabilities(
                providers,
                auth_store,
                discovery_cache,
                request.input,
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
        "mediacapabilities" => Ok(InternalToolPermissionResponse::allow()),
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

/// Which media kinds a parallel batch wants to run that are still pending approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct MediaDemand {
    image: bool,
    video: bool,
}

impl MediaDemand {
    fn is_empty(&self) -> bool {
        !self.image && !self.video
    }
}

/// Classifies a batch's Bash commands and returns which media kinds are both
/// invoked and still `Ask` in `perms`. Already-`Allow` kinds need no prompt;
/// `Deny` kinds are never promoted. Reuses the canonical parser so detection
/// matches how generated-media outputs are recognized elsewhere.
fn batch_media_demand(commands: &[&str], perms: &MediaPermissionSnapshot) -> MediaDemand {
    use puffer_media::GeneratedMediaInternalCommandKind as Kind;
    let mut demand = MediaDemand::default();
    for command in commands {
        match puffer_media::generated_media_internal_command_kind(command) {
            Some(Kind::Image) if perms.image == ToolPermissionBehavior::Ask => demand.image = true,
            Some(Kind::Video) if perms.video == ToolPermissionBehavior::Ask => demand.video = true,
            _ => {}
        }
    }
    demand
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use puffer_config::MediaGenerationConfig;
    use puffer_provider_registry::{
        AuthMode, AuthStore, Axis, AxisRole, ControlKind, MediaBatchDescriptor,
        MediaExecutionDescriptor, MediaExecutionKind, MediaKindDescriptor, MediaMap,
        MediaModelDescriptor, MediaOperation, MediaSizeMap, ModelDescriptor, ProviderDescriptor,
        ProviderMediaDescriptor, ProviderRegistry, Variant, Variants, WireType,
    };
    use puffer_resources::{LoadedItem, SourceInfo, SourceKind, ToolSpec};
    use puffer_session_store::SessionMetadata;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::thread;
    use tempfile::tempdir;
    use uuid::Uuid;

    #[test]
    fn telegram_routes_to_handler_in_parallel_batch_when_allowed() {
        // Regression for issue #723: the parallel-batch broker used to bail on every
        // non-media internal tool. An Allowed telegram call must now route to its
        // handler instead of returning the "not available in a parallel batch"
        // guidance. With no subscription runtime in a unit test the handler then
        // fails — but on the subscriber-manager path, proving it routed.
        let dir = tempdir().unwrap();
        let response = execute_subscriber_internal_tool(
            SubscriberPermissionSnapshot {
                telegram: ToolPermissionBehavior::Allow,
                email: ToolPermissionBehavior::Deny,
            },
            dir.path(),
            InternalToolExecutionRequest {
                tool_id: "telegram".to_string(),
                input: json!({ "action": "list_peers" }),
            },
        );
        assert!(!response.success);
        let reason = response.reason.unwrap_or_default();
        assert!(
            !reason.contains("not available in a parallel batch"),
            "allowed telegram should route to its handler, got: {reason}"
        );
        assert!(
            reason.contains("subscription runtime is not running"),
            "expected the subscriber-manager error (proves it routed), got: {reason}"
        );
    }

    #[test]
    fn telegram_gated_in_parallel_batch_when_not_allowed() {
        // A telegram call whose permission is `Ask`/`Deny` for this batch must not
        // execute in parallel; it falls back with guidance to run it as a single
        // command, where the serial path can prompt.
        let dir = tempdir().unwrap();
        let response = execute_subscriber_internal_tool(
            SubscriberPermissionSnapshot {
                telegram: ToolPermissionBehavior::Ask,
                email: ToolPermissionBehavior::Deny,
            },
            dir.path(),
            InternalToolExecutionRequest {
                tool_id: "telegram".to_string(),
                input: json!({ "action": "list_peers" }),
            },
        );
        assert!(!response.success);
        let reason = response.reason.unwrap_or_default();
        assert!(
            reason.contains("was not approved for this batch"),
            "ask/deny telegram should be gated, got: {reason}"
        );
    }

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

    // ── helpers shared by the media_internal_tool_* tests ──────────────────

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut buffer = [0_u8; 8192];
        let size = stream.read(&mut buffer).expect("read request");
        String::from_utf8_lossy(&buffer[..size]).to_string()
    }

    fn spawn_image_generation_server() -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let request_text = read_http_request(&mut stream);
            let body = json!({
                "data": [{"b64_json": "aW1hZ2UtYnl0ZXM="}]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("response");
            request_text
        });
        (format!("http://{address}"), handle)
    }

    fn registry_with_provider(base_url: String) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        registry.register(ProviderDescriptor {
            id: "exact-provider".to_string(),
            display_name: "Exact Provider".to_string(),
            base_url,
            default_api: "openai-responses".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            chat_completions_path: None,
            discovery: None,
            media: Some(ProviderMediaDescriptor {
                image: Some(MediaKindDescriptor {
                    discovery: None,
                    execution: Some(MediaExecutionDescriptor {
                        adapter: MediaExecutionKind::ImagesJson,
                        base_url: None,
                        path: "/custom/images".to_string(),
                        batch: MediaBatchDescriptor::default(),
                        prompt_format: Default::default(),
                    }),
                    models: vec![MediaModelDescriptor {
                        id: "exact-image-model".to_string(),
                        display_name: Some("Exact Image Model".to_string()),
                        max_outputs: Some(9),
                        execution: None,
                        operations: vec![MediaOperation::Generate],
                        axes: vec![
                            Axis {
                                id: "mode".to_string(),
                                label: "Mode".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec!["1K SD".to_string()],
                                    default: "1K SD".to_string(),
                                },
                                request_field: None,
                                wire_type: WireType::String,
                            },
                            Axis {
                                id: "ratio".to_string(),
                                label: "Ratio".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec!["1:1".to_string(), "16:9".to_string()],
                                    default: "1:1".to_string(),
                                },
                                request_field: None,
                                wire_type: WireType::String,
                            },
                        ],
                        variants: Variants::Single(Variant {
                            model_id: "exact-image-model".to_string(),
                            base_params: BTreeMap::from([
                                ("quality".to_string(), "auto".to_string()),
                                ("output_format".to_string(), "png".to_string()),
                            ]),
                        }),
                        media_map: Some(MediaMap {
                            ratio: None,
                            size: Some(MediaSizeMap {
                                field: "size".to_string(),
                                values: BTreeMap::from([(
                                    "1K SD".to_string(),
                                    BTreeMap::from([
                                        ("1:1".to_string(), Some("1024x1024".to_string())),
                                        ("16:9".to_string(), Some("1536x1024".to_string())),
                                    ]),
                                )]),
                            }),
                        }),
                    }],
                }),
                video: None,
            }),
            models: Vec::<ModelDescriptor>::new(),
        });
        registry
    }

    fn auth_store_with_key() -> AuthStore {
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("exact-provider", "sk-test");
        auth_store
    }

    // ── Task-2 TDD tests ───────────────────────────────────────────────────

    #[test]
    fn media_internal_tool_executes_image_when_allowed() {
        let (base_url, _server) = spawn_image_generation_server();
        let dir = tempfile::tempdir().unwrap();
        let providers = registry_with_provider(base_url);
        let auth_store = auth_store_with_key();
        let cache = puffer_media::ExactMediaDiscoveryCache::empty();
        let image_cfg = puffer_config::MediaGenerationConfig {
            provider_id: "exact-provider".to_string(),
            logical_model_id: "exact-image-model".to_string(),
            selections: std::collections::BTreeMap::from([
                ("mode".to_string(), "1K SD".to_string()),
                ("ratio".to_string(), "1:1".to_string()),
                ("output".to_string(), "1".to_string()),
            ]),
        };
        let ctx = MediaCapabilityContext {
            permissions: MediaPermissionSnapshot {
                image: ToolPermissionBehavior::Allow,
                video: ToolPermissionBehavior::Allow,
                capabilities: ToolPermissionBehavior::Allow,
            },
            subscriber: SubscriberPermissionSnapshot {
                telegram: ToolPermissionBehavior::Deny,
                email: ToolPermissionBehavior::Deny,
            },
            image_settings: Some(&image_cfg),
            video_settings: None,
            providers: &providers,
            auth_store: &auth_store,
            discovery_cache: &cache,
            process_store: None,
        };
        let resp = execute_media_internal_tool(
            &ctx,
            dir.path(),
            InternalToolExecutionRequest {
                tool_id: "image-generation".to_string(),
                input: serde_json::json!({"prompt": "draw a ship", "count": 1}),
            },
        );
        assert!(resp.success, "expected success, got {:?}", resp.reason);
    }

    #[test]
    fn media_internal_tool_denies_when_not_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let providers = puffer_provider_registry::ProviderRegistry::new();
        let auth_store = puffer_provider_registry::AuthStore::default();
        let cache = puffer_media::ExactMediaDiscoveryCache::empty();
        let ctx = MediaCapabilityContext {
            permissions: MediaPermissionSnapshot {
                image: ToolPermissionBehavior::Ask,
                video: ToolPermissionBehavior::Allow,
                capabilities: ToolPermissionBehavior::Allow,
            },
            subscriber: SubscriberPermissionSnapshot {
                telegram: ToolPermissionBehavior::Deny,
                email: ToolPermissionBehavior::Deny,
            },
            image_settings: None,
            video_settings: None,
            providers: &providers,
            auth_store: &auth_store,
            discovery_cache: &cache,
            process_store: None,
        };
        let resp = execute_media_internal_tool(
            &ctx,
            dir.path(),
            InternalToolExecutionRequest {
                tool_id: "image-generation".to_string(),
                input: serde_json::json!({"prompt": "x"}),
            },
        );
        assert!(!resp.success);
        assert!(resp.reason.unwrap().contains("not approved for this batch"));
    }

    #[test]
    fn media_internal_tool_rejects_non_media_tool() {
        let dir = tempfile::tempdir().unwrap();
        let providers = puffer_provider_registry::ProviderRegistry::new();
        let auth_store = puffer_provider_registry::AuthStore::default();
        let cache = puffer_media::ExactMediaDiscoveryCache::empty();
        let ctx = MediaCapabilityContext {
            permissions: MediaPermissionSnapshot {
                image: ToolPermissionBehavior::Allow,
                video: ToolPermissionBehavior::Allow,
                capabilities: ToolPermissionBehavior::Allow,
            },
            subscriber: SubscriberPermissionSnapshot {
                telegram: ToolPermissionBehavior::Deny,
                email: ToolPermissionBehavior::Deny,
            },
            image_settings: None,
            video_settings: None,
            providers: &providers,
            auth_store: &auth_store,
            discovery_cache: &cache,
            process_store: None,
        };
        let resp = execute_media_internal_tool(
            &ctx,
            dir.path(),
            InternalToolExecutionRequest {
                // A tool that is neither media nor a subscriber tool (telegram/email)
                // still falls through to the catch-all guidance.
                tool_id: "totally-unknown-internal-tool".to_string(),
                input: serde_json::json!({}),
            },
        );
        assert!(!resp.success);
        assert!(resp.reason.unwrap().contains("not available in a parallel batch"));
    }

    fn snapshot_all_ask() -> MediaPermissionSnapshot {
        MediaPermissionSnapshot {
            image: ToolPermissionBehavior::Ask,
            video: ToolPermissionBehavior::Ask,
            capabilities: ToolPermissionBehavior::Allow,
        }
    }

    #[test]
    fn demand_detects_image_only() {
        let d = batch_media_demand(&["imagegen '{\"prompt\":\"x\"}'"], &snapshot_all_ask());
        assert!(d.image && !d.video);
        assert!(!d.is_empty());
    }

    #[test]
    fn demand_detects_video_only() {
        let d = batch_media_demand(&["videogen '{\"prompt\":\"x\"}'"], &snapshot_all_ask());
        assert!(d.video && !d.image);
    }

    #[test]
    fn demand_detects_both_kinds_in_batch() {
        let d = batch_media_demand(&["imagegen '{}'", "videogen '{}'"], &snapshot_all_ask());
        assert!(d.image && d.video);
    }

    #[test]
    fn demand_empty_for_non_media_batch() {
        let d = batch_media_demand(&["ls -la", "grep foo bar.txt"], &snapshot_all_ask());
        assert!(d.is_empty());
    }

    #[test]
    fn demand_excludes_already_allowed_kind() {
        let mut perms = snapshot_all_ask();
        perms.image = ToolPermissionBehavior::Allow;
        let d = batch_media_demand(&["imagegen '{}'"], &perms);
        assert!(!d.image, "already-Allow image needs no prompt");
    }

    #[test]
    fn demand_excludes_denied_kind() {
        let mut perms = snapshot_all_ask();
        perms.video = ToolPermissionBehavior::Deny;
        let d = batch_media_demand(&["videogen '{}'"], &perms);
        assert!(!d.video, "never prompt to override an explicit Deny");
    }

    #[test]
    fn demand_ignores_compound_media_command() {
        // generated_media_internal_command_kind rejects unquoted ; | & — no classification.
        let d = batch_media_demand(&["imagegen '{}' ; imagegen '{}'"], &snapshot_all_ask());
        assert!(d.is_empty());
    }

    #[test]
    fn gate_upgrades_demanded_kind_when_authorized() {
        let mut perms = snapshot_all_ask();
        perms.authorize_media_batch(&["imagegen '{}'"], |_| true);
        assert_eq!(perms.image, ToolPermissionBehavior::Allow);
        assert_eq!(perms.video, ToolPermissionBehavior::Ask, "video not demanded → unchanged");
    }

    #[test]
    fn gate_leaves_kind_when_denied() {
        let mut perms = snapshot_all_ask();
        perms.authorize_media_batch(&["imagegen '{}'"], |_| false);
        assert_eq!(perms.image, ToolPermissionBehavior::Ask);
    }

    #[test]
    fn gate_no_call_when_no_demand() {
        let mut perms = snapshot_all_ask();
        let mut calls = 0;
        perms.authorize_media_batch(&["ls"], |_| {
            calls += 1;
            true
        });
        assert_eq!(calls, 0, "authorize never invoked without demand");
        assert_eq!(perms.image, ToolPermissionBehavior::Ask);
    }

    #[test]
    fn gate_upgrades_only_authorized_kind_in_mixed_batch() {
        let mut perms = snapshot_all_ask();
        perms.authorize_media_batch(&["imagegen '{}'", "videogen '{}'"], |tool_id| {
            tool_id == "image-generation"
        });
        assert_eq!(perms.image, ToolPermissionBehavior::Allow);
        assert_eq!(perms.video, ToolPermissionBehavior::Ask);
    }
}
