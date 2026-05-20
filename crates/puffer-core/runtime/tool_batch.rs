//! Tool-batch execution paths split out of `agent_loop.rs` to keep
//! that module under the repo's 1000-line file-size limit.
//!
//! Both the streaming and blocking turn loops call
//! [`execute_tool_batch`] after the provider returns a set of tool
//! calls. The function decides between a parallel-safe batch path
//! (multiple parallel-safe tools spawn into a `std::thread::scope`)
//! and a serial fallback. Span scaffolding mirrors the agent loop's
//! shape: each tool gets its own `tool.<id>` SPAN parented to the
//! turn span passed in via `parent_span_ctx`. Parallel-safe tools
//! must explicitly clone the parent OtelContext into the scoped
//! thread closure since OTel's thread-local context does not cross
//! `thread::scope` boundaries.

use anyhow::Result;
use puffer_tools::ToolDefinition;
use serde_json::Value;
use uuid::Uuid;

use super::agent_loop::{LoopInputs, TurnSession};
use super::claude_tools::{self, ProviderToolContext};
use super::tool_executor::{
    execute_tool_call, is_parallel_safe_tool, resolve_tool_permission, PermissionOutcome,
    ToolExecutionBackend,
};
use super::{
    enforce_tool_result_budget, process_tool_result, ToolCallRequest, ToolInvocation,
    MAX_TOOL_RESULT_CHARS,
};

/// Execute the tool batch produced by one provider round. Splits into
/// the parallel path (multiple parallel-safe tools spawn into a
/// `std::thread::scope` with explicit OtelContext propagation) or
/// the serial fallback. Returns the per-tool `ToolInvocation` results
/// in submission order, ready to feed back as `FunctionCallOutput`
/// items in the next turn.
///
/// `parent_span_ctx` is the per-turn OtelContext; each tool span uses
/// it as its parent so the trace tree shows
/// `turn → tool.<id>` regardless of which path runs.
pub(super) fn execute_tool_batch(
    inputs: &mut LoopInputs<'_>,
    session: &mut dyn TurnSession,
    cwd: &std::path::Path,
    tool_calls: &[ToolCallRequest],
    parent_span_ctx: Option<&puffer_observability::OtelContext>,
) -> Result<Vec<ToolInvocation>> {
    let parallel_count = tool_calls
        .iter()
        .filter(|tc| is_parallel_safe_tool(&tc.tool_id))
        .count();

    if tool_calls.len() <= 1 || parallel_count <= 1 {
        return execute_tool_batch_serial(inputs, session, cwd, tool_calls, parent_span_ctx);
    }

    let mut permissions: Vec<PermissionOutcome> = Vec::with_capacity(tool_calls.len());
    for tc in tool_calls {
        let args: Value = serde_json::from_str(&tc.input).unwrap_or(Value::Null);
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

    let session_id = inputs.state.session.id;
    let provider_context =
        backend_to_provider_context(session.tool_execution_backend(), inputs.model_id);
    // Cloned once before `thread::scope` because the worker closures cannot
    // touch `inputs.state` (no `&mut AppState` across spawn boundaries).
    // `Arc<dyn ToolRunner>` is `Send + Sync` and clones cheaply.
    let runner = inputs.state.tool_runner.clone();

    let mut results: Vec<Option<(String, bool, bool)>> = vec![None; tool_calls.len()];

    let observability_handle = inputs.observability.clone();
    let parent_ctx_owned = parent_span_ctx.cloned();
    std::thread::scope(|s| {
        let mut handles: Vec<(
            usize,
            std::thread::ScopedJoinHandle<'_, (String, bool, bool)>,
        )> = Vec::new();
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
            let filesystem_policy = match &permissions[i] {
                PermissionOutcome::Allowed(policy) => policy.clone(),
                PermissionOutcome::Denied(_) => unreachable!(),
            };
            let definition = match inputs.registry.definition(&tc.tool_id) {
                Some(d) => d.clone(),
                None => {
                    results[i] = Some((format!("unknown tool {}", tc.tool_id), false, false));
                    continue;
                }
            };
            let args: Value = serde_json::from_str(&tc.input).unwrap_or(Value::Null);
            let resources = inputs.resources;
            let registry = inputs.registry;
            let provider_context_ref = &provider_context;
            let runner_clone = runner.clone();
            // Observability-only clones are gated on a live handle so
            // the disabled path does no per-tool string allocations
            // (review v4 BLOCK #1).
            let span_meta = observability_handle.as_ref().map(|h| {
                (
                    h.clone(),
                    parent_ctx_owned.clone(),
                    tc.tool_id.clone(),
                    tc.call_id.clone(),
                    tc.input.clone(),
                )
            });
            handles.push((
                i,
                s.spawn(move || {
                    let mut tool_span = match span_meta.as_ref() {
                        Some((handle, parent_ctx, tool_id, call_id, input)) => {
                            let mut span = puffer_observability::start_tool_span(
                                Some(handle),
                                parent_ctx.as_ref(),
                                tool_id,
                                call_id,
                                true,
                            );
                            span.set_content(
                                puffer_observability::LANGFUSE_OBSERVATION_INPUT,
                                puffer_observability::ContentKind::ToolInput {
                                    tool_id: tool_id.clone(),
                                },
                                input,
                            );
                            span
                        }
                        None => puffer_observability::SpanGuard::Disabled,
                    };
                    let result = match claude_tools::execute_parallel_tool(
                        &definition,
                        cwd,
                        &filesystem_policy.workspace_roots,
                        &filesystem_policy,
                        &session_id,
                        args,
                        resources,
                        registry,
                        provider_context_ref,
                        &runner_clone,
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
                    if let Some((_, _, tool_id, _, _)) = span_meta.as_ref() {
                        tool_span.set_content(
                            puffer_observability::LANGFUSE_OBSERVATION_OUTPUT,
                            puffer_observability::ContentKind::ToolOutput {
                                tool_id: tool_id.clone(),
                            },
                            &result.0,
                        );
                        tool_span.set_str("puffer.tool.success", result.1.to_string());
                        if !result.1 {
                            tool_span.mark_error("tool_failed".to_string());
                        }
                        tool_span.end();
                    }
                    result
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
        // Span path is gated on a live handle so the disabled path
        // does no per-call clones (review v4 BLOCK #1).
        let mut tool_span = if let Some(handle) = inputs.observability.as_ref() {
            let mut span = puffer_observability::start_tool_span(
                Some(handle),
                parent_span_ctx,
                &tc.tool_id,
                &tc.call_id,
                false,
            );
            span.set_content(
                puffer_observability::LANGFUSE_OBSERVATION_INPUT,
                puffer_observability::ContentKind::ToolInput {
                    tool_id: tc.tool_id.clone(),
                },
                &tc.input,
            );
            span
        } else {
            puffer_observability::SpanGuard::Disabled
        };
        let backend = session.tool_execution_backend();
        let args: Value = serde_json::from_str(&tc.input).unwrap_or(Value::Null);
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
        if inputs.observability.is_some() {
            tool_span.set_content(
                puffer_observability::LANGFUSE_OBSERVATION_OUTPUT,
                puffer_observability::ContentKind::ToolOutput {
                    tool_id: tc.tool_id.clone(),
                },
                &exec.0,
            );
            tool_span.set_str("puffer.tool.success", exec.1.to_string());
            if !exec.1 {
                tool_span.mark_error("tool_failed".to_string());
            }
            tool_span.end();
        }
        results[i] = Some(exec);
    }

    let mut invocations = Vec::with_capacity(tool_calls.len());
    for (i, tc) in tool_calls.iter().enumerate() {
        let (raw_output, success, terminate) = results[i]
            .take()
            .unwrap_or_else(|| ("Tool was not executed".to_string(), false, false));
        let output_text =
            process_tool_result(&raw_output, MAX_TOOL_RESULT_CHARS, &inputs.state.session.id);
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

fn execute_tool_batch_serial(
    inputs: &mut LoopInputs<'_>,
    session: &mut dyn TurnSession,
    cwd: &std::path::Path,
    tool_calls: &[ToolCallRequest],
    parent_span_ctx: Option<&puffer_observability::OtelContext>,
) -> Result<Vec<ToolInvocation>> {
    let mut invocations = Vec::with_capacity(tool_calls.len());

    for call in tool_calls {
        let mut tool_span = if let Some(handle) = inputs.observability.as_ref() {
            let mut span = puffer_observability::start_tool_span(
                Some(handle),
                parent_span_ctx,
                &call.tool_id,
                &call.call_id,
                false,
            );
            span.set_content(
                puffer_observability::LANGFUSE_OBSERVATION_INPUT,
                puffer_observability::ContentKind::ToolInput {
                    tool_id: call.tool_id.clone(),
                },
                &call.input,
            );
            span
        } else {
            puffer_observability::SpanGuard::Disabled
        };
        let backend = session.tool_execution_backend();
        let input_value: Value = serde_json::from_str(&call.input).unwrap_or(Value::Null);
        let execution = match execute_tool_call(
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
        ) {
            Ok(exec) => exec,
            Err(error) => {
                let output_text = process_tool_result(
                    &format!("Tool execution failed: {error}"),
                    MAX_TOOL_RESULT_CHARS,
                    &inputs.state.session.id,
                );
                if inputs.observability.is_some() {
                    tool_span.set_content(
                        puffer_observability::LANGFUSE_OBSERVATION_OUTPUT,
                        puffer_observability::ContentKind::ToolOutput {
                            tool_id: call.tool_id.clone(),
                        },
                        &output_text,
                    );
                    tool_span.set_str("puffer.tool.success", "false".to_string());
                    tool_span.mark_error("tool_failed".to_string());
                    tool_span.end();
                }
                invocations.push(ToolInvocation {
                    call_id: call.call_id.clone(),
                    tool_id: call.tool_id.clone(),
                    input: call.input.clone(),
                    output: output_text,
                    success: false,
                    terminate: false,
                });
                continue;
            }
        };
        let terminate = extract_terminate(&execution.output.metadata);
        let raw_output = if execution.output.stderr.is_empty() {
            execution.output.stdout
        } else if execution.output.stdout.is_empty() {
            execution.output.stderr
        } else {
            format!("{}\n{}", execution.output.stdout, execution.output.stderr)
        };
        let output_text =
            process_tool_result(&raw_output, MAX_TOOL_RESULT_CHARS, &inputs.state.session.id);
        if inputs.observability.is_some() {
            tool_span.set_content(
                puffer_observability::LANGFUSE_OBSERVATION_OUTPUT,
                puffer_observability::ContentKind::ToolOutput {
                    tool_id: call.tool_id.clone(),
                },
                &output_text,
            );
            tool_span.set_str("puffer.tool.success", execution.success.to_string());
            if !execution.success {
                tool_span.mark_error("tool_failed".to_string());
            }
            tool_span.end();
        }
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

fn extract_terminate(metadata: &Value) -> bool {
    metadata
        .get("terminate")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn enforce_tool_result_budget_in_place(invocations: &mut [ToolInvocation], session_id: &Uuid) {
    let mut output_strings: Vec<String> = invocations.iter().map(|i| i.output.clone()).collect();
    enforce_tool_result_budget(&mut output_strings, session_id);
    for (i, new_output) in output_strings.into_iter().enumerate() {
        if new_output != invocations[i].output {
            invocations[i].output = new_output;
        }
    }
}

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

#[allow(dead_code)]
fn _unused_def_marker(_d: &ToolDefinition) {}

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
