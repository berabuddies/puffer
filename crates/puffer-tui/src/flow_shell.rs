use anyhow::Result;
use puffer_core::{run_resource_hooks, AppState, MessageRole};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, TranscriptEvent};
use puffer_tools::{ToolInput, ToolRegistry};
use std::sync::mpsc;
use std::thread;

use crate::state::{PendingSubmit, PendingSubmitEvent, ShellShortcutResult};
use crate::TuiState;

/// Starts a `!cmd` shell shortcut without blocking the interactive TUI loop.
pub(crate) fn execute_shell_shortcut(
    state: &mut AppState,
    resources: &LoadedResources,
    session_store: &SessionStore,
    tui: &mut TuiState,
    shell_command: &str,
) -> Result<()> {
    let rendered_command = format!("!{shell_command}");
    state.push_message(MessageRole::User, rendered_command.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: rendered_command,
            actor: Some(state.user_actor()),
        },
    )?;

    let resources = resources.clone();
    let cwd = state.cwd.clone();
    let command = shell_command.to_string();
    let prompt = format!("!{shell_command}");
    let (event_tx, event_rx) = mpsc::channel();
    thread::spawn(move || {
        let tool_input_json = shell_tool_input_json(&command);
        run_resource_hooks(
            &resources,
            &cwd,
            "tool_start",
            &[
                ("PUFFER_TOOL_ID", "bash".to_string()),
                ("PUFFER_TOOL_INPUT", tool_input_json.clone()),
                ("PUFFER_TOOL_SUCCESS", String::new()),
                ("PUFFER_TOOL_STDOUT", String::new()),
                ("PUFFER_TOOL_STDERR", String::new()),
            ],
        );
        let registry = ToolRegistry::from_resources(&resources);
        let result = shell_shortcut_result(
            command.clone(),
            registry.execute(
                "bash",
                &cwd,
                ToolInput::Bash {
                    command: command.clone(),
                    timeout: None,
                    run_in_background: false,
                    dangerously_disable_sandbox: false,
                },
            ),
        );
        run_resource_hooks(
            &resources,
            &cwd,
            "tool_end",
            &[
                ("PUFFER_TOOL_ID", "bash".to_string()),
                ("PUFFER_TOOL_INPUT", tool_input_json),
                (
                    "PUFFER_TOOL_SUCCESS",
                    if result.success { "true" } else { "false" }.to_string(),
                ),
                ("PUFFER_TOOL_STDOUT", result.stdout.clone()),
                ("PUFFER_TOOL_STDERR", result.stderr.clone()),
            ],
        );
        let _ = event_tx.send(PendingSubmitEvent::ShellShortcutFinished(result));
    });
    tui.pending_submit = Some(PendingSubmit {
        prompt,
        receiver: event_rx,
        transcript_persisted_len: state.transcript.len(),
        rendered_tool_invocations: 0,
        pending_tool_calls: Vec::new(),
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: Some("Running shell command".to_string()),
        cancel: puffer_core::CancelToken::new(),
    });
    Ok(())
}

/// Executes a `!cmd` shell shortcut inline for non-interactive submit paths.
pub(crate) fn execute_shell_shortcut_inline(
    state: &mut AppState,
    resources: &LoadedResources,
    session_store: &SessionStore,
    shell_command: &str,
) -> Result<()> {
    let rendered_command = format!("!{shell_command}");
    state.push_message(MessageRole::User, rendered_command.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: rendered_command,
            actor: Some(state.user_actor()),
        },
    )?;

    let tool_input_json = shell_tool_input_json(shell_command);
    run_resource_hooks(
        resources,
        &state.cwd,
        "tool_start",
        &[
            ("PUFFER_TOOL_ID", "bash".to_string()),
            ("PUFFER_TOOL_INPUT", tool_input_json.clone()),
            ("PUFFER_TOOL_SUCCESS", String::new()),
            ("PUFFER_TOOL_STDOUT", String::new()),
            ("PUFFER_TOOL_STDERR", String::new()),
        ],
    );
    let registry = ToolRegistry::from_resources(resources);
    let result = shell_shortcut_result(
        shell_command.to_string(),
        registry.execute(
            "bash",
            &state.cwd,
            ToolInput::Bash {
                command: shell_command.to_string(),
                timeout: None,
                run_in_background: false,
                dangerously_disable_sandbox: false,
            },
        ),
    );
    run_resource_hooks(
        resources,
        &state.cwd,
        "tool_end",
        &[
            ("PUFFER_TOOL_ID", "bash".to_string()),
            ("PUFFER_TOOL_INPUT", tool_input_json),
            (
                "PUFFER_TOOL_SUCCESS",
                if result.success { "true" } else { "false" }.to_string(),
            ),
            ("PUFFER_TOOL_STDOUT", result.stdout.clone()),
            ("PUFFER_TOOL_STDERR", result.stderr.clone()),
        ],
    );
    finalize_shell_shortcut_result(state, session_store, result)
}

/// Records a completed shell shortcut result in task state and transcript.
pub(crate) fn finalize_shell_shortcut_result(
    state: &mut AppState,
    session_store: &SessionStore,
    result: ShellShortcutResult,
) -> Result<()> {
    state.record_task("bash", result.command.clone(), result.success);
    let reply = if result.stderr.is_empty() {
        result.stdout
    } else if result.stdout.is_empty() {
        result.stderr
    } else {
        format!("{}\n{}", result.stdout, result.stderr)
    };
    let role = if result.success {
        MessageRole::Assistant
    } else {
        MessageRole::System
    };
    state.push_message(role, reply.clone());
    let event = if result.success {
        TranscriptEvent::AssistantMessage {
            text: reply,
            actor: Some(state.assistant_actor()),
        }
    } else {
        TranscriptEvent::SystemMessage {
            text: reply,
            actor: Some(state.system_actor()),
        }
    };
    session_store.append_event(state.session.id, event)?;
    Ok(())
}

/// Parses the `!cmd` shell shortcut form used by Claude/Codex-style CLIs.
pub(crate) fn parse_shell_shortcut(input: &str) -> Option<&str> {
    let command = input
        .strip_prefix("!!")
        .or_else(|| input.strip_prefix('!'))?;
    let trimmed = command.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn shell_shortcut_result(
    command: String,
    result: Result<puffer_tools::ToolExecutionResult>,
) -> ShellShortcutResult {
    match result {
        Ok(result) => ShellShortcutResult {
            command,
            success: result.success,
            stdout: result.output.stdout,
            stderr: result.output.stderr,
        },
        Err(error) => ShellShortcutResult {
            command,
            success: false,
            stdout: String::new(),
            stderr: error.to_string(),
        },
    }
}

fn shell_tool_input_json(shell_command: &str) -> String {
    format!("{{\"command\":\"{}\"}}", shell_command.replace('"', "\\\""))
}
