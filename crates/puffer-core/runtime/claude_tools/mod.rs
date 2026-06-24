use crate::permissions::FilesystemPermissionPolicy;
use crate::runner_adapter;
use crate::runtime::structured_output_support::StructuredOutputConfig;
use crate::state::ClaudeReadState;
use crate::AppState;
use anyhow::{bail, Context, Result};
use puffer_config::ProxyConfig;
use puffer_provider_openai::OpenAIRequestConfig;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
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
pub(crate) mod bash_internal_permissions;
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
pub mod write_stdin;

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
        proxy: &'a ProxyConfig,
        structured_output: Option<&'a StructuredOutputConfig>,
    },
    Anthropic {
        request_config: &'a AnthropicRequestConfig,
        model_id: &'a str,
        proxy: &'a ProxyConfig,
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
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    registry: &ToolRegistry,
    definition: &ToolDefinition,
    cwd: &Path,
    filesystem_policy: &FilesystemPermissionPolicy,
    input: Value,
    provider_context: ProviderToolContext<'_>,
) -> Result<ToolExecutionResult> {
    let skip_runner = definition.id == "Bash";
    if !skip_runner && runner_adapter::is_runner_supported(definition.id.as_str()) {
        if let Some(result) =
            try_runner_dispatch(state, definition, cwd, &input, filesystem_policy)?
        {
            return Ok(result);
        }
    }
    match definition.id.as_str() {
        "Bash" => {
            let internal_media_discovery_cache = state
                .exact_media_discovery_cache
                .clone()
                .unwrap_or_else(puffer_media::ExactMediaDiscoveryCache::empty);
            let session_id = state.session.id;
            let process_store = state.process_store.clone();
            let mut internal_permission_handler = |request| match request {
                bash_internal_permissions::InternalToolBrokerRequest::Permission(request) => {
                    bash_internal_permissions::InternalToolBrokerResponse::Permission(
                        super::internal_tool_permissions::resolve_internal_tool_permission(
                            state, resources, registry, cwd, request,
                        ),
                    )
                }
                bash_internal_permissions::InternalToolBrokerRequest::Execution(request) => {
                    bash_internal_permissions::InternalToolBrokerResponse::Execution(
                        super::internal_tool_permissions::execute_internal_tool_request(
                            state,
                            resources,
                            registry,
                            providers,
                            auth_store,
                            &internal_media_discovery_cache,
                            cwd,
                            request,
                        ),
                    )
                }
            };
            let execution = bash::execute_from_value_with_internal_permissions(
                cwd,
                &session_id,
                input,
                Some(&process_store),
                Some(&mut internal_permission_handler),
            )?;
            let output = serde_json::to_string_pretty(&execution.output)
                .context("failed to serialize Bash output")?;
            Ok(tool_result(definition, execution.success, output))
        }
        "WriteStdin" => {
            let (success, output) = write_stdin::execute(&state.process_store, input)?;
            Ok(tool_result(definition, success, output))
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
                &filesystem_policy.workspace_roots,
                &filesystem_policy.runner_policy(),
                input.clone(),
            )?;
            record_read_from_input(state, &input)?;
            Ok(tool_result(definition, true, output))
        }
        "Write" => {
            let mut read_state = clone_read_state(state);
            let output = write::execute_claude_write_tool(
                cwd,
                &filesystem_policy.workspace_roots,
                &filesystem_policy.runner_policy(),
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
                &filesystem_policy.workspace_roots,
                &filesystem_policy.runner_policy(),
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
            glob::execute_claude_glob(
                cwd,
                &filesystem_policy.workspace_roots,
                &filesystem_policy.runner_policy(),
                input,
            )?,
        )),
        "Grep" => Ok(tool_result(
            definition,
            true,
            grep::execute_claude_grep(
                cwd,
                &filesystem_policy.workspace_roots,
                &filesystem_policy.runner_policy(),
                input,
            )?,
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
                &filesystem_policy.workspace_roots,
                &filesystem_policy.runner_policy(),
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
            skill::execute_claude_skill_tool(state, resources, input)?,
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
                state,
                resources,
                registry,
                definition,
                cwd,
                filesystem_policy,
                input,
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
                    proxy,
                    ..
                } => web_search::execute_claude_openai_web_search(
                    request_config,
                    model_id,
                    input,
                    proxy,
                )?,
                ProviderToolContext::Anthropic {
                    request_config,
                    model_id,
                    proxy,
                    ..
                } => web_search::execute_claude_anthropic_web_search(
                    request_config,
                    model_id,
                    input,
                    proxy,
                )?,
                ProviderToolContext::None => {
                    bail!("WebSearch requires provider execution context")
                }
            };
            Ok(tool_result(definition, true, output))
        }
        _ if definition.handler.starts_with("runtime:workflow:") => {
            let workflow_media_discovery_cache = state
                .exact_media_discovery_cache
                .clone()
                .unwrap_or_else(puffer_media::ExactMediaDiscoveryCache::empty);
            let stdout = execute_workflow_tool_with_media_context(
                state,
                resources,
                Some(workflow::image_generation::ImageGenerationMediaContext {
                    providers,
                    auth_store,
                    discovery_cache: &workflow_media_discovery_cache,
                }),
                Some(workflow::video_generation::VideoGenerationMediaContext {
                    providers,
                    auth_store,
                    discovery_cache: &workflow_media_discovery_cache,
                }),
                cwd,
                definition.id.as_str(),
                input,
                provider_context.structured_output(),
            )?;
            // Some workflow tools want to set `metadata.terminate = true`
            // so the agent loop ends the turn after their result is
            // delivered to the model — pi-mono parity for
            // `AgentToolResult.terminate`. The post-process is opt-in
            // per tool id; default is `Value::Null` for everything else.
            let metadata = workflow_terminate_metadata(definition.id.as_str(), &stdout);
            Ok(tool_result_with_metadata(
                definition, true, stdout, metadata,
            ))
        }
        _ if definition.handler == "runtime:project_memory" => Ok(tool_result(
            definition,
            true,
            crate::memory::execute_memory_tool(state, input)?,
        )),
        _ if super::local_tools::is_runtime_local_tool(definition) => Ok(tool_result(
            definition,
            true,
            super::local_tools::execute_runtime_local_tool(
                state,
                resources,
                registry,
                definition,
                cwd,
                filesystem_policy,
                input,
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
/// `Glob | Grep | WebFetch` in the parallel path), execution is routed through the supplied
/// `Arc<dyn ToolRunner>` so a `RemoteToolRunner` can intercept parallel
/// batches the same way it intercepts serial calls. The remaining
/// parallel-safe tools (`WebSearch | ToolSearch`) intentionally stay on the
/// in-process path: WebSearch needs provider context that isn't on the runner
/// trait, and ToolSearch is local-only.
pub(crate) fn execute_parallel_tool(
    definition: &ToolDefinition,
    cwd: &Path,
    working_dirs: &[PathBuf],
    filesystem_policy: &FilesystemPermissionPolicy,
    session_id: &Uuid,
    input: Value,
    _resources: &LoadedResources,
    registry: &ToolRegistry,
    provider_context: &ProviderToolContext<'_>,
    runner: &Arc<dyn ToolRunner>,
) -> Result<ToolExecutionResult> {
    if runner_adapter::is_runner_supported(definition.id.as_str()) {
        let request = RunnerToolRequest {
            tool_id: definition.id.clone(),
            cwd: cwd.to_path_buf(),
            working_dirs: working_dirs.to_vec(),
            filesystem: filesystem_policy.runner_policy(),
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
                    proxy,
                    ..
                } => web_search::execute_claude_openai_web_search(
                    request_config,
                    model_id,
                    input,
                    proxy,
                )?,
                ProviderToolContext::Anthropic {
                    request_config,
                    model_id,
                    proxy,
                    ..
                } => web_search::execute_claude_anthropic_web_search(
                    request_config,
                    model_id,
                    input,
                    proxy,
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
    filesystem_policy: &FilesystemPermissionPolicy,
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
        working_dirs: filesystem_policy.workspace_roots.clone(),
        filesystem: filesystem_policy.runner_policy(),
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
    tool_result_with_metadata(definition, success, stdout, Value::Null)
}

/// Build a `ToolExecutionResult` with explicit metadata. The metadata
/// is what the tool-batch dispatcher inspects for `"terminate": true`
/// (see `runtime/tool_batch.rs::extract_terminate`) — used by tools
/// that want to end the turn after their result is delivered.
fn tool_result_with_metadata(
    definition: &ToolDefinition,
    success: bool,
    stdout: String,
    metadata: Value,
) -> ToolExecutionResult {
    ToolExecutionResult {
        tool_id: definition.id.clone(),
        success,
        output: ToolOutput {
            stdout,
            stderr: String::new(),
            metadata,
        },
    }
}

/// Decide whether a workflow tool's result should carry
/// `metadata.terminate = true`. Today only `update_goal` opts in,
/// and only when the model successfully marked the goal `complete`
/// (we sniff the JSON response since the workflow handler returns
/// pretty-printed JSON). Pi-mono pattern: tools that mark a unit of
/// work done can short-circuit the next provider round-trip.
///
/// Returning `Value::Null` is a no-op and matches the historical
/// behavior for every other workflow tool.
fn workflow_terminate_metadata(tool_id: &str, stdout: &str) -> Value {
    if tool_id != "update_goal" {
        return Value::Null;
    }
    // Cheap parse — workflow handlers always emit pretty-printed
    // JSON. If parsing fails (shouldn't, but defensive) treat it
    // as no-terminate.
    let Ok(parsed) = serde_json::from_str::<Value>(stdout) else {
        return Value::Null;
    };
    if parsed
        .pointer("/goal/status")
        .and_then(Value::as_str)
        .map(|s| s == "complete")
        .unwrap_or(false)
    {
        serde_json::json!({ "terminate": true })
    } else {
        Value::Null
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
        // defense-in-depth: pre-flight gate at mod.rs:402 catches first; keep messages aligned with StalenessRejection
        bail!(StalenessRejection::PARTIAL_READ_MESSAGE);
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
    execute_workflow_tool_with_media_context(
        state,
        resources,
        None,
        None,
        cwd,
        tool_id,
        input,
        structured_output,
    )
}

fn execute_workflow_tool_with_media_context(
    state: &mut AppState,
    resources: &LoadedResources,
    image_media_context: Option<workflow::image_generation::ImageGenerationMediaContext<'_>>,
    video_media_context: Option<workflow::video_generation::VideoGenerationMediaContext<'_>>,
    cwd: &Path,
    tool_id: &str,
    input: Value,
    structured_output: Option<&StructuredOutputConfig>,
) -> Result<String> {
    match tool_id {
        "Agent" => workflow::agent::execute_agent(state, cwd, input),
        "AnthropicStream" => {
            workflow::anthropic_stream::execute_anthropic_stream(state, cwd, input)
        }
        "AskUserQuestion" => {
            workflow::ask_user_question::execute_ask_user_question(state, cwd, input)
        }
        "requestuserbrowseraction" | "RequestUserBrowserAction" => {
            workflow::request_user_browser_action::execute_request_user_browser_action(
                state, cwd, input,
            )
        }
        "Canvas" => workflow::canvas::execute_canvas(state, cwd, input),
        "CanvasState" => workflow::canvas_state::execute_canvas_state(state, cwd, input),
        "ComfyUiAction" => workflow::comfyui_action::execute_comfyui_action(state, cwd, input),
        "ComputerUseAction" => {
            workflow::computer_use_action::execute_computer_use_action(state, cwd, input)
        }
        "Config" => workflow::config::execute_config(state, cwd, input),
        "ConnectionCreate" => {
            workflow::connector_tools::execute_connection_create(state, cwd, input)
        }
        "ConnectionDelete" => {
            workflow::connector_tools::execute_connection_delete(state, cwd, input)
        }
        "ConnectionList" => workflow::connector_tools::execute_connection_list(state, cwd, input),
        "ConnectorAct" => workflow::connector_tools::execute_connector_act(state, cwd, input),
        "ConnectorCreation" => {
            workflow::connector_tools::execute_connector_creation(state, cwd, input)
        }
        "ConnectorDelete" => workflow::connector_tools::execute_connector_delete(state, cwd, input),
        "ConnectorList" => workflow::connector_tools::execute_connector_list(state, cwd, input),
        "ConnectorRegister" => {
            workflow::connector_tools::execute_connector_register(state, cwd, input)
        }
        "ConnectorTemplate" => {
            workflow::connector_tools::execute_connector_template(state, cwd, input)
        }
        "ConnectorUpdate" => workflow::connector_tools::execute_connector_update(state, cwd, input),
        "CronCreate" => workflow::cron_create::execute_cron_create(state, cwd, input),
        "CronDelete" => workflow::cron_delete::execute_cron_delete(state, cwd, input),
        "CronList" => workflow::cron_list::execute_cron_list(state, cwd, input),
        "Email" => workflow::email_configure::execute_email(state, cwd, input),
        "EmailConfigure" => workflow::email_configure::execute_email_configure(state, cwd, input),
        "EnterPlanMode" => workflow::enter_plan_mode::execute_enter_plan_mode(state, cwd, input),
        "EnterWorktree" => workflow::enter_worktree::execute_enter_worktree(state, cwd, input),
        "ExitPlanMode" => workflow::exit_plan_mode::execute_exit_plan_mode(state, cwd, input),
        "ExitWorktree" => workflow::exit_worktree::execute_exit_worktree(state, cwd, input),
        "get_goal" => workflow::goal::execute_get_goal(state, cwd, input),
        "create_goal" => workflow::goal::execute_create_goal(state, cwd, input),
        "update_goal" => workflow::goal::execute_update_goal(state, cwd, input),
        "HttpRequest" => workflow::http_request::execute_http_request(state, cwd, input),
        "ImageGeneration" => workflow::image_generation::execute_image_generation(
            state,
            cwd,
            input,
            image_media_context,
        ),
        "VideoGeneration" => workflow::video_generation::execute_video_generation(
            state,
            cwd,
            input,
            video_media_context,
        ),
        "DebugpyAction" => workflow::debugpy_action::execute_debugpy_action(state, cwd, input),
        "DiscordAction" => workflow::discord_action::execute_discord_action(state, cwd, input),
        "LSP" => workflow::lsp::execute_lsp(state, resources, cwd, input),
        "McpToolCall" => workflow::mcp_tool_call::execute_mcp_tool_call(state, cwd, input),
        "McpStatus" => workflow::mcp_status::execute_mcp_status(state, cwd, input),
        "ModalAction" => workflow::modal_action::execute_modal_action(state, cwd, input),
        "MonitorActionDraft" => {
            workflow::monitor_action_draft::execute_monitor_action_draft(state, cwd, input)
        }
        "MonitorReplyDraft" => {
            workflow::monitor_reply_draft::execute_monitor_reply_draft(state, cwd, input)
        }
        "MonitorReplySend" => {
            workflow::monitor_reply_send::execute_monitor_reply_send(state, cwd, input)
        }
        "NativeMcpAction" => {
            workflow::native_mcp_action::execute_native_mcp_action(state, cwd, input)
        }
        "ProcessControl" => workflow::process_control::execute_process_control(state, cwd, input),
        "PowerShell" => workflow::powershell::execute_powershell(state, cwd, input),
        "Recall" => workflow::recall::execute_recall(state, cwd, input),
        "Remember" => workflow::remember::execute_remember(state, cwd, input),
        "RequestSecret" => workflow::request_secret::execute_request_secret(state, cwd, input),
        "SecretValue" => workflow::secret_value::execute_secret_value(state, cwd, input),
        "SendMessage" => workflow::send_message::execute_send_message(state, cwd, input),
        "SendUserMessage" | "Brief" => {
            workflow::send_user_message::execute_send_user_message(state, cwd, input)
        }
        "ShopifyAction" => workflow::shopify_action::execute_shopify_action(state, cwd, input),
        "Slack" => workflow::slack::execute_slack(state, cwd, input),
        "SlackAction" => workflow::slack_action::execute_slack_action(state, cwd, input),
        "SpotifyAction" => workflow::spotify_action::execute_spotify_action(state, cwd, input),
        "StructuredOutput" => workflow::structured_output::execute_structured_output(
            state,
            cwd,
            input,
            structured_output,
        ),
        "VisionAnalyze" => workflow::vision_analyze::execute_vision_analyze(state, cwd, input),
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
        "TouchDesignerAction" => {
            workflow::touchdesigner_action::execute_touchdesigner_action(state, cwd, input)
        }
        "WorkflowInspect" => workflow::workflow_tools::execute_workflow_inspect(state, cwd, input),
        "WorkflowList" => workflow::workflow_tools::execute_workflow_list(state, cwd, input),
        "WorkflowToggle" => workflow::workflow_tools::execute_workflow_toggle(state, cwd, input),
        "WorkflowCreate" => workflow::workflow_tools::execute_workflow_create(state, cwd, input),
        "WorkflowValidate" => {
            workflow::workflow_tools::execute_workflow_validate(state, cwd, input)
        }
        "TaskCreate" => workflow::task_create::execute_task_create(state, cwd, input),
        "TaskFlow" => workflow::task_flow::execute_task_flow(state, cwd, input),
        "TaskGet" => workflow::task_get::execute_task_get(state, cwd, input),
        "TaskList" => workflow::task_list::execute_task_list(state, cwd, input),
        "TaskOutput" => workflow::task_output::execute_task_output(state, cwd, input),
        "TaskStop" => workflow::task_stop::execute_task_stop(state, cwd, input),
        "TaskUpdate" => workflow::task_update::execute_task_update(state, cwd, input),
        "TeamCreate" => workflow::team_create::execute_team_create(state, cwd, input),
        "TeamDelete" => workflow::team_delete::execute_team_delete(state, cwd, input),
        "Telegram" => workflow::telegram_login::execute_telegram(state, cwd, input),
        "TelegramImportDesktop" => {
            workflow::telegram_login::execute_telegram_import_desktop(state, cwd, input)
        }
        "TelegramLoginQr" => workflow::telegram_login::execute_telegram_login_qr(state, cwd, input),
        "TelegramLoginQrWait" => {
            workflow::telegram_login::execute_telegram_login_qr_wait(state, cwd, input)
        }
        "TelegramLoginStart" => {
            workflow::telegram_login::execute_telegram_login_start(state, cwd, input)
        }
        "TelegramLoginSubmitCode" => {
            workflow::telegram_login::execute_telegram_login_submit_code(state, cwd, input)
        }
        "TelegramLoginSubmitPassword" => {
            workflow::telegram_login::execute_telegram_login_submit_password(state, cwd, input)
        }
        "LambdaInternal" => workflow::lambda_internal::execute_lambda_internal(state, cwd, input),
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
mod tests;
