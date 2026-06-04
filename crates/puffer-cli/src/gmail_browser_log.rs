//! Low-sensitivity diagnostics for Gmail browser setup and polling.

use std::time::{SystemTime, UNIX_EPOCH};

use puffer_config::ConfigPaths;
use serde_json::Value;

/// Writes one Gmail browser diagnostic line to stderr.
pub(crate) fn line(message: impl AsRef<str>) {
    eprintln!("gmail-browser: {}", message.as_ref());
}

/// Logs subscriber startup state without exposing configured account names.
pub(crate) fn subscriber_start(
    topic: &str,
    state_dir: &std::path::Path,
    seen_initialized: bool,
    seen_count: usize,
) {
    line(format!(
        "subscriber_start ts_ms={} topic={topic} state_dir={} config_exists={} seen_exists={} auth_state_exists={} seen_initialized={seen_initialized} seen_count={seen_count}",
        now_ms(),
        state_dir.display(),
        state_dir.join("config.toml").exists(),
        state_dir.join("seen.json").exists(),
        state_dir.join("auth_state.json").exists()
    ));
}

/// Logs why a subscriber cannot poll yet.
pub(crate) fn config_required(topic: &str, state_dir: &std::path::Path, reason: &str) {
    line(format!(
        "config_required topic={topic} state_dir={} reason={reason}",
        state_dir.display()
    ));
}

/// Logs that a subscriber loaded pollable config.
pub(crate) fn config_ready(
    topic: &str,
    state_dir: &std::path::Path,
    account_count: usize,
    auth_required_count: usize,
    seen_initialized: bool,
    seen_count: usize,
) {
    line(format!(
        "config_ready topic={topic} state_dir={} account_count={account_count} auth_required_count={auth_required_count} seen_initialized={seen_initialized} seen_count={seen_count}",
        state_dir.display()
    ));
}

/// Logs a top-level subscriber poll loop error.
pub(crate) fn poll_loop_error(topic: &str, error: &anyhow::Error) {
    line(format!("poll_loop_error topic={topic} error={error:#}"));
}

/// Logs the daemon browser path selected for Gmail polling.
pub(crate) fn browser_daemon_connect(paths: &ConfigPaths) {
    line(format!(
        "browser_daemon_connect workspace_root={} user_config_dir={}",
        paths.workspace_root.display(),
        paths.user_config_dir.display()
    ));
}

/// Logs an account-level Gmail auth-required transition.
pub(crate) fn account_auth_required(topic: &str, account: &str, result: &Value) {
    line(format!(
        "account_auth_required topic={topic} account_hash={} href_host={} title_class={}",
        account_hash(account),
        host_hint(result.get("href").and_then(Value::as_str)),
        title_class(result.get("title").and_then(Value::as_str))
    ));
}

/// Logs an account-level Gmail poll error.
pub(crate) fn account_poll_error(topic: &str, account: &str, status: &str, result: &Value) {
    line(format!(
        "account_poll_error topic={topic} account_hash={} status={status} href_host={} title_class={}",
        account_hash(account),
        host_hint(result.get("href").and_then(Value::as_str)),
        title_class(result.get("title").and_then(Value::as_str))
    ));
}

/// Logs the result returned by the Gmail DOM polling script.
pub(crate) fn account_poll_result(
    topic: &str,
    account: &str,
    result: &Value,
    elapsed_ms: u128,
    timed_out: bool,
) {
    line(format!(
        "account_poll_result topic={topic} account_hash={} status={} elapsed_ms={elapsed_ms} timed_out={timed_out} href_host={} title_class={} empty={} rows={} candidates={} visible={} filtered={} selector_version={}",
        account_hash(account),
        result.get("status").and_then(Value::as_str).unwrap_or("ok"),
        host_hint(result.get("href").and_then(Value::as_str)),
        title_class(result.get("title").and_then(Value::as_str)),
        result.get("empty").and_then(Value::as_bool).unwrap_or(false),
        result.get("rows").and_then(Value::as_array).map_or(0, Vec::len),
        result.get("candidateRowCount").and_then(Value::as_u64).unwrap_or(0),
        result.get("visibleRowCount").and_then(Value::as_u64).unwrap_or(0),
        result.get("filteredRowCount").and_then(Value::as_u64).unwrap_or(0),
        result.get("selectorVersion").and_then(Value::as_str).unwrap_or("unknown")
    ));
}

/// Logs one completed poll cycle summary.
#[allow(clippy::too_many_arguments)]
pub(crate) fn poll_complete(
    topic: &str,
    successful: bool,
    observed_rows: usize,
    emitted_rows: usize,
    initialized_before: bool,
    initialized_after: bool,
    seen_count_before: usize,
    seen_count_after: usize,
) {
    line(format!(
        "poll_complete topic={topic} successful={successful} observed_rows={observed_rows} emitted_rows={emitted_rows} seen_initialized_before={initialized_before} seen_initialized_after={initialized_after} seen_count_before={seen_count_before} seen_count_after={seen_count_after}"
    ));
}

/// Logs a Gmail row event before it is emitted to the subscriber runtime.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_message(
    topic: &str,
    account: &str,
    dedup_key: &str,
    row: &Value,
    sender: &str,
    subject: &str,
    snippet: &str,
    text_len: usize,
) {
    line(format!(
        "emit_message topic={topic} account_hash={} dedup_hash={} has_sender={} has_subject={} has_snippet={} unread={} text_len={text_len}",
        account_hash(account),
        value_hash(dedup_key),
        !sender.trim().is_empty(),
        !subject.trim().is_empty(),
        !snippet.trim().is_empty(),
        row.get("unread").and_then(Value::as_bool).unwrap_or(false)
    ));
}

/// Returns a stable, non-PII fingerprint for a Gmail account identifier.
pub(crate) fn account_hash(account: &str) -> String {
    value_hash(&account.trim().to_ascii_lowercase())
}

/// Returns a stable, non-PII fingerprint for an event or row identifier.
pub(crate) fn value_hash(value: &str) -> String {
    format!("{:016x}", fnv1a64(value.as_bytes()))
}

/// Returns only the host portion of a URL-like value.
pub(crate) fn host_hint(value: Option<&str>) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return "none".to_string();
    };
    match url::Url::parse(value) {
        Ok(url) => url.host_str().unwrap_or("none").to_string(),
        Err(_) => "invalid".to_string(),
    }
}

/// Classifies a browser title without logging its raw contents.
pub(crate) fn title_class(value: Option<&str>) -> &'static str {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return "none";
    };
    let lower = value.to_ascii_lowercase();
    if lower.contains("sign in") || lower.contains("google accounts") {
        "signin"
    } else if lower.contains("gmail") || lower.contains("inbox") {
        "gmail"
    } else {
        "other"
    }
}

/// Returns the current wall-clock time in milliseconds for log correlation.
pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
