//! Provider-agnostic turn loop driver.
//!
//! Mirrors pi-mono's `agent-loop.ts` shape: this module owns the
//! turn-by-turn driver — tool execution, reflection observation, and
//! compaction. Providers only perform a single round-trip mapping
//! `(messages, tools) → response items + pending tool calls`.
//!
//! The seam is the [`TurnSession`] trait. Each provider builds a
//! session that captures its vendor-specific setup (auth, URL, headers,
//! serialized tools, system blocks) once per user prompt, then exposes
//! neutral methods (`one_turn_streaming`, `generate_summary`,
//! `tool_execution_backend`) that the driver calls per iteration.
//!
//! What stays in the provider:
//! - HTTP request/response shape, SSE parsing, vendor JSON synthesis
//! - Auth/credentials/refresh
//! - Tool serialization to vendor wire (anthropic vs openai shape)
//!
//! What lives in the driver:
//! - Transcript ↔ `ConversationItem` boundary (`transcript_to_items`)
//! - Pre/post-turn compaction (`compact_conversation_with`)
//! - Background-task drain (`drain_completed_shell_tasks`)
//! - Tool execution (`execute_tool_call`)
//! - FunctionCallOutput synthesis from `ToolInvocation`
//! - Per-turn reflection observation
//! - End-of-turn hooks (`run_turn_hooks`)

use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;

use super::microcompact::{
    microcompact_in_place, MicrocompactConfig, MicrocompactInputs, MicrocompactOutcome,
    MicrocompactTrigger,
};
use super::openai::conversation::{
    compact_conversation_with, inject_post_compact_context, transcript_to_items, ConversationItem,
    ToolOutputPayload,
};
use super::reflection::{ReflectionConfig, ReflectionTraceEvent, ReflectionTracker};
use super::request_tool_filter::RequestToolFilter;
use super::tool_batch::execute_tool_batch;
use super::tool_executor::ToolExecutionBackend;
use super::{
    enforce_tool_result_budget, process_tool_result, run_turn_hooks, CancelToken, ToolCallRequest,
    ToolInvocation, TurnExecution, TurnStreamEvent, TurnUsageReport, MAX_TOOL_RESULT_CHARS,
};
use crate::AppState;

/// Output of one provider round-trip. Tool execution and
/// `FunctionCallOutput` synthesis are the loop's job — sessions only
/// return pre-tool items and pending tool calls.
pub(crate) struct AssistantTurn {
    /// Items to append BEFORE tool execution: assistant Message,
    /// Reasoning items, FunctionCall items.
    pub pre_tool_items: Vec<ConversationItem>,
    /// Pending tool calls extracted from the response.
    pub tool_calls: Vec<ToolCallRequest>,
    /// Final assistant text (joined from text content blocks).
    pub assistant_text: String,
    /// Optional input-token usage hint for compaction sizing.
    pub input_tokens_hint: Option<usize>,
    /// Tool call ids that the session already surfaced through
    /// `on_event` during streaming (e.g. via SSE
    /// `tool_call_start` events). Used by the loop to suppress
    /// duplicate `ToolCallsRequested` emissions.
    pub emitted_tool_call_ids: std::collections::HashSet<String>,
}

/// Provider-side session that captures vendor-specific setup and
/// performs a single LLM round-trip per call.
pub(crate) trait TurnSession {
    /// Sends one provider request with streaming events flowing through
    /// `on_event`. Returns synthesized response items + pending tool calls.
    ///
    /// `items` is `&mut` so the session can implement provider-side
    /// recovery (Anthropic's 413 / prompt_too_long path drops oldest
    /// items in place and retries before returning).
    fn one_turn_streaming(
        &mut self,
        state: &mut AppState,
        auth_store: &mut AuthStore,
        items: &mut Vec<ConversationItem>,
        on_event: &mut dyn FnMut(TurnStreamEvent),
    ) -> Result<AssistantTurn>;

    /// Non-streaming variant. Default impl forwards to
    /// `one_turn_streaming` with a no-op event sink. Providers that do
    /// genuine non-streaming HTTP (Anthropic blocking JSON) override
    /// this for transport-level differences (e.g. 413 recovery).
    fn one_turn_blocking(
        &mut self,
        state: &mut AppState,
        auth_store: &mut AuthStore,
        items: &mut Vec<ConversationItem>,
    ) -> Result<AssistantTurn> {
        let mut sink = |_: TurnStreamEvent| {};
        self.one_turn_streaming(state, auth_store, items, &mut sink)
    }

    /// Provider-specific compaction summary generation.
    fn generate_summary(&self, old_context: &str, model_id: &str) -> Option<String>;

    /// Backend descriptor for `execute_tool_call`. Carries vendor refs
    /// (e.g. `&AnthropicRequestConfig`) borrowed from session state.
    fn tool_execution_backend(&self) -> ToolExecutionBackend<'_>;

    /// Hook invoked once after `transcript_to_items` and before the
    /// first iteration. Lets vendor sessions inject preamble items
    /// (e.g. OpenAI's per-turn `currentDate / gitStatus` context
    /// reminder pinned at index 0). Default: no-op.
    fn pre_loop_inject(&mut self, _items: &mut Vec<ConversationItem>) {}

    /// Hook invoked after the loop performs a compaction so the session
    /// can invalidate threading state (OpenAI `previous_response_id` +
    /// `continuation_start` must reset because the server-side cached
    /// state no longer matches the local transcript). Default: no-op.
    fn notify_compacted(&mut self) {}
}

/// Static-per-turn inputs the loop needs from the call site. Mutable
/// references stay short-lived inside `run_*_loop` to keep the borrow
/// checker happy.
pub(crate) struct LoopInputs<'a> {
    pub state: &'a mut AppState,
    pub resources: &'a LoadedResources,
    pub providers: &'a ProviderRegistry,
    pub provider: &'a ProviderDescriptor,
    pub model_id: &'a str,
    pub auth_store: &'a mut AuthStore,
    pub input: &'a str,
    pub reflection_config: Option<ReflectionConfig>,
    pub tool_filter: Option<&'a RequestToolFilter>,
    pub registry: &'a ToolRegistry,
    /// Cooperative cancellation token. Checked at turn boundaries
    /// (before each provider call, between tool calls). When unset,
    /// the loop runs uninterruptibly. Mirrors pi-mono's `signal:
    /// AbortSignal` (`pi-mono/packages/ai/src/types.ts:70`).
    pub cancel: Option<&'a CancelToken>,
    /// Optional observability handle. When `Some`, the loop wraps
    /// each turn / provider call / tool batch in OTel spans and pushes
    /// them to the configured OTLP endpoint (e.g. Langfuse). When
    /// `None`, every span helper short-circuits to `Disabled` —
    /// strictly zero-cost. Owned (Arc-backed) so the loop can clone
    /// it across `thread::scope` parallel-tool branches. See
    /// `crates/puffer-observability` and
    /// `docs/observability/langfuse-design.md`.
    pub observability: Option<puffer_observability::ObservabilityHandle>,
}

/// Streaming turn loop. Drives the conversation until the model stops
/// requesting tool calls.
pub(crate) fn run_streaming_loop(
    inputs: &mut LoopInputs<'_>,
    session: &mut dyn TurnSession,
    on_event: &mut dyn FnMut(TurnStreamEvent),
) -> Result<TurnExecution> {
    let cwd = inputs.state.cwd.clone();

    let mut items = transcript_to_items(inputs.state, inputs.input);
    session.pre_loop_inject(&mut items);

    let mut invocations: Vec<ToolInvocation> = Vec::new();
    let mut reflection_traces: Vec<ReflectionTraceEvent> = Vec::new();
    let mut reflection = inputs
        .reflection_config
        .clone()
        .map(|config| ReflectionTracker::new(inputs.input, config));

    // Root span = `agent_loop`. All subsequent provider / tool /
    // compaction spans hang under this. The whole block is gated on
    // `observability.is_some()` so the disabled path does not even
    // allocate the session-id string or clone the provider id; review
    // v4 BLOCK #1.
    let mut agent_span = if let Some(handle) = inputs.observability.as_ref() {
        let session_str = inputs.state.session.id.to_string();
        let cwd_str = cwd.to_string_lossy();
        let mut span =
            puffer_observability::start_agent_loop_span(Some(handle), &session_str, &cwd_str);
        span.set_str(
            puffer_observability::PUFFER_PROVIDER_ID,
            inputs.provider.id.clone(),
        );
        let is_subagent = inputs.state.session.parent_session_id.is_some();
        if let Some(parent_sid) = inputs.state.session.parent_session_id {
            span.set_str("puffer.parent.session_id", parent_sid.to_string());
            span.set_str("puffer.subagent.kind", "agent_tool");
        }
        // Subagent runs (reflection judge, spawned agents) build
        // their input prompt by rendering the parent transcript
        // including tool calls + outputs. Use `PromptWithEmbeddedToolIo`
        // so the redaction layer requires BOTH `include_prompts` AND
        // `include_tool_io` for those traces; top-level user prompts
        // use plain `Prompt` (review v5 BLOCK #1). Either way the
        // attribute is always emitted so the trace's Input pane
        // shows the `[redacted: N bytes]` summary instead of `null`
        // when the flag is off.
        let kind = if is_subagent {
            puffer_observability::ContentKind::PromptWithEmbeddedToolIo
        } else {
            puffer_observability::ContentKind::Prompt
        };
        span.set_content(
            puffer_observability::LANGFUSE_TRACE_INPUT,
            kind.clone(),
            inputs.input,
        );
        span.set_content("puffer.input", kind, inputs.input);
        span
    } else {
        puffer_observability::SpanGuard::Disabled
    };

    // Pre-turn compaction. The summary closure wraps the actual
    // `session.generate_summary` LLM call in a `subagent.compaction_summary`
    // GENERATION span so the trace captures the *real* latency and any
    // error of the round-trip — not a post-hoc marker. Review v3 #4.
    let pre_compacted = {
        let mut compaction_span = puffer_observability::start_compaction_span(
            inputs.observability.as_ref(),
            agent_span.context(),
            0,
        );
        let observability = inputs.observability.as_ref();
        let parent_ctx = compaction_span.context();
        let summary_fn = |old: &str, mid: &str| -> Option<String> {
            let mut gen_span = puffer_observability::start_subagent_generation_span(
                observability,
                parent_ctx,
                "compaction_summary",
            );
            gen_span.set_str("puffer.compaction.phase", "0");
            let result = session.generate_summary(old, mid);
            if let Some(ref text) = result {
                gen_span.set_content(
                    puffer_observability::LANGFUSE_OBSERVATION_OUTPUT,
                    puffer_observability::ContentKind::OutputWithEmbeddedToolIo,
                    text,
                );
            } else {
                gen_span.mark_error("summary_returned_none".to_string());
            }
            gen_span.end();
            result
        };
        let did = compact_conversation_with(
            &mut items,
            inputs.provider,
            inputs.model_id,
            None,
            &summary_fn,
        );
        if !did {
            compaction_span.set_str("puffer.compaction.skipped", "true");
        }
        did
    };
    if pre_compacted {
        inject_post_compact_context(&mut items, &cwd);
        session.notify_compacted();
    }

    // Pre-loop microcompact (env-gated, off by default). Mirrors Claude
    // Code's pre-request pruning at `query.ts:401`. The trigger conditions
    // (time-gap / token-budget) and the `[Old tool result content cleared]`
    // stub check make this idempotent — calling it again inside the loop
    // (below, per iteration) only fires when there is something *new* to
    // clear, so the boundary message isn't spammed.
    maybe_microcompact(inputs.state, &mut items, Some(&mut agent_span));

    let mut turn_index: u32 = 0;
    loop {
        // Cancel boundary: check before each turn's provider round-trip.
        // Mark the root span cancelled before bailing so the trace
        // closes with the right status (review v3 #5).
        if let Some(cancel) = inputs.cancel {
            if let Err(error) = cancel.check() {
                agent_span.mark_cancelled();
                return Err(error);
            }
        }

        // Per-iteration microcompact. Mirrors Claude Code's
        // `query.ts:401` placement INSIDE the loop. Idempotent because
        // already-cleared FCOs equal CLEARED_STUB and are skipped, so
        // this only fires when a NEW compactable tool result was
        // appended since the previous pass.
        maybe_microcompact(inputs.state, &mut items, Some(&mut agent_span));

        // Drain completed background tasks and inject as user messages.
        let completed = crate::runtime::claude_tools::workflow::drain_completed_shell_tasks(
            &inputs.state.cwd,
            &inputs.state.session.id,
        );
        if !completed.is_empty() {
            let notice = format!(
                "<system-reminder>\n{}\nUse TaskOutput to retrieve the full output if needed.\n</system-reminder>",
                completed.join("\n")
            );
            items.push(ConversationItem::user_message(&notice));
        }

        // Per-turn span (Langfuse "span"; gen_ai child spans hang
        // under it). We collect token usage by intercepting Usage
        // events from the provider's `on_event` callback.
        let mut turn_span = puffer_observability::start_turn_span(
            inputs.observability.as_ref(),
            agent_span.context(),
            turn_index,
        );
        turn_index += 1;
        // Provider span (Langfuse "generation"). Stamp request
        // parameters from gen_ai semconv so the Langfuse generation
        // panel renders model invocation settings (review against
        // Langfuse demo: max_tokens + reasoning_effort were missing).
        let mut provider_span = puffer_observability::start_provider_span(
            inputs.observability.as_ref(),
            turn_span.context(),
            &inputs.provider.id,
            &inputs.provider.default_api,
            inputs.model_id,
        );
        if inputs.observability.is_some() {
            if let Some(model_info) = inputs
                .provider
                .models
                .iter()
                .find(|m| m.id == inputs.model_id)
            {
                if model_info.max_output_tokens > 0 {
                    provider_span.set_str(
                        puffer_observability::GEN_AI_REQUEST_MAX_TOKENS,
                        model_info.max_output_tokens.to_string(),
                    );
                }
            }
            if !inputs.state.effort_level.is_empty() {
                provider_span.set_str(
                    "gen_ai.request.reasoning_effort",
                    inputs.state.effort_level.clone(),
                );
            }
        }
        // Langfuse renders the generation Input pane from
        // `langfuse.observation.input`. Build the messages-array JSON
        // ONLY when observability is on AND the redaction policy
        // permits both prompts (envelope) and the tool-I/O (function
        // call args + tool output text) we'd otherwise embed. This
        // keeps the disabled path zero-cost and respects the split
        // include flags so `INCLUDE_PROMPTS=1, INCLUDE_TOOL_IO=0`
        // does not leak tool data through the LLM input pane.
        if let Some(handle) = inputs.observability.as_ref() {
            let policy = handle.redaction();
            if policy.include_prompts() {
                let include_tool_io = policy.include_tool_io();
                let provider_input_json = serde_json::to_string(
                    &items
                        .iter()
                        .map(|item| match item {
                            ConversationItem::Message { role, content } => {
                                let text: String = content
                                    .iter()
                                    .filter_map(|p| match p {
                                        super::openai::conversation::ContentPart::Text { text } => {
                                            Some(text.as_str())
                                        }
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                // Compaction stores its summary as a
                                // user `Message` prefixed with the
                                // marker below. The summary can carry
                                // prior tool I/O verbatim, so redact
                                // when `include_tool_io=false` — review
                                // v6 BLOCK #2.
                                let body = if !include_tool_io
                                    && text.starts_with("[Conversation compacted —")
                                {
                                    format!(
                                        "[redacted: {} bytes compaction summary message]",
                                        text.len()
                                    )
                                } else {
                                    text
                                };
                                serde_json::json!({ "role": role, "content": body })
                            }
                            ConversationItem::FunctionCall {
                                name, arguments, ..
                            } => {
                                let args = if include_tool_io {
                                    serde_json::Value::String(arguments.clone())
                                } else {
                                    serde_json::Value::String(format!(
                                        "[redacted: {} bytes tool args]",
                                        arguments.len()
                                    ))
                                };
                                serde_json::json!({
                                    "role": "assistant",
                                    "tool_call": { "name": name, "arguments": args }
                                })
                            }
                            ConversationItem::FunctionCallOutput { call_id, output } => {
                                let body = if include_tool_io {
                                    serde_json::Value::String(output.text.clone())
                                } else {
                                    serde_json::Value::String(format!(
                                        "[redacted: {} bytes tool output]",
                                        output.text.len()
                                    ))
                                };
                                serde_json::json!({
                                    "role": "tool",
                                    "tool_call_id": call_id,
                                    "content": body,
                                    "is_error": output.is_error
                                })
                            }
                            ConversationItem::Reasoning { redacted, .. } => {
                                serde_json::json!({
                                    "role": "assistant",
                                    "reasoning": { "redacted": redacted }
                                })
                            }
                            ConversationItem::Compaction { summary } => {
                                // Compaction summaries can carry prior
                                // tool I/O verbatim, so respect the
                                // `include_tool_io` flag here too —
                                // review v5 finding around
                                // ConversationItem::Compaction leak.
                                let body = if include_tool_io {
                                    serde_json::Value::String(summary.clone())
                                } else {
                                    serde_json::Value::String(format!(
                                        "[redacted: {} bytes compaction summary]",
                                        summary.len()
                                    ))
                                };
                                serde_json::json!({
                                    "role": "system",
                                    "compaction_summary": body
                                })
                            }
                        })
                        .collect::<Vec<_>>(),
                )
                .unwrap_or_else(|_| "[]".to_string());
                provider_span.set_content(
                    puffer_observability::LANGFUSE_OBSERVATION_INPUT,
                    puffer_observability::ContentKind::Prompt,
                    &provider_input_json,
                );
            }
        }
        // Capture token usage from the streaming Usage event for the
        // provider span. Only wrap when observability is on so the
        // disabled path doesn't clone every Usage report
        // (review v6 BLOCK #3). The same wrapper also captures the
        // monotonic timestamp of the first non-usage stream event
        // (TextDelta / ThinkingDelta / ToolCallStart), which we
        // surface as TTFT (`gen_ai.response.first_token_ms`) — a
        // production-grade signal for diagnosing slow streams that
        // OTel's GenAI semconv standardizes but most agents miss.
        let observability_handle = inputs.observability.clone();
        let captured_usage = std::cell::RefCell::new(None::<TurnUsageReport>);
        let captured_first_token = std::cell::RefCell::new(None::<std::time::Instant>);
        let request_started = std::time::Instant::now();
        let result = if observability_handle.is_some() {
            let captured_usage_ref = &captured_usage;
            let captured_first_token_ref = &captured_first_token;
            let mut wrapped = |event: TurnStreamEvent| {
                match &event {
                    TurnStreamEvent::Usage(u) => {
                        *captured_usage_ref.borrow_mut() = Some(u.clone());
                    }
                    TurnStreamEvent::TextDelta(_)
                    | TurnStreamEvent::ThinkingDelta(_)
                    | TurnStreamEvent::ToolCallsRequested(_) => {
                        if captured_first_token_ref.borrow().is_none() {
                            *captured_first_token_ref.borrow_mut() =
                                Some(std::time::Instant::now());
                        }
                    }
                    _ => {}
                }
                on_event(event);
            };
            session.one_turn_streaming(inputs.state, inputs.auth_store, &mut items, &mut wrapped)
        } else {
            session.one_turn_streaming(inputs.state, inputs.auth_store, &mut items, on_event)
        };
        // Surface usage on the provider span before propagating any
        // error so failed calls still record their token cost.
        if let Some(u) = captured_usage.into_inner() {
            provider_span.set_token_usage(
                Some(u.input_tokens),
                Some(u.output_tokens),
                Some(u.cache_read_tokens),
            );
            if u.cache_creation_tokens > 0 {
                provider_span.set_str(
                    "gen_ai.usage.cache_creation_input_tokens",
                    u.cache_creation_tokens.to_string(),
                );
            }
            // Cache hit ratio = cache_read / input. Surfaces a single
            // top-line metric in Langfuse without making the viewer do
            // the arithmetic.
            if u.input_tokens > 0 {
                let ratio = u.cache_read_tokens as f64 / u.input_tokens as f64;
                provider_span.set_f64("puffer.cache.hit_ratio", ratio);
            }
        }
        // Time-to-first-token: monotonic ms from request start to the
        // first stream event that carries content. Numeric so Langfuse
        // can chart latency percentiles.
        if let Some(first) = captured_first_token.into_inner() {
            let ttft_ms = first.duration_since(request_started).as_secs_f64() * 1000.0;
            provider_span.set_f64("gen_ai.response.first_token_ms", ttft_ms);
        }
        let turn = match result {
            Ok(turn) => turn,
            Err(error) => {
                provider_span.mark_error(error.to_string());
                turn_span.mark_error(error.to_string());
                agent_span.mark_error(error.to_string());
                drop(observability_handle);
                return Err(error);
            }
        };
        provider_span.set_content(
            puffer_observability::LANGFUSE_OBSERVATION_OUTPUT,
            puffer_observability::ContentKind::Output,
            &turn.assistant_text,
        );
        provider_span.set_content(
            "puffer.assistant_text",
            puffer_observability::ContentKind::Output,
            &turn.assistant_text,
        );
        provider_span.end();

        // Cancel boundary: check after the LLM call returns, before
        // tool execution kicks off. This prevents the loop from
        // launching a fresh tool batch when the user already pressed
        // ESC during streaming.
        if let Some(cancel) = inputs.cancel {
            if let Err(error) = cancel.check() {
                turn_span.mark_cancelled();
                agent_span.mark_cancelled();
                return Err(error);
            }
        }

        // No tool calls → final assistant text, run hooks, return.
        if turn.tool_calls.is_empty() {
            run_turn_hooks(
                inputs.resources,
                &cwd,
                &turn.assistant_text,
                invocations.len(),
            );
            agent_span.set_content(
                puffer_observability::LANGFUSE_TRACE_OUTPUT,
                puffer_observability::ContentKind::Output,
                &turn.assistant_text,
            );
            agent_span.set_content(
                "puffer.output",
                puffer_observability::ContentKind::Output,
                &turn.assistant_text,
            );
            return Ok(TurnExecution {
                assistant_text: turn.assistant_text,
                tool_invocations: invocations,
                reflection_traces,
            });
        }

        // Append response items (assistant text + reasoning + FunctionCall) BEFORE running tools.
        items.extend(turn.pre_tool_items);

        // Suppress duplicate ToolCallsRequested for ids the session
        // already surfaced via streaming events.
        let pending_for_event: Vec<ToolCallRequest> = turn
            .tool_calls
            .iter()
            .filter(|tc| !turn.emitted_tool_call_ids.contains(&tc.call_id))
            .cloned()
            .collect();
        if !pending_for_event.is_empty() {
            on_event(TurnStreamEvent::ToolCallsRequested(pending_for_event));
        }

        // Execute tools (sequential — parallel-safe batching is a follow-up).
        let new_invocations = match execute_tool_batch(
            inputs,
            session,
            &cwd,
            &turn.tool_calls,
            turn_span.context(),
        ) {
            Ok(v) => v,
            Err(error) => {
                turn_span.mark_error(error.to_string());
                agent_span.mark_error(error.to_string());
                return Err(error);
            }
        };

        if !new_invocations.is_empty() {
            on_event(TurnStreamEvent::ToolInvocations(new_invocations.clone()));
        }

        // Append FunctionCallOutput items.
        for inv in &new_invocations {
            items.push(ConversationItem::FunctionCallOutput {
                call_id: inv.call_id.clone(),
                output: if inv.success {
                    ToolOutputPayload::success(inv.output.clone())
                } else {
                    ToolOutputPayload::error(inv.output.clone())
                },
            });
        }

        invocations.extend(new_invocations.iter().cloned());

        // Pi-mono parity: early-terminate when EVERY invocation in the
        // batch sets `terminate: true`. The loop returns immediately
        // with the assistant text we have so far (typically empty for a
        // tool-only turn — that is fine, the tool itself owns the user
        // signal). See `pi-mono/packages/agent/src/agent-loop.ts:499`.
        if !new_invocations.is_empty() && new_invocations.iter().all(|inv| inv.terminate) {
            run_turn_hooks(
                inputs.resources,
                &cwd,
                &turn.assistant_text,
                invocations.len(),
            );
            return Ok(TurnExecution {
                assistant_text: turn.assistant_text,
                tool_invocations: invocations,
                reflection_traces,
            });
        }

        // Reflection is the only LLM round-trip puffer makes outside
        // the main `agent_loop → turn → provider_call` path. Mark it
        // as a subagent so the Langfuse tree visibly distinguishes it
        // from regular provider calls (kind attribute + span-name
        // prefix). Every stage of the reflection pipeline is mirrored
        // onto attributes so a viewer can tell exactly what happened
        // (config-disabled / judge-skipped / code-judge-fired /
        // llm-judge-fired-with-decision-X / checkpoint-injected).
        // Capture start_time before the observe call so the inner
        // judge generation span (created post-hoc when we see the
        // LlmJudgeRequest event) can backdate to the real LLM call
        // start — review v5 BLOCK #3.
        let reflection_start_time = std::time::SystemTime::now();
        let mut reflection_span = puffer_observability::start_reflection_span(
            inputs.observability.as_ref(),
            turn_span.context(),
        );
        if inputs.observability.is_some() {
            reflection_span.set_str(
                "puffer.reflection.config.enabled",
                inputs.reflection_config.is_some().to_string(),
            );
        }
        // Inner subagent generation span — only created if the LLM
        // judge actually fires. Cached lazily so the reflection
        // wrapper stays a plain SPAN when nothing of substance happens.
        let mut judge_gen_span: Option<puffer_observability::SpanGuard> = None;
        if let Some(observation) = reflection.as_mut().and_then(|tracker| {
            tracker.observe_batch_with_judge(
                &new_invocations,
                &items,
                inputs.state,
                inputs.resources,
                inputs.providers,
                inputs.auth_store,
            )
        }) {
            // Always emit ReflectionTrace events for the TUI / trajectory.
            for trace_event in &observation.trace_events {
                on_event(TurnStreamEvent::ReflectionTrace(trace_event.clone()));
            }
            // Span attribute mutations are gated on a live handle so
            // the disabled path skips all `to_string()` / `clone()`
            // (review v5 BLOCK #2).
            if inputs.observability.is_some() {
                reflection_span.set_str("puffer.reflection.observed", "true");
                for trace_event in &observation.trace_events {
                    match trace_event {
                        ReflectionTraceEvent::BatchObserved {
                            evaluation_score,
                            evaluation_threshold,
                            should_evaluate,
                            skip_reason,
                            ..
                        } => {
                            reflection_span.set_str(
                                "puffer.reflection.assessment.score",
                                evaluation_score.to_string(),
                            );
                            reflection_span.set_str(
                                "puffer.reflection.assessment.threshold",
                                evaluation_threshold.to_string(),
                            );
                            reflection_span.set_str(
                                "puffer.reflection.assessment.should_evaluate",
                                should_evaluate.to_string(),
                            );
                            if let Some(reason) = skip_reason {
                                reflection_span.set_str(
                                    "puffer.reflection.assessment.skip_reason",
                                    reason.clone(),
                                );
                            }
                        }
                        ReflectionTraceEvent::CodeJudgeDecision {
                            triggered,
                            score,
                            threshold,
                            ..
                        } => {
                            reflection_span.set_str(
                                "puffer.reflection.code_judge.triggered",
                                triggered.to_string(),
                            );
                            reflection_span
                                .set_str("puffer.reflection.code_judge.score", score.to_string());
                            reflection_span.set_str(
                                "puffer.reflection.code_judge.threshold",
                                threshold.to_string(),
                            );
                        }
                        ReflectionTraceEvent::LlmJudgeSkipped { mode, reason } => {
                            reflection_span.set_str("puffer.reflection.llm_judge.fired", "false");
                            reflection_span
                                .set_str("puffer.reflection.llm_judge.skip_mode", mode.clone());
                            reflection_span
                                .set_str("puffer.reflection.llm_judge.skip_reason", reason.clone());
                        }
                        ReflectionTraceEvent::LlmJudgeRequest {
                            provider,
                            model,
                            context_scope,
                            prompt_chars,
                            prompt,
                            ..
                        } => {
                            // First evidence the judge actually fired:
                            // create the inner subagent generation span,
                            // backdated to the start of `observe_batch_with_judge`
                            // so the span bounds the real LLM latency.
                            let mut span = puffer_observability::start_subagent_generation_span_at(
                                inputs.observability.as_ref(),
                                reflection_span.context(),
                                "reflection_judge",
                                Some(reflection_start_time),
                            );
                            if let Some(p) = provider {
                                span.set_str(puffer_observability::GEN_AI_SYSTEM, p.clone());
                            }
                            if let Some(m) = model {
                                span.set_str(puffer_observability::GEN_AI_REQUEST_MODEL, m.clone());
                            }
                            span.set_str("puffer.reflection.context_scope", context_scope.clone());
                            span.set_str(
                                "puffer.reflection.prompt_chars",
                                prompt_chars.to_string(),
                            );
                            // Reflection's prompt embeds rendered tool
                            // calls + outputs (`render_items`), so gating
                            // on `include_prompts` alone would leak tool
                            // I/O. Require BOTH flags before emitting the
                            // raw prompt (review v4 BLOCK #3).
                            if let Some(handle) = inputs.observability.as_ref() {
                                let policy = handle.redaction();
                                if policy.include_prompts() && policy.include_tool_io() {
                                    span.set_content(
                                        puffer_observability::LANGFUSE_OBSERVATION_INPUT,
                                        puffer_observability::ContentKind::Prompt,
                                        prompt,
                                    );
                                }
                            }
                            judge_gen_span = Some(span);
                            reflection_span.set_str("puffer.reflection.llm_judge.fired", "true");
                        }
                        ReflectionTraceEvent::LlmJudgeResponse {
                            decision,
                            confidence,
                            reason,
                            next_action,
                            input_tokens,
                            output_tokens,
                            cached_input_tokens,
                            raw_response_text,
                            ..
                        } => {
                            if let Some(span) = judge_gen_span.as_mut() {
                                span.set_str("puffer.reflection.decision", decision.clone());
                                if let Some(c) = confidence {
                                    span.set_str("puffer.reflection.confidence", c.clone());
                                }
                                span.set_str("puffer.reflection.next_action", next_action.clone());
                                span.set_token_usage(
                                    *input_tokens,
                                    *output_tokens,
                                    *cached_input_tokens,
                                );
                                span.set_content(
                                    puffer_observability::LANGFUSE_OBSERVATION_OUTPUT,
                                    puffer_observability::ContentKind::Output,
                                    raw_response_text,
                                );
                                // Cache hit ratio: derive locally so the
                                // viewer sees the percentage even when
                                // upstream doesn't surface it directly.
                                if let Some(in_tok) = input_tokens {
                                    if *in_tok > 0 {
                                        let ratio = cached_input_tokens.unwrap_or(0) as f64
                                            / *in_tok as f64;
                                        span.set_f64("puffer.cache.hit_ratio", ratio);
                                    }
                                }
                            }
                            reflection_span
                                .set_str("puffer.reflection.llm_judge.decision", decision.clone());
                            reflection_span
                                .set_str("puffer.reflection.llm_judge.reason", reason.clone());
                        }
                        ReflectionTraceEvent::LlmJudgeError { stage, error, .. } => {
                            if let Some(span) = judge_gen_span.as_mut() {
                                span.mark_error(error.clone());
                            }
                            reflection_span
                                .set_str("puffer.reflection.llm_judge.error_stage", stage.clone());
                        }
                        ReflectionTraceEvent::FinalDecision {
                            selected_source,
                            triggered_checkpoint,
                            ..
                        } => {
                            if let Some(src) = selected_source {
                                reflection_span
                                    .set_str("puffer.reflection.final.signal_source", src.clone());
                            }
                            reflection_span.set_str(
                                "puffer.reflection.final.checkpoint_triggered",
                                triggered_checkpoint.to_string(),
                            );
                        }
                    }
                }
            }
            reflection_traces.extend(observation.trace_events);
            if let Some(checkpoint) = observation.checkpoint {
                on_event(TurnStreamEvent::ReflectionCheckpoint(
                    checkpoint.summary.clone(),
                ));
                items.push(ConversationItem::user_message(checkpoint.prompt));
            }
        } else if inputs.observability.is_some() {
            reflection_span.set_str("puffer.reflection.observed", "false");
        }
        if let Some(span) = judge_gen_span {
            span.end();
        }
        reflection_span.end();

        // Post-iteration compaction. The compaction trigger itself
        // may issue an LLM round-trip via `session.generate_summary`
        // — wrapping in a span lets Langfuse see the cost. Most turns
        // skip compaction (transcript under threshold), so we mark
        // skipped spans with `puffer.compaction.skipped=true` instead
        // of suppressing them.
        let mut post_compaction_span = puffer_observability::start_compaction_span(
            inputs.observability.as_ref(),
            turn_span.context(),
            1,
        );
        let compacted = {
            let observability = inputs.observability.as_ref();
            let parent_ctx = post_compaction_span.context();
            let summary_fn = |old: &str, mid: &str| -> Option<String> {
                let mut gen_span = puffer_observability::start_subagent_generation_span(
                    observability,
                    parent_ctx,
                    "compaction_summary",
                );
                gen_span.set_str("puffer.compaction.phase", "1");
                let result = session.generate_summary(old, mid);
                if let Some(ref text) = result {
                    gen_span.set_content(
                        puffer_observability::LANGFUSE_OBSERVATION_OUTPUT,
                        puffer_observability::ContentKind::OutputWithEmbeddedToolIo,
                        text,
                    );
                } else {
                    gen_span.mark_error("summary_returned_none".to_string());
                }
                gen_span.end();
                result
            };
            compact_conversation_with(
                &mut items,
                inputs.provider,
                inputs.model_id,
                turn.input_tokens_hint,
                &summary_fn,
            )
        };
        if compacted {
            inject_post_compact_context(&mut items, &cwd);
            session.notify_compacted();
        } else {
            post_compaction_span.set_str("puffer.compaction.skipped", "true");
        }
        post_compaction_span.end();
    }
}

// `execute_tool_batch` lives in `runtime/tool_batch.rs`; the streaming
// loop above and `blocking_loop::run_blocking_loop` call into it.

/// Runs one microcompact pass and, on a successful clear, appends a
/// boundary `<system-reminder>` user message describing what happened.
/// Mirrors `services/compact/microCompact.ts:484` (boundary insertion).
///
/// Disabled by default — `MicrocompactConfig::from_env()` returns `None`
/// unless `PUFFER_MICROCOMPACT=1` is set. See env var docs in
/// `microcompact.rs`. When the trait grows a typed observability hook
/// for context-edit operations, swap the println for that.
pub(crate) fn maybe_microcompact(
    state: &crate::AppState,
    items: &mut Vec<ConversationItem>,
    span: Option<&mut puffer_observability::SpanGuard>,
) -> Option<MicrocompactOutcome> {
    let config = MicrocompactConfig::from_env()?;
    let inputs = MicrocompactInputs {
        last_assistant_at: state.last_assistant_at,
        last_input_tokens: state.last_input_tokens,
        config: &config,
    };
    let outcome = microcompact_in_place(items.as_mut_slice(), &inputs)?;
    let trigger_str = match outcome.trigger {
        MicrocompactTrigger::TimeGap => "time_gap",
        MicrocompactTrigger::TokenBudget => "token_budget",
    };
    let boundary = format!(
        "<system-reminder>\n[microcompact] cleared {} stale tool result{} (~{} tokens reclaimed via {} trigger). \
Their call ids remain in context — re-run the tool if you need the content again.\n</system-reminder>",
        outcome.cleared_count,
        if outcome.cleared_count == 1 { "" } else { "s" },
        outcome.tokens_saved,
        trigger_str,
    );
    items.push(ConversationItem::user_message(&boundary));
    if let Some(span) = span {
        span.set_str("puffer.microcompact.trigger", trigger_str);
        span.set_str(
            "puffer.microcompact.cleared_count",
            outcome.cleared_count.to_string(),
        );
        span.set_str(
            "puffer.microcompact.tokens_saved",
            outcome.tokens_saved.to_string(),
        );
        span.set_str(
            "puffer.microcompact.cleared_call_ids",
            outcome.cleared_call_ids.join(","),
        );
    }
    Some(outcome)
}

#[cfg(test)]
mod cancel_token_tests {
    use super::CancelToken;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn fresh_token_is_not_cancelled() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled());
        assert!(t.check().is_ok());
    }

    #[test]
    fn cancel_flips_state() {
        let t = CancelToken::new();
        t.cancel();
        assert!(t.is_cancelled());
        assert!(t.check().is_err());
    }

    #[test]
    fn cancel_is_idempotent() {
        let t = CancelToken::new();
        t.cancel();
        t.cancel();
        t.cancel();
        assert!(t.is_cancelled());
    }

    #[test]
    fn clone_shares_state() {
        let t1 = CancelToken::new();
        let t2 = t1.clone();
        assert!(!t2.is_cancelled());
        t1.cancel();
        assert!(t2.is_cancelled());
    }

    /// The token must be safe to flip from another thread. The agent
    /// loop runs in a worker thread; the TUI's ESC handler runs on the
    /// main thread.
    #[test]
    fn cancellable_across_threads() {
        let t = CancelToken::new();
        let observed = Arc::new(AtomicBool::new(false));
        let observed_in_worker = Arc::clone(&observed);
        let worker_token = t.clone();
        let worker = thread::spawn(move || {
            for _ in 0..100 {
                if worker_token.is_cancelled() {
                    observed_in_worker.store(true, Ordering::Relaxed);
                    return;
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        thread::sleep(Duration::from_millis(5));
        t.cancel();
        worker.join().unwrap();
        assert!(observed.load(Ordering::Relaxed));
    }
}
