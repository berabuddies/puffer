mod actions;
mod agents;
pub(crate) mod artifacts;
mod auth;
mod branch;
mod common;
mod config;
mod doctor;
mod ecosystem;
mod model;
mod plugins;
pub(crate) mod prompt;
mod reflect;
mod resume;
mod session;
mod tasks;
mod terminal_setup;

pub use actions::CommandActionEntry;
pub(crate) use agents::handle_agents_command;
pub use artifacts::CopyActionEntry;
pub(crate) use artifacts::{handle_copy_command, handle_export_command, render_copy_actions};
#[cfg(test)]
pub(crate) use auth::with_login_flow_handler;
pub(crate) use auth::{
    remove_provider_credentials, render_login_guidance, run_provider_login_flow, supports_auth_mode,
};
pub(crate) use branch::handle_branch_command;
pub(crate) use common::{
    describe_context, describe_files_in_context, describe_git_diff, emit_system,
    execute_skill_command, list_skills, record_command_checkpoint, render_skills_panel,
    rewind_transcript,
};
pub(crate) use config::{
    handle_config_command, handle_hooks_command, handle_keybindings_command,
    handle_permissions_command, handle_sandbox_command, persist_user_model_selection,
    persist_user_settings, reload_config_from_disk, render_config_summary,
    render_permissions_panel, render_sandbox_actions,
};
pub(crate) use doctor::{render_doctor_report, run_doctor};
pub use ecosystem::McpActionEntry;
pub(crate) use ecosystem::{
    handle_ide_command, handle_mcp_command, reload_resources_from_disk, render_ide_actions,
    render_mcp_actions, render_mcp_summary,
};
pub(crate) use model::{
    apply_model_preferences, handle_effort_command, handle_fast_command, handle_model_command,
};
pub use plugins::PluginActionEntry;
pub(crate) use plugins::{
    handle_plugin_command, reload_plugins_summary, render_plugin_actions, render_plugin_summary,
};
pub(crate) use prompt::handle_plan_command;
pub(crate) use reflect::handle_reflect_command;
pub(crate) use resume::handle_resume_command;
pub(crate) use resume::resumable_sessions_for_picker;
pub use resume::{resolve_resume_launch, ResumeLaunchResolution};
pub use session::SessionOverlayView;
pub(crate) use session::{
    append_tool_invocations, handle_memory_command, handle_remote_control_command,
    handle_remote_env_command, handle_session_command, handle_tag_command, render_memory_panel,
    render_session_overlay, render_session_panel,
};
pub use tasks::TaskActionEntry;
pub(crate) use tasks::{handle_tasks_command, render_task_actions, render_tasks_panel_text};
pub(crate) use terminal_setup::{
    handle_terminal_setup_command, should_hide_terminal_setup_command,
    terminal_setup_command_description,
};

use crate::AppState;
use anyhow::Result;
use puffer_resources::LoadedResources;

/// Renders the hooks summary shown by `/hooks`.
pub(crate) fn render_hooks_summary(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<String> {
    config::render_hooks_summary(state, resources)
}

/// Builds the interactive `/hooks` action list used by the TUI picker.
pub(crate) fn render_hooks_actions(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Vec<CommandActionEntry>> {
    config::render_hooks_actions(state, resources)
}
