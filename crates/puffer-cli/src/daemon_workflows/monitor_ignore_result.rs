use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Persists the async ignore-analysis agent outcome onto a monitor task.
pub(crate) fn write_ignore_analysis_result(
    path: &Path,
    task_id: &str,
    status: &str,
    output: Option<&str>,
    error: Option<&str>,
    usage: Option<puffer_subscriptions::ActionUsage>,
    started_at_ms: u64,
) -> Result<()> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut store: Value =
        serde_json::from_str(&raw).with_context(|| format!("invalid {}", path.display()))?;
    let tasks = store
        .get_mut("tasks")
        .and_then(Value::as_array_mut)
        .context("monitor task store missing tasks array")?;
    let task = tasks
        .iter_mut()
        .find(|task| task_id_matches(task, task_id))
        .with_context(|| format!("monitor task `{task_id}` not found"))?;
    let metadata = task_metadata(
        task.as_object_mut()
            .context("monitor task must be an object")?,
    );
    metadata.insert(
        "ignore_analysis_status".into(),
        Value::String(status.into()),
    );
    metadata.insert(
        "ignore_analysis_started_at_ms".into(),
        Value::from(started_at_ms),
    );
    metadata.insert(
        "ignore_analysis_completed_at_ms".into(),
        Value::from(now_ms()),
    );
    if let Some(output) = output {
        metadata.insert(
            "ignore_analysis_result".into(),
            Value::String(output.into()),
        );
    }
    if let Some(error) = error {
        metadata.insert("ignore_analysis_error".into(), Value::String(error.into()));
    }
    if let Some(usage) = usage {
        metadata.insert(
            "ignore_analysis_usage".into(),
            serde_json::to_value(usage_with_total(usage))?,
        );
    }
    fs::write(path, serde_json::to_string_pretty(&store)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn usage_with_total(usage: puffer_subscriptions::ActionUsage) -> Value {
    serde_json::json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cache_read_tokens": usage.cache_read_tokens,
        "cache_creation_tokens": usage.cache_creation_tokens,
        "spent_tokens": usage.spent_tokens(),
    })
}

fn task_id_matches(task: &Value, task_id: &str) -> bool {
    ["task_id", "taskId", "id"]
        .iter()
        .find_map(|key| task.get(*key).and_then(Value::as_str))
        .map(str::trim)
        == Some(task_id)
}

fn task_metadata(task: &mut Map<String, Value>) -> &mut Map<String, Value> {
    if !matches!(task.get("metadata"), Some(Value::Object(_))) {
        task.insert("metadata".into(), Value::Object(Map::new()));
    }
    task.get_mut("metadata")
        .and_then(Value::as_object_mut)
        .expect("metadata object was just inserted")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
