//! Task snapshot helpers for the desktop workflow/task screens.

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::normalize_contact_id;
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
    append_task_file(
        &workflow.join("tasks.json"),
        "agent",
        "workspace",
        "workspace",
        &mut tasks,
        &mut errors,
    );
    append_scoped_agent_tasks(
        &workflow.join("sessions"),
        "session",
        &mut tasks,
        &mut errors,
    );
    append_scoped_agent_tasks(
        &workflow.join("team_tasks"),
        "team",
        &mut tasks,
        &mut errors,
    );
    append_task_file(
        &workflow.join("monitor_tasks.json"),
        "monitor",
        "monitor",
        "monitors",
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
    tasks: &mut Vec<Value>,
    errors: &mut Vec<String>,
) {
    match load_task_file(path, source, scope, scope_label) {
        Ok(rows) => tasks.extend(rows),
        Err(error) => errors.push(error.to_string()),
    }
}

fn load_task_file(path: &Path, source: &str, scope: &str, scope_label: &str) -> Result<Vec<Value>> {
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
        .map(|task| task_json(task, source, scope, scope_label))
        .collect())
}

fn task_json(task: TaskSnapshotRecord, source: &str, scope: &str, scope_label: &str) -> Value {
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
        "source_context": monitor_source_context(&task.metadata),
        "completion_policy": monitor_completion_policy(&task.metadata),
        "pending_reply": metadata_value(
            &task.metadata,
            &["pending_reply", "pendingReply"],
            &["reply", "draft"]
        ),
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
        "actions": monitor_actions(&task.metadata),
        "possible_ignore_reasons": monitor_ignore_reasons(&task.metadata),
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

fn metadata_string(
    metadata: &Map<String, Value>,
    top_level_keys: &[&str],
    monitor_keys: &[&str],
) -> Option<String> {
    top_level_keys
        .iter()
        .find_map(|key| string_value(metadata.get(*key)))
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(Value::as_object)
                .and_then(|monitor| {
                    monitor_keys
                        .iter()
                        .find_map(|key| string_value(monitor.get(*key)))
                })
        })
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

pub(super) fn monitor_source_context(metadata: &Map<String, Value>) -> Option<Value> {
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
    let sender = context.get("sender").and_then(Value::as_object);
    let mut ids = BTreeSet::new();
    if let Some(sender_id) = sender
        .and_then(|sender| scalar_string(sender.get("id")))
        .or_else(|| {
            metadata_scalar_string(
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
            metadata_scalar_string(
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

fn metadata_scalar_string(
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
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let mut snapshot = json!({});
        add_task_context(&paths, &mut snapshot);
        let tasks = snapshot["tasks"].as_array().unwrap();

        assert_eq!(tasks.len(), 3);
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
}
