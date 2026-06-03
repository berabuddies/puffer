use super::super::openai_sse::{
    is_openai_response_incomplete_error, is_retryable_openai_sse_api_error,
};
use super::super::{OPENAI_CHATGPT_BASE_URL, OPENAI_CODEX_COMPAT_VERSION};
use super::StructuredOutputConfig;
use crate::AppState;
use anyhow::{Error, Result};
use puffer_provider_openai::{OpenAIResponsesTextConfig, OpenAIResponsesTool};
use puffer_provider_registry::{
    ModelCompat, ModelDescriptor, OAuthCredential, ProviderDescriptor, ResponsesPath,
};
use rand::Rng;
use serde_json::{json, Value};
use std::io;
use std::time::Duration;

pub(super) const OPENAI_STRUCTURED_OUTPUT_FAMILY: &str = "openai";
const OPENAI_TRANSPORT_DEFAULT_MAX_ATTEMPTS: usize = 5;
const OPENAI_STREAM_DEFAULT_MAX_ATTEMPTS: usize = 6;
const OPENAI_RETRY_MAX_ATTEMPTS_CAP: usize = 101;
const OPENAI_RETRY_DEFAULT_BASE_DELAY_MS: u64 = 200;
const OPENAI_RETRY_BASE_DELAY_CAP_MS: u64 = 10_000;
const OPENAI_RETRY_BACKOFF_CAP_MS: u64 = 60_000;

pub(crate) fn build_codex_openai_request_body(
    state: &AppState,
    base_url: &str,
    model_id: &str,
    instructions: &str,
    input: Value,
    tools: &[OpenAIResponsesTool],
    supports_reasoning: bool,
    text: Option<OpenAIResponsesTextConfig>,
    stream: bool,
) -> Value {
    build_codex_openai_request_body_with_reasoning_include(
        state,
        base_url,
        model_id,
        instructions,
        input,
        tools,
        supports_reasoning,
        text,
        stream,
        request_reasoning_encrypted_content_include(),
    )
}

fn build_codex_openai_request_body_with_reasoning_include(
    state: &AppState,
    base_url: &str,
    model_id: &str,
    instructions: &str,
    input: Value,
    tools: &[OpenAIResponsesTool],
    supports_reasoning: bool,
    text: Option<OpenAIResponsesTextConfig>,
    stream: bool,
    include_reasoning_encrypted_content: bool,
) -> Value {
    let reasoning = codex_reasoning_config(state, supports_reasoning);
    let mut include: Vec<Value> = Vec::new();
    if reasoning.is_some() && include_reasoning_encrypted_content {
        include.push(json!(reasoning_encrypted_content_include(base_url)));
    }
    let store = std::env::var("PUFFER_OPENAI_STORE_RESPONSES")
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    let mut body = json!({
        "model": model_id,
        "instructions": instructions,
        "input": codex_input_items(input),
        "store": store,
        "stream": stream,
        "include": include,
        "prompt_cache_key": state
            .prompt_cache_key_override
            .clone()
            .unwrap_or_else(|| state.session.id.to_string()),
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(true);
    }
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

fn reasoning_encrypted_content_include(_base_url: &str) -> &'static str {
    "reasoning.encryptedcontent"
}

fn request_reasoning_encrypted_content_include() -> bool {
    env_flag("PUFFER_OPENAI_INCLUDE_REASONING_ENCRYPTED_CONTENT")
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

pub(super) fn retry_openai_transport<F, T>(
    mut operation: F,
    mut on_retry: impl FnMut(usize, usize, &str),
) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let attempts = openai_transport_max_attempts();
    let base_delay = openai_transport_retry_base_delay();
    for attempt in 1..=attempts {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt < attempts && is_retryable_openai_transport_error(&error) => {
                let delay = openai_retry_backoff(base_delay, attempt);
                tracing::warn!(
                    target: "puffer::runtime::openai",
                    attempt,
                    max_attempts = attempts,
                    retry_delay_ms = delay.as_millis(),
                    error = %error,
                    "OpenAI transport failed; retrying request"
                );
                on_retry(attempt, attempts, &error.to_string());
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("retry loop always returns or errors")
}

/// Returns true when an OpenAI streaming failure is safe to retry by
/// restarting the whole sampling request.
pub(super) fn is_retryable_openai_stream_error(error: &Error) -> bool {
    is_retryable_openai_transport_error(error) || is_openai_response_incomplete_error(error)
}

fn openai_transport_max_attempts() -> usize {
    std::env::var("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(OPENAI_TRANSPORT_DEFAULT_MAX_ATTEMPTS)
        .clamp(1, OPENAI_RETRY_MAX_ATTEMPTS_CAP)
}

fn openai_transport_retry_base_delay() -> Duration {
    retry_base_delay_ms(&[
        "PUFFER_OPENAI_HTTP_RETRY_BASE_DELAY_MS",
        "PUFFER_OPENAI_HTTP_RETRY_DELAY_MS",
    ])
    .map(Duration::from_millis)
    .unwrap_or(Duration::from_millis(OPENAI_RETRY_DEFAULT_BASE_DELAY_MS))
}

/// Maximum attempts for full OpenAI stream restarts after a dropped SSE body.
pub(super) fn openai_stream_max_attempts() -> usize {
    std::env::var("PUFFER_OPENAI_STREAM_MAX_ATTEMPTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(OPENAI_STREAM_DEFAULT_MAX_ATTEMPTS)
        .clamp(1, OPENAI_RETRY_MAX_ATTEMPTS_CAP)
}

/// Delay before a full OpenAI stream restart attempt.
pub(super) fn openai_stream_retry_delay(attempt: usize) -> Duration {
    openai_retry_backoff(openai_stream_retry_base_delay(), attempt)
}

fn openai_stream_retry_base_delay() -> Duration {
    retry_base_delay_ms(&[
        "PUFFER_OPENAI_STREAM_RETRY_BASE_DELAY_MS",
        "PUFFER_OPENAI_STREAM_RETRY_DELAY_MS",
        "PUFFER_OPENAI_HTTP_RETRY_BASE_DELAY_MS",
        "PUFFER_OPENAI_HTTP_RETRY_DELAY_MS",
    ])
    .map(Duration::from_millis)
    .unwrap_or(Duration::from_millis(OPENAI_RETRY_DEFAULT_BASE_DELAY_MS))
}

fn retry_base_delay_ms(env_names: &[&str]) -> Option<u64> {
    env_names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|value| value.min(OPENAI_RETRY_BASE_DELAY_CAP_MS))
    })
}

fn openai_retry_backoff(base: Duration, attempt: usize) -> Duration {
    if base.is_zero() {
        return Duration::ZERO;
    }
    let exponent = (attempt as u32).saturating_sub(1);
    let factor = 2u128.saturating_pow(exponent);
    let nominal_ms = base
        .as_millis()
        .saturating_mul(factor)
        .min(OPENAI_RETRY_BACKOFF_CAP_MS as u128) as u64;
    let jitter = rand::thread_rng().gen_range(0.9_f64..1.1_f64);
    let jittered_ms = ((nominal_ms as f64 * jitter) as u64).min(OPENAI_RETRY_BACKOFF_CAP_MS);
    Duration::from_millis(jittered_ms)
}

pub(super) fn openai_stream_read_timeout() -> Duration {
    let timeout_ms = std::env::var("PUFFER_OPENAI_STREAM_READ_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(180_000)
        .clamp(1_000, 300_000);
    Duration::from_millis(timeout_ms)
}

pub(super) fn is_openai_include_validation_error(error: &Error) -> bool {
    error.chain().any(|cause| {
        let text = cause.to_string().to_ascii_lowercase();
        text.contains("include[0]")
            && text.contains("invalid")
            && (text.contains("reasoning.encrypted_content")
                || text.contains("reasoning.encryptedcontent")
                || text.contains("rea...ent")
                || text.contains("supported values"))
    })
}

fn is_retryable_openai_transport_error(error: &Error) -> bool {
    if is_retryable_openai_sse_api_error(error) {
        return true;
    }
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

pub(super) fn apply_previous_response_id(body: &mut Value, previous_response_id: Option<&str>) {
    if let Some(previous_response_id) = previous_response_id {
        body["previous_response_id"] = json!(previous_response_id);
    }
}

pub(super) fn openai_supports_response_threading(
    provider: &ProviderDescriptor,
    base_url: &str,
    model: Option<&ModelDescriptor>,
) -> bool {
    if env_flag("PUFFER_OPENAI_DISABLE_RESPONSE_THREADING")
        || env_flag("PUFFER_OPENAI_DISABLE_PREVIOUS_RESPONSE_ID")
    {
        return false;
    }
    if env_flag("PUFFER_OPENAI_ENABLE_CUSTOM_RESPONSE_THREADING") {
        return true;
    }
    if let Some(declared) =
        openai_responses_compat(model).and_then(|c| c.supports_response_threading)
    {
        return declared;
    }
    auto_detect_response_threading(provider, base_url)
}

fn auto_detect_response_threading(provider: &ProviderDescriptor, base_url: &str) -> bool {
    let trimmed = base_url.trim_end_matches('/');
    (provider.id == "openai" && trimmed.contains("api.openai.com"))
        || (trimmed.contains("/api/codex") && !trimmed.contains("chatgpt.com/backend-api"))
}

pub(super) fn openai_responses_path(base_url: &str) -> &'static str {
    auto_detect_responses_path_str(base_url)
}

/// Same as `openai_responses_path` but consults declared model compat
/// before falling back to URL auto-detection. The URL-only path is
/// retained because some helpers (codex-style detection inside
/// `is_codex_openai_provider`) don't have a model handy.
pub(super) fn openai_responses_path_for_model(
    base_url: &str,
    model: Option<&ModelDescriptor>,
) -> &'static str {
    if let Some(declared) = openai_responses_compat(model).and_then(|c| c.responses_path) {
        return match declared {
            ResponsesPath::V1Responses => "/v1/responses",
            ResponsesPath::Responses => "/responses",
        };
    }
    auto_detect_responses_path_str(base_url)
}

fn auto_detect_responses_path_str(base_url: &str) -> &'static str {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.contains("/backend-api") || trimmed.contains("/api/codex") {
        "/responses"
    } else {
        "/v1/responses"
    }
}

fn openai_responses_compat(
    model: Option<&ModelDescriptor>,
) -> Option<&puffer_provider_registry::OpenAiResponsesCompat> {
    model
        .and_then(|m| m.compat.as_ref())
        .and_then(ModelCompat::as_openai_responses)
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
    model: Option<&ModelDescriptor>,
) {
    let send_version = openai_responses_compat(model)
        .and_then(|c| c.send_codex_version_header)
        .unwrap_or_else(|| provider_id == "openai");
    if send_version && !has_header(headers, "version") {
        headers.push((
            "version".to_string(),
            OPENAI_CODEX_COMPAT_VERSION.to_string(),
        ));
    }
    append_env_header(headers, "OpenAI-Organization", "OPENAI_ORGANIZATION");
    append_env_header(headers, "OpenAI-Project", "OPENAI_PROJECT");
}

pub(super) fn is_codex_openai_provider(provider: &ProviderDescriptor) -> bool {
    is_codex_openai_provider_for_model(provider, None)
}

pub(super) fn is_codex_openai_provider_for_model(
    provider: &ProviderDescriptor,
    model: Option<&ModelDescriptor>,
) -> bool {
    if let Some(declared) = openai_responses_compat(model).and_then(|c| c.codex_style) {
        return declared;
    }
    auto_detect_codex_style(provider)
}

fn auto_detect_codex_style(provider: &ProviderDescriptor) -> bool {
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
    openai_base_url_for_auth_with_model(provider, oauth, None)
}

pub(super) fn openai_base_url_for_auth_with_model(
    provider: &ProviderDescriptor,
    oauth: bool,
    model: Option<&ModelDescriptor>,
) -> String {
    if !oauth {
        return provider.base_url.clone();
    }
    if let Some(declared) = openai_responses_compat(model).and_then(|c| c.oauth_base_url.clone()) {
        return declared;
    }
    if provider.id != "openai" {
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

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
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

// NOTE: openai_input_items, assistant_output_items, extend_input_with_response_items,
// and extend_input_with_continuation have been replaced by the unified
// ConversationItem pipeline in conversation.rs.

// NOTE: compact_openai_input, compact_openai_chat_messages, generate_openai_summary,
// and build_items_summary_text have been replaced by the unified
// compact_conversation and generate_summary in conversation.rs.

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
    // Fast mode only controls service_tier (set in the caller), not reasoning.
    // This mirrors the Anthropic path where fast_mode sets body["speed"] but
    // does not disable thinking.  The effort_level is the sole control for
    // reasoning depth.
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
    use super::build_codex_openai_request_body;
    use super::build_codex_openai_request_body_with_reasoning_include;
    use super::is_openai_include_validation_error;
    use super::is_retryable_openai_transport_error;
    use super::openai_retry_backoff;
    use super::openai_stream_max_attempts;
    use super::openai_stream_retry_base_delay;
    use super::openai_supports_response_threading;
    use super::openai_transport_max_attempts;
    use super::openai_transport_retry_base_delay;
    use crate::runtime::openai_sse::parse_openai_sse_response;
    use crate::runtime::tests::state;
    use crate::runtime::OPENAI_CHATGPT_BASE_URL;
    use anyhow::anyhow;
    use puffer_provider_registry::ProviderDescriptor;
    use serde_json::{json, Value};
    use std::ffi::OsString;
    use std::time::Duration;

    struct ScopedEnvVar {
        name: &'static str,
        old_value: Option<OsString>,
    }

    impl ScopedEnvVar {
        fn set(name: &'static str, value: &str) -> Self {
            let old_value = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, old_value }
        }

        fn unset(name: &'static str) -> Self {
            let old_value = std::env::var_os(name);
            std::env::remove_var(name);
            Self { name, old_value }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            if let Some(value) = self.old_value.take() {
                std::env::set_var(self.name, value);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    fn provider(id: &str, base_url: &str) -> ProviderDescriptor {
        ProviderDescriptor {
            id: id.to_string(),
            display_name: id.to_string(),
            base_url: base_url.to_string(),
            default_api: "openai-responses".to_string(),
            auth_modes: Vec::new(),
            headers: Default::default(),
            query_params: Default::default(),
            discovery: None,
            models: Vec::new(),
            chat_completions_path: None,
        }
    }

    #[test]
    fn openai_retry_defaults_match_codex_retry_budgets() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _vars = vec![
            ScopedEnvVar::unset("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS"),
            ScopedEnvVar::unset("PUFFER_OPENAI_STREAM_MAX_ATTEMPTS"),
            ScopedEnvVar::unset("PUFFER_OPENAI_HTTP_RETRY_BASE_DELAY_MS"),
            ScopedEnvVar::unset("PUFFER_OPENAI_HTTP_RETRY_DELAY_MS"),
            ScopedEnvVar::unset("PUFFER_OPENAI_STREAM_RETRY_BASE_DELAY_MS"),
            ScopedEnvVar::unset("PUFFER_OPENAI_STREAM_RETRY_DELAY_MS"),
        ];

        assert_eq!(openai_transport_max_attempts(), 5);
        assert_eq!(openai_stream_max_attempts(), 6);
        assert_eq!(
            openai_transport_retry_base_delay(),
            Duration::from_millis(200)
        );
        assert_eq!(openai_stream_retry_base_delay(), Duration::from_millis(200));
    }

    #[test]
    fn openai_retry_env_values_clamp_attempts_and_base_delay() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _http_attempts = ScopedEnvVar::set("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS", "999");
        let _stream_attempts = ScopedEnvVar::set("PUFFER_OPENAI_STREAM_MAX_ATTEMPTS", "0");
        let _http_delay = ScopedEnvVar::set("PUFFER_OPENAI_HTTP_RETRY_BASE_DELAY_MS", "999999");

        assert_eq!(openai_transport_max_attempts(), 101);
        assert_eq!(openai_stream_max_attempts(), 1);
        assert_eq!(
            openai_transport_retry_base_delay(),
            Duration::from_millis(10_000)
        );
    }

    #[test]
    fn openai_stream_retry_delay_aliases_preserve_legacy_env() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _stream_base = ScopedEnvVar::unset("PUFFER_OPENAI_STREAM_RETRY_BASE_DELAY_MS");
        let _stream_delay = ScopedEnvVar::set("PUFFER_OPENAI_STREAM_RETRY_DELAY_MS", "400");
        let _http_base = ScopedEnvVar::set("PUFFER_OPENAI_HTTP_RETRY_BASE_DELAY_MS", "800");
        let _http_delay = ScopedEnvVar::set("PUFFER_OPENAI_HTTP_RETRY_DELAY_MS", "900");

        assert_eq!(openai_stream_retry_base_delay(), Duration::from_millis(400));
    }

    #[test]
    fn openai_stream_retry_base_delay_falls_back_to_http_base_delay() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _stream_base = ScopedEnvVar::unset("PUFFER_OPENAI_STREAM_RETRY_BASE_DELAY_MS");
        let _stream_delay = ScopedEnvVar::unset("PUFFER_OPENAI_STREAM_RETRY_DELAY_MS");
        let _http_base = ScopedEnvVar::set("PUFFER_OPENAI_HTTP_RETRY_BASE_DELAY_MS", "350");
        let _http_delay = ScopedEnvVar::set("PUFFER_OPENAI_HTTP_RETRY_DELAY_MS", "900");

        assert_eq!(openai_stream_retry_base_delay(), Duration::from_millis(350));
    }

    #[test]
    fn openai_retry_backoff_scales_with_jitter_and_caps() {
        let first = openai_retry_backoff(Duration::from_millis(200), 1);
        assert!(first >= Duration::from_millis(180), "{first:?}");
        assert!(first <= Duration::from_millis(220), "{first:?}");

        let second = openai_retry_backoff(Duration::from_millis(200), 2);
        assert!(second >= Duration::from_millis(360), "{second:?}");
        assert!(second <= Duration::from_millis(440), "{second:?}");
        assert_eq!(openai_retry_backoff(Duration::ZERO, 10), Duration::ZERO);
        let capped = openai_retry_backoff(Duration::from_millis(10_000), 20);
        assert!(capped <= Duration::from_millis(60_000), "{capped:?}");
    }

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
    fn retries_openai_sse_server_errors() {
        let stream = concat!(
            "event: response.failed\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"try again\"}}}\n\n"
        );
        let error = parse_openai_sse_response(stream)
            .unwrap_err()
            .context("failed to parse SSE response from http://example.test/v1/responses");

        assert!(is_retryable_openai_transport_error(&error));
    }

    #[test]
    fn detects_openai_include_validation_errors() {
        let error = anyhow!(
            "request failed with status 400 Bad Request: {{\"error\":{{\"message\":\"Invalid value: 'rea...ent'. Supported values are: 'reasoning.encryptedcontent'.\",\"param\":\"include[0]\"}}}}"
        );

        assert!(is_openai_include_validation_error(&error));
        assert!(!is_openai_include_validation_error(&anyhow!(
            "request failed with status 400 Bad Request: invalid model"
        )));
    }

    #[test]
    fn request_body_uses_prompt_cache_key_override_when_present() {
        let mut state = state();
        state.prompt_cache_key_override = Some("benchmark-cache-key".to_string());

        let body = build_codex_openai_request_body(
            &state,
            "https://api.openai.com",
            "gpt-5",
            "instructions",
            Value::String("hello".to_string()),
            &Vec::new(),
            true,
            None,
            true,
        );

        assert_eq!(body["prompt_cache_key"], json!("benchmark-cache-key"));
    }

    #[test]
    fn request_body_omits_reasoning_include_by_default() {
        let state = state();

        let body = build_codex_openai_request_body_with_reasoning_include(
            &state,
            "https://api.openai.com",
            "gpt-5",
            "instructions",
            Value::String("hello".to_string()),
            &Vec::new(),
            true,
            None,
            true,
            false,
        );

        assert!(body["reasoning"].is_object(), "body: {body}");
        assert_eq!(body["include"], json!([]), "body: {body}");
    }

    #[test]
    fn request_body_omits_tool_fields_without_tools() {
        let state = state();

        let body = build_codex_openai_request_body(
            &state,
            "https://api.openai.com",
            "gpt-5",
            "instructions",
            Value::String("hello".to_string()),
            &Vec::new(),
            false,
            None,
            true,
        );

        assert!(body.get("tools").is_none(), "body: {body}");
        assert!(body.get("tool_choice").is_none(), "body: {body}");
        assert!(body.get("parallel_tool_calls").is_none(), "body: {body}");
    }

    #[test]
    fn request_body_can_opt_into_encrypted_reasoning_include() {
        let state = state();

        let body = build_codex_openai_request_body_with_reasoning_include(
            &state,
            OPENAI_CHATGPT_BASE_URL,
            "gpt-5",
            "instructions",
            Value::String("hello".to_string()),
            &Vec::new(),
            true,
            None,
            true,
            true,
        );

        assert_eq!(body["include"][0], json!("reasoning.encryptedcontent"));
    }

    #[test]
    fn request_body_omits_unsupported_web_search_sources_include() {
        use puffer_provider_openai::OpenAIResponsesTool;
        let state = state();
        let tools = vec![OpenAIResponsesTool {
            kind: "web_search".to_string(),
            name: String::new(),
            description: String::new(),
            strict: false,
            parameters: Value::Null,
            filters: None,
            user_location: None,
            external_web_access: None,
        }];

        let body = build_codex_openai_request_body(
            &state,
            "https://api.openai.com",
            "gpt-5",
            "instructions",
            Value::String("hello".to_string()),
            &tools,
            false,
            None,
            true,
        );

        let include = body["include"].as_array().expect("include array");
        assert!(!include.contains(&json!("web_search_call.action.sources")));
    }

    #[test]
    fn request_body_omits_web_search_sources_when_no_native_tool() {
        let state = state();
        let body = build_codex_openai_request_body(
            &state,
            "https://api.openai.com",
            "gpt-5",
            "instructions",
            Value::String("hello".to_string()),
            &Vec::new(),
            false,
            None,
            true,
        );
        let include = body["include"].as_array().expect("include array");
        assert!(!include.contains(&json!("web_search_call.action.sources")));
    }

    #[test]
    fn official_openai_endpoints_keep_response_threading_enabled() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _disable = ScopedEnvVar::unset("PUFFER_OPENAI_DISABLE_RESPONSE_THREADING");
        let _legacy_disable = ScopedEnvVar::unset("PUFFER_OPENAI_DISABLE_PREVIOUS_RESPONSE_ID");
        let _force_enable = ScopedEnvVar::unset("PUFFER_OPENAI_ENABLE_CUSTOM_RESPONSE_THREADING");
        let provider = provider("openai", "https://api.openai.com");

        assert!(openai_supports_response_threading(
            &provider,
            "https://api.openai.com",
            None,
        ));
    }

    #[test]
    fn custom_openai_proxy_disables_response_threading_by_default() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _disable = ScopedEnvVar::unset("PUFFER_OPENAI_DISABLE_RESPONSE_THREADING");
        let _legacy_disable = ScopedEnvVar::unset("PUFFER_OPENAI_DISABLE_PREVIOUS_RESPONSE_ID");
        let _force_enable = ScopedEnvVar::unset("PUFFER_OPENAI_ENABLE_CUSTOM_RESPONSE_THREADING");
        let provider = provider("openai", "http://84.32.32.146:8317/v1");

        assert!(!openai_supports_response_threading(
            &provider,
            "http://84.32.32.146:8317/v1",
            None,
        ));
    }

    #[test]
    fn custom_openai_proxy_can_opt_back_into_response_threading() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _disable = ScopedEnvVar::unset("PUFFER_OPENAI_DISABLE_RESPONSE_THREADING");
        let _legacy_disable = ScopedEnvVar::unset("PUFFER_OPENAI_DISABLE_PREVIOUS_RESPONSE_ID");
        let _force_enable =
            ScopedEnvVar::set("PUFFER_OPENAI_ENABLE_CUSTOM_RESPONSE_THREADING", "true");
        let provider = provider("openai", "http://84.32.32.146:8317/v1");

        assert!(openai_supports_response_threading(
            &provider,
            "http://84.32.32.146:8317/v1",
            None,
        ));
    }

    #[test]
    fn chatgpt_codex_backend_disables_response_threading_by_default() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _disable = ScopedEnvVar::unset("PUFFER_OPENAI_DISABLE_RESPONSE_THREADING");
        let _legacy_disable = ScopedEnvVar::unset("PUFFER_OPENAI_DISABLE_PREVIOUS_RESPONSE_ID");
        let _force_enable = ScopedEnvVar::unset("PUFFER_OPENAI_ENABLE_CUSTOM_RESPONSE_THREADING");
        let provider = provider("openai", "https://api.openai.com");

        assert!(!openai_supports_response_threading(
            &provider,
            "https://chatgpt.com/backend-api/codex",
            None,
        ));
    }
}
