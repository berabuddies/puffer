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

use super::openai::conversation::{
    compact_conversation_with, inject_post_compact_context, transcript_to_items, ConversationItem,
    ToolOutputPayload,
};
use super::reflection::{ReflectionConfig, ReflectionTraceEvent, ReflectionTracker};
use super::request_tool_filter::RequestToolFilter;
use super::claude_tools::{self, ProviderToolContext};
use super::tool_executor::{
    execute_tool_call, is_parallel_safe_tool, resolve_tool_permission, PermissionOutcome,
    ToolExecutionBackend,
};
use crate::workspace_paths;
use super::{
    enforce_tool_result_budget, process_tool_result, run_turn_hooks, CancelToken, ToolCallRequest,
    ToolInvocation, TurnExecution, TurnStreamEvent, MAX_TOOL_RESULT_CHARS,
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

    // Pre-turn compaction.
    let pre_compacted = {
        let summary_fn = |old: &str, mid: &str| session.generate_summary(old, mid);
        compact_conversation_with(&mut items, inputs.provider, inputs.model_id, None, &summary_fn)
    };
    if pre_compacted {
        inject_post_compact_context(&mut items, &cwd);
        session.notify_compacted();
    }

    loop {
        // Cancel boundary: check before each turn's provider round-trip.
        if let Some(cancel) = inputs.cancel {
            cancel.check()?;
        }

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

        // Provider single round-trip.
        let turn = session.one_turn_streaming(inputs.state, inputs.auth_store, &mut items, on_event)?;

        // Cancel boundary: check after the LLM call returns, before
        // tool execution kicks off. This prevents the loop from
        // launching a fresh tool batch when the user already pressed
        // ESC during streaming.
        if let Some(cancel) = inputs.cancel {
            cancel.check()?;
        }

        // No tool calls → final assistant text, run hooks, return.
        if turn.tool_calls.is_empty() {
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
        let new_invocations = execute_tool_batch(inputs, session, &cwd, &turn.tool_calls)?;

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
        if !new_invocations.is_empty()
            && new_invocations.iter().all(|inv| inv.terminate)
        {
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

        // Reflection observation over THIS batch only.
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

        // Post-iteration compaction.
        let compacted = {
            let summary_fn = |old: &str, mid: &str| session.generate_summary(old, mid);
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
        }
    }
}

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
    let mut reflection = inputs
        .reflection_config
        .clone()
        .map(|config| ReflectionTracker::new(inputs.input, config));

    {
        let summary_fn = |old: &str, mid: &str| session.generate_summary(old, mid);
        if compact_conversation_with(&mut items, inputs.provider, inputs.model_id, None, &summary_fn)
        {
            inject_post_compact_context(&mut items, &cwd);
        }
    }
    session.notify_compacted();

    loop {
        if let Some(cancel) = inputs.cancel {
            cancel.check()?;
        }
        let turn = session.one_turn_blocking(inputs.state, inputs.auth_store, &mut items)?;
        if let Some(cancel) = inputs.cancel {
            cancel.check()?;
        }

        if turn.tool_calls.is_empty() {
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

        items.extend(turn.pre_tool_items);

        let new_invocations = execute_tool_batch(inputs, session, &cwd, &turn.tool_calls)?;

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
        if !new_invocations.is_empty()
            && new_invocations.iter().all(|inv| inv.terminate)
        {
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
            )
        }) {
            reflection_traces.extend(observation.trace_events);
            if let Some(checkpoint) = observation.checkpoint {
                items.push(ConversationItem::user_message(checkpoint.prompt));
            }
        }

        let compacted = {
            let summary_fn = |old: &str, mid: &str| session.generate_summary(old, mid);
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
        }
    }
}

/// Executes one batch of tool calls produced by a single assistant turn.
///
/// Mirrors the existing serial behavior of `execute_anthropic_tool_calls`
/// (head-truncation per tool, aggregate budget). Parallel-safe batching
/// Mirrors the OLD `execute_openai_tool_calls` parallel batching for ALL
/// providers: parallel-safe tools run concurrently in a `thread::scope`,
/// the rest fall back to serial execution with full `&mut state` access.
/// Permission resolution always runs serially up front because
/// `AllowSession` mutates `state`.
fn execute_tool_batch(
    inputs: &mut LoopInputs<'_>,
    session: &mut dyn TurnSession,
    cwd: &std::path::Path,
    tool_calls: &[ToolCallRequest],
) -> Result<Vec<ToolInvocation>> {
    let parallel_count = tool_calls
        .iter()
        .filter(|tc| is_parallel_safe_tool(&tc.tool_id))
        .count();

    // Serial fast-path: no parallelism gain available.
    if tool_calls.len() <= 1 || parallel_count <= 1 {
        return execute_tool_batch_serial(inputs, session, cwd, tool_calls);
    }

    // Phase 1: resolve permissions serially (mutates state on AllowSession).
    let mut permissions: Vec<PermissionOutcome> = Vec::with_capacity(tool_calls.len());
    for tc in tool_calls {
        let args: serde_json::Value =
            serde_json::from_str(&tc.input).unwrap_or(serde_json::Value::Null);
        permissions.push(resolve_tool_permission(
            inputs.state,
            inputs.resources,
            inputs.registry,
            cwd,
            &tc.tool_id,
            &args,
            inputs.tool_filter,
        )?);
    }

    // Phase 2: spawn parallel-safe tools. We snapshot the immutable
    // bits of state we need so the parallel closures only borrow
    // refs, never `&mut state`.
    let working_dirs = inputs.state.working_dirs.clone();
    let allow_all_paths = workspace_paths::sandbox_allows_all_paths(&inputs.state.sandbox_mode);
    let session_id = inputs.state.session.id;
    let provider_context = backend_to_provider_context(
        session.tool_execution_backend(),
        inputs.model_id,
    );

    let mut results: Vec<Option<(String, bool, bool)>> = vec![None; tool_calls.len()];

    std::thread::scope(|s| {
        let mut handles: Vec<(usize, std::thread::ScopedJoinHandle<'_, (String, bool, bool)>)> =
            Vec::new();
        for (i, tc) in tool_calls.iter().enumerate() {
            if !is_parallel_safe_tool(&tc.tool_id) {
                continue;
            }
            if let PermissionOutcome::Denied(ref denied) = permissions[i] {
                results[i] = Some((
                    denied.output.stdout.clone(),
                    denied.success,
                    extract_terminate(&denied.output.metadata),
                ));
                continue;
            }
            let definition = match inputs.registry.definition(&tc.tool_id) {
                Some(d) => d.clone(),
                None => {
                    results[i] = Some((format!("unknown tool {}", tc.tool_id), false, false));
                    continue;
                }
            };
            let args: serde_json::Value =
                serde_json::from_str(&tc.input).unwrap_or(serde_json::Value::Null);
            let resources = inputs.resources;
            let registry = inputs.registry;
            let working_dirs_ref = &working_dirs;
            let provider_context_ref = &provider_context;
            handles.push((
                i,
                s.spawn(move || {
                    match claude_tools::execute_parallel_tool(
                        &definition,
                        cwd,
                        working_dirs_ref,
                        allow_all_paths,
                        &session_id,
                        args,
                        resources,
                        registry,
                        provider_context_ref,
                    ) {
                        Ok(exec) => {
                            let terminate = extract_terminate(&exec.output.metadata);
                            let output = if exec.output.stderr.is_empty() {
                                exec.output.stdout
                            } else if exec.output.stdout.is_empty() {
                                exec.output.stderr
                            } else {
                                format!("{}\n{}", exec.output.stdout, exec.output.stderr)
                            };
                            (output, exec.success, terminate)
                        }
                        Err(error) => (format!("Tool execution failed: {error}"), false, false),
                    }
                }),
            ));
        }
        for (i, handle) in handles {
            results[i] = Some(
                handle
                    .join()
                    .unwrap_or_else(|_| ("Tool execution panicked".to_string(), false, false)),
            );
        }
    });

    // Phase 3: serial execution for non-parallel + denied (and unknown
    // tool fallthroughs that we did not pre-fill above).
    for (i, tc) in tool_calls.iter().enumerate() {
        if results[i].is_some() {
            continue;
        }
        if let PermissionOutcome::Denied(ref denied) = permissions[i] {
            results[i] = Some((
                denied.output.stdout.clone(),
                denied.success,
                extract_terminate(&denied.output.metadata),
            ));
            continue;
        }
        let backend = session.tool_execution_backend();
        let args: serde_json::Value =
            serde_json::from_str(&tc.input).unwrap_or(serde_json::Value::Null);
        let exec = match execute_tool_call(
            inputs.state,
            inputs.resources,
            inputs.providers,
            inputs.auth_store,
            inputs.registry,
            inputs.model_id,
            cwd,
            backend,
            inputs.tool_filter,
            &tc.tool_id,
            args,
        ) {
            Ok(exec) => {
                let terminate = extract_terminate(&exec.output.metadata);
                let output = if exec.output.stderr.is_empty() {
                    exec.output.stdout
                } else if exec.output.stdout.is_empty() {
                    exec.output.stderr
                } else {
                    format!("{}\n{}", exec.output.stdout, exec.output.stderr)
                };
                (output, exec.success, terminate)
            }
            Err(error) => (format!("Tool execution failed: {error}"), false, false),
        };
        results[i] = Some(exec);
    }

    // Phase 4: assemble in original order with per-tool truncation.
    let mut invocations = Vec::with_capacity(tool_calls.len());
    for (i, tc) in tool_calls.iter().enumerate() {
        let (raw_output, success, terminate) = results[i]
            .take()
            .unwrap_or_else(|| ("Tool was not executed".to_string(), false, false));
        let output_text = process_tool_result(
            &raw_output,
            MAX_TOOL_RESULT_CHARS,
            &inputs.state.session.id,
        );
        invocations.push(ToolInvocation {
            call_id: tc.call_id.clone(),
            tool_id: tc.tool_id.clone(),
            input: tc.input.clone(),
            output: output_text,
            success,
            terminate,
        });
    }

    enforce_tool_result_budget_in_place(&mut invocations, &inputs.state.session.id);
    Ok(invocations)
}

/// Serial fallback used when parallelism would not help. Mirrors the
/// earlier serial path bit-for-bit.
fn execute_tool_batch_serial(
    inputs: &mut LoopInputs<'_>,
    session: &mut dyn TurnSession,
    cwd: &std::path::Path,
    tool_calls: &[ToolCallRequest],
) -> Result<Vec<ToolInvocation>> {
    let mut invocations = Vec::with_capacity(tool_calls.len());

    for call in tool_calls {
        let backend = session.tool_execution_backend();
        let input_value: serde_json::Value =
            serde_json::from_str(&call.input).unwrap_or(serde_json::Value::Null);
        let execution = execute_tool_call(
            inputs.state,
            inputs.resources,
            inputs.providers,
            inputs.auth_store,
            inputs.registry,
            inputs.model_id,
            cwd,
            backend,
            inputs.tool_filter,
            &call.tool_id,
            input_value,
        )?;
        let terminate = extract_terminate(&execution.output.metadata);
        let raw_output = if execution.output.stderr.is_empty() {
            execution.output.stdout
        } else if execution.output.stdout.is_empty() {
            execution.output.stderr
        } else {
            format!("{}\n{}", execution.output.stdout, execution.output.stderr)
        };
        let output_text = process_tool_result(
            &raw_output,
            MAX_TOOL_RESULT_CHARS,
            &inputs.state.session.id,
        );
        invocations.push(ToolInvocation {
            call_id: call.call_id.clone(),
            tool_id: call.tool_id.clone(),
            input: call.input.clone(),
            output: output_text,
            success: execution.success,
            terminate,
        });
    }

    enforce_tool_result_budget_in_place(&mut invocations, &inputs.state.session.id);
    Ok(invocations)
}

/// Extracts a `terminate: true` hint from a tool's `ToolOutput.metadata`.
/// Honored only when the **entire** tool batch sets it, mirroring
/// pi-mono's "early terminate when every tool result terminates"
/// (`pi-mono/packages/agent/src/agent-loop.ts:499`).
fn extract_terminate(metadata: &serde_json::Value) -> bool {
    metadata
        .get("terminate")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn enforce_tool_result_budget_in_place(
    invocations: &mut [ToolInvocation],
    session_id: &uuid::Uuid,
) {
    let mut output_strings: Vec<String> = invocations.iter().map(|i| i.output.clone()).collect();
    enforce_tool_result_budget(&mut output_strings, session_id);
    for (i, new_output) in output_strings.into_iter().enumerate() {
        if new_output != invocations[i].output {
            invocations[i].output = new_output;
        }
    }
}

/// Map the neutral `ToolExecutionBackend` into the
/// `claude_tools::ProviderToolContext` shape that `execute_parallel_tool`
/// expects (these are isomorphic — same fields, different name space).
fn backend_to_provider_context<'a>(
    backend: ToolExecutionBackend<'a>,
    model_id: &'a str,
) -> ProviderToolContext<'a> {
    match backend {
        ToolExecutionBackend::OpenAi {
            request_config,
            structured_output,
        } => ProviderToolContext::OpenAI {
            request_config,
            model_id,
            structured_output,
        },
        ToolExecutionBackend::Anthropic {
            request_config,
            structured_output,
        } => ProviderToolContext::Anthropic {
            request_config,
            model_id,
            structured_output,
        },
    }
}

#[cfg(test)]
mod cancel_token_tests {
    use super::CancelToken;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
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

#[cfg(test)]
mod terminate_tests {
    use super::extract_terminate;
    use serde_json::json;

    #[test]
    fn missing_metadata_field_returns_false() {
        assert!(!extract_terminate(&json!({})));
    }

    #[test]
    fn explicit_true_extracts() {
        assert!(extract_terminate(&json!({ "terminate": true })));
    }

    #[test]
    fn explicit_false_extracts_false() {
        assert!(!extract_terminate(&json!({ "terminate": false })));
    }

    #[test]
    fn non_bool_value_falls_back_to_false() {
        assert!(!extract_terminate(&json!({ "terminate": "true" })));
        assert!(!extract_terminate(&json!({ "terminate": 1 })));
    }

    #[test]
    fn null_metadata_returns_false() {
        assert!(!extract_terminate(&serde_json::Value::Null));
    }

    /// Pi-mono parity: the loop terminates ONLY when every invocation in
    /// the batch sets `terminate: true`. Locks the predicate that drives
    /// the early-stop branch in `run_streaming_loop` /
    /// `run_blocking_loop`.
    #[test]
    fn batch_unanimity_predicate() {
        // Helper closure mirroring the `iter().all(|inv| inv.terminate)`
        // shape used in the loops, so refactors of the predicate get
        // caught here.
        let unanimous = |flags: &[bool]| !flags.is_empty() && flags.iter().all(|f| *f);
        assert!(unanimous(&[true]));
        assert!(unanimous(&[true, true, true]));
        assert!(!unanimous(&[true, false]));
        assert!(!unanimous(&[false]));
        // Empty batch → no early stop (loop has no batch to act on).
        assert!(!unanimous(&[]));
    }
}
