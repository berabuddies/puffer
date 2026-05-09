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

    /// Regression: prior to the classify-before-retry fix the inner
    /// `send_http_request_raw` retry loop saw a 429 and retried 3 more
    /// times before any caller could classify. By the time the
    /// provider adapter saw the response, the orchestrator had already
    /// burned ~10s of cooldown — exactly the budget the typed quota
    /// path is supposed to protect.
    ///
    /// This test stands up a TCP listener that always replies 429,
    /// counts inbound connections, configures the retry loop for 3
    /// retries (= 4 attempts), and asserts the listener saw exactly 1
    /// connection — proving the loop bails on first 429 instead of
    /// retrying.
    #[test]
    fn quota_429_short_circuits_inner_retry_loop() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};
        use std::thread;

        // Serialize against other tests that mutate the retry env vars.
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Force 3 retries (4 total attempts) and a tiny delay so the
        // test runs quickly even if the regression bug returns. With
        // the fix in place we should still only see 1 attempt.
        let prev_attempts = std::env::var_os(super::super::HTTP_RETRY_ATTEMPTS_ENV);
        let prev_delay = std::env::var_os(super::super::HTTP_RETRY_DELAY_MS_ENV);
        std::env::set_var(super::super::HTTP_RETRY_ATTEMPTS_ENV, "3");
        std::env::set_var(super::super::HTTP_RETRY_DELAY_MS_ENV, "1");

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let connection_count = Arc::new(Mutex::new(0_usize));
        let counter = Arc::clone(&connection_count);

        let server = thread::spawn(move || {
            // Accept up to 5 connections so a regression (which would
            // produce 4) is observable rather than hanging the test on
            // accept(). Each connection drains one HTTP request and
            // replies 429.
            for _ in 0..5 {
                listener.set_nonblocking(false).ok();
                let accept = listener.accept();
                let (mut stream, _) = match accept {
                    Ok(pair) => pair,
                    Err(_) => return,
                };
                {
                    let mut count = counter.lock().unwrap();
                    *count += 1;
                }
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer);
                let body = r#"{"error":{"message":"rate_limit_reached"}}"#;
                let response = format!(
                    "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        // Fire one logical request — should NOT spin the inner retry
        // loop now that 429 is classified before the retry decision.
        let url = format!("http://{address}/v1/messages");
        let result = super::super::send_http_request_raw(&url, &[], "{}", true);

        // Restore env before any assertion can panic.
        match prev_attempts {
            Some(value) => std::env::set_var(super::super::HTTP_RETRY_ATTEMPTS_ENV, value),
            None => std::env::remove_var(super::super::HTTP_RETRY_ATTEMPTS_ENV),
        }
        match prev_delay {
            Some(value) => std::env::set_var(super::super::HTTP_RETRY_DELAY_MS_ENV, value),
            None => std::env::remove_var(super::super::HTTP_RETRY_DELAY_MS_ENV),
        }

        // Drop the listener handle by closing the spawned thread once
        // a small grace period passes. We don't join here because the
        // server only exits after `accept()` returns an error or 5
        // connections — and on the success path we only sent 1.
        drop(server);

        // The raw call returns Ok(response) with status 429 (the
        // typed-error promotion happens in the parser path). The
        // critical invariant is: only 1 inbound HTTP request reached
        // the listener, not 4.
        let response = result.expect("send_http_request_raw should return Ok with 429 body");
        assert_eq!(response.status.as_u16(), 429);

        let connections = *connection_count.lock().unwrap();
        assert_eq!(
            connections, 1,
            "expected the inner retry loop to short-circuit on 429, but it made {connections} attempts"
        );
    }

    /// Pair test: once the response reaches `parse_http_json_response`,
    /// the 429 must be promoted to a typed `QuotaError` rather than a
    /// generic `bail!`. Without this the entire Anthropic blocking
    /// path would lose the typed signal.
    #[test]
    fn parse_http_json_response_promotes_429_to_quota_error() {
        use reqwest::StatusCode;

        let raw = super::super::RawHttpResponse {
            status: StatusCode::TOO_MANY_REQUESTS,
            content_type: Some("application/json".to_string()),
            text: r#"{"error":{"message":"rate_limit"}}"#.to_string(),
        };
        let err = super::super::parse_http_json_response(
            "https://api.anthropic.com/v1/messages",
            true,
            raw,
        )
        .expect_err("429 must surface as Err");
        let quota = err
            .downcast_ref::<QuotaError>()
            .expect("error must downcast to QuotaError");
        assert_eq!(quota.kind, QuotaErrorKind::RateLimit);
        assert_eq!(quota.status, 429);
        assert_eq!(quota.provider, "anthropic");
    }
}
