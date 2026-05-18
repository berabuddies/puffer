use super::common::open_text_file_in_editor;
use super::emit_system;
use super::{append_tool_invocations, append_trace_events};
use crate::plan_mode::{enter_plan_mode, preview_plan_mode_context_message};
use crate::plans::{plan_file_path, plan_has_user_content, read_plan_text};
use crate::runtime::RequestToolFilter;
use crate::{AppState, MessageRole};
use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, TranscriptEvent};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

/// Describes how a prompt command should be handled after specialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PromptCommandPreparation {
    /// The command already produced local output and should skip provider execution.
    HandledLocally,
    /// The command should execute through the provider with a custom prompt body.
    PromptOverride(String),
    /// The command should submit the provided text as a normal user prompt.
    DirectPrompt(String),
    /// The command should execute as a one-off side question outside the main transcript.
    SideQuestion(String),
    /// The command should render its resource prompt with extra computed variables.
    VariableOverrides(BTreeMap<String, String>),
}

/// Returns any specialized handling required for prompt commands with local semantics.
pub(crate) fn prepare_prompt_command_specialization(
    state: &mut AppState,
    session_store: &SessionStore,
    command_name: &str,
    args: &str,
) -> Result<Option<PromptCommandPreparation>> {
    match command_name {
        "btw" => Ok(Some(prepare_btw_prompt_command(
            state,
            session_store,
            args,
        )?)),
        "compact" => Ok(Some(prepare_compact_prompt_command(
            state,
            session_store,
            args,
        )?)),
        "commit" => Ok(Some(prepare_commit_prompt_command(state)?)),
        "pr-comments" => Ok(Some(prepare_pr_comments_prompt_command(args))),
        "security-review" => Ok(Some(prepare_security_review_prompt_command(state)?)),
        "statusline" => Ok(Some(prepare_statusline_prompt_command(args)?)),
        _ => Ok(None),
    }
}

/// Prepares `/btw` side-question handling without appending a user prompt to the main transcript.
pub(crate) fn prepare_btw_prompt_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<PromptCommandPreparation> {
    let question = args.trim();
    if question.is_empty() {
        emit_system(
            state,
            session_store,
            "Usage: /btw <your question>".to_string(),
        )?;
        return Ok(PromptCommandPreparation::HandledLocally);
    }
    Ok(PromptCommandPreparation::SideQuestion(question.to_string()))
}

/// Prepares `/compact` by generating a provider-driven compaction prompt override.
pub(crate) fn prepare_compact_prompt_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<PromptCommandPreparation> {
    if state.transcript.is_empty() {
        emit_system(
            state,
            session_store,
            "No messages are available to compact.".to_string(),
        )?;
        return Ok(PromptCommandPreparation::HandledLocally);
    }
    Ok(PromptCommandPreparation::PromptOverride(
        build_compact_prompt_override(state, args),
    ))
}

/// Computes git-aware context variables for `/commit`.
pub(crate) fn prepare_commit_prompt_command(state: &AppState) -> Result<PromptCommandPreparation> {
    Ok(PromptCommandPreparation::VariableOverrides(
        build_commit_prompt_variables(&state.cwd)?,
    ))
}

/// Handles `/plan` local behaviors using Claude-style plan-mode semantics.
pub(crate) fn prepare_plan_prompt_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<PromptCommandPreparation> {
    let plan_path = plan_file_path(state)?;
    let trimmed = args.trim();

    if !state.plan_mode {
        enter_plan_mode(state)?;
        emit_system(state, session_store, "Enabled plan mode".to_string())?;
        if !trimmed.is_empty() && trimmed != "open" {
            state.queue_pending_query_prompt(trimmed.to_string());
        }
        return Ok(PromptCommandPreparation::HandledLocally);
    }

    let Some(plan_body) = read_plan_text(state)?.filter(|text| plan_has_user_content(text)) else {
        emit_system(
            state,
            session_store,
            "Already in plan mode. No plan written yet.".to_string(),
        )?;
        return Ok(PromptCommandPreparation::HandledLocally);
    };

    if trimmed.split_whitespace().next() == Some("open") {
        let status = match open_text_file_in_editor(&plan_path) {
            Ok(_) => format!("Opened plan in editor: {}", plan_path.display()),
            Err(error) => format!("Failed to open plan in editor: {error}"),
        };
        emit_system(state, session_store, status)?;
        return Ok(PromptCommandPreparation::HandledLocally);
    }
    emit_system(
        state,
        session_store,
        render_current_plan_message(&plan_path, &plan_body),
    )?;
    Ok(PromptCommandPreparation::HandledLocally)
}

/// Handles `/plan` from the local command path.
pub(crate) fn handle_plan_command(
    state: &mut AppState,
    _resources: &LoadedResources,
    _providers: &ProviderRegistry,
    _auth_store: &mut AuthStore,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let _ = prepare_plan_prompt_command(state, session_store, args)?;
    Ok(())
}

/// Supplies the optional user-input block used by the declarative `/pr-comments` prompt.
pub(crate) fn prepare_pr_comments_prompt_command(args: &str) -> PromptCommandPreparation {
    PromptCommandPreparation::VariableOverrides(build_pr_comments_prompt_variables(args))
}

/// Computes git-aware context variables for `/security-review`.
pub(crate) fn prepare_security_review_prompt_command(
    state: &AppState,
) -> Result<PromptCommandPreparation> {
    Ok(PromptCommandPreparation::VariableOverrides(
        build_security_review_prompt_variables(&state.cwd),
    ))
}

/// Builds the Claude-style `/statusline` setup variables.
pub(crate) fn prepare_statusline_prompt_command(args: &str) -> Result<PromptCommandPreparation> {
    Ok(PromptCommandPreparation::VariableOverrides(
        build_statusline_prompt_variables(args)?,
    ))
}

/// Executes the provider-backed `/compact` prompt and persists the compacted transcript.
pub(crate) fn execute_compact_prompt_command(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    session_store: &SessionStore,
    rendered: &str,
    tool_filter: Option<&RequestToolFilter>,
) -> Result<()> {
    if state.memory_flush_enabled() {
        let _ = crate::flush_project_memory(state, resources, providers, auth_store);
    }
    record_specialized_prompt_request(state, session_store, rendered)?;
    match crate::runtime::execute_user_prompt_with_tool_filter(
        state,
        resources,
        providers,
        auth_store,
        rendered,
        tool_filter,
    ) {
        Ok(turn) => {
            append_tool_invocations(state, session_store, &turn.tool_invocations)?;
            append_trace_events(session_store, state.session.id, &turn.reflection_traces);
            finalize_compact_prompt_command(state, session_store, &turn.assistant_text)
        }
        Err(error) => emit_system(
            state,
            session_store,
            format!("Prompt command /compact failed: {error}"),
        ),
    }
}

fn record_specialized_prompt_request(
    state: &mut AppState,
    session_store: &SessionStore,
    rendered: &str,
) -> Result<()> {
    state.push_message(MessageRole::User, rendered.to_string());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: rendered.to_string(),
        },
    )?;
    Ok(())
}

/// Applies a provider-generated compaction summary and persists the transcript rewrite.
pub(crate) fn finalize_compact_prompt_command(
    state: &mut AppState,
    session_store: &SessionStore,
    summary: &str,
) -> Result<()> {
    session_store.append_transcript_clear(state.session.id)?;
    state.apply_transcript_rewrite(&puffer_session_store::TranscriptRewrite::Clear);
    // Re-inject the summary as a user message so the model retains context
    // in subsequent turns (CC uses a "boundary marker" + summary message).
    let boundary = format!(
        "[Conversation compacted — prior context summarized below]\n\n{}",
        summary.trim_end()
    );
    state.push_message(MessageRole::User, boundary.clone());
    session_store.append_event(
        state.session.id,
        puffer_session_store::TranscriptEvent::UserMessage { text: boundary },
    )?;
    emit_system(
        state,
        session_store,
        "Conversation compacted. Summary preserved in context.".to_string(),
    )
}

/// Renders the next active plan-mode reminder for previews and context estimates.
pub(crate) fn plan_mode_context_message(
    state: &AppState,
    resources: &LoadedResources,
) -> Result<Option<String>> {
    preview_plan_mode_context_message(state, resources)
}

fn build_compact_prompt_override(state: &AppState, args: &str) -> String {
    let trimmed_instruction = args.trim();
    let mut user_messages = 0usize;
    let mut assistant_messages = 0usize;
    let mut system_messages = 0usize;

    for message in &state.transcript {
        match message.role {
            MessageRole::User => user_messages += 1,
            MessageRole::Assistant => assistant_messages += 1,
            MessageRole::System | MessageRole::ToolCall | MessageRole::ToolResult => {
                system_messages += 1
            }
        }
    }

    let mut text = String::new();
    text.push_str(
        "Summarize the conversation into a compact context block that preserves all information \
         needed to continue work seamlessly. Use NO tools — return only the summary text.\n\n",
    );
    text.push_str("Structure your summary with these sections:\n");
    text.push_str("1. **User Intent** — what the user is trying to accomplish\n");
    text.push_str(
        "2. **Key Concepts** — important terms, patterns, or architectural decisions discussed\n",
    );
    text.push_str(
        "3. **Files & Code** — files read, edited, or created, with paths and brief descriptions\n",
    );
    text.push_str("4. **Errors & Fixes** — errors encountered and how they were resolved\n");
    text.push_str("5. **Pending Tasks** — any incomplete work, open questions, or next steps\n");
    text.push_str("6. **Current State** — what was just completed before this compaction\n\n");

    let _ = writeln!(
        &mut text,
        "Conversation stats: {} user, {} assistant, {} system messages.",
        user_messages, assistant_messages, system_messages
    );
    if !trimmed_instruction.is_empty() {
        let _ = writeln!(&mut text, "Additional instruction: {trimmed_instruction}");
    }
    text.push_str("\nBe thorough but concise. Preserve file paths, function names, and error messages verbatim.\n");
    text
}

fn build_pr_comments_prompt_variables(args: &str) -> BTreeMap<String, String> {
    let trimmed = args.trim();
    BTreeMap::from([(
        "ADDITIONAL_USER_INPUT_BLOCK".to_string(),
        if trimmed.is_empty() {
            String::new()
        } else {
            format!("Additional user input: {trimmed}")
        },
    )])
}

fn build_commit_prompt_variables(cwd: &Path) -> Result<BTreeMap<String, String>> {
    Ok(BTreeMap::from([
        (
            "GIT_STATUS".to_string(),
            run_git_command_for_prompt(cwd, &["status"]).map_err(anyhow::Error::msg)?,
        ),
        (
            "GIT_DIFF".to_string(),
            run_git_command_for_prompt(cwd, &["diff", "HEAD"]).map_err(anyhow::Error::msg)?,
        ),
        (
            "CURRENT_BRANCH".to_string(),
            run_git_command_for_prompt(cwd, &["branch", "--show-current"])
                .map_err(anyhow::Error::msg)?,
        ),
        (
            "RECENT_COMMITS".to_string(),
            run_git_command_for_prompt(cwd, &["log", "--oneline", "-10"])
                .map_err(anyhow::Error::msg)?,
        ),
        ("COMMIT_ATTRIBUTION_BLOCK".to_string(), String::new()),
    ]))
}

fn build_statusline_prompt_variables(args: &str) -> Result<BTreeMap<String, String>> {
    let prompt = if args.trim().is_empty() {
        "Configure my statusLine from my shell PS1 configuration".to_string()
    } else {
        args.trim().to_string()
    };
    Ok(BTreeMap::from([(
        "STATUSLINE_PROMPT_JSON".to_string(),
        serde_json::to_string(&prompt)?,
    )]))
}

fn build_security_review_prompt_variables(cwd: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "GIT_STATUS".to_string(),
            run_git_with_fallbacks(cwd, &[&["status"]]),
        ),
        (
            "FILES_MODIFIED".to_string(),
            run_git_with_fallbacks(
                cwd,
                &[
                    &["diff", "--name-only", "origin/HEAD..."],
                    &["diff", "--name-only"],
                ],
            ),
        ),
        (
            "COMMITS".to_string(),
            run_git_with_fallbacks(
                cwd,
                &[
                    &["log", "--no-decorate", "origin/HEAD..."],
                    &["log", "--no-decorate", "-n", "10"],
                ],
            ),
        ),
        (
            "DIFF_CONTENT".to_string(),
            run_git_with_fallbacks(cwd, &[&["diff", "origin/HEAD..."], &["diff"]]),
        ),
    ])
}

fn run_git_with_fallbacks(cwd: &Path, candidates: &[&[&str]]) -> String {
    let mut last_failure = String::new();
    for candidate in candidates {
        match run_git_command(cwd, candidate) {
            Ok(output) => return output,
            Err(error) => last_failure = error,
        }
    }
    last_failure
}

fn run_git_command_for_prompt(cwd: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|error| format!("Failed to run `git {}`: {error}", args.join(" ")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        return Ok(format_prompt_shell_output(&stdout, &stderr));
    }

    let rendered = format_prompt_shell_output(&stdout, &stderr);
    let detail = if rendered.is_empty() {
        "<no output>".to_string()
    } else {
        rendered
    };
    Err(format!("Command `git {}` failed: {detail}", args.join(" ")))
}

fn run_git_command(cwd: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|error| format!("Failed to run `git {}`: {error}", args.join(" ")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        if stdout.is_empty() {
            Ok("<no output>".to_string())
        } else {
            Ok(stdout)
        }
    } else {
        let exit = output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string());
        Err(format!(
            "Command `git {}` failed with exit code {exit}.\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            if stdout.is_empty() {
                "<no output>"
            } else {
                &stdout
            },
            if stderr.is_empty() {
                "<no output>"
            } else {
                &stderr
            }
        ))
    }
}

fn format_prompt_shell_output(stdout: &str, stderr: &str) -> String {
    let mut parts = Vec::new();
    if !stdout.is_empty() {
        parts.push(stdout.to_string());
    }
    if !stderr.is_empty() {
        parts.push(format!("[stderr]\n{stderr}"));
    }
    parts.join("\n")
}

fn single_line_excerpt(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or("");
    if line.chars().count() <= 120 {
        line.to_string()
    } else {
        let mut shortened = String::new();
        for ch in line.chars().take(117) {
            shortened.push(ch);
        }
        shortened.push_str("...");
        shortened
    }
}
fn render_current_plan_message(plan_path: &Path, plan_body: &str) -> String {
    let mut message = format!("Current Plan\n{}", plan_path.display());
    if !plan_body.is_empty() {
        let _ = write!(&mut message, "\n\n{}", plan_body.trim_end());
    }
    if let Some(editor_name) = configured_editor_display_name() {
        let _ = writeln!(
            &mut message,
            "\n\n\"/plan open\" to edit this plan in {}",
            editor_name
        );
        return message.trim_end().to_string();
    }
    message
}

fn configured_editor_display_name() -> Option<String> {
    std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok())
        .and_then(|command| {
            let binary = command.split_whitespace().next()?;
            let basename = std::path::Path::new(binary)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(binary)
                .to_ascii_lowercase();
            let display = match basename.as_str() {
                "code" => "VS Code",
                "cursor" => "Cursor",
                "windsurf" => "Windsurf",
                "codium" => "VSCodium",
                "nvim" => "Neovim",
                "vim" => "Vim",
                "vi" => "vi",
                "nano" => "nano",
                _ => binary,
            };
            Some(display.to_string())
        })
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
