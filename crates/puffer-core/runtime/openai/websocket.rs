use super::super::openai_sse::OpenAISseResult;
use super::super::openai_ws::{OpenAIWebSocket, WsApiError};
use super::super::structured_output_support::{
    openai_responses_text_config, openai_tool_definitions_for_request,
};
use super::super::system_prompt::render_runtime_system_prompt;
use super::super::{run_turn_hooks, TurnStreamEvent};
use super::conversation::{
    append_reasoning_items, append_tool_results, compact_conversation, inject_post_compact_context,
    items_to_responses_input, transcript_to_items, ConversationItem,
};
use super::support::{
    apply_previous_response_id, is_openai_structured_output_error, openai_model_supports_reasoning,
    openai_responses_path, openai_supports_response_threading, prefer_native_structured_output,
    structured_output_endpoint_id, OPENAI_STRUCTURED_OUTPUT_FAMILY,
};
use super::{
    build_context_reminder_message, execute_openai_tool_calls, openai_request_instructions,
    parse_openai_text, parse_openai_text_fallback, resolve_openai_execution_config,
    OpenAIExecutionConfig,
};
use crate::permissions::load_runtime_permission_context;
use crate::AppState;
use anyhow::Result;
use puffer_provider_openai::{
    codex_user_agent, refresh_oauth_token, OpenAIAuth, OpenAIRequestConfig,
};
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use serde_json::Value;

/// Returns `true` when the WebSocket transport is enabled via environment.
pub(in super::super) fn openai_websocket_enabled() -> bool {
    std::env::var("PUFFER_OPENAI_WEBSOCKET")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Executes a streaming turn over a persistent WebSocket connection to the
/// OpenAI Responses API. Falls back to the SSE streaming path on connection
/// failure so the user never sees a hard error from transport issues alone.
pub(in super::super) fn execute_openai_websocket_streaming<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::super::TurnRequestOptions<'_>,
    on_event: &mut F,
) -> Result<super::super::TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    let structured_output = options.structured_output;
    let use_native = prefer_native_structured_output(state, provider, &model_id, structured_output);
    match execute_openai_websocket_streaming_once(
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
            execute_openai_websocket_streaming_once(
                state, resources, providers, provider, model_id, auth_store, input, options, false,
                on_event,
            )
        }
        Err(error) => Err(error),
    }
}

/// Constructs the WebSocket URL from the provider's HTTP base URL by swapping
/// the scheme from `https://` to `wss://` (or `http://` to `ws://` for local
/// development), appending the responses path, and including any configured
/// query parameters.
fn build_ws_url(base_url: &str, query_params: &[(String, String)]) -> String {
    let responses_path = openai_responses_path(base_url);
    let ws_base = base_url
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    let trimmed = ws_base.trim_end_matches('/');
    let raw_url = format!("{trimmed}{responses_path}");
    if query_params.is_empty() {
        return raw_url;
    }
    // Append query parameters using the `url` crate for proper encoding.
    match url::Url::parse(&raw_url) {
        Ok(mut parsed) => {
            {
                let mut pairs = parsed.query_pairs_mut();
                for (key, value) in query_params {
                    pairs.append_pair(key, value);
                }
            }
            parsed.to_string()
        }
        Err(_) => raw_url, // Degrade gracefully — use URL without params.
    }
}

/// Collects the HTTP headers needed for the WebSocket upgrade request from
/// the current execution config. Mirrors the headers sent by the SSE path's
/// `build_request_to_path` (minus `Content-Type` and `Accept` which are
/// HTTP-POST-specific).
fn ws_headers_from_config(config: &OpenAIRequestConfig) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    // User-Agent and originator — matches the SSE path.
    headers.push((
        "User-Agent".to_string(),
        codex_user_agent(&config.version, &config.originator),
    ));
    headers.push(("originator".to_string(), config.originator.clone()));
    // Session and account identification — needed for server-side tracking
    // and ChatGPT backend-api / Codex OAuth flows.
    if let Some(session_id) = config.session_id.as_deref() {
        headers.push(("session_id".to_string(), session_id.to_string()));
        headers.push(("x-client-request-id".to_string(), session_id.to_string()));
    }
    if let Some(account_id) = config.account_id.as_deref() {
        headers.push(("ChatGPT-Account-ID".to_string(), account_id.to_string()));
    }
    // Provider-configured custom headers (includes version, OpenAI-Organization,
    // OpenAI-Project, and any user-configured headers).
    for (key, value) in &config.custom_headers {
        headers.push((key.clone(), value.clone()));
    }
    // Authorization — must come after custom_headers to match SSE ordering.
    match &config.auth {
        OpenAIAuth::ApiKey(key) => {
            headers.push(("Authorization".to_string(), format!("Bearer {key}")));
        }
        OpenAIAuth::OAuthBearer(token) => {
            headers.push(("Authorization".to_string(), format!("Bearer {token}")));
        }
        OpenAIAuth::None => {}
    }
    headers
}

/// Attempts to establish a WebSocket connection. On failure returns `None`
/// so the caller can fall back to the SSE path.
fn try_connect_ws(execution: &OpenAIExecutionConfig) -> Option<OpenAIWebSocket> {
    let url = build_ws_url(
        &execution.request_config.base_url,
        &execution.request_config.query_params,
    );
    let headers = ws_headers_from_config(&execution.request_config);
    match OpenAIWebSocket::connect(&url, &headers) {
        Ok(ws) => Some(ws),
        Err(error) => {
            eprintln!("[ws] failed to connect to {url}: {error:#}");
            None
        }
    }
}

/// Inner implementation of the WebSocket streaming agent loop.
#[allow(clippy::too_many_arguments)]
fn execute_openai_websocket_streaming_once<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::super::TurnRequestOptions<'_>,
    use_native: bool,
    on_event: &mut F,
) -> Result<super::super::TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    let structured_output = options.structured_output;
    let mut execution = resolve_openai_execution_config(state, auth_store, provider)?;
    let registry = super::super::mcp_discovery::registry_with_mcp_tools(
        resources,
        state.tool_runner.as_ref(),
    );
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
    let mut items = transcript_to_items(state, input);

    let context_reminder = build_context_reminder_message();
    super::conversation::insert_context_reminder_preserving_legacy_leading_system(
        &mut items,
        &context_reminder,
    );

    let mut invocations = Vec::new();
    let supports_reasoning = openai_model_supports_reasoning(provider, &model_id);
    let model = provider.models.iter().find(|m| m.id == model_id);
    let supports_response_threading =
        openai_supports_response_threading(provider, &execution.request_config.base_url, model);
    let mut previous_response_id: Option<String> = None;
    let mut continuation_start: Option<usize> = None;

    // Attempt to establish the WebSocket connection. If this fails, fall back
    // to the regular SSE streaming path immediately.
    let mut ws = match try_connect_ws(&execution) {
        Some(ws) => ws,
        None => {
            return super::execute_openai_streaming(
                state, resources, providers, provider, model_id, auth_store, input, options,
                on_event,
            );
        }
    };

    // Clone the reflection config because `options` is still needed — the
    // mid-loop fallback branches below consume `options` by value when
    // handing control back to the SSE path, and the SSE path constructs its
    // own tracker from `options.reflection`. If we moved `options.reflection`
    // into the websocket tracker instead, those fallbacks would lose the
    // reflection policy.
    let mut reflection = options
        .reflection
        .clone()
        .map(|config| super::super::reflection::ReflectionTracker::new(input, config));
    let mut reflection_traces: Vec<super::super::ReflectionTraceEvent> = Vec::new();

    loop {
        // Check for background tasks that completed since the last turn.
        let completed = super::super::claude_tools::workflow::drain_completed_shell_tasks(
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

        // Wire boundary: ConversationItem -> Responses API input.
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

        // Build the request body (same as the SSE path).
        let mut body = super::build_codex_openai_request_body(
            state,
            &model_id,
            &instructions,
            wire_input,
            &tools,
            supports_reasoning,
            text.clone(),
            true, // stream flag — will be stripped by send_response_create
        );
        apply_previous_response_id(&mut body, prev_resp_id.as_deref());

        // Reconnect if the connection is approaching its 60-minute limit.
        if ws.is_expired() {
            ws.close();
            ws = match try_connect_ws(&execution) {
                Some(new_ws) => new_ws,
                None => {
                    if invocations.is_empty() {
                        return fallback_to_sse(
                            state, resources, providers, provider, &model_id, auth_store, input,
                            options, on_event,
                        );
                    }
                    anyhow::bail!(
                        "WebSocket reconnect failed after tool calls were already executed"
                    );
                }
            };
        }

        // Send the request and read events over WebSocket.
        let response = match send_and_read_ws_with_retry(&mut ws, &execution, &body, on_event) {
            Ok(result) => result,
            Err(error) => {
                // Extract the structured error code if this is a WS API error.
                let ws_code = error.downcast_ref::<WsApiError>().map(|e| e.code.as_str());

                // Check for previous_response_not_found — reset chain and retry.
                if ws_code == Some("previous_response_not_found") {
                    previous_response_id = None;
                    continuation_start = None;
                    // Reconnect and retry with full input.
                    ws.close();
                    ws = match try_connect_ws(&execution) {
                        Some(new_ws) => new_ws,
                        None => {
                            if invocations.is_empty() {
                                return fallback_to_sse(
                                    state, resources, providers, provider, &model_id, auth_store,
                                    input, options, on_event,
                                );
                            }
                            anyhow::bail!(
                                "WebSocket reconnect failed after tool calls were already executed"
                            );
                        }
                    };
                    continue;
                }
                // For auth errors, try refreshing OAuth and reconnecting.
                let is_auth_error = matches!(
                    ws_code,
                    Some("invalid_api_key" | "token_expired" | "authentication_error")
                );
                if is_auth_error {
                    if let Some(ref refresh_token) = execution.refresh_token {
                        if let Ok(refreshed) = refresh_oauth_token(refresh_token) {
                            let stored = super::support::openai_registry_credential(refreshed);
                            execution.request_config.auth =
                                OpenAIAuth::OAuthBearer(stored.access_token.clone());
                            execution.request_config.account_id = stored.account_id.clone();
                            execution.refresh_token = Some(stored.refresh_token.clone());
                            auth_store.set_oauth(execution.provider_id.clone(), stored);
                            ws.close();
                            ws = match try_connect_ws(&execution) {
                                Some(new_ws) => new_ws,
                                None => {
                                    if invocations.is_empty() {
                                        return fallback_to_sse(
                                            state, resources, providers, provider, &model_id,
                                            auth_store, input, options, on_event,
                                        );
                                    }
                                    anyhow::bail!(
                                        "WebSocket reconnect failed after tool calls were already executed"
                                    );
                                }
                            };
                            continue;
                        }
                    }
                }
                // Generic WS error.
                if invocations.is_empty() {
                    return fallback_to_sse(
                        state, resources, providers, provider, &model_id, auth_store, input,
                        options, on_event,
                    );
                }
                return Err(error.context("WebSocket error after tool calls were already executed"));
            }
        };

        // Extract results (same logic as the SSE streaming path).
        previous_response_id = if supports_response_threading {
            response.response_id
        } else {
            None
        };
        let input_tokens = response.input_tokens;
        if let Some(input) = input_tokens {
            state.last_input_tokens = Some(input as u32);
            let cached = response.cached_tokens.unwrap_or(0);
            let output = response.output_tokens.unwrap_or(0);
            state.update_cache_stats(input as u64, cached as u64);
            on_event(TurnStreamEvent::Usage(super::super::TurnUsageReport {
                input_tokens: input as u64,
                output_tokens: output as u64,
                cache_read_tokens: cached as u64,
                cache_creation_tokens: 0,
            }));
        }

        if response.tool_calls.is_empty() {
            let assistant_text = if response.assistant_text.trim().is_empty() {
                parse_openai_text(&response.raw_response)
                    .or_else(|_| parse_openai_text_fallback(&response.raw_response, state))?
            } else {
                response.assistant_text
            };
            run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
            ws.close();
            return Ok(super::super::TurnExecution {
                assistant_text,
                tool_invocations: invocations,
                reflection_traces,
            });
        }

        let tool_calls = response.tool_calls;
        let pending_tool_calls = tool_calls
            .iter()
            .filter(|tc| !response.emitted_tool_call_ids.contains(&tc.call_id))
            .map(|tc| super::super::ToolCallRequest {
                call_id: tc.call_id.clone(),
                tool_id: tc.name.clone(),
                input: serde_json::to_string(&tc.arguments).unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        if !pending_tool_calls.is_empty() {
            on_event(TurnStreamEvent::ToolCallsRequested(pending_tool_calls));
        }

        if !response.assistant_text.trim().is_empty() {
            items.push(ConversationItem::assistant_message(
                &response.assistant_text,
            ));
        }
        append_reasoning_items(&mut items, &response.reasoning_items);
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

        append_tool_results(&mut items, &tool_results.invocations);
        if let Some(observation) = reflection.as_mut().and_then(|tracker| {
            tracker.observe_batch_with_judge(
                &tool_results.invocations,
                &items,
                state,
                resources,
                providers,
                auth_store,
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

/// Sends a `response.create` message and reads events until completion.
fn send_and_read_ws<F>(
    ws: &mut OpenAIWebSocket,
    body: &Value,
    on_event: &mut F,
) -> Result<OpenAISseResult>
where
    F: FnMut(TurnStreamEvent),
{
    ws.send_response_create(body)?;
    ws.read_events(on_event)
}

/// Sends a request with retry logic for transient errors (rate limits, server
/// errors, connection resets). Mirrors the SSE path's `retry_openai_transport`
/// but adapted for WebSocket semantics.
fn send_and_read_ws_with_retry<F>(
    ws: &mut OpenAIWebSocket,
    execution: &OpenAIExecutionConfig,
    body: &Value,
    on_event: &mut F,
) -> Result<OpenAISseResult>
where
    F: FnMut(TurnStreamEvent),
{
    let max_attempts = ws_transport_max_attempts();
    let delay = ws_transport_retry_delay();
    for attempt in 1..=max_attempts {
        match send_and_read_ws(ws, body, on_event) {
            Ok(result) => return Ok(result),
            Err(error) if attempt < max_attempts && is_retryable_ws_error(&error) => {
                eprintln!("[ws] retryable error on attempt {attempt}/{max_attempts}: {error:#}",);
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                // The connection may be broken — try to reconnect.
                ws.close();
                let url = build_ws_url(
                    &execution.request_config.base_url,
                    &execution.request_config.query_params,
                );
                let headers = ws_headers_from_config(&execution.request_config);
                match OpenAIWebSocket::connect(&url, &headers) {
                    Ok(new_ws) => *ws = new_ws,
                    Err(_) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("retry loop always returns or errors")
}

/// Returns `true` for WebSocket errors that are worth retrying: rate limits,
/// server errors, and connection-level transients.
fn is_retryable_ws_error(error: &anyhow::Error) -> bool {
    // Check structured WS API error codes first.
    if let Some(ws_err) = error.downcast_ref::<WsApiError>() {
        return matches!(
            ws_err.code.as_str(),
            "rate_limit_exceeded" | "server_error" | "overloaded"
        );
    }
    // Connection-level transient errors (tungstenite / IO).
    let text = error.to_string().to_ascii_lowercase();
    if text.contains("connection reset")
        || text.contains("broken pipe")
        || text.contains("operation timed out")
        || text.contains("unexpected eof")
    {
        return true;
    }
    // IO timeout errors in the error chain.
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::TimedOut)
    })
}

/// Maximum retry attempts for WS transport errors.
fn ws_transport_max_attempts() -> usize {
    std::env::var("PUFFER_OPENAI_WS_MAX_ATTEMPTS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(3)
        .clamp(1, 5)
}

/// Delay between WS retry attempts.
fn ws_transport_retry_delay() -> std::time::Duration {
    let ms = std::env::var("PUFFER_OPENAI_WS_RETRY_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1_000)
        .min(10_000);
    std::time::Duration::from_millis(ms)
}

/// Falls back to a single SSE HTTP request for the current turn body.
/// This is used when the WebSocket connection cannot be established or
/// encounters an unrecoverable error mid-session. The original user
/// `input` is threaded through to avoid re-deriving it from the transcript,
/// which may have been mutated during the WS agent loop.
#[allow(clippy::too_many_arguments)]
fn fallback_to_sse<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: &str,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::super::TurnRequestOptions<'_>,
    on_event: &mut F,
) -> Result<super::super::TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    super::execute_openai_streaming(
        state,
        resources,
        providers,
        provider,
        model_id.to_string(),
        auth_store,
        input,
        options,
        on_event,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ws_url_standard_openai() {
        let url = build_ws_url("https://api.openai.com", &[]);
        assert_eq!(url, "wss://api.openai.com/v1/responses");
    }

    #[test]
    fn build_ws_url_trailing_slash() {
        let url = build_ws_url("https://api.openai.com/", &[]);
        assert_eq!(url, "wss://api.openai.com/v1/responses");
    }

    #[test]
    fn build_ws_url_backend_api() {
        let url = build_ws_url("https://chatgpt.com/backend-api/codex", &[]);
        assert_eq!(url, "wss://chatgpt.com/backend-api/codex/responses");
    }

    #[test]
    fn build_ws_url_local_http() {
        let url = build_ws_url("http://localhost:8080", &[]);
        assert_eq!(url, "ws://localhost:8080/v1/responses");
    }

    #[test]
    fn build_ws_url_only_replaces_scheme_prefix() {
        // Ensure replacen(..., 1) only swaps the leading scheme, not any
        // embedded "https://" in the path.
        let url = build_ws_url(
            "https://proxy.example.com/forward-to-https://api.openai.com",
            &[],
        );
        assert!(url.starts_with("wss://proxy.example.com/"));
        // The embedded https:// in the path must NOT be replaced.
        assert!(url.contains("https://api.openai.com"));
    }

    #[test]
    fn build_ws_url_infer_api() {
        // The infer API base URL already includes /v1, so openai_responses_path
        // appends /v1/responses, resulting in /v1/v1/responses. This matches
        // the HTTP path construction used by the SSE path.
        let url = build_ws_url("https://api-infer.agentsey.ai/v1", &[]);
        assert_eq!(url, "wss://api-infer.agentsey.ai/v1/v1/responses");
    }

    #[test]
    fn build_ws_url_appends_query_params() {
        let params = vec![
            ("api-version".to_string(), "2024-06-01".to_string()),
            ("deployment".to_string(), "gpt4".to_string()),
        ];
        let url = build_ws_url("https://api.openai.com", &params);
        assert!(url.starts_with("wss://api.openai.com/v1/responses?"));
        assert!(url.contains("api-version=2024-06-01"));
        assert!(url.contains("deployment=gpt4"));
    }

    #[test]
    fn build_ws_url_empty_query_params_no_question_mark() {
        let url = build_ws_url("https://api.openai.com", &[]);
        assert!(!url.contains('?'));
    }

    /// Helper to find a header value by name in a headers list.
    fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn ws_headers_api_key() {
        let config = OpenAIRequestConfig {
            base_url: "https://api.openai.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test-key".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: Some("sess-123".to_string()),
            account_id: None,
            custom_headers: vec![("X-Custom".to_string(), "value".to_string())],
            query_params: vec![],
        };
        let headers = ws_headers_from_config(&config);
        assert_eq!(
            find_header(&headers, "Authorization"),
            Some("Bearer sk-test-key")
        );
        assert_eq!(find_header(&headers, "X-Custom"), Some("value"));
        // New: verify parity headers are present.
        assert!(find_header(&headers, "User-Agent").is_some());
        assert_eq!(find_header(&headers, "originator"), Some("codex_cli_rs"));
        assert_eq!(find_header(&headers, "session_id"), Some("sess-123"));
        assert_eq!(
            find_header(&headers, "x-client-request-id"),
            Some("sess-123")
        );
    }

    #[test]
    fn ws_headers_oauth_bearer() {
        let config = OpenAIRequestConfig {
            base_url: "https://api.openai.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::OAuthBearer("oauth-token-123".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: Some("acct-456".to_string()),
            custom_headers: vec![],
            query_params: vec![],
        };
        let headers = ws_headers_from_config(&config);
        assert_eq!(
            find_header(&headers, "Authorization"),
            Some("Bearer oauth-token-123")
        );
        assert_eq!(
            find_header(&headers, "ChatGPT-Account-ID"),
            Some("acct-456")
        );
        assert!(find_header(&headers, "User-Agent").is_some());
        assert_eq!(find_header(&headers, "originator"), Some("codex_cli_rs"));
        // No session_id configured — headers should be absent.
        assert!(find_header(&headers, "session_id").is_none());
    }

    #[test]
    fn ws_headers_no_auth() {
        let config = OpenAIRequestConfig {
            base_url: "https://api.openai.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::None,
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: vec![],
            query_params: vec![],
        };
        let headers = ws_headers_from_config(&config);
        assert!(find_header(&headers, "Authorization").is_none());
        // Even without auth, User-Agent and originator must be present.
        assert!(find_header(&headers, "User-Agent").is_some());
        assert_eq!(find_header(&headers, "originator"), Some("codex_cli_rs"));
    }

    #[test]
    fn openai_websocket_enabled_parses_true_values() {
        // NOTE: These tests mutate env vars and may interfere with parallel
        // tests. In practice the test runner serialises within a single file.
        std::env::set_var("PUFFER_OPENAI_WEBSOCKET", "1");
        assert!(openai_websocket_enabled());

        std::env::set_var("PUFFER_OPENAI_WEBSOCKET", "true");
        assert!(openai_websocket_enabled());

        std::env::set_var("PUFFER_OPENAI_WEBSOCKET", "TRUE");
        assert!(openai_websocket_enabled());

        std::env::set_var("PUFFER_OPENAI_WEBSOCKET", "0");
        assert!(!openai_websocket_enabled());

        std::env::set_var("PUFFER_OPENAI_WEBSOCKET", "false");
        assert!(!openai_websocket_enabled());

        std::env::set_var("PUFFER_OPENAI_WEBSOCKET", "yes");
        assert!(!openai_websocket_enabled());

        std::env::remove_var("PUFFER_OPENAI_WEBSOCKET");
        assert!(!openai_websocket_enabled());
    }

    #[test]
    fn is_retryable_ws_error_matches_api_codes() {
        let err = WsApiError {
            code: "rate_limit_exceeded".to_string(),
            message: "too many requests".to_string(),
        };
        assert!(is_retryable_ws_error(&anyhow::Error::new(err)));

        let err = WsApiError {
            code: "server_error".to_string(),
            message: "internal".to_string(),
        };
        assert!(is_retryable_ws_error(&anyhow::Error::new(err)));

        let err = WsApiError {
            code: "overloaded".to_string(),
            message: "try later".to_string(),
        };
        assert!(is_retryable_ws_error(&anyhow::Error::new(err)));
    }

    #[test]
    fn is_retryable_ws_error_rejects_auth_errors() {
        let err = WsApiError {
            code: "invalid_api_key".to_string(),
            message: "bad key".to_string(),
        };
        assert!(!is_retryable_ws_error(&anyhow::Error::new(err)));

        let err = WsApiError {
            code: "previous_response_not_found".to_string(),
            message: "gone".to_string(),
        };
        assert!(!is_retryable_ws_error(&anyhow::Error::new(err)));
    }

    #[test]
    fn is_retryable_ws_error_matches_connection_errors() {
        let err = anyhow::anyhow!("connection reset by peer");
        assert!(is_retryable_ws_error(&err));

        let err = anyhow::anyhow!("broken pipe");
        assert!(is_retryable_ws_error(&err));

        let err = anyhow::anyhow!("operation timed out");
        assert!(is_retryable_ws_error(&err));
    }

    #[test]
    fn ws_api_error_rejects_all_auth_codes() {
        // T3: Verify every auth error code used in the dispatch logic is
        // NOT retryable (they need special handling, not blind retry).
        for code in &["invalid_api_key", "token_expired", "authentication_error"] {
            let err = WsApiError {
                code: code.to_string(),
                message: "auth problem".to_string(),
            };
            assert!(
                !is_retryable_ws_error(&anyhow::Error::new(err)),
                "{code} should NOT be retryable"
            );
        }
    }

    #[test]
    fn is_retryable_ws_error_detects_io_timeout_in_chain() {
        // T5: An IO TimedOut error wrapped in an anyhow chain should be
        // detected via the error chain walk, not just string matching.
        let io_err = std::io::Error::new(std::io::ErrorKind::TimedOut, "read timed out");
        let chained: anyhow::Error = anyhow::Error::new(io_err).context("WebSocket read failed");
        assert!(is_retryable_ws_error(&chained));
    }

    #[test]
    fn is_retryable_ws_error_rejects_non_retryable_io() {
        // An IO PermissionDenied error should NOT be retryable.
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "not allowed");
        let chained: anyhow::Error = anyhow::Error::new(io_err).context("WebSocket connect failed");
        assert!(!is_retryable_ws_error(&chained));
    }

    #[test]
    fn ws_transport_max_attempts_defaults_and_clamps() {
        // T6: Verify default and env-var parsing for max attempts.
        // Clear any existing env var first.
        std::env::remove_var("PUFFER_OPENAI_WS_MAX_ATTEMPTS");
        assert_eq!(ws_transport_max_attempts(), 3);

        std::env::set_var("PUFFER_OPENAI_WS_MAX_ATTEMPTS", "1");
        assert_eq!(ws_transport_max_attempts(), 1);

        // Clamps to max 5.
        std::env::set_var("PUFFER_OPENAI_WS_MAX_ATTEMPTS", "99");
        assert_eq!(ws_transport_max_attempts(), 5);

        // Clamps to min 1.
        std::env::set_var("PUFFER_OPENAI_WS_MAX_ATTEMPTS", "0");
        assert_eq!(ws_transport_max_attempts(), 1);

        // Invalid value falls back to default.
        std::env::set_var("PUFFER_OPENAI_WS_MAX_ATTEMPTS", "not_a_number");
        assert_eq!(ws_transport_max_attempts(), 3);

        std::env::remove_var("PUFFER_OPENAI_WS_MAX_ATTEMPTS");
    }

    #[test]
    fn ws_transport_retry_delay_defaults_and_caps() {
        // T6: Verify default and env-var parsing for retry delay.
        std::env::remove_var("PUFFER_OPENAI_WS_RETRY_DELAY_MS");
        assert_eq!(
            ws_transport_retry_delay(),
            std::time::Duration::from_millis(1_000)
        );

        std::env::set_var("PUFFER_OPENAI_WS_RETRY_DELAY_MS", "500");
        assert_eq!(
            ws_transport_retry_delay(),
            std::time::Duration::from_millis(500)
        );

        // Caps at 10_000ms.
        std::env::set_var("PUFFER_OPENAI_WS_RETRY_DELAY_MS", "99999");
        assert_eq!(
            ws_transport_retry_delay(),
            std::time::Duration::from_millis(10_000)
        );

        // Invalid value falls back to default.
        std::env::set_var("PUFFER_OPENAI_WS_RETRY_DELAY_MS", "bad");
        assert_eq!(
            ws_transport_retry_delay(),
            std::time::Duration::from_millis(1_000)
        );

        std::env::remove_var("PUFFER_OPENAI_WS_RETRY_DELAY_MS");
    }
}
