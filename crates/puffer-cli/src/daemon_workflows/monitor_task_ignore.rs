use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{installed_workflow_runner, FilterSpec, WorkflowBindingSpec};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Deserialize)]
struct MonitorTaskIgnoreParams {
    #[serde(alias = "taskId")]
    task_id: String,
    #[serde(default)]
    reason: Option<String>,
}

/// Marks a monitor-created task ignored and returns the refreshed task snapshot.
pub(crate) fn handle_monitor_task_ignore(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: MonitorTaskIgnoreParams =
        serde_json::from_value(params.clone()).context("invalid monitor task ignore params")?;
    let task_id = non_empty(params.task_id.as_str()).context("missing task_id")?;
    let reason = normalize_reason(params.reason.as_deref());
    let analysis_runner = installed_workflow_runner();
    let path = monitor_tasks_path(paths);
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut store: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid monitor task store {}", path.display()))?;
    let tasks = store
        .get_mut("tasks")
        .and_then(Value::as_array_mut)
        .context("monitor task store missing tasks array")?;
    let task = tasks
        .iter_mut()
        .find(|task| task_id_matches(task, task_id))
        .with_context(|| format!("monitor task `{task_id}` not found"))?;
    let subject = task_string(task, &["subject"]).unwrap_or_else(|| task_id.to_string());
    let description = task_string(task, &["description"]).unwrap_or_default();

    let task_object = task
        .as_object_mut()
        .context("monitor task entry must be an object")?;
    let analysis_started = analysis_runner.is_some();
    let (memory_path, metadata_snapshot, ignore_filter) = {
        let metadata = task_metadata(task_object)?;
        let memory_path = monitor_memory_path(paths, metadata);
        let ignore_filter = ensure_monitor_ignore_filter(metadata)?;
        append_monitor_memory(
            &memory_path,
            task_id,
            &subject,
            &description,
            &reason,
            ignore_filter.as_ref(),
        )?;
        metadata.insert("ignored".to_string(), Value::Bool(true));
        metadata.insert("ignore_reason".to_string(), Value::String(reason.clone()));
        metadata.insert(
            "monitor_memory_path".to_string(),
            Value::String(memory_path.display().to_string()),
        );
        metadata.insert(
            "ignore_analysis_started".to_string(),
            Value::Bool(analysis_started),
        );
        if let Some(filter) = &ignore_filter {
            metadata.insert("ignore_filter".to_string(), filter.clone());
        }
        (memory_path, metadata.clone(), ignore_filter)
    };
    let memory_content = fs::read_to_string(&memory_path).unwrap_or_default();
    task_object.insert("status".to_string(), Value::String("completed".to_string()));
    task_object.insert("updated_at_ms".to_string(), Value::from(now_ms()));
    fs::write(&path, serde_json::to_string_pretty(&store)?)
        .with_context(|| format!("failed to write {}", path.display()))?;
    start_ignore_analysis_agent(
        analysis_runner,
        task_id.to_string(),
        subject,
        description,
        reason,
        metadata_snapshot,
        memory_path,
        memory_content,
        ignore_filter,
    );
    super::handle_workflow_list(paths)
}

/// Backfills pre-agent ignore filters from tasks that were ignored before
/// the router understood ignore filters.
pub(crate) fn sync_monitor_ignore_filters_from_tasks(paths: &ConfigPaths) -> Result<()> {
    let path = monitor_tasks_path(paths);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let mut store: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid monitor task store {}", path.display()))?;
    let Some(tasks) = store.get_mut("tasks").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let mut ignored_filters = Vec::new();
    for task in tasks.iter() {
        let Some(metadata) = task.get("metadata").and_then(Value::as_object) else {
            continue;
        };
        if metadata_bool(metadata, "ignored") {
            if let Some(filter) = ensure_monitor_ignore_filter(metadata)? {
                ignored_filters.push(filter);
            }
        }
    }
    let mut changed = false;
    for task in tasks.iter_mut() {
        let Some(task_object) = task.as_object_mut() else {
            continue;
        };
        let metadata = task_metadata(task_object)?;
        if metadata_bool(metadata, "ignored") {
            continue;
        }
        let task_filter_json = monitor_ignore_filter(metadata)
            .map(serde_json::to_value)
            .transpose()
            .context("serialize ignore filter")?;
        let matched_ignore_filter = task_filter_json
            .as_ref()
            .is_some_and(|task_filter| ignored_filters.iter().any(|filter| filter == task_filter));
        if !matched_ignore_filter {
            continue;
        }
        let memory_path = monitor_memory_path(paths, metadata);
        metadata.insert("ignored".to_string(), Value::Bool(true));
        metadata.insert(
            "ignore_reason".to_string(),
            Value::String("Matched previous monitor ignore filter.".to_string()),
        );
        if let Some(task_filter_json) = task_filter_json {
            metadata.insert("ignore_filter".to_string(), task_filter_json);
        }
        metadata.insert(
            "monitor_memory_path".to_string(),
            Value::String(memory_path.display().to_string()),
        );
        metadata.insert("ignore_analysis_started".to_string(), Value::Bool(false));
        task_object.insert("status".to_string(), Value::String("completed".to_string()));
        task_object.insert("updated_at_ms".to_string(), Value::from(now_ms()));
        changed = true;
    }
    if changed {
        fs::write(&path, serde_json::to_string_pretty(&store)?)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn normalize_reason(reason: Option<&str>) -> String {
    reason
        .and_then(non_empty)
        .unwrap_or("User ignored this monitored task.")
        .replace(['\n', '\r'], " ")
}

fn monitor_tasks_path(paths: &ConfigPaths) -> PathBuf {
    paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
        .join("monitor_tasks.json")
}

fn task_id_matches(task: &Value, task_id: &str) -> bool {
    task_string(task, &["task_id", "taskId", "id"]).as_deref() == Some(task_id)
}

fn task_string(task: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| task.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn task_metadata(task: &mut Map<String, Value>) -> Result<&mut Map<String, Value>> {
    if !matches!(task.get("metadata"), Some(Value::Object(_))) {
        task.insert("metadata".to_string(), Value::Object(Map::new()));
    }
    task.get_mut("metadata")
        .and_then(Value::as_object_mut)
        .context("monitor task metadata must be an object")
}

fn monitor_memory_path(paths: &ConfigPaths, metadata: &Map<String, Value>) -> PathBuf {
    metadata_string(
        metadata,
        &["monitor_memory_path", "monitorMemoryPath"],
        &["memory_path", "memoryPath"],
    )
    .map(PathBuf::from)
    .unwrap_or_else(|| {
        let connection = metadata_string(
            metadata,
            &["monitor_connection", "monitorConnection"],
            &["connection", "connection_slug", "connectionSlug"],
        )
        .unwrap_or_else(|| "memory".to_string());
        paths
            .workspace_config_dir
            .join("runtime")
            .join("monitors")
            .join(format!("{connection}.md"))
    })
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
    metadata.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn string_value(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn append_monitor_memory(
    path: &Path,
    task_id: &str,
    subject: &str,
    description: &str,
    reason: &str,
    ignore_filter: Option<&Value>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let filter_note = ignore_filter
        .map(|filter| {
            let body = serde_json::to_string_pretty(filter).unwrap_or_else(|_| filter.to_string());
            format!("\nPre-agent ignore filter:\n```json\n{body}\n```\n")
        })
        .unwrap_or_default();
    let entry = format!(
        "\n## Ignored Task: {task_id}\n\nReason: {reason}\n\nTitle: {subject}\n\nDescription:\n{description}\n{filter_note}"
    );
    use std::io::Write as _;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(entry.as_bytes())
        .with_context(|| format!("failed to append {}", path.display()))
}

fn ensure_monitor_ignore_filter(metadata: &Map<String, Value>) -> Result<Option<Value>> {
    let Some(filter) = monitor_ignore_filter(metadata) else {
        return Ok(None);
    };
    let filter_json = serde_json::to_value(&filter).context("serialize ignore filter")?;
    let Some(connection_slug) = monitor_connection(metadata) else {
        return Ok(Some(filter_json));
    };
    let Ok(manager) = subscription_manager() else {
        return Ok(Some(filter_json));
    };
    let Some(mut binding) = manager.store().get(&monitor_slug(&connection_slug)) else {
        return Ok(Some(filter_json));
    };
    if !binding_has_ignore_filter(&binding, &filter_json) {
        binding.ignore_filters.push(filter);
        manager.store().upsert(binding)?;
        manager.refresh_connection_consumers()?;
    }
    Ok(Some(filter_json))
}

fn monitor_ignore_filter(metadata: &Map<String, Value>) -> Option<FilterSpec> {
    explicit_monitor_ignore_filter(metadata).or_else(|| derived_monitor_ignore_filter(metadata))
}

fn explicit_monitor_ignore_filter(metadata: &Map<String, Value>) -> Option<FilterSpec> {
    let keys = [
        "monitor_ignore_filter",
        "monitorIgnoreFilter",
        "event_ignore_filter",
        "eventIgnoreFilter",
        "ignore_filter",
        "ignoreFilter",
    ];
    keys.iter()
        .find_map(|key| exact_filter_from_value(metadata.get(*key)?))
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(Value::as_object)
                .and_then(|monitor| {
                    keys.iter()
                        .find_map(|key| exact_filter_from_value(monitor.get(*key)?))
                })
        })
}

fn exact_filter_from_value(value: &Value) -> Option<FilterSpec> {
    let shape = value.as_object()?;
    exact_filter_shape_is_safe(shape).then(|| FilterSpec::Json(Value::Object(shape.clone())))
}

fn derived_monitor_ignore_filter(metadata: &Map<String, Value>) -> Option<FilterSpec> {
    let candidates = metadata_identity_candidates(metadata);
    let scope = choose_identity_candidate(&candidates, IdentityRole::Scope)?;
    let actor = choose_identity_candidate(&candidates, IdentityRole::Actor)?;
    if scope.key == actor.key {
        return None;
    }

    let mut shape = Map::new();
    shape.insert(scope.key.clone(), scope.value.clone());
    shape.insert(actor.key.clone(), actor.value.clone());
    exact_filter_shape_is_safe(&shape).then(|| FilterSpec::Json(Value::Object(shape)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdentityRole {
    Scope,
    Actor,
}

#[derive(Clone, Debug)]
struct IgnoreCandidate {
    key: String,
    value: Value,
    role: IdentityRole,
    strength: u8,
}

fn metadata_identity_candidates(metadata: &Map<String, Value>) -> Vec<IgnoreCandidate> {
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    for (key, value) in metadata {
        push_identity_candidate(&mut candidates, &mut seen, key, value);
    }
    for container_key in ["payload", "event_payload", "eventPayload"] {
        let Some(payload) = metadata.get(container_key).and_then(Value::as_object) else {
            continue;
        };
        for (key, value) in payload {
            push_identity_candidate(&mut candidates, &mut seen, key, value);
        }
    }
    candidates
}

fn push_identity_candidate(
    candidates: &mut Vec<IgnoreCandidate>,
    seen: &mut BTreeSet<String>,
    key: &str,
    value: &Value,
) {
    let normalized_key = normalize_filter_key(key);
    if !seen.insert(normalized_key.clone()) {
        return;
    }
    let Some(value) = exact_filter_scalar(value) else {
        return;
    };
    let Some(role) = identity_role(&normalized_key) else {
        return;
    };
    if filter_key_is_excluded(&normalized_key) {
        return;
    }
    let strength = identity_key_strength(&normalized_key);
    if strength == 0 {
        return;
    }
    candidates.push(IgnoreCandidate {
        key: key.to_string(),
        value,
        role,
        strength,
    });
}

fn choose_identity_candidate(
    candidates: &[IgnoreCandidate],
    role: IdentityRole,
) -> Option<&IgnoreCandidate> {
    candidates
        .iter()
        .filter(|candidate| candidate.role == role)
        .max_by_key(|candidate| (candidate.strength, std::cmp::Reverse(candidate.key.len())))
}

fn exact_filter_shape_is_safe(shape: &Map<String, Value>) -> bool {
    let mut leaves = Vec::new();
    if !collect_exact_filter_leaves(Vec::new(), &Value::Object(shape.clone()), &mut leaves) {
        return false;
    }
    if leaves.len() < 2 {
        return false;
    }

    let mut scope_is_stable = false;
    let mut actor_is_stable = false;
    for leaf in leaves {
        if filter_key_is_excluded(&leaf) {
            return false;
        }
        match identity_role(&leaf) {
            Some(IdentityRole::Scope) => scope_is_stable |= identity_key_strength(&leaf) >= 2,
            Some(IdentityRole::Actor) => actor_is_stable |= identity_key_strength(&leaf) >= 2,
            None => return false,
        }
    }
    scope_is_stable && actor_is_stable
}

fn collect_exact_filter_leaves(path: Vec<String>, value: &Value, leaves: &mut Vec<String>) -> bool {
    match value {
        Value::Object(object) => {
            if object.is_empty() {
                return false;
            }
            object.iter().all(|(key, value)| {
                let mut child_path = path.clone();
                child_path.push(normalize_filter_key(key));
                collect_exact_filter_leaves(child_path, value, leaves)
            })
        }
        Value::String(value) => {
            if value.trim().is_empty() || path.is_empty() {
                return false;
            }
            leaves.push(path.join("_"));
            true
        }
        Value::Number(_) => {
            if path.is_empty() {
                return false;
            }
            leaves.push(path.join("_"));
            true
        }
        _ => false,
    }
}

fn exact_filter_scalar(value: &Value) -> Option<Value> {
    match value {
        Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| Value::String(value.to_string()))
        }
        Value::Number(_) => Some(value.clone()),
        _ => None,
    }
}

fn normalize_filter_key(key: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_separator = false;
    for (index, ch) in key.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 && !previous_was_separator {
                normalized.push('_');
            }
            normalized.push(ch.to_ascii_lowercase());
            previous_was_separator = false;
        } else if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator {
            normalized.push('_');
            previous_was_separator = true;
        }
    }
    normalized.trim_matches('_').to_string()
}

fn filter_key_is_excluded(key: &str) -> bool {
    matches!(
        key,
        "_monitor"
            | "monitor_connection"
            | "monitor_connector"
            | "monitor_memory_path"
            | "monitor_ignore_filter"
            | "event_ignore_filter"
            | "ignore_filter"
            | "ignored"
            | "ignore_reason"
            | "ignore_analysis_started"
            | "actions"
            | "possible_ignore_reasons"
            | "message_id"
            | "event_id"
            | "dedup_key"
            | "date"
            | "date_ms"
            | "timestamp"
            | "received_at"
            | "received_at_ms"
            | "subject"
            | "description"
            | "body"
            | "text"
            | "content"
            | "title"
    ) || key.ends_with("_message_id")
        || key.ends_with("_event_id")
        || key.ends_with("_timestamp")
        || key.ends_with("_date")
}

fn identity_role(key: &str) -> Option<IdentityRole> {
    let parts = key.split('_').collect::<Vec<_>>();
    let has_actor = parts.iter().any(|part| {
        matches!(
            *part,
            "sender" | "from" | "author" | "actor" | "user" | "account" | "bot" | "owner"
        )
    });
    if has_actor {
        return Some(IdentityRole::Actor);
    }
    let has_scope = parts.iter().any(|part| {
        matches!(
            *part,
            "chat"
                | "channel"
                | "room"
                | "conversation"
                | "thread"
                | "mailbox"
                | "repo"
                | "repository"
                | "project"
                | "workspace"
                | "guild"
                | "server"
                | "stream"
                | "group"
        )
    });
    has_scope.then_some(IdentityRole::Scope)
}

fn identity_key_strength(key: &str) -> u8 {
    let parts = key.split('_').collect::<Vec<_>>();
    if parts
        .iter()
        .any(|part| matches!(*part, "id" | "uuid" | "username" | "email" | "handle"))
    {
        return 3;
    }
    if parts
        .iter()
        .any(|part| matches!(*part, "slug" | "url" | "address" | "key"))
    {
        return 2;
    }
    if parts.iter().any(|part| matches!(*part, "name" | "title")) {
        return 1;
    }
    0
}

fn monitor_connection(metadata: &Map<String, Value>) -> Option<String> {
    metadata_string(
        metadata,
        &["monitor_connection", "monitorConnection"],
        &["connection", "connection_slug", "connectionSlug"],
    )
    .or_else(|| {
        metadata_string(
            metadata,
            &["monitor_memory_path", "monitorMemoryPath"],
            &["memory_path", "memoryPath"],
        )
        .and_then(|path| {
            Path::new(&path)
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
    })
}

fn monitor_slug(connection_slug: &str) -> String {
    format!("monitor-{connection_slug}")
}

fn binding_has_ignore_filter(binding: &WorkflowBindingSpec, filter_json: &Value) -> bool {
    binding
        .ignore_filters
        .iter()
        .any(|filter| matches!(serde_json::to_value(filter), Ok(value) if &value == filter_json))
}

fn start_ignore_analysis_agent(
    runner: Option<Arc<dyn puffer_subscriptions::WorkflowActionRunner>>,
    task_id: String,
    subject: String,
    description: String,
    reason: String,
    metadata: Map<String, Value>,
    memory_path: PathBuf,
    memory_content: String,
    ignore_filter: Option<Value>,
) {
    let Some(runner) = runner else {
        return;
    };
    let prompt = ignore_analysis_prompt(
        &task_id,
        &subject,
        &description,
        &reason,
        &metadata,
        &memory_path,
        &memory_content,
        ignore_filter.as_ref(),
    );
    let trigger = json!({
        "type": "monitor_task_ignore",
        "task_id": task_id,
        "subject": subject,
        "description": description,
        "reason": reason,
        "metadata": metadata,
        "monitor_memory_path": memory_path.display().to_string(),
        "ignore_filter": ignore_filter,
    });
    let _ = thread::Builder::new()
        .name("puffer-ignore-analysis".to_string())
        .spawn(move || {
            if let Err(error) = runner.ignore_analysis_agent(&prompt, None, trigger) {
                eprintln!("monitor ignore analysis failed: {error:#}");
            }
        });
}

fn ignore_analysis_prompt(
    task_id: &str,
    subject: &str,
    description: &str,
    reason: &str,
    metadata: &Map<String, Value>,
    memory_path: &Path,
    memory_content: &str,
    ignore_filter: Option<&Value>,
) -> String {
    let metadata_json = serde_json::to_string_pretty(metadata).unwrap_or_else(|_| "{}".to_string());
    let filter_json = ignore_filter
        .map(|filter| serde_json::to_string_pretty(filter).unwrap_or_else(|_| filter.to_string()))
        .unwrap_or_else(|| "null".to_string());
    format!(
        "You are the monitor ignore-analysis agent. This is a read-only analysis turn with no tools.\n\nA user ignored monitor task `{task_id}`. Analyze whether future connector events like this should be suppressed before the monitor triage agent starts. You cannot apply filters, edit files, create tasks, or send connector replies. The daemon owns filter writes and only installs validated exact JSON filters with both a stable scope identity and a stable actor/source identity from connector event metadata.\n\nIf the shown filter looks too broad or wrong, say so. If no filter was installed, explain what stable metadata is missing. If monitor memory should contain a clearer general rule, propose the exact memory text, but do not claim that you wrote it. The daemon does not automatically apply your output.\n\nIgnored reason: {reason}\n\nTask title: {subject}\n\nTask description:\n{description}\n\nPre-agent ignore filter installed by daemon:\n```json\n{filter_json}\n```\n\nTask metadata:\n```json\n{metadata_json}\n```\n\nCurrent monitor memory at `{}`:\n```md\n{}\n```\n\nReturn a concise summary and any proposed memory text or filter concern.",
        memory_path.display(),
        memory_content
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::{
        handle_monitor_task_ignore, monitor_ignore_filter, monitor_tasks_path,
        sync_monitor_ignore_filters_from_tasks,
    };
    use puffer_config::ConfigPaths;
    use serde_json::{json, Value};
    use std::fs;

    #[test]
    fn ignore_monitor_task_marks_task_and_appends_memory() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let memory_path = tempdir.path().join("feed-connection.md");
        let task_path = monitor_tasks_path(&paths);
        fs::create_dir_all(task_path.parent().unwrap()).unwrap();
        fs::write(
            &task_path,
            serde_json::to_string_pretty(&json!({
                "tasks": [{
                    "task_id": "monitor-1",
                    "subject": "Noisy alert",
                    "description": "Ignore this class of alert.",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "feed-connection",
                        "monitor_memory_path": memory_path.display().to_string(),
                        "possibleIgnoreReasons": ["low signal alert"]
                    },
                    "started_at_ms": 10
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let snapshot = handle_monitor_task_ignore(
            &paths,
            &json!({
                "taskId": "monitor-1",
                "reason": "low signal alert"
            }),
        )
        .unwrap();
        let tasks = snapshot["tasks"].as_array().unwrap();
        assert_eq!(tasks[0]["ignored"], Value::Bool(true));
        assert_eq!(tasks[0]["status"], "completed");
        assert_eq!(tasks[0]["task_id"], "monitor-1");

        let stored: Value = serde_json::from_str(&fs::read_to_string(task_path).unwrap()).unwrap();
        assert_eq!(
            stored["tasks"][0]["metadata"]["ignore_reason"],
            "low signal alert"
        );
        let memory = fs::read_to_string(memory_path).unwrap();
        assert!(memory.contains("Ignored Task: monitor-1"));
        assert!(memory.contains("Reason: low signal alert"));
        assert!(memory.contains("Title: Noisy alert"));
    }

    #[test]
    fn ignore_filter_derives_scoped_actor_identity() {
        let metadata = json!({
            "room_id": "security-room",
            "author_handle": "alert-bot"
        });
        let filter = monitor_ignore_filter(metadata.as_object().unwrap()).unwrap();

        assert_eq!(
            serde_json::to_value(filter).unwrap(),
            json!({
                "room_id": "security-room",
                "author_handle": "alert-bot"
            })
        );
    }

    #[test]
    fn ignore_filter_accepts_explicit_safe_exact_filter() {
        let metadata = json!({
            "monitor_ignore_filter": {
                "conversation_id": "room-1",
                "from_email": "alerts@example.test"
            }
        });
        let filter = monitor_ignore_filter(metadata.as_object().unwrap()).unwrap();

        assert_eq!(
            serde_json::to_value(filter).unwrap(),
            json!({
                "conversation_id": "room-1",
                "from_email": "alerts@example.test"
            })
        );
    }

    #[test]
    fn ignore_filter_rejects_single_or_unstable_identity() {
        let single = json!({
            "author_handle": "alert-bot"
        });
        assert!(monitor_ignore_filter(single.as_object().unwrap()).is_none());

        let unstable = json!({
            "room_id": "security-room",
            "sender_name": "Alert Bot"
        });
        assert!(monitor_ignore_filter(unstable.as_object().unwrap()).is_none());
    }

    #[test]
    fn sync_marks_pending_tasks_matching_prior_ignore_filter() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let task_path = monitor_tasks_path(&paths);
        fs::create_dir_all(task_path.parent().unwrap()).unwrap();
        fs::write(
            &task_path,
            serde_json::to_string_pretty(&json!({
                "tasks": [
                    {
                        "task_id": "monitor-1",
                        "subject": "ignored bot",
                        "description": "old ignored example",
                        "status": "completed",
                        "metadata": {
                            "_monitor": true,
                            "monitor_connection": "feed-connection",
                            "room_id": "security-room",
                            "author_handle": "alert-bot",
                            "ignored": true
                        }
                    },
                    {
                        "task_id": "monitor-2",
                        "subject": "same bot",
                        "description": "new pending example",
                        "status": "pending",
                        "metadata": {
                            "_monitor": true,
                            "monitor_connection": "feed-connection",
                            "room_id": "security-room",
                            "author_handle": "alert-bot"
                        }
                    },
                    {
                        "task_id": "monitor-3",
                        "subject": "other sender",
                        "description": "should stay pending",
                        "status": "pending",
                        "metadata": {
                            "_monitor": true,
                            "monitor_connection": "feed-connection",
                            "room_id": "security-room",
                            "author_handle": "human-user"
                        }
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        sync_monitor_ignore_filters_from_tasks(&paths).unwrap();

        let stored: Value = serde_json::from_str(&fs::read_to_string(task_path).unwrap()).unwrap();
        assert_eq!(stored["tasks"][1]["status"], "completed");
        assert_eq!(stored["tasks"][1]["metadata"]["ignored"], true);
        assert_eq!(
            stored["tasks"][1]["metadata"]["ignore_reason"],
            "Matched previous monitor ignore filter."
        );
        assert_eq!(stored["tasks"][2]["status"], "pending");
        assert_eq!(stored["tasks"][2]["metadata"]["ignored"], Value::Null);
    }

    #[test]
    fn sync_does_not_parse_unstructured_task_text() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let task_path = monitor_tasks_path(&paths);
        fs::create_dir_all(task_path.parent().unwrap()).unwrap();
        fs::write(
            &task_path,
            serde_json::to_string_pretty(&json!({
                "tasks": [
                    {
                        "task_id": "monitor-1",
                        "subject": "ignored bot",
                        "description": "old ignored example",
                        "status": "completed",
                        "metadata": {
                            "_monitor": true,
                            "monitor_connection": "feed-connection",
                            "room_id": "security-room",
                            "author_handle": "alert-bot",
                            "ignored": true
                        }
                    },
                    {
                        "task_id": "monitor-2",
                        "subject": "Review alert from alert bot",
                        "description": "Source author is alert-bot in security-room.",
                        "status": "pending",
                        "metadata": {
                            "_monitor": true,
                            "monitor_connection": "feed-connection"
                        }
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        sync_monitor_ignore_filters_from_tasks(&paths).unwrap();

        let stored: Value = serde_json::from_str(&fs::read_to_string(task_path).unwrap()).unwrap();
        assert_eq!(stored["tasks"][1]["status"], "pending");
        assert_eq!(stored["tasks"][1]["metadata"]["ignored"], Value::Null);
    }
}
