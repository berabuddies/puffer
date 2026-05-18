use super::{
    execute_tool_call, is_parallel_safe_tool, parse_http_json_response, resolve_tool_permission,
    run_turn_hooks, send_http_request_raw, PermissionOutcome, ToolExecutionBackend, ToolInvocation,
    TurnStreamEvent, APP_VERSION, OPENAI_CODEX_COMPAT_VERSION,
};
use crate::permissions::{load_runtime_permission_context_with_inputs, RuntimePermissionInputs};
mod completions_session;
pub(crate) mod conversation;
mod responses_session;
mod support;
mod websocket;

pub(super) use self::support::build_codex_openai_request_body;
use self::support::{
    append_default_openai_headers, apply_previous_response_id, is_codex_openai_provider,
    is_openai_structured_output_error, openai_base_url_for_auth, openai_model_supports_reasoning,
    openai_registry_credential, openai_responses_path, openai_stream_read_timeout,
    openai_supports_response_threading, prefer_native_structured_output, retry_openai_transport,
    structured_output_endpoint_id, trace_openai_http_request, trace_openai_http_response_headers,
    OPENAI_STRUCTURED_OUTPUT_FAMILY,
};
pub(super) use self::websocket::{execute_openai_websocket_streaming, openai_websocket_enabled};
use super::structured_output_support::{
    openai_chat_completion_tools_for_request, openai_chat_response_format,
    openai_responses_text_config, openai_tool_definitions_for_request, StructuredOutputConfig,
};
use super::system_prompt::render_runtime_system_prompt;
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_provider_openai::{
    build_chat_completions_request, build_json_post_request, extract_chat_completions_text,
    extract_chat_completions_tool_calls, extract_responses_text, extract_responses_tool_calls,
    parse_chat_completions_response, refresh_oauth_token, OpenAIAuth, OpenAIChatCompletionsRequest,
    OpenAIRequestConfig, OpenAIResponseToolCall, OpenAIResponsesFunctionCallOutput,
    OpenAIResponsesResponse, OpenAIResponsesToolChoiceMode,
};
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry, StoredCredential};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;
use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufRead, Read};

pub(super) use super::openai_sse::{is_event_stream, parse_openai_sse_response};
use super::openai_sse::{parse_openai_sse_reader_typed, OpenAISseResult};

#[cfg(test)]
pub(super) use super::openai_sse::parse_openai_sse_response_streaming;

#[cfg(test)]
pub(super) use super::structured_output_support::openai_tool_definitions;

const OPENAI_CODEX_ORIGINATOR: &str = "codex_cli_rs";

#[derive(Debug, Clone)]
pub(super) struct OpenAIExecutionConfig {
    pub(super) provider_id: String,
    pub(super) request_config: OpenAIRequestConfig,
    pub(super) refresh_token: Option<String>,
    pub(super) codex_style: bool,
}

pub(super) struct OpenAIToolResults {
    pub(super) outputs: Vec<OpenAIResponsesFunctionCallOutput>,
    pub(super) invocations: Vec<ToolInvocation>,
}

fn openai_request_version(provider: &ProviderDescriptor, oauth: bool) -> String {
    if is_codex_openai_provider(provider) || (oauth && provider.id == "openai") {
        OPENAI_CODEX_COMPAT_VERSION.to_string()
    } else {
        APP_VERSION.to_string()
    }
}

/// Legacy non-`agent_loop` streaming path. **Only reachable from
/// `runtime/openai/websocket.rs`** (it falls back to this when
/// websocket negotiation fails or the env-flag points at SSE).
/// Production SSE traffic goes through `OpenAIResponsesAdapter` →
/// `agent_loop::run_streaming_loop` → `OpenAIResponsesTurnSession`.
///
/// Do not change this function in isolation — any behavior fix here
/// also needs mirroring in `responses_session.rs::one_turn_streaming`
/// (and vice-versa) until the websocket path is migrated to its own
/// `TurnSession` impl in a follow-up.
pub(super) fn execute_openai_streaming<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
    on_event: &mut F,
) -> Result<super::TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    let structured_output = options.structured_output;
    let use_native = prefer_native_structured_output(state, provider, &model_id, structured_output);
    match execute_openai_streaming_once(
        state,
        resources,
        providers,
        provider,
        model_id.clone(),
        auth_store,
        input,
        options.clone(),
        use_native,
        on_event,
    ) {
        Ok(turn) => Ok(turn),
        Err(error) if use_native && is_openai_structured_output_error(&error) => {
            state.mark_native_structured_output_unsupported(
                OPENAI_STRUCTURED_OUTPUT_FAMILY,
                provider.id.as_str(),
                &model_id,
                structured_output_endpoint_id(provider),
            );
            execute_openai_streaming_once(
                state, resources, providers, provider, model_id, auth_store, input, options, false,
                on_event,
            )
        }
        Err(error) => Err(error),
    }
}

fn execute_openai_streaming_once<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
    use_native: bool,
    on_event: &mut F,
) -> Result<super::TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    use self::conversation::{
        append_managed_system_prompt_1_to_instructions, append_reasoning_items,
        append_tool_results, compact_conversation, inject_post_compact_context,
        items_to_responses_input, managed_system_prompt_1_from_env, transcript_to_items,
        ConversationItem,
    };

    let structured_output = options.structured_output;
    let mut execution = resolve_openai_execution_config(state, auth_store, provider)?;
    let registry =
        super::mcp_discovery::registry_with_mcp_tools(resources, state.tool_runner.as_ref());
    let permission_context = load_runtime_permission_context_with_inputs(
        &state.cwd,
        resources,
        state,
        RuntimePermissionInputs {
            request_tool_filter: options.tool_filter.cloned(),
        },
    )?;
    let text = openai_responses_text_config(structured_output, use_native);
    let tools = openai_tool_definitions_for_request(
        &registry,
        structured_output,
        use_native,
        Some(&permission_context),
    )?;
    let mut instructions = if options.lightweight_context {
        "Reply directly and concisely.".to_string()
    } else {
        let system_prompt = render_runtime_system_prompt(
            state,
            resources,
            &model_id,
            &tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<std::collections::BTreeSet<_>>(),
        )?;
        openai_request_instructions(state, resources, Some(&system_prompt))?
    };
    // Unified: all internal logic on Vec<ConversationItem>.
    let mut items = transcript_to_items(state, input);
    let managed_system_prompt_1 = if options.lightweight_context {
        None
    } else {
        managed_system_prompt_1_from_env()
    };
    append_managed_system_prompt_1_to_instructions(
        &mut instructions,
        managed_system_prompt_1.as_deref(),
    );
    let mut reflection = options
        .reflection
        .map(|config| super::reflection::ReflectionTracker::new(input, config));
    let mut reflection_traces: Vec<super::ReflectionTraceEvent> = Vec::new();

    // Inject dynamic context as a user message at the start of the input
    // array (matching Codex/CC pattern).
    if !options.lightweight_context {
        let context_reminder = build_context_reminder_message(state);
        super::openai::conversation::insert_context_reminder_preserving_legacy_leading_system(
            &mut items,
            &context_reminder,
        );
    }

    let mut invocations = Vec::new();
    let supports_reasoning = openai_model_supports_reasoning(provider, &model_id);
    let model = provider.models.iter().find(|m| m.id == model_id);
    let supports_response_threading =
        openai_supports_response_threading(provider, &execution.request_config.base_url, model);
    let mut previous_response_id: Option<String> = None;
    // Index where "continuation" items start — used for previous_response_id optimization.
    // When previous_response_id is set, only items[start..] are sent as wire input.
    let mut continuation_start: Option<usize> = None;

    loop {
        // Check for background tasks that completed since the last turn and inject
        // a system reminder so the model learns about them without needing to poll.
        let completed = super::claude_tools::workflow::drain_completed_shell_tasks(
            &state.cwd,
            &state.session.id,
        );
        if !completed.is_empty() {
            let notice = format!(
                "<system-reminder>\n{}\nUse TaskOutput to retrieve the full output if needed.\n</system-reminder>",
                completed.join("\n")
            );
            items.push(ConversationItem::user_message(&notice));
        }

        // Wire boundary: ConversationItem → Responses API input.
        // When previous_response_id is set, only send continuation items.
        let wire_input = match (
            supports_response_threading,
            previous_response_id.as_ref(),
            continuation_start,
        ) {
            (true, Some(_), Some(start)) => items_to_responses_input(&items[start..]),
            _ => items_to_responses_input(&items),
        };

        let prev_resp_id = if supports_response_threading {
            previous_response_id.clone()
        } else {
            None
        };
        let mut retry_events: Vec<TurnStreamEvent> = Vec::new();
        let response = retry_openai_transport(
            || {
                send_openai_request_with_refresh_streaming(
                    auth_store,
                    &mut execution,
                    |request_config| {
                        let mut body = build_codex_openai_request_body(
                            state,
                            &model_id,
                            &instructions,
                            wire_input.clone(),
                            &tools,
                            supports_reasoning,
                            text.clone(),
                            true,
                        );
                        apply_previous_response_id(&mut body, prev_resp_id.as_deref());
                        build_json_post_request(
                            request_config,
                            openai_responses_path(&request_config.base_url),
                            &body,
                        )
                    },
                    on_event,
                )
            },
            |attempt, max, error| {
                retry_events.push(TurnStreamEvent::RetryAttempt {
                    attempt,
                    max_attempts: max,
                    error: error.to_string(),
                });
            },
        )?;
        for event in retry_events {
            on_event(event);
        }

        // Typed fields extracted during SSE — no Value→String→typed roundtrip.
        previous_response_id = if supports_response_threading {
            response.response_id
        } else {
            None
        };
        let input_tokens = response.input_tokens;
        if let Some(tokens) = input_tokens {
            state.last_input_tokens = Some(tokens as u32);
        }
        // Emit per-turn usage with cache hit data.
        if let Some(input) = response.input_tokens {
            let cached = response.cached_tokens.unwrap_or(0);
            let output = response.output_tokens.unwrap_or(0);
            state.update_cache_stats(input as u64, cached as u64);
            on_event(TurnStreamEvent::Usage(super::TurnUsageReport {
                input_tokens: input as u64,
                output_tokens: output as u64,
                cache_read_tokens: cached as u64,
                cache_creation_tokens: 0,
            }));
        }
        if response.tool_calls.is_empty() {
            // Final turn — extract assistant text (typed, with raw fallback).
            let assistant_text = if response.assistant_text.trim().is_empty() {
                parse_openai_text(&response.raw_response)
                    .or_else(|_| parse_openai_text_fallback(&response.raw_response, state))?
            } else {
                response.assistant_text
            };
            run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
            return Ok(super::TurnExecution {
                assistant_text,
                tool_invocations: invocations,
                reflection_traces,
            });
        }

        let tool_calls = response.tool_calls;
        let pending_tool_calls = tool_calls
            .iter()
            .filter(|tool_call| !response.emitted_tool_call_ids.contains(&tool_call.call_id))
            .map(|tool_call| super::ToolCallRequest {
                call_id: tool_call.call_id.clone(),
                tool_id: tool_call.name.clone(),
                input: serde_json::to_string(&tool_call.arguments).unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        if !pending_tool_calls.is_empty() {
            on_event(TurnStreamEvent::ToolCallsRequested(pending_tool_calls));
        }

        // Add assistant text from this round to maintain full history.
        if !response.assistant_text.trim().is_empty() {
            items.push(ConversationItem::assistant_message(
                &response.assistant_text,
            ));
        }
        // Preserve the model's reasoning chain (see non-streaming path above
        // for why this matters — proxies/models that don't support server-side
        // `previous_response_id` threading rely on us replaying the reasoning
        // items on every turn).
        append_reasoning_items(&mut items, &response.reasoning_items);
        // Record where continuation starts (tool calls + outputs for next request).
        continuation_start = Some(items.len());

        let cwd = state.cwd.clone();
        let tool_results = execute_openai_tool_calls(
            state,
            resources,
            providers,
            auth_store,
            &tool_calls,
            &registry,
            &cwd,
            &execution.request_config,
            &model_id,
            structured_output,
            options.tool_filter,
        )?;
        if !tool_results.invocations.is_empty() {
            on_event(TurnStreamEvent::ToolInvocations(
                tool_results.invocations.clone(),
            ));
        }

        // Shared: append tool calls + outputs to canonical items.
        append_tool_results(&mut items, &tool_results.invocations);
        if let Some(observation) = reflection.as_mut().and_then(|tracker| {
            tracker.observe_batch_with_judge(
                &tool_results.invocations,
                &items,
                state,
                resources,
                providers,
                auth_store,
                // Legacy openai path doesn't thread the agent loop's
                // cancel token in today; preserved as None. Same
                // follow-up note as websocket.rs.
                None,
            )
        }) {
            for trace_event in &observation.trace_events {
                on_event(TurnStreamEvent::ReflectionTrace(trace_event.clone()));
            }
            reflection_traces.extend(observation.trace_events);
            if let Some(checkpoint) = observation.checkpoint {
                on_event(TurnStreamEvent::ReflectionCheckpoint(
                    checkpoint.summary.clone(),
                ));
                items.push(ConversationItem::user_message(checkpoint.prompt));
            }
        }
        invocations.extend(tool_results.invocations);

        // Shared: unified compaction.
        let compacted = compact_conversation(
            &mut items,
            provider,
            &model_id,
            &execution.request_config,
            input_tokens,
        );
        if compacted {
            previous_response_id = None;
            continuation_start = None;
            inject_post_compact_context(&mut items, state);
        }
    }
}

pub(super) fn execute_openai_tool_calls(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    tool_calls: &[OpenAIResponseToolCall],
    registry: &ToolRegistry,
    cwd: &std::path::Path,
    request_config: &OpenAIRequestConfig,
    model_id: &str,
    structured_output: Option<&StructuredOutputConfig>,
    tool_filter: Option<&super::RequestToolFilter>,
) -> Result<OpenAIToolResults> {
    // Count how many parallel-safe tools we have.
    let parallel_count = tool_calls
        .iter()
        .filter(|tc| is_parallel_safe_tool(&tc.name))
        .count();

    // If 0-1 tool calls or nothing to parallelize, use the serial fast-path.
    if tool_calls.len() <= 1 || parallel_count <= 1 {
        return execute_openai_tool_calls_serial(
            state,
            resources,
            providers,
            auth_store,
            tool_calls,
            registry,
            cwd,
            request_config,
            model_id,
            structured_output,
            tool_filter,
        );
    }

    // ---------- Phase 1: Pre-resolve permissions for all tools (serial) ----------
    // Permission prompts require user interaction and &mut state (for AllowSession),
    // so they must be resolved before we enter the parallel phase.
    let mut permissions: Vec<PermissionOutcome> = Vec::with_capacity(tool_calls.len());
    for tc in tool_calls {
        permissions.push(resolve_tool_permission(
            state,
            resources,
            registry,
            cwd,
            &tc.name,
            &tc.arguments,
            tool_filter,
        )?);
    }

    // ---------- Phase 2: Execute tools ----------
    // Clone immutable data needed by parallel tools.
    let working_dirs = state.working_dirs.clone();
    let provider_context = super::claude_tools::ProviderToolContext::OpenAI {
        request_config,
        model_id,
        structured_output,
    };
    // Cloned before `thread::scope` so each worker can route through the
    // active `ToolRunner` (e.g. `RemoteToolRunner`) without touching `state`.
    let runner = state.tool_runner.clone();

    // Pre-allocate results array; each slot filled by either parallel or serial exec.
    let mut results: Vec<Option<(String, bool)>> = vec![None; tool_calls.len()];

    // Execute parallel-safe permitted tools concurrently.
    std::thread::scope(|s| {
        let mut handles: Vec<(usize, std::thread::ScopedJoinHandle<'_, (String, bool)>)> =
            Vec::new();
        for (i, tc) in tool_calls.iter().enumerate() {
            // Skip denied tools and non-parallel tools.
            if !is_parallel_safe_tool(&tc.name) {
                continue;
            }
            if let PermissionOutcome::Denied(ref denied) = permissions[i] {
                results[i] = Some((denied.output.stdout.clone(), denied.success));
                continue;
            }
            let filesystem_policy = match &permissions[i] {
                PermissionOutcome::Allowed(policy) => policy.clone(),
                PermissionOutcome::Denied(_) => unreachable!(),
            };
            let definition = match registry.definition(&tc.name) {
                Some(d) => d.clone(),
                None => {
                    results[i] = Some((format!("unknown tool {}", tc.name), false));
                    continue;
                }
            };
            let args = tc.arguments.clone();
            let wd = &working_dirs;
            let pc = &provider_context;
            let sid = &state.session.id;
            let runner_clone = runner.clone();
            handles.push((
                i,
                s.spawn(move || {
                    match super::claude_tools::execute_parallel_tool(
                        &definition,
                        cwd,
                        wd,
                        &filesystem_policy,
                        sid,
                        args,
                        resources,
                        registry,
                        pc,
                        &runner_clone,
                    ) {
                        Ok(exec) => {
                            let output = if exec.output.stderr.is_empty() {
                                exec.output.stdout
                            } else if exec.output.stdout.is_empty() {
                                exec.output.stderr
                            } else {
                                format!("{}\n{}", exec.output.stdout, exec.output.stderr)
                            };
                            (output, exec.success)
                        }
                        Err(error) => (format!("Tool execution failed: {error}"), false),
                    }
                }),
            ));
        }
        for (i, handle) in handles {
            results[i] = Some(
                handle
                    .join()
                    .unwrap_or_else(|_| ("Tool execution panicked".to_string(), false)),
            );
        }
    });

    // Execute serial tools (those that need &mut state).
    for (i, tc) in tool_calls.iter().enumerate() {
        if results[i].is_some() {
            continue; // Already executed in parallel or denied.
        }
        if let PermissionOutcome::Denied(ref denied) = permissions[i] {
            results[i] = Some((denied.output.stdout.clone(), denied.success));
            continue;
        }
        // Serial execution with full &mut state access.
        let (output, success) = match execute_tool_call(
            state,
            resources,
            providers,
            auth_store,
            registry,
            model_id,
            cwd,
            ToolExecutionBackend::OpenAi {
                request_config,
                structured_output,
            },
            tool_filter,
            &tc.name,
            tc.arguments.clone(),
        ) {
            Ok(exec) => {
                let output = if exec.output.stderr.is_empty() {
                    exec.output.stdout
                } else if exec.output.stdout.is_empty() {
                    exec.output.stderr
                } else {
                    format!("{}\n{}", exec.output.stdout, exec.output.stderr)
                };
                (output, exec.success)
            }
            Err(error) => (format!("Tool execution failed: {error}"), false),
        };
        results[i] = Some((output, success));
    }

    // ---------- Phase 3: Assemble outputs in original order ----------
    let session_id = &state.session.id;
    let mut outputs = Vec::with_capacity(tool_calls.len());
    let mut invocations = Vec::with_capacity(tool_calls.len());
    for (i, tc) in tool_calls.iter().enumerate() {
        let (raw_output, success) = results[i]
            .take()
            .unwrap_or_else(|| ("Tool was not executed".to_string(), false));
        let output =
            super::process_tool_result(&raw_output, super::MAX_TOOL_RESULT_CHARS, session_id);
        outputs.push(OpenAIResponsesFunctionCallOutput {
            kind: "function_call_output".to_string(),
            call_id: tc.call_id.clone(),
            output: output.clone(),
        });
        invocations.push(ToolInvocation {
            call_id: tc.call_id.clone(),
            tool_id: tc.name.clone(),
            input: serde_json::to_string(&tc.arguments)?,
            output,
            success,
            terminate: false,
        });
    }

    // Enforce per-message aggregate budget (CC: 200K).
    let mut output_strings: Vec<String> = outputs.iter().map(|o| o.output.clone()).collect();
    super::enforce_tool_result_budget(&mut output_strings, session_id);
    for (i, new_output) in output_strings.into_iter().enumerate() {
        if new_output != outputs[i].output {
            invocations[i].output = new_output.clone();
            outputs[i].output = new_output;
        }
    }

    Ok(OpenAIToolResults {
        outputs,
        invocations,
    })
}

/// Serial fallback for single tool calls or when no parallelism is beneficial.
fn execute_openai_tool_calls_serial(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    tool_calls: &[OpenAIResponseToolCall],
    registry: &ToolRegistry,
    cwd: &std::path::Path,
    request_config: &OpenAIRequestConfig,
    model_id: &str,
    structured_output: Option<&StructuredOutputConfig>,
    tool_filter: Option<&super::RequestToolFilter>,
) -> Result<OpenAIToolResults> {
    let mut outputs = Vec::new();
    let mut invocations = Vec::new();
    for tool_call in tool_calls {
        let (output, success) = match execute_tool_call(
            state,
            resources,
            providers,
            auth_store,
            registry,
            model_id,
            cwd,
            ToolExecutionBackend::OpenAi {
                request_config,
                structured_output,
            },
            tool_filter,
            &tool_call.name,
            tool_call.arguments.clone(),
        ) {
            Ok(execution) => {
                let output = if execution.output.stderr.is_empty() {
                    execution.output.stdout
                } else if execution.output.stdout.is_empty() {
                    execution.output.stderr
                } else {
                    format!("{}\n{}", execution.output.stdout, execution.output.stderr)
                };
                (output, execution.success)
            }
            Err(error) => (format!("Tool execution failed: {error}"), false),
        };
        let output =
            super::process_tool_result(&output, super::MAX_TOOL_RESULT_CHARS, &state.session.id);
        outputs.push(OpenAIResponsesFunctionCallOutput {
            kind: "function_call_output".to_string(),
            call_id: tool_call.call_id.clone(),
            output: output.clone(),
        });
        invocations.push(ToolInvocation {
            call_id: tool_call.call_id.clone(),
            tool_id: tool_call.name.clone(),
            input: serde_json::to_string(&tool_call.arguments)?,
            output,
            success,
            terminate: false,
        });
    }

    // Enforce per-message aggregate budget (CC: 200K).
    let mut output_strings: Vec<String> = outputs.iter().map(|o| o.output.clone()).collect();
    super::enforce_tool_result_budget(&mut output_strings, &state.session.id);
    for (i, new_output) in output_strings.into_iter().enumerate() {
        if new_output != outputs[i].output {
            invocations[i].output = new_output.clone();
            outputs[i].output = new_output;
        }
    }

    Ok(OpenAIToolResults {
        outputs,
        invocations,
    })
}

pub(super) fn parse_openai_text(response: &Value) -> Result<String> {
    if let Some(text) = response.get("output_text").and_then(Value::as_str) {
        return Ok(text.to_string());
    }

    let mut parts = Vec::new();
    if let Some(items) = response.get("output").and_then(Value::as_array) {
        for item in items {
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for block in content {
                    let block_type = block
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if matches!(block_type, "output_text" | "text") {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            parts.push(text.to_string());
                        }
                    }
                }
            }
        }
    }
    if parts.is_empty() {
        bail!("openai response did not contain output text");
    }
    Ok(parts.join("\n"))
}

pub(super) fn openai_request_instructions(
    state: &mut AppState,
    resources: &LoadedResources,
    system_prompt: Option<&str>,
) -> Result<String> {
    let mut sections = Vec::new();
    if let Some(system_prompt) = system_prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        sections.push(system_prompt.to_string());
    }
    if let Some(plan_mode_context) =
        crate::plan_mode::take_plan_mode_context_message(state, resources)?
            .map(|message| message.trim().to_string())
            .filter(|message| !message.is_empty())
    {
        sections.push(plan_mode_context);
    }
    // Dynamic context (date, git status, CLAUDE.md) is now injected as a
    // context user message in the `input` array, not here.  This keeps
    // `instructions` static and cacheable (matching Codex's design where
    // `instructions` = pure developer instructions, and contextual data
    // lives in `input` items).
    Ok(sections.join("\n\n"))
}

/// Builds the dynamic context message injected into the `input` array.
///
/// This follows CC/Codex's pattern of separating static instructions
/// (in `instructions`) from dynamic context (in `input` messages).
/// The `<system-reminder>` XML tag helps the model distinguish
/// system-injected context from user-authored messages.
pub(super) fn build_context_reminder_message(state: &AppState) -> String {
    let reminder = self::conversation::build_system_reminder(state, &super::git_status_context());
    format!(
        "<system-reminder>\n{}\n\n      IMPORTANT: this context may or may not be relevant to your tasks. You should not respond to this context unless it is highly relevant to your task.\n</system-reminder>",
        reminder
    )
}

pub(super) fn parse_openai_assistant_text(
    parsed: &OpenAIResponsesResponse,
    response: &Value,
    state: &AppState,
) -> Result<String> {
    let text = extract_responses_text(parsed);
    if text.trim().is_empty() {
        parse_openai_text(response).or_else(|_| parse_openai_text_fallback(response, state))
    } else {
        Ok(text)
    }
}

pub(super) fn parse_openai_text_fallback(response: &Value, state: &AppState) -> Result<String> {
    if let Some(text) = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        return Ok(text);
    }
    let output_kinds = response
        .get("output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("type").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    bail!(
        "provider {} returned an unsupported response shape for session {} (output types: {})",
        state.current_provider.as_deref().unwrap_or("unknown"),
        state.session.id,
        if output_kinds.is_empty() {
            "<none>"
        } else {
            output_kinds.as_str()
        }
    )
}

pub(super) fn resolve_openai_execution_config(
    state: &AppState,
    auth_store: &AuthStore,
    provider: &ProviderDescriptor,
) -> Result<OpenAIExecutionConfig> {
    let mut custom_headers = provider
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    // The execution config is built once per session (no model_id in
    // scope); compat-driven version-header gating consults the
    // descriptor in `support::append_default_openai_headers` only when
    // a model is supplied. Pass `None` here — auto-detect handles the
    // canonical providers (`provider.id == "openai"`).
    append_default_openai_headers(&mut custom_headers, provider.id.as_str(), None);
    let session_id = Some(state.session.id.to_string());
    let originator = OPENAI_CODEX_ORIGINATOR.to_string();
    match auth_store.get(provider.id.as_str()) {
        Some(StoredCredential::ApiKey { key }) => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: provider.base_url.clone(),
                version: openai_request_version(provider, false),
                auth: OpenAIAuth::ApiKey(key.clone()),
                originator,
                session_id,
                account_id: None,
                custom_headers,
                query_params: provider
                    .query_params
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                chat_completions_path: provider.chat_completions_path.clone(),
                responses_path: None,
            },
            refresh_token: None,
            codex_style: codex_style_for_provider(provider, false),
        }),
        Some(StoredCredential::OAuth(credential)) => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: openai_base_url_for_auth(provider, true),
                version: openai_request_version(provider, true),
                auth: OpenAIAuth::OAuthBearer(credential.access_token.clone()),
                originator,
                session_id,
                account_id: credential.account_id.clone(),
                custom_headers,
                query_params: provider
                    .query_params
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                chat_completions_path: provider.chat_completions_path.clone(),
                responses_path: None,
            },
            refresh_token: Some(credential.refresh_token.clone()),
            codex_style: codex_style_for_provider(provider, true),
        }),
        None if provider.auth_modes.is_empty() => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: provider.base_url.clone(),
                version: openai_request_version(provider, false),
                auth: OpenAIAuth::None,
                originator,
                session_id,
                account_id: None,
                custom_headers,
                query_params: provider
                    .query_params
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                chat_completions_path: provider.chat_completions_path.clone(),
                responses_path: None,
            },
            refresh_token: None,
            codex_style: codex_style_for_provider(provider, false),
        }),
        None => bail!(
            "no credentials configured for provider {}; use `puffer auth set-api-key {}` first",
            provider.id,
            provider.id
        ),
    }
}

fn codex_style_for_provider(provider: &ProviderDescriptor, oauth: bool) -> bool {
    let requested = if oauth && provider.id == "openai" {
        true
    } else {
        is_codex_openai_provider(provider)
    };
    requested
        && std::env::var("PUFFER_OPENAI_DISABLE_CODEX_STYLE")
            .ok()
            .as_deref()
            != Some("1")
}

pub(super) fn send_openai_request_with_refresh<F>(
    auth_store: &mut AuthStore,
    execution: &mut OpenAIExecutionConfig,
    build_request: F,
) -> Result<Value>
where
    F: Fn(&OpenAIRequestConfig) -> Result<puffer_provider_openai::BuiltOpenAIRequest>,
{
    retry_openai_transport(
        || send_openai_request_with_refresh_once(auth_store, execution, &build_request),
        |_, _, _| {},
    )
}

fn send_openai_request_with_refresh_once<F>(
    auth_store: &mut AuthStore,
    execution: &mut OpenAIExecutionConfig,
    build_request: &F,
) -> Result<Value>
where
    F: Fn(&OpenAIRequestConfig) -> Result<puffer_provider_openai::BuiltOpenAIRequest>,
{
    let request = build_request(&execution.request_config)?;
    let response = send_http_request_raw(&request.url, &request.headers, &request.body, false)?;
    if response.status != StatusCode::UNAUTHORIZED || execution.refresh_token.is_none() {
        return parse_http_json_response(&request.url, false, response);
    }

    let refresh_token = execution
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing refresh token for OpenAI OAuth retry"))?;
    let refreshed = refresh_oauth_token(&refresh_token)
        .context("failed to refresh OpenAI OAuth credentials after 401")?;
    let stored = openai_registry_credential(refreshed);
    execution.request_config.auth = OpenAIAuth::OAuthBearer(stored.access_token.clone());
    execution.request_config.account_id = stored.account_id.clone();
    execution.refresh_token = Some(stored.refresh_token.clone());
    auth_store.set_oauth(execution.provider_id.clone(), stored);

    let retry = build_request(&execution.request_config)?;
    let retry_response = send_http_request_raw(&retry.url, &retry.headers, &retry.body, false)?;
    parse_http_json_response(&retry.url, false, retry_response)
}

pub(super) fn send_openai_request_with_refresh_streaming<F, G>(
    auth_store: &mut AuthStore,
    execution: &mut OpenAIExecutionConfig,
    build_request: F,
    on_event: &mut G,
) -> Result<OpenAISseResult>
where
    F: Fn(&OpenAIRequestConfig) -> Result<puffer_provider_openai::BuiltOpenAIRequest>,
    G: FnMut(TurnStreamEvent),
{
    let request = build_request(&execution.request_config)?;
    // Layered retry: inner = connection-level (`retry_openai_transport`)
    // surfaces `RetryAttempt` events on `on_event`; outer =
    // HTTP 5xx response status (`runtime::retry_on_5xx`) traces
    // via `tracing::warn!` to avoid the closure borrow conflict
    // (both branches would otherwise want `&mut on_event`).
    // CC's SDK retries on >=500 the same way (`shouldRetry` in
    // claude-2.1.133 bundle).
    let response = super::retry_on_5xx(
        || {
            retry_openai_transport(
                || send_openai_request_stream_raw(&request.url, &request.headers, &request.body),
                |attempt, max, error| {
                    on_event(TurnStreamEvent::RetryAttempt {
                        attempt,
                        max_attempts: max,
                        error: error.to_string(),
                    });
                },
            )
        },
        |attempt, max, status| {
            tracing::warn!(
                target: "puffer::runtime::openai",
                "5xx retry: attempt {attempt}/{max}, HTTP {}, sleeping before retry",
                status.as_u16()
            );
        },
    )?;
    if response.status() != StatusCode::UNAUTHORIZED || execution.refresh_token.is_none() {
        return parse_openai_stream_response(&request.url, response, on_event);
    }

    let refresh_token = execution
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing refresh token for OpenAI OAuth retry"))?;
    let refreshed = refresh_oauth_token(&refresh_token)
        .context("failed to refresh OpenAI OAuth credentials after 401")?;
    let stored = openai_registry_credential(refreshed);
    execution.request_config.auth = OpenAIAuth::OAuthBearer(stored.access_token.clone());
    execution.request_config.account_id = stored.account_id.clone();
    execution.refresh_token = Some(stored.refresh_token.clone());
    auth_store.set_oauth(execution.provider_id.clone(), stored);

    let retry = build_request(&execution.request_config)?;
    let retry_response = super::retry_on_5xx(
        || {
            retry_openai_transport(
                || send_openai_request_stream_raw(&retry.url, &retry.headers, &retry.body),
                |attempt, max, error| {
                    on_event(TurnStreamEvent::RetryAttempt {
                        attempt,
                        max_attempts: max,
                        error: error.to_string(),
                    });
                },
            )
        },
        |attempt, max, status| {
            tracing::warn!(
                target: "puffer::runtime::openai",
                "5xx retry (post-401-refresh): attempt {attempt}/{max}, HTTP {}",
                status.as_u16()
            );
        },
    )?;
    parse_openai_stream_response(&retry.url, retry_response, on_event)
}

fn send_openai_request_stream_raw(
    url: &str,
    headers: &[(String, String)],
    body: &str,
) -> Result<Response> {
    trace_openai_http_request(url, headers, body);
    let client = Client::builder()
        .timeout(openai_stream_read_timeout())
        .build()?;
    let mut request = client.post(url);
    for (key, value) in headers {
        request = request.header(key, value);
    }
    if !headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("content-type"))
    {
        request = request.header("content-type", "application/json");
    }
    let response = request
        .body(body.to_string())
        .send()
        .with_context(|| format!("request to {url} failed"))?;
    trace_openai_http_response_headers(
        url,
        response.status().as_u16(),
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value: &reqwest::header::HeaderValue| value.to_str().ok()),
    );
    Ok(response)
}

fn parse_openai_stream_response<G>(
    url: &str,
    response: Response,
    on_event: &mut G,
) -> Result<OpenAISseResult>
where
    G: FnMut(TurnStreamEvent),
{
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    if !status.is_success() {
        // Use lossy decode (`unwrap_or_default`) instead of `?` so a
        // non-UTF8 / partially-truncated body still reaches the
        // classifier — for the quota-error path losing a byte or two
        // is preferable to surfacing a UTF-8 error and missing the
        // 429/403 promotion entirely. Mirrors anthropic.rs:266.
        let text = response.text().unwrap_or_default();
        // Promote 429 / 403-access-terminated to a typed `QuotaError`
        // so the benchmark CLI can exit with a distinct code instead
        // of letting the orchestration layer burn its retry budget on
        // a quota window. See `runtime::quota` for design notes.
        if let Some(quota) = super::quota::classify_response("openai", status.as_u16(), &text) {
            return Err(anyhow::Error::new(quota));
        }
        bail!("request failed with status {}: {}", status, text);
    }
    let mut reader = std::io::BufReader::new(response);
    let looks_like_sse = if is_event_stream(content_type.as_deref(), "") {
        true
    } else {
        let prefix = reader.fill_buf()?;
        let prefix = std::str::from_utf8(prefix).unwrap_or_default();
        is_event_stream(content_type.as_deref(), prefix)
    };
    if looks_like_sse {
        return parse_openai_sse_reader_typed(reader, on_event)
            .with_context(|| format!("failed to parse SSE response from {url}"));
    }
    // Non-SSE fallback: parse JSON directly into typed struct — one parse, no roundtrip.
    let mut text = String::new();
    reader.read_to_string(&mut text)?;
    let raw: Value = serde_json::from_str(&text)
        .with_context(|| format!("response from {url} was not valid JSON"))?;
    let response_id = raw.get("id").and_then(Value::as_str).map(str::to_string);
    let input_tokens = raw
        .pointer("/usage/input_tokens")
        .and_then(Value::as_u64)
        .map(|v| v as usize);
    let parsed: OpenAIResponsesResponse = serde_json::from_value(raw.clone())
        .with_context(|| format!("response from {url} was not a valid Responses payload"))?;
    let assistant_text = extract_responses_text(&parsed);
    let tool_calls = extract_responses_tool_calls(&parsed)?;
    let reasoning_items: Vec<Value> = raw
        .pointer("/output")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    Ok(OpenAISseResult {
        response_id,
        input_tokens,
        output_tokens: None,
        cached_tokens: None,
        assistant_text,
        tool_calls,
        emitted_tool_call_ids: HashSet::new(),
        reasoning_items,
        raw_response: raw,
    })
}

/// Adapter for the OpenAI Responses API family — covers `openai-responses`,
/// `azure-openai-responses`, and `openai-codex-responses`. The streaming
/// implementation switches between the websocket and SSE transports based
/// on `openai_websocket_enabled()`, so the loop never branches on transport.
pub(crate) struct OpenAIResponsesAdapter;

impl super::provider_adapter::ProviderAdapter for OpenAIResponsesAdapter {
    fn api_id(&self) -> &'static str {
        // The adapter handles three aliases; the canonical id used in
        // error messages is the most common one.
        "openai-responses"
    }

    fn execute_turn(
        &self,
        state: &mut AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        provider: &ProviderDescriptor,
        model_id: String,
        auth_store: &mut AuthStore,
        input: &str,
        options: super::TurnRequestOptions<'_>,
    ) -> Result<super::TurnExecution> {
        let use_native =
            prefer_native_structured_output(state, provider, &model_id, options.structured_output);
        match run_responses_attempt(
            state, resources, providers, provider, &model_id, auth_store, input, &options,
            use_native, None,
        ) {
            Ok(turn) => Ok(turn),
            Err(error) if use_native && is_openai_structured_output_error(&error) => {
                state.mark_native_structured_output_unsupported(
                    OPENAI_STRUCTURED_OUTPUT_FAMILY,
                    provider.id.as_str(),
                    &model_id,
                    structured_output_endpoint_id(provider),
                );
                run_responses_attempt(
                    state, resources, providers, provider, &model_id, auth_store, input, &options,
                    false, None,
                )
            }
            Err(error) => Err(error),
        }
    }

    fn execute_turn_streaming(
        &self,
        state: &mut AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        provider: &ProviderDescriptor,
        model_id: String,
        auth_store: &mut AuthStore,
        input: &str,
        options: super::TurnRequestOptions<'_>,
        on_event: &mut dyn FnMut(super::TurnStreamEvent),
    ) -> Result<super::TurnExecution> {
        // WebSocket transport keeps the legacy (non-agent_loop) path —
        // it has fundamentally different framing and per-event flow
        // control. SSE path uses agent_loop + responses_session.
        if openai_websocket_enabled() {
            let mut wrapped = |event: super::TurnStreamEvent| on_event(event);
            return execute_openai_websocket_streaming(
                state,
                resources,
                providers,
                provider,
                model_id,
                auth_store,
                input,
                options,
                &mut wrapped,
            );
        }
        let use_native =
            prefer_native_structured_output(state, provider, &model_id, options.structured_output);
        match run_responses_attempt(
            state,
            resources,
            providers,
            provider,
            &model_id,
            auth_store,
            input,
            &options,
            use_native,
            Some(on_event as &mut dyn FnMut(super::TurnStreamEvent)),
        ) {
            Ok(turn) => Ok(turn),
            Err(error) if use_native && is_openai_structured_output_error(&error) => {
                state.mark_native_structured_output_unsupported(
                    OPENAI_STRUCTURED_OUTPUT_FAMILY,
                    provider.id.as_str(),
                    &model_id,
                    structured_output_endpoint_id(provider),
                );
                run_responses_attempt(
                    state,
                    resources,
                    providers,
                    provider,
                    &model_id,
                    auth_store,
                    input,
                    &options,
                    false,
                    Some(on_event),
                )
            }
            Err(error) => Err(error),
        }
    }
}

/// One attempt of OpenAI Responses execution: build a session with the
/// requested `use_native` flag, hand it off to `agent_loop`. The
/// adapter retries with `use_native=false` if the first attempt fails
/// with a native structured-output unsupported error.
fn run_responses_attempt(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: &str,
    auth_store: &mut AuthStore,
    input: &str,
    options: &super::TurnRequestOptions<'_>,
    use_native: bool,
    on_event: Option<&mut dyn FnMut(super::TurnStreamEvent)>,
) -> Result<super::TurnExecution> {
    let registry =
        super::mcp_discovery::registry_with_mcp_tools(resources, state.tool_runner.as_ref());
    let mut session = self::responses_session::setup_responses_session(
        state,
        resources,
        provider,
        model_id.to_string(),
        auth_store,
        options,
        use_native,
    )?;
    let mut inputs = super::agent_loop::LoopInputs {
        state,
        resources,
        providers,
        provider,
        model_id,
        auth_store,
        input,
        reflection_config: options.reflection.clone(),
        tool_filter: options.tool_filter,
        registry: &registry,
        cancel: options.cancel,
        max_turns: options.max_turns,
        observability: options.observability.clone(),
    };
    match on_event {
        Some(sink) => super::agent_loop::run_streaming_loop(&mut inputs, &mut session, sink),
        None => super::blocking_loop::run_blocking_loop(&mut inputs, &mut session),
    }
}

/// Adapter for OpenAI Chat Completions (`openai-completions`). No
/// streaming implementation today — the default trait impl falls back
/// to the non-streaming path, matching the prior dispatcher's
/// behavior.
pub(crate) struct OpenAICompletionsAdapter;

impl super::provider_adapter::ProviderAdapter for OpenAICompletionsAdapter {
    fn api_id(&self) -> &'static str {
        "openai-completions"
    }

    fn execute_turn(
        &self,
        state: &mut AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        provider: &ProviderDescriptor,
        model_id: String,
        auth_store: &mut AuthStore,
        input: &str,
        options: super::TurnRequestOptions<'_>,
    ) -> Result<super::TurnExecution> {
        let use_native =
            prefer_native_structured_output(state, provider, &model_id, options.structured_output);
        match run_completions_attempt(
            state, resources, providers, provider, &model_id, auth_store, input, &options,
            use_native, None,
        ) {
            Ok(turn) => Ok(turn),
            Err(error) if use_native && is_openai_structured_output_error(&error) => {
                state.mark_native_structured_output_unsupported(
                    OPENAI_STRUCTURED_OUTPUT_FAMILY,
                    provider.id.as_str(),
                    &model_id,
                    structured_output_endpoint_id(provider),
                );
                run_completions_attempt(
                    state, resources, providers, provider, &model_id, auth_store, input, &options,
                    false, None,
                )
            }
            Err(error) => Err(error),
        }
    }

    fn execute_turn_streaming(
        &self,
        state: &mut AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        provider: &ProviderDescriptor,
        model_id: String,
        auth_store: &mut AuthStore,
        input: &str,
        options: super::TurnRequestOptions<'_>,
        on_event: &mut dyn FnMut(super::TurnStreamEvent),
    ) -> Result<super::TurnExecution> {
        let use_native =
            prefer_native_structured_output(state, provider, &model_id, options.structured_output);
        // Without an explicit streaming attempt the session's
        // `one_turn_streaming` (which synthesizes ThinkingDelta /
        // TextDelta from `reasoning_content`) is never invoked — the
        // default trait impl routes to `execute_turn` and therefore
        // `run_blocking_loop`, dropping the live thinking signal that
        // reasoning-capable Chat Completions providers (Moonshot Kimi
        // `k2p5`, Deepseek, OpenRouter) emit. Wire it explicitly.
        match run_completions_attempt(
            state,
            resources,
            providers,
            provider,
            &model_id,
            auth_store,
            input,
            &options,
            use_native,
            Some(on_event),
        ) {
            Ok(turn) => Ok(turn),
            Err(error) if use_native && is_openai_structured_output_error(&error) => {
                state.mark_native_structured_output_unsupported(
                    OPENAI_STRUCTURED_OUTPUT_FAMILY,
                    provider.id.as_str(),
                    &model_id,
                    structured_output_endpoint_id(provider),
                );
                run_completions_attempt(
                    state,
                    resources,
                    providers,
                    provider,
                    &model_id,
                    auth_store,
                    input,
                    &options,
                    false,
                    Some(on_event),
                )
            }
            Err(error) => Err(error),
        }
    }
}

/// One attempt of OpenAI Chat Completions execution.
///
/// `on_event` distinguishes blocking from streaming dispatch: when
/// `Some(...)`, route to `run_streaming_loop` so the session's
/// `one_turn_streaming` fires `ThinkingDelta` + `TextDelta` events;
/// otherwise route to `run_blocking_loop` (no events). Without this
/// branch, the streaming path never reaches the session's event-emit
/// site and reasoning-capable providers' thinking blocks stay silent.
fn run_completions_attempt(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: &str,
    auth_store: &mut AuthStore,
    input: &str,
    options: &super::TurnRequestOptions<'_>,
    use_native: bool,
    on_event: Option<&mut dyn FnMut(super::TurnStreamEvent)>,
) -> Result<super::TurnExecution> {
    let registry =
        super::mcp_discovery::registry_with_mcp_tools(resources, state.tool_runner.as_ref());
    let mut session = self::completions_session::setup_completions_session(
        state,
        resources,
        provider,
        model_id.to_string(),
        auth_store,
        options,
        use_native,
    )?;
    let mut inputs = super::agent_loop::LoopInputs {
        state,
        resources,
        providers,
        provider,
        model_id,
        auth_store,
        input,
        reflection_config: options.reflection.clone(),
        tool_filter: options.tool_filter,
        registry: &registry,
        cancel: options.cancel,
        max_turns: options.max_turns,
        observability: options.observability.clone(),
    };
    match on_event {
        Some(sink) => super::agent_loop::run_streaming_loop(&mut inputs, &mut session, sink),
        None => super::blocking_loop::run_blocking_loop(&mut inputs, &mut session),
    }
}
