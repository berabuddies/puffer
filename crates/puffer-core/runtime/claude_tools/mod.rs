use crate::runner_adapter;
use crate::runtime::structured_output_support::StructuredOutputConfig;
use crate::state::ClaudeReadState;
use crate::workspace_paths;
use crate::AppState;
use anyhow::{bail, Context, Result};
use puffer_provider_openai::OpenAIRequestConfig;
use puffer_resources::LoadedResources;
use puffer_runner_api::{
    check_read_freshness, NullChunkSink, ReadStateSnapshot, ReadStateUpdate, StalenessRejection,
    ToolRequest as RunnerToolRequest, ToolRunner,
};
use puffer_tools::{ToolDefinition, ToolExecutionResult, ToolOutput, ToolRegistry};
use puffer_transport_anthropic::AnthropicRequestConfig;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub(super) mod mcp_resources;
pub mod notebook_edit;
pub mod read;
pub(crate) mod skill;
pub(crate) mod tool_search;
pub mod web_fetch;
pub mod web_search;

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
pub mod write;

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
    if runner_adapter::is_runner_supported(definition.id.as_str()) {
        if let Some(result) = try_runner_dispatch(state, definition, cwd, &input, allow_all_paths)?
        {
            return Ok(result);
        }
    }
    match definition.id.as_str() {
        "Bash" => {
            let execution = bash::execute_from_value(cwd, &state.session.id, input)?;
            let output = serde_json::to_string_pretty(&execution.output)
                .context("failed to serialize Bash output")?;
            Ok(tool_result(definition, execution.success, output))
        }
        "Read" => {
            if is_full_read_request(&input) {
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
///
/// For tools in `runner_adapter::is_runner_supported(...)` (currently
/// `Bash | Glob | Grep | WebFetch`), execution is routed through the supplied
/// `Arc<dyn ToolRunner>` so a `RemoteToolRunner` can intercept parallel
/// batches the same way it intercepts serial calls. The remaining
/// parallel-safe tools (`WebSearch | ToolSearch | Skill`) intentionally stay
/// on the in-process path: WebSearch needs provider context that isn't on
/// the runner trait, and Skill / ToolSearch are local-only.
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
    runner: &Arc<dyn ToolRunner>,
) -> Result<ToolExecutionResult> {
    if runner_adapter::is_runner_supported(definition.id.as_str()) {
        let request = RunnerToolRequest {
            tool_id: definition.id.clone(),
            cwd: cwd.to_path_buf(),
            working_dirs: working_dirs.to_vec(),
            allow_all_paths,
            input: input.clone(),
            session_id: Some(session_id.to_string()),
        };
        let mut sink = NullChunkSink;
        let outcome = runner
            .execute_tool(request, &mut sink)
            .map_err(|e| anyhow::anyhow!(e))?;
        // Parallel-safe tools never touch read-state (Read/Write/Edit/NotebookEdit
        // are excluded by `is_parallel_safe_tool`), so any updates returned
        // here would be a runner bug. Assert in debug, ignore in release.
        debug_assert!(
            outcome.read_state_updates.is_empty(),
            "parallel-safe tool {} returned read_state_updates",
            definition.id
        );
        return Ok(ToolExecutionResult {
            tool_id: outcome.tool_id,
            success: outcome.success,
            output: ToolOutput {
                stdout: outcome.stdout,
                stderr: outcome.stderr,
                metadata: outcome.metadata,
            },
        });
    }
    match definition.id.as_str() {
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

/// Tries dispatching the call through the active [`puffer_runner_api::ToolRunner`].
///
/// Returns `Ok(Some(result))` when the runner handled the call (success or
/// pre-flight rejection), `Ok(None)` when the tool needs the legacy in-place
/// path (e.g. WebSearch's provider context, or Read's "file unchanged"
/// short-circuit), and `Err` when the underlying execution fails.
fn try_runner_dispatch(
    state: &mut AppState,
    definition: &ToolDefinition,
    cwd: &Path,
    input: &Value,
    allow_all_paths: bool,
) -> Result<Option<ToolExecutionResult>> {
    let tool_id = definition.id.as_str();

    // Read keeps its "file_unchanged" short-circuit on the legacy path —
    // the runner DTO doesn't model that bookkeeping yet.
    if tool_id == "Read" && is_full_read_request(input) {
        if let Some(path) = input_file_path(input, "file_path")? {
            if let Some(snapshot) = state.claude_read_state.get(&path) {
                let timestamp_ms = file_timestamp_ms(&path)?;
                if !snapshot.is_partial_view && timestamp_ms == snapshot.timestamp_ms {
                    return Ok(None);
                }
            }
        }
    }

    // Pre-flight staleness gate, hoisted out of the per-tool implementations.
    let needs_freshness_check = matches!(tool_id, "Write" | "NotebookEdit")
        || (tool_id == "Edit" && edit::requires_prior_read(input));
    if needs_freshness_check {
        let path_field = if tool_id == "NotebookEdit" {
            "notebook_path"
        } else {
            "file_path"
        };
        if let Some(path) = input_file_path(input, path_field)? {
            let snapshot = state
                .claude_read_state
                .get(&path)
                .map(|snap| ReadStateSnapshot {
                    timestamp_ms: snap.timestamp_ms,
                    is_partial_view: snap.is_partial_view,
                });
            // Only enforce when the file already exists; Write/Edit on a
            // brand-new path are allowed without a prior Read.
            if path.exists() {
                let current_mtime = file_timestamp_ms(&path)?;
                if let Err(rejection) = check_read_freshness(snapshot.as_ref(), current_mtime) {
                    return Ok(Some(staleness_failure(definition, &rejection)));
                }
            }
        }
    }

    let request = RunnerToolRequest {
        tool_id: tool_id.to_string(),
        cwd: cwd.to_path_buf(),
        working_dirs: state.working_dirs.clone(),
        allow_all_paths,
        input: input.clone(),
        session_id: Some(state.session.id.to_string()),
    };
    let runner = state.tool_runner.clone();
    let mut sink = NullChunkSink;
    let outcome = runner
        .execute_tool(request, &mut sink)
        .map_err(|e| anyhow::anyhow!(e))?;

    apply_read_state_updates(state, &outcome.read_state_updates);

    Ok(Some(ToolExecutionResult {
        tool_id: outcome.tool_id,
        success: outcome.success,
        output: ToolOutput {
            stdout: outcome.stdout,
            stderr: outcome.stderr,
            metadata: outcome.metadata,
        },
    }))
}

fn apply_read_state_updates(state: &mut AppState, updates: &[ReadStateUpdate]) {
    for update in updates {
        state.claude_read_state.insert(
            update.path.clone(),
            ClaudeReadState {
                timestamp_ms: update.timestamp_ms,
                is_partial_view: update.is_partial_view,
            },
        );
    }
}

fn staleness_failure(
    definition: &ToolDefinition,
    rejection: &StalenessRejection,
) -> ToolExecutionResult {
    ToolExecutionResult {
        tool_id: definition.id.clone(),
        success: false,
        output: ToolOutput {
            stdout: rejection.message().to_string(),
            stderr: String::new(),
            metadata: Value::Null,
        },
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

fn input_file_path(input: &Value, field: &str) -> Result<Option<PathBuf>> {
    Ok(input.get(field).and_then(Value::as_str).map(PathBuf::from))
}

fn is_full_read_request(input: &Value) -> bool {
    !read_field_is_present(input, "offset")
        && !read_field_is_present(input, "limit")
        && !read_pages_field_is_present(input)
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
    use puffer_runner_api::{
        ChunkSink, DirEntry, McpPrompt, McpPromptContent, McpResourceContent, McpResourceRecord,
        McpResult, McpServerInfo, McpTool, RunnerCapabilities, RunnerError, ToolRequest,
        ToolResult, ToolRunner,
    };
    use puffer_resources::LoadedResources;
    use puffer_tools::{
        ToolDefinition, ToolDisplayHints, ToolInputSchema, ToolKind, ToolMetadata,
        ToolPolicyHints, ToolRegistry,
    };
    use serde_json::json;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use uuid::Uuid;

    /// Records every `execute_tool` call and forwards execution to an inner
    /// `LocalToolRunner`. Used to prove that the parallel-batch path actually
    /// dispatches through the trait instead of bypassing it.
    #[derive(Debug)]
    struct RecordingRunner {
        inner: Arc<dyn ToolRunner>,
        execute_calls: AtomicUsize,
    }

    impl RecordingRunner {
        fn new(inner: Arc<dyn ToolRunner>) -> Self {
            Self {
                inner,
                execute_calls: AtomicUsize::new(0),
            }
        }

        fn execute_calls(&self) -> usize {
            self.execute_calls.load(Ordering::SeqCst)
        }
    }

    impl ToolRunner for RecordingRunner {
        fn ping(&self) -> Result<puffer_runner_api::RunnerPing, RunnerError> {
            self.inner.ping()
        }
        fn capabilities(&self) -> RunnerCapabilities {
            self.inner.capabilities()
        }
        fn execute_tool(
            &self,
            req: ToolRequest,
            sink: &mut dyn ChunkSink,
        ) -> Result<ToolResult, RunnerError> {
            self.execute_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.execute_tool(req, sink)
        }
        fn read_file(&self, path: &Path) -> Result<Vec<u8>, RunnerError> {
            self.inner.read_file(path)
        }
        fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, RunnerError> {
            self.inner.list_dir(path)
        }
        fn glob(
            &self,
            root: &Path,
            pattern: &str,
        ) -> Result<Vec<std::path::PathBuf>, RunnerError> {
            self.inner.glob(root, pattern)
        }
        fn list_mcp_servers(&self) -> Result<Vec<McpServerInfo>, RunnerError> {
            self.inner.list_mcp_servers()
        }
        fn list_mcp_tools(&self, server: &str) -> Result<Vec<McpTool>, RunnerError> {
            self.inner.list_mcp_tools(server)
        }
        fn call_mcp_tool(
            &self,
            server: &str,
            tool: &str,
            args: serde_json::Value,
            sink: &mut dyn ChunkSink,
        ) -> Result<McpResult, RunnerError> {
            self.inner.call_mcp_tool(server, tool, args, sink)
        }
        fn list_mcp_resources(
            &self,
            server: Option<&str>,
        ) -> Result<Vec<McpResourceRecord>, RunnerError> {
            self.inner.list_mcp_resources(server)
        }
        fn read_mcp_resource(
            &self,
            server: &str,
            uri: &str,
        ) -> Result<McpResourceContent, RunnerError> {
            self.inner.read_mcp_resource(server, uri)
        }
        fn list_mcp_prompts(&self, server: &str) -> Result<Vec<McpPrompt>, RunnerError> {
            self.inner.list_mcp_prompts(server)
        }
        fn get_mcp_prompt(
            &self,
            server: &str,
            name: &str,
            args: serde_json::Value,
        ) -> Result<McpPromptContent, RunnerError> {
            self.inner.get_mcp_prompt(server, name, args)
        }
    }

    /// Verifies the parallel-tool path routes runner-supported tools through
    /// `Arc<dyn ToolRunner>::execute_tool` instead of calling in-process
    /// helpers directly. This is the regression test for the gap where a
    /// parallel batch of two Bash calls bypassed `RemoteToolRunner` even
    /// though a single serial Bash call went through it.
    #[test]
    fn parallel_path_dispatches_through_runner() {
        let inner: Arc<dyn ToolRunner> = Arc::new(crate::runner_adapter::LocalToolRunner::new());
        let recording = Arc::new(RecordingRunner::new(inner));
        let runner: Arc<dyn ToolRunner> = recording.clone();

        let resources = LoadedResources::default();
        let registry = ToolRegistry::default();
        let provider_context = ProviderToolContext::None;
        let session_id = Uuid::new_v4();
        let workspace = tempfile::tempdir().expect("tempdir");
        let cwd = workspace.path().to_path_buf();
        let working_dirs: Vec<std::path::PathBuf> = Vec::new();

        // Claude-parity tools use capitalized ids that the dispatcher
        // matches on; build minimal definitions directly so neither the
        // builtin lowercase `bash` nor a `runtime:` handler mismatch
        // perturbs the dispatch path under test.
        fn claude_tool_def(id: &str, handler: &str) -> ToolDefinition {
            ToolDefinition {
                id: id.to_string(),
                name: id.to_string(),
                description: id.to_string(),
                handler: handler.to_string(),
                aliases: Vec::new(),
                handler_args: Vec::new(),
                kind: ToolKind::Custom,
                input_schema: ToolInputSchema::default(),
                metadata: ToolMetadata::default(),
                policy: ToolPolicyHints::default(),
                shared_lib: None,
                enabled_if: None,
                display: ToolDisplayHints::default(),
            }
        }
        let bash_def = claude_tool_def("Bash", "runtime:claude_bash");
        let glob_def = claude_tool_def("Glob", "runtime:claude_glob");

        let bash_input = json!({"command": "echo parallel-runner"});
        let bash_result = execute_parallel_tool(
            &bash_def,
            &cwd,
            &working_dirs,
            true,
            &session_id,
            bash_input,
            &resources,
            &registry,
            &provider_context,
            &runner,
        )
        .expect("Bash through runner");
        assert!(bash_result.success, "Bash should succeed");
        assert!(
            bash_result.output.stdout.contains("parallel-runner"),
            "Bash stdout missing marker: {}",
            bash_result.output.stdout
        );

        let glob_input = json!({"pattern": "*"});
        let glob_result = execute_parallel_tool(
            &glob_def,
            &cwd,
            &working_dirs,
            true,
            &session_id,
            glob_input,
            &resources,
            &registry,
            &provider_context,
            &runner,
        )
        .expect("Glob through runner");
        assert!(glob_result.success, "Glob should succeed");

        assert_eq!(
            recording.execute_calls(),
            2,
            "expected the runner to be invoked once per parallel-safe runner-supported tool",
        );
    }

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
