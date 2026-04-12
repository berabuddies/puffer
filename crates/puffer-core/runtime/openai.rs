use super::{
    execute_tool_call, is_parallel_safe_tool, parse_http_json_response, resolve_tool_permission,
    run_turn_hooks, send_http_request_raw, PermissionOutcome, ToolExecutionBackend, ToolInvocation,
    TurnStreamEvent, APP_VERSION,
};
use crate::permissions::load_runtime_permission_context;
use crate::workspace_paths;
pub(crate) mod conversation;
mod support;

pub(super) use self::support::build_codex_openai_request_body;
use self::support::{
    append_default_openai_headers, apply_previous_response_id, is_codex_openai_provider,
    is_openai_structured_output_error, openai_base_url_for_auth, openai_model_supports_reasoning,
    openai_registry_credential, openai_responses_path, openai_stream_read_timeout,
    prefer_native_structured_output, retry_openai_transport, structured_output_endpoint_id,
    trace_openai_http_request, trace_openai_http_response_headers, OPENAI_STRUCTURED_OUTPUT_FAMILY,
};
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
    parse_chat_completions_response, parse_responses_response, refresh_oauth_token, OpenAIAuth,
    OpenAIChatCompletionsRequest, OpenAIRequestConfig, OpenAIResponseToolCall,
    OpenAIResponsesFunctionCallOutput, OpenAIResponsesResponse, OpenAIResponsesToolChoiceMode,
};
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry, StoredCredential};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;
use serde_json::Value;
use std::collections::HashSet;

pub(super) use super::openai_sse::{
    is_event_stream, parse_openai_sse_reader_with_metadata, parse_openai_sse_response,
};

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

#[derive(Debug)]
struct OpenAIStreamResponse {
    body: Value,
    emitted_tool_call_ids: HashSet<String>,
}

pub(super) fn execute_openai(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
) -> Result<super::TurnExecution> {
    let structured_output = options.structured_output;
    let use_native = prefer_native_structured_output(state, provider, &model_id, structured_output);
    match execute_openai_once(
        state,
        resources,
        providers,
        provider,
        model_id.clone(),
        auth_store,
        input,
        options,
        use_native,
    ) {
        Ok(turn) => Ok(turn),
        Err(error) if use_native && is_openai_structured_output_error(&error) => {
            state.mark_native_structured_output_unsupported(
                OPENAI_STRUCTURED_OUTPUT_FAMILY,
                provider.id.as_str(),
                &model_id,
                structured_output_endpoint_id(provider),
            );
            execute_openai_once(
                state, resources, providers, provider, model_id, auth_store, input, options, false,
            )
        }
        Err(error) => Err(error),
    }
}

fn execute_openai_once(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
    use_native: bool,
) -> Result<super::TurnExecution> {
    use self::conversation::{
        append_tool_results, compact_conversation, inject_post_compact_context,
        items_to_responses_input, transcript_to_items, ConversationItem,
    };

    let structured_output = options.structured_output;
    let mut execution = resolve_openai_execution_config(state, auth_store, provider)?;
    let registry = ToolRegistry::from_resources(resources);
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let text = openai_responses_text_config(structured_output, use_native);
    let tools = openai_tool_definitions_for_request(
        &registry,
        structured_output,
        use_native,
        Some(&permission_context),
        options.tool_filter,
    )?;
    let system_prompt = render_runtime_system_prompt(
        state,
        resources,
        &model_id,
        &tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<std::collections::BTreeSet<_>>(),
    )?;
    let instructions = openai_request_instructions(state, resources, Some(&system_prompt))?;
    // Unified: all internal logic on Vec<ConversationItem>.
    let mut items = transcript_to_items(state, input);
    let mut invocations = Vec::new();
    let supports_reasoning = openai_model_supports_reasoning(provider, &model_id);
    let mut previous_response_id = None;
    // Index where "continuation" items start — used for previous_response_id optimization.
    // When previous_response_id is set, only items[start..] are sent as wire input.
    let mut continuation_start: Option<usize> = None;

    loop {
        // Wire boundary: ConversationItem → Responses API input.
        let wire_input = match (previous_response_id.as_ref(), continuation_start) {
            (Some(_), Some(start)) => items_to_responses_input(&items[start..]),
            _ => items_to_responses_input(&items),
        };

        let response =
            send_openai_request_with_refresh(auth_store, &mut execution, |request_config| {
                let mut body = build_codex_openai_request_body(
                    state,
                    &model_id,
                    &instructions,
                    wire_input.clone(),
                    &tools,
                    supports_reasoning,
                    text.clone(),
                    false,
                );
                apply_previous_response_id(&mut body, previous_response_id.as_deref());
                build_json_post_request(
                    request_config,
                    openai_responses_path(&request_config.base_url),
                    &body,
                )
            })?;

        let parsed = parse_responses_response(&serde_json::to_string(&response)?)?;
        previous_response_id = parsed.id.clone();
        // Extract server-reported input token count for precise compaction & context display.
        let input_tokens = response
            .pointer("/usage/input_tokens")
            .and_then(Value::as_u64)
            .map(|v| v as usize);
        if let Some(tokens) = input_tokens {
            state.last_input_tokens = Some(tokens as u32);
        }
        let tool_calls = extract_responses_tool_calls(&parsed)?;
        if tool_calls.is_empty() {
            let assistant_text = parse_openai_assistant_text(&parsed, &response, state)?;
            run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
            return Ok(super::TurnExecution {
                assistant_text,
                tool_invocations: invocations,
            });
        }

        // Add assistant text from this round (if any) to maintain full history.
        let assistant_text = extract_responses_text(&parsed);
        if !assistant_text.trim().is_empty() {
            items.push(ConversationItem::assistant_message(&assistant_text));
        }
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

        // Shared: append tool calls + outputs to canonical items.
        append_tool_results(&mut items, &tool_results.invocations);
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
            inject_post_compact_context(&mut items, &cwd);
        }
    }
}

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
        options,
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
        append_tool_results, compact_conversation, inject_post_compact_context,
        items_to_responses_input, transcript_to_items, ConversationItem,
    };

    let structured_output = options.structured_output;
    let mut execution = resolve_openai_execution_config(state, auth_store, provider)?;
    let registry = ToolRegistry::from_resources(resources);
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let text = openai_responses_text_config(structured_output, use_native);
    let tools = openai_tool_definitions_for_request(
        &registry,
        structured_output,
        use_native,
        Some(&permission_context),
        options.tool_filter,
    )?;
    let system_prompt = render_runtime_system_prompt(
        state,
        resources,
        &model_id,
        &tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<std::collections::BTreeSet<_>>(),
    )?;
    let instructions = openai_request_instructions(state, resources, Some(&system_prompt))?;
    // Unified: all internal logic on Vec<ConversationItem>.
    let mut items = transcript_to_items(state, input);
    let mut invocations = Vec::new();
    let supports_reasoning = openai_model_supports_reasoning(provider, &model_id);
    let mut previous_response_id: Option<String> = None;
    // Index where "continuation" items start — used for previous_response_id optimization.
    // When previous_response_id is set, only items[start..] are sent as wire input.
    let mut continuation_start: Option<usize> = None;

    loop {
        // Check for background tasks that completed since the last turn and inject
        // a system reminder so the model learns about them without needing to poll.
        let completed = super::claude_tools::workflow::drain_completed_shell_tasks(&state.cwd);
        if !completed.is_empty() {
            let notice = format!(
                "<system-reminder>\n{}\nUse TaskOutput to retrieve the full output if needed.\n</system-reminder>",
                completed.join("\n")
            );
            items.push(ConversationItem::user_message(&notice));
        }

        // Wire boundary: ConversationItem → Responses API input.
        // When previous_response_id is set, only send continuation items.
        let wire_input = match (previous_response_id.as_ref(), continuation_start) {
            (Some(_), Some(start)) => items_to_responses_input(&items[start..]),
            _ => items_to_responses_input(&items),
        };

        let prev_resp_id = previous_response_id.clone();
        let response = retry_openai_transport(|| {
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
        })?;

        let parsed = parse_responses_response(&serde_json::to_string(&response.body)?)?;
        previous_response_id = parsed.id.clone();
        // Extract server-reported input token count for context display & compaction.
        let input_tokens = response
            .body
            .pointer("/usage/input_tokens")
            .and_then(Value::as_u64)
            .map(|v| v as usize);
        if let Some(tokens) = input_tokens {
            state.last_input_tokens = Some(tokens as u32);
        }
        let tool_calls = extract_responses_tool_calls(&parsed)?;
        if tool_calls.is_empty() {
            let assistant_text = parse_openai_assistant_text(&parsed, &response.body, state)?;
            run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
            return Ok(super::TurnExecution {
                assistant_text,
                tool_invocations: invocations,
            });
        }

        let pending_tool_calls = tool_calls
            .iter()
            .filter(|tool_call| !response.emitted_tool_call_ids.contains(&tool_call.call_id))
            .map(|tool_call| super::ToolCallRequest {
                tool_id: tool_call.name.clone(),
                input: serde_json::to_string(&tool_call.arguments).unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        if !pending_tool_calls.is_empty() {
            on_event(TurnStreamEvent::ToolCallsRequested(pending_tool_calls));
        }

        // Add assistant text from this round to maintain full history.
        let assistant_text = extract_responses_text(&parsed);
        if !assistant_text.trim().is_empty() {
            items.push(ConversationItem::assistant_message(&assistant_text));
        }
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
            inject_post_compact_context(&mut items, &cwd);
        }
    }
}

pub(super) fn execute_openai_completions(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
) -> Result<super::TurnExecution> {
    let structured_output = options.structured_output;
    let use_native = prefer_native_structured_output(state, provider, &model_id, structured_output);
    match execute_openai_completions_once(
        state,
        resources,
        providers,
        provider,
        model_id.clone(),
        auth_store,
        input,
        options,
        use_native,
    ) {
        Ok(turn) => Ok(turn),
        Err(error) if use_native && is_openai_structured_output_error(&error) => {
            state.mark_native_structured_output_unsupported(
                OPENAI_STRUCTURED_OUTPUT_FAMILY,
                provider.id.as_str(),
                &model_id,
                structured_output_endpoint_id(provider),
            );
            execute_openai_completions_once(
                state, resources, providers, provider, model_id, auth_store, input, options, false,
            )
        }
        Err(error) => Err(error),
    }
}

fn execute_openai_completions_once(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
    use_native: bool,
) -> Result<super::TurnExecution> {
    use self::conversation::{
        append_tool_results, build_system_reminder, compact_conversation,
        inject_post_compact_context, items_to_chat_messages, transcript_to_items, ConversationItem,
    };

    let structured_output = options.structured_output;
    let mut execution = resolve_openai_execution_config(state, auth_store, provider)?;
    let registry = ToolRegistry::from_resources(resources);
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let response_format = openai_chat_response_format(structured_output, use_native);
    let tools = openai_chat_completion_tools_for_request(
        &registry,
        structured_output,
        use_native,
        Some(&permission_context),
        options.tool_filter,
    )?;
    let system_prompt = render_runtime_system_prompt(
        state,
        resources,
        &model_id,
        &tools
            .iter()
            .map(|tool| tool.function.name.clone())
            .collect::<std::collections::BTreeSet<_>>(),
    )?;
    let plan_mode_context = crate::plan_mode::take_plan_mode_context_message(state, resources)?;
    let system_reminder = build_system_reminder(&super::git_status_context());

    // Unified: all internal logic on Vec<ConversationItem>.
    let mut items = transcript_to_items(state, input);
    let mut invocations = Vec::new();

    loop {
        // Check for background tasks that completed since the last turn.
        let completed = super::claude_tools::workflow::drain_completed_shell_tasks(&state.cwd);
        if !completed.is_empty() {
            let notice = format!(
                "<system-reminder>\n{}\nUse TaskOutput to retrieve the full output if needed.\n</system-reminder>",
                completed.join("\n")
            );
            items.push(ConversationItem::user_message(&notice));
        }

        // Wire boundary: ConversationItem → Chat Completions messages.
        let messages = items_to_chat_messages(
            &items,
            Some(&system_prompt),
            plan_mode_context.as_deref(),
            Some(&system_reminder),
        );
        let response =
            send_openai_request_with_refresh(auth_store, &mut execution, |request_config| {
                build_chat_completions_request(
                    request_config,
                    &OpenAIChatCompletionsRequest {
                        model: model_id.clone(),
                        messages: messages.clone(),
                        tools: tools.clone(),
                        tool_choice: if tools.is_empty() {
                            None
                        } else {
                            Some(OpenAIResponsesToolChoiceMode::Auto)
                        },
                        response_format: response_format.clone(),
                    },
                )
            })?;
        let parsed = parse_chat_completions_response(&serde_json::to_string(&response)?)?;
        let tool_calls = extract_chat_completions_tool_calls(&parsed)?;
        if tool_calls.is_empty() {
            let text = extract_chat_completions_text(&parsed);
            let assistant_text = if text.trim().is_empty() {
                parse_openai_text(&response)
                    .or_else(|_| parse_openai_text_fallback(&response, state))?
            } else {
                text
            };
            run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
            return Ok(super::TurnExecution {
                assistant_text,
                tool_invocations: invocations,
            });
        }

        // Preserve assistant text from this round (the model may emit text
        // alongside tool calls, e.g. "Let me check that file.").
        let assistant_text = extract_chat_completions_text(&parsed);
        if !assistant_text.trim().is_empty() {
            items.push(ConversationItem::assistant_message(&assistant_text));
        }

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

        // Shared: append tool calls + outputs to canonical items.
        append_tool_results(&mut items, &tool_results.invocations);
        invocations.extend(tool_results.invocations);

        // Shared: unified compaction (previously missing post-compact context).
        let compacted = compact_conversation(
            &mut items,
            provider,
            &model_id,
            &execution.request_config,
            None, // Chat Completions doesn't return input_tokens
        );
        if compacted {
            inject_post_compact_context(&mut items, &cwd);
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
    let allow_all_paths = workspace_paths::sandbox_allows_all_paths(&state.sandbox_mode);
    let provider_context = super::claude_tools::ProviderToolContext::OpenAI {
        request_config,
        model_id,
        structured_output,
    };

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
            handles.push((
                i,
                s.spawn(move || {
                    match super::claude_tools::execute_parallel_tool(
                        &definition,
                        cwd,
                        wd,
                        allow_all_paths,
                        args,
                        resources,
                        registry,
                        pc,
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

fn parse_openai_text(response: &Value) -> Result<String> {
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

fn openai_request_instructions(
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
    // Inject system reminder (current date + git status) matching Anthropic path.
    let now = time::OffsetDateTime::now_utc();
    let date_str = format!("{}-{:02}-{:02}", now.year(), now.month() as u8, now.day());
    let git_status = super::git_status_context();
    let mut reminder = format!("# currentDate\nToday's date is {date_str}.");
    if !git_status.is_empty() {
        reminder.push_str(&format!("\n\n# gitStatus\n{git_status}"));
    }
    sections.push(reminder);
    Ok(sections.join("\n\n"))
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
    append_default_openai_headers(&mut custom_headers, provider.id.as_str());
    let session_id = Some(state.session.id.to_string());
    let originator = OPENAI_CODEX_ORIGINATOR.to_string();
    match auth_store.get(provider.id.as_str()) {
        Some(StoredCredential::ApiKey { key }) => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: provider.base_url.clone(),
                version: APP_VERSION.to_string(),
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
            },
            refresh_token: None,
            codex_style: codex_style_for_provider(provider, false),
        }),
        Some(StoredCredential::OAuth(credential)) => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: openai_base_url_for_auth(provider, true),
                version: APP_VERSION.to_string(),
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
            },
            refresh_token: Some(credential.refresh_token.clone()),
            codex_style: codex_style_for_provider(provider, true),
        }),
        None if provider.auth_modes.is_empty() => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: provider.base_url.clone(),
                version: APP_VERSION.to_string(),
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
    retry_openai_transport(|| {
        send_openai_request_with_refresh_once(auth_store, execution, &build_request)
    })
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

fn send_openai_request_with_refresh_streaming<F, G>(
    auth_store: &mut AuthStore,
    execution: &mut OpenAIExecutionConfig,
    build_request: F,
    on_event: &mut G,
) -> Result<OpenAIStreamResponse>
where
    F: Fn(&OpenAIRequestConfig) -> Result<puffer_provider_openai::BuiltOpenAIRequest>,
    G: FnMut(TurnStreamEvent),
{
    let request = build_request(&execution.request_config)?;
    let response = retry_openai_transport(|| {
        send_openai_request_stream_raw(&request.url, &request.headers, &request.body)
    })?;
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
    let retry_response = retry_openai_transport(|| {
        send_openai_request_stream_raw(&retry.url, &retry.headers, &retry.body)
    })?;
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
) -> Result<OpenAIStreamResponse>
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
        let text = response.text()?;
        bail!("request failed with status {}: {}", status, text);
    }
    if is_event_stream(content_type.as_deref(), "") {
        return parse_openai_sse_reader_with_metadata(std::io::BufReader::new(response), on_event)
            .map(|parsed| OpenAIStreamResponse {
                body: parsed.response,
                emitted_tool_call_ids: parsed.emitted_tool_call_ids,
            })
            .with_context(|| format!("failed to parse SSE response from {url}"));
    }
    let text = response.text()?;
    serde_json::from_str::<Value>(&text)
        .map(|body| OpenAIStreamResponse {
            body,
            emitted_tool_call_ids: HashSet::new(),
        })
        .with_context(|| format!("response from {url} was not valid JSON"))
}
