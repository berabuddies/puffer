use super::openai::{is_event_stream, parse_openai_sse_response};
use super::openai_sse::is_openai_sse_api_error;
use super::quota;
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde_json::Value;
use std::time::Duration;

pub(crate) const HTTP_RETRY_ATTEMPTS_ENV: &str = "PUFFER_HTTP_RETRY_ATTEMPTS";
pub(crate) const HTTP_RETRY_DELAY_MS_ENV: &str = "PUFFER_HTTP_RETRY_DELAY_MS";

#[derive(Debug)]
pub(crate) struct RawHttpResponse {
    pub(crate) status: StatusCode,
    pub(crate) content_type: Option<String>,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HttpRetryConfig {
    pub(crate) retries: usize,
    pub(crate) delay_ms: u64,
}

pub(crate) fn send_http_request(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    anthropic: bool,
) -> Result<Value> {
    let response = send_http_request_raw(url, headers, body, anthropic)?;
    parse_http_json_response(url, anthropic, response)
}

pub(crate) fn send_http_request_with_proxy(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    anthropic: bool,
    proxy: &puffer_config::ProxyConfig,
) -> Result<Value> {
    let response = send_http_request_raw_with_proxy(url, headers, body, anthropic, proxy)?;
    parse_http_json_response(url, anthropic, response)
}

pub(crate) fn send_http_request_raw(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    anthropic: bool,
) -> Result<RawHttpResponse> {
    send_http_request_raw_with_proxy(
        url,
        headers,
        body,
        anthropic,
        &puffer_config::ProxyConfig::default(),
    )
}

pub(crate) fn send_http_request_raw_with_proxy(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    anthropic: bool,
    proxy: &puffer_config::ProxyConfig,
) -> Result<RawHttpResponse> {
    trace_http_exchange("request", url, headers, body);
    let retry_config = http_retry_config();
    let total_attempts = retry_config.retries.saturating_add(1);
    for attempt in 1..=total_attempts {
        match send_http_request_raw_once_with_proxy(url, headers, body, anthropic, proxy) {
            Ok(response) => {
                trace_http_response(url, response.status.as_u16(), &response.text);
                // Retry on 429 (rate limit) and 5xx (server errors) unless the
                // response classifies as quota/access termination. Retrying quota
                // errors burns cooldown before the typed classifier can act.
                let status = response.status.as_u16();
                let provider = if anthropic { "anthropic" } else { "openai" };
                if quota::classify_response(provider, status, &response.text).is_some() {
                    return Ok(response);
                }
                if attempt < total_attempts && (status == 429 || (500..=599).contains(&status)) {
                    let delay = retry_delay(retry_config, attempt);
                    if !delay.is_zero() {
                        std::thread::sleep(delay);
                    }
                    continue;
                }
                return Ok(response);
            }
            Err(error) if attempt < total_attempts && is_retryable_http_error(&error) => {
                trace_http_retry(url, attempt, &error);
                let delay = retry_delay(retry_config, attempt);
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("http retry loop exited without returning")
}

fn send_http_request_raw_once_with_proxy(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    anthropic: bool,
    proxy: &puffer_config::ProxyConfig,
) -> Result<RawHttpResponse> {
    let client = crate::network::blocking_client_for_url(
        proxy,
        crate::network::HttpPurpose::Model,
        url,
        Duration::from_secs(300),
    )
    .unwrap_or_else(|_| Client::new());
    let mut request = client.post(url);
    for (key, value) in headers {
        request = request.header(key, value);
    }
    if !headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("content-type"))
    {
        request = request.header("content-type", "application/json");
    }
    if anthropic
        && !headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("anthropic-version"))
    {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let response = request
        .body(body.to_string())
        .send()
        .with_context(|| format!("request to {url} failed"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let text = response
        .text()
        .with_context(|| format!("failed to read response body from {url}"))?;
    Ok(RawHttpResponse {
        status,
        content_type,
        text,
    })
}

pub(crate) fn http_retry_config() -> HttpRetryConfig {
    HttpRetryConfig {
        retries: parsed_env_usize(HTTP_RETRY_ATTEMPTS_ENV)
            .unwrap_or(3)
            .min(10),
        delay_ms: parsed_env_u64(HTTP_RETRY_DELAY_MS_ENV)
            .unwrap_or(1_000)
            .min(30_000),
    }
}

fn parsed_env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn parsed_env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

pub(crate) fn retry_delay(config: HttpRetryConfig, attempt: usize) -> Duration {
    if config.delay_ms == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(config.delay_ms.saturating_mul(attempt as u64))
}

/// Provider-agnostic helper that retries a closure returning a
/// `reqwest::blocking::Response` whenever the response status is 5xx.
pub(crate) fn retry_on_5xx<F>(
    mut op: F,
    mut on_retry: impl FnMut(usize, usize, reqwest::StatusCode),
) -> Result<reqwest::blocking::Response>
where
    F: FnMut() -> Result<reqwest::blocking::Response>,
{
    let max_attempts = http_5xx_max_attempts();
    let base_delay = http_5xx_base_delay();
    for attempt in 1..=max_attempts {
        let response = op()?;
        let status = response.status();
        if !status.is_server_error() || attempt == max_attempts {
            return Ok(response);
        }
        let server_hint = parse_retry_after_headers(response.headers());
        drop(response);
        on_retry(attempt, max_attempts, status);
        let delay =
            server_hint.unwrap_or_else(|| http_5xx_backoff_with_jitter(base_delay, attempt));
        std::thread::sleep(delay);
    }
    unreachable!("retry_on_5xx loop always returns or errors")
}

const RETRY_AFTER_CAP_MS: u64 = 60_000;

pub(crate) fn parse_retry_after_headers(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    if let Some(value) = headers.get("retry-after-ms").and_then(|v| v.to_str().ok()) {
        if let Ok(ms) = value.trim().parse::<u64>() {
            return Some(Duration::from_millis(ms.min(RETRY_AFTER_CAP_MS)));
        }
    }
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let trimmed = raw.trim();
    if let Ok(seconds) = trimmed.parse::<u64>() {
        let ms = seconds.saturating_mul(1_000).min(RETRY_AFTER_CAP_MS);
        return Some(Duration::from_millis(ms));
    }
    if let Ok(target) =
        time::OffsetDateTime::parse(trimmed, &time::format_description::well_known::Rfc2822)
    {
        let now = time::OffsetDateTime::now_utc();
        let diff = target - now;
        let ms = diff.whole_milliseconds();
        if ms <= 0 {
            return Some(Duration::ZERO);
        }
        let ms = (ms as u64).min(RETRY_AFTER_CAP_MS);
        return Some(Duration::from_millis(ms));
    }
    None
}

/// Returns the configured 5xx retry attempt count.
pub(crate) fn http_5xx_max_attempts() -> usize {
    std::env::var("PUFFER_HTTP_5XX_MAX_ATTEMPTS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(3)
        .clamp(1, 5)
}

/// Returns the configured base delay for 5xx retries.
pub(crate) fn http_5xx_base_delay() -> Duration {
    let ms = std::env::var("PUFFER_HTTP_5XX_BASE_DELAY_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(500)
        .clamp(100, 8_000);
    Duration::from_millis(ms)
}

/// Computes capped exponential backoff with one-sided jitter.
pub(crate) fn http_5xx_backoff_with_jitter(base: Duration, attempt: usize) -> Duration {
    let factor = 2u64.saturating_pow((attempt as u32).saturating_sub(1));
    let nominal_ms = base.as_millis().saturating_mul(factor as u128).min(8_000) as u64;
    let jitter_factor = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| (d.subsec_nanos() % 250) as u64)
        .unwrap_or(0);
    let jitter_ms = nominal_ms.saturating_mul(jitter_factor) / 1_000;
    Duration::from_millis(nominal_ms.saturating_sub(jitter_ms))
}

pub(crate) fn is_retryable_http_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|value| value.is_timeout() || value.is_connect())
            || cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(is_retryable_io_error)
    })
}

fn is_retryable_io_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
    )
}

fn trace_http_exchange(kind: &str, url: &str, headers: &[(String, String)], body: &str) {
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
                "--- {} {} ---\n{}\n\n{}\n",
                kind.to_ascii_uppercase(),
                url,
                rendered_headers,
                body
            )
        });
}

fn trace_http_response(url: &str, status: u16, body: &str) {
    let Ok(path) = std::env::var("PUFFER_HTTP_TRACE_PATH") else {
        return;
    };
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(file, "--- RESPONSE {} {} ---\n{}\n", status, url, body)
        });
}

fn trace_http_retry(url: &str, attempt: usize, error: &anyhow::Error) {
    let Ok(path) = std::env::var("PUFFER_HTTP_TRACE_PATH") else {
        return;
    };
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(file, "--- RETRY {} {} ---\n{}\n", attempt, url, error)
        });
}

pub(crate) fn parse_http_json_response(
    url: &str,
    anthropic: bool,
    response: RawHttpResponse,
) -> Result<Value> {
    if !response.status.is_success() {
        let provider = if anthropic { "anthropic" } else { "openai" };
        if let Some(quota) =
            quota::classify_response(provider, response.status.as_u16(), &response.text)
        {
            return Err(anyhow::Error::new(quota));
        }
        bail!(
            "request failed with status {}: {}",
            response.status,
            response.text
        );
    }
    if !anthropic && is_event_stream(response.content_type.as_deref(), &response.text) {
        return match parse_openai_sse_response(&response.text) {
            Ok(value) => Ok(value),
            Err(error) if is_openai_sse_api_error(&error) => Err(error),
            Err(error) => {
                Err(error).with_context(|| format!("failed to parse SSE response from {url}"))
            }
        };
    }
    serde_json::from_str::<Value>(&response.text)
        .with_context(|| format!("response from {url} was not valid JSON"))
}
