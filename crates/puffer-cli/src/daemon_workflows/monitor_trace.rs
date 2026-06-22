use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{connection_subscriber_manifest, ConnectionRecord, SubscriptionManager};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 1000;
const MAX_DIAGNOSTIC_RECORDS: usize = 1000;
const MAX_DIAGNOSTIC_BYTES: u64 = 8 * 1024 * 1024;
const SUMMARY_LIMIT: usize = 180;
const TEXT_PREVIEW_LIMIT: usize = 200;

#[derive(Debug, Deserialize)]
struct MonitorTraceListParams {
    #[serde(default, alias = "connectionSlug")]
    connection_slug: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default, alias = "includePayload")]
    include_payload: bool,
}

/// Returns the per-Telegram-message monitor pipeline trace.
pub(crate) fn handle_monitor_trace_list(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: MonitorTraceListParams =
        serde_json::from_value(params.clone()).context("invalid monitor trace list params")?;
    let connection_slug = params
        .connection_slug
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let manager = subscription_manager()?;

    let mut messages = manager
        .monitor_trace_store()
        .list_recent(connection_slug, limit)
        .into_iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .context("serialize monitor trace messages")?;

    let diagnostics = read_subscriber_diagnostics(paths, manager.as_ref(), connection_slug)?;
    merge_diagnostic_records(&mut messages, diagnostics, params.include_payload);
    add_monitor_task_links(paths, &mut messages)?;
    finalize_messages(&mut messages, params.include_payload);
    messages.sort_by(|left, right| {
        last_activity_ms(right)
            .cmp(&last_activity_ms(left))
            .then_with(|| message_key(left).cmp(&message_key(right)))
    });
    messages.truncate(limit);

    Ok(json!({
        "messages": messages,
        "payloads_included": params.include_payload,
    }))
}

fn read_subscriber_diagnostics(
    paths: &ConfigPaths,
    manager: &SubscriptionManager,
    connection_slug: Option<&str>,
) -> Result<Vec<Value>> {
    let connections = diagnostic_connections(manager, connection_slug);
    let mut records = Vec::new();
    let mut bytes_read = 0_u64;
    for connection in connections {
        for path in diagnostic_paths_for_connection(paths, manager, &connection) {
            if bytes_read >= MAX_DIAGNOSTIC_BYTES {
                break;
            }
            read_diagnostic_file(
                &path,
                MAX_DIAGNOSTIC_BYTES.saturating_sub(bytes_read),
                &mut bytes_read,
                &mut records,
            )?;
        }
    }
    trim_diagnostic_records(&mut records);
    Ok(records)
}

fn diagnostic_connections(
    manager: &SubscriptionManager,
    connection_slug: Option<&str>,
) -> Vec<ConnectionRecord> {
    match connection_slug {
        Some(connection_slug) => manager
            .connection_store()
            .get(connection_slug)
            .into_iter()
            .collect(),
        None => manager.connection_store().list(),
    }
}

fn diagnostic_paths_for_connection(
    paths: &ConfigPaths,
    manager: &SubscriptionManager,
    connection: &ConnectionRecord,
) -> Vec<PathBuf> {
    let base = subscriber_diagnostic_path(paths, manager, connection)
        .unwrap_or_else(|| fallback_subscriber_diagnostic_path(paths, &connection.slug));
    let mut paths = vec![base.clone()];
    for generation in 1..=3 {
        paths.push(PathBuf::from(format!("{}.{}", base.display(), generation)));
    }
    paths
}

fn subscriber_diagnostic_path(
    paths: &ConfigPaths,
    manager: &SubscriptionManager,
    connection: &ConnectionRecord,
) -> Option<PathBuf> {
    let template = manager.connector_store().get(&connection.connector_slug)?;
    let manifest = connection_subscriber_manifest(
        &super::subscriber_manifest_roots(paths),
        connection,
        &template,
    )
    .ok()
    .flatten()?;
    let state = manifest.spec.state?;
    let state_dir = if Path::new(&state.dir).is_absolute() {
        PathBuf::from(state.dir)
    } else {
        manifest.dir.join(state.dir)
    };
    Some(state_dir.join("message-diagnostics.ndjson"))
}

fn fallback_subscriber_diagnostic_path(paths: &ConfigPaths, connection_slug: &str) -> PathBuf {
    paths
        .user_config_dir
        .join("state")
        .join(connection_slug)
        .join("message-diagnostics.ndjson")
}

fn read_diagnostic_file(
    path: &Path,
    remaining_bytes: u64,
    bytes_read: &mut u64,
    records: &mut Vec<Value>,
) -> Result<()> {
    if remaining_bytes == 0 {
        return Ok(());
    }
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("stat {}", path.display()));
        }
    };
    if meta.len() > remaining_bytes && *bytes_read > 0 {
        return Ok(());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    *bytes_read = bytes_read.saturating_add(meta.len());
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<Value>(line) {
            Ok(value) => records.push(value),
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "skipping invalid telegram diagnostic line"
                );
            }
        }
    }
    Ok(())
}

fn trim_diagnostic_records(records: &mut Vec<Value>) {
    if records.len() <= MAX_DIAGNOSTIC_RECORDS {
        return;
    }
    records.sort_by(|left, right| {
        i128_value(right, &["at_ms"])
            .cmp(&i128_value(left, &["at_ms"]))
            .then_with(|| diagnostic_message_key(right).cmp(&diagnostic_message_key(left)))
    });
    records.truncate(MAX_DIAGNOSTIC_RECORDS);
}

fn merge_diagnostic_records(
    messages: &mut Vec<Value>,
    diagnostics: Vec<Value>,
    include_payload: bool,
) {
    let mut by_key = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        if let Some(key) = message_key(message) {
            by_key.insert(key, index);
        }
    }

    for record in diagnostics {
        let Some(key) = diagnostic_message_key(&record) else {
            continue;
        };
        let stage = diagnostic_stage(&record);
        if let Some(index) = by_key.get(&key).copied() {
            merge_diagnostic_record_into_message(
                &mut messages[index],
                &record,
                stage,
                include_payload,
            );
            continue;
        }
        let message = diagnostic_message(&record, stage, include_payload);
        by_key.insert(key, messages.len());
        messages.push(message);
    }
}

fn merge_diagnostic_record_into_message(
    message: &mut Value,
    record: &Value,
    stage: Value,
    include_payload: bool,
) {
    let Some(object) = message.as_object_mut() else {
        return;
    };
    fill_missing_string(object, "connection_slug", record, &["connection_slug"]);
    fill_missing_string(object, "connector_slug", record, &["connector_slug"]);
    fill_missing_string(object, "topic", record, &["connection_slug", "topic"]);
    fill_missing_string(object, "kind", record, &["kind"]);
    fill_missing_string(object, "chat_id", record, &["chat_id"]);
    fill_missing_string(
        object,
        "chat_title",
        record,
        &["chat_title", "chat_username"],
    );
    fill_missing_string(object, "sender_id", record, &["sender_id"]);
    fill_missing_string(
        object,
        "sender_name",
        record,
        &["sender_name", "sender_username"],
    );
    fill_missing_string(object, "message_id", record, &["message_id"]);
    fill_missing_string(object, "dedup_key", record, &["dedup_key"]);
    fill_missing_string(object, "text", record, &["text_prefix"]);
    fill_missing_i128(object, "event_date_ms", record, &["date_ms"]);
    fill_missing_i128(
        object,
        "received_at_ms",
        record,
        &["source_received_at_ms", "at_ms"],
    );
    push_stage(object, stage);
    set_payload_fields(object, include_payload, Some(record));
}

fn diagnostic_message(record: &Value, stage: Value, include_payload: bool) -> Value {
    let mut object = Map::new();
    object.insert(
        "message_key".to_string(),
        Value::String(diagnostic_message_key(record).unwrap_or_default()),
    );
    object.insert(
        "connection_slug".to_string(),
        string_value(record, &["connection_slug"])
            .unwrap_or_else(|| "unknown".to_string())
            .into(),
    );
    insert_string(
        &mut object,
        "connector_slug",
        string_value(record, &["connector_slug"]),
    );
    insert_string(
        &mut object,
        "topic",
        string_value(record, &["connection_slug", "topic"]),
    );
    insert_string(&mut object, "kind", string_value(record, &["kind"]));
    insert_string(&mut object, "chat_id", string_value(record, &["chat_id"]));
    insert_string(
        &mut object,
        "chat_title",
        string_value(record, &["chat_title", "chat_username"]),
    );
    insert_string(
        &mut object,
        "sender_id",
        string_value(record, &["sender_id"]),
    );
    insert_string(
        &mut object,
        "sender_name",
        string_value(record, &["sender_name", "sender_username"]),
    );
    insert_string(
        &mut object,
        "message_id",
        string_value(record, &["message_id"]),
    );
    insert_string(
        &mut object,
        "dedup_key",
        string_value(record, &["dedup_key"]),
    );
    insert_string(&mut object, "text", string_value(record, &["text_prefix"]));
    insert_i128(
        &mut object,
        "event_date_ms",
        i128_value(record, &["date_ms"]),
    );
    insert_i128(
        &mut object,
        "received_at_ms",
        i128_value(record, &["source_received_at_ms", "at_ms"]),
    );
    object.insert(
        "latest_status".to_string(),
        Value::String("received".to_string()),
    );
    object.insert("terminal_reason".to_string(), Value::Null);
    object.insert("stages".to_string(), Value::Array(vec![stage]));
    set_payload_fields(&mut object, include_payload, Some(record));
    Value::Object(object)
}

fn diagnostic_stage(record: &Value) -> Value {
    let id =
        string_value(record, &["stage"]).unwrap_or_else(|| "telegram_update_received".to_string());
    let status = if id == "delivery_emit_failed"
        || record.get("error").is_some_and(|value| !value.is_null())
    {
        "failed"
    } else {
        "completed"
    };
    json!({
        "id": id,
        "status": status,
        "at_ms": i128_value(record, &["at_ms"]).unwrap_or_default(),
        "source": "telegram_subscriber",
        "summary": diagnostic_stage_summary(record),
        "raw_source": string_value(record, &["delivery_source"]),
    })
}

fn diagnostic_stage_summary(record: &Value) -> String {
    let stage = string_value(record, &["stage"]).unwrap_or_default();
    if let Some(error) = string_value(record, &["error"]) {
        return format!("Telegram subscriber failed to emit the message: {error}");
    }
    match stage.as_str() {
        "telegram_update_received" => "Telegram subscriber received the update.".to_string(),
        "delivery_duplicate" => "Telegram subscriber skipped a duplicate message.".to_string(),
        "delivery_suppressed" => {
            let muted = bool_value(record, "notification_muted");
            let silent = bool_value(record, "notification_silent");
            format!("Telegram subscriber suppressed the message (muted={muted}, silent={silent}).")
        }
        "delivery_emitted" => "Telegram subscriber emitted the message to puffer.".to_string(),
        "delivery_emit_failed" => "Telegram subscriber failed to emit the message.".to_string(),
        _ => format!("Telegram subscriber recorded `{stage}`."),
    }
}

fn add_monitor_task_links(paths: &ConfigPaths, messages: &mut [Value]) -> Result<()> {
    let tasks = load_monitor_tasks(paths)?;
    if tasks.is_empty() {
        return Ok(());
    }
    for message in messages {
        let linked = linked_tasks_for_message(message, &tasks);
        if linked.is_empty() {
            continue;
        }
        if let Some(object) = message.as_object_mut() {
            object.insert("linked_tasks".to_string(), Value::Array(linked.clone()));
            if !has_stage(object, "task_created") && !has_stage(object, "task_updated") {
                push_stage(
                    object,
                    json!({
                        "id": "task_created",
                        "status": "completed",
                        "at_ms": linked_task_activity_ms(&linked).unwrap_or_else(|| last_activity_ms(&Value::Object(object.clone()))),
                        "source": "monitor_task",
                        "summary": format!("Linked monitor task {}.", linked[0].get("task_id").and_then(Value::as_str).unwrap_or("")),
                    }),
                );
            }
        }
    }
    Ok(())
}

fn load_monitor_tasks(paths: &ConfigPaths) -> Result<Vec<Value>> {
    let path = super::monitor_task_ignore::monitor_tasks_path(paths);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let value: Value =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    if let Some(tasks) = value.as_array() {
        return Ok(tasks.clone());
    }
    Ok(value
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn linked_tasks_for_message(message: &Value, tasks: &[Value]) -> Vec<Value> {
    let envelope_id = string_value(message, &["envelope_id"]);
    let mut linked = Vec::new();
    for task in tasks {
        if !task_matches_message(task, envelope_id.as_deref()) {
            continue;
        }
        linked.push(json!({
            "task_id": task_identifier(task),
            "subject": string_value(task, &["subject", "title"]).unwrap_or_default(),
            "status": string_value(task, &["status"]).unwrap_or_else(|| "unknown".to_string()),
            "created_at_ms": i128_value(task, &["created_at_ms", "createdAtMs"]),
            "updated_at_ms": i128_value(task, &["updated_at_ms", "updatedAtMs"]),
        }));
    }
    linked
}

fn task_matches_message(task: &Value, envelope_id: Option<&str>) -> bool {
    if let Some(envelope_id) = envelope_id {
        if task_string_in_metadata(task, &["monitor_envelope_id", "monitorEnvelopeId"]).as_deref()
            == Some(envelope_id)
        {
            return true;
        }
    }
    false
}

fn task_string_in_metadata(task: &Value, keys: &[&str]) -> Option<String> {
    string_value(task, keys).or_else(|| {
        task.get("metadata")
            .and_then(|metadata| string_value(metadata, keys))
    })
}

fn task_identifier(task: &Value) -> String {
    string_value(task, &["task_id", "taskId", "id"]).unwrap_or_else(|| "unknown".to_string())
}

fn linked_task_activity_ms(linked: &[Value]) -> Option<i128> {
    linked
        .iter()
        .filter_map(|task| i128_value(task, &["updated_at_ms", "created_at_ms"]))
        .max()
}

fn finalize_messages(messages: &mut [Value], include_payload: bool) {
    for message in messages {
        let Some(object) = message.as_object_mut() else {
            continue;
        };
        if !object.contains_key("linked_tasks") {
            object.insert("linked_tasks".to_string(), Value::Array(Vec::new()));
        }
        if !include_payload {
            bound_text_field(object);
        }
        if !object.contains_key("summary") {
            object.insert(
                "summary".to_string(),
                Value::String(message_summary(object)),
            );
        }
        set_payload_fields(object, include_payload, None);
        let stages = object
            .get("stages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        object.insert(
            "latest_status".to_string(),
            Value::String(derive_status(&stages).to_string()),
        );
        object.insert(
            "terminal_reason".to_string(),
            terminal_reason(&stages)
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        object.insert(
            "triage_decision".to_string(),
            latest_triage_decision(&stages).unwrap_or(Value::Null),
        );
    }
}

fn latest_triage_decision(stages: &[Value]) -> Option<Value> {
    stages.iter().rev().find_map(|stage| {
        if stage.get("id").and_then(Value::as_str) != Some("triage_decision") {
            return None;
        }
        stage
            .get("decision")
            .filter(|decision| decision.is_object())
            .cloned()
    })
}

fn set_payload_fields(
    object: &mut Map<String, Value>,
    include_payload: bool,
    record: Option<&Value>,
) {
    if let Some(record) = record {
        object.insert("payload_available".to_string(), Value::Bool(true));
        object.insert("payload_truncated".to_string(), Value::Bool(false));
        object.insert("payload_redacted".to_string(), Value::Bool(true));
        object.insert(
            "payload".to_string(),
            include_payload
                .then(|| redact_value(record))
                .unwrap_or(Value::Null),
        );
        return;
    }
    object
        .entry("payload_available")
        .or_insert(Value::Bool(false));
    object
        .entry("payload_truncated")
        .or_insert(Value::Bool(false));
    object
        .entry("payload_redacted")
        .or_insert(Value::Bool(true));
    object.entry("payload").or_insert(Value::Null);
    if !include_payload {
        object.insert("payload".to_string(), Value::Null);
    }
}

fn bound_text_field(object: &mut Map<String, Value>) {
    let Some(text) = object.get("text").and_then(Value::as_str) else {
        return;
    };
    object.insert(
        "text".to_string(),
        Value::String(truncate_text(text, TEXT_PREVIEW_LIMIT)),
    );
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        truncated.push_str("...");
    }
    truncated
}

fn push_stage(object: &mut Map<String, Value>, stage: Value) {
    let Some(stage_id) = stage.get("id").and_then(Value::as_str).map(str::to_string) else {
        return;
    };
    let stages = object
        .entry("stages")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut();
    let Some(stages) = stages else {
        return;
    };
    if let Some(index) = stages
        .iter()
        .position(|existing| existing.get("id").and_then(Value::as_str) == Some(stage_id.as_str()))
    {
        stages[index] = stage;
    } else {
        stages.push(stage);
    }
    stages.sort_by(|left, right| i128_value(left, &["at_ms"]).cmp(&i128_value(right, &["at_ms"])));
}

fn has_stage(object: &Map<String, Value>, stage_id: &str) -> bool {
    object
        .get("stages")
        .and_then(Value::as_array)
        .is_some_and(|stages| {
            stages
                .iter()
                .any(|stage| stage.get("id").and_then(Value::as_str) == Some(stage_id))
        })
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TraceStatus {
    Received,
    Suppressed,
    Emitted,
    RouterSkipped,
    DigestWaiting,
    TriageRunning,
    TriagedNoTask,
    TaskCreated,
    TaskUpdated,
    Failed,
}

impl std::fmt::Display for TraceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TraceStatus::Received => "received",
            TraceStatus::Suppressed => "suppressed",
            TraceStatus::Emitted => "emitted",
            TraceStatus::RouterSkipped => "router_skipped",
            TraceStatus::DigestWaiting => "digest_waiting",
            TraceStatus::TriageRunning => "triage_running",
            TraceStatus::TriagedNoTask => "triaged_no_task",
            TraceStatus::TaskCreated => "task_created",
            TraceStatus::TaskUpdated => "task_updated",
            TraceStatus::Failed => "failed",
        })
    }
}

fn derive_status(stages: &[Value]) -> TraceStatus {
    if stage_exists(stages, "task_created") {
        return TraceStatus::TaskCreated;
    }
    if stage_exists(stages, "task_updated") || stage_exists(stages, "reply_sent") {
        return TraceStatus::TaskUpdated;
    }
    if stages
        .iter()
        .any(|stage| stage.get("status").and_then(Value::as_str) == Some("failed"))
        || stage_exists(stages, "delivery_emit_failed")
    {
        return TraceStatus::Failed;
    }
    if stage_exists(stages, "triage_completed") {
        return TraceStatus::TriagedNoTask;
    }
    if stage_exists(stages, "triage_started") {
        return TraceStatus::TriageRunning;
    }
    if stage_exists(stages, "router_digest_queued") {
        return TraceStatus::DigestWaiting;
    }
    if stages.iter().any(|stage| {
        stage
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(matches_router_skip_stage)
    }) {
        return TraceStatus::RouterSkipped;
    }
    if stage_exists(stages, "delivery_duplicate") || stage_exists(stages, "delivery_suppressed") {
        return TraceStatus::Suppressed;
    }
    if stage_exists(stages, "delivery_emitted") || stage_exists(stages, "connector_stdout_received")
    {
        return TraceStatus::Emitted;
    }
    TraceStatus::Received
}

fn stage_exists(stages: &[Value], stage_id: &str) -> bool {
    stages
        .iter()
        .any(|stage| stage.get("id").and_then(Value::as_str) == Some(stage_id))
}

fn matches_router_skip_stage(id: &str) -> bool {
    matches!(
        id,
        "router_no_monitor_binding"
            | "router_binding_paused"
            | "router_dedup_seen"
            | "router_self_gate_skipped"
            | "router_muted_skip"
            | "router_ignore_filter"
            | "router_contact_filter_skip"
            | "router_filter_skip"
            | "router_classifier_skip"
    )
}

fn terminal_reason(stages: &[Value]) -> Option<String> {
    stages
        .iter()
        .rev()
        .find(|stage| {
            stage.get("status").and_then(Value::as_str) == Some("failed")
                || stage.get("id").and_then(Value::as_str).is_some_and(|id| {
                    matches_router_skip_stage(id)
                        || id == "delivery_duplicate"
                        || id == "delivery_suppressed"
                        || id == "delivery_emit_failed"
                })
        })
        .and_then(|stage| string_value(stage, &["summary"]))
}

fn diagnostic_message_key(record: &Value) -> Option<String> {
    string_value(record, &["message_key"]).or_else(|| {
        let connection_slug = string_value(record, &["connection_slug"])?;
        let chat_id = string_value(record, &["chat_id"]);
        let message_id = string_value(record, &["message_id"]);
        match (chat_id, message_id) {
            (Some(chat_id), Some(message_id)) => {
                Some(format!("{connection_slug}:{chat_id}:{message_id}"))
            }
            _ => string_value(record, &["dedup_key"])
                .map(|dedup_key| format!("{connection_slug}:{dedup_key}")),
        }
    })
}

fn message_key(message: &Value) -> Option<String> {
    string_value(message, &["message_key"])
}

fn last_activity_ms(message: &Value) -> i128 {
    let stage_ms = message
        .get("stages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|stage| i128_value(stage, &["at_ms"]))
        .max();
    stage_ms
        .or_else(|| i128_value(message, &["received_at_ms"]))
        .or_else(|| i128_value(message, &["event_date_ms"]))
        .unwrap_or_default()
}

fn message_summary(object: &Map<String, Value>) -> String {
    let sender = object
        .get("sender_name")
        .and_then(Value::as_str)
        .or_else(|| object.get("sender_id").and_then(Value::as_str));
    let chat = object
        .get("chat_title")
        .and_then(Value::as_str)
        .or_else(|| object.get("chat_id").and_then(Value::as_str));
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let summary = match (chat, sender) {
        (Some(chat), Some(sender)) if !text.is_empty() => format!("{chat} from {sender}: {text}"),
        (_, Some(sender)) if !text.is_empty() => format!("{sender}: {text}"),
        (Some(chat), _) if !text.is_empty() => format!("{chat}: {text}"),
        _ => text.to_string(),
    };
    truncate_summary(&summary)
}

fn truncate_summary(value: &str) -> String {
    let mut summary = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.chars().count() <= SUMMARY_LIMIT {
        return summary;
    }
    summary = summary.chars().take(SUMMARY_LIMIT).collect::<String>();
    format!("{}...", summary.trim_end())
}

fn fill_missing_string(
    object: &mut Map<String, Value>,
    output_key: &str,
    source: &Value,
    source_keys: &[&str],
) {
    if object.get(output_key).is_some_and(|value| !value.is_null()) {
        return;
    }
    insert_string(object, output_key, string_value(source, source_keys));
}

fn fill_missing_i128(
    object: &mut Map<String, Value>,
    output_key: &str,
    source: &Value,
    source_keys: &[&str],
) {
    if object.get(output_key).is_some_and(|value| !value.is_null()) {
        return;
    }
    insert_i128(object, output_key, i128_value(source, source_keys));
}

fn insert_string(object: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        object.insert(key.to_string(), Value::String(value));
    }
}

fn insert_i128(object: &mut Map<String, Value>, key: &str, value: Option<i128>) {
    if let Some(value) = value {
        object.insert(key.to_string(), json_i128(value));
    }
}

fn json_i128(value: i128) -> Value {
    match i64::try_from(value) {
        Ok(value) => Value::from(value),
        Err(_) => Value::String(value.to_string()),
    }
}

fn string_value(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    keys.iter().find_map(|key| scalar_string(object.get(*key)))
}

fn scalar_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn i128_value(value: &Value, keys: &[&str]) -> Option<i128> {
    let object = value.as_object()?;
    keys.iter().find_map(|key| match object.get(*key)? {
        Value::Number(number) => number
            .as_i64()
            .map(i128::from)
            .or_else(|| number.as_u64().map(i128::from))
            .or_else(|| number.as_f64().map(|value| value as i128)),
        Value::String(value) => value.trim().parse::<i128>().ok(),
        _ => None,
    })
}

fn bool_value(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn redact_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    if is_sensitive_key(key) {
                        (key.clone(), Value::String("[redacted]".to_string()))
                    } else {
                        (key.clone(), redact_value(value))
                    }
                })
                .collect(),
        ),
        Value::Array(array) => Value::Array(array.iter().map(redact_value).collect()),
        _ => value.clone(),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "authorization",
        "auth",
        "api_key",
        "access_key",
        "session",
        "cookie",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diagnostics_merge_into_existing_message_and_hide_payload_by_default() {
        let mut messages = vec![json!({
            "message_key": "telegram-user:42:7",
            "connection_slug": "telegram-user",
            "stages": [{
                "id": "connector_stdout_received",
                "status": "completed",
                "at_ms": 1000,
                "source": "connector_stream",
                "summary": "received"
            }]
        })];
        merge_diagnostic_records(
            &mut messages,
            vec![json!({
                "at_ms": 900,
                "stage": "delivery_emitted",
                "connection_slug": "telegram-user",
                "message_key": "telegram-user:42:7",
                "chat_id": 42,
                "sender_name": "Alice",
                "message_id": 7,
                "text_prefix": "hello",
                "session_token": "secret"
            })],
            false,
        );
        finalize_messages(&mut messages, false);

        let message = &messages[0];
        assert_eq!(message["payload"], Value::Null);
        assert_eq!(message["payload_available"], true);
        assert_eq!(message["latest_status"], "emitted");
        assert!(message["stages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stage| { stage["id"] == "delivery_emitted" }));
    }

    #[test]
    fn store_message_text_is_bounded_when_payload_is_hidden() {
        let full_text = "private launch notes ".repeat(20);
        let mut messages = vec![json!({
            "message_key": "telegram-user:42:7",
            "connection_slug": "telegram-user",
            "text": full_text,
            "stages": [{
                "id": "connector_stdout_received",
                "status": "completed",
                "at_ms": 1000,
                "source": "connector_stream",
                "summary": "received"
            }]
        })];

        finalize_messages(&mut messages, false);

        let text = messages[0]["text"].as_str().unwrap();
        assert_ne!(text, full_text);
        assert!(text.chars().count() <= 203);
        assert!(text.ends_with("..."));
        assert_eq!(messages[0]["payload"], Value::Null);
    }

    #[test]
    fn include_payload_returns_redacted_diagnostic_payload() {
        let mut messages = Vec::new();
        merge_diagnostic_records(
            &mut messages,
            vec![json!({
                "at_ms": 900,
                "stage": "telegram_update_received",
                "connection_slug": "telegram-user",
                "chat_id": 42,
                "message_id": 7,
                "session_token": "secret",
                "text_prefix": "hello"
            })],
            true,
        );
        finalize_messages(&mut messages, true);

        assert_eq!(messages[0]["payload"]["session_token"], "[redacted]");
        assert_eq!(messages[0]["payload"]["text_prefix"], "hello");
    }

    #[test]
    fn diagnostic_reader_is_bounded_to_latest_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("message-diagnostics.ndjson");
        let body = (0..=MAX_DIAGNOSTIC_RECORDS)
            .map(|idx| {
                json!({
                    "at_ms": idx,
                    "stage": "delivery_emitted",
                    "connection_slug": "telegram-user",
                    "chat_id": 42,
                    "message_id": idx,
                })
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, body).unwrap();
        let mut bytes_read = 0;
        let mut records = Vec::new();
        read_diagnostic_file(&path, MAX_DIAGNOSTIC_BYTES, &mut bytes_read, &mut records).unwrap();
        trim_diagnostic_records(&mut records);

        assert_eq!(records.len(), MAX_DIAGNOSTIC_RECORDS);
        assert_eq!(records[0]["message_id"], MAX_DIAGNOSTIC_RECORDS);
        assert_eq!(records.last().unwrap()["message_id"], 1);
    }

    #[test]
    fn linked_task_adds_task_created_stage() {
        let mut message = json!({
            "message_key": "telegram-user:42:7",
            "connection_slug": "telegram-user",
            "chat_id": "42",
            "envelope_id": "env-7",
            "stages": [{
                "id": "triage_completed",
                "status": "completed",
                "at_ms": 1000,
                "source": "triage_agent",
                "summary": "created"
            }]
        });
        let tasks = vec![json!({
            "task_id": "task-1",
            "subject": "reply",
            "status": "open",
            "created_at_ms": 1100,
            "metadata": {
                "monitor_envelope_id": "env-7"
            }
        })];
        let linked = linked_tasks_for_message(&message, &tasks);
        let object = message.as_object_mut().unwrap();
        object.insert("linked_tasks".to_string(), Value::Array(linked.clone()));
        push_stage(
            object,
            json!({
                "id": "task_created",
                "status": "completed",
                "at_ms": linked_task_activity_ms(&linked).unwrap(),
                "source": "monitor_task",
                "summary": "Linked monitor task task-1."
            }),
        );
        let mut messages = vec![message];
        finalize_messages(&mut messages, false);

        assert_eq!(messages[0]["latest_status"], "task_created");
        assert_eq!(messages[0]["linked_tasks"][0]["task_id"], "task-1");
    }

    #[test]
    fn finalize_promotes_latest_triage_decision() {
        let mut messages = vec![json!({
            "message_key": "telegram-user:42:8",
            "connection_slug": "telegram-user",
            "latest_status": "triaged_no_task",
            "stages": [
                {
                    "id": "triage_completed",
                    "status": "completed",
                    "at_ms": 1000,
                    "source": "subscription_router",
                    "summary": "No task required."
                },
                {
                    "id": "triage_decision",
                    "status": "completed",
                    "at_ms": 1001,
                    "source": "subscription_router",
                    "summary": "这是状态通知，没有请求你处理。",
                    "decision": {
                        "envelope_id": "env-8",
                        "outcome": "no_task",
                        "policy": "status_update_no_action",
                        "reason": "这是状态通知，没有请求你处理。"
                    }
                }
            ]
        })];

        finalize_messages(&mut messages, false);

        assert_eq!(messages[0]["latest_status"], "triaged_no_task");
        assert_eq!(
            messages[0]["triage_decision"]["reason"],
            "这是状态通知，没有请求你处理。"
        );
        assert_eq!(
            messages[0]["triage_decision"]["policy"],
            "status_update_no_action"
        );
    }
}
