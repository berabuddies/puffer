use anyhow::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Carries provider failure facts in a stable shape for media tooling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaFailureDiagnostic {
    pub kind: String,
    #[serde(rename = "provider")]
    pub provider_id: String,
    pub adapter: Option<String>,
    #[serde(rename = "model")]
    pub model_id: Option<String>,
    pub phase: Option<String>,
    pub provider_job_id: Option<String>,
    pub remote_status: Option<String>,
    pub http_status: Option<u16>,
    pub provider_code: Option<String>,
    pub request_id: Option<String>,
    pub error: String,
    pub hint: Option<String>,
}

/// Carries media context used to build a failure diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFailureContext {
    kind: String,
    provider_id: String,
    adapter: Option<String>,
    model_id: Option<String>,
    phase: Option<String>,
    provider_job_id: Option<String>,
    remote_status: Option<String>,
}

impl MediaFailureContext {
    /// Creates a diagnostic context for one media provider.
    pub fn new(kind: impl Into<String>, provider_id: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            provider_id: provider_id.into(),
            adapter: None,
            model_id: None,
            phase: None,
            provider_job_id: None,
            remote_status: None,
        }
    }

    /// Adds an adapter id.
    pub fn adapter(mut self, adapter: impl Into<String>) -> Self {
        self.adapter = Some(adapter.into());
        self
    }

    /// Adds a model id.
    pub fn model(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }

    /// Adds a phase label.
    pub fn phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = Some(phase.into());
        self
    }

    /// Adds a provider job id.
    pub fn provider_job_id(mut self, provider_job_id: impl Into<String>) -> Self {
        self.provider_job_id = Some(provider_job_id.into());
        self
    }

    /// Adds a remote status.
    pub fn remote_status(mut self, remote_status: impl Into<String>) -> Self {
        self.remote_status = Some(remote_status.into());
        self
    }
}

/// Captures an HTTP provider error before provider context is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHttpError {
    pub label: String,
    pub http_status: Option<u16>,
    pub body: String,
    pub provider_code: Option<String>,
    pub provider_message: Option<String>,
    pub request_id: Option<String>,
}

impl ProviderHttpError {
    /// Creates an HTTP provider error and extracts common JSON fields.
    pub fn new(label: impl Into<String>, http_status: u16, body: impl Into<String>) -> Self {
        let label = label.into();
        let body = body.into();
        let parsed = serde_json::from_str::<Value>(&body).ok();
        Self {
            label,
            http_status: Some(http_status),
            provider_code: parsed.as_ref().and_then(provider_code),
            provider_message: parsed.as_ref().and_then(provider_message),
            request_id: parsed.as_ref().and_then(request_id),
            body,
        }
    }
}

impl fmt::Display for ProviderHttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.http_status {
            Some(status) => write!(
                formatter,
                "{} failed with status {}: {}",
                self.label, status, self.body
            ),
            None => write!(formatter, "{} failed: {}", self.label, self.body),
        }
    }
}

impl std::error::Error for ProviderHttpError {}

/// Error wrapper that preserves a structured media failure diagnostic.
#[derive(Debug, Clone)]
pub struct MediaFailureError {
    diagnostic: MediaFailureDiagnostic,
}

impl MediaFailureError {
    /// Creates a media failure error from a diagnostic.
    pub fn new(diagnostic: MediaFailureDiagnostic) -> Self {
        Self { diagnostic }
    }

    /// Returns the carried diagnostic.
    pub fn diagnostic(&self) -> &MediaFailureDiagnostic {
        &self.diagnostic
    }
}

impl fmt::Display for MediaFailureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.diagnostic.error)
    }
}

impl std::error::Error for MediaFailureError {}

impl MediaFailureDiagnostic {
    /// Builds a diagnostic from a provider HTTP error.
    pub fn from_http_error(context: MediaFailureContext, error: ProviderHttpError) -> Self {
        let message = match error.provider_message.clone() {
            Some(message) => format!("{}: {message}", error.label),
            None => error.to_string(),
        };
        let mut diagnostic = Self::from_parts(
            context,
            error.http_status,
            error.provider_code,
            error.request_id,
            message,
        );
        diagnostic.hint = diagnostic_hint(&diagnostic);
        diagnostic
    }

    /// Builds a diagnostic from an arbitrary error.
    pub fn from_error(context: MediaFailureContext, error: &Error) -> Self {
        Self::from_message(context, format!("{error:#}"))
    }

    /// Builds a diagnostic from a plain error message.
    pub(crate) fn from_message(context: MediaFailureContext, message: impl Into<String>) -> Self {
        let mut diagnostic = Self::from_parts(context, None, None, None, message);
        diagnostic.hint = diagnostic_hint(&diagnostic);
        diagnostic
    }

    /// Redacts known secrets from diagnostic text fields.
    pub fn redact(mut self, secrets: &[String]) -> Self {
        self.error = redact_text(&self.error, secrets);
        self.provider_code = self.provider_code.map(|value| redact_text(&value, secrets));
        self.request_id = self.request_id.map(|value| redact_text(&value, secrets));
        self.hint = self.hint.map(|value| redact_text(&value, secrets));
        self
    }

    fn from_parts(
        context: MediaFailureContext,
        http_status: Option<u16>,
        provider_code: Option<String>,
        request_id: Option<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            kind: context.kind,
            provider_id: context.provider_id,
            adapter: context.adapter,
            model_id: context.model_id,
            phase: context.phase,
            provider_job_id: context.provider_job_id,
            remote_status: context.remote_status,
            http_status,
            provider_code,
            request_id,
            error: error.into(),
            hint: None,
        }
    }
}

/// Converts an anyhow error into a diagnostic-carrying error.
pub fn media_failure_error(context: MediaFailureContext, error: Error) -> Error {
    let mut http_error = None;
    let mut media_error = None;
    for cause in error.chain() {
        if http_error.is_none() {
            http_error = cause.downcast_ref::<ProviderHttpError>().cloned();
        }
        if media_error.is_none() {
            media_error = cause
                .downcast_ref::<MediaFailureError>()
                .map(|failure| failure.diagnostic().clone());
        }
    }
    let diagnostic = http_error
        .map(|http| MediaFailureDiagnostic::from_http_error(context.clone(), http))
        .or(media_error)
        .unwrap_or_else(|| MediaFailureDiagnostic::from_error(context, &error));
    Error::new(MediaFailureError::new(diagnostic))
}

/// Returns the structured media diagnostic carried by an error, if present.
pub fn media_failure_diagnostic(error: &Error) -> Option<MediaFailureDiagnostic> {
    error
        .downcast_ref::<MediaFailureError>()
        .map(|error| error.diagnostic().clone())
}

fn provider_code(value: &Value) -> Option<String> {
    text_at(value, &["error", "code"])
        .or_else(|| text_at(value, &["code"]))
        .or_else(|| text_at(value, &["type"]))
}

fn provider_message(value: &Value) -> Option<String> {
    text_at(value, &["error", "message"])
        .or_else(|| text_at(value, &["message"]))
        .or_else(|| text_at(value, &["failure_reason"]))
        .or_else(|| text_at(value, &["fail_reason"]))
        .or_else(|| text_at(value, &["reason"]))
}

fn request_id(value: &Value) -> Option<String> {
    text_at(value, &["requestId"])
        .or_else(|| text_at(value, &["request_id"]))
        .or_else(|| text_at(value, &["error", "request_id"]))
        .or_else(|| text_at(value, &["error", "requestId"]))
}

fn text_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn diagnostic_hint(diagnostic: &MediaFailureDiagnostic) -> Option<String> {
    let provider = diagnostic.provider_id.as_str();
    let code = diagnostic.provider_code.as_deref().unwrap_or("");
    let message = diagnostic.error.to_ascii_lowercase();
    if provider == "worldrouter" && code == "seedance_balance_too_low" {
        return Some(
            "WorldRouter Seedance credits appear to be too low; check team credits.".to_string(),
        );
    }
    if provider == "worldrouter" && code == "seedance_too_many_pending_tasks" {
        return Some(
            "WorldRouter has too many pending Seedance tasks; wait for active jobs to finish."
                .to_string(),
        );
    }
    if provider == "worldrouter" && code == "unsupported_model" {
        return Some("WorldRouter rejected the model; use seedance-2.0 or seedance-2.0-fast on the Seedance task endpoint.".to_string());
    }
    if provider == "worldrouter" && message.contains("upload assets first") {
        return Some(
            "WorldRouter image references must be uploaded through asset helpers before generation."
                .to_string(),
        );
    }
    if provider == "byteplus" && (message.contains("sensitive") || message.contains("moderation")) {
        return Some(
            "BytePlus rejected the generated media or references; revise the prompt or inputs."
                .to_string(),
        );
    }
    if provider == "relaydance" && (message.contains("copyright") || message.contains("sensitive"))
    {
        return Some(
            "Relaydance rejected the generated media or references; revise the prompt or inputs."
                .to_string(),
        );
    }
    match diagnostic.http_status {
        Some(401) | Some(403) => {
            Some("Provider credentials or permissions appear invalid.".to_string())
        }
        Some(402) => {
            Some("Provider account may not have enough media-generation credits.".to_string())
        }
        Some(408) => Some("Provider or network timed out; retry later.".to_string()),
        Some(429) => Some("Provider rate limit or pending-task limit was reached; wait before retrying.".to_string()),
        Some(400) => Some(
            "Provider rejected the request; check model, parameters, endpoint, and media references."
                .to_string(),
        ),
        Some(status) if status >= 500 => Some(
            "Provider or upstream service returned an internal error; retry later or compare another provider."
                .to_string(),
        ),
        _ if diagnostic.phase.as_deref() == Some("download") => {
            Some("Provider task completed, but artifact download failed.".to_string())
        }
        _ if diagnostic.phase.as_deref() == Some("persist") => {
            Some("Provider task completed, but local artifact persistence failed.".to_string())
        }
        _ => None,
    }
}

fn redact_text(text: &str, secrets: &[String]) -> String {
    secrets.iter().fold(text.to_string(), |redacted, secret| {
        let secret = secret.trim();
        if secret.is_empty() {
            redacted
        } else {
            redacted.replace(secret, "[redacted]")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_error_extracts_nested_provider_facts() {
        let error = ProviderHttpError::new(
            "submit video task",
            402,
            r#"{"error":{"code":"seedance_balance_too_low","message":"low credits","request_id":"req-1"}}"#,
        );

        assert_eq!(error.http_status, Some(402));
        assert_eq!(
            error.provider_code.as_deref(),
            Some("seedance_balance_too_low")
        );
        assert_eq!(error.provider_message.as_deref(), Some("low credits"));
        assert_eq!(error.request_id.as_deref(), Some("req-1"));
    }

    #[test]
    fn diagnostic_hint_covers_worldrouter_seedance_balance() {
        let diagnostic = MediaFailureDiagnostic::from_http_error(
            MediaFailureContext::new("video", "worldrouter")
                .adapter("worldrouter_video")
                .model("seedance-2.0-fast")
                .phase("submit"),
            ProviderHttpError::new(
                "submit WorldRouter video task",
                402,
                r#"{"error":{"code":"seedance_balance_too_low","message":"top up"}}"#,
            ),
        );

        assert_eq!(diagnostic.provider_id, "worldrouter");
        assert_eq!(diagnostic.phase.as_deref(), Some("submit"));
        assert_eq!(diagnostic.http_status, Some(402));
        assert_eq!(
            diagnostic.provider_code.as_deref(),
            Some("seedance_balance_too_low")
        );
        assert_eq!(
            diagnostic.hint.as_deref(),
            Some("WorldRouter Seedance credits appear to be too low; check team credits.")
        );
    }

    #[test]
    fn http_error_without_provider_message_does_not_duplicate_label() {
        let diagnostic = MediaFailureDiagnostic::from_http_error(
            MediaFailureContext::new("video", "byteplus")
                .adapter("byteplus_video")
                .model("dreamina-seedance-2-0-fast-260128")
                .phase("submit"),
            ProviderHttpError::new("submit video task", 500, "upstream unavailable"),
        );

        assert_eq!(
            diagnostic.error,
            "submit video task failed with status 500: upstream unavailable"
        );
        assert!(!diagnostic
            .error
            .contains("submit video task: submit video task"));
    }

    #[test]
    fn diagnostic_from_message_builds_plain_failed_job_diagnostic() {
        let diagnostic = MediaFailureDiagnostic::from_message(
            MediaFailureContext::new("video", "relaydance")
                .adapter("relaydance_video")
                .model("doubao-seedance-2-0-720p")
                .phase("poll")
                .provider_job_id("rd-task-1")
                .remote_status("failed"),
            "content blocked upstream",
        );

        assert_eq!(diagnostic.provider_id, "relaydance");
        assert_eq!(diagnostic.provider_job_id.as_deref(), Some("rd-task-1"));
        assert_eq!(diagnostic.remote_status.as_deref(), Some("failed"));
        assert_eq!(diagnostic.error, "content blocked upstream");
    }

    #[test]
    fn redaction_removes_secret_from_error_and_hint_inputs() {
        let diagnostic = MediaFailureDiagnostic::from_http_error(
            MediaFailureContext::new("video", "byteplus")
                .adapter("byteplus_video")
                .model("dreamina-seedance-2-0-fast-260128")
                .phase("submit"),
            ProviderHttpError::new(
                "submit video task",
                500,
                r#"{"error":{"message":"upstream leaked sk-secret-token"}}"#,
            ),
        )
        .redact(&["sk-secret-token".to_string()]);

        assert!(!diagnostic.error.contains("sk-secret-token"));
        assert!(diagnostic.error.contains("[redacted]"));
    }
}
