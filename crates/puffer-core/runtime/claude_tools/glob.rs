use crate::workspace_paths;
use anyhow::{anyhow, bail, Context, Result};
use glob::Pattern;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_GLOB_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
struct ClaudeGlobInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct ClaudeGlobOutput {
    #[serde(rename = "durationMs")]
    duration_ms: u128,
    #[serde(rename = "numFiles")]
    num_files: usize,
    filenames: Vec<String>,
    truncated: bool,
}

/// Executes the Claude-compatible `Glob` tool over the current workspace.
///
/// The input shape matches Claude Code:
/// - `pattern` (required): glob pattern to match
/// - `path` (optional): directory to scope the search
///
/// Output matches Claude Code's shape:
/// - `durationMs`, `numFiles`, `filenames`, `truncated`
pub fn execute_claude_glob(
    cwd: &Path,
    working_dirs: &[PathBuf],
    allow_all_paths: bool,
    input: Value,
) -> Result<String> {
    let started = Instant::now();
    let input: ClaudeGlobInput = serde_json::from_value(input).context("invalid Glob input")?;
    if input.pattern.trim().is_empty() {
        bail!("Glob pattern cannot be empty");
    }

    let pattern = Pattern::new(&input.pattern)
        .map_err(|error| anyhow!("invalid glob pattern `{}`: {error}", input.pattern))?;
    let sandbox_mode = if allow_all_paths {
        "danger-full-access"
    } else {
        "workspace-write"
    };
    let root = input
        .path
        .as_deref()
        .map(|path| {
            workspace_paths::resolve_path_for_session(
                cwd,
                working_dirs,
                sandbox_mode,
                Path::new(path),
            )
        })
        .transpose()?
        .unwrap_or_else(|| cwd.to_path_buf());
    if !root.exists() {
        bail!("Directory does not exist: {}", root.display());
    }
    if !root.is_dir() {
        bail!("Path is not a directory: {}", root.display());
    }

    let mut matches = Vec::new();
    collect_glob_matches(&root, &root, &pattern, &mut matches)?;
    matches.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    let truncated = matches.len() > DEFAULT_GLOB_LIMIT;
    let filenames = matches
        .into_iter()
        .take(DEFAULT_GLOB_LIMIT)
        .map(|(path, _)| path)
        .collect::<Vec<_>>();
    let output = ClaudeGlobOutput {
        duration_ms: started.elapsed().as_millis(),
        num_files: filenames.len(),
        filenames,
        truncated,
    };
    Ok(serde_json::to_string_pretty(&output)?)
}

fn collect_glob_matches(
    workspace_root: &Path,
    current: &Path,
    pattern: &Pattern,
    matches: &mut Vec<(String, u128)>,
) -> Result<()> {
    for entry in fs::read_dir(current)
        .with_context(|| format!("failed to list directory {}", current.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_glob_matches(workspace_root, &path, pattern, matches)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        let relative = path.strip_prefix(workspace_root).unwrap_or(&path);
        let relative_text = relative.to_string_lossy().replace('\\', "/");
        if pattern.matches(&relative_text) {
            matches.push((relative_text, file_mtime_ms(&path)));
        }
    }
    Ok(())
}

fn file_mtime_ms(path: &Path) -> u128 {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(system_time_to_epoch_ms)
        .unwrap_or(0)
}

fn system_time_to_epoch_ms(time: SystemTime) -> Option<u128> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn glob_returns_expected_shape() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(temp.path().join("src/lib.rs"), "pub fn x() {}\n").unwrap();

        let output = execute_claude_glob(
            temp.path(),
            &[],
            false,
            json!({
                "pattern": "src/*.rs"
            }),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["truncated"], false);
        assert_eq!(parsed["numFiles"], 2);
        assert_eq!(parsed["filenames"][0], "src/lib.rs");
        assert_eq!(parsed["filenames"][1], "src/main.rs");
    }

    #[test]
    fn glob_sorts_by_mtime_descending() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("older.txt"), "1").unwrap();
        std::thread::sleep(Duration::from_millis(5));
        fs::write(temp.path().join("newer.txt"), "2").unwrap();

        let output = execute_claude_glob(
            temp.path(),
            &[],
            false,
            json!({
                "pattern": "*.txt"
            }),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["filenames"][0], "newer.txt");
        assert_eq!(parsed["filenames"][1], "older.txt");
    }

    #[test]
    fn glob_rejects_paths_outside_working_directories() {
        let temp = tempfile::tempdir().unwrap();
        let error = execute_claude_glob(
            temp.path(),
            &[],
            false,
            json!({
                "pattern": "*.rs",
                "path": "../"
            }),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("outside the current working directories"));
    }

    #[test]
    fn glob_searches_added_working_directories_relative_to_selected_root() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("repo");
        let extra = temp.path().join("extra");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(extra.join("src")).unwrap();
        fs::write(extra.join("src/lib.rs"), "pub fn extra() {}\n").unwrap();

        let output = execute_claude_glob(
            &cwd,
            &[extra.clone()],
            false,
            json!({
                "pattern": "src/*.rs",
                "path": extra.display().to_string()
            }),
        )
        .unwrap();

        let parsed: Value = serde_json::from_str(&output).unwrap();
        let filenames = parsed["filenames"].as_array().unwrap();
        assert_eq!(filenames.len(), 1);
        assert_eq!(filenames[0], json!("src/lib.rs"));
    }
}
