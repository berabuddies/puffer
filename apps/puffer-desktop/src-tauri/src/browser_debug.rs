//! Shared debug helpers for desktop Browser diagnostics.

use serde_json::Value;

/// Logs a native CEF diagnostic line when Browser debugging is enabled.
pub(crate) fn cef_log(event: impl AsRef<str>, details: impl AsRef<str>) {
    if !cef_enabled() {
        return;
    }
    eprintln!("[puffer-cef] {} {}", event.as_ref(), details.as_ref());
}

/// Logs a native CEF command result with a compact state summary.
pub(crate) fn cef_result(event: &str, session_id: &str, state: &Result<Value, String>) {
    match state {
        Ok(value) => cef_log(
            format!("{event} ok"),
            format!("session_id={} {}", session_id, cef_state_summary(value)),
        ),
        Err(error) => cef_log(
            format!("{event} error"),
            format!("session_id={} error={}", session_id, error),
        ),
    }
}

/// Formats a compact native CEF state summary for crash diagnostics.
pub(crate) fn cef_state_summary(value: &Value) -> String {
    format!(
        "connected={} url={} title={} loading={} error={}",
        value
            .get("connected")
            .and_then(Value::as_bool)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        value.get("url").and_then(Value::as_str).unwrap_or("-"),
        value.get("title").and_then(Value::as_str).unwrap_or("-"),
        value
            .get("loading")
            .and_then(Value::as_bool)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        value.get("error").and_then(Value::as_str).unwrap_or("-"),
    )
}

fn cef_enabled() -> bool {
    cfg!(debug_assertions) || std::env::var_os("PUFFER_BROWSER_DEBUG").is_some()
}
