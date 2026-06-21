use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{ActionSpec, WorkflowBindingRun, WorkflowBindingSpec};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};

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
    let runs = manager
        .history_store()
        .list()
        .into_iter()
        .filter(|run| monitor_slugs.contains(&run.workflow_slug) || run_has_monitor_action(run))
        .collect::<Vec<_>>();
    let digest_batches = digest_batch_index(&runs);
    let messages = runs
        .into_iter()
        .take(limit)
        .map(|run| history_message_json(run, &digest_batches))
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

#[derive(Clone)]
struct DigestBatchInfo {
    id: String,
    count: usize,
    position: usize,
}

fn digest_batch_index(runs: &[WorkflowBindingRun]) -> HashMap<u64, DigestBatchInfo> {
    let mut groups: HashMap<String, Vec<u64>> = HashMap::new();
    for run in runs {
        let Some(action) = run
            .action_log
            .iter()
            .find(|log| log.action == "triage_agent")
        else {
            continue;
        };
        let key = format!(
            "{}:{}:{}:{}",
            run.workflow_slug, action.started_at_ms, action.ended_at_ms, action.summary
        );
        groups.entry(key).or_default().push(run.idx);
    }
    let mut batches = HashMap::new();
    for (key, mut indexes) in groups {
        if indexes.len() <= 1 {
            continue;
        }
        indexes.sort_unstable();
        let count = indexes.len();
        let id = format!("digest-{:x}", stable_hash(&key));
        for (position, idx) in indexes.into_iter().enumerate() {
            batches.insert(
                idx,
                DigestBatchInfo {
                    id: id.clone(),
                    count,
                    position: position + 1,
                },
            );
        }
    }
    batches
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn history_message_json(
    run: WorkflowBindingRun,
    digest_batches: &HashMap<u64, DigestBatchInfo>,
) -> Result<Value> {
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
    let digest_batch = digest_batches.get(&run.idx);
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
        "digest_batch_id": digest_batch.map(|batch| batch.id.clone()),
        "digest_batch_count": digest_batch.map(|batch| batch.count),
        "digest_batch_position": digest_batch.map(|batch| batch.position),
        "digest_outcome_shared": digest_batch.is_some(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_subscriptions::{WorkflowActionLog, WorkflowBindingRunStatus};

    #[test]
    fn digest_batch_index_groups_shared_triage_outcomes() {
        let runs = vec![
            run(11, "first", 100, 200, "Created one task."),
            run(12, "second", 100, 200, "Created one task."),
            run(13, "third", 300, 400, "Created another task."),
        ];

        let index = digest_batch_index(&runs);

        assert_eq!(index[&11].count, 2);
        assert_eq!(index[&11].position, 1);
        assert_eq!(index[&12].count, 2);
        assert_eq!(index[&12].position, 2);
        assert!(!index.contains_key(&13));
    }

    fn run(
        idx: u64,
        text: &str,
        action_started_at_ms: i128,
        action_ended_at_ms: i128,
        summary: &str,
    ) -> WorkflowBindingRun {
        WorkflowBindingRun {
            idx,
            run_id: format!("run-{idx}"),
            workflow_slug: "monitor-telegram-user".into(),
            trigger_info: json!({ "text": text, "payload": { "message": text } }),
            action_summary: json!({ "summary": summary }),
            action_log: vec![WorkflowActionLog {
                action: "triage_agent".into(),
                status: WorkflowBindingRunStatus::Completed,
                summary: summary.into(),
                started_at_ms: action_started_at_ms,
                ended_at_ms: action_ended_at_ms,
                usage: None,
            }],
            status: WorkflowBindingRunStatus::Completed,
            started_at_ms: action_started_at_ms - 1,
            ended_at_ms: action_ended_at_ms + 1,
        }
    }
}
