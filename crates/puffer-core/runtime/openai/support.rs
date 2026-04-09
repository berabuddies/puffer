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
        "parallel_tool_calls": false,
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
    let text = error.to_string().to_ascii_lowercase();
    text.contains("operation timed out")
        || text.contains("error decoding response body")
        || text.contains("connection reset")
        || text.contains("unexpected eof")
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
    let mut items = openai_input_items(input)
        .into_iter()
        .filter(|item| {
            !matches!(
                item.get("type").and_then(Value::as_str),
                Some("function_call" | "function_call_output")
            )
        })
        .collect::<Vec<_>>();
    items.extend(openai_input_items(continuation));
    Value::Array(items)
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

fn codex_reasoning_config(state: &AppState, supports_reasoning: bool) -> Option<Value> {
    if !supports_reasoning {
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
