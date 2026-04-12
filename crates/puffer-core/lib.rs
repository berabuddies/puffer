mod agent_catalog;
mod command;
mod command_helpers;
mod command_summary;
mod config_settings;
mod hooks;
mod model_preferences;
mod permissions;
mod plan_mode;
mod plans;
mod runtime;
mod skill_support;
mod state;
#[cfg(test)]
pub(crate) mod test_locks;
mod tool_names;
mod workspace_paths;

pub use agent_catalog::{load_agent_catalog, AgentCatalogEntry};
pub use command::{
    command_surface, dispatch_command, find_command, supported_commands, CommandKind, CommandSpec,
};
pub use command_helpers::CommandActionEntry;
pub use command_helpers::CopyActionEntry;
pub use command_helpers::McpActionEntry;
pub use command_helpers::PluginActionEntry;
pub use command_helpers::ResumeLaunchResolution;
pub use command_helpers::SessionOverlayView;
pub use command_helpers::TaskActionEntry;
pub(crate) use command_summary::{render_buddy_summary, render_cost_summary, render_usage_summary};
pub use hooks::run_resource_hooks;
pub use model_preferences::{
    default_effort_level, effort_level_is_supported, normalized_effort_level,
    provider_preference_family, supported_effort_levels, ModelPreferenceFamily,
};
pub use runtime::background_tasks;
pub use runtime::claude_tools::execute_workflow_tool;
pub use runtime::execute_user_prompt as execute_user_turn;
pub use runtime::teammate_loop;
pub use runtime::{
    execute_side_question, execute_user_prompt_streaming as execute_user_turn_streaming,
    execute_user_prompt_streaming_with_permissions as execute_user_turn_streaming_with_permissions,
    execute_user_prompt_streaming_with_structured_output as execute_user_turn_streaming_with_structured_output,
    execute_user_prompt_with_structured_output as execute_user_turn_with_structured_output,
    shutdown_runtime_services, with_permission_prompt_handler, PermissionPromptAction,
    PermissionPromptRequest, StructuredOutputConfig, ToolCallRequest, ToolInvocation,
    TurnExecution, TurnStreamEvent,
};
pub use state::{AppState, MessageRole, RenderedMessage, TaskRecord, TaskStatus};

use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, SessionSummary};
use std::path::Path;

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

/// Renders the current `/context` summary used by interactive overlays.
pub fn render_context_panel(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
) -> Result<String> {
    runtime::render_context_usage_summary(state, resources, providers)
}

/// Returns the number of background shell tasks currently running.
pub fn running_background_task_count(state: &AppState) -> usize {
    runtime::claude_tools::workflow::running_shell_task_count(&state.cwd)
}

/// Drains background task completion notifications (clears after reading).
pub fn drain_background_task_completions(state: &AppState) -> Vec<String> {
    runtime::claude_tools::workflow::drain_completed_shell_tasks(&state.cwd)
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

/// Reloads declarative resources and rebuilds the provider registry for the active session.
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
    command_helpers::reload_plugins_summary(state, resources)
}
