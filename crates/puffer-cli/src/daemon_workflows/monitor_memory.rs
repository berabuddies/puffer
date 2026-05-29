use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_MEMORY_CHARS: usize = 24_000;

#[derive(Debug, Deserialize)]
struct MonitorMemorySaveParams {
    #[serde(alias = "connectionSlug")]
    connection_slug: String,
    content: String,
}

/// Adds monitor memory files to a workflow/task snapshot.
pub(crate) fn add_monitor_memory_context(paths: &ConfigPaths, snapshot: &mut Value) {
    let Some(object) = snapshot.as_object_mut() else {
        return;
    };
    match load_monitor_memories(paths) {
        Ok(memories) => {
            object.insert("monitor_memories".to_string(), Value::Array(memories));
            object.insert("monitor_memory_error".to_string(), Value::Null);
        }
        Err(error) => {
            object.insert("monitor_memories".to_string(), Value::Array(Vec::new()));
            object.insert(
                "monitor_memory_error".to_string(),
                Value::String(error.to_string()),
            );
        }
    }
}

/// Saves one monitor memory file and returns the refreshed workflow snapshot.
pub(crate) fn handle_monitor_memory_save(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: MonitorMemorySaveParams =
        serde_json::from_value(params.clone()).context("invalid monitor memory save params")?;
    let connection_slug = valid_connection_slug(&params.connection_slug)?;
    let path = monitor_memory_dir(paths).join(format!("{connection_slug}.md"));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&path, params.content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    super::handle_workflow_list(paths)
}

fn load_monitor_memories(paths: &ConfigPaths) -> Result<Vec<Value>> {
    let dir = monitor_memory_dir(paths);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", dir.display()));
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read {}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        files.push(path);
    }
    files.sort();
    files.into_iter().map(memory_json).collect()
}

fn monitor_memory_dir(paths: &ConfigPaths) -> PathBuf {
    paths.workspace_config_dir.join("runtime").join("monitors")
}

fn valid_connection_slug(slug: &str) -> Result<&str> {
    let trimmed = slug.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
    {
        anyhow::bail!("invalid monitor memory connection slug");
    }
    Ok(trimmed)
}

fn memory_json(path: PathBuf) -> Result<Value> {
    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let (content, truncated) = truncate_memory(content);
    Ok(json!({
        "connection_slug": connection_slug_from_path(&path),
        "path": path.display().to_string(),
        "content": content,
        "truncated": truncated,
    }))
}

fn connection_slug_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("monitor")
        .to_string()
}

fn truncate_memory(content: String) -> (String, bool) {
    if content.chars().count() <= MAX_MEMORY_CHARS {
        return (content, false);
    }
    let truncated = content.chars().take(MAX_MEMORY_CHARS).collect();
    (truncated, true)
}

#[cfg(test)]
mod tests {
    use super::{add_monitor_memory_context, handle_monitor_memory_save};
    use puffer_config::ConfigPaths;
    use serde_json::{json, Value};
    use std::fs;

    #[test]
    fn monitor_memory_context_reads_markdown_files() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let memory_dir = paths.workspace_config_dir.join("runtime").join("monitors");
        fs::create_dir_all(&memory_dir).unwrap();
        fs::write(memory_dir.join("telegram-user.md"), "# memory").unwrap();

        let mut snapshot = json!({});
        add_monitor_memory_context(&paths, &mut snapshot);

        assert_eq!(snapshot["monitor_memory_error"], Value::Null);
        assert_eq!(
            snapshot["monitor_memories"][0]["connection_slug"],
            "telegram-user"
        );
        assert_eq!(snapshot["monitor_memories"][0]["content"], "# memory");
        assert_eq!(snapshot["monitor_memories"][0]["truncated"], false);
    }

    #[test]
    fn monitor_memory_save_updates_workspace_memory_file() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());

        let snapshot = handle_monitor_memory_save(
            &paths,
            &json!({
                "connectionSlug": "telegram-user",
                "content": "# edited\n"
            }),
        )
        .unwrap();

        let path = paths
            .workspace_config_dir
            .join("runtime")
            .join("monitors")
            .join("telegram-user.md");
        assert_eq!(fs::read_to_string(path).unwrap(), "# edited\n");
        assert_eq!(
            snapshot["monitor_memories"][0]["connection_slug"],
            "telegram-user"
        );
        assert_eq!(snapshot["monitor_memories"][0]["content"], "# edited\n");
    }

    #[test]
    fn monitor_memory_save_rejects_path_traversal_slug() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());

        let error = handle_monitor_memory_save(
            &paths,
            &json!({
                "connectionSlug": "../outside",
                "content": "bad"
            }),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("invalid monitor memory connection slug"));
        assert!(!tempdir.path().join("outside.md").exists());
    }
}
