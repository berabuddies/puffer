//! Task snapshot helpers for the desktop workflow/task screens.

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::monitor_contract::{display_source_context, parse_monitor_contract};
use puffer_subscriptions::{normalize_contact_id, OutboundAction, OutboundStore};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default)]
struct TaskStoreSnapshot {
    #[serde(default)]
    tasks: Vec<TaskSnapshotRecord>,
}

#[derive(Debug, Deserialize)]
struct TaskSnapshotRecord {
    #[serde(alias = "id", alias = "taskId")]
    task_id: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    description: String,
    #[serde(default, alias = "activeForm")]
    active_form: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    blocks: Vec<String>,
    #[serde(default, alias = "blockedBy")]
    blocked_by: Vec<String>,
    #[serde(default)]
    metadata: Map<String, Value>,
    #[serde(default, alias = "taskType")]
    task_type: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default, alias = "processId")]
    process_id: Option<u32>,
    #[serde(default, alias = "outputFile")]
    output_file: Option<String>,
    #[serde(default, rename = "receivedAt", alias = "received_at")]
    received_at: Option<String>,
    #[serde(default, rename = "expiresAt", alias = "expires_at")]
    expires_at: Option<String>,
    #[serde(default, alias = "startedAtMs")]
    started_at_ms: Option<u64>,
    #[serde(default, alias = "updatedAtMs")]
    updated_at_ms: Option<u64>,
    #[serde(default, alias = "exitCode")]
    exit_code: Option<i32>,
}

/// Adds normalized agent-created and monitor-created task rows to a workflow snapshot.
pub(crate) fn add_task_context(paths: &ConfigPaths, snapshot: &mut Value) {
    let Some(object) = snapshot.as_object_mut() else {
        return;
    };
    let mut tasks = Vec::new();
    let mut errors = Vec::new();
    let workflow = workflow_root(paths);
    let outbound_store = outbound_store(paths, &mut errors);
    append_task_file(
        &workflow.join("tasks.json"),
        "agent",
        "workspace",
        "workspace",
        outbound_store.as_ref(),
        &mut tasks,
        &mut errors,
    );
    append_scoped_agent_tasks(
        &workflow.join("sessions"),
        "session",
        outbound_store.as_ref(),
        &mut tasks,
        &mut errors,
    );
    append_scoped_agent_tasks(
        &workflow.join("team_tasks"),
        "team",
        outbound_store.as_ref(),
        &mut tasks,
        &mut errors,
    );
    append_task_file(
        &workflow.join("monitor_tasks.json"),
        "monitor",
        "monitor",
        "monitors",
        outbound_store.as_ref(),
        &mut tasks,
        &mut errors,
    );
    object.insert("tasks".to_string(), Value::Array(tasks));
    object.insert(
        "task_error".to_string(),
        if errors.is_empty() {
            Value::Null
        } else {
            Value::String(errors.join("; "))
        },
    );
}

fn append_scoped_agent_tasks(
    parent: &Path,
    scope_kind: &str,
    outbound_store: Option<&OutboundStore>,
    tasks: &mut Vec<Value>,
    errors: &mut Vec<String>,
) {
    match sorted_child_dirs(parent) {
        Ok(dirs) => {
            for dir in dirs {
                let Some(scope_id) = dir.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                let scope = format!("{scope_kind}:{scope_id}");
                let label = scope_label(scope_kind, scope_id);
                append_task_file(
                    &dir.join("tasks.json"),
                    "agent",
                    &scope,
                    &label,
                    outbound_store,
                    tasks,
                    errors,
                );
            }
        }
        Err(error) => errors.push(error.to_string()),
    }
}

fn append_task_file(
    path: &Path,
    source: &str,
    scope: &str,
    scope_label: &str,
    outbound_store: Option<&OutboundStore>,
    tasks: &mut Vec<Value>,
    errors: &mut Vec<String>,
) {
    match load_task_file(path, source, scope, scope_label, outbound_store) {
        Ok(rows) => tasks.extend(rows),
        Err(error) => errors.push(error.to_string()),
    }
}

fn load_task_file(
    path: &Path,
    source: &str,
    scope: &str,
    scope_label: &str,
    outbound_store: Option<&OutboundStore>,
) -> Result<Vec<Value>> {
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let store: TaskStoreSnapshot = serde_json::from_str(&raw)
        .with_context(|| format!("invalid task store {}", path.display()))?;
    Ok(store
        .tasks
        .into_iter()
        .map(|task| task_json(task, source, scope, scope_label, outbound_store))
        .collect())
}

fn task_json(
    task: TaskSnapshotRecord,
    source: &str,
    scope: &str,
    scope_label: &str,
    outbound_store: Option<&OutboundStore>,
) -> Value {
    json!({
        "task_id": task.task_id,
        "subject": task.subject,
        "description": task.description,
        "active_form": task.active_form,
        "status": task.status,
        "source": source,
        "task_scope": scope,
        "task_scope_label": scope_label,
        "task_type": task.task_type.unwrap_or_else(|| "task".to_string()),
        "owner": task.owner,
        "blocks": task.blocks,
        "blocked_by": task.blocked_by,
        "command": task.command,
        "process_id": task.process_id,
        "output_file": task.output_file,
        "received_at": task.received_at,
        "expires_at": task.expires_at,
        "started_at_ms": task.started_at_ms,
        "updated_at_ms": task.updated_at_ms,
        "exit_code": task.exit_code,
        "ignored": metadata_bool(&task.metadata, "ignored"),
        "monitor_connection": metadata_string(
            &task.metadata,
            &["monitor_connection", "monitorConnection"],
            &["connection", "connection_slug", "connectionSlug"]
        ),
        "monitor_connector": metadata_string(
            &task.metadata,
            &["monitor_connector", "monitorConnector"],
            &["connector", "connector_slug", "connectorSlug"]
        ),
        "monitor_memory_path": metadata_string(
            &task.metadata,
            &["monitor_memory_path", "monitorMemoryPath"],
            &["memory_path", "memoryPath"]
        ),
        "monitor_envelope_id": metadata_string(
            &task.metadata,
            &["monitor_envelope_id", "monitorEnvelopeId"],
            &["envelope_id", "envelopeId"]
        ),
        "monitor": metadata_value(&task.metadata, &["monitor"], &[]),
        "source_context": monitor_source_context(&task.metadata),
        "source_messages": monitor_source_messages(&task.metadata),
        "completion_policy": monitor_completion_policy(&task.metadata),
        "outboundAction": outbound_action_for_metadata(outbound_store, &task.metadata),
        "ignore_reason": metadata_string(
            &task.metadata,
            &["ignore_reason", "ignoreReason"],
            &["reason"]
        ),
        "ignore_analysis_started": metadata_bool(&task.metadata, "ignore_analysis_started"),
        "ignore_analysis_status": metadata_string(
            &task.metadata,
            &["ignore_analysis_status", "ignoreAnalysisStatus"],
            &["status"]
        ),
        "ignore_analysis_result": metadata_string(
            &task.metadata,
            &["ignore_analysis_result", "ignoreAnalysisResult"],
            &["result"]
        ),
        "ignore_analysis_error": metadata_string(
            &task.metadata,
            &["ignore_analysis_error", "ignoreAnalysisError"],
            &["error"]
        ),
        "ignore_analysis_usage": metadata_value(
            &task.metadata,
            &["ignore_analysis_usage", "ignoreAnalysisUsage"],
            &["usage"]
        ),
        "ignore_analysis_completed_at_ms": metadata_u64(
            &task.metadata,
            &["ignore_analysis_completed_at_ms", "ignoreAnalysisCompletedAtMs"],
            &["completed_at_ms", "completedAtMs"]
        ),
        "actions": monitor_actions_for_status(&task.metadata, &task.status),
        "possible_ignore_reasons": monitor_ignore_reasons(&task.metadata),
    })
}

pub(super) fn outbound_store(
    paths: &ConfigPaths,
    errors: &mut Vec<String>,
) -> Option<OutboundStore> {
    match OutboundStore::load(outbound_actions_path(paths)) {
        Ok(store) => Some(store),
        Err(error) => {
            errors.push(error.to_string());
            None
        }
    }
}

pub(super) fn outbound_action_for_metadata(
    store: Option<&OutboundStore>,
    metadata: &Map<String, Value>,
) -> Option<Value> {
    let action_id = metadata_string(
        metadata,
        &["outbound_action_id", "outboundActionId"],
        &["outbound_action_id", "outboundActionId"],
    )?;
    let action = store?.get(&action_id).ok().flatten()?;
    Some(outbound_action_json(&action))
}

fn outbound_actions_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("outbound_actions.json")
}

fn outbound_action_json(action: &OutboundAction) -> Value {
    json!({
        "id": action.id,
        "version": action.version,
        "status": action.status,
        "message": action.message,
        "approvedMessage": action.approved_message,
        "recipientStableId": action.recipient_stable_id,
        "recipientSource": action.recipient_source,
        "createdAtMs": action.created_at_ms,
        "expiresAtMs": action.expires_at_ms,
        "receipt": action.receipt,
        "error": action.error,
    })
}

fn workflow_root(paths: &ConfigPaths) -> PathBuf {
    paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
}

fn sorted_child_dirs(parent: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", parent.display()));
        }
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read {}", parent.display()))?;
        if entry
            .file_type()
            .with_context(|| format!("failed to stat {}", entry.path().display()))?
            .is_dir()
        {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn scope_label(scope_kind: &str, scope_id: &str) -> String {
    if scope_kind == "session" {
        let short = scope_id.get(..8).unwrap_or(scope_id);
        return format!("session {short}");
    }
    format!("{scope_kind} {scope_id}")
}

fn metadata_bool(metadata: &Map<String, Value>, key: &str) -> bool {
    metadata
        .get(key)
        .and_then(Value::as_bool)
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(Value::as_object)
                .and_then(|monitor| monitor.get(key))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

fn metadata_u64(
    metadata: &Map<String, Value>,
    top_level_keys: &[&str],
    monitor_keys: &[&str],
) -> Option<u64> {
    top_level_keys
        .iter()
        .find_map(|key| metadata.get(*key).and_then(Value::as_u64))
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(Value::as_object)
                .and_then(|monitor| {
                    monitor_keys
                        .iter()
                        .find_map(|key| monitor.get(*key).and_then(Value::as_u64))
                })
        })
}

pub(super) fn metadata_value(
    metadata: &Map<String, Value>,
    top_level_keys: &[&str],
    monitor_keys: &[&str],
) -> Option<Value> {
    top_level_keys
        .iter()
        .find_map(|key| metadata.get(*key).cloned())
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(Value::as_object)
                .and_then(|monitor| {
                    monitor_keys
                        .iter()
                        .find_map(|key| monitor.get(*key).cloned())
                })
        })
}

pub(super) fn monitor_source_messages(metadata: &Map<String, Value>) -> Option<Value> {
    if let Some(messages) = metadata_value(metadata, &["source_messages", "sourceMessages"], &[])
        .and_then(trim_source_message_array)
    {
        return Some(messages);
    }

    let source_context = metadata
        .get("source_context")
        .or_else(|| metadata.get("sourceContext"));
    let mut messages = Vec::new();
    if let Some(context) = source_context {
        append_message_array(
            &mut messages,
            context
                .get("source_messages")
                .or_else(|| context.get("sourceMessages")),
        );
        append_message_array(
            &mut messages,
            context
                .get("context_messages")
                .or_else(|| context.get("contextMessages")),
        );
        append_message_array(
            &mut messages,
            context
                .get("conversation_context")
                .and_then(|conversation_context| conversation_context.get("messages")),
        );
    }
    if messages.is_empty() {
        return None;
    }

    if let Some(current) = current_source_message(metadata, source_context) {
        if !message_array_contains_current(&messages, &current) {
            messages.push(current);
        }
    }
    let start = messages.len().saturating_sub(8);
    Some(Value::Array(messages.into_iter().skip(start).collect()))
}

fn trim_source_message_array(value: Value) -> Option<Value> {
    let messages = value.as_array()?.clone();
    if messages.is_empty() {
        return None;
    }
    let start = messages.len().saturating_sub(8);
    Some(Value::Array(messages.into_iter().skip(start).collect()))
}

fn append_message_array(messages: &mut Vec<Value>, raw: Option<&Value>) {
    let Some(raw_messages) = raw.and_then(Value::as_array) else {
        return;
    };
    messages.extend(raw_messages.iter().cloned());
}

fn current_source_message(
    metadata: &Map<String, Value>,
    source_context: Option<&Value>,
) -> Option<Value> {
    let text = metadata_string(metadata, &["source_text", "sourceText"], &[]).or_else(|| {
        source_context
            .and_then(|context| context.get("text"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })?;
    let message_id = metadata_number_i64(metadata, &["source_message_id", "sourceMessageId"])
        .or_else(|| {
            source_context
                .and_then(|context| {
                    context
                        .get("message_id")
                        .or_else(|| context.get("messageId"))
                })
                .and_then(number_i64)
        });
    let sender = source_context
        .and_then(|context| context.get("sender"))
        .cloned()
        .unwrap_or(Value::Null);

    Some(json!({
        "from": "them",
        "direction": "incoming",
        "sender": sender,
        "message_id": message_id,
        "text": text,
    }))
}

fn message_array_contains_current(messages: &[Value], current: &Value) -> bool {
    let current_text = current.get("text").and_then(Value::as_str);
    let current_message_id = current.get("message_id").and_then(number_i64);
    messages.iter().any(|message| {
        let same_message_id = current_message_id
            .zip(message.get("message_id").and_then(number_i64))
            .is_some_and(|(left, right)| left == right);
        let same_text = current_text
            .zip(message.get("text").and_then(Value::as_str))
            .is_some_and(|(left, right)| left == right);
        same_message_id || same_text
    })
}

fn metadata_number_i64(metadata: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| metadata.get(*key).and_then(number_i64))
}

fn number_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok())),
        Value::String(value) => value.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn monitor_actions(metadata: &Map<String, Value>) -> Vec<Value> {
    metadata
        .get("actions")
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(|monitor| monitor.get("actions"))
        })
        .and_then(Value::as_array)
        .map(|actions| {
            actions
                .iter()
                .filter_map(|action| {
                    let object = action.as_object()?;
                    let name = string_value(object.get("name"))
                        .or_else(|| string_value(object.get("actionName")))?;
                    let prompt = string_value(object.get("prompt"))
                        .or_else(|| string_value(object.get("actionPrompt")))?;
                    Some(json!({ "name": name, "prompt": prompt }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn monitor_actions_for_status(metadata: &Map<String, Value>, status: &str) -> Vec<Value> {
    if monitor_task_status_is_terminal(status) {
        return Vec::new();
    }
    monitor_actions(metadata)
}

fn monitor_task_status_is_terminal(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "completed" | "cancelled" | "deleted" | "ignored" | "stopped"
    )
}

pub(super) fn monitor_source_context(metadata: &Map<String, Value>) -> Option<Value> {
    if let Some(contract) = parse_monitor_contract(metadata).ok().flatten() {
        return with_verbatim_source_text(metadata, Some(display_source_context(&contract)));
    }
    let context = metadata
        .get("source_context")
        .or_else(|| metadata.get("sourceContext"))
        .cloned()
        .or_else(|| derived_monitor_source_context(metadata));
    with_verbatim_source_text(metadata, context)
}

pub(super) fn monitor_source_context_with_sender_identity(
    metadata: &Map<String, Value>,
    telegram_peer_avatars: &HashMap<String, String>,
    telegram_peer_names: &HashMap<String, String>,
) -> Option<Value> {
    monitor_source_context(metadata).map(|context| {
        let context = with_sender_avatar_url(metadata, context, telegram_peer_avatars);
        with_sender_name(metadata, context, telegram_peer_names)
    })
}

/// Fills `sender.name` from the cached Telegram peer display names when the
/// stored context lacks one. Triage only copies stable identity fields
/// (sender_id / sender_username) onto tasks, so accounts without an
/// @username surfaced as "Unknown sender" even though the peer cache knows
/// their display name.
fn with_sender_name(
    metadata: &Map<String, Value>,
    mut context: Value,
    telegram_peer_names: &HashMap<String, String>,
) -> Value {
    if telegram_peer_names.is_empty() || context_sender_name(&context).is_some() {
        return context;
    }
    let Some(name) = sender_identity_from_cache(metadata, &context, telegram_peer_names) else {
        return context;
    };
    let Some(object) = context.as_object_mut() else {
        return context;
    };
    let sender = object
        .entry("sender".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(sender) = sender.as_object_mut() else {
        return context;
    };
    sender.insert("name".to_string(), Value::String(name));
    context
}

fn context_sender_name(context: &Value) -> Option<String> {
    context
        .get("sender")
        .and_then(Value::as_object)
        .and_then(|sender| {
            scalar_string(sender.get("name"))
                .or_else(|| scalar_string(sender.get("display_name")))
                .or_else(|| scalar_string(sender.get("displayName")))
        })
}

fn sender_identity_from_cache(
    metadata: &Map<String, Value>,
    context: &Value,
    cache: &HashMap<String, String>,
) -> Option<String> {
    for contact_id in sender_avatar_contact_ids(metadata, context) {
        if let Some(value) = cache
            .get(&contact_id)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_string());
        }
    }
    None
}

fn with_sender_avatar_url(
    metadata: &Map<String, Value>,
    mut context: Value,
    telegram_peer_avatars: &HashMap<String, String>,
) -> Value {
    if telegram_peer_avatars.is_empty() || context_sender_avatar_url(&context).is_some() {
        return context;
    }
    let Some(avatar) = sender_avatar_url_from_cache(metadata, &context, telegram_peer_avatars)
    else {
        return context;
    };
    let Some(object) = context.as_object_mut() else {
        return context;
    };
    let sender = object
        .entry("sender".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(sender) = sender.as_object_mut() else {
        return context;
    };
    sender.insert("avatar_url".to_string(), Value::String(avatar));
    context
}

fn context_sender_avatar_url(context: &Value) -> Option<String> {
    context
        .get("sender")
        .and_then(Value::as_object)
        .and_then(|sender| {
            scalar_string(sender.get("avatar_url"))
                .or_else(|| scalar_string(sender.get("avatarUrl")))
        })
}

fn sender_avatar_url_from_cache(
    metadata: &Map<String, Value>,
    context: &Value,
    telegram_peer_avatars: &HashMap<String, String>,
) -> Option<String> {
    for contact_id in sender_avatar_contact_ids(metadata, context) {
        if let Some(avatar) = telegram_peer_avatars
            .get(&contact_id)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(avatar.to_string());
        }
    }
    None
}

fn sender_avatar_contact_ids(metadata: &Map<String, Value>, context: &Value) -> Vec<String> {
    // A mixed-sender burst has no single contact. Attributing the task to one
    // member — or to a leaked singular sender hint — is exactly the
    // wrong-contact bug (agentenv/monorepo#682): return no ids so the snapshot
    // enriches no single contact rather than fabricating one.
    if is_mixed_sender_burst(metadata) {
        return Vec::new();
    }
    let sender = context.get("sender").and_then(Value::as_object);
    let mut ids = BTreeSet::new();
    if let Some(sender_id) = sender
        .and_then(|sender| scalar_string(sender.get("id")))
        .or_else(|| {
            metadata_string(
                metadata,
                &["sender_id", "senderId"],
                &["sender_id", "senderId"],
            )
        })
    {
        if sender_id
            .parse::<i64>()
            .ok()
            .filter(|value| *value > 0)
            .is_some()
        {
            if let Some(contact_id) = normalize_contact_id(&format!("telegram-user-id@{sender_id}"))
            {
                ids.insert(contact_id);
            }
        }
    }
    if let Some(username) = sender
        .and_then(|sender| scalar_string(sender.get("username")))
        .or_else(|| {
            metadata_string(
                metadata,
                &["sender_username", "senderUsername"],
                &["sender_username", "senderUsername"],
            )
        })
    {
        let username = username.trim().trim_start_matches('@');
        if !username.is_empty() {
            if let Some(contact_id) = normalize_contact_id(&format!("telegram@{username}")) {
                ids.insert(contact_id);
            }
        }
    }
    ids.into_iter().collect()
}

/// Whether the plural `sender_ids` stamp marks this as a mixed-sender burst
/// (>=2 distinct members) — a task with no single contact it can be
/// attributed to (agentenv/monorepo#682, #655).
fn is_mixed_sender_burst(metadata: &Map<String, Value>) -> bool {
    metadata
        .get("sender_ids")
        .or_else(|| metadata.get("senderIds"))
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values
                .iter()
                .filter_map(|value| scalar_string(Some(value)))
                .collect::<BTreeSet<_>>()
                .len()
                >= 2
        })
}

fn metadata_string(
    metadata: &Map<String, Value>,
    top_level_keys: &[&str],
    monitor_keys: &[&str],
) -> Option<String> {
    top_level_keys
        .iter()
        .find_map(|key| scalar_string(metadata.get(*key)))
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(Value::as_object)
                .and_then(|monitor| {
                    monitor_keys
                        .iter()
                        .find_map(|key| scalar_string(monitor.get(*key)))
                })
        })
}

/// Surfaces the server-stamped verbatim event text (`metadata.source_text`,
/// written by the triage runner) as `source_context.text` when the stored or
/// derived context lacks one. Task subject/description are LLM paraphrases;
/// this field is the ground truth UIs and reply flows can quote
/// (agentenv/monorepo#619).
pub(super) fn with_verbatim_source_text(
    metadata: &Map<String, Value>,
    context: Option<Value>,
) -> Option<Value> {
    let mut context = context?;
    if let Some(object) = context.as_object_mut() {
        let has_text = object
            .get("text")
            .and_then(Value::as_str)
            .map_or(false, |value| !value.trim().is_empty());
        if !has_text {
            if let Some(text) = metadata
                .get("source_text")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                object.insert("text".to_string(), Value::String(text.to_string()));
            }
        }
        if object.get("message_id").and_then(Value::as_i64).is_none() {
            if let Some(message_id) = metadata.get("source_message_id").and_then(Value::as_i64) {
                object.insert("message_id".to_string(), Value::from(message_id));
            }
        }
    }
    Some(context)
}

fn derived_monitor_source_context(metadata: &Map<String, Value>) -> Option<Value> {
    let connector_slug = metadata_string(
        metadata,
        &["monitor_connector", "monitorConnector"],
        &["connector", "connector_slug", "connectorSlug"],
    )?;
    if !connector_slug.contains("telegram") {
        return None;
    }
    // Ids are stamped as i64 by the subscriber/burst paths and as strings by
    // older records — the number-tolerant reader keeps legacy no-contract
    // tasks renderable either way (agentenv/monorepo#682).
    let chat_id = metadata_string(metadata, &["chat_id", "chatId"], &["chat_id", "chatId"])?;
    let chat_kind = metadata_string(
        metadata,
        &["chat_kind", "chatKind"],
        &["chat_kind", "chatKind"],
    )
    .map(|value| value.to_ascii_lowercase())
    .unwrap_or_else(|| "user".to_string());
    let (source_kind, summary_kind) = match chat_kind.as_str() {
        "group" | "supergroup" => ("telegram_group_message", "Telegram group message"),
        "channel" => ("telegram_channel_message", "Telegram channel message"),
        _ => ("telegram_direct_message", "Telegram direct message"),
    };
    let connection_slug = metadata_string(
        metadata,
        &["monitor_connection", "monitorConnection"],
        &["connection", "connection_slug", "connectionSlug"],
    );
    let sender_id = metadata_string(
        metadata,
        &["sender_id", "senderId"],
        &["sender_id", "senderId"],
    );
    let sender_username = metadata_string(
        metadata,
        &["sender_username", "senderUsername"],
        &["sender_username", "senderUsername"],
    );
    let mut sender = Map::new();
    if let Some(sender_id) = sender_id {
        sender.insert("id".to_string(), Value::String(sender_id));
    }
    if let Some(sender_username) = sender_username {
        sender.insert("username".to_string(), Value::String(sender_username));
    }
    Some(json!({
        "kind": source_kind,
        "connection_slug": connection_slug,
        "connector_slug": connector_slug,
        "summary": format!("{summary_kind} from chat_id {chat_id}"),
        "delivery_target": {
            "type": "telegram_chat",
            "chat_id": chat_id,
            "chat_kind": chat_kind,
        },
        "sender": sender,
    }))
}

pub(super) fn monitor_completion_policy(metadata: &Map<String, Value>) -> Option<Value> {
    metadata
        .get("completion_policy")
        .or_else(|| metadata.get("completionPolicy"))
        .cloned()
        .or_else(|| default_monitor_completion_policy(metadata))
}

fn default_monitor_completion_policy(metadata: &Map<String, Value>) -> Option<Value> {
    if !monitor_actions_require_reply(metadata) {
        return None;
    }
    monitor_source_context(metadata)
        .and_then(|context| {
            context
                .get("delivery_target")
                .or_else(|| context.get("deliveryTarget"))
                .cloned()
        })
        .map(|_| {
            json!({
                "mode": "draft_then_approve",
                "requires_human_approval": true,
                "requires_receipt": true,
            })
        })
}

fn monitor_actions_require_reply(metadata: &Map<String, Value>) -> bool {
    monitor_actions(metadata).iter().any(|action| {
        let name = action
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let prompt = action
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let text = format!("{name}\n{prompt}").to_ascii_lowercase();
        [
            "reply",
            "respond",
            "send it back",
            "send back",
            "answer back",
            "message back",
        ]
        .iter()
        .any(|needle| text.contains(needle))
    })
}

fn monitor_ignore_reasons(metadata: &Map<String, Value>) -> Vec<Value> {
    metadata
        .get("possible_ignore_reasons")
        .or_else(|| metadata.get("possibleIgnoreReasons"))
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(|monitor| monitor.get("possible_ignore_reasons"))
        })
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(|monitor| monitor.get("possibleIgnoreReasons"))
        })
        .and_then(Value::as_array)
        .map(|reasons| {
            reasons
                .iter()
                .filter_map(|reason| string_value(Some(reason)))
                .map(Value::String)
                .collect()
        })
        .unwrap_or_default()
}

fn string_value(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(ToOwned::to_owned)
}

fn scalar_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::ConfigPaths;
    use puffer_subscriptions::{NewOutboundDraft, OutboundOrigin, OutboundStore, RecipientSource};

    #[test]
    fn task_context_reads_agent_session_team_and_monitor_tasks() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let workflow = workflow_root(&paths);
        std::fs::create_dir_all(workflow.join("sessions/session-one")).unwrap();
        std::fs::create_dir_all(workflow.join("team_tasks/alpha")).unwrap();
        std::fs::write(
            workflow.join("sessions/session-one/tasks.json"),
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "task-1",
                    "subject": "Fix failing check",
                    "description": "Run the targeted test and patch the failure.",
                    "active_form": "Fixing failing check",
                    "status": "in_progress",
                    "metadata": {},
                    "started_at_ms": 10,
                    "updated_at_ms": 20
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            workflow.join("team_tasks/alpha/tasks.json"),
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "team-1",
                    "subject": "Review worker patch",
                    "description": "Review the delegated patch.",
                    "active_form": "Reviewing worker patch",
                    "status": "pending",
                    "metadata": {}
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            workflow.join("monitor_tasks.json"),
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "monitor-1",
                    "subject": "Handle support ping",
                    "description": "Alice asked for a deployment update.",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "monitor_envelope_id": "env-monitor-1",
                        "chat_id": "8759047281",
                        "sender_id": "8759047281",
                        "ignore_analysis_result": "filter looks scoped",
                        "ignore_analysis_usage": {
                            "input_tokens": 12,
                            "output_tokens": 3,
                            "cache_read_tokens": 2,
                            "spent_tokens": 13
                        },
                        "actions": [{
                            "actionName": "Draft reply",
                            "actionPrompt": "Draft a concise reply."
                        }]
                    }
                }, {
                    "task_id": "monitor-sent",
                    "subject": "Handle sent support reply",
                    "description": "Alice already received a Telegram reply.",
                    "status": "completed",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "actions": [{
                            "actionName": "Send",
                            "actionPrompt": "Send the approved Telegram reply."
                        }]
                    }
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let mut snapshot = json!({});
        add_task_context(&paths, &mut snapshot);
        let tasks = snapshot["tasks"].as_array().unwrap();

        assert_eq!(tasks.len(), 4);
        let session_task = tasks
            .iter()
            .find(|task| task["task_id"] == "task-1")
            .unwrap();
        assert_eq!(session_task["source"], "agent");
        assert_eq!(session_task["task_scope"], "session:session-one");
        let team_task = tasks
            .iter()
            .find(|task| task["task_id"] == "team-1")
            .unwrap();
        assert_eq!(team_task["task_scope"], "team:alpha");
        let monitor_task = tasks
            .iter()
            .find(|task| task["task_id"] == "monitor-1")
            .unwrap();
        assert_eq!(monitor_task["source"], "monitor");
        assert_eq!(monitor_task["monitor_connection"], "telegram-user");
        assert_eq!(monitor_task["monitor_connector"], "telegram-login");
        assert_eq!(monitor_task["monitor_envelope_id"], "env-monitor-1");
        assert_eq!(
            monitor_task["source_context"]["kind"],
            "telegram_direct_message"
        );
        assert_eq!(
            monitor_task["source_context"]["delivery_target"]["chat_id"],
            "8759047281"
        );
        assert_eq!(
            monitor_task["completion_policy"]["mode"],
            "draft_then_approve"
        );
        assert_eq!(
            monitor_task["completion_policy"]["requires_human_approval"],
            true
        );
        assert_eq!(
            monitor_task["ignore_analysis_result"],
            "filter looks scoped"
        );
        assert_eq!(monitor_task["ignore_analysis_usage"]["spent_tokens"], 13);
        assert_eq!(monitor_task["actions"][0]["name"], "Draft reply");
        let sent_monitor_task = tasks
            .iter()
            .find(|task| task["task_id"] == "monitor-sent")
            .unwrap();
        assert!(sent_monitor_task["outboundAction"].is_null());
        assert!(sent_monitor_task["actions"].as_array().unwrap().is_empty());
        assert!(snapshot["task_error"].is_null());
    }

    #[test]
    fn task_context_does_not_default_reply_policy_for_non_reply_actions() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let workflow = workflow_root(&paths);
        std::fs::create_dir_all(&workflow).unwrap();
        std::fs::write(
            workflow.join("monitor_tasks.json"),
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "monitor-reminder",
                    "subject": "Remember Telegram deadline",
                    "description": "A Telegram message contains a deadline.",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "chat_id": "8759047281",
                        "sender_id": "8759047281",
                        "actions": [{
                            "actionName": "Add reminder",
                            "actionPrompt": "Create a reminder from the deadline."
                        }]
                    }
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let mut snapshot = json!({});
        add_task_context(&paths, &mut snapshot);
        let tasks = snapshot["tasks"].as_array().unwrap();
        let monitor_task = tasks
            .iter()
            .find(|task| task["task_id"] == "monitor-reminder")
            .unwrap();

        assert_eq!(
            monitor_task["source_context"]["delivery_target"]["chat_id"],
            "8759047281"
        );
        assert!(monitor_task["completion_policy"].is_null());
    }

    #[test]
    fn task_context_embeds_referenced_outbound_action() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let action = OutboundStore::load(paths.user_config_dir.join("outbound_actions.json"))
            .unwrap()
            .create_draft(NewOutboundDraft {
                connector_slug: "telegram-login".to_string(),
                connection_slug: "telegram-user".to_string(),
                action: "send_message".to_string(),
                input: json!({ "chat_id": "42", "message": "Deployment finished." }),
                recipient_stable_id: "telegram:42".to_string(),
                recipient_source: RecipientSource::Stamped,
                message: "Deployment finished.".to_string(),
                origin: OutboundOrigin {
                    session_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    task_id: Some("monitor-1".to_string()),
                },
                ttl_ms: None,
            })
            .unwrap();
        let workflow = workflow_root(&paths);
        std::fs::create_dir_all(&workflow).unwrap();
        std::fs::write(
            workflow.join("monitor_tasks.json"),
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "monitor-1",
                    "subject": "Answer Telegram support ping",
                    "description": "Alice asked whether the deployment is finished.",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "outbound_action_id": action.id
                    }
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let mut snapshot = json!({});
        add_task_context(&paths, &mut snapshot);
        let tasks = snapshot["tasks"].as_array().unwrap();
        let monitor_task = tasks
            .iter()
            .find(|task| task["task_id"] == "monitor-1")
            .unwrap();

        assert_eq!(monitor_task["outboundAction"]["id"], action.id);
        assert_eq!(monitor_task["outboundAction"]["version"], 1);
        assert_eq!(monitor_task["outboundAction"]["status"], "draft_ready");
        assert_eq!(
            monitor_task["outboundAction"]["message"],
            "Deployment finished."
        );
        assert_eq!(
            monitor_task["outboundAction"]["recipientStableId"],
            "telegram:42"
        );
        assert_eq!(monitor_task["outboundAction"]["recipientSource"], "stamped");
    }

    #[test]
    fn task_context_preserves_telegram_group_source_context() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let workflow = workflow_root(&paths);
        std::fs::create_dir_all(&workflow).unwrap();
        std::fs::write(
            workflow.join("monitor_tasks.json"),
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "monitor-group",
                    "subject": "Reply to group mention",
                    "description": "A Telegram group mentioned me.",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "chat_kind": "group",
                        "chat_id": "-10012345",
                        "sender_id": "8759047281",
                        "actions": [{
                            "actionName": "Draft reply",
                            "actionPrompt": "Draft a concise group reply."
                        }]
                    }
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let mut snapshot = json!({});
        add_task_context(&paths, &mut snapshot);
        let tasks = snapshot["tasks"].as_array().unwrap();
        let monitor_task = tasks
            .iter()
            .find(|task| task["task_id"] == "monitor-group")
            .unwrap();

        assert_eq!(
            monitor_task["source_context"]["kind"],
            "telegram_group_message"
        );
        assert_eq!(
            monitor_task["source_context"]["delivery_target"]["chat_kind"],
            "group"
        );
    }

    #[test]
    fn task_context_promotes_legacy_telegram_context_messages_to_source_messages() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let workflow = workflow_root(&paths);
        std::fs::create_dir_all(&workflow).unwrap();
        std::fs::write(
            workflow.join("monitor_tasks.json"),
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "monitor-f1",
                    "subject": "简报今年 F1 的竞争格局",
                    "description": "联系人要求简报今年 F1 的竞争格局。",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "monitor_envelope_id": "env-f1",
                        "source_text": "简报下今年F1的竞争格局",
                        "source_message_id": 53953,
                        "source_context": {
                            "kind": "telegram_direct_message",
                            "sender": { "name": "博阿 杜" },
                            "context_messages": [
                                { "from": "them", "direction": "incoming", "text": "old 1", "message_id": 1 },
                                { "from": "them", "direction": "incoming", "text": "old 2", "message_id": 2 },
                                { "from": "them", "direction": "incoming", "text": "old 3", "message_id": 3 },
                                { "from": "them", "direction": "incoming", "text": "old 4", "message_id": 4 },
                                { "from": "them", "direction": "incoming", "text": "old 5", "message_id": 5 },
                                { "from": "them", "direction": "incoming", "text": "old 6", "message_id": 6 },
                                { "from": "them", "direction": "incoming", "text": "在吗", "message_id": 53950 },
                                { "from": "me", "direction": "outgoing", "text": "在", "message_id": 53952 }
                            ]
                        },
                        "monitor": {
                            "schema_version": 2,
                            "kind": "telegram.reply",
                            "source": {
                                "connector_slug": "telegram-login",
                                "connection_slug": "telegram-user",
                                "chat_id": 8759047281_i64,
                                "chat_kind": "user",
                                "message_id": 53953,
                                "sender_name": "博阿 杜",
                                "text": "简报下今年F1的竞争格局"
                            },
                            "action": {
                                "type": "telegram_reply_draft",
                                "approval": "draft_then_send"
                            }
                        }
                    }
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let mut snapshot = json!({});
        add_task_context(&paths, &mut snapshot);
        let tasks = snapshot["tasks"].as_array().unwrap();
        let monitor_task = tasks
            .iter()
            .find(|task| task["task_id"] == "monitor-f1")
            .unwrap();
        let source_messages = monitor_task["source_messages"].as_array().unwrap();

        assert_eq!(source_messages.len(), 8);
        assert_eq!(source_messages[0]["text"], "old 2");
        assert_eq!(source_messages[6]["text"], "在");
        assert_eq!(source_messages[6]["direction"], "outgoing");
        assert_eq!(source_messages[7]["text"], "简报下今年F1的竞争格局");
        assert_eq!(source_messages[7]["message_id"], 53953);
    }

    #[test]
    fn issue_682_group_message_resolves_actual_sender_contact() {
        // A telegram GROUP message from member B (in a group owned by A) must
        // resolve to B's contact id, never the group/chat or its owner
        // (agentenv/monorepo#682). The subscriber stamps sender_id as B's i64;
        // the render layer must carry it into source_context.sender.id and the
        // snapshot must derive exactly B's contact id.
        let group_chat_id = -1_001_234_567_890i64; // a group chat id, never a contact
        let sender_b = 5_229_190_700i64;
        let metadata: Map<String, Value> = serde_json::from_value(json!({
            "_monitor": true,
            "monitor_connection": "telegram-user",
            "monitor_connector": "telegram-login",
            "monitor": {
                "schema_version": 2,
                "kind": "telegram.reply",
                "source": {
                    "connector_slug": "telegram-login",
                    "connection_slug": "telegram-user",
                    "chat_id": group_chat_id,
                    "chat_kind": "group",
                    "sender_id": sender_b,
                    "message_id": 6090
                },
                "action": { "type": "draft_then_approve" }
            }
        }))
        .unwrap();
        let context = monitor_source_context(&metadata).expect("group message context");
        assert_eq!(
            context["sender"]["id"],
            json!(sender_b.to_string()),
            "render must carry the actual group-message sender id"
        );
        let ids = sender_avatar_contact_ids(&metadata, &context);
        assert_eq!(
            ids,
            vec![format!("telegram-user-id@{sender_b}")],
            "group message must attribute to the actual sender B, never the group/chat"
        );
    }

    #[test]
    fn issue_682_numeric_sender_id_resolves_to_contact_end_to_end() {
        // Subscriber-shaped identity arrives as i64. The render layer coerces it
        // into source_context.sender.id (string) and the snapshot derives the
        // telegram-user-id contact id — locked end to end (agentenv/monorepo#682).
        let sender = 8_759_047_281i64;
        let metadata: Map<String, Value> = serde_json::from_value(json!({
            "_monitor": true,
            "monitor_connection": "telegram-user",
            "monitor_connector": "telegram-login",
            "monitor": {
                "schema_version": 2,
                "kind": "telegram.reply",
                "source": {
                    "connector_slug": "telegram-login",
                    "connection_slug": "telegram-user",
                    "chat_id": sender,
                    "chat_kind": "user",
                    "sender_id": sender,
                    "message_id": 6090
                },
                "action": { "type": "draft_then_approve" }
            }
        }))
        .unwrap();
        let context = monitor_source_context(&metadata).expect("numeric sender context");
        assert_eq!(
            context["sender"]["id"],
            json!(sender.to_string()),
            "numeric sender id must render as a string"
        );
        let ids = sender_avatar_contact_ids(&metadata, &context);
        assert!(
            ids.contains(&format!("telegram-user-id@{sender}")),
            "numeric sender id must derive its telegram-user-id contact, got {ids:?}"
        );
    }

    #[test]
    fn issue_682_multi_sender_burst_has_no_single_contact_attribution() {
        // A mixed-sender burst stamps the plural `sender_ids` set (>=2 distinct
        // members). It has no single contact, so the task must not be attributed
        // to one member — even a leaked singular sender hint must not win over
        // the known multi-sender set (agentenv/monorepo#682, #655).
        let metadata: Map<String, Value> = serde_json::from_value(json!({
            "_monitor": true,
            "monitor_connection": "telegram-user",
            "monitor_connector": "telegram-login",
            "chat_id": -1_001_234_567_890i64,
            "chat_kind": "group",
            "sender_ids": [42, 43],
            // A stray singular hint must not fabricate a single-contact link.
            "sender_id": 42,
            "source_message_ids": [6090, 6091]
        }))
        .unwrap();
        let context = monitor_source_context(&metadata)
            .expect("a mixed-sender burst still derives a source context");
        let ids = sender_avatar_contact_ids(&metadata, &context);
        assert!(
            ids.is_empty(),
            "mixed-sender burst must not attribute to one contact, got {ids:?}"
        );
    }

    #[test]
    fn issue_682_legacy_numeric_chat_id_derives_human_gated_completion_policy() {
        // The number-tolerant derive also feeds default_monitor_completion_policy:
        // a legacy reply-shaped telegram task with a numeric chat_id must now
        // ADVERTISE the human gating the daemon already enforces (its
        // enforcement leg was number-tolerant all along) instead of showing no
        // policy (agentenv/monorepo#682).
        let metadata: Map<String, Value> = serde_json::from_value(json!({
            "_monitor": true,
            "monitor_connection": "telegram-user",
            "monitor_connector": "telegram-login",
            "chat_id": 42i64,
            "chat_kind": "user",
            "sender_id": 42i64,
            "actions": [{"name": "reply", "prompt": "Reply to the sender"}]
        }))
        .unwrap();
        assert_eq!(
            monitor_completion_policy(&metadata),
            Some(json!({
                "mode": "draft_then_approve",
                "requires_human_approval": true,
                "requires_receipt": true,
            }))
        );
    }

    #[test]
    fn issue_682_legacy_numeric_ids_still_derive_source_context() {
        // A legacy task without a typed contract falls back to
        // derived_monitor_source_context; ids stamped as i64 must not kill the
        // whole derivation (agentenv/monorepo#682).
        let metadata: Map<String, Value> = serde_json::from_value(json!({
            "_monitor": true,
            "monitor_connection": "telegram-user",
            "monitor_connector": "telegram-login",
            "chat_id": 42i64,
            "chat_kind": "user",
            "sender_id": 42i64
        }))
        .unwrap();
        let context = monitor_source_context(&metadata).expect("legacy numeric context");
        assert_eq!(context["sender"]["id"], json!("42"));
        assert_eq!(context["delivery_target"]["chat_id"], json!("42"));
        let ids = sender_avatar_contact_ids(&metadata, &context);
        assert_eq!(ids, vec!["telegram-user-id@42".to_string()]);
    }
}
