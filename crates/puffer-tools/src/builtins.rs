use crate::model::TypedToolInput;
use crate::{
    BashToolInput, ListDirToolInput, MovePathToolInput, ReadFileToolInput, RemovePathToolInput,
    ReplaceInFileToolInput, SearchTextToolInput, ToolExecutionResult, ToolInput, ToolKind,
    ToolOutput, WriteFileToolInput,
};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Returns the user's default shell if it supports `-lc`, otherwise falls back to `bash`.
pub fn detected_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .filter(|s| {
            let base = s.rsplit('/').next().unwrap_or(s);
            matches!(base, "bash" | "zsh" | "sh" | "dash" | "ksh")
        })
        .unwrap_or_else(|| "bash".to_string())
}

/// Returns the built-in tool kind for a declarative handler id.
pub fn builtin_tool_kind(handler: &str) -> Option<ToolKind> {
    match handler {
        "bash" => Some(ToolKind::Bash),
        "read_file" => Some(ToolKind::ReadFile),
        "write_file" => Some(ToolKind::WriteFile),
        "replace_in_file" => Some(ToolKind::ReplaceInFile),
        "move_path" => Some(ToolKind::MovePath),
        "remove_path" => Some(ToolKind::RemovePath),
        "list_dir" => Some(ToolKind::ListDir),
        "search_text" => Some(ToolKind::SearchText),
        _ => None,
    }
}

/// Parses raw JSON input into the typed payload expected by one built-in tool.
pub fn parse_builtin_input(kind: ToolKind, input: Value) -> Result<ToolInput> {
    match kind {
        ToolKind::Custom => Err(anyhow!("custom tools do not use built-in typed inputs")),
        ToolKind::Bash => {
            let input = serde_json::from_value::<BashToolInput>(input)?;
            Ok(ToolInput::Bash {
                command: input.command,
                timeout: input.timeout,
                run_in_background: input.run_in_background,
            })
        }
        ToolKind::ReadFile => {
            let input = serde_json::from_value::<ReadFileToolInput>(input)?;
            Ok(ToolInput::ReadFile {
                path: input.path,
                offset: input.offset,
                limit: input.limit,
            })
        }
        ToolKind::WriteFile => {
            let input = serde_json::from_value::<WriteFileToolInput>(input)?;
            Ok(ToolInput::WriteFile {
                path: input.path,
                contents: input.contents,
            })
        }
        ToolKind::ReplaceInFile => {
            let input = serde_json::from_value::<ReplaceInFileToolInput>(input)?;
            Ok(ToolInput::ReplaceInFile {
                path: input.path,
                old: input.old,
                new: input.new,
                replace_all: input.replace_all,
            })
        }
        ToolKind::MovePath => {
            let input = serde_json::from_value::<MovePathToolInput>(input)?;
            Ok(ToolInput::MovePath {
                from: input.from,
                to: input.to,
            })
        }
        ToolKind::RemovePath => {
            let input = serde_json::from_value::<RemovePathToolInput>(input)?;
            Ok(ToolInput::RemovePath {
                path: input.path,
                recursive: input.recursive,
            })
        }
        ToolKind::ListDir => {
            let input = serde_json::from_value::<ListDirToolInput>(input)?;
            Ok(ToolInput::ListDir { path: input.path })
        }
        ToolKind::SearchText => {
            let input = serde_json::from_value::<SearchTextToolInput>(input)?;
            Ok(ToolInput::SearchText {
                query: input.query,
                path: input.path,
            })
        }
    }
}

/// Executes one built-in tool with typed input under the given working directory.
pub fn execute_builtin_tool(
    tool_id: &str,
    kind: ToolKind,
    cwd: &Path,
    input: ToolInput,
) -> Result<ToolExecutionResult> {
    let (actual_kind, payload) = input.into_kind_payload();
    if actual_kind != kind {
        return Err(anyhow!(
            "tool input mismatch for {tool_id}: expected {:?}, got {:?}",
            kind,
            actual_kind
        ));
    }

    match payload {
        TypedToolInput::Bash(input) => execute_bash_tool(tool_id, cwd, input),
        TypedToolInput::ReadFile(input) => execute_read_file_tool(tool_id, cwd, input),
        TypedToolInput::WriteFile(input) => execute_write_file_tool(tool_id, cwd, input),
        TypedToolInput::ReplaceInFile(input) => execute_replace_in_file_tool(tool_id, cwd, input),
        TypedToolInput::MovePath(input) => execute_move_path_tool(tool_id, cwd, input),
        TypedToolInput::RemovePath(input) => execute_remove_path_tool(tool_id, cwd, input),
        TypedToolInput::ListDir(input) => execute_list_dir_tool(tool_id, cwd, input),
        TypedToolInput::SearchText(input) => execute_search_text_tool(tool_id, cwd, input),
    }
}

/// Executes the built-in `bash` tool.
pub fn execute_bash_tool(
    tool_id: &str,
    cwd: &Path,
    input: BashToolInput,
) -> Result<ToolExecutionResult> {
    if input.run_in_background {
        return Err(anyhow!(
            "background bash execution is not implemented in this runtime"
        ));
    }
    let execution = run_bash_command(cwd, &input.command, input.timeout)?;
    let output = execution.output;
    Ok(ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: output.status.success() && !execution.timed_out,
        output: ToolOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: bash_stderr_message(&output.stderr, execution.timed_out, input.timeout),
            metadata: serde_json::json!({
                "status_code": output.status.code(),
                "command": input.command,
                "timeout_ms": input.timeout,
                "run_in_background": input.run_in_background,
                "timed_out": execution.timed_out,
            }),
        },
    })
}

/// Executes the built-in `read_file` tool.
pub fn execute_read_file_tool(
    tool_id: &str,
    cwd: &Path,
    input: ReadFileToolInput,
) -> Result<ToolExecutionResult> {
    let path = resolve_workspace_path(cwd, &input.path)?;
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read file {}", path.display()))?;
    let selected = select_file_lines(&contents, input.offset, input.limit);
    Ok(ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: true,
        output: ToolOutput {
            stdout: selected,
            stderr: String::new(),
            metadata: serde_json::json!({
                "path": path,
                "offset": input.offset,
                "limit": input.limit,
            }),
        },
    })
}

/// Executes the built-in `write_file` tool.
pub fn execute_write_file_tool(
    tool_id: &str,
    cwd: &Path,
    input: WriteFileToolInput,
) -> Result<ToolExecutionResult> {
    let path = resolve_workspace_path(cwd, &input.path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dir {}", parent.display()))?;
    }
    fs::write(&path, &input.contents)
        .with_context(|| format!("failed to write file {}", path.display()))?;
    Ok(ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: true,
        output: ToolOutput {
            stdout: format!("wrote {}", path.display()),
            stderr: String::new(),
            metadata: serde_json::json!({
                "path": path,
                "bytes_written": input.contents.len(),
            }),
        },
    })
}

/// Executes the built-in `replace_in_file` tool.
pub fn execute_replace_in_file_tool(
    tool_id: &str,
    cwd: &Path,
    input: ReplaceInFileToolInput,
) -> Result<ToolExecutionResult> {
    let path = resolve_workspace_path(cwd, &input.path)?;
    let original = fs::read_to_string(&path)
        .with_context(|| format!("failed to read file {}", path.display()))?;
    if !original.contains(&input.old) {
        return Err(anyhow!(
            "failed to replace text in {}: pattern not found",
            path.display()
        ));
    }
    let updated = if input.replace_all {
        original.replace(&input.old, &input.new)
    } else {
        original.replacen(&input.old, &input.new, 1)
    };
    fs::write(&path, &updated)
        .with_context(|| format!("failed to write file {}", path.display()))?;
    Ok(ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: true,
        output: ToolOutput {
            stdout: format!("updated {}", path.display()),
            stderr: String::new(),
            metadata: serde_json::json!({
                "path": path,
                "replace_all": input.replace_all,
            }),
        },
    })
}

/// Executes the built-in `move_path` tool.
pub fn execute_move_path_tool(
    tool_id: &str,
    cwd: &Path,
    input: MovePathToolInput,
) -> Result<ToolExecutionResult> {
    let from = resolve_workspace_path(cwd, &input.from)?;
    let to = resolve_workspace_path(cwd, &input.to)?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dir {}", parent.display()))?;
    }
    fs::rename(&from, &to)
        .with_context(|| format!("failed to move {} to {}", from.display(), to.display()))?;
    Ok(ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: true,
        output: ToolOutput {
            stdout: format!("moved {} -> {}", from.display(), to.display()),
            stderr: String::new(),
            metadata: serde_json::json!({
                "from": from,
                "to": to,
            }),
        },
    })
}

/// Executes the built-in `remove_path` tool.
pub fn execute_remove_path_tool(
    tool_id: &str,
    cwd: &Path,
    input: RemovePathToolInput,
) -> Result<ToolExecutionResult> {
    let path = resolve_workspace_path(cwd, &input.path)?;
    if path.is_dir() {
        if input.recursive {
            fs::remove_dir_all(&path)
                .with_context(|| format!("failed to remove directory {}", path.display()))?;
        } else {
            fs::remove_dir(&path)
                .with_context(|| format!("failed to remove directory {}", path.display()))?;
        }
    } else {
        fs::remove_file(&path)
            .with_context(|| format!("failed to remove file {}", path.display()))?;
    }
    Ok(ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: true,
        output: ToolOutput {
            stdout: format!("removed {}", path.display()),
            stderr: String::new(),
            metadata: serde_json::json!({
                "path": path,
                "recursive": input.recursive,
            }),
        },
    })
}

/// Executes the built-in `list_dir` tool.
pub fn execute_list_dir_tool(
    tool_id: &str,
    cwd: &Path,
    input: ListDirToolInput,
) -> Result<ToolExecutionResult> {
    let path = input
        .path
        .as_deref()
        .map(|path| resolve_workspace_path(cwd, path))
        .transpose()?
        .unwrap_or_else(|| cwd.to_path_buf());
    let mut entries = fs::read_dir(&path)
        .with_context(|| format!("failed to list directory {}", path.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let stdout = entries
        .into_iter()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let suffix = if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                "/"
            } else {
                ""
            };
            format!("{name}{suffix}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: true,
        output: ToolOutput {
            stdout,
            stderr: String::new(),
            metadata: serde_json::json!({
                "path": path,
            }),
        },
    })
}

/// Executes the built-in `search_text` tool.
pub fn execute_search_text_tool(
    tool_id: &str,
    cwd: &Path,
    input: SearchTextToolInput,
) -> Result<ToolExecutionResult> {
    let target = input
        .path
        .as_deref()
        .map(|path| resolve_workspace_path(cwd, path))
        .transpose()?
        .unwrap_or_else(|| cwd.to_path_buf());
    let result = if command_exists("rg") {
        run_search_command("rg", &build_rg_search_args(&input.query, &target))?
    } else {
        run_search_command("grep", &build_grep_search_args(&input.query, &target))?
    };
    Ok(ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: result.success,
        output: ToolOutput {
            stdout: result.stdout,
            stderr: result.stderr,
            metadata: serde_json::json!({
                "path": target,
                "query": input.query,
            }),
        },
    })
}

fn resolve_workspace_path(cwd: &Path, path: &Path) -> Result<PathBuf> {
    let workspace_root = fs::canonicalize(cwd)
        .with_context(|| format!("failed to resolve workspace root {}", cwd.display()))?;
    let workspace_path = normalize_path(cwd);
    let candidate = if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&cwd.join(path))
    };
    if !candidate.starts_with(&workspace_path) {
        return Err(anyhow!(
            "path {} escapes workspace {}",
            path.display(),
            cwd.display()
        ));
    }
    let ancestor = nearest_existing_ancestor(&candidate).ok_or_else(|| {
        anyhow!(
            "failed to resolve path {} inside workspace {}",
            path.display(),
            cwd.display()
        )
    })?;
    let canonical_ancestor = fs::canonicalize(&ancestor)
        .with_context(|| format!("failed to canonicalize {}", ancestor.display()))?;
    if !canonical_ancestor.starts_with(&workspace_root) {
        return Err(anyhow!(
            "path {} resolves through symlink outside workspace {}",
            path.display(),
            cwd.display()
        ));
    }
    if candidate.exists() {
        let canonical_candidate = fs::canonicalize(&candidate)
            .with_context(|| format!("failed to canonicalize {}", candidate.display()))?;
        if !canonical_candidate.starts_with(&workspace_root) {
            return Err(anyhow!(
                "path {} resolves outside workspace {}",
                path.display(),
                cwd.display()
            ));
        }
    }
    Ok(candidate)
}

fn command_exists(command: &str) -> bool {
    Command::new(command).arg("--version").output().is_ok()
}

fn build_rg_search_args(query: &str, target: &Path) -> Vec<String> {
    vec![
        "-n".to_string(),
        "--no-heading".to_string(),
        "-e".to_string(),
        query.to_string(),
        "--".to_string(),
        target.to_string_lossy().to_string(),
    ]
}

fn build_grep_search_args(query: &str, target: &Path) -> Vec<String> {
    vec![
        "-RIn".to_string(),
        "-e".to_string(),
        query.to_string(),
        "--".to_string(),
        target.to_string_lossy().to_string(),
    ]
}

fn run_search_command(command: &str, args: &[String]) -> Result<SearchOutput> {
    let output = Command::new(command)
        .args(args)
        .output()
        .with_context(|| format!("failed to execute {command}"))?;
    let status = output.status.code().unwrap_or_default();
    let success = output.status.success() || status == 1;
    Ok(SearchOutput {
        success,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

struct SearchOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

fn select_file_lines(contents: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    match (offset, limit) {
        (None, None) => contents.to_string(),
        _ => {
            let lines = contents.lines().collect::<Vec<_>>();
            let start = offset.unwrap_or(0).min(lines.len());
            let end = limit
                .map(|limit| start.saturating_add(limit).min(lines.len()))
                .unwrap_or(lines.len());
            let mut selected = lines[start..end].join("\n");
            if contents.ends_with('\n') && !selected.is_empty() {
                selected.push('\n');
            }
            selected
        }
    }
}

struct TimedCommandOutput {
    output: Output,
    timed_out: bool,
}

fn run_bash_command(
    cwd: &Path,
    command: &str,
    timeout_ms: Option<u64>,
) -> Result<TimedCommandOutput> {
    let mut child = Command::new(detected_shell())
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to execute bash tool in {}", cwd.display()))?;
    let Some(timeout_ms) = timeout_ms else {
        return Ok(TimedCommandOutput {
            output: child
                .wait_with_output()
                .with_context(|| format!("failed to wait for bash tool in {}", cwd.display()))?,
            timed_out: false,
        });
    };
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if child
            .try_wait()
            .with_context(|| format!("failed to poll bash tool in {}", cwd.display()))?
            .is_some()
        {
            return Ok(TimedCommandOutput {
                output: child.wait_with_output().with_context(|| {
                    format!("failed to collect bash tool output in {}", cwd.display())
                })?,
                timed_out: false,
            });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return Ok(TimedCommandOutput {
                output: child.wait_with_output().with_context(|| {
                    format!(
                        "failed to collect timed-out bash output in {}",
                        cwd.display()
                    )
                })?,
                timed_out: true,
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn bash_stderr_message(stderr: &[u8], timed_out: bool, timeout_ms: Option<u64>) -> String {
    let mut message = String::from_utf8_lossy(stderr).to_string();
    if timed_out {
        let timeout_note = format!(
            "command timed out after {}ms",
            timeout_ms.unwrap_or_default()
        );
        if message.trim().is_empty() {
            message = timeout_note;
        } else {
            message.push('\n');
            message.push_str(&timeout_note);
        }
    }
    message
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if current.exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_and_write_tools_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let path = std::path::PathBuf::from("note.txt");
        let write = execute_builtin_tool(
            "write_file",
            ToolKind::WriteFile,
            temp.path(),
            ToolInput::WriteFile {
                path: path.clone(),
                contents: "hello".to_string(),
            },
        )
        .unwrap();
        assert!(write.success);

        let read = execute_builtin_tool(
            "read_file",
            ToolKind::ReadFile,
            temp.path(),
            ToolInput::ReadFile {
                path,
                offset: None,
                limit: None,
            },
        )
        .unwrap();
        assert_eq!(read.output.stdout, "hello");
    }

    #[test]
    fn builtin_helpers_cover_registered_handlers() {
        assert_eq!(builtin_tool_kind("bash"), Some(ToolKind::Bash));
        assert_eq!(builtin_tool_kind("read_file"), Some(ToolKind::ReadFile));
        assert_eq!(builtin_tool_kind("write_file"), Some(ToolKind::WriteFile));
        assert_eq!(
            builtin_tool_kind("replace_in_file"),
            Some(ToolKind::ReplaceInFile)
        );
        assert_eq!(builtin_tool_kind("move_path"), Some(ToolKind::MovePath));
        assert_eq!(builtin_tool_kind("remove_path"), Some(ToolKind::RemovePath));
        assert_eq!(builtin_tool_kind("list_dir"), Some(ToolKind::ListDir));
        assert_eq!(builtin_tool_kind("search_text"), Some(ToolKind::SearchText));
        assert_eq!(builtin_tool_kind("unknown"), None);
    }

    #[test]
    fn parse_builtin_input_uses_the_same_tool_shapes_as_runtime_execution() {
        let parsed = parse_builtin_input(
            ToolKind::WriteFile,
            serde_json::json!({
                "path": "note.txt",
                "contents": "hello",
            }),
        )
        .unwrap();
        assert_eq!(
            parsed,
            ToolInput::WriteFile {
                path: "note.txt".into(),
                contents: "hello".to_string(),
            }
        );
    }

    #[test]
    fn list_dir_tool_reports_directory_entries() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("nested")).unwrap();
        fs::write(temp.path().join("note.txt"), "hello").unwrap();
        let result =
            execute_list_dir_tool("list_dir", temp.path(), ListDirToolInput { path: None })
                .unwrap();
        assert!(result.output.stdout.contains("nested/"));
        assert!(result.output.stdout.contains("note.txt"));
    }

    #[test]
    fn replace_in_file_tool_updates_first_match() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("note.txt");
        fs::write(&path, "alpha beta alpha").unwrap();
        let result = execute_replace_in_file_tool(
            "replace_in_file",
            temp.path(),
            ReplaceInFileToolInput {
                path: "note.txt".into(),
                old: "alpha".to_string(),
                new: "omega".to_string(),
                replace_all: false,
            },
        )
        .unwrap();
        assert!(result.output.stdout.contains("updated"));
        let updated = fs::read_to_string(path).unwrap();
        assert_eq!(updated, "omega beta alpha");
    }

    #[test]
    fn move_path_tool_moves_files() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from.txt");
        let to = temp.path().join("nested/to.txt");
        fs::write(&from, "hello").unwrap();
        let result = execute_move_path_tool(
            "move_path",
            temp.path(),
            MovePathToolInput {
                from: "from.txt".into(),
                to: "nested/to.txt".into(),
            },
        )
        .unwrap();
        assert!(result.output.stdout.contains("moved"));
        assert!(!from.exists());
        assert_eq!(fs::read_to_string(to).unwrap(), "hello");
    }

    #[test]
    fn remove_path_tool_removes_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("dead.txt");
        fs::write(&path, "hello").unwrap();
        let result = execute_remove_path_tool(
            "remove_path",
            temp.path(),
            RemovePathToolInput {
                path: "dead.txt".into(),
                recursive: false,
            },
        )
        .unwrap();
        assert!(result.output.stdout.contains("removed"));
        assert!(!path.exists());
    }

    #[test]
    fn search_text_tool_finds_matching_lines() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("note.txt"), "needle\nhaystack\n").unwrap();
        let result = execute_search_text_tool(
            "search_text",
            temp.path(),
            SearchTextToolInput {
                query: "needle".to_string(),
                path: None,
            },
        )
        .unwrap();
        assert!(result.success);
        assert!(result.output.stdout.contains("needle"));
    }

    #[test]
    fn execute_builtin_tool_rejects_mismatched_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let error = execute_builtin_tool(
            "read_file",
            ToolKind::ReadFile,
            temp.path(),
            ToolInput::Bash {
                command: "printf hi".to_string(),
                timeout: None,
                run_in_background: false,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("tool input mismatch"));
    }

    #[test]
    fn read_file_tool_supports_offset_and_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("note.txt");
        fs::write(&path, "zero\none\ntwo\nthree\n").unwrap();
        let result = execute_read_file_tool(
            "read_file",
            temp.path(),
            ReadFileToolInput {
                path: "note.txt".into(),
                offset: Some(1),
                limit: Some(2),
            },
        )
        .unwrap();
        assert_eq!(result.output.stdout, "one\ntwo\n");
    }

    #[test]
    fn bash_tool_supports_timeout() {
        let temp = tempfile::tempdir().unwrap();
        let result = execute_bash_tool(
            "bash",
            temp.path(),
            BashToolInput {
                command: "sleep 1".to_string(),
                timeout: Some(10),
                run_in_background: false,
            },
        )
        .unwrap();
        assert!(!result.success);
        assert!(result.output.stderr.contains("timed out"));
    }

    #[test]
    fn bash_tool_uses_detected_shell() {
        let temp = tempfile::tempdir().unwrap();
        let result = execute_bash_tool(
            "bash",
            temp.path(),
            BashToolInput {
                command: "printf '%s' \"${BASH_VERSION:-${ZSH_VERSION:-${KSH_VERSION:-shell}}}\""
                    .to_string(),
                timeout: None,
                run_in_background: false,
            },
        )
        .unwrap();
        assert!(result.success);
        assert!(!result.output.stdout.trim().is_empty());
    }

    #[test]
    fn bash_tool_rejects_background_execution() {
        let temp = tempfile::tempdir().unwrap();
        let error = execute_bash_tool(
            "bash",
            temp.path(),
            BashToolInput {
                command: "printf hi".to_string(),
                timeout: None,
                run_in_background: true,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("background bash execution"));
    }

    #[test]
    fn read_file_rejects_parent_escape_outside_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let error = execute_read_file_tool(
            "read_file",
            temp.path(),
            ReadFileToolInput {
                path: "../secret.txt".into(),
                offset: None,
                limit: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("escapes workspace"));
    }

    #[test]
    fn write_file_rejects_absolute_path_outside_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let error = execute_write_file_tool(
            "write_file",
            workspace.path(),
            WriteFileToolInput {
                path: outside.path().join("secret.txt"),
                contents: "nope".to_string(),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("escapes workspace"));
    }

    #[cfg(unix)]
    #[test]
    fn read_file_rejects_symlink_escape_outside_workspace() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "classified").unwrap();
        symlink(outside.path(), workspace.path().join("link")).unwrap();
        let error = execute_read_file_tool(
            "read_file",
            workspace.path(),
            ReadFileToolInput {
                path: "link/secret.txt".into(),
                offset: None,
                limit: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("symlink outside workspace"));
    }

    #[test]
    fn rg_search_args_treat_leading_dash_queries_as_patterns() {
        let args = build_rg_search_args("-n", Path::new("/tmp/workspace"));
        assert_eq!(
            args,
            vec![
                "-n".to_string(),
                "--no-heading".to_string(),
                "-e".to_string(),
                "-n".to_string(),
                "--".to_string(),
                "/tmp/workspace".to_string(),
            ]
        );
    }

    #[test]
    fn grep_search_args_treat_leading_dash_queries_as_patterns() {
        let args = build_grep_search_args("--help", Path::new("/tmp/workspace"));
        assert_eq!(
            args,
            vec![
                "-RIn".to_string(),
                "-e".to_string(),
                "--help".to_string(),
                "--".to_string(),
                "/tmp/workspace".to_string(),
            ]
        );
    }
}
