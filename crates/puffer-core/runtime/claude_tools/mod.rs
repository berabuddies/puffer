use crate::runtime::structured_output_support::StructuredOutputConfig;
use crate::state::ClaudeReadState;
use crate::workspace_paths;
use crate::AppState;
use anyhow::{bail, Context, Result};
use puffer_provider_openai::OpenAIRequestConfig;
use puffer_remote_tools::RemoteToolExecutionContext;
use puffer_resources::LoadedResources;
use puffer_tools::{ToolDefinition, ToolExecutionResult, ToolOutput, ToolRegistry};
use puffer_transport_anthropic::AnthropicRequestConfig;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use uuid::Uuid;

pub(crate) mod bash;
pub(crate) use bash::BashOutputStream;
pub(crate) mod edit;
pub(crate) mod glob;
pub(crate) mod grep;
pub(super) mod mcp_resources;
pub(crate) mod notebook_edit;
pub(crate) mod read;
pub(crate) mod skill;
pub(crate) mod tool_search;
pub(crate) mod web_fetch;
pub(crate) mod web_search;

/// Retries a blocking HTTP send operation up to `max_attempts` times with 1s delay
/// on transient connection/timeout errors.
fn retry_http_send<F>(
    max_attempts: usize,
    mut operation: F,
) -> anyhow::Result<reqwest::blocking::Response>
where
    F: FnMut() -> anyhow::Result<reqwest::blocking::Response>,
{
    let max_attempts = max_attempts.max(1);
    for attempt in 1..=max_attempts {
        match operation() {
            Ok(response) => return Ok(response),
            Err(error) if attempt < max_attempts && is_retryable_send_error(&error) => {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

fn is_retryable_send_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|e| e.is_timeout() || e.is_connect() || e.is_request())
    })
}
pub(crate) mod workflow;
pub(crate) mod write;

/// Carries provider-specific execution context for runtime-backed tools.
pub(crate) enum ProviderToolContext<'a> {
    None,
    OpenAI {
        request_config: &'a OpenAIRequestConfig,
        model_id: &'a str,
        structured_output: Option<&'a StructuredOutputConfig>,
    },
    Anthropic {
        request_config: &'a AnthropicRequestConfig,
        model_id: &'a str,
        structured_output: Option<&'a StructuredOutputConfig>,
    },
}

/// Builds any remote execution context required for a provider-backed tool.
pub(crate) fn build_remote_execution_context(
    tool_id: &str,
    provider_context: &ProviderToolContext<'_>,
    input: &Value,
) -> Result<Option<RemoteToolExecutionContext>> {
    match tool_id {
        "WebSearch" => Ok(Some(web_search::build_remote_web_search_context(
            provider_context,
            input.clone(),
        )?)),
        _ => Ok(None),
    }
}

/// Returns true when the handler should be routed through the Claude tool dispatcher.
pub(crate) fn is_claude_runtime_handler(handler: &str) -> bool {
    handler.starts_with("runtime:claude_") || handler.starts_with("runtime:workflow:")
}

/// Executes one tool invocation using the Claude parity dispatcher when applicable.
pub(crate) fn execute_tool(
    state: &mut AppState,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    definition: &ToolDefinition,
    cwd: &Path,
    input: Value,
    provider_context: ProviderToolContext<'_>,
) -> Result<ToolExecutionResult> {
    let allow_all_paths = workspace_paths::sandbox_allows_all_paths(&state.sandbox_mode);
    match definition.id.as_str() {
        "Bash" => {
            let execution = bash::execute_from_value(cwd, &state.session.id, input)?;
            let output = serde_json::to_string_pretty(&execution.output)
                .context("failed to serialize Bash output")?;
            Ok(tool_result(definition, execution.success, output))
        }
        "Read" => {
            if should_use_local_read_shortcut(state) && is_full_read_request(&input) {
                if let Some(path) = input_file_path(&input, "file_path")? {
                    if let Some(snapshot) = state.claude_read_state.get(&path) {
                        let timestamp_ms = file_timestamp_ms(&path)?;
                        if !snapshot.is_partial_view && timestamp_ms == snapshot.timestamp_ms {
                            let output = read::execute_claude_file_unchanged(
                                path.display().to_string().as_str(),
                            )?;
                            return Ok(tool_result(definition, true, output));
                        }
                    }
                }
            }
            let output = read::execute_claude_read_tool(
                cwd,
                &state.working_dirs,
                allow_all_paths,
                input.clone(),
            )?;
            record_read_from_input(state, &input)?;
            Ok(tool_result(definition, true, output))
        }
        "Write" => {
            let mut read_state = clone_read_state(state);
            let output = write::execute_claude_write_tool(
                cwd,
                &state.working_dirs,
                allow_all_paths,
                input.clone(),
                &mut read_state,
            )?;
            sync_read_state(state, read_state);
            if let Some(path) = input_file_path(&input, "file_path")? {
                mark_fully_read(state, &path)?;
            }
            Ok(tool_result(definition, true, output))
        }
        "Edit" => {
            if edit::requires_prior_read(&input) {
                enforce_read_precondition(state, input_file_path(&input, "file_path")?.as_deref())?;
            }
            let output = edit::execute_claude_edit(
                cwd,
                &state.working_dirs,
                allow_all_paths,
                input.clone(),
            )?;
            if let Some(path) = input_file_path(&input, "file_path")? {
                mark_fully_read(state, &path)?;
            }
            Ok(tool_result(definition, true, output))
        }
        "Glob" => Ok(tool_result(
            definition,
            true,
            glob::execute_claude_glob(cwd, &state.working_dirs, allow_all_paths, input)?,
        )),
        "Grep" => Ok(tool_result(
            definition,
            true,
            grep::execute_claude_grep(cwd, &state.working_dirs, allow_all_paths, input)?,
        )),
        "NotebookEdit" => {
            if let Err(error) = enforce_read_precondition(
                state,
                input_file_path(&input, "notebook_path")?.as_deref(),
            ) {
                return Ok(tool_result(definition, false, error.to_string()));
            }
            let output = notebook_edit::execute_notebook_edit_tool(
                cwd,
                &state.working_dirs,
                allow_all_paths,
                input.clone(),
            )?;
            if let Some(path) = input_file_path(&input, "notebook_path")? {
                mark_fully_read(state, &path)?;
            }
            Ok(tool_result(definition, true, output))
        }
        "Skill" => Ok(tool_result(
            definition,
            true,
            skill::execute_claude_skill_tool(resources, input)?,
        )),
        "ToolSearch" => Ok(tool_result(
            definition,
            true,
            tool_search::execute_claude_tool_search_tool(registry, input)?,
        )),
        "ListMcpResourcesTool" | "ReadMcpResourceTool" => Ok(tool_result(
            definition,
            true,
            super::local_tools::execute_runtime_local_tool(
                state, resources, registry, definition, cwd, input,
            )?,
        )),
        "WebFetch" => {
            let output = serde_json::to_string_pretty(&web_fetch::execute_claude_web_fetch(input)?)
                .context("failed to serialize WebFetch output")?;
            Ok(tool_result(definition, true, output))
        }
        "WebSearch" => {
            let output = match provider_context {
                ProviderToolContext::OpenAI {
                    request_config,
                    model_id,
                    ..
                } => web_search::execute_claude_openai_web_search(request_config, model_id, input)?,
                ProviderToolContext::Anthropic {
                    request_config,
                    model_id,
                    ..
                } => web_search::execute_claude_anthropic_web_search(
                    request_config,
                    model_id,
                    input,
                )?,
                ProviderToolContext::None => {
                    bail!("WebSearch requires provider execution context")
                }
            };
            Ok(tool_result(definition, true, output))
        }
        _ if definition.handler.starts_with("runtime:workflow:") => Ok(tool_result(
            definition,
            true,
            execute_workflow_tool(
                state,
                resources,
                cwd,
                definition.id.as_str(),
                input,
                provider_context.structured_output(),
            )?,
        )),
        _ if super::local_tools::is_runtime_local_tool(definition) => Ok(tool_result(
            definition,
            true,
            super::local_tools::execute_runtime_local_tool(
                state, resources, registry, definition, cwd, input,
            )?,
        )),
        _ => registry.execute_json(&definition.id, cwd, input),
    }
}

/// Executes a parallel-safe tool without `&mut AppState`.
///
/// This handles only tools identified by `is_parallel_safe_tool()` and
/// replicates the corresponding match arms from `execute_tool`. All data
/// needed is passed by value/reference; no mutable application state is
/// touched, enabling concurrent execution via `std::thread::scope`.
pub(crate) fn execute_parallel_tool(
    definition: &ToolDefinition,
    cwd: &Path,
    working_dirs: &[PathBuf],
    allow_all_paths: bool,
    session_id: &Uuid,
    input: Value,
    resources: &LoadedResources,
    registry: &ToolRegistry,
    provider_context: &ProviderToolContext<'_>,
) -> Result<ToolExecutionResult> {
    match definition.id.as_str() {
        "Bash" => {
            let execution = bash::execute_from_value(cwd, session_id, input)?;
            let output = serde_json::to_string_pretty(&execution.output)
                .context("failed to serialize Bash output")?;
            Ok(tool_result(definition, execution.success, output))
        }
        "Glob" => Ok(tool_result(
            definition,
            true,
            glob::execute_claude_glob(cwd, working_dirs, allow_all_paths, input)?,
        )),
        "Grep" => Ok(tool_result(
            definition,
            true,
            grep::execute_claude_grep(cwd, working_dirs, allow_all_paths, input)?,
        )),
        "WebFetch" => {
            let output = serde_json::to_string_pretty(&web_fetch::execute_claude_web_fetch(input)?)
                .context("failed to serialize WebFetch output")?;
            Ok(tool_result(definition, true, output))
        }
        "WebSearch" => {
            let output = match provider_context {
                ProviderToolContext::OpenAI {
                    request_config,
                    model_id,
                    ..
                } => web_search::execute_claude_openai_web_search(request_config, model_id, input)?,
                ProviderToolContext::Anthropic {
                    request_config,
                    model_id,
                    ..
                } => web_search::execute_claude_anthropic_web_search(
                    request_config,
                    model_id,
                    input,
                )?,
                ProviderToolContext::None => {
                    bail!("WebSearch requires provider execution context")
                }
            };
            Ok(tool_result(definition, true, output))
        }
        "ToolSearch" => Ok(tool_result(
            definition,
            true,
            tool_search::execute_claude_tool_search_tool(registry, input)?,
        )),
        "Skill" => Ok(tool_result(
            definition,
            true,
            skill::execute_claude_skill_tool(resources, input)?,
        )),
        other => bail!("tool {other} is not parallel-safe"),
    }
}

fn tool_result(definition: &ToolDefinition, success: bool, stdout: String) -> ToolExecutionResult {
    ToolExecutionResult {
        tool_id: definition.id.clone(),
        success,
        output: ToolOutput {
            stdout,
            stderr: String::new(),
            metadata: Value::Null,
        },
    }
}

/// Applies the local prior-read guard for tools that will execute on a remote runner.
pub(crate) fn validate_remote_tool_preconditions(
    state: &AppState,
    tool_id: &str,
    input: &Value,
) -> Result<()> {
    match tool_id {
        "Edit" => enforce_read_precondition(state, input_file_path(input, "file_path")?.as_deref()),
        "NotebookEdit" => {
            enforce_read_precondition(state, input_file_path(input, "notebook_path")?.as_deref())
        }
        "Write" => {
            let Some(path) = input_file_path(input, "file_path")? else {
                return Ok(());
            };
            if path.exists() {
                enforce_read_precondition(state, Some(&path))
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

/// Mirrors the local read-state updates after a remote tool run completes successfully.
pub(crate) fn apply_remote_tool_state_update(
    state: &mut AppState,
    tool_id: &str,
    input: &Value,
) -> Result<()> {
    match tool_id {
        "Read" => record_read_from_input(state, input),
        "Write" | "Edit" => {
            if let Some(path) = input_file_path(input, "file_path")? {
                mark_fully_read(state, &path)?;
            }
            Ok(())
        }
        "NotebookEdit" => {
            if let Some(path) = input_file_path(input, "notebook_path")? {
                mark_fully_read(state, &path)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn input_file_path(input: &Value, field: &str) -> Result<Option<PathBuf>> {
    Ok(input.get(field).and_then(Value::as_str).map(PathBuf::from))
}

fn is_full_read_request(input: &Value) -> bool {
    !read_field_is_present(input, "offset")
        && !read_field_is_present(input, "limit")
        && !read_pages_field_is_present(input)
}

fn should_use_local_read_shortcut(state: &AppState) -> bool {
    state.config.remote_tool_runner.is_none()
}

fn clone_read_state(state: &AppState) -> HashMap<PathBuf, write::ClaudeReadSnapshot> {
    state
        .claude_read_state
        .iter()
        .map(|(path, snapshot)| {
            (
                path.clone(),
                write::ClaudeReadSnapshot {
                    timestamp_ms: snapshot.timestamp_ms,
                    is_partial_view: snapshot.is_partial_view,
                },
            )
        })
        .collect()
}

fn sync_read_state(state: &mut AppState, read_state: HashMap<PathBuf, write::ClaudeReadSnapshot>) {
    state.claude_read_state = read_state
        .into_iter()
        .map(|(path, snapshot)| {
            (
                path,
                ClaudeReadState {
                    timestamp_ms: snapshot.timestamp_ms,
                    is_partial_view: snapshot.is_partial_view,
                },
            )
        })
        .collect();
}

fn record_read_from_input(state: &mut AppState, input: &Value) -> Result<()> {
    let Some(path) = input_file_path(input, "file_path")? else {
        return Ok(());
    };
    let timestamp_ms = file_timestamp_ms(&path)?;
    // Mark as partial only when the read genuinely skips content.
    // Models often send offset:0 or offset:1 meaning "from the start" (0- vs 1-based),
    // so treat offset <= 1 as full-file when combined with a covering limit.
    let offset = input.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let limit = input.get("limit").and_then(Value::as_u64);
    let line_count = std::fs::read_to_string(&path)
        .map(|content| content.lines().count() as u64)
        .unwrap_or(u64::MAX);
    let has_partial_offset = offset > 1; // offset 0 or 1 = start of file
    let has_restrictive_limit = limit.is_some_and(|l| {
        let effective_remaining = line_count.saturating_sub(offset);
        l < effective_remaining
    });
    let is_partial_view =
        has_partial_offset || has_restrictive_limit || read_pages_field_is_present(input);
    state.claude_read_state.insert(
        path,
        ClaudeReadState {
            timestamp_ms,
            is_partial_view,
        },
    );
    Ok(())
}

fn read_field_is_present(input: &Value, field: &str) -> bool {
    !matches!(input.get(field), None | Some(Value::Null))
}

fn read_pages_field_is_present(input: &Value) -> bool {
    match input.get("pages") {
        None | Some(Value::Null) => false,
        Some(Value::String(value)) => !value.trim().is_empty(),
        Some(_) => true,
    }
}

fn mark_fully_read(state: &mut AppState, path: &Path) -> Result<()> {
    let timestamp_ms = file_timestamp_ms(path)?;
    state.claude_read_state.insert(
        path.to_path_buf(),
        ClaudeReadState {
            timestamp_ms,
            is_partial_view: false,
        },
    );
    Ok(())
}

fn enforce_read_precondition(state: &AppState, path: Option<&Path>) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let Some(snapshot) = state.claude_read_state.get(path) else {
        bail!("File has not been read yet. Read it first before writing to it.");
    };
    if snapshot.is_partial_view {
        bail!("File has not been read yet. Read it first before writing to it.");
    }
    if state.config.remote_tool_runner.is_some() {
        return Ok(());
    }
    let timestamp_ms = file_timestamp_ms(path)?;
    if timestamp_ms > snapshot.timestamp_ms {
        bail!(
            "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it."
        );
    }
    Ok(())
}

fn file_timestamp_ms(path: &Path) -> Result<u128> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat file {}", path.display()))?;
    let modified = metadata
        .modified()
        .with_context(|| format!("failed to read mtime for {}", path.display()))?;
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .with_context(|| format!("mtime for {} predates UNIX_EPOCH", path.display()))?;
    Ok(duration.as_millis())
}

pub fn execute_workflow_tool(
    state: &mut AppState,
    resources: &LoadedResources,
    cwd: &Path,
    tool_id: &str,
    input: Value,
    structured_output: Option<&StructuredOutputConfig>,
) -> Result<String> {
    match tool_id {
        "Agent" => workflow::agent::execute_agent(state, cwd, input),
        "AskUserQuestion" => {
            workflow::ask_user_question::execute_ask_user_question(state, cwd, input)
        }
        "Config" => workflow::config::execute_config(state, cwd, input),
        "CronCreate" => workflow::cron_create::execute_cron_create(state, cwd, input),
        "CronDelete" => workflow::cron_delete::execute_cron_delete(state, cwd, input),
        "CronList" => workflow::cron_list::execute_cron_list(state, cwd, input),
        "EmailConfigure" => workflow::email_configure::execute_email_configure(state, cwd, input),
        "EnterPlanMode" => workflow::enter_plan_mode::execute_enter_plan_mode(state, cwd, input),
        "EnterWorktree" => workflow::enter_worktree::execute_enter_worktree(state, cwd, input),
        "ExitPlanMode" => workflow::exit_plan_mode::execute_exit_plan_mode(state, cwd, input),
        "ExitWorktree" => workflow::exit_worktree::execute_exit_worktree(state, cwd, input),
        "LSP" => workflow::lsp::execute_lsp(state, resources, cwd, input),
        "PowerShell" => workflow::powershell::execute_powershell(state, cwd, input),
        "SendMessage" => workflow::send_message::execute_send_message(state, cwd, input),
        "SendUserMessage" | "Brief" => {
            workflow::send_user_message::execute_send_user_message(state, cwd, input)
        }
        "StructuredOutput" => workflow::structured_output::execute_structured_output(
            state,
            cwd,
            input,
            structured_output,
        ),
        "SubscriberInstall" => {
            workflow::subscriber_install::execute_subscriber_install(state, cwd, input)
        }
        "SubscriberList" => workflow::subscriber_list::execute_subscriber_list(state, cwd, input),
        "SubscriberScaffold" => {
            workflow::subscriber_scaffold::execute_subscriber_scaffold(state, cwd, input)
        }
        "SubscriptionCreate" => {
            workflow::subscription_create::execute_subscription_create(state, cwd, input)
        }
        "SubscriptionDelete" => {
            workflow::subscription_delete::execute_subscription_delete(state, cwd, input)
        }
        "SubscriptionList" => {
            workflow::subscription_list::execute_subscription_list(state, cwd, input)
        }
        "SubscriptionPause" => {
            workflow::subscription_pause::execute_subscription_pause(state, cwd, input)
        }
        "WorkflowRegister" => {
            workflow::workflow_register::execute_workflow_register(state, cwd, input)
        }
        "TaskCreate" => workflow::task_create::execute_task_create(state, cwd, input),
        "TaskGet" => workflow::task_get::execute_task_get(state, cwd, input),
        "TaskList" => workflow::task_list::execute_task_list(state, cwd, input),
        "TaskOutput" => workflow::task_output::execute_task_output(state, cwd, input),
        "TaskStop" => workflow::task_stop::execute_task_stop(state, cwd, input),
        "TaskUpdate" => workflow::task_update::execute_task_update(state, cwd, input),
        "TeamCreate" => workflow::team_create::execute_team_create(state, cwd, input),
        "TeamDelete" => workflow::team_delete::execute_team_delete(state, cwd, input),
        "TelegramLoginStart" => {
            workflow::telegram_login::execute_telegram_login_start(state, cwd, input)
        }
        "TelegramLoginSubmitCode" => {
            workflow::telegram_login::execute_telegram_login_submit_code(state, cwd, input)
        }
        "TelegramLoginSubmitPassword" => {
            workflow::telegram_login::execute_telegram_login_submit_password(state, cwd, input)
        }
        "TodoWrite" => workflow::todo_write::execute_todo_write(state, cwd, input),
        other => bail!("workflow tool `{other}` is not implemented"),
    }
}

impl<'a> ProviderToolContext<'a> {
    fn structured_output(self) -> Option<&'a StructuredOutputConfig> {
        match self {
            Self::OpenAI {
                structured_output, ..
            }
            | Self::Anthropic {
                structured_output, ..
            } => structured_output,
            Self::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn blank_pages_do_not_make_read_partial() {
        let input = json!({
            "file_path": "/tmp/demo.txt",
            "pages": "   ",
        });

        assert!(is_full_read_request(&input));
        assert!(!read_pages_field_is_present(&input));
    }

    #[test]
    fn null_optional_read_fields_are_treated_as_absent() {
        let input = json!({
            "file_path": "/tmp/demo.txt",
            "offset": null,
            "limit": null,
            "pages": null,
        });

        assert!(is_full_read_request(&input));
        assert!(!read_field_is_present(&input, "offset"));
        assert!(!read_field_is_present(&input, "limit"));
    }
}
