//! Typed error for provider quota exhaustion.
//!
//! Without this layer every 429 / quota-403 is just an `anyhow::Error`
//! whose message string mentions the status code. Downstream
//! orchestration (`run_tb2.py`, `puffer_harbor_agent.py`) sees only
//! the generic non-zero exit and burns its retry budget back-to-back
//! against a quota window that won't recover for minutes (or hours).
//!
//! On `kimi-v16-full89` (2026-04-21) trajectory analysis found 4 of 5
//! sampled "unsolved" tasks were quota-cascade deaths, not capability
//! failures — each wasted ~3 retries. With 20–40 unsolved × 3, that
//! cost hours of wall-clock and hid real failure modes in the final
//! summary.
//!
//! This module defines `QuotaError`. Provider adapters
//! (`openai.rs`, `anthropic.rs`) detect 429 / 403-access-terminated
//! at HTTP-response inspection sites and return `QuotaError` wrapped
//! in `anyhow::Error::new(...)`. The `benchmark-run` CLI command
//! downcasts on the error path, stamps `error_kind` in `result.json`,
//! and exits with a distinct code so the orchestration layer can
//! delay the next retry instead of burning the budget.

use std::fmt;

/// What kind of quota signal the provider returned. Different
/// recovery cadences in practice — `RateLimit` typically clears
/// within a minute; `AccessTerminated` means the day's / period's
/// budget is gone and recovery is measured in hours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaErrorKind {
    /// HTTP 429. Either the provider's vanilla rate-limit or
    /// `rate_limit_reached_error` body. Recovers in seconds-to-minutes.
    RateLimit,
    /// HTTP 403 with an `access_terminated_error` body (Kimi /
    /// kimi-coding signature when the period quota is gone). Recovery
    /// is measured in hours; orchestration should down-prioritize
    /// retrying the same model and prefer to skip ahead.
    AccessTerminated,
}

impl QuotaErrorKind {
    /// Tag used in `result.json` and exit-code mapping.
    pub fn slug(self) -> &'static str {
        match self {
            Self::RateLimit => "quota_rate_limit",
            Self::AccessTerminated => "quota_access_terminated",
        }
    }
}

/// Provider-quota signal carrying enough context for orchestration
/// to make a delay decision without re-parsing the wire body.
#[derive(Debug, Clone)]
pub struct QuotaError {
    pub kind: QuotaErrorKind,
    pub status: u16,
    pub provider: String,
    pub body: String,
}

impl fmt::Display for QuotaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} quota exhausted ({} status {}): {}",
            self.provider,
            self.kind.slug(),
            self.status,
            self.body
        )
    }
}

impl std::error::Error for QuotaError {}

/// Distinct process exit code so `puffer_harbor_agent.py` and
/// `run_tb2.py` can detect quota deaths via `wait_status >> 8` (or
/// `subprocess.returncode`) without parsing stderr.
///
/// 3 was picked deliberately over the conventional 2 (anyhow's bail
/// path uses 1; 2 is reserved by clap for arg-parse failures).
pub const QUOTA_EXIT_CODE: i32 = 3;

/// Inspect an HTTP status + response body and classify the failure.
/// Returns `Some(QuotaError)` when this is unambiguously a quota
/// signal; `None` for anything else (the caller should fall back to
/// its existing `bail!` path).
///
/// This intentionally does not allocate when the status is success —
/// the caller is expected to short-circuit on `status.is_success()`
/// before calling here.
pub fn classify_response(provider: &str, status: u16, body: &str) -> Option<QuotaError> {
    match status {
        429 => Some(QuotaError {
            kind: QuotaErrorKind::RateLimit,
            status,
            provider: provider.to_string(),
            body: body.to_string(),
        }),
        403 if body.contains("access_terminated_error")
            || body.contains("usage limit reached for this period") =>
        {
            Some(QuotaError {
                kind: QuotaErrorKind::AccessTerminated,
                status,
                provider: provider.to_string(),
                body: body.to_string(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_429_as_rate_limit() {
        let qe = classify_response("openai", 429, r#"{"error":{"message":"too many"}}"#).unwrap();
        assert_eq!(qe.kind, QuotaErrorKind::RateLimit);
        assert_eq!(qe.status, 429);
        assert_eq!(qe.provider, "openai");
    }

    #[test]
    fn classify_403_with_access_terminated_signature() {
        let body = r#"{"error":{"type":"access_terminated_error","message":"…"}}"#;
        let qe = classify_response("kimi-coding", 403, body).unwrap();
        assert_eq!(qe.kind, QuotaErrorKind::AccessTerminated);
    }

    #[test]
    fn classify_403_with_kimi_period_signature() {
        let body = "usage limit reached for this period";
        let qe = classify_response("kimi", 403, body).unwrap();
        assert_eq!(qe.kind, QuotaErrorKind::AccessTerminated);
    }

    #[test]
    fn classify_403_without_quota_body_returns_none() {
        // 403 from misconfigured auth or a banned tool is NOT a
        // quota event; orchestration must not treat it as retryable.
        let body = r#"{"error":{"type":"permission_denied"}}"#;
        assert!(classify_response("openai", 403, body).is_none());
    }

    #[test]
    fn classify_500_returns_none() {
        assert!(classify_response("openai", 500, "internal").is_none());
    }

    #[test]
    fn slug_round_trips() {
        assert_eq!(QuotaErrorKind::RateLimit.slug(), "quota_rate_limit");
        assert_eq!(
            QuotaErrorKind::AccessTerminated.slug(),
            "quota_access_terminated"
        );
    }

    #[test]
    fn display_includes_provider_and_kind() {
        let qe = QuotaError {
            kind: QuotaErrorKind::RateLimit,
            status: 429,
            provider: "openai".to_string(),
            body: "too many".to_string(),
        };
        let rendered = qe.to_string();
        assert!(rendered.contains("openai"));
        assert!(rendered.contains("quota_rate_limit"));
        assert!(rendered.contains("429"));
    }
}
