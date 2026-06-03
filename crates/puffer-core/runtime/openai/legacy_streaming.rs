use super::support::{
    apply_previous_response_id, build_codex_openai_request_body, is_openai_structured_output_error,
    is_retryable_openai_stream_error, openai_model_supports_reasoning, openai_responses_path,
    openai_stream_max_attempts, openai_stream_retry_delay, openai_supports_response_threading,
    prefer_native_structured_output, structured_output_endpoint_id,
    OPENAI_STRUCTURED_OUTPUT_FAMILY,
};
use super::{
    build_context_reminder_message, execute_openai_tool_calls, openai_request_instructions,
    parse_openai_text, parse_openai_text_fallback, resolve_openai_execution_config,
    send_openai_request_with_refresh_streaming,
};
use crate::permissions::{load_runtime_permission_context_with_inputs, RuntimePermissionInputs};
use crate::runtime;
use crate::runtime::structured_output_support::{
    openai_responses_text_config, openai_tool_definitions_for_request,
};
use crate::runtime::system_prompt::render_runtime_system_prompt;
use crate::runtime::{run_turn_hooks, TurnStreamEvent};
use crate::AppState;
use anyhow::Result;
use puffer_provider_openai::build_json_post_request;
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry};
use puffer_resources::LoadedResources;

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
    options: runtime::TurnRequestOptions<'_>,
    on_event: &mut F,
) -> Result<runtime::TurnExecution>
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
    options: runtime::TurnRequestOptions<'_>,
    use_native: bool,
    on_event: &mut F,
) -> Result<runtime::TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    use super::conversation::{
        append_managed_system_prompt_1_to_instructions, append_reasoning_items,
        append_tool_results, compact_conversation, inject_post_compact_context,
        items_to_responses_input, managed_system_prompt_1_from_env, transcript_to_items,
        ConversationItem,
    };

    let structured_output = options.structured_output;
    let mut execution = resolve_openai_execution_config(state, auth_store, provider)?;
    let registry =
        runtime::mcp_discovery::registry_with_mcp_tools(resources, state.tool_runner.as_ref());
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
        .map(|config| runtime::reflection::ReflectionTracker::new(input, config));
    let mut reflection_traces: Vec<runtime::ReflectionTraceEvent> = Vec::new();

    // Inject dynamic context as a user message at the start of the input
    // array (matching Codex/CC pattern).
    if !options.lightweight_context {
        let context_reminder = build_context_reminder_message(state);
        super::conversation::insert_context_reminder_preserving_legacy_leading_system(
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
        let completed = runtime::claude_tools::workflow::drain_completed_shell_tasks(
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
        let max_attempts = openai_stream_max_attempts();
        let response = 'stream_retry: loop {
            for attempt in 1..=max_attempts {
                match send_openai_request_with_refresh_streaming(
                    auth_store,
                    &mut execution,
                    &state.config.network.proxy,
                    |request_config| {
                        let mut body = build_codex_openai_request_body(
                            state,
                            &request_config.base_url,
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
                ) {
                    Ok(response) => break 'stream_retry response,
                    Err(error)
                        if attempt < max_attempts && is_retryable_openai_stream_error(&error) =>
                    {
                        let delay = openai_stream_retry_delay(attempt);
                        tracing::warn!(
                            target: "puffer::runtime::openai",
                            attempt,
                            max_attempts,
                            retry_delay_ms = delay.as_millis(),
                            error = %error,
                            "OpenAI Responses stream failed before a terminal event; restarting sampling request"
                        );
                        on_event(TurnStreamEvent::RetryAttempt {
                            attempt,
                            max_attempts,
                            error: error.to_string(),
                        });
                        if !delay.is_zero() {
                            std::thread::sleep(delay);
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
            unreachable!("OpenAI stream retry loop always returns or errors")
        };

        // Typed fields extracted during SSE — no Value→String→typed roundtrip.
        previous_response_id = if supports_response_threading {
            response.response_id
        } else {
            None
        };
        let input_tokens = response.input_tokens;
        if let Some(tokens) = input_tokens {
            if tokens > 0 {
                state.last_input_tokens = Some(tokens as u32);
            }
        }
        // Emit per-turn usage with cache hit data.
        if let Some(input) = response.input_tokens {
            let cached = response.cached_tokens.unwrap_or(0);
            let output = response.output_tokens.unwrap_or(0);
            state.update_cache_stats(input as u64, cached as u64);
            on_event(TurnStreamEvent::Usage(runtime::TurnUsageReport {
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
            return Ok(runtime::TurnExecution {
                assistant_text,
                tool_invocations: invocations,
                reflection_traces,
            });
        }

        let tool_calls = response.tool_calls;
        let pending_tool_calls = tool_calls
            .iter()
            .filter(|tool_call| !response.emitted_tool_call_ids.contains(&tool_call.call_id))
            .map(|tool_call| runtime::ToolCallRequest {
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
