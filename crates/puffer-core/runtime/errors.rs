//! Typed provider-error enum, modeled on codex's `CodexErr`.
//!
//! # Why this exists
//!
//! Today every provider failure (Anthropic 429, OpenAI stream
//! disconnect, kimi 403-access-terminated, OpenAI invalid-request
//! 400, …) bubbles up as `anyhow::Error` whose only structure is the
//! message string. Three concrete pain points have been observed in
//! the last month of `tb2-bench` runs:
//!
//! 1. **Quota cascades waste retries** (#83 follow-up). Without a
//!    typed signal, `run_tb2.py` cannot tell a transient 429 from a
//!    capability failure and burns 3 retries per task back-to-back.
//! 2. **Context-window overflow looks like a network blip** (#92
//!    follow-up). `ContextWindowExceeded` deserves its own recovery
//!    path (microcompact / new-thread) but currently surfaces as a
//!    generic stream-disconnect string.
//! 3. **Silent-completion failures hide auth/permission issues**
//!    (#82 follow-up). Auth errors that need user action get retried
//!    as if they were transient.
//!
//! Each provider adapter today carries its own ad-hoc string-matching
//! to decide retry vs. bail. Codex solved this with a single
//! `CodexErr` enum that is the *only* error type returned by the
//! provider layer, paired with `is_retryable()` and
//! `http_status_code_value()` methods. Downstream code switches on
//! variants instead of grepping messages.
//!
//! # Scope of this module
//!
//! This PR is **foundation only** — it defines the enum, the impls,
//! the conversion helpers, and the tests. Migration of the existing
//! call sites (`runtime/openai.rs`, `runtime/anthropic.rs`,
//! `runtime.rs:785-787`, and the #83 quota classification points) is
//! deliberately deferred to follow-up PRs so this one stays
//! reviewable and can land while #83/#89/#92 are still being shaped.
//!
//! # Mapping to codex
//!
//! | Codex `CodexErr` variant       | Puffer `ProviderError` variant |
//! |--------------------------------|--------------------------------|
//! | `UsageLimitReached(_)`         | `RateLimit { .. }`             |
//! | `QuotaExceeded`                | `AccessTerminated { .. }`      |
//! | `ContextWindowExceeded`        | `ContextWindowExceeded { .. }` |
//! | `ServerOverloaded`             | `ServerOverloaded { .. }`      |
//! | `Stream(msg, retry_after)`     | `Stream { .. }`                |
//! | `InvalidRequest(_)`            | `InvalidRequest { .. }`        |
//! | `RefreshTokenFailed(_)`        | `AuthError { .. }`             |
//! | `ConnectionFailed(_)` / Io     | `Network(reqwest::Error)`      |
//! | `Timeout`                      | `Timeout`                      |
//! | `UnexpectedStatus(_)`          | `Unknown { .. }`               |
//!
//! Variants codex carries that puffer doesn't (yet) need are dropped
//! intentionally: `TurnAborted`, `Sandbox`, `Landlock*`, `Spawn`,
//! `Interrupted`, `ThreadNotFound`, `AgentLimitReached`. These are
//! either codex-specific orchestration concerns or live in different
//! puffer modules.

use std::time::Duration;

use reqwest::header::HeaderMap;
use thiserror::Error;
use time::OffsetDateTime;

/// Typed error returned by the provider transport layer.
///
/// Adapters classify HTTP responses into one of these variants via
/// [`classify`] and the orchestration layer matches on the variant
/// to decide retry / delay / bail.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// HTTP 429, or a 200 body that signals rate-limit. `retry_after`
    /// comes from the `Retry-After` header when present;
    /// `resets_at` from provider-specific reset headers (Anthropic
    /// emits `anthropic-ratelimit-*-reset` as RFC3339).
    #[error("rate limit (status {status}, retry_after={retry_after:?}): {body}")]
    RateLimit {
        retry_after: Option<Duration>,
        resets_at: Option<OffsetDateTime>,
        body: String,
        status: u16,
    },

    /// HTTP 403 with an `access_terminated_error` body, or kimi's
    /// "usage limit reached for this period" string. Recovery is
    /// measured in hours; orchestration should not retry.
    #[error("access terminated by {provider}: {body}")]
    AccessTerminated { provider: String, body: String },

    /// Model returned context-window overflow. Caller should trigger
    /// microcompact or start a new thread; not retryable in place.
    #[error("context window exceeded ({tokens_used:?}/{tokens_max:?}): {message}")]
    ContextWindowExceeded {
        tokens_used: Option<u32>,
        tokens_max: Option<u32>,
        message: String,
    },

    /// HTTP 5xx with overloaded body, or Anthropic's `overloaded_error`.
    #[error("server overloaded (retry_after={retry_after:?})")]
    ServerOverloaded { retry_after: Option<Duration> },

    /// SSE stream disconnected after a successful HTTP handshake but
    /// before the provider signaled completion. Mirrors codex's
    /// `Stream(String, Option<Duration>)` — the model loop treats this
    /// as transient and retries the turn.
    #[error("stream disconnected: {message}")]
    Stream {
        message: String,
        retry_after: Option<Duration>,
    },

    /// HTTP 400 / provider-side validation failure. Not retryable —
    /// retrying with the same payload will fail again.
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },

    /// HTTP 401 / token refresh failed / API key revoked. Needs user
    /// action; not retryable.
    #[error("auth error from {provider}")]
    AuthError { provider: String },

    /// Underlying `reqwest::Error` (DNS, TCP, TLS, body decode).
    /// Retryable — most of these are transient.
    #[error(transparent)]
    Network(#[from] reqwest::Error),

    /// Request or stream hit a deadline. Retryable.
    #[error("timeout")]
    Timeout,

    /// Anything we couldn't classify. `status` is `Some` when this
    /// came from an HTTP response, `None` for non-HTTP failures.
    /// Defaults to non-retryable so we don't burn budget on unknowns.
    #[error("unknown provider error (status={status:?}): {message}")]
    Unknown {
        status: Option<u16>,
        message: String,
    },
}

impl ProviderError {
    /// Whether the orchestration layer should retry the request that
    /// produced this error. Mirrors codex's
    /// `CodexErr::is_retryable()` triage:
    ///
    /// - **retryable**: transient signals where the next attempt has
    ///   a real chance (`Network`, `Timeout`, `Stream`,
    ///   `ServerOverloaded`, `RateLimit` after the retry-after delay)
    /// - **non-retryable**: deterministic / user-action signals
    ///   (`InvalidRequest`, `AuthError`, `AccessTerminated`,
    ///   `ContextWindowExceeded`, `Unknown`)
    ///
    /// `RateLimit` is retryable in the sense that the *same* request
    /// will succeed once the window resets — but the caller MUST
    /// honor `retry_after` before issuing the retry, otherwise it
    /// will just burn its budget against the same window. See
    /// `quota.rs` (#83) for the orchestration-side delay logic.
    ///
    /// `ContextWindowExceeded` is intentionally non-retryable: the
    /// recovery path is microcompact / new thread, not a vanilla
    /// retry. The caller should match on this variant explicitly and
    /// drive the recovery itself.
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::RateLimit { .. }
            | ProviderError::ServerOverloaded { .. }
            | ProviderError::Stream { .. }
            | ProviderError::Network(_)
            | ProviderError::Timeout => true,

            ProviderError::AccessTerminated { .. }
            | ProviderError::ContextWindowExceeded { .. }
            | ProviderError::InvalidRequest { .. }
            | ProviderError::AuthError { .. }
            | ProviderError::Unknown { .. } => false,
        }
    }

    /// HTTP status code that produced this error, when known.
    /// Useful for telemetry / `result.json` stamping without
    /// re-walking the response.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            ProviderError::RateLimit { status, .. } => Some(*status),
            ProviderError::AccessTerminated { .. } => Some(403),
            ProviderError::ServerOverloaded { .. } => Some(503),
            ProviderError::AuthError { .. } => Some(401),
            ProviderError::InvalidRequest { .. } => Some(400),
            ProviderError::Network(e) => e.status().map(|s| s.as_u16()),
            ProviderError::Unknown { status, .. } => *status,
            ProviderError::ContextWindowExceeded { .. }
            | ProviderError::Stream { .. }
            | ProviderError::Timeout => None,
        }
    }

    /// Short slug for `result.json` `error_kind` stamping. Stable
    /// across releases — orchestration scripts grep on these.
    pub fn slug(&self) -> &'static str {
        match self {
            ProviderError::RateLimit { .. } => "rate_limit",
            ProviderError::AccessTerminated { .. } => "access_terminated",
            ProviderError::ContextWindowExceeded { .. } => "context_window_exceeded",
            ProviderError::ServerOverloaded { .. } => "server_overloaded",
            ProviderError::Stream { .. } => "stream_disconnected",
            ProviderError::InvalidRequest { .. } => "invalid_request",
            ProviderError::AuthError { .. } => "auth_error",
            ProviderError::Network(_) => "network",
            ProviderError::Timeout => "timeout",
            ProviderError::Unknown { .. } => "unknown",
        }
    }
}

/// Bridge from the existing `runtime::quota::QuotaError` (introduced
/// in #83) into the typed `ProviderError` enum. Lets adapters that
/// already classify quota signals upstream lift them into the unified
/// error vocabulary without a second round of body inspection.
///
/// `QuotaErrorKind::RateLimit` → `ProviderError::RateLimit { .. }` —
/// `retry_after` / `resets_at` are left as `None` because `QuotaError`
/// does not currently carry header-derived hints; callers that need
/// them should classify via [`classify`] (which sees the `HeaderMap`)
/// instead of going through this conversion.
///
/// `QuotaErrorKind::AccessTerminated` →
/// `ProviderError::AccessTerminated { .. }` — preserves the original
/// provider tag and body so downstream telemetry / `result.json`
/// stamping is unchanged.
impl From<crate::runtime::quota::QuotaError> for ProviderError {
    fn from(quota: crate::runtime::quota::QuotaError) -> Self {
        use crate::runtime::quota::QuotaErrorKind;
        match quota.kind {
            QuotaErrorKind::RateLimit => ProviderError::RateLimit {
                retry_after: None,
                resets_at: None,
                body: quota.body,
                status: quota.status,
            },
            QuotaErrorKind::AccessTerminated => ProviderError::AccessTerminated {
                provider: quota.provider,
                body: quota.body,
            },
        }
    }
}

/// Parse a `Retry-After` header value. Per RFC 7231 the value is
/// either a non-negative integer (seconds) or an HTTP-date; this
/// helper handles only the integer form, which is what every LLM
/// provider observed in the wild emits. HTTP-date form returns
/// `None` and the caller falls back to the variant's default.
fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?;
    let s = value.to_str().ok()?;
    s.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Classify an HTTP response into a [`ProviderError`]. This is the
/// future single chokepoint — adapters today have their own ad-hoc
/// string-matching, and the migration plan is to route them through
/// here so retry/delay decisions live in exactly one place.
///
/// Returns `None` for 2xx (success) and for 4xx/5xx that don't match
/// a known signature; the caller should fall back to its existing
/// path (typically `Unknown { status, message }` constructed by
/// hand, or — during the migration — a vanilla `anyhow::bail!`).
///
/// `body` is expected to be the response body text already drained
/// off the wire; this function does not read network state.
pub fn classify(
    provider: &str,
    status: u16,
    headers: &HeaderMap,
    body: &str,
) -> Option<ProviderError> {
    match status {
        // 200 / 204 / etc — caller shouldn't have called us, but
        // be defensive: a 200 with an embedded error body is the
        // caller's job to detect.
        s if (200..300).contains(&s) => None,

        400 => Some(ProviderError::InvalidRequest {
            message: body.to_string(),
        }),

        401 => Some(ProviderError::AuthError {
            provider: provider.to_string(),
        }),

        403 if body.contains("access_terminated_error")
            || body.contains("usage limit reached for this period") =>
        {
            Some(ProviderError::AccessTerminated {
                provider: provider.to_string(),
                body: body.to_string(),
            })
        }

        429 => Some(ProviderError::RateLimit {
            retry_after: parse_retry_after(headers),
            resets_at: None, // provider-specific reset headers parsed in follow-up
            body: body.to_string(),
            status,
        }),

        503 => Some(ProviderError::ServerOverloaded {
            retry_after: parse_retry_after(headers),
        }),

        // 5xx with overloaded body (Anthropic returns 529 occasionally,
        // and 500 with `overloaded_error` type).
        s if (500..600).contains(&s) && body.contains("overloaded_error") => {
            Some(ProviderError::ServerOverloaded {
                retry_after: parse_retry_after(headers),
            })
        }

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    fn headers_with_retry_after(seconds: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            reqwest::header::RETRY_AFTER,
            HeaderValue::from_str(seconds).unwrap(),
        );
        h
    }

    #[test]
    fn rate_limit_is_retryable() {
        let e = ProviderError::RateLimit {
            retry_after: Some(Duration::from_secs(30)),
            resets_at: None,
            body: "too many".into(),
            status: 429,
        };
        assert!(e.is_retryable());
    }

    #[test]
    fn access_terminated_is_not_retryable() {
        let e = ProviderError::AccessTerminated {
            provider: "kimi".into(),
            body: "access_terminated_error".into(),
        };
        assert!(!e.is_retryable());
    }

    #[test]
    fn context_window_exceeded_is_not_retryable() {
        // Recovery is microcompact / new thread, NOT a vanilla retry.
        let e = ProviderError::ContextWindowExceeded {
            tokens_used: Some(200_000),
            tokens_max: Some(200_000),
            message: "input too long".into(),
        };
        assert!(!e.is_retryable());
    }

    #[test]
    fn server_overloaded_is_retryable() {
        let e = ProviderError::ServerOverloaded {
            retry_after: Some(Duration::from_secs(5)),
        };
        assert!(e.is_retryable());
    }

    #[test]
    fn stream_is_retryable() {
        let e = ProviderError::Stream {
            message: "connection reset".into(),
            retry_after: None,
        };
        assert!(e.is_retryable());
    }

    #[test]
    fn invalid_request_is_not_retryable() {
        let e = ProviderError::InvalidRequest {
            message: "bad json".into(),
        };
        assert!(!e.is_retryable());
    }

    #[test]
    fn auth_error_is_not_retryable() {
        let e = ProviderError::AuthError {
            provider: "openai".into(),
        };
        assert!(!e.is_retryable());
    }

    #[test]
    fn timeout_is_retryable() {
        assert!(ProviderError::Timeout.is_retryable());
    }

    #[test]
    fn unknown_defaults_to_non_retryable() {
        // Conservative default: never burn retry budget on errors we
        // haven't explicitly classified.
        let e = ProviderError::Unknown {
            status: Some(418),
            message: "teapot".into(),
        };
        assert!(!e.is_retryable());
    }

    #[test]
    fn http_status_round_trips() {
        let e = ProviderError::RateLimit {
            retry_after: None,
            resets_at: None,
            body: String::new(),
            status: 429,
        };
        assert_eq!(e.http_status(), Some(429));

        let e = ProviderError::AccessTerminated {
            provider: "kimi".into(),
            body: String::new(),
        };
        assert_eq!(e.http_status(), Some(403));

        assert_eq!(ProviderError::Timeout.http_status(), None);
    }

    #[test]
    fn slug_is_stable() {
        // These slugs are part of the `result.json` contract that
        // run_tb2.py / puffer_harbor_agent.py grep on. Don't change
        // them without a follow-up to those scripts.
        assert_eq!(
            ProviderError::RateLimit {
                retry_after: None,
                resets_at: None,
                body: String::new(),
                status: 429,
            }
            .slug(),
            "rate_limit"
        );
        assert_eq!(
            ProviderError::AccessTerminated {
                provider: "kimi".into(),
                body: String::new(),
            }
            .slug(),
            "access_terminated"
        );
        assert_eq!(ProviderError::Timeout.slug(), "timeout");
    }

    #[test]
    fn classify_429_with_retry_after() {
        let headers = headers_with_retry_after("30");
        let e = classify("openai", 429, &headers, r#"{"error":"too many"}"#).unwrap();
        match e {
            ProviderError::RateLimit {
                retry_after,
                status,
                ..
            } => {
                assert_eq!(retry_after, Some(Duration::from_secs(30)));
                assert_eq!(status, 429);
            }
            other => panic!("expected RateLimit, got {other:?}"),
        }
    }

    #[test]
    fn classify_429_without_retry_after_header() {
        let headers = HeaderMap::new();
        let e = classify("openai", 429, &headers, "rate limited").unwrap();
        match e {
            ProviderError::RateLimit { retry_after, .. } => {
                assert_eq!(retry_after, None);
            }
            other => panic!("expected RateLimit, got {other:?}"),
        }
    }

    #[test]
    fn classify_403_access_terminated() {
        let body = r#"{"error":{"type":"access_terminated_error","message":"…"}}"#;
        let e = classify("kimi-coding", 403, &HeaderMap::new(), body).unwrap();
        match e {
            ProviderError::AccessTerminated { provider, .. } => {
                assert_eq!(provider, "kimi-coding");
            }
            other => panic!("expected AccessTerminated, got {other:?}"),
        }
    }

    #[test]
    fn classify_403_kimi_period_signature() {
        let body = "usage limit reached for this period";
        let e = classify("kimi", 403, &HeaderMap::new(), body).unwrap();
        assert!(matches!(e, ProviderError::AccessTerminated { .. }));
    }

    #[test]
    fn classify_403_without_quota_signature_returns_none() {
        // 403 from misconfigured auth or a banned tool is NOT a
        // quota event; mirrors quota.rs invariant from #83.
        let body = r#"{"error":{"type":"permission_denied"}}"#;
        assert!(classify("openai", 403, &HeaderMap::new(), body).is_none());
    }

    #[test]
    fn classify_503_with_retry_after() {
        let headers = headers_with_retry_after("5");
        let e = classify("anthropic", 503, &headers, "overloaded").unwrap();
        match e {
            ProviderError::ServerOverloaded { retry_after } => {
                assert_eq!(retry_after, Some(Duration::from_secs(5)));
            }
            other => panic!("expected ServerOverloaded, got {other:?}"),
        }
    }

    #[test]
    fn classify_500_with_overloaded_body() {
        // Anthropic returns 529 / 500 with `overloaded_error` type;
        // map both to ServerOverloaded so the retry path is uniform.
        let body = r#"{"type":"error","error":{"type":"overloaded_error"}}"#;
        let e = classify("anthropic", 500, &HeaderMap::new(), body).unwrap();
        assert!(matches!(e, ProviderError::ServerOverloaded { .. }));
    }

    #[test]
    fn classify_500_without_overloaded_body_returns_none() {
        // Plain 500 falls through; caller decides whether to wrap as
        // Unknown or bail.
        assert!(classify("openai", 500, &HeaderMap::new(), "internal").is_none());
    }

    #[test]
    fn classify_400_invalid_request() {
        let e = classify("openai", 400, &HeaderMap::new(), "bad json").unwrap();
        assert!(matches!(e, ProviderError::InvalidRequest { .. }));
        assert!(!e.is_retryable());
    }

    #[test]
    fn classify_401_auth_error() {
        let e = classify("openai", 401, &HeaderMap::new(), "no key").unwrap();
        match e {
            ProviderError::AuthError { provider } => assert_eq!(provider, "openai"),
            other => panic!("expected AuthError, got {other:?}"),
        }
    }

    #[test]
    fn classify_2xx_returns_none() {
        // Defensive: a 200 with an embedded error body is the
        // caller's responsibility, not classify's.
        assert!(classify("openai", 200, &HeaderMap::new(), "ok").is_none());
    }

    #[test]
    fn from_quota_error_rate_limit_round_trip() {
        use crate::runtime::quota::{QuotaError, QuotaErrorKind};
        let q = QuotaError {
            kind: QuotaErrorKind::RateLimit,
            provider: "anthropic".to_string(),
            status: 429,
            body: "rate limited".to_string(),
        };
        let err: ProviderError = q.into();
        assert!(matches!(err, ProviderError::RateLimit { status: 429, .. }));
    }

    #[test]
    fn from_quota_error_access_terminated_round_trip() {
        use crate::runtime::quota::{QuotaError, QuotaErrorKind};
        let q = QuotaError {
            kind: QuotaErrorKind::AccessTerminated,
            provider: "kimi".to_string(),
            status: 403,
            body: "usage limit reached for this period".to_string(),
        };
        let err: ProviderError = q.into();
        match err {
            ProviderError::AccessTerminated { provider, body } => {
                assert_eq!(provider, "kimi");
                assert!(body.contains("usage limit reached"));
            }
            other => panic!("expected AccessTerminated, got {other:?}"),
        }
    }

    #[test]
    fn parse_retry_after_handles_garbage() {
        // HTTP-date form is intentionally not supported; bogus
        // values fall back to None instead of panicking.
        let mut h = HeaderMap::new();
        h.insert(
            reqwest::header::RETRY_AFTER,
            HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"),
        );
        assert_eq!(parse_retry_after(&h), None);
    }
}
