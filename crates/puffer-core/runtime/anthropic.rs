//! Anthropic Messages API adapter.
//!
//! Vendor-specific concerns (auth, request URL, headers, fingerprinting,
//! tool serialization, SSE parsing, response Value → ConversationItem
//! synthesis) live in this module. The turn-by-turn driver (tool
//! execution, reflection, compaction, hooks) lives in
//! [`crate::runtime::agent_loop`] and is shared with other providers.
//!
//! The seam is the [`AnthropicTurnSession`] struct, which captures all
//! per-prompt setup once and then implements the neutral
//! [`TurnSession`](crate::runtime::agent_loop::TurnSession) trait so
//! `agent_loop::run_streaming_loop` and `run_blocking_loop` can drive it
//! without seeing any vendor types.

use super::agent_loop::{self, AssistantTurn, LoopInputs, TurnSession};
use super::anthropic_sse::parse_anthropic_sse;
use super::openai::conversation::{
    build_system_reminder, items_to_anthropic_messages, ConversationItem, ReasoningSummary,
    ToolOutputPayload,
};
use super::request_tool_filter::RequestToolFilter;
use super::structured_output_support::{
    anthropic_tool_definitions_for_request, StructuredOutputConfig,
};
use super::system_prompt::render_runtime_system_prompt;
use super::tool_executor::{execute_tool_call, ToolExecutionBackend};
use super::{
    enforce_tool_result_budget, git_status_context, process_tool_result, resolve_max_output_tokens,
    send_http_request, send_http_request_with_proxy, state_has_image_attachments, ToolCallRequest,
    ToolInvocation, TurnExecution, TurnRequestOptions, TurnStreamEvent, APP_VERSION,
    MAX_TOOL_RESULT_CHARS,
};
use crate::permissions::{load_runtime_permission_context_with_inputs, RuntimePermissionInputs};
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_provider_registry::{
    AuthStore, OAuthCredential, ProviderDescriptor, ProviderRegistry, StoredCredential,
};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use puffer_transport_anthropic::{
    build_messages_request, get_session_ingress_auth, AnthropicAuth, AnthropicMessage,
    AnthropicModelRequest, AnthropicRequestConfig,
};
use reqwest::blocking::Client;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Session — per-prompt setup that the turn loop reuses across iterations.
// ---------------------------------------------------------------------------

/// All static-per-prompt state the Anthropic adapter needs to perform a
/// single LLM round-trip. Computed once in
/// [`setup_anthropic_session`], then used by `TurnSession` impls.
pub(super) struct AnthropicTurnSession {
    request_url: String,
    request_headers: Vec<(String, String)>,
    attribution_prefix_block: String,
    request_config: AnthropicRequestConfig,
    system_prompt: String,
    plan_mode_context: Option<String>,
    system_reminder: String,
    tools: Vec<Value>,
    structured_output: Option<StructuredOutputConfig>,
    model_id: String,
    model_supports_thinking: bool,
    provider_supports_thinking_api: bool,
    max_output: u32,
}

fn setup_anthropic_session(
    state: &mut AppState,
    resources: &LoadedResources,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &AuthStore,
    options: &TurnRequestOptions<'_>,
    input: &str,
) -> Result<AnthropicTurnSession> {
    let auth = anthropic_auth_for_provider(auth_store, provider)?;
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
    let plan_mode_context = crate::plan_mode::take_plan_mode_context_message(state, resources)?;

    let request_config = AnthropicRequestConfig {
        base_url: provider.base_url.clone(),
        session_id: state.session.id.to_string(),
        custom_headers: provider.headers.clone(),
        remote_container_id: None,
        remote_session_id: None,
        client_app: None,
        entrypoint: "cli".to_string(),
        user_type: "external".to_string(),
        version: APP_VERSION.to_string(),
        workload: None,
        additional_protection: false,
        cch_enabled: true,
        auth: auth.clone(),
        beta_header: None,
        client_request_id: None,
    };
    let request = build_messages_request(
        &request_config,
        &AnthropicModelRequest {
            model: model_id.clone(),
            max_tokens: resolve_max_output_tokens(provider, &model_id),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: input.to_string(),
            }],
        },
    )?;

    let mut tools = anthropic_tool_definitions_for_request(
        &registry,
        options.structured_output,
        Some(&permission_context),
    )?;
    if !options.excluded_tools.is_empty() {
        tools.retain(|tool| {
            !tool
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| options.excluded_tools.iter().any(|ex| ex == name))
        });
    }
    if state_has_image_attachments(state) {
        tools.retain(|tool| tool.get("name").and_then(Value::as_str) != Some("VisionAnalyze"));
    }

    let system_prompt = render_runtime_system_prompt(
        state,
        resources,
        &model_id,
        &tools
            .iter()
            .filter_map(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<std::collections::BTreeSet<_>>(),
    )?;

    let git_status = git_status_context();
    let system_reminder = build_system_reminder(state, &git_status);

    let model_supports_thinking = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.supports_reasoning)
        .unwrap_or(false);
    let provider_supports_thinking_api = anthropic_supports_thinking_api(provider, &model_id);
    let max_output = resolve_max_output_tokens(provider, &model_id);

    Ok(AnthropicTurnSession {
        request_url: request.url,
        request_headers: request.headers,
        attribution_prefix_block: request.attribution_prefix_block,
        request_config,
        system_prompt,
        plan_mode_context,
        system_reminder,
        tools,
        structured_output: options.structured_output.cloned(),
        model_id,
        model_supports_thinking,
        provider_supports_thinking_api,
        max_output,
    })
}

impl AnthropicTurnSession {
    /// Builds the Anthropic Messages API JSON body. `stream` toggles the
    /// `stream: true` flag plus the streaming-specific field set (the
    /// blocking path additionally emits `context_management`, `speed`,
    /// and `metadata` fields).
    fn build_body(&self, items: &[ConversationItem], state: &AppState, stream: bool) -> Value {
        let wire_messages = items_to_anthropic_messages(items);
        let mut body = json!({
            "model": self.model_id,
            "max_tokens": self.max_output,
            "messages": wire_messages,
            "system": anthropic_system_blocks(
                &self.attribution_prefix_block,
                Some(&self.system_prompt),
                self.plan_mode_context.as_deref(),
                Some(&self.system_reminder),
            )
        });
        if stream {
            body["stream"] = json!(true);
        }
        if !self.tools.is_empty() {
            body["tools"] = Value::Array(self.tools.clone());
            body["tool_choice"] = json!({"type": "auto"});
        }
        if self.model_supports_thinking
            && self.provider_supports_thinking_api
            && state.effort_level != "low"
        {
            let thinking_budget = match state.effort_level.as_str() {
                "high" | "max" => self.max_output.saturating_sub(1).min(16_384),
                _ => self.max_output.saturating_sub(1).min(8_192),
            };
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": thinking_budget
            });
        } else {
            body["temperature"] = json!(1);
        }
        if !stream {
            // Blocking path injects extra fields. (Streaming path
            // historically omits these — preserved to keep wire bytes
            // identical pre/post refactor.)
            if self.model_supports_thinking && self.provider_supports_thinking_api {
                body["context_management"] = json!({
                    "edits": [{
                        "type": "clear_thinking_20251015",
                        "keep": "all"
                    }]
                });
            }
            if state.fast_mode {
                body["speed"] = json!("fast");
            }
            body["metadata"] = json!({
                "user_id": format!(
                    "{{\"session_id\":\"{}\",\"device_id\":\"puffer-cli\"}}",
                    state.session.id
                )
            });
        }
        body
    }
}

impl TurnSession for AnthropicTurnSession {
    fn one_turn_streaming(
        &mut self,
        state: &mut AppState,
        _auth_store: &mut AuthStore,
        items: &mut Vec<ConversationItem>,
        on_event: &mut dyn FnMut(TurnStreamEvent),
    ) -> Result<AssistantTurn> {
        let body = self.build_body(items, state, true);

        // Mirror the blocking path's wire trace for streaming requests
        // when `PUFFER_HTTP_TRACE_PATH` is set, so multi-turn tool flows
        // are debuggable end-to-end (e.g. for diagnosing relay-specific
        // shape requirements like Kimi's "reasoning_content is missing
        // in assistant tool call message" rejection).
        trace_anthropic_stream_request(&self.request_url, &body);

        // Send streaming request.
        let client = crate::network::blocking_client_for_url(
            &state.config.network.proxy,
            crate::network::HttpPurpose::Model,
            &self.request_url,
            std::time::Duration::from_secs(300),
        )
        .unwrap_or_else(|_| Client::new());
        let mut http_request = client.post(&self.request_url);
        for (key, value) in &self.request_headers {
            http_request = http_request.header(key, value);
        }
        http_request = http_request
            .header("content-type", "application/json")
            .header("accept", "text/event-stream");
        // Wrap send in 5xx retry so transient gateway errors (502 /
        // 503 / 504, common during Anthropic load spikes) don't
        // abort the whole turn. Connection-level errors (timeouts,
        // resets) are handled by reqwest's own retry on the
        // `with_context` path; those would surface as `?` errors
        // before reaching this site.
        let request_body = body.to_string();
        let http_response = super::retry_on_5xx(
            || {
                let mut http_request = client.post(&self.request_url);
                for (key, value) in &self.request_headers {
                    http_request = http_request.header(key, value);
                }
                http_request
                    .header("content-type", "application/json")
                    .header("accept", "text/event-stream")
                    .body(request_body.clone())
                    .send()
                    .with_context(|| {
                        format!("failed to send streaming request to {}", self.request_url)
                    })
            },
            |attempt, max, status| {
                trace_anthropic_stream_error(
                    &self.request_url,
                    status.as_u16(),
                    &format!("[5xx retry] attempt {attempt}/{max}, sleeping before next try"),
                );
            },
        )?;
        // Drop the now-unused initial builder so we don't have two
        // outstanding `http_request`s in scope.
        drop(http_request);

        if !http_response.status().is_success() {
            let status = http_response.status();
            let text = http_response.text().unwrap_or_default();
            // Capture error responses too so 400/429 wire bodies are
            // visible in the trace file. Without this, only successful
            // SSE-parsed responses would be logged and rate-limit /
            // schema-rejection diagnoses would be opaque.
            trace_anthropic_stream_error(&self.request_url, status.as_u16(), &text);
            // Promote 429 / 403-access-terminated to typed `QuotaError`
            // so orchestration can delay-retry instead of burning the
            // budget. See `runtime::quota`.
            if let Some(quota) =
                super::quota::classify_response("anthropic", status.as_u16(), &text)
            {
                return Err(anyhow::Error::new(quota));
            }
            bail!("request failed with status {status}: {text}");
        }

        // Wrap `&mut dyn FnMut` in a sized closure so the generic
        // `parse_anthropic_sse<F: FnMut(...)>` signature accepts it.
        let mut wrapped = |event: TurnStreamEvent| on_event(event);
        let response = parse_anthropic_sse(http_response, &mut wrapped)?;
        // Trace the reconstructed response so we can see what content
        // blocks (text, thinking, tool_use) the upstream actually
        // produced. Critical for diagnosing replay-shape bugs like
        // "reasoning_content missing in assistant tool call message".
        trace_anthropic_stream_response(&self.request_url, &response);
        turn_from_response(&response)
    }

    fn one_turn_blocking(
        &mut self,
        state: &mut AppState,
        _auth_store: &mut AuthStore,
        items: &mut Vec<ConversationItem>,
    ) -> Result<AssistantTurn> {
        // Anthropic-specific 413 / prompt_too_long recovery: drop oldest
        // items in place and retry until the request fits or we hit the
        // floor (3 items).
        loop {
            let body = self.build_body(items, state, false);
            match send_http_request_with_proxy(
                &self.request_url,
                &self.request_headers,
                &body.to_string(),
                true,
                &state.config.network.proxy,
            ) {
                Ok(response) => return turn_from_response(&response),
                Err(error) => {
                    if is_anthropic_too_large_error(&error.to_string()) && items.len() > 3 {
                        let drop_count = (items.len() / 3).max(1);
                        items.drain(..drop_count);
                        if !matches!(items.first(), Some(ConversationItem::Message { .. })) {
                            items.insert(
                                0,
                                ConversationItem::user_message(
                                    "[Context truncated to fit within model limits]",
                                ),
                            );
                        }
                        continue;
                    }
                    return Err(error);
                }
            }
        }
    }

    fn generate_summary(&self, old_context: &str, model_id: &str) -> Option<String> {
        anthropic_generate_summary(
            old_context,
            model_id,
            &self.request_url,
            &self.request_headers,
        )
    }

    fn tool_execution_backend(&self) -> ToolExecutionBackend<'_> {
        ToolExecutionBackend::Anthropic {
            request_config: &self.request_config,
            structured_output: self.structured_output.as_ref(),
        }
    }
}

/// Recognizes the surface forms Anthropic / Anthropic-relays use
/// when a request exceeds the model's context cap, so the blocking
/// loop's drop-oldest-items recovery can trigger. Matches
/// case-insensitively to handle:
///
/// - `413` — HTTP status code in the error string.
/// - `prompt_too_long` — legacy / OpenAI-style typed error tag.
/// - `request_too_large` — Anthropic's own `error.type` (also the
///   bracketed tag emitted by `parse_anthropic_sse` after the
///   error.type-surfacing change).
/// - `payload too large` / `request entity too large` — HTTP status
///   text from various proxies and gateways.
/// - `too long` — legacy substring fallback for free-form messages
///   like "prompt is too long: …".
///
/// Pulled out as its own function so the unit test can pin every
/// form independently and so we don't accumulate the match list
/// inline in the recovery loop.
fn is_anthropic_too_large_error(err_msg: &str) -> bool {
    let lower = err_msg.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "413",
        "prompt_too_long",
        "request_too_large",
        "payload too large",
        "request entity too large",
        "too long",
    ];
    NEEDLES.iter().any(|needle| lower.contains(needle))
}

/// Converts an Anthropic response Value into the neutral
/// [`AssistantTurn`] shape: extracts text + tool_use blocks, leaves
/// FunctionCallOutput synthesis to the loop driver.
fn turn_from_response(response: &Value) -> Result<AssistantTurn> {
    let pre_tool_items = synthesize_pre_tool_items(response);
    let tool_calls = extract_tool_calls(response)?;
    let assistant_text = if tool_calls.is_empty() {
        parse_anthropic_text(response)?
    } else {
        // When tool calls exist, assistant text is partial and the
        // returned TurnExecution.assistant_text comes from the FINAL
        // (no-tool-calls) iteration anyway. Empty here is fine.
        String::new()
    };
    let input_tokens_hint = response
        .pointer("/usage/input_tokens")
        .and_then(Value::as_u64)
        .map(|v| v as usize);
    // Surface per-turn usage on the **blocking** path. Anthropic's
    // non-streaming JSON returns the same `usage` block as the SSE
    // stream's terminal `message_delta`. Without this, the goal
    // accounting hook (`account_token_usage`) only fires for the
    // streaming path — meaning subagent / teammate / reflection-judge
    // calls that go through `run_blocking_loop` (see
    // `runtime/blocking_loop.rs`) never tick the budget either.
    let usage_report = response.get("usage").map(|usage| super::TurnUsageReport {
        input_tokens: usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_read_tokens: usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_creation_tokens: usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    });
    Ok(AssistantTurn {
        pre_tool_items,
        tool_calls,
        assistant_text,
        input_tokens_hint,
        emitted_tool_call_ids: std::collections::HashSet::new(),
        usage_report,
    })
}

/// Pulls assistant thinking + text + tool_use blocks from an Anthropic
/// response Value into neutral `ConversationItem`s. The loop driver
/// later appends `FunctionCallOutput` entries from tool execution.
///
/// Order matters: `thinking` precedes `text` precedes `tool_use` in the
/// resulting items so that on the next turn `items_to_anthropic_messages`
/// re-emits them in the same order Anthropic requires within an assistant
/// message. Capturing the thinking block (with its `signature`) is what
/// keeps providers like `kimi-coding/k2p5` from rejecting the next-turn
/// request with "reasoning_content is missing in assistant tool call
/// message".
fn synthesize_pre_tool_items(response: &Value) -> Vec<ConversationItem> {
    let mut out = Vec::new();
    let Some(content) = response.get("content").and_then(Value::as_array) else {
        return out;
    };

    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("thinking") => {
                // Anthropic requires `signature` to round-trip the thinking
                // block; without it the upstream rejects the replay.
                let Some(signature) = block.get("signature").and_then(Value::as_str) else {
                    continue;
                };
                let thinking_text = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let summary = if thinking_text.is_empty() {
                    Vec::new()
                } else {
                    vec![ReasoningSummary::SummaryText {
                        text: thinking_text,
                    }]
                };
                out.push(ConversationItem::Reasoning {
                    summary,
                    encrypted_content: Some(signature.to_string()),
                    redacted: false,
                });
            }
            Some("redacted_thinking") => {
                // Pi-mono parity: capture the opaque `data` payload so
                // we can re-emit it as `redacted_thinking` on the next
                // turn. Without this, providers that expect contiguous
                // reasoning context reject the replay. See
                // `pi-mono/packages/ai/src/providers/anthropic.ts:511`.
                let Some(data) = block.get("data").and_then(Value::as_str) else {
                    continue;
                };
                out.push(ConversationItem::Reasoning {
                    summary: Vec::new(),
                    encrypted_content: Some(data.to_string()),
                    redacted: true,
                });
            }
            _ => {}
        }
    }

    let texts: Vec<&str> = content
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect();
    if !texts.is_empty() {
        out.push(ConversationItem::assistant_message(texts.join("\n")));
    }

    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let call_id = block
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let arguments = block
            .get("input")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "{}".to_string());
        out.push(ConversationItem::FunctionCall {
            call_id,
            name,
            arguments,
        });
    }

    out
}

/// Pure parsing: vendor `content[].type == "tool_use"` → neutral
/// `ToolCallRequest`. Used by both the loop driver and the test helper
/// `execute_anthropic_tool_calls`.
fn extract_tool_calls(response: &Value) -> Result<Vec<ToolCallRequest>> {
    let Some(content) = response.get("content").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut calls = Vec::with_capacity(content.len());
    for item in content {
        if item.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let tool_id = item
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("anthropic tool_use block missing name"))?;
        let call_id = item
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("anthropic tool_use block missing id"))?;
        let input = item
            .get("input")
            .ok_or_else(|| anyhow!("anthropic tool_use block missing input"))?;
        calls.push(ToolCallRequest {
            call_id: call_id.to_string(),
            tool_id: tool_id.to_string(),
            input: serde_json::to_string(input)?,
        });
    }
    Ok(calls)
}

// ---------------------------------------------------------------------------
// Vendor helpers (kept across the refactor — used by session impl + tests).
// ---------------------------------------------------------------------------

fn anthropic_auth_for_provider(
    auth_store: &AuthStore,
    provider: &ProviderDescriptor,
) -> Result<AnthropicAuth> {
    match auth_store.get(&provider.id) {
        Some(StoredCredential::ApiKey { key }) => Ok(AnthropicAuth::ApiKey(key.clone())),
        Some(StoredCredential::OAuth(OAuthCredential { access_token, .. })) => {
            Ok(AnthropicAuth::OAuthBearer(access_token.clone()))
        }
        None if provider.auth_modes.is_empty() => Ok(AnthropicAuth::None),
        None => get_session_ingress_auth().ok_or_else(|| {
            anyhow!(
                "no credentials configured for provider {}; use `puffer auth set-api-key {}` first",
                provider.id,
                provider.id
            )
        }),
    }
}

fn parse_anthropic_text(response: &Value) -> Result<String> {
    let parts = response
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("anthropic response missing content array"))?
        .iter()
        .filter_map(|item| {
            let item_type = item.get("type").and_then(Value::as_str)?;
            if item_type == "text" {
                item.get("text").and_then(Value::as_str).map(str::to_string)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        bail!("anthropic response did not contain text content");
    }
    Ok(parts.join("\n"))
}

fn anthropic_generate_summary(
    old_context: &str,
    model_id: &str,
    url: &str,
    headers: &[(String, String)],
) -> Option<String> {
    let compact_prompt = format!(
        "Summarize this conversation fragment into a compact context block. \
         Preserve file paths, function names, errors, and key decisions verbatim. \
         Structure: 1) Intent 2) Key Concepts 3) Files & Code 4) Errors & Fixes \
         5) Pending Tasks 6) Current State. Be thorough but concise. \
         Do NOT use any tools.\n\n---\n\n{old_context}"
    );

    let body = json!({
        "model": model_id,
        "max_tokens": 16_384,
        "messages": [{"role": "user", "content": compact_prompt}],
    });

    match send_http_request(url, headers, &body.to_string(), true) {
        Ok(response) => parse_anthropic_text(&response).ok(),
        Err(_) => None,
    }
}

fn anthropic_system_blocks(
    attribution_prefix_block: &str,
    system_prompt: Option<&str>,
    plan_mode_context: Option<&str>,
    system_reminder: Option<&str>,
) -> Vec<Value> {
    let mut blocks = vec![json!({
        "type": "text",
        "text": attribution_prefix_block,
    })];
    if let Some(system_prompt) = system_prompt.filter(|prompt| !prompt.trim().is_empty()) {
        blocks.push(json!({
            "type": "text",
            "text": system_prompt,
            "cache_control": { "type": "ephemeral" }
        }));
    }
    if let Some(plan_mode_context) = plan_mode_context {
        blocks.push(json!({
            "type": "text",
            "text": plan_mode_context,
        }));
    }
    if let Some(reminder) = system_reminder.filter(|r| !r.trim().is_empty()) {
        blocks.push(json!({
            "type": "text",
            "text": format!(
                "<system-reminder>\nAs you answer the user's questions, you can use the following context:\n\
                 {reminder}\n\n\
                 IMPORTANT: this context may or may not be relevant to your tasks. \
                 You should not respond to this context unless it is highly relevant to your task.\n\
                 </system-reminder>"
            ),
        }));
    }
    blocks
}

/// Echoes the streaming-request URL + body to `PUFFER_HTTP_TRACE_PATH`
/// when set, so multi-turn tool flows can be inspected end-to-end.
/// Mirrors `trace_http_response`'s output style for the blocking path,
/// just on the request side. Cheap no-op when the env var is unset.
fn trace_anthropic_stream_request(url: &str, body: &serde_json::Value) {
    write_trace(|file| {
        use std::io::Write as _;
        let pretty = serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string());
        writeln!(
            file,
            "--- ANTHROPIC STREAM REQUEST {} ---\n{}\n",
            url, pretty
        )
    });
}

/// Echoes the reconstructed response payload (after SSE accumulation)
/// to the trace file. Captures the upstream's content blocks
/// (text / thinking / tool_use) so we can correlate request shape
/// with response shape on multi-turn flows.
fn trace_anthropic_stream_response(url: &str, body: &serde_json::Value) {
    write_trace(|file| {
        use std::io::Write as _;
        let pretty = serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string());
        writeln!(
            file,
            "--- ANTHROPIC STREAM RESPONSE {} ---\n{}\n",
            url, pretty
        )
    });
}

/// Echoes a non-2xx error response to the trace file. Useful for
/// inspecting 400 / 429 bodies that never reach the SSE parser.
fn trace_anthropic_stream_error(url: &str, status: u16, body: &str) {
    write_trace(|file| {
        use std::io::Write as _;
        writeln!(
            file,
            "--- ANTHROPIC STREAM ERROR {} status={} ---\n{}\n",
            url, status, body
        )
    });
}

fn write_trace<F>(write: F)
where
    F: FnOnce(&mut std::fs::File) -> std::io::Result<()>,
{
    let Ok(path) = std::env::var("PUFFER_HTTP_TRACE_PATH") else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let _ = write(&mut file);
}

/// Resolve whether the model accepts the Anthropic-style `thinking`
/// block. Decision is **family-based**, not id-based: this function
/// only runs inside the Anthropic-Messages adapter, so by the time we
/// get here the request is already going to an Anthropic-Messages
/// endpoint. Per the Anthropic protocol, every compliant endpoint
/// (canonical `api.anthropic.com`, Kimi `kimi-coding`, Bedrock /
/// Vertex Anthropic relays, self-hosted proxies, …) accepts the
/// `thinking` field — verified via curl against `kimi-coding/k2p5`,
/// which returns full `thinking` content blocks when the request
/// carries `thinking: { type: "enabled", … }`.
///
/// The `Model.supports_reasoning` flag still gates whether the field
/// is actually sent, so non-reasoning models keep their cheap
/// no-thinking path. To opt OUT for a relay that rejects the field
/// despite the spec, set
/// `Model.compat.AnthropicMessages.supports_thinking_api: false` on
/// the descriptor.
///
/// Historical note: this used to gate on
/// `provider.id == "anthropic" || base_url.contains("anthropic.com")`
/// — too narrow, silently dropped reasoning for every third-party
/// Anthropic-compatible reasoning model.
fn anthropic_supports_thinking_api(provider: &ProviderDescriptor, model_id: &str) -> bool {
    provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.compat.as_ref())
        .and_then(|c| c.as_anthropic_messages())
        .and_then(|c| c.supports_thinking_api)
        .unwrap_or(true)
}

// ---------------------------------------------------------------------------
// Adapter — thin wrapper that builds the session and hands off to the
// neutral agent_loop driver.
// ---------------------------------------------------------------------------

pub(crate) struct AnthropicAdapter;

impl super::provider_adapter::ProviderAdapter for AnthropicAdapter {
    fn api_id(&self) -> &'static str {
        "anthropic-messages"
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
        options: TurnRequestOptions<'_>,
    ) -> Result<TurnExecution> {
        let registry =
            super::mcp_discovery::registry_with_mcp_tools(resources, state.tool_runner.as_ref());
        let mut session = setup_anthropic_session(
            state, resources, provider, model_id, auth_store, &options, input,
        )?;
        let mut inputs = LoopInputs {
            state,
            resources,
            providers,
            provider,
            model_id: &session.model_id.clone(),
            auth_store,
            input,
            reflection_config: options.reflection.clone(),
            tool_filter: options.tool_filter,
            registry: &registry,
            cancel: options.cancel,
            max_turns: options.max_turns,
            observability: options.observability.clone(),
        };
        super::blocking_loop::run_blocking_loop(&mut inputs, &mut session)
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
        options: TurnRequestOptions<'_>,
        on_event: &mut dyn FnMut(TurnStreamEvent),
    ) -> Result<TurnExecution> {
        let registry =
            super::mcp_discovery::registry_with_mcp_tools(resources, state.tool_runner.as_ref());
        let mut session = setup_anthropic_session(
            state, resources, provider, model_id, auth_store, &options, input,
        )?;
        let mut inputs = LoopInputs {
            state,
            resources,
            providers,
            provider,
            model_id: &session.model_id.clone(),
            auth_store,
            input,
            reflection_config: options.reflection.clone(),
            tool_filter: options.tool_filter,
            registry: &registry,
            cancel: options.cancel,
            max_turns: options.max_turns,
            observability: options.observability.clone(),
        };
        agent_loop::run_streaming_loop(&mut inputs, &mut session, on_event)
    }
}

// ---------------------------------------------------------------------------
// Test-only helpers — preserve existing test coverage of the
// extract+execute pipeline against fake response payloads. Production
// code path goes through `agent_loop` and never calls these.
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(super) fn anthropic_tool_schema(handler: &str) -> Value {
    match handler {
        "bash" => json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" }
            },
            "required": ["command"],
        }),
        "read_file" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"],
        }),
        "write_file" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "contents": { "type": "string" }
            },
            "required": ["path", "contents"],
        }),
        "replace_in_file" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old": { "type": "string" },
                "new": { "type": "string" },
                "replace_all": { "type": "boolean" }
            },
            "required": ["path", "old", "new"],
        }),
        "list_dir" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": [],
        }),
        "search_text" => json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "path": { "type": "string" }
            },
            "required": ["query"],
        }),
        _ => json!({
            "type": "object",
            "properties": {},
        }),
    }
}

#[cfg(test)]
pub(super) struct AnthropicToolResults {
    #[allow(dead_code)]
    pub(super) results: Value,
    pub(super) invocations: Vec<ToolInvocation>,
}

#[cfg(test)]
pub(super) fn execute_anthropic_tool_calls(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    response: &Value,
    registry: &ToolRegistry,
    cwd: &std::path::Path,
    request_config: &AnthropicRequestConfig,
    model_id: &str,
    structured_output: Option<&StructuredOutputConfig>,
    tool_filter: Option<&RequestToolFilter>,
) -> Result<Option<AnthropicToolResults>> {
    let calls = extract_tool_calls(response)?;
    if calls.is_empty() {
        return Ok(None);
    }

    let mut results = Vec::new();
    let mut invocations = Vec::new();
    for call in &calls {
        let input_value: Value = serde_json::from_str(&call.input).unwrap_or(Value::Null);
        let execution = execute_tool_call(
            state,
            resources,
            providers,
            auth_store,
            registry,
            model_id,
            cwd,
            ToolExecutionBackend::Anthropic {
                request_config,
                structured_output,
            },
            tool_filter,
            &call.tool_id,
            input_value,
        )?;
        let raw_output = if execution.output.stderr.is_empty() {
            execution.output.stdout
        } else if execution.output.stdout.is_empty() {
            execution.output.stderr
        } else {
            format!("{}\n{}", execution.output.stdout, execution.output.stderr)
        };
        let raw_output = super::secrets::redact_known_secrets(state, &raw_output);
        let metadata = super::secrets::redact_json_value(state, &execution.output.metadata);
        let output_text =
            process_tool_result(&raw_output, MAX_TOOL_RESULT_CHARS, &state.session.id);
        results.push(json!({
            "type": "tool_result",
            "tool_use_id": call.call_id,
            "content": output_text,
            "is_error": !execution.success,
        }));
        invocations.push(ToolInvocation {
            call_id: call.call_id.clone(),
            tool_id: call.tool_id.clone(),
            input: call.input.clone(),
            output: output_text.clone(),
            success: execution.success,
            metadata,
            terminate: false,
        });
    }

    let mut output_strings: Vec<String> = invocations.iter().map(|i| i.output.clone()).collect();
    enforce_tool_result_budget(&mut output_strings, &state.session.id);
    for (i, new_output) in output_strings.into_iter().enumerate() {
        if new_output != invocations[i].output {
            results[i]["content"] = json!(new_output);
            invocations[i].output = new_output;
        }
    }

    Ok(Some(AnthropicToolResults {
        results: Value::Array(results),
        invocations,
    }))
}

#[cfg(test)]
#[allow(dead_code)]
fn append_anthropic_response_to_items(
    items: &mut Vec<ConversationItem>,
    response: &Value,
    tool_results: &AnthropicToolResults,
) {
    items.extend(synthesize_pre_tool_items(response));
    for inv in &tool_results.invocations {
        items.push(ConversationItem::FunctionCallOutput {
            call_id: inv.call_id.clone(),
            output: if inv.success {
                ToolOutputPayload::success(inv.output.clone())
            } else {
                ToolOutputPayload::error(inv.output.clone())
            },
        });
    }
}

#[cfg(test)]
mod thinking_gate_tests {
    use super::anthropic_supports_thinking_api;
    use puffer_provider_registry::{
        AnthropicMessagesCompat, AuthMode, Modality, ModelCompat, ModelDescriptor,
        ProviderDescriptor,
    };

    fn provider_with(
        id: &str,
        base_url: &str,
        model_compat: Option<ModelCompat>,
    ) -> ProviderDescriptor {
        ProviderDescriptor {
            id: id.to_string(),
            display_name: id.to_string(),
            base_url: base_url.to_string(),
            default_api: "anthropic-messages".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: Default::default(),
            query_params: Default::default(),
            discovery: None,
            media: None,
            models: vec![ModelDescriptor {
                id: "claude-sonnet-4-5".to_string(),
                display_name: "Claude".to_string(),
                provider: id.to_string(),
                api: "anthropic-messages".to_string(),
                context_window: 200_000,
                max_output_tokens: 8192,
                supports_reasoning: true,
                input: vec![Modality::Text],
                cost: None,
                compat: model_compat,
            }],
            chat_completions_path: None,
        }
    }

    #[test]
    fn canonical_anthropic_defaults_to_thinking_supported() {
        let provider = provider_with("anthropic", "https://api.anthropic.com", None);
        assert!(anthropic_supports_thinking_api(
            &provider,
            "claude-sonnet-4-5"
        ));
    }

    #[test]
    fn third_party_anthropic_compatible_relay_defaults_to_supported() {
        // Regression: kimi-coding/k2p5 returns full Anthropic-shaped
        // `thinking` content blocks when the request carries the
        // thinking field, but the previous heuristic gated on
        // `provider.id == "anthropic" || base_url contains anthropic.com`
        // and silently dropped reasoning for any non-anthropic.com URL.
        // Verified via curl against api.kimi.com/coding before flipping
        // the default.
        let provider = provider_with("kimi-coding", "https://api.kimi.com/coding", None);
        assert!(anthropic_supports_thinking_api(
            &provider,
            "claude-sonnet-4-5"
        ));
    }

    #[test]
    fn explicit_compat_opt_out_wins_over_default() {
        // For a relay that does NOT accept the thinking field, users
        // can drop a workspace-level provider yaml with
        // `compat: { kind: anthropic_messages, supports_thinking_api: false }`
        // to keep the agent loop from sending it.
        let compat = ModelCompat::AnthropicMessages(AnthropicMessagesCompat {
            supports_thinking_api: Some(false),
        });
        let provider = provider_with("custom-anthropic", "http://1.2.3.4/", Some(compat));
        assert!(!anthropic_supports_thinking_api(
            &provider,
            "claude-sonnet-4-5"
        ));
    }

    #[test]
    fn explicit_compat_opt_in_wins_over_default() {
        let compat = ModelCompat::AnthropicMessages(AnthropicMessagesCompat {
            supports_thinking_api: Some(true),
        });
        let provider = provider_with("custom-anthropic", "http://1.2.3.4/", Some(compat));
        assert!(anthropic_supports_thinking_api(
            &provider,
            "claude-sonnet-4-5"
        ));
    }

    #[test]
    fn unknown_model_id_falls_back_to_default_supported() {
        let provider = provider_with("kimi-coding", "https://api.kimi.com/coding", None);
        // No model with id="not-a-model" → no compat to read → default true.
        assert!(anthropic_supports_thinking_api(&provider, "not-a-model"));
    }
}

#[cfg(test)]
mod thinking_round_trip_tests {
    use super::synthesize_pre_tool_items;
    use crate::runtime::openai::conversation::{
        items_to_anthropic_messages, ConversationItem, ReasoningSummary,
    };
    use serde_json::json;

    /// Regression: Anthropic relays like `kimi-coding/k2p5` reject the
    /// next-turn replay with "reasoning_content is missing in assistant
    /// tool call message" when the assistant message preceding a
    /// `tool_result` does not include a paired `thinking` block. The
    /// fix captures `thinking` blocks (with their `signature`) into a
    /// `Reasoning` item and re-emits them as a `thinking` content block
    /// before the `tool_use` block on the next request.
    #[test]
    fn synthesize_then_replay_preserves_thinking_signature() {
        let response = json!({
            "content": [
                {
                    "type": "thinking",
                    "thinking": "Need to read /tmp/sample.txt.",
                    "signature": "sig-abc-123",
                },
                {
                    "type": "tool_use",
                    "id": "tool_1",
                    "name": "Read",
                    "input": { "path": "/tmp/sample.txt" },
                }
            ]
        });

        let items = synthesize_pre_tool_items(&response);

        // Expected order: Reasoning first (so it merges into the same
        // assistant message and lands BEFORE tool_use, as Anthropic
        // requires).
        assert_eq!(items.len(), 2);
        match &items[0] {
            ConversationItem::Reasoning {
                summary,
                encrypted_content,
                redacted,
            } => {
                assert!(!*redacted);
                assert_eq!(encrypted_content.as_deref(), Some("sig-abc-123"));
                let ReasoningSummary::SummaryText { text } = &summary[0];
                assert_eq!(text, "Need to read /tmp/sample.txt.");
            }
            other => panic!("expected Reasoning, got {other:?}"),
        }
        assert!(matches!(&items[1], ConversationItem::FunctionCall { .. }));

        // Now replay through items_to_anthropic_messages and verify the
        // assistant message contains the thinking block followed by the
        // tool_use block.
        let mut transcript = vec![ConversationItem::user_message("read it")];
        transcript.extend(items);

        let messages = items_to_anthropic_messages(&transcript);
        // [user, assistant]
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "assistant");
        let content = messages[1]["content"]
            .as_array()
            .expect("assistant content should be a block array");
        assert_eq!(
            content.len(),
            2,
            "expected [thinking, tool_use], got {content:?}"
        );
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "Need to read /tmp/sample.txt.");
        assert_eq!(content[0]["signature"], "sig-abc-123");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["id"], "tool_1");
    }

    /// Anthropic returns thinking signatures with a multi-block content
    /// shape (thinking + text + tool_use). Verify all three round-trip
    /// in the correct order: thinking → text → tool_use.
    #[test]
    fn synthesize_then_replay_preserves_thinking_text_and_tool_use_order() {
        let response = json!({
            "content": [
                {
                    "type": "thinking",
                    "thinking": "I will list the dir first.",
                    "signature": "sig-1",
                },
                {
                    "type": "text",
                    "text": "Listing /tmp.",
                },
                {
                    "type": "tool_use",
                    "id": "call_42",
                    "name": "Bash",
                    "input": { "command": "ls /tmp" },
                }
            ]
        });

        let items = synthesize_pre_tool_items(&response);
        let messages = items_to_anthropic_messages(&items);
        let assistant = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant message should be present");
        let content = assistant["content"]
            .as_array()
            .expect("multi-block content");
        let kinds: Vec<&str> = content
            .iter()
            .map(|b| b["type"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(kinds, vec!["thinking", "text", "tool_use"]);
    }

    /// Thinking blocks without a `signature` are unverifiable on replay
    /// (Anthropic rejects the request). Drop them rather than send an
    /// invalid block — the fall-through is "no thinking block in
    /// replay", which is the same shape we had before this fix and
    /// should not regress.
    #[test]
    fn synthesize_drops_thinking_block_without_signature() {
        let response = json!({
            "content": [
                {
                    "type": "thinking",
                    "thinking": "Stub without signature.",
                },
                {
                    "type": "tool_use",
                    "id": "call_x",
                    "name": "Read",
                    "input": { "path": "/x" },
                }
            ]
        });

        let items = synthesize_pre_tool_items(&response);
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], ConversationItem::FunctionCall { .. }));
    }

    /// Pi-mono parity: when a Reasoning item lacks a signature (e.g.,
    /// from an aborted stream that left the item in transcript), the
    /// Anthropic wire builder falls back to emitting the thinking text
    /// as a plain `text` block instead of dropping it. Preserves the
    /// model's chain-of-thought without sending an invalid signature.
    #[test]
    fn replay_without_signature_falls_back_to_text_block() {
        let items = vec![
            ConversationItem::user_message("hi"),
            ConversationItem::Reasoning {
                summary: vec![ReasoningSummary::SummaryText {
                    text: "Reasoned without signature.".to_string(),
                }],
                encrypted_content: None,
                redacted: false,
            },
            ConversationItem::FunctionCall {
                call_id: "call_y".to_string(),
                name: "Bash".to_string(),
                arguments: "{\"command\":\"ls\"}".to_string(),
            },
        ];
        let messages = items_to_anthropic_messages(&items);
        let assistant = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant message");
        let content = assistant["content"]
            .as_array()
            .expect("multi-block assistant content");
        // [text fallback, tool_use]
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Reasoned without signature.");
        assert_eq!(content[1]["type"], "tool_use");
    }

    /// Pi-mono parity: when the upstream returns a `redacted_thinking`
    /// block (encrypted reasoning blocked from display by safety
    /// filters), Puffer captures the opaque `data` payload and
    /// re-emits it verbatim on the next turn. Emitting the data as a
    /// regular thinking signature would fail upstream cryptographic
    /// verification.
    #[test]
    fn redacted_thinking_round_trips_via_data_payload() {
        let response = json!({
            "content": [
                {
                    "type": "redacted_thinking",
                    "data": "OPAQUE-REDACTED-PAYLOAD",
                },
                {
                    "type": "tool_use",
                    "id": "call_redacted",
                    "name": "Read",
                    "input": { "path": "fixture.txt" }
                }
            ]
        });

        let items = synthesize_pre_tool_items(&response);
        assert_eq!(items.len(), 2);
        match &items[0] {
            ConversationItem::Reasoning {
                summary,
                encrypted_content,
                redacted,
            } => {
                assert!(*redacted);
                assert_eq!(
                    encrypted_content.as_deref(),
                    Some("OPAQUE-REDACTED-PAYLOAD")
                );
                assert!(summary.is_empty());
            }
            other => panic!("expected redacted Reasoning, got {other:?}"),
        }

        // Round-trip through items_to_anthropic_messages: assistant
        // message must include {type: "redacted_thinking", data: ...}
        // BEFORE the tool_use block.
        let mut transcript = vec![ConversationItem::user_message("read it")];
        transcript.extend(items);
        let messages = items_to_anthropic_messages(&transcript);
        let assistant = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant message expected");
        let blocks = assistant["content"]
            .as_array()
            .expect("multi-block content array");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "redacted_thinking");
        assert_eq!(blocks[0]["data"], "OPAQUE-REDACTED-PAYLOAD");
        assert_eq!(blocks[1]["type"], "tool_use");
    }

    /// `redacted_thinking` blocks lacking a `data` field are dropped
    /// rather than emitting a malformed block (the upstream would
    /// reject `redacted_thinking` without `data`).
    #[test]
    fn redacted_thinking_without_data_is_dropped() {
        let response = json!({
            "content": [
                { "type": "redacted_thinking" },
                {
                    "type": "tool_use",
                    "id": "call_x",
                    "name": "Read",
                    "input": { "path": "x" }
                }
            ]
        });
        let items = synthesize_pre_tool_items(&response);
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], ConversationItem::FunctionCall { .. }));
    }
}

#[cfg(test)]
mod too_large_error_tests {
    use super::is_anthropic_too_large_error;

    #[test]
    fn matches_413_status_code() {
        assert!(is_anthropic_too_large_error(
            "request to https://api.anthropic.com/v1/messages failed with status 413: Request Entity Too Large"
        ));
    }

    #[test]
    fn matches_request_too_large_typed_error() {
        // The Anthropic API's actual error.type for too-large
        // requests; also the form `parse_anthropic_sse` now emits
        // (`Anthropic SSE error [request_too_large]: ...`) after
        // the error.type-surfacing change.
        assert!(is_anthropic_too_large_error(
            "Anthropic SSE error [request_too_large]: prompt is too long for context window"
        ));
    }

    #[test]
    fn matches_legacy_prompt_too_long() {
        assert!(is_anthropic_too_large_error(
            "request failed with status 400: prompt_too_long"
        ));
    }

    #[test]
    fn matches_payload_too_large_status_text() {
        assert!(is_anthropic_too_large_error("Payload Too Large"));
    }

    #[test]
    fn matches_request_entity_too_large_status_text() {
        // Some upstream proxies / gateways emit this verbatim.
        assert!(is_anthropic_too_large_error(
            "HTTP 413 Request Entity Too Large"
        ));
    }

    #[test]
    fn matches_too_long_substring() {
        // Catch-all for free-form messages.
        assert!(is_anthropic_too_large_error(
            "the prompt is too long to process"
        ));
    }

    #[test]
    fn matches_case_insensitively() {
        assert!(is_anthropic_too_large_error(
            "REQUEST_TOO_LARGE encountered"
        ));
        assert!(is_anthropic_too_large_error("PAYLOAD TOO LARGE"));
    }

    #[test]
    fn does_not_match_other_4xx_errors() {
        assert!(!is_anthropic_too_large_error(
            "request failed with status 400: invalid_request_error"
        ));
        assert!(!is_anthropic_too_large_error(
            "request failed with status 401: authentication_error"
        ));
        assert!(!is_anthropic_too_large_error(
            "Anthropic SSE error [overloaded_error]: Service is temporarily overloaded"
        ));
    }
}
