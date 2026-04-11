use crate::workspace_paths;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct ClaudeEditInput {
    file_path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

/// Executes a Claude-style `Edit` operation and returns a JSON tool result payload.
pub fn execute_claude_edit(
    cwd: &Path,
    working_dirs: &[std::path::PathBuf],
    allow_all_paths: bool,
    input: Value,
) -> Result<String> {
    let input: ClaudeEditInput = serde_json::from_value(input).context("invalid Edit input")?;

    let raw_path = Path::new(&input.file_path);
    if !raw_path.is_absolute()
        && !input.file_path.trim().starts_with("~/")
        && input.file_path.trim() != "~"
    {
        bail!("Edit expects `file_path` to be an absolute path");
    }
    let sandbox_mode = if allow_all_paths {
        "danger-full-access"
    } else {
        "workspace-write"
    };
    let path = workspace_paths::resolve_path_for_session(
        cwd,
        working_dirs,
        sandbox_mode,
        Path::new(&input.file_path),
    )?;
    if input.old_string == input.new_string {
        bail!("No changes to make: old_string and new_string are exactly the same.");
    }

    let original = if path.exists() {
        fs::read_to_string(&path)
            .with_context(|| format!("failed to read file {}", path.display()))?
    } else {
        String::new()
    };
    let (updated, original_file) = if !path.exists() && input.old_string.is_empty() {
        (input.new_string.clone(), String::new())
    } else if input.old_string.is_empty() {
        if !original.is_empty() {
            bail!("Edit with empty old_string requires the target file to be empty or missing.");
        }
        (input.new_string.clone(), original.clone())
    } else {
        let occurrences = occurrence_count(&original, &input.old_string);
        if occurrences == 0 {
            bail!(
                "Edit failed: old_string was not found in {}",
                path.display()
            );
        }
        if occurrences > 1 && !input.replace_all {
            bail!(
                "Edit failed: old_string is not unique in {}. Use replace_all or provide more context.",
                path.display()
            );
        }
        let updated = if input.replace_all {
            original.replace(&input.old_string, &input.new_string)
        } else {
            original.replacen(&input.old_string, &input.new_string, 1)
        };
        (updated, original.clone())
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent.to_path_buf())
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }
    fs::write(&path, &updated)
        .with_context(|| format!("failed to write file {}", path.display()))?;

    let output = json!({
        "filePath": input.file_path,
        "oldString": input.old_string,
        "newString": input.new_string,
        "originalFile": original_file,
        "structuredPatch": build_structured_patch(&original, &updated),
        "userModified": false,
        "replaceAll": input.replace_all
    });
    Ok(serde_json::to_string_pretty(&output)?)
}

/// Returns true when an Edit request targets an existing file mutation that
/// should require a prior full-file Read in the runtime dispatcher.
pub(crate) fn requires_prior_read(input: &Value) -> bool {
    let Some(file_path) = input.get("file_path").and_then(Value::as_str) else {
        return true;
    };
    let old_string = input
        .get("old_string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    !(old_string.is_empty() && !Path::new(file_path).exists())
}

fn occurrence_count(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

fn build_structured_patch(original: &str, updated: &str) -> Vec<Value> {
    let old_lines = split_lines(original);
    let new_lines = split_lines(updated);
    vec![json!({
        "oldStart": if old_lines.is_empty() { 0 } else { 1 },
        "oldLines": old_lines.len(),
        "newStart": if new_lines.is_empty() { 0 } else { 1 },
        "newLines": new_lines.len(),
        "lines": old_lines
            .iter()
            .map(|line| format!("-{line}"))
            .chain(new_lines.iter().map(|line| format!("+{line}")))
            .collect::<Vec<_>>(),
    })]
}

fn split_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        Vec::new()
    } else {
        content.lines().map(str::to_string).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edit_replaces_unique_occurrence() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sample.txt");
        fs::write(&file, "alpha\nbeta\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "beta",
            "new_string": "gamma"
        });

        let output = execute_claude_edit(temp.path(), &[], false, input).unwrap();
        assert!(output.contains("\"replaceAll\": false"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
    }

    #[test]
    fn edit_replace_all_updates_every_occurrence() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sample.txt");
        fs::write(&file, "a\nx\nx\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "x",
            "new_string": "y",
            "replace_all": true
        });

        let _ = execute_claude_edit(temp.path(), &[], false, input).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "a\ny\ny\n");
    }

    #[test]
    fn edit_rejects_non_unique_without_replace_all() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sample.txt");
        fs::write(&file, "x\nx\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "x",
            "new_string": "y"
        });

        let error = execute_claude_edit(temp.path(), &[], false, input).unwrap_err();
        assert!(error.to_string().contains("not unique"));
    }

    #[test]
    fn edit_rejects_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let input = json!({
            "file_path": "relative.txt",
            "old_string": "x",
            "new_string": "y"
        });

        let error = execute_claude_edit(temp.path(), &[], false, input).unwrap_err();
        assert!(error.to_string().contains("absolute path"));
    }

    #[test]
    fn edit_can_create_missing_file_with_empty_old_string() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("new.txt");
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "",
            "new_string": "hello"
        });

        let output = execute_claude_edit(temp.path(), &[], false, input).unwrap();
        assert!(output.contains("\"originalFile\": \"\""));
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
    }

    #[test]
    fn missing_file_creation_edit_does_not_require_prior_read() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("new.txt");
        assert!(!requires_prior_read(&json!({
            "file_path": file.display().to_string(),
            "old_string": "",
            "new_string": "hello"
        })));
    }

    #[test]
    fn edit_rejects_paths_outside_working_directories() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("sample.txt");
        fs::write(&file, "alpha\nbeta\n").unwrap();
        let input = json!({
            "file_path": file.display().to_string(),
            "old_string": "beta",
            "new_string": "gamma"
        });

        let error = execute_claude_edit(temp.path(), &[], false, input).unwrap_err();
        assert!(error
            .to_string()
            .contains("outside the current working directories"));
    }
}
