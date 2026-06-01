use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{ActionSpec, WorkflowBindingRun, WorkflowBindingSpec};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 500;
const SUMMARY_LIMIT: usize = 180;

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
    if let Some(summary) = string_field(payload.get("summary")) {
        return truncate_summary(&summary);
    }
    let subject = first_payload_string(payload, &["subject", "title", "event_title"]);
    let sender = first_payload_string(
        payload,
        &[
            "from_email",
            "sender_email",
            "sender_username",
            "author_handle",
            "from",
            "sender",
            "author",
        ],
    );
    let scope = first_payload_string(
        payload,
        &[
            "chat_title",
            "chat_name",
            "room_name",
            "channel_name",
            "calendar_id",
            "mailbox",
        ],
    );
    let body = first_payload_string(payload, &["message", "snippet", "text", "body"])
        .unwrap_or_else(|| text.trim().to_string());
    let combined = match (scope, sender, subject) {
        (Some(scope), Some(sender), Some(subject)) => format!("{scope} from {sender}: {subject}"),
        (Some(scope), Some(sender), None) => format!("{scope} from {sender}: {body}"),
        (None, Some(sender), Some(subject)) => format!("{sender}: {subject}"),
        (None, Some(sender), None) => format!("{sender}: {body}"),
        (_, None, Some(subject)) => subject,
        _ => body,
    };
    truncate_summary(&combined)
}

fn first_payload_string(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| string_field(payload.get(*key)))
}

fn truncate_summary(value: &str) -> String {
    let mut summary = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.chars().count() <= SUMMARY_LIMIT {
        return summary;
    }
    summary = summary.chars().take(SUMMARY_LIMIT).collect::<String>();
    format!("{}...", summary.trim_end())
}
