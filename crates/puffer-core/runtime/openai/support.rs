use super::super::{OPENAI_CHATGPT_BASE_URL, OPENAI_CODEX_COMPAT_VERSION};
use super::StructuredOutputConfig;
use crate::AppState;
use anyhow::{Error, Result};
use puffer_provider_openai::{OpenAIResponsesTextConfig, OpenAIResponsesTool};
use puffer_provider_registry::{
    ModelCompat, ModelDescriptor, OAuthCredential, ProviderDescriptor, ResponsesPath,
};
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
    let mut include: Vec<Value> = Vec::new();
    if reasoning.is_some() {
        include.push(json!("reasoning.encrypted_content"));
    }
    if tools.iter().any(|tool| tool.kind == "web_search") {
        include.push(json!("web_search_call.action.sources"));
    }
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
        "prompt_cache_key": state
            .prompt_cache_key_override
            .clone()
            .unwrap_or_else(|| state.session.id.to_string()),
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

pub(super) fn retry_openai_transport<F, T>(
    mut operation: F,
    mut on_retry: impl FnMut(usize, usize, &str),
) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let attempts = openai_transport_max_attempts();
    let delay = openai_transport_retry_delay();
    for attempt in 1..=attempts {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt < attempts && is_retryable_openai_transport_error(&error) => {
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
        .unwrap_or(180_000)
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
    use super::is_retryable_openai_transport_error;
    use super::openai_supports_response_threading;
    use crate::runtime::tests::state;
    use anyhow::anyhow;
    use puffer_provider_registry::ProviderDescriptor;
    use serde_json::{json, Value};
    use std::ffi::OsString;

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
    fn request_body_uses_prompt_cache_key_override_when_present() {
        let mut state = state();
        state.prompt_cache_key_override = Some("benchmark-cache-key".to_string());

        let body = build_codex_openai_request_body(
            &state,
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
    fn request_body_includes_web_search_sources_when_native_tool_present() {
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
            "gpt-5",
            "instructions",
            Value::String("hello".to_string()),
            &tools,
            false,
            None,
            true,
        );

        let include = body["include"].as_array().expect("include array");
        assert!(include.contains(&json!("web_search_call.action.sources")));
    }

    #[test]
    fn request_body_omits_web_search_sources_when_no_native_tool() {
        let state = state();
        let body = build_codex_openai_request_body(
            &state,
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
