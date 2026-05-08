use crate::workspace_paths;
use anyhow::{anyhow, bail, Context, Result};
use glob::{MatchOptions, Pattern};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_GLOB_LIMIT: usize = 100;

/// Linux pseudo-filesystem roots that the Glob walker must skip even
/// when an agent passes a broad `path` like `/`. Without these, a
/// `Glob("**/*.c", "/")` request recurses into `/proc/<pid>/...`,
/// `/sys/...`, etc. — millions of synthetic entries that produce no
/// matches but burn wall-clock and pollute tool output. Observed in
/// 2026-04-12 trajectories on `make-doom-for-mips` (the related grep
/// invocation with `path: "/"` produced 1.8MB of stderr on
/// `/proc/sys/kernel/...` — see `runtime/claude_tools/grep.rs` for
/// the matching guard there).
///
/// Match is **whole-path equality** (not basename), so a normal
/// project directory accidentally named `proc` is unaffected — only
/// the OS-level pseudo-FS roots get skipped.
const PSEUDO_FS_ROOTS: &[&str] = &["/proc", "/sys", "/dev", "/run"];

fn is_pseudo_fs_root(path: &Path) -> bool {
    PSEUDO_FS_ROOTS.iter().any(|root| path == Path::new(root))
}

/// Match options aligned with shell / gitignore glob semantics:
/// `*` does NOT match the path separator `/`. This matches Claude Code's
/// behavior (which delegates to `ripgrep --glob`) so that `Glob("*", "/app")`
/// returns only direct children of `/app`, not every file under every subdir.
const MATCH_OPTIONS: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
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
    let hint = if truncated {
        Some(format!(
            "Results are truncated to {DEFAULT_GLOB_LIMIT} files. \
             Use a more specific pattern (e.g. `**/*.rs`) or narrow `path` to reduce the result set."
        ))
    } else {
        None
    };
    let output = ClaudeGlobOutput {
        duration_ms: started.elapsed().as_millis(),
        num_files: filenames.len(),
        filenames,
        truncated,
        hint,
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
            // Skip Linux pseudo-FS roots when traversing — see
            // `PSEUDO_FS_ROOTS` doc comment for the trajectory
            // anchor and the grep.rs counterpart.
            if is_pseudo_fs_root(&path) {
                continue;
            }
            collect_glob_matches(workspace_root, &path, pattern, matches)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        let relative = path.strip_prefix(workspace_root).unwrap_or(&path);
        let relative_text = relative.to_string_lossy().replace('\\', "/");
        if pattern.matches_with(&relative_text, MATCH_OPTIONS) {
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
    fn pseudo_fs_root_check_recognizes_each_root() {
        for root in PSEUDO_FS_ROOTS {
            assert!(
                is_pseudo_fs_root(Path::new(root)),
                "{root} should be flagged as pseudo-FS"
            );
        }
        // Whole-path equality only: a project subdir named `proc`
        // should NOT trigger the skip.
        assert!(!is_pseudo_fs_root(Path::new("/home/user/project/proc")));
        assert!(!is_pseudo_fs_root(Path::new("/proc/cmdline"))); // descendant
        assert!(!is_pseudo_fs_root(Path::new("/app")));
    }

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

    /// Regression: `Glob("*", root)` must only return files directly under
    /// `root`, NOT recurse into subdirectories. Previously the underlying
    /// `glob::Pattern::matches` default allowed `*` to cross `/`, which meant
    /// a single `*` returned files like `sub/deep/file.txt` and could push
    /// the intended root-level target beyond the 100-file truncation limit.
    ///
    /// Reproduces the crack-7z-hash benchmark failure where 100+ john/**
    /// build artifacts hid /app/secrets.7z from the agent.
    #[test]
    fn glob_star_does_not_cross_path_separator() {
        let temp = tempfile::tempdir().unwrap();
        // Target file directly under the search root.
        fs::write(temp.path().join("secrets.7z"), "x").unwrap();
        // A subdir full of files whose mtime is *newer* (so they would sort
        // ahead of secrets.7z and push it past the 100-file limit if `*`
        // recursed into them).
        fs::create_dir_all(temp.path().join("john/build")).unwrap();
        for i in 0..120 {
            fs::write(temp.path().join(format!("john/build/f{i}.o")), "x").unwrap();
        }

        let output = execute_claude_glob(
            temp.path(),
            &[],
            false,
            json!({
                "pattern": "*",
                "path": temp.path().display().to_string(),
            }),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        let filenames = parsed["filenames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>();

        assert!(
            filenames.iter().any(|f| f == "secrets.7z"),
            "direct-child target must appear; got {filenames:?}",
        );
        assert!(
            filenames.iter().all(|f| !f.contains('/')),
            "`*` must not match nested paths; got {filenames:?}",
        );
    }

    /// Regression companion: `**/*.o` SHOULD recurse and find nested files,
    /// proving the fix only restricts `*` (not `**`).
    #[test]
    fn glob_double_star_still_recurses() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("a/b/c")).unwrap();
        fs::write(temp.path().join("top.o"), "x").unwrap();
        fs::write(temp.path().join("a/mid.o"), "x").unwrap();
        fs::write(temp.path().join("a/b/c/deep.o"), "x").unwrap();

        let output =
            execute_claude_glob(temp.path(), &[], false, json!({ "pattern": "**/*.o" })).unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        let filenames = parsed["filenames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<_>>();

        assert!(filenames.iter().any(|f| f == "top.o"));
        assert!(filenames.iter().any(|f| f == "a/mid.o"));
        assert!(filenames.iter().any(|f| f == "a/b/c/deep.o"));
    }

    /// Truncation surfaces a textual hint so the agent can narrow, mirroring
    /// Claude Code's "(Results are truncated. Consider using a more specific
    /// path or pattern.)" block. Agent sees it in the structured output's
    /// `hint` field.
    #[test]
    fn glob_truncation_includes_hint() {
        let temp = tempfile::tempdir().unwrap();
        for i in 0..(DEFAULT_GLOB_LIMIT + 20) {
            fs::write(temp.path().join(format!("f{i}.txt")), "x").unwrap();
        }

        let output =
            execute_claude_glob(temp.path(), &[], false, json!({ "pattern": "*.txt" })).unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["truncated"], true);
        let hint = parsed["hint"]
            .as_str()
            .expect("hint should be set on truncation");
        assert!(hint.contains("truncated"));
        assert!(hint.contains("specific"));
    }

    /// Ensures the non-truncated case does NOT include a spurious hint.
    #[test]
    fn glob_no_hint_when_not_truncated() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.txt"), "x").unwrap();

        let output =
            execute_claude_glob(temp.path(), &[], false, json!({ "pattern": "*.txt" })).unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["truncated"], false);
        assert!(
            parsed.get("hint").is_none() || parsed["hint"].is_null(),
            "hint should be omitted when results fit",
        );
    }
}
