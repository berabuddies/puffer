//! Shared post-condition verification helpers for browser connector actions.
//!
//! Change-type actions (send / delete / mark-read / RSVP) must re-navigate to
//! an authoritative view and assert the real outcome before reporting
//! success. Failures are built through [`verification_failure`] so the error
//! string carries the diagnostics (href/title/status/reason) returned by the
//! probe script or list poll -- the action error channel flattens errors to a
//! string, so diagnostics must live in the message itself.

use serde_json::Value;

/// Formats a post-condition verification failure, embedding the diagnostic
/// fields returned by the probe script or list poll into the error message.
pub(crate) fn verification_failure(
    action: &str,
    expectation: &str,
    result: &Value,
) -> anyhow::Error {
    let field = |key: &str| result.get(key).and_then(Value::as_str).unwrap_or("unknown");
    anyhow::anyhow!(
        "Browser action `{action}` could not be verified: {expectation}; last URL `{}`, title `{}`, status `{}`, reason `{}`",
        field("href"),
        field("title"),
        field("status"),
        field("reason"),
    )
}

/// Returns true when a Gmail list row refers to the given thread id.
///
/// Action inputs may carry any of the id forms the inbox script extracts
/// (legacy hex, `thread-f:...` raw ids, or a `#`-prefixed hash fragment), so
/// the row's `threadId`/`legacyThreadId`/`gmailThreadId`/`id` fields are all
/// compared after normalization.
pub(crate) fn row_matches_thread(row: &Value, thread_id: &str) -> bool {
    let expected = normalize_thread_id(thread_id);
    if expected.is_empty() {
        return false;
    }
    ["threadId", "legacyThreadId", "gmailThreadId", "id"]
        .iter()
        .filter_map(|key| row.get(*key).and_then(Value::as_str))
        .map(normalize_thread_id)
        .any(|value| !value.is_empty() && value == expected)
}

fn normalize_thread_id(value: &str) -> String {
    value.trim().trim_start_matches('#').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn verification_failure_embeds_diagnostics_and_defaults() {
        let result = json!({
            "href": "https://mail.google.com/mail/#sent",
            "status": "loading",
            "reason": "rows empty"
        });
        let message = format!(
            "{:#}",
            verification_failure("send_email", "email not in Sent", &result)
        );
        assert!(message.contains("send_email"));
        assert!(message.contains("email not in Sent"));
        assert!(message.contains("https://mail.google.com/mail/#sent"));
        assert!(message.contains("status `loading`"));
        assert!(message.contains("title `unknown`"));
    }

    #[test]
    fn row_matches_thread_normalizes_id_forms() {
        let row = json!({
            "threadId": "18cabc123def4567",
            "legacyThreadId": "18cabc123def4567",
            "gmailThreadId": "#thread-f:1789012345678901234",
            "id": "18cabc123def4567"
        });
        assert!(row_matches_thread(&row, "18CABC123DEF4567"));
        assert!(row_matches_thread(&row, "thread-f:1789012345678901234"));
        assert!(row_matches_thread(&row, "#18cabc123def4567"));
        assert!(!row_matches_thread(&row, "somethingelse"));
        assert!(!row_matches_thread(&row, ""));
        assert!(!row_matches_thread(&json!({}), "18cabc123def4567"));
    }
}
