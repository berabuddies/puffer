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
use super::skill_obligation::{NoToolDecision, SkillActionObligation};
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
    let mut obligation = SkillActionObligation::default();
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
            inject_post_compact_context(&mut items, inputs.state);
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
                let assistant_text = obligation.fail_if_pending().unwrap_or(last_assistant_text);
                agent_span.set_content(
                    puffer_observability::LANGFUSE_TRACE_OUTPUT,
                    puffer_observability::ContentKind::Output,
                    &assistant_text,
                );
                return Ok(TurnExecution {
                    assistant_text,
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
            let assistant_text = match obligation.no_tool_decision() {
                NoToolDecision::Complete => turn.assistant_text,
                NoToolDecision::ContinueWithReminder(reminder) => {
                    items.extend(turn.pre_tool_items);
                    items.push(ConversationItem::user_message(reminder));
                    continue;
                }
                NoToolDecision::FailNotStarted(message) => message,
            };
            run_turn_hooks(inputs.resources, &cwd, &assistant_text, invocations.len());
            agent_span.set_content(
                puffer_observability::LANGFUSE_TRACE_OUTPUT,
                puffer_observability::ContentKind::Output,
                &assistant_text,
            );
            return Ok(TurnExecution {
                assistant_text,
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
        obligation.observe_invocations(inputs.resources, &new_invocations);

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
            inject_post_compact_context(&mut items, inputs.state);
            session.notify_compacted();
        } else {
            post_compaction_span.set_str("puffer.compaction.skipped", "true");
        }
        post_compaction_span.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::agent_loop::AssistantTurn;
    use crate::runtime::openai::conversation::ConversationItem;
    use crate::runtime::tool_executor::ToolExecutionBackend;
    use crate::runtime::{ToolCallRequest, TurnStreamEvent};
    use crate::AppState;
    use puffer_config::PufferConfig;
    use puffer_provider_openai::{OpenAIAuth, OpenAIRequestConfig};
    use puffer_provider_registry::{AuthMode, AuthStore, ProviderDescriptor, ProviderRegistry};
    use puffer_resources::{
        LoadedItem, LoadedResources, SkillSpec, SourceInfo, SourceKind, ToolSpec,
    };
    use puffer_session_store::SessionMetadata;
    use puffer_tools::ToolRegistry;
    use serde_json::json;
    use std::collections::{HashSet, VecDeque};
    use uuid::Uuid;

    struct FakeBlockingSession {
        turns: VecDeque<AssistantTurn>,
        request_config: OpenAIRequestConfig,
    }

    impl TurnSession for FakeBlockingSession {
        fn one_turn_streaming(
            &mut self,
            _state: &mut AppState,
            _auth_store: &mut AuthStore,
            _items: &mut Vec<ConversationItem>,
            _on_event: &mut dyn FnMut(TurnStreamEvent),
        ) -> Result<AssistantTurn> {
            unreachable!("blocking test should call one_turn_blocking")
        }

        fn one_turn_blocking(
            &mut self,
            _state: &mut AppState,
            _auth_store: &mut AuthStore,
            _items: &mut Vec<ConversationItem>,
        ) -> Result<AssistantTurn> {
            Ok(self.turns.pop_front().expect("scripted blocking turn"))
        }

        fn generate_summary(&self, _old_context: &str, _model_id: &str) -> Option<String> {
            None
        }

        fn tool_execution_backend(&self) -> ToolExecutionBackend<'_> {
            ToolExecutionBackend::OpenAi {
                request_config: &self.request_config,
                structured_output: None,
            }
        }
    }

    fn assistant_turn(assistant_text: &str) -> AssistantTurn {
        AssistantTurn {
            pre_tool_items: vec![ConversationItem::assistant_message(assistant_text)],
            tool_calls: Vec::new(),
            assistant_text: assistant_text.to_string(),
            input_tokens_hint: None,
            emitted_tool_call_ids: HashSet::new(),
            usage_report: None,
        }
    }

    fn tool_turn(tool_id: &str, call_id: &str, input: String) -> AssistantTurn {
        AssistantTurn {
            pre_tool_items: vec![ConversationItem::FunctionCall {
                call_id: call_id.to_string(),
                name: tool_id.to_string(),
                arguments: input.clone(),
            }],
            tool_calls: vec![ToolCallRequest {
                call_id: call_id.to_string(),
                tool_id: tool_id.to_string(),
                input,
            }],
            assistant_text: String::new(),
            input_tokens_hint: None,
            emitted_tool_call_ids: HashSet::new(),
            usage_report: None,
        }
    }

    fn loaded_tool(id: &str, description: &str, handler: &str) -> LoadedItem<ToolSpec> {
        LoadedItem {
            value: ToolSpec {
                id: id.to_string(),
                name: id.to_string(),
                description: description.to_string(),
                handler: handler.to_string(),
                ..ToolSpec::default()
            },
            source_info: SourceInfo {
                path: format!("{id}.yaml").into(),
                kind: SourceKind::Builtin,
            },
        }
    }

    fn loaded_skill(name: &str, requires_action: bool) -> LoadedItem<SkillSpec> {
        LoadedItem {
            value: SkillSpec {
                name: name.to_string(),
                description: format!("{name} description"),
                content: format!("{name} body"),
                requires_action,
                ..SkillSpec::default()
            },
            source_info: SourceInfo {
                path: format!("skills/{name}/SKILL.md").into(),
                kind: SourceKind::Builtin,
            },
        }
    }

    fn provider() -> ProviderDescriptor {
        ProviderDescriptor {
            id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            base_url: "http://127.0.0.1".to_string(),
            default_api: "openai-responses".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: Default::default(),
            query_params: Default::default(),
            discovery: None,
            media: None,
            models: vec![puffer_provider_registry::ModelDescriptor {
                id: "gpt-5".to_string(),
                display_name: "GPT-5".to_string(),
                provider: "openai".to_string(),
                api: "openai-responses".to_string(),
                context_window: 272_000,
                max_output_tokens: 16_384,
                supports_reasoning: true,
                compat: None,
                input: vec![puffer_provider_registry::Modality::Text],
                cost: None,
            }],
            chat_completions_path: None,
        }
    }

    fn request_config() -> OpenAIRequestConfig {
        OpenAIRequestConfig {
            base_url: "http://127.0.0.1".to_string(),
            version: "test".to_string(),
            auth: OpenAIAuth::None,
            originator: "puffer-test".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        }
    }

    fn run_scripted_blocking_loop(
        cwd: &std::path::Path,
        resources: &LoadedResources,
        turns: Vec<AssistantTurn>,
        max_turns: Option<u32>,
    ) -> TurnExecution {
        let registry = ToolRegistry::from_resources(resources);
        let provider = provider();
        let mut providers = ProviderRegistry::new();
        providers.register(provider.clone());
        let mut auth_store = AuthStore::default();
        let mut state = AppState::new(PufferConfig::default(), cwd.to_path_buf(), session_for(cwd));
        state.current_provider = Some("openai".to_string());
        state.current_model = Some("openai/gpt-5".to_string());
        state.grant_all_tools_for_session();
        let mut session = FakeBlockingSession {
            turns: VecDeque::from(turns),
            request_config: request_config(),
        };
        let mut inputs = LoopInputs {
            state: &mut state,
            resources,
            providers: &providers,
            provider: &provider,
            model_id: "gpt-5",
            auth_store: &mut auth_store,
            input: "make a short drama",
            reflection_config: None,
            tool_filter: None,
            registry: &registry,
            cancel: None,
            max_turns,
            observability: None,
        };

        run_blocking_loop(&mut inputs, &mut session).unwrap()
    }

    fn session_for(cwd: &std::path::Path) -> SessionMetadata {
        SessionMetadata {
            id: Uuid::new_v4(),
            display_name: None,
            generated_title: None,
            cwd: cwd.to_path_buf(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        }
    }

    #[test]
    fn blocking_loop_skill_action_obligation_reminds_then_accepts_tool() {
        let temp = tempfile::tempdir().unwrap();
        let write_path = temp.path().join("started.txt");
        let resources = LoadedResources {
            tools: vec![
                loaded_tool("Skill", "Load a skill", "runtime:skill"),
                loaded_tool("Write", "Write file", "runtime:claude_write"),
            ],
            skills: vec![loaded_skill("short-drama-generation", true)],
            ..LoadedResources::default()
        };

        let write_input =
            json!({"file_path": write_path.display().to_string(), "content": "started"})
                .to_string();
        let turn = run_scripted_blocking_loop(
            temp.path(),
            &resources,
            vec![
                tool_turn(
                    "Skill",
                    "call_skill",
                    r#"{"skill":"short-drama-generation"}"#.to_string(),
                ),
                assistant_turn("I'll start..."),
                tool_turn("Write", "call_write", write_input),
                assistant_turn("done"),
            ],
            Some(6),
        );

        assert_eq!(turn.assistant_text, "done");
        assert!(turn
            .tool_invocations
            .iter()
            .any(|invocation| invocation.tool_id == "Write" && invocation.success));
        assert_eq!(
            std::fs::read_to_string(write_path).unwrap(),
            "started",
            "Write should satisfy the action obligation by starting tool work"
        );
    }

    #[test]
    fn blocking_loop_skill_action_obligation_fails_when_turn_budget_expires() {
        let temp = tempfile::tempdir().unwrap();
        let resources = LoadedResources {
            tools: vec![loaded_tool("Skill", "Load a skill", "runtime:skill")],
            skills: vec![loaded_skill("short-drama-generation", true)],
            ..LoadedResources::default()
        };

        let turn = run_scripted_blocking_loop(
            temp.path(),
            &resources,
            vec![tool_turn(
                "Skill",
                "call_skill",
                r#"{"skill":"short-drama-generation"}"#.to_string(),
            )],
            Some(1),
        );

        assert!(turn.assistant_text.contains("No work was started"));
        assert_eq!(turn.tool_invocations.len(), 1);
        assert_eq!(turn.tool_invocations[0].tool_id, "Skill");
    }
}
