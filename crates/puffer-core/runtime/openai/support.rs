use super::super::{APP_VERSION, OPENAI_CHATGPT_BASE_URL};
use super::StructuredOutputConfig;
use crate::AppState;
use anyhow::{Error, Result};
use puffer_provider_openai::{OpenAIResponsesTextConfig, OpenAIResponsesTool};
use puffer_provider_registry::{OAuthCredential, ProviderDescriptor};
use serde_json::{json, Value};
use std::io;
use std::time::Duration;

pub(super) const OPENAI_STRUCTURED_OUTPUT_FAMILY: &str = "openai";

pub(crate) fn build_codex_openai_request_body(
    state: &AppState,
    model_id: &str,
    instructions: &str,
    input: Value,
    tools: &[OpenAIResponsesTool],
    supports_reasoning: bool,
    text: Option<OpenAIResponsesTextConfig>,
    stream: bool,
) -> Value {
    let reasoning = codex_reasoning_config(state, supports_reasoning);
    let include = if reasoning.is_some() {
        vec![json!("reasoning.encrypted_content")]
    } else {
        Vec::new()
    };
    let store = std::env::var("PUFFER_OPENAI_STORE_RESPONSES")
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    let mut body = json!({
        "model": model_id,
        "instructions": instructions,
        "input": codex_input_items(input),
        "tools": tools,
        "tool_choice": "auto",
        "parallel_tool_calls": !tools.is_empty(),
        "store": store,
        "stream": stream,
        "include": include,
        "prompt_cache_key": state.session.id.to_string(),
    });
    if let Some(reasoning) = reasoning {
        body["reasoning"] = reasoning;
    }
    if let Some(text) = text {
        body["text"] = serde_json::to_value(text).unwrap_or(Value::Null);
    }
    if state.fast_mode {
        body["service_tier"] = json!("priority");
    }
    body
}

pub(super) fn prefer_native_structured_output(
    state: &AppState,
    provider: &ProviderDescriptor,
    model_id: &str,
    structured_output: Option<&StructuredOutputConfig>,
) -> bool {
    structured_output.is_some()
        && !state.is_native_structured_output_unsupported(
            OPENAI_STRUCTURED_OUTPUT_FAMILY,
            provider.id.as_str(),
            model_id,
            provider.base_url.as_str(),
        )
}

pub(super) fn structured_output_endpoint_id(provider: &ProviderDescriptor) -> &str {
    provider.base_url.as_str()
}

pub(super) fn is_openai_structured_output_error(error: &anyhow::Error) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    [
        "response_format",
        "text.format",
        "\"text\"",
        "json_schema",
        "json schema",
        "structured output",
        "structured_output",
        "\"strict\"",
    ]
    .iter()
    .any(|pattern| text.contains(pattern))
}

pub(super) fn retry_openai_transport<F, T>(mut operation: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let attempts = openai_transport_max_attempts();
    let delay = openai_transport_retry_delay();
    for attempt in 1..=attempts {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt < attempts && is_retryable_openai_transport_error(&error) => {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("retry loop always returns or errors")
}

fn openai_transport_max_attempts() -> usize {
    std::env::var("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(3)
        .clamp(1, 5)
}

fn openai_transport_retry_delay() -> Duration {
    let delay_ms = std::env::var("PUFFER_OPENAI_HTTP_RETRY_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_000)
        .min(10_000);
    Duration::from_millis(delay_ms)
}

pub(super) fn openai_stream_read_timeout() -> Duration {
    let timeout_ms = std::env::var("PUFFER_OPENAI_STREAM_READ_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30_000)
        .clamp(1_000, 300_000);
    Duration::from_millis(timeout_ms)
}

fn is_retryable_openai_transport_error(error: &Error) -> bool {
    if error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|value| value.is_timeout() || value.is_connect() || value.is_body())
    }) {
        return true;
    }
    if error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|value| value.kind() == io::ErrorKind::TimedOut)
    }) {
        return true;
    }
    error.chain().any(|cause| {
        let text = cause.to_string().to_ascii_lowercase();
        text.contains("operation timed out")
            || text.contains("error decoding response body")
            || text.contains("connection reset")
            || text.contains("unexpected eof")
            || text.contains("stream closed before response.completed")
            || text.contains("idle timeout waiting for sse")
    })
}

pub(super) fn openai_registry_credential(
    credential: puffer_provider_openai::OpenAIOAuthCredentials,
) -> OAuthCredential {
    OAuthCredential {
        access_token: credential.access_token,
        refresh_token: credential.refresh_token,
        expires_at_ms: credential.expires_at_ms,
        account_id: credential.account_id,
        organization_id: None,
        email: credential.email,
        plan_type: credential.plan_type,
        rate_limit_tier: None,
        scopes: Vec::new(),
        organization_name: None,
        organization_role: None,
        workspace_role: None,
    }
}

pub(super) fn extend_input_with_continuation(input: Value, continuation: Value) -> Value {
    let mut items = openai_input_items(input);
    items.extend(openai_input_items(continuation));
    Value::Array(items)
}

pub(super) fn apply_previous_response_id(body: &mut Value, previous_response_id: Option<&str>) {
    if let Some(previous_response_id) = previous_response_id {
        body["previous_response_id"] = json!(previous_response_id);
    }
}

pub(super) fn next_openai_input(
    previous_response_id: Option<&str>,
    input: Value,
    continuation: Value,
) -> Value {
    if previous_response_id.is_none() {
        return extend_input_with_continuation(input, continuation);
    }
    continuation
}

pub(super) fn openai_responses_path(base_url: &str) -> &'static str {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.contains("/backend-api") || trimmed.contains("/api/codex") {
        "/responses"
    } else {
        "/v1/responses"
    }
}

pub(super) fn openai_model_supports_reasoning(
    provider: &ProviderDescriptor,
    model_id: &str,
) -> bool {
    provider
        .models
        .iter()
        .find(|model| model.id == model_id)
        .map(|model| model.supports_reasoning)
        .unwrap_or(false)
}

pub(super) fn append_default_openai_headers(
    headers: &mut Vec<(String, String)>,
    provider_id: &str,
) {
    if provider_id == "openai" && !has_header(headers, "version") {
        headers.push(("version".to_string(), APP_VERSION.to_string()));
    }
    append_env_header(headers, "OpenAI-Organization", "OPENAI_ORGANIZATION");
    append_env_header(headers, "OpenAI-Project", "OPENAI_PROJECT");
}

pub(super) fn is_codex_openai_provider(provider: &ProviderDescriptor) -> bool {
    provider.default_api == "openai-codex-responses"
        || provider
            .base_url
            .trim_end_matches('/')
            .contains("/backend-api")
        || provider
            .base_url
            .trim_end_matches('/')
            .contains("/api/codex")
}

pub(super) fn openai_base_url_for_auth(provider: &ProviderDescriptor, oauth: bool) -> String {
    if !oauth || provider.id != "openai" {
        return provider.base_url.clone();
    }
    let trimmed = provider.base_url.trim_end_matches('/');
    if trimmed.contains("/backend-api") || trimmed.contains("/api/codex") {
        trimmed.to_string()
    } else {
        OPENAI_CHATGPT_BASE_URL.to_string()
    }
}

fn append_env_header(headers: &mut Vec<(String, String)>, header: &str, env_var: &str) {
    if has_header(headers, header) {
        return;
    }
    if let Ok(value) = std::env::var(env_var) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            headers.push((header.to_string(), trimmed.to_string()));
        }
    }
}

fn has_header(headers: &[(String, String)], name: &str) -> bool {
    headers
        .iter()
        .any(|(header, _)| header.eq_ignore_ascii_case(name))
}

pub(super) fn trace_openai_http_request(url: &str, headers: &[(String, String)], body: &str) {
    let Ok(path) = std::env::var("PUFFER_HTTP_TRACE_PATH") else {
        return;
    };
    let rendered_headers = headers
        .iter()
        .map(|(key, value)| {
            if key.eq_ignore_ascii_case("authorization") {
                format!("{key}: <redacted>")
            } else {
                format!("{key}: {value}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(
                file,
                "--- REQUEST {} ---\n{}\n\n{}\n",
                url, rendered_headers, body
            )
        });
}

pub(super) fn trace_openai_http_response_headers(
    url: &str,
    status: u16,
    content_type: Option<&str>,
) {
    let Ok(path) = std::env::var("PUFFER_HTTP_TRACE_PATH") else {
        return;
    };
    let content_type = content_type.unwrap_or("<missing>");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(
                file,
                "--- RESPONSE_HEADERS {} {} ---\ncontent-type: {}\n",
                status, url, content_type
            )
        });
}

fn codex_input_items(input: Value) -> Value {
    match input {
        Value::Array(_) => input,
        Value::String(text) => json!([
            {
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": text,
                    }
                ],
            }
        ]),
        other => other,
    }
}

fn openai_input_items(input: Value) -> Vec<Value> {
    match input {
        Value::Array(items) => items,
        Value::String(text) => vec![json!({
            "type": "message",
            "role": "user",
            "content": [
                {
                    "type": "input_text",
                    "text": text,
                }
            ],
        })],
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

fn assistant_output_items(response: &Value) -> Vec<Value> {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            item.get("type").and_then(Value::as_str) == Some("message")
                && item.get("role").and_then(Value::as_str) == Some("assistant")
        })
        .cloned()
        .collect()
}

pub(super) fn extend_input_with_response_items(
    input: Value,
    response: &Value,
    continuation: Value,
) -> Value {
    let mut items = openai_input_items(input);
    items.extend(assistant_output_items(response));
    items.extend(openai_input_items(continuation));
    Value::Array(items)
}

/// Maximum number of consecutive auto-compact cycles before giving up.
const MAX_COMPACT_CYCLES: usize = 10;

/// Compact the OpenAI input array when total size exceeds ~80% of context.
/// Phase 1: Snip old function_call_output items.
/// Phase 2: Generate AI summary of old items via the Responses API.
/// Phase 3 (fallback): Drop oldest items if summary fails.
pub(super) fn compact_openai_input(
    input: &mut Value,
    provider: &ProviderDescriptor,
    model_id: &str,
) {
    let items = match input.as_array_mut() {
        Some(arr) => arr,
        None => return,
    };
    if items.len() <= 3 {
        return;
    }

    let context_window = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.context_window as usize)
        .unwrap_or(200_000);
    let threshold = context_window * 80 / 100;

    let estimate = |arr: &[Value]| -> usize {
        arr.iter()
            .map(|item| {
                item.get("output")
                    .and_then(Value::as_str)
                    .map(|s| s.len())
                    .unwrap_or_else(|| {
                        item.get("content")
                            .map(|c| c.to_string().len())
                            .unwrap_or(50)
                    })
            })
            .sum::<usize>()
            / 4
    };

    if estimate(items) <= threshold {
        return;
    }

    // Phase 1: Snip old function_call_output items (keep last 6).
    let keep_recent = 6;
    if items.len() > keep_recent {
        let cutoff = items.len() - keep_recent;
        for item in &mut items[..cutoff] {
            if item.get("type").and_then(Value::as_str) == Some("function_call_output") {
                if let Some(output) = item.get("output").and_then(Value::as_str) {
                    if output.len() > 500 {
                        let snipped: String = output.chars().take(500).collect();
                        item["output"] =
                            Value::String(format!("{snipped}\n[...output snipped...]"));
                    }
                }
            }
        }
    }

    if estimate(items) <= threshold {
        return;
    }

    // Phase 2: Generate AI summary of old items, keep last 4 intact.
    let keep_count = 4.min(items.len());
    let to_summarize = &items[..items.len() - keep_count];
    if !to_summarize.is_empty() {
        // Build a text representation of old items for summarization.
        let mut old_context = String::new();
        for item in to_summarize {
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("?");
            match item_type {
                "message" => {
                    let role = item.get("role").and_then(Value::as_str).unwrap_or("?");
                    let text = item
                        .get("content")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|b| b.get("text"))
                        .and_then(Value::as_str)
                        .or_else(|| item.get("content").and_then(Value::as_str))
                        .unwrap_or("");
                    let preview: String = text.chars().take(500).collect();
                    old_context.push_str(&format!("[{role}]: {preview}\n\n"));
                }
                "function_call" => {
                    let name = item.get("name").and_then(Value::as_str).unwrap_or("?");
                    let args = item.get("arguments").and_then(Value::as_str).unwrap_or("");
                    let preview: String = args.chars().take(200).collect();
                    old_context.push_str(&format!("[tool_call {name}]: {preview}\n\n"));
                }
                "function_call_output" => {
                    let out = item.get("output").and_then(Value::as_str).unwrap_or("");
                    let preview: String = out.chars().take(300).collect();
                    old_context.push_str(&format!("[tool_result]: {preview}\n\n"));
                }
                _ => {}
            }
        }

        // Try to generate summary (best effort — if API unavailable, fall through to Phase 3).
        let summary = generate_openai_summary(&old_context, model_id, provider);

        if let Some(summary_text) = summary {
            let kept: Vec<Value> = items.split_off(items.len() - keep_count);
            items.clear();
            items.push(serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text":
                    format!("[Conversation compacted — prior context summarized]\n\n{summary_text}")
                }]
            }));
            items.extend(kept);
            return;
        }
    }

    // Phase 3 (fallback): Drop oldest items with circuit breaker.
    let mut cycles = 0;
    while items.len() > 3 && estimate(items) > threshold && cycles < MAX_COMPACT_CYCLES {
        items.remove(0);
        cycles += 1;
    }

    if items
        .first()
        .and_then(|i| i.get("type").and_then(Value::as_str))
        != Some("message")
    {
        items.insert(
            0,
            serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "[Earlier context compacted]"}]
            }),
        );
    }
}

/// Try to generate a summary of old context via the OpenAI Responses API.
fn generate_openai_summary(
    old_context: &str,
    model_id: &str,
    provider: &ProviderDescriptor,
) -> Option<String> {
    use reqwest::blocking::Client;

    let api_key = std::env::var("OPENAI_API_KEY").ok()?;
    let base_url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| provider.base_url.clone());

    let prompt = format!(
        "Summarize this conversation fragment into a compact context block. \
         Preserve file paths, function names, errors, and key decisions verbatim. \
         Structure: 1) Intent 2) Key Concepts 3) Files & Code 4) Errors & Fixes \
         5) Pending Tasks 6) Current State. Be thorough but concise. \
         Do NOT use any tools.\n\n---\n\n{old_context}"
    );

    let body = serde_json::json!({
        "model": model_id,
        "input": prompt,
        "stream": false,
    });

    let url = format!("{}/responses", base_url.trim_end_matches('/'));
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .ok()?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .ok()?;

    if !response.status().is_success() {
        return None;
    }

    let json: serde_json::Value = response.json().ok()?;
    json.get("output")?
        .as_array()?
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .and_then(|msg| msg.get("content"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
        .map(String::from)
}

fn codex_reasoning_config(state: &AppState, supports_reasoning: bool) -> Option<Value> {
    if !supports_reasoning {
        return None;
    }
    if std::env::var("PUFFER_OPENAI_DISABLE_REASONING")
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
    {
        return None;
    }
    let mut reasoning = json!({ "summary": "auto" });
    match state.effort_level.as_str() {
        "auto" | "unset" | "default" => {
            reasoning["effort"] = json!("medium");
        }
        "minimal" | "low" | "medium" | "high" | "xhigh" => {
            reasoning["effort"] = json!(state.effort_level);
        }
        "max" => {
            reasoning["effort"] = json!("high");
        }
        _ => {}
    }
    Some(reasoning)
}

#[cfg(test)]
mod tests {
    use super::{extend_input_with_response_items, is_retryable_openai_transport_error};
    use anyhow::anyhow;
    use serde_json::json;

    #[test]
    fn retries_stream_closed_before_completed_errors() {
        let error = anyhow!("stream closed before response.completed");
        assert!(is_retryable_openai_transport_error(&error));
    }

    #[test]
    fn retries_idle_timeout_waiting_for_sse_errors() {
        let error = anyhow!("idle timeout waiting for SSE");
        assert!(is_retryable_openai_transport_error(&error));
    }

    #[test]
    fn retries_wrapped_stream_closed_before_completed_errors() {
        let error = anyhow!("stream closed before response.completed")
            .context("failed to parse SSE response from http://example.test/v1/responses");
        assert!(is_retryable_openai_transport_error(&error));
    }

    #[test]
    fn extends_stateless_input_with_assistant_message_output_items() {
        let input = json!([{
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": "solve task" }],
        }]);
        let response = json!({
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{ "type": "output_text", "text": "working" }],
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "Read",
                    "arguments": "{}",
                }
            ]
        });
        let continuation = json!([{
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "done",
        }]);

        let combined = extend_input_with_response_items(input, &response, continuation);
        assert_eq!(
            combined,
            json!([
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "solve task" }],
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{ "type": "output_text", "text": "working" }],
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "done",
                }
            ])
        );
    }
}
