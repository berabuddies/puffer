use crate::workspace_paths;
use anyhow::{bail, Context, Result};
use puffer_runner_api::FilesystemExecutionPolicy;
use puffer_runner_api::StalenessRejection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const NOT_READ_ERROR: &str = "File has not been read yet. Read it first before writing to it.";
const STALE_READ_ERROR: &str = "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it.";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ClaudeWriteInput {
    file_path: String,
    content: String,
}

/// Captures the latest read-state metadata used by Claude-style stale-write checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeReadSnapshot {
    pub timestamp_ms: u128,
    pub is_partial_view: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct StructuredPatchHunk {
    #[serde(rename = "oldStart")]
    old_start: usize,
    #[serde(rename = "oldLines")]
    old_lines: usize,
    #[serde(rename = "newStart")]
    new_start: usize,
    #[serde(rename = "newLines")]
    new_lines: usize,
    lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ClaudeWriteOutput {
    #[serde(rename = "type")]
    write_type: String,
    #[serde(rename = "filePath")]
    file_path: String,
    content: String,
    #[serde(rename = "structuredPatch")]
    structured_patch: Vec<StructuredPatchHunk>,
    #[serde(rename = "originalFile")]
    original_file: Option<String>,
    #[serde(rename = "gitDiff")]
    git_diff: Option<Value>,
}

/// Executes a Claude-compatible `Write` tool call and returns a JSON result payload.
pub fn execute_claude_write_tool(
    cwd: &Path,
    working_dirs: &[PathBuf],
    filesystem: &FilesystemExecutionPolicy,
    input: Value,
    read_file_state: &mut HashMap<PathBuf, ClaudeReadSnapshot>,
) -> Result<String> {
    let input: ClaudeWriteInput =
        serde_json::from_value(input).context("invalid Write tool input")?;
    let raw_path = PathBuf::from(&input.file_path);
    if !raw_path.is_absolute()
        && !input.file_path.trim().starts_with("~/")
        && input.file_path.trim() != "~"
    {
        bail!("file_path must be an absolute path");
    }
    let path = workspace_paths::resolve_path_for_filesystem_policy(
        cwd,
        working_dirs,
        filesystem.sandbox_mode,
        Path::new(&input.file_path),
    )?;
    if path.is_dir() {
        bail!("file_path points to a directory");
    }

    let existed = path.exists();
    let original_file = if existed {
        Some(
            fs::read_to_string(&path)
                .with_context(|| format!("failed to read file {}", path.display()))?,
        )
    } else {
        None
    };
    if existed {
        validate_existing_write(&path, read_file_state)?;
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent.to_path_buf())
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }
    fs::write(&path, &input.content)
        .with_context(|| format!("failed to write file {}", path.display()))?;

    let timestamp_ms = file_timestamp_ms(&path)?;
    read_file_state.insert(
        path.clone(),
        ClaudeReadSnapshot {
            timestamp_ms,
            is_partial_view: false,
        },
    );

    let output = ClaudeWriteOutput {
        write_type: if original_file.is_some() {
            "update".to_string()
        } else {
            "create".to_string()
        },
        file_path: path.display().to_string(),
        content: input.content,
        structured_patch: if original_file.is_some() {
            build_structured_patch(original_file.as_deref(), &path)?
        } else {
            Vec::new()
        },
        original_file: original_file.clone(),
        git_diff: build_git_diff(original_file.as_deref(), &path)?,
    };
    serde_json::to_string_pretty(&output).context("failed to serialize Write output")
}

fn validate_existing_write(
    path: &Path,
    read_file_state: &HashMap<PathBuf, ClaudeReadSnapshot>,
) -> Result<()> {
    let Some(snapshot) = read_file_state.get(path) else {
        bail!(NOT_READ_ERROR);
    };
    if snapshot.is_partial_view {
        // defense-in-depth: pre-flight gate at mod.rs:402 catches first; keep messages aligned with StalenessRejection
        bail!(StalenessRejection::PARTIAL_READ_MESSAGE);
    }
    let last_write_time = file_timestamp_ms(path)?;
    if last_write_time > snapshot.timestamp_ms {
        bail!(STALE_READ_ERROR);
    }
    Ok(())
}

fn build_structured_patch(
    original_file: Option<&str>,
    path: &Path,
) -> Result<Vec<StructuredPatchHunk>> {
    let updated_file = fs::read_to_string(path)
        .with_context(|| format!("failed to read updated file {}", path.display()))?;
    if original_file.is_some_and(|original| original == updated_file) {
        return Ok(Vec::new());
    }

    let old_lines = split_lines(original_file.unwrap_or(""));
    let new_lines = split_lines(&updated_file);
    if old_lines.is_empty() && new_lines.is_empty() {
        return Ok(Vec::new());
    }

    let mut lines = Vec::with_capacity(old_lines.len() + new_lines.len());
    lines.extend(old_lines.iter().map(|line| format!("-{line}")));
    lines.extend(new_lines.iter().map(|line| format!("+{line}")));

    Ok(vec![StructuredPatchHunk {
        old_start: if old_lines.is_empty() { 0 } else { 1 },
        old_lines: old_lines.len(),
        new_start: if new_lines.is_empty() { 0 } else { 1 },
        new_lines: new_lines.len(),
        lines,
    }])
}

fn build_git_diff(original_file: Option<&str>, path: &Path) -> Result<Option<Value>> {
    let Some(original_file) = original_file else {
        return Ok(None);
    };
    let updated_file = fs::read_to_string(path)
        .with_context(|| format!("failed to read updated file {}", path.display()))?;
    if original_file == updated_file {
        return Ok(None);
    }

    let mut diff_lines = Vec::new();
    diff_lines.push(format!("--- {}", path.display()));
    diff_lines.push(format!("+++ {}", path.display()));
    diff_lines.extend(
        split_lines(original_file)
            .into_iter()
            .map(|line| format!("-{line}")),
    );
    diff_lines.extend(
        split_lines(&updated_file)
            .into_iter()
            .map(|line| format!("+{line}")),
    );
    Ok(Some(Value::String(diff_lines.join("\n"))))
}

fn split_lines(contents: &str) -> Vec<String> {
    if contents.is_empty() {
        return Vec::new();
    }
    contents.lines().map(str::to_string).collect()
}

fn file_timestamp_ms(path: &Path) -> Result<u128> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat file {}", path.display()))?;
    let modified = metadata
        .modified()
        .with_context(|| format!("failed to read mtime for {}", path.display()))?;
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .with_context(|| format!("mtime for {} predates UNIX_EPOCH", path.display()))?;
    Ok(duration.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_runner_api::{FilesystemExecutionPolicy, FilesystemSandboxMode};
    use serde_json::json;

    fn workspace_write_policy() -> FilesystemExecutionPolicy {
        FilesystemExecutionPolicy {
            sandbox_mode: FilesystemSandboxMode::WorkspaceWrite,
        }
    }

    fn danger_full_access_policy() -> FilesystemExecutionPolicy {
        FilesystemExecutionPolicy {
            sandbox_mode: FilesystemSandboxMode::DangerFullAccess,
        }
    }

    fn write_input(path: &Path, content: &str) -> Value {
        json!({
            "file_path": path.display().to_string(),
            "content": content,
        })
    }

    #[test]
    fn write_creates_new_file_without_prior_read() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("note.txt");
        let mut read_state = HashMap::new();

        let output = execute_claude_write_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            write_input(&path, "hello"),
            &mut read_state,
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["type"], "create");
        assert_eq!(parsed["filePath"], path.display().to_string());
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
        assert!(read_state
            .get(&path)
            .is_some_and(|snapshot| !snapshot.is_partial_view));
    }

    #[test]
    fn write_requires_prior_read_for_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("note.txt");
        fs::write(&path, "old").unwrap();
        let mut read_state = HashMap::new();

        let error = execute_claude_write_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            write_input(&path, "new"),
            &mut read_state,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(NOT_READ_ERROR));
    }

    #[test]
    fn write_rejects_partial_reads_for_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("note.txt");
        fs::write(&path, "old").unwrap();
        let mut read_state = HashMap::new();
        read_state.insert(
            path.clone(),
            ClaudeReadSnapshot {
                timestamp_ms: file_timestamp_ms(&path).unwrap(),
                is_partial_view: true,
            },
        );

        let error = execute_claude_write_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            write_input(&path, "new"),
            &mut read_state,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(StalenessRejection::PARTIAL_READ_MESSAGE));
    }

    #[test]
    fn write_rejects_stale_read_state_for_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("note.txt");
        fs::write(&path, "old").unwrap();
        let mut read_state = HashMap::new();
        read_state.insert(
            path.clone(),
            ClaudeReadSnapshot {
                timestamp_ms: 0,
                is_partial_view: false,
            },
        );

        let error = execute_claude_write_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            write_input(&path, "new"),
            &mut read_state,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(STALE_READ_ERROR));
    }

    #[test]
    fn write_updates_existing_file_with_fresh_read_state() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("note.txt");
        fs::write(&path, "old").unwrap();
        let mut read_state = HashMap::new();
        read_state.insert(
            path.clone(),
            ClaudeReadSnapshot {
                timestamp_ms: file_timestamp_ms(&path).unwrap(),
                is_partial_view: false,
            },
        );

        let output = execute_claude_write_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            write_input(&path, "new"),
            &mut read_state,
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["type"], "update");
        assert_eq!(parsed["originalFile"], "old");
        assert!(parsed["structuredPatch"]
            .as_array()
            .is_some_and(|hunks| !hunks.is_empty()));
        assert!(parsed["gitDiff"].as_str().is_some());
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert!(read_state
            .get(&path)
            .is_some_and(|snapshot| snapshot.timestamp_ms >= 1 && !snapshot.is_partial_view));
    }

    #[test]
    fn write_create_uses_empty_structured_patch() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fresh.txt");
        let mut read_state = HashMap::new();

        let output = execute_claude_write_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            write_input(&path, "hello"),
            &mut read_state,
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["type"], "create");
        assert_eq!(parsed["structuredPatch"], json!([]));
        assert!(parsed["gitDiff"].is_null());
    }

    #[test]
    fn write_rejects_paths_outside_default_writable_roots() {
        // Path outside cwd, /tmp, $TMPDIR, and any /add-dir root.
        // Codex-style default writable set: [cwd, /tmp, $TMPDIR, ...add-dir].
        let temp = tempfile::tempdir().unwrap();
        let path = std::path::PathBuf::from("/__puffer_test_outside_writable_set__/fresh.txt");
        let mut read_state = HashMap::new();

        let error = execute_claude_write_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            write_input(&path, "hello"),
            &mut read_state,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("outside the current working directories"));
    }

    #[test]
    fn write_allows_paths_under_slash_tmp_under_workspace_write() {
        // Codex-style default writable set includes /tmp on Unix, so the
        // model writing to /tmp/<test>.txt should not bounce off the path
        // gate the way it did pre-this-change.
        if !cfg!(unix) || !std::path::Path::new("/tmp").is_dir() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let uniq = format!(
            "puffer_test_{}_{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::path::PathBuf::from("/tmp").join(&uniq);
        let mut read_state = HashMap::new();

        let result = execute_claude_write_tool(
            temp.path(),
            &[],
            &workspace_write_policy(),
            write_input(&path, "hello"),
            &mut read_state,
        );
        let _ = std::fs::remove_file(&path);
        assert!(
            result.is_ok(),
            "write to /tmp must succeed under workspace-write; got: {result:?}"
        );
    }

    #[test]
    fn write_allows_paths_outside_working_directories_in_danger_full_access() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = outside.path().join("fresh.txt");
        let mut read_state = HashMap::new();

        let output = execute_claude_write_tool(
            temp.path(),
            &[],
            &danger_full_access_policy(),
            write_input(&path, "hello"),
            &mut read_state,
        )
        .unwrap();

        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["type"], "create");
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }
}
