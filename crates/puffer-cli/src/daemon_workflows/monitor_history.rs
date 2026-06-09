use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{ActionSpec, WorkflowBindingRun, WorkflowBindingSpec};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 500;
const SUMMARY_LIMIT: usize = 80;

/// Returns recent monitor-triggered connector messages and agent outcomes.
pub(crate) fn handle_monitor_history_list(_paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let manager = subscription_manager()?;
    let monitor_slugs = manager
        .store()
        .list()
        .into_iter()
        .filter(is_monitor_binding)
        .map(|binding| binding.slug)
        .collect::<BTreeSet<_>>();
    let messages = manager
        .history_store()
        .list()
        .into_iter()
        .filter(|run| monitor_slugs.contains(&run.workflow_slug) || run_has_monitor_action(run))
        .take(limit)
        .map(history_message_json)
        .collect::<Result<Vec<_>>>()?;
    eprintln!(
        "monitor-history: monitor_count={} message_count={} limit={limit}",
        monitor_slugs.len(),
        messages.len()
    );
    Ok(json!({ "messages": messages }))
}

fn is_monitor_binding(binding: &WorkflowBindingSpec) -> bool {
    binding.slug.starts_with("monitor-")
        || (matches!(binding.action, ActionSpec::TriageAgent { .. })
            && binding.description.to_ascii_lowercase().contains("monitor"))
}

fn run_has_monitor_action(run: &WorkflowBindingRun) -> bool {
    run.action_log.iter().any(|log| {
        log.action == "triage_agent"
            || log.action == "ignore_analysis_agent"
            || log.action.starts_with("monitor_")
    })
}

fn history_message_json(run: WorkflowBindingRun) -> Result<Value> {
    let trigger = run
        .trigger_info
        .as_object()
        .context("workflow history trigger_info must be an object")?;
    let payload = trigger.get("payload").cloned().unwrap_or(Value::Null);
    let text = trigger
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(json!({
        "idx": run.idx,
        "run_id": run.run_id,
        "workflow_slug": run.workflow_slug,
        "connection_slug": string_field(trigger.get("connection_slug")),
        "connector_slug": string_field(trigger.get("connector_slug")),
        "envelope_id": string_field(trigger.get("envelope_id")),
        "received_at_ms": trigger.get("received_at_ms").and_then(Value::as_i64),
        "topic": string_field(trigger.get("topic")),
        "kind": string_field(trigger.get("kind")),
        "dedup_key": trigger.get("dedup_key").and_then(Value::as_str),
        "summary": message_summary(&payload, &text),
        "text": text,
        "payload": payload,
        "action_log": run.action_log,
        "status": run.status,
        "started_at_ms": run.started_at_ms,
        "ended_at_ms": run.ended_at_ms,
    }))
}

fn string_field(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn message_summary(payload: &Value, text: &str) -> String {
    let candidate = first_payload_string(
        payload,
        &[
            "message",
            "snippet",
            "text",
            "body",
            "subject",
            "title",
            "event_title",
            "summary",
        ],
    )
    .unwrap_or_else(|| text.trim().to_string());
    let cleaned = strip_source_prefixes(&normalize_inline_text(&candidate));
    let preview = error_preview(&cleaned)
        .or_else(|| question_preview(&cleaned))
        .or_else(|| request_preview(&cleaned))
        .unwrap_or(cleaned);
    truncate_summary(&preview)
}

fn first_payload_string(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| string_field(payload.get(*key)))
}

fn strip_source_prefixes(value: &str) -> String {
    let mut current = value.trim().to_string();
    for _ in 0..4 {
        let next = strip_one_source_prefix(&current);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn strip_one_source_prefix(value: &str) -> String {
    let text = value.trim();
    let lower = text.to_ascii_lowercase();
    for prefix in [
        "telegram message from ",
        "incoming telegram message from ",
        "incoming telegram message",
        "telegram user sent",
        "in telegram group ",
        "message from chat_id ",
    ] {
        if lower.starts_with(prefix) {
            return strip_prefix_subject(&text[prefix.len()..]);
        }
    }
    strip_generic_colon_source(text).unwrap_or_else(|| text.to_string())
}

fn strip_prefix_subject(value: &str) -> String {
    let rest = value
        .trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, ':' | '-' | ','));
    if let Some((_, tail)) = rest.split_once(':') {
        return tail.trim().to_string();
    }
    rest.trim().to_string()
}

fn strip_generic_colon_source(value: &str) -> Option<String> {
    let (head, tail) = value.split_once(':')?;
    if head.chars().count() > 80 {
        return None;
    }
    let lower = head.to_ascii_lowercase();
    let looks_like_source = [
        "telegram", "slack", "discord", "lark", "email", "mail", "chat_id", "channel", "room",
        "group", " from ",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    looks_like_source.then(|| tail.trim().to_string())
}

fn error_preview(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let mut earliest = None;
    for marker in [
        "error:",
        "failed:",
        "failure:",
        "fatal:",
        "panic:",
        "exception:",
        "traceback",
        "could not ",
        "cannot ",
    ] {
        if let Some(index) = lower.find(marker) {
            earliest = Some(earliest.map_or(index, |current: usize| current.min(index)));
        }
    }
    let index = earliest?;
    let excerpt = first_sentence(&value[index..]);
    let core = strip_error_label(&excerpt);
    (!core.trim().is_empty()).then_some(core)
}

fn strip_error_label(value: &str) -> String {
    let trimmed =
        value.trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, '-' | ':'));
    if let Some((label, tail)) = trimmed.split_once(':') {
        let lower = label.to_ascii_lowercase();
        if label.chars().count() <= 48
            && [
                "error",
                "failed",
                "failure",
                "fatal",
                "panic",
                "exception",
                "traceback",
            ]
            .iter()
            .any(|marker| lower.contains(marker))
        {
            return tail.trim().to_string();
        }
    }
    trimmed.to_string()
}

fn question_preview(value: &str) -> Option<String> {
    if let Some(index) = value.find('?') {
        return Some(first_sentence(&value[..=index]));
    }
    let lower = value.to_ascii_lowercase();
    [
        "can you ",
        "could you ",
        "would you ",
        "do you ",
        "is there ",
        "are there ",
        "what ",
        "why ",
        "how ",
        "when ",
        "where ",
        "should ",
    ]
    .iter()
    .any(|marker| lower.starts_with(marker))
    .then(|| first_sentence(value))
}

fn request_preview(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    for prefix in ["please ", "pls "] {
        if lower.starts_with(prefix) {
            return Some(first_sentence(value[prefix.len()..].trim()));
        }
    }
    [
        "need ",
        "need you to ",
        "restart ",
        "fix ",
        "check ",
        "review ",
        "send ",
        "update ",
        "create ",
        "delete ",
        "deploy ",
    ]
    .iter()
    .any(|marker| lower.starts_with(marker))
    .then(|| first_sentence(value))
}

fn first_sentence(value: &str) -> String {
    for separator in [". ", "; ", " | "] {
        if let Some((head, _)) = value.split_once(separator) {
            return head.trim().to_string();
        }
    }
    value.trim().to_string()
}

fn normalize_inline_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_summary(value: &str) -> String {
    let summary = normalize_inline_text(value);
    if summary.chars().count() <= SUMMARY_LIMIT {
        return summary;
    }
    let truncated = summary.chars().take(SUMMARY_LIMIT).collect::<String>();
    let trimmed = truncated
        .rsplit_once(' ')
        .map(|(head, _)| head)
        .unwrap_or(truncated.as_str())
        .trim_end();
    format!("{trimmed}...")
}

#[cfg(test)]
mod tests {
    use super::message_summary;
    use serde_json::json;

    #[test]
    fn message_summary_prefers_body_over_source_fields() {
        let payload = json!({
            "chat_title": "Support",
            "sender_username": "alice",
            "message": "deployment status?"
        });

        assert_eq!(message_summary(&payload, ""), "deployment status?");
    }

    #[test]
    fn message_summary_strips_telegram_source_prefixes() {
        let payload = json!({
            "summary": "Telegram message from Alice: deployment status?"
        });

        assert_eq!(message_summary(&payload, ""), "deployment status?");
    }

    #[test]
    fn message_summary_extracts_core_error_text() {
        let payload = json!({
            "message": "Incoming Telegram message from Alice: Error: deployment failed in production"
        });

        assert_eq!(
            message_summary(&payload, ""),
            "deployment failed in production"
        );
    }

    #[test]
    fn message_summary_keeps_question_intent() {
        let payload = json!({
            "channel_name": "#ops",
            "author_handle": "sam",
            "text": "Can you check the failing deploy?"
        });

        assert_eq!(
            message_summary(&payload, ""),
            "Can you check the failing deploy?"
        );
    }

    #[test]
    fn message_summary_keeps_request_action() {
        let payload = json!({
            "from_email": "ops@example.com",
            "body": "please restart the staging worker when the deploy is done"
        });

        assert_eq!(
            message_summary(&payload, ""),
            "restart the staging worker when the deploy is done"
        );
    }

    #[test]
    fn message_summary_falls_back_to_short_clean_content() {
        let payload = json!({
            "body": "This is a casual note with enough words that the preview should stay short and readable for the sidebar row."
        });

        assert_eq!(
            message_summary(&payload, ""),
            "This is a casual note with enough words that the preview should stay short and..."
        );
    }
}
