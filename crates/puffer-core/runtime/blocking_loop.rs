//! Non-streaming turn loop, split out of `agent_loop.rs` to keep that
//! module under the repo's 1000-line file-size limit. The blocking
//! loop is invoked by `execute_user_prompt_with_options` for the
//! synchronous provider transports (Anthropic blocking JSON with
//! 413/`prompt_too_long` recovery, plus all spawned-agent /
//! teammate / reflection-judge side calls). Span scaffolding mirrors
//! the streaming loop so spawned subagents are visible in Langfuse.

use anyhow::Result;

use super::agent_loop::{maybe_microcompact, LoopInputs, TurnSession};
use super::openai::conversation::{
    compact_conversation_with, inject_post_compact_context, transcript_to_items, ConversationItem,
    ToolOutputPayload,
};
use super::reflection::{ReflectionTraceEvent, ReflectionTracker};
use super::tool_batch::execute_tool_batch;
use super::{run_turn_hooks, ToolInvocation, TurnExecution};

/// Non-streaming turn loop. Same shape as streaming but uses
/// `one_turn_blocking` so providers can route through their non-stream
/// transport (Anthropic blocking JSON, with 413 recovery).
pub(crate) fn run_blocking_loop(
    inputs: &mut LoopInputs<'_>,
    session: &mut dyn TurnSession,
) -> Result<TurnExecution> {
    let cwd = inputs.state.cwd.clone();

    let mut items = transcript_to_items(inputs.state, inputs.input);
    session.pre_loop_inject(&mut items);

    let mut invocations: Vec<ToolInvocation> = Vec::new();
    let mut reflection_traces: Vec<ReflectionTraceEvent> = Vec::new();
    // Carries the most recent assistant text across iterations so the
    // `max_turns` break path can return whatever reasoning the model
    // produced before its budget was cut. Empty until the first
    // provider response.
    let mut last_assistant_text = String::new();
    let mut reflection = inputs
        .reflection_config
        .clone()
        .map(|config| ReflectionTracker::new(inputs.input, config));

    // Span scaffolding parity with the streaming loop. Spawned agents
    // / teammates / reflection judge route through this path, so
    // without these spans they'd be invisible in Langfuse. Review v3
    // #3 flagged the gap. Token usage isn't surfaced from
    // `one_turn_blocking` today — provider spans are duration-only
    // until the trait grows a usage return.
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
        // Subagent runs may embed tool I/O in their input prompt; use
        // `PromptWithEmbeddedToolIo` so redaction requires both flags.
        // Always emit so the trace's Input pane shows a redacted
        // summary instead of `null` when the flag is off.
        let kind = if is_subagent {
            puffer_observability::ContentKind::PromptWithEmbeddedToolIo
        } else {
            puffer_observability::ContentKind::Prompt
        };
        span.set_content(
            puffer_observability::LANGFUSE_TRACE_INPUT,
            kind,
            inputs.input,
        );
        span
    } else {
        puffer_observability::SpanGuard::Disabled
    };

    {
        let mut compaction_span = puffer_observability::start_compaction_span(
            inputs.observability.as_ref(),
            agent_span.context(),
            0,
        );
        // Wrap the actual `session.generate_summary` LLM call inside
        // the closure so the `subagent.compaction_summary` GENERATION
        // span bounds real LLM latency and any error — review v4 #2.
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
        if did {
            inject_post_compact_context(&mut items, &cwd);
        } else {
            compaction_span.set_str("puffer.compaction.skipped", "true");
        }
        compaction_span.end();
    }
    session.notify_compacted();

    // Pre-loop microcompact. Same idempotence and rationale as the
    // streaming loop. Disabled unless `PUFFER_MICROCOMPACT=1`.
    maybe_microcompact(inputs.state, &mut items, Some(&mut agent_span));

    let mut turn_index: u32 = 0;
    loop {
        if let Some(cancel) = inputs.cancel {
            if let Err(error) = cancel.check() {
                agent_span.mark_cancelled();
                return Err(error);
            }
        }
        // Hard cap on inner-loop iterations — see streaming loop. Used
        // by reflection sub-agents to bound a grader's tool-use budget.
        // Returns whatever assistant text the model produced on its
        // last full round-trip; tool calls from the in-progress turn
        // are abandoned (the budget is exhausted, by design).
        if let Some(max_turns) = inputs.max_turns {
            if turn_index >= max_turns {
                agent_span.set_str("puffer.subagent.max_turns_reached", "true");
                agent_span.set_content(
                    puffer_observability::LANGFUSE_TRACE_OUTPUT,
                    puffer_observability::ContentKind::Output,
                    &last_assistant_text,
                );
                return Ok(TurnExecution {
                    assistant_text: last_assistant_text,
                    tool_invocations: invocations,
                    reflection_traces,
                });
            }
        }
        // Per-iteration microcompact (idempotent — see streaming loop comment).
        maybe_microcompact(inputs.state, &mut items, Some(&mut agent_span));
        let mut turn_span = puffer_observability::start_turn_span(
            inputs.observability.as_ref(),
            agent_span.context(),
            turn_index,
        );
        turn_index += 1;
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
        let turn = match session.one_turn_blocking(inputs.state, inputs.auth_store, &mut items) {
            Ok(t) => t,
            Err(error) => {
                provider_span.mark_error(error.to_string());
                turn_span.mark_error(error.to_string());
                agent_span.mark_error(error.to_string());
                return Err(error);
            }
        };
        last_assistant_text.clone_from(&turn.assistant_text);
        // Goal accounting parity with the streaming loop. The blocking
        // transport can't emit `TurnStreamEvent::Usage` (no event
        // channel), so providers surface usage on
        // `AssistantTurn.usage_report` (populated by the Anthropic
        // blocking JSON path — `runtime/anthropic.rs:turn_from_response`).
        // Without this hook, `/goal` budgets never trip for blocking
        // entrypoints (subagents, teammates, the reflection judge).
        if let Some(ref usage) = turn.usage_report {
            provider_span.set_token_usage(
                Some(usage.input_tokens),
                Some(usage.output_tokens),
                Some(usage.cache_read_tokens),
            );
            if usage.cache_creation_tokens > 0 {
                provider_span.set_str(
                    "gen_ai.usage.cache_creation_input_tokens",
                    usage.cache_creation_tokens.to_string(),
                );
            }
            super::goals::account_token_usage(inputs.state, usage);
        }
        provider_span.set_content(
            puffer_observability::LANGFUSE_OBSERVATION_OUTPUT,
            puffer_observability::ContentKind::Output,
            &turn.assistant_text,
        );
        provider_span.end();
        if let Some(cancel) = inputs.cancel {
            if let Err(error) = cancel.check() {
                turn_span.mark_cancelled();
                agent_span.mark_cancelled();
                return Err(error);
            }
        }

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
            return Ok(TurnExecution {
                assistant_text: turn.assistant_text,
                tool_invocations: invocations,
                reflection_traces,
            });
        }

        items.extend(turn.pre_tool_items);

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

        // Pi-mono parity: early-terminate on unanimous `terminate: true`.
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

        if let Some(observation) = reflection.as_mut().and_then(|tracker| {
            tracker.observe_batch_with_judge(
                &new_invocations,
                &items,
                inputs.state,
                inputs.resources,
                inputs.providers,
                inputs.auth_store,
                inputs.cancel,
            )
        }) {
            reflection_traces.extend(observation.trace_events);
            if let Some(checkpoint) = observation.checkpoint {
                items.push(ConversationItem::user_message(checkpoint.prompt));
            }
        }

        // Post-iteration compaction. Mirrors the streaming loop:
        // wrapper SPAN around the threshold check, inner GENERATION
        // around the actual summary LLM call so latency/errors land
        // on the right span (review v4 #2).
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
