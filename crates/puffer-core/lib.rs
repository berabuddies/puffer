mod agent_catalog;
mod autodream;
mod command;
mod command_helpers;
mod command_summary;
mod config_settings;
mod hooks;
mod memory;
mod model_preferences;
mod network;
mod permissions;
mod plan_mode;
mod plans;
pub mod recap;
pub mod runner_adapter;
pub mod runner_mcp;
mod runtime;
mod skill_support;
mod state;
#[cfg(test)]
pub(crate) mod test_locks;
mod tool_names;
pub mod ultrareview;
pub mod user_memory;
mod workspace_paths;

pub use agent_catalog::{load_agent_catalog, AgentCatalogEntry};
pub use autodream::{
    autodream_status, autodream_status_with_store, autodream_suggestions_with_store,
    autodream_turn_completed, autodream_turn_completed_with_store,
    ensure_manual_autodream_project_memory, render_manual_autodream_result, run_autodream_review,
    should_show_manual_autodream_genskill_suggestion, spawn_autodream_review,
    spawn_autodream_review_with_store, visible_autodream_assistant_text, AutoDreamOutcome,
    ManualAutoDreamBootstrap,
};
pub use command::{
    command_surface, dispatch_command, find_command, supported_commands, CommandKind, CommandSpec,
};
pub use command_helpers::append_trace_events;
pub use command_helpers::execute_connect_flow;
pub use command_helpers::execute_monitor_flow;
pub use command_helpers::CommandActionEntry;
pub use command_helpers::CopyActionEntry;
pub use command_helpers::LambdaSkillStatus;
pub use command_helpers::McpActionEntry;
pub use command_helpers::PluginActionEntry;
pub use command_helpers::ResumeLaunchResolution;
pub use command_helpers::SessionOverlayView;
pub use command_helpers::TaskActionEntry;
pub use command_helpers::{
    prepare_pentest_command, PentestCommand, PentestStart, DEFAULT_PENTEST_MAX_DISPATCHES_PER_ITER,
    DEFAULT_PENTEST_MAX_ITERATIONS, PENTEST_USAGE,
};
pub use command_summary::render_cost_summary;
pub(crate) use command_summary::{render_buddy_summary, render_usage_summary};
pub use hooks::run_resource_hooks;
pub use memory::{
    activate_project_memory, flush_project_memory, project_memory_turn_completed,
    spawn_project_memory_review,
};
pub use model_preferences::{
    default_effort_level, effort_level_is_supported, normalized_effort_level,
    provider_preference_family, supported_effort_levels, ModelPreferenceFamily,
};
pub use network::{blocking_client_for_url, test_proxy_endpoint, HttpPurpose, ProxyTestOutcome};
pub use permissions::browser_action_set_for_action;
pub use permissions::is_browser_tool_selector;
pub use permissions::BrowserActionSet;
pub use permissions::SessionPermissionState;
pub use plan_mode::enter_plan_mode;
pub use runtime::background_tasks;
pub use runtime::claude_tools::execute_workflow_tool;
pub use runtime::execute_tool_action_once;
pub use runtime::execute_user_prompt as execute_user_turn;
pub use runtime::install_subscription_manager;
pub use runtime::lambda_gate::LambdaHostConcreteToolBinding;
pub use runtime::mcp_discovery;
pub use runtime::quota::{QuotaError, QuotaErrorKind, QUOTA_EXIT_CODE};
pub use runtime::resource_watcher;
pub use runtime::resource_watcher::ResourceWatcher;
pub use runtime::subscription_manager;
pub use runtime::teammate_loop;
pub use runtime::{
    browser_auto_review_runtime_result_from_json, execute_side_question,
    execute_user_prompt_streaming as execute_user_turn_streaming,
    execute_user_prompt_streaming_with_cancel as execute_user_turn_streaming_with_cancel,
    execute_user_prompt_streaming_with_permissions as execute_user_turn_streaming_with_permissions,
    execute_user_prompt_streaming_with_permissions_and_cancel as execute_user_turn_streaming_with_permissions_and_cancel,
    execute_user_prompt_streaming_with_prompt_tools_and_cancel as execute_user_turn_streaming_with_prompt_tools_and_cancel,
    execute_user_prompt_streaming_with_reflection as execute_user_turn_streaming_with_reflection,
    execute_user_prompt_streaming_with_structured_output as execute_user_turn_streaming_with_structured_output,
    execute_user_prompt_streaming_without_tools as execute_user_turn_streaming_without_tools,
    execute_user_prompt_with_structured_output as execute_user_turn_with_structured_output,
    execute_user_prompt_without_tools as execute_user_turn_without_tools, runtime_work_active,
    shutdown_runtime_services, with_permission_prompt_handler, with_user_question_prompt_handler,
    BrowserAutoReviewActionSet, BrowserAutoReviewRawAction, BrowserAutoReviewRequest,
    BrowserAutoReviewRuntimeResult, BrowserAutoReviewSessionTargeting, BrowserAutoReviewSource,
    BrowserAutoReviewSuggestedGrantScope, BrowserAutoReviewTargetClass, BrowserAutoReviewUrlSource,
    BrowserPermissionPromptActionSet, BrowserPermissionPromptPayload,
    BrowserPermissionPromptSource, BrowserPermissionPromptTargetClass, CancelToken,
    CodeJudgeConfig, LlmJudgeConfig, LlmJudgeContextScope, LlmJudgeMode, LlmJudgePromptCacheMode,
    PermissionPromptAction, PermissionPromptRequest, PermissionPromptReviewPayload,
    ReflectionConfig, ReflectionLanguage, ReflectionTraceEvent, RetryAttemptKind,
    StructuredOutputConfig, ToolCallRequest, ToolInvocation, TurnExecution, TurnStreamEvent,
    TurnUsageReport, UserQuestionPromptRequest, UserQuestionPromptResponse,
};
pub use runtime::{install_observability, observability_handle};
pub use state::{AppState, MessageRole, RenderedMessage, TaskRecord, TaskStatus};

use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, SessionSummary};
use std::path::Path;

/// Executes a standalone LSP query against the configured workspace language servers.
pub fn execute_lsp_query(
    resources: &LoadedResources,
    cwd: &Path,
    input: serde_json::Value,
) -> Result<String> {
    runtime::claude_tools::workflow::lsp::execute_lsp_query(resources, cwd, input)
}

/// Renders the current session/provider/tool status summary used by `/status`.
pub fn render_status_summary(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> String {
    command_summary::render_status_summary(state, resources, providers, auth_store)
}

/// Renders the current `/config` summary used by interactive overlays.
pub fn render_config_summary(state: &AppState) -> Result<String> {
    command_helpers::render_config_summary(state)
}

/// Renders the current `/doctor` diagnostics report used by interactive overlays.
pub fn render_doctor_report(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> Result<String> {
    command_helpers::render_doctor_report(state, resources, providers, auth_store)
}

/// Renders Lambda Skill harness status lines used by doctor surfaces.
pub fn render_lambda_skill_doctor_status(resources: &LoadedResources) -> String {
    command_helpers::render_lambda_skill_doctor_status(resources)
}

/// Returns Lambda Skill harness warning summaries used by doctor surfaces.
pub fn lambda_skill_doctor_warning_lines(resources: &LoadedResources) -> Vec<String> {
    command_helpers::lambda_skill_doctor_warnings(resources)
        .into_iter()
        .map(|warning| format!("{}; {}", warning.summary, warning.detail))
        .collect()
}

/// Returns Lambda Skill readiness rows used by UI surfaces.
pub fn lambda_skill_statuses(resources: &LoadedResources) -> Vec<LambdaSkillStatus> {
    command_helpers::lambda_skill_statuses(resources)
}

/// Validates a precompiled Lambda Skill host catalogue for runtime use.
pub fn validate_lambda_host_catalogue_runtime(raw: &str) -> Result<()> {
    runtime::lambda_gate::validate_host_catalogue_runtime(raw)
}

/// Returns the concrete tool bindings declared by a Lambda Skill host catalogue.
pub fn lambda_host_catalogue_concrete_tool_bindings(
    raw: &str,
) -> Result<Vec<LambdaHostConcreteToolBinding>> {
    runtime::lambda_gate::host_catalogue_concrete_tool_bindings(raw)
}

/// Renders the current `/context` summary used by interactive overlays.
pub fn render_context_panel(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
) -> Result<String> {
    runtime::render_context_usage_summary(state, resources, providers)
}

/// Renders the full raw context that would be sent to the model.
pub fn render_debug_context(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
) -> Result<String> {
    runtime::render_debug_context(state, resources, providers)
}

/// Returns the number of background shell tasks currently running.
pub fn running_background_task_count(state: &AppState) -> usize {
    runtime::claude_tools::workflow::running_shell_task_count(&state.cwd, &state.session.id)
}

/// Drains background task completion notifications (clears after reading).
pub fn drain_background_task_completions(state: &AppState) -> Vec<String> {
    runtime::claude_tools::workflow::drain_completed_shell_tasks(&state.cwd, &state.session.id)
}

/// Estimates the remaining context-window percentage for the active model.
///
/// When `state.last_input_tokens` is available (populated from `usage.input_tokens`
/// in API responses), uses that for an accurate calculation. Falls back to a
/// heuristic estimation from rendered transcript text.
pub fn estimate_remaining_context_percent(state: &AppState, providers: &ProviderRegistry) -> u32 {
    let context_window = state
        .current_model
        .as_deref()
        .and_then(|selector| providers.resolve_model(selector))
        .map(|model| model.context_window)
        .unwrap_or(0);
    if context_window == 0 {
        return 0;
    }

    let used_tokens = if let Some(api_tokens) = state.last_input_tokens {
        api_tokens
    } else {
        // Fallback: estimate from rendered transcript (inaccurate — misses system
        // prompt, tool definitions, function call/output items).
        state
            .transcript
            .iter()
            .map(|message| estimate_footer_tokens(&message.text).saturating_add(4))
            .sum::<u32>()
    };
    let remaining_tokens = context_window.saturating_sub(used_tokens);
    remaining_tokens.saturating_mul(100) / context_window
}

/// Renders the current `/permissions` summary used by interactive overlays.
pub fn render_permissions_panel(state: &AppState, resources: &LoadedResources) -> Result<String> {
    command_helpers::render_permissions_panel(state, resources)
}

/// Renders the current `/hooks` summary used by interactive overlays.
pub fn render_hooks_summary(state: &AppState, resources: &LoadedResources) -> Result<String> {
    command_helpers::render_hooks_summary(state, resources)
}

/// Builds the interactive `/hooks` action list used by the TUI picker.
pub fn render_hooks_actions(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Vec<CommandActionEntry>> {
    command_helpers::render_hooks_actions(state, resources)
}

/// Builds Claude-style interactive `/copy` picker entries for the current assistant response.
pub fn render_copy_actions(state: &AppState, args: &str) -> Result<Option<Vec<CopyActionEntry>>> {
    command_helpers::render_copy_actions(state, args)
}

/// Renders the grouped `/skills` summary used by the interactive overlay.
pub fn render_skills_panel(resources: &LoadedResources) -> String {
    command_helpers::render_skills_panel(resources)
}

/// Renders the current `/mcp` summary used by interactive overlays.
pub fn render_mcp_summary(state: &AppState, resources: &LoadedResources) -> Result<String> {
    command_helpers::render_mcp_summary(state, resources)
}

/// Builds the interactive `/mcp` action list used by the TUI picker.
pub fn render_mcp_actions(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Vec<McpActionEntry>> {
    command_helpers::render_mcp_actions(state, resources)
}

/// Builds the interactive `/ide` action list used by the TUI picker.
pub fn render_ide_actions(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Vec<CommandActionEntry>> {
    command_helpers::render_ide_actions(state, resources)
}

/// Renders the current `/plugin` summary used by interactive overlays.
pub fn render_plugin_summary(state: &AppState, resources: &LoadedResources) -> Result<String> {
    command_helpers::render_plugin_summary(state, resources)
}

/// Builds the interactive `/plugin` action list used by the TUI picker.
pub fn render_plugin_actions(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Vec<PluginActionEntry>> {
    command_helpers::render_plugin_actions(state, resources)
}

/// Builds the interactive `/sandbox` action list used by the TUI picker.
pub fn render_sandbox_actions(state: &AppState) -> Result<Vec<CommandActionEntry>> {
    command_helpers::render_sandbox_actions(state)
}

/// Builds the interactive `/tasks` action list used by the TUI picker.
pub fn render_task_actions(state: &mut AppState) -> Result<Vec<TaskActionEntry>> {
    command_helpers::render_task_actions(state)
}

/// Renders read-only `/tasks` subcommands for inline TUI overlays.
pub fn render_tasks_panel_text(state: &mut AppState, args: &str) -> Result<Option<String>> {
    command_helpers::render_tasks_panel_text(state, args)
}

/// Renders the current `/memory` summary used by interactive overlays.
pub fn render_memory_panel(state: &AppState) -> String {
    command_helpers::render_memory_panel(state)
}

/// Renders the current `/session` summary used by interactive overlays.
pub fn render_session_panel(state: &AppState) -> String {
    command_helpers::render_session_panel(state)
}

/// Builds the dedicated `/session` overlay view used by the interactive TUI.
pub fn render_session_overlay(state: &AppState) -> SessionOverlayView {
    command_helpers::render_session_overlay(state)
}

fn estimate_footer_tokens(text: &str) -> u32 {
    // CJK-aware: ASCII ~0.25 tokens/char, non-ASCII ~1.5 tokens/char.
    let mut units = 0u32;
    for ch in text.chars() {
        if ch.is_ascii() {
            units += 1;
        } else {
            units += 6;
        }
    }
    (units + 3) / 4
}

/// Applies a provider/model/effort/fast-mode selection and persists it to user config.
pub fn apply_model_preferences(
    state: &mut AppState,
    provider_id: &str,
    model_id: &str,
    effort: &str,
    fast_mode: bool,
) -> Result<()> {
    command_helpers::apply_model_preferences(state, provider_id, model_id, effort, fast_mode)
}

/// Removes stored credentials for one provider and clears the active selection when it matches.
pub fn logout_provider_credentials(
    state: &mut AppState,
    auth_store: &mut AuthStore,
    provider_id: &str,
) -> Result<String> {
    command_helpers::remove_provider_credentials(state, auth_store, provider_id)
}

/// Executes the interactive login flow for one provider and reloads stored credentials.
pub fn login_provider_credentials(
    state: &AppState,
    auth_store: &mut AuthStore,
    provider_id: &str,
) -> Result<String> {
    command_helpers::run_provider_login_flow(state, auth_store, provider_id)
}

/// Lists the sessions that should appear in the current `/resume` picker.
pub fn resumable_sessions_for_picker(
    session_store: &SessionStore,
    current_session_id: uuid::Uuid,
    current_cwd: &Path,
) -> Result<Vec<SessionSummary>> {
    command_helpers::resumable_sessions_for_picker(session_store, current_session_id, current_cwd)
}

/// Resolves a startup `--resume` request into a direct session or picker state.
pub fn resolve_resume_launch(
    session_store: &SessionStore,
    current_cwd: &Path,
    query: Option<&str>,
) -> Result<ResumeLaunchResolution> {
    command_helpers::resolve_resume_launch(session_store, current_cwd, query)
}

/// Reloads declarative resources, rebuilds the provider registry, and
/// hot-swaps the MCP roster on the live tool runner for the active session.
///
/// Skills are picked up implicitly because the system prompt is rebuilt
/// from `resources.skills` on every turn; replacing `*resources` is enough.
/// MCP servers need an extra step: the in-memory `McpHost` cached inside
/// `LocalToolRunner` keeps live subprocess handles indexed by the old
/// roster, so we hot-swap it via [`runner_adapter::LocalToolRunner::replace_mcp_roster`]
/// so newly added manifests can launch on the next call and removed ones
/// stop being callable immediately.
pub fn reload_runtime_resources(
    state: &AppState,
    resources: &mut LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &AuthStore,
) -> Result<String> {
    let _ = shutdown_runtime_services();
    *resources = command_helpers::reload_resources_from_disk(state)?;
    *providers = ProviderRegistry::new();
    for provider in &resources.providers {
        providers.register_with_source(
            provider.value.clone().into_descriptor(),
            provider.source_info.as_provider_source(),
        );
    }
    let _ = providers.discover_and_merge_all(auth_store);
    apply_mcp_roster_to_runner(state, resources);
    command_helpers::reload_plugins_summary(state, resources)
}

/// Pushes the freshly-loaded MCP server roster onto the live tool runner.
///
/// Walks `Arc<dyn ToolRunner>` through the `as_any()` seam exposed by
/// `puffer_runner_api::ToolRunner` and, if the underlying type is a
/// `LocalToolRunner`, rebuilds its `McpHost`. Non-local runners (e.g.
/// `RemoteToolRunner`) ignore this: they manage MCP lifecycle out of
/// band, and forcing a reload here would step on the remote's roster.
fn apply_mcp_roster_to_runner(state: &AppState, resources: &LoadedResources) {
    let Some(any) = state.tool_runner.as_any() else {
        return;
    };
    let Some(local) = any.downcast_ref::<runner_adapter::LocalToolRunner>() else {
        return;
    };
    let servers = collect_runner_mcp_roster(resources);
    let workspace = Some(state.cwd.clone());
    local.replace_mcp_roster(servers, workspace);
}

/// Merges `resources.mcp_servers` with plugin-embedded MCP servers,
/// deduplicating by id (case-insensitive). Mirrors the merge done at
/// startup in `puffer_runner_local::local_runner_from_resources` so a
/// hot-reload yields the same effective roster as a fresh start.
fn collect_runner_mcp_roster(resources: &LoadedResources) -> Vec<puffer_resources::McpServerSpec> {
    let mut servers: Vec<puffer_resources::McpServerSpec> = resources
        .mcp_servers
        .iter()
        .map(|item| item.value.clone())
        .collect();
    for (_plugin, spec) in puffer_resources::plugin_mcp_servers(resources) {
        if servers
            .iter()
            .any(|existing| existing.id.eq_ignore_ascii_case(&spec.id))
        {
            continue;
        }
        servers.push(spec.clone());
    }
    servers
}
