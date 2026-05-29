pub mod agent;
pub mod anthropic_stream;
pub mod ask_user_question;
mod ask_user_question_types;
pub mod canvas;
pub mod comfyui_action;
pub mod computer_use_action;
pub mod config;
pub mod connector_tools;
pub mod cron_create;
pub mod cron_delete;
pub mod cron_list;
pub mod debugpy_action;
pub mod discord_action;
pub mod email_configure;
pub mod enter_plan_mode;
pub mod enter_worktree;
pub mod exit_plan_mode;
pub mod exit_worktree;
pub mod goal;
pub mod http_request;
pub mod image_generation;
pub mod lambda_internal;
pub mod lark;
pub mod lsp;
mod lsp_live;
mod lsp_live_diagnostics;
pub mod mcp_status;
pub mod mcp_tool_call;
pub mod modal_action;
pub mod native_mcp_action;
pub mod powershell;
pub mod process_control;
pub mod recall;
pub mod request_user_browser_action;
pub mod secret_value;
pub mod send_message;
pub mod send_user_message;
pub mod shopify_action;
pub mod slack;
pub mod slack_action;
pub mod spotify_action;
pub mod structured_output;
pub mod subscriber_install;
pub mod subscriber_list;
pub mod subscriber_scaffold;
pub mod subscription_create;
pub mod subscription_delete;
pub(crate) mod subscription_globals;
pub mod subscription_list;
pub mod subscription_pause;
pub mod task_create;
pub mod task_flow;
pub mod task_get;
pub mod task_list;
pub mod task_output;
pub mod task_stop;
pub mod task_update;
pub mod team_create;
pub mod team_delete;
mod telegram_format;
pub mod telegram_login;
pub mod todo_write;
pub mod touchdesigner_action;
pub mod vision_analyze;
pub mod workflow_tools;

pub(crate) mod store;
mod support;
mod task_runtime;
mod task_tools;

use anyhow::Result;
use std::path::Path;
use uuid::Uuid;

/// Registers one background Bash task in the session-scoped workflow task store.
pub(crate) fn register_background_shell_task(
    cwd: &Path,
    session_id: &Uuid,
    task_id: &str,
    subject: &str,
    command: &str,
    process_id: u32,
    output_file: &Path,
) -> Result<()> {
    support::register_background_shell_task(
        cwd,
        session_id,
        task_id,
        subject,
        command,
        process_id,
        output_file,
    )
}

/// Marks a background shell task as completed after its process exits.
/// Called from the reaper thread in bash.rs.
pub(crate) fn mark_shell_task_completed(
    cwd: &Path,
    session_id: &Uuid,
    task_id: &str,
    exit_code: Option<i32>,
) -> Result<()> {
    support::mark_shell_task_completed(cwd, session_id, task_id, exit_code)
}

/// Returns descriptions of background shell tasks that finished since last check.
/// Each call returns newly completed tasks and clears their "pending notification" flag.
pub(crate) fn drain_completed_shell_tasks(cwd: &Path, session_id: &Uuid) -> Vec<String> {
    support::drain_completed_shell_tasks(cwd, session_id)
}

/// Returns the number of background shell tasks currently running.
pub(crate) fn running_shell_task_count(cwd: &Path, session_id: &Uuid) -> usize {
    // Best-effort: if the workflow directory doesn't exist or isn't writable
    // (e.g. in test environments), return 0.
    let tasks_path = match store::workflow_root_opt(cwd) {
        Some(root) => {
            let dir = root.join("sessions").join(session_id.to_string());
            dir.join("tasks.json")
        }
        None => return 0,
    };
    store::load_store::<store::TaskStore>(&tasks_path)
        .map(|s| {
            s.tasks
                .iter()
                .filter(|t| t.status == "running" || t.status == "in_progress")
                .count()
        })
        .unwrap_or(0)
}
