use serde_json::Value;
use std::{error::Error as StdError, fmt};

#[derive(Debug)]
pub(crate) struct OpenAIStatusError {
    pub(crate) status: reqwest::StatusCode,
    pub(crate) body: String,
}

impl fmt::Display for OpenAIStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OpenAI request failed with status {}: body_bytes={} body_preview={}",
            self.status,
            self.body.len(),
            bounded_body_preview(&self.body)
        )
    }
}

impl StdError for OpenAIStatusError {}

#[derive(Debug)]
pub(crate) struct OpenAIWallTimeoutError {
    pub(crate) timeout_secs: u64,
}

impl fmt::Display for OpenAIWallTimeoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OpenAI request exceeded wall timeout of {} seconds",
            self.timeout_secs
        )
    }
}

impl StdError for OpenAIWallTimeoutError {}

#[derive(Debug)]
pub(crate) struct OpenAINoStructuralAssistantContent {
    pub(crate) endpoint_kind: &'static str,
    pub(crate) finish_reason: Option<String>,
    pub(crate) message_keys: Vec<String>,
}

impl fmt::Display for OpenAINoStructuralAssistantContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let finish_reason = self.finish_reason.as_deref().unwrap_or("unknown");
        let message_keys = if self.message_keys.is_empty() {
            "none".to_string()
        } else {
            self.message_keys.join(",")
        };
        write!(
            formatter,
            "OpenAI response contained no structural assistant content (endpoint_kind={}, finish_reason={}, message_keys={})",
            self.endpoint_kind, finish_reason, message_keys
        )
    }
}

impl StdError for OpenAINoStructuralAssistantContent {}

/// Returns whether an OpenAI provider error is worth retrying.
pub(crate) fn retryable_openai_error(error: &anyhow::Error) -> bool {
    if assistant_content_missing(error) {
        return true;
    }
    error.chain().any(|cause| {
        cause
            .downcast_ref::<OpenAIStatusError>()
            .is_some_and(|error| retryable_openai_status(error.status))
            || cause.downcast_ref::<OpenAIWallTimeoutError>().is_some()
            || cause
                .downcast_ref::<reqwest::Error>()
                .is_some_and(retryable_reqwest_error)
    })
}

/// Returns whether a provider response lacked structural assistant content.
pub(crate) fn assistant_content_missing(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<OpenAINoStructuralAssistantContent>()
            .is_some()
    })
}

/// Returns whether the provider stopped before final content due to output budget.
pub(crate) fn assistant_content_stopped_by_length(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<OpenAINoStructuralAssistantContent>()
            .and_then(|error| error.finish_reason.as_deref())
            == Some("length")
    })
}

/// Extracts a typed HTTP status from an OpenAI provider error.
pub(crate) fn openai_error_status(error: &anyhow::Error) -> Option<reqwest::StatusCode> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<OpenAIStatusError>()
            .map(|error| error.status)
            .or_else(|| {
                cause
                    .downcast_ref::<reqwest::Error>()
                    .and_then(reqwest::Error::status)
            })
    })
}

/// Returns whether a structured provider error field matches an expected value.
pub(crate) fn response_error_field_equals(body: &str, field: &str, expected: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    parsed
        .get("error")
        .and_then(|error| error.get(field))
        .or_else(|| parsed.get(field))
        .and_then(Value::as_str)
        .is_some_and(|value| value == expected)
}

fn retryable_reqwest_error(error: &reqwest::Error) -> bool {
    error.status().is_some_and(retryable_openai_status)
        || error.is_timeout()
        || error.is_connect()
        || error.is_body()
        || error.is_decode()
        || error.is_request()
}

fn retryable_openai_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::REQUEST_TIMEOUT
            | reqwest::StatusCode::TOO_MANY_REQUESTS
            | reqwest::StatusCode::INTERNAL_SERVER_ERROR
            | reqwest::StatusCode::BAD_GATEWAY
            | reqwest::StatusCode::SERVICE_UNAVAILABLE
            | reqwest::StatusCode::GATEWAY_TIMEOUT
    )
}

fn bounded_body_preview(body: &str) -> String {
    const MAX_PREVIEW_CHARS: usize = 512;
    let mut preview = body.chars().take(MAX_PREVIEW_CHARS).collect::<String>();
    if body.chars().count() > MAX_PREVIEW_CHARS {
        preview.push_str("...");
    }
    preview.replace(['\n', '\r'], "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_error_display_bounds_provider_body() {
        let error = OpenAIStatusError {
            status: reqwest::StatusCode::BAD_REQUEST,
            body: format!("{}SECRET", "x".repeat(2048)),
        };
        let display = error.to_string();

        assert!(display.contains("body_bytes=2054"));
        assert!(display.len() < 700);
        assert!(!display.contains("SECRET"));
    }
}
