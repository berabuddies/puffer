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

/// Returns true when `body` carries the `access_terminated_error`
/// signature in a way that is unambiguous (i.e. *not* just a docs URL
/// like `https://example.com/docs/access_terminated_error`).
///
/// Three layers, in order of trust:
///
/// 1. **Structured JSON match** — try parsing the body as JSON and
///    look for `error.type == "access_terminated_error"`. This is the
///    Anthropic-shape and the strongest signal.
/// 2. **JSON-shaped substring** — look for the literal
///    `"type":"access_terminated_error"` (with quote characters). This
///    catches Kimi's non-conforming responses that quote the marker
///    inside a non-JSON-validating envelope.
/// 3. **Period-quota string** — Kimi's older-shape body uses the
///    free-form `"usage limit reached for this period"`. Kept as a
///    last-ditch fallback only because there's no JSON-field equivalent.
///
/// Bare `access_terminated_error` substring is intentionally NOT a
/// match: a docs URL with that path segment is a false positive that
/// would trip the long quota cooldown on a transient 403.
fn body_signals_access_terminated(body: &str) -> bool {
    if body.is_empty() {
        return false;
    }

    // Structured-JSON path. Case-sensitive — JSON field values are
    // case-sensitive on the wire. Don't pre-lowercase here.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(kind) = value.get("error").and_then(|err| err.get("type")).and_then(serde_json::Value::as_str) {
            if kind == "access_terminated_error" {
                return true;
            }
        }
    }

    // Fallback substring matches — lowercase the haystack so casings
    // like `Access_Terminated_Error` still classify. The needles are
    // already lowercase. This branch only runs when the structured
    // parse failed or the field was missing.
    let lowered = body.to_ascii_lowercase();
    if lowered.contains(r#""type":"access_terminated_error""#) {
        return true;
    }
    // Tolerate one space after the colon (`"type": "access_terminated_error"`)
    // — some providers pretty-print the body.
    if lowered.contains(r#""type": "access_terminated_error""#) {
        return true;
    }
    // Kimi's free-form period-quota message. Distinctive enough that
    // we accept the bare substring even without JSON structure.
    if lowered.contains("usage limit reached for this period") {
        return true;
    }
    false
}

/// Inspect an HTTP status + response body and classify the failure.
/// Returns `Some(QuotaError)` when this is unambiguously a quota
/// signal; `None` for anything else (the caller should fall back to
/// its existing `bail!` path).
///
/// This intentionally does not allocate when the status is success —
/// the caller is expected to short-circuit on `status.is_success()`
/// before calling here.
///
/// When the body carries an `access_terminated_error` marker the
/// classifier promotes the result to `AccessTerminated` regardless of
/// status code. Kimi has been observed returning the marker on a 429
/// envelope, and the *kind* drives orchestration's cooldown — getting
/// it right matters more than mirroring the wire status.
pub fn classify_response(provider: &str, status: u16, body: &str) -> Option<QuotaError> {
    let access_terminated = body_signals_access_terminated(body);

    if access_terminated {
        return Some(QuotaError {
            kind: QuotaErrorKind::AccessTerminated,
            status,
            provider: provider.to_string(),
            body: body.to_string(),
        });
    }

    match status {
        429 => Some(QuotaError {
            kind: QuotaErrorKind::RateLimit,
            status,
            provider: provider.to_string(),
            body: body.to_string(),
        }),
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

    /// Reviewer-flagged false positive: a 403 error body whose only
    /// occurrence of `access_terminated_error` is inside a docs URL
    /// (e.g. an HTML page or a free-form error pointing at
    /// `https://example.com/docs/access_terminated_error`) must NOT
    /// classify as quota — it would otherwise trigger the 600s
    /// AccessTerminated cooldown on what is actually a transient or
    /// unrelated 403.
    #[test]
    fn classify_403_with_docs_url_mention_is_not_quota() {
        let body = "See https://example.com/docs/access_terminated_error for details.";
        assert!(
            classify_response("openai", 403, body).is_none(),
            "URL path containing the marker must not trip AccessTerminated"
        );
    }

    /// Anthropic-shape: structured JSON with `error.type ==
    /// access_terminated_error`. Strongest signal, matched at the
    /// JSON-field level.
    #[test]
    fn classify_403_with_quoted_field_marker_is_quota() {
        let body = r#"{"error":{"type":"access_terminated_error","message":"…"}}"#;
        let qe = classify_response("anthropic", 403, body)
            .expect("structured access_terminated_error must classify");
        assert_eq!(qe.kind, QuotaErrorKind::AccessTerminated);
    }

    /// Kimi-shape: not valid JSON end-to-end, but the `"type":"access_terminated_error"`
    /// quoted-key fragment is present. Classifier should still match
    /// via the JSON-shaped-substring fallback.
    #[test]
    fn classify_403_with_kimi_style_string_is_quota() {
        let body = r#"some prefix "type":"access_terminated_error" some suffix"#;
        let qe = classify_response("kimi", 403, body)
            .expect("Kimi-style quoted marker must classify");
        assert_eq!(qe.kind, QuotaErrorKind::AccessTerminated);
    }

    /// Casing variants — providers occasionally upper-case the
    /// machine-readable type field. ASCII-lowercasing the haystack
    /// before the substring fallback keeps these classified.
    #[test]
    fn classify_403_capitalized_uppercase() {
        let body = r#"{"error":{"type":"Access_Terminated_Error"}}"#;
        let qe = classify_response("kimi", 403, body)
            .expect("upper-cased Access_Terminated_Error should still classify");
        assert_eq!(qe.kind, QuotaErrorKind::AccessTerminated);
    }

    /// Kimi has been observed returning 429 (RateLimit-shaped status)
    /// with an `access_terminated_error` body. The *kind* drives
    /// cooldown duration in orchestration (RateLimit ≈ seconds vs
    /// AccessTerminated ≈ hours), so the marker must win regardless
    /// of status.
    #[test]
    fn classify_429_with_access_terminated_body_promotes_to_access_terminated() {
        let body = r#"{"error":{"type":"access_terminated_error","message":"period exhausted"}}"#;
        let qe = classify_response("kimi", 429, body)
            .expect("429 with access_terminated body must classify");
        assert_eq!(
            qe.kind,
            QuotaErrorKind::AccessTerminated,
            "marker must take priority over status-derived RateLimit"
        );
        // Status is still recorded faithfully on the wire.
        assert_eq!(qe.status, 429);
    }

    /// Empty 403 body. Today this returns `None` — we have no signal
    /// to disambiguate auth-vs-quota without a body. Kimi has been
    /// observed sending bare 403s on quota exhaustion; the right fix
    /// is provider-aware fallback (e.g. "if provider=='kimi' and
    /// status==403 with empty body, assume AccessTerminated") but
    /// that requires plumbing provider hints down further. Captured
    /// as a TODO; not implemented in this PR.
    ///
    /// TODO(quota): provider-aware fallback for empty-body 403 on
    /// Kimi → AccessTerminated.
    #[test]
    fn classify_403_empty_body() {
        assert!(
            classify_response("kimi", 403, "").is_none(),
            "empty body currently returns None; provider-aware fallback is a follow-up"
        );
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
