use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;

/// Claude-compatible input payload for the `Bash` tool.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ClaudeBashInput {
    pub command: String,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub run_in_background: bool,
    #[serde(default, rename = "dangerouslyDisableSandbox")]
    pub dangerously_disable_sandbox: bool,
}

/// Claude-compatible output payload for the `Bash` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeBashOutput {
    pub stdout: String,
    pub stderr: String,
    pub interrupted: bool,
    #[serde(skip_serializing_if = "Option::is_none", rename = "backgroundTaskId")]
    pub background_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "outputFile")]
    pub output_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "backgroundedByUser")]
    pub backgrounded_by_user: Option<bool>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "assistantAutoBackgrounded"
    )]
    pub assistant_auto_backgrounded: Option<bool>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "dangerouslyDisableSandbox"
    )]
    pub dangerously_disable_sandbox: Option<bool>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "returnCodeInterpretation"
    )]
    pub return_code_interpretation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "noOutputExpected")]
    pub no_output_expected: Option<bool>,
}

/// Normalized result envelope for one Claude-style Bash execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeBashExecution {
    pub success: bool,
    pub output: ClaudeBashOutput,
}

/// Returns the model-facing description text used for one `Bash` invocation.
pub fn tool_description(input: &ClaudeBashInput) -> String {
    input
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "Run shell command".to_string())
}

/// Parses JSON input and executes a Claude-style `Bash` tool invocation.
pub fn execute_from_value(
    cwd: &Path,
    session_id: &Uuid,
    input: Value,
) -> Result<ClaudeBashExecution> {
    let typed: ClaudeBashInput =
        serde_json::from_value(input).context("invalid Bash tool input payload")?;
    execute(cwd, session_id, typed)
}

/// Executes a Claude-style `Bash` tool invocation in the provided working directory.
pub fn execute(
    cwd: &Path,
    session_id: &Uuid,
    input: ClaudeBashInput,
) -> Result<ClaudeBashExecution> {
    if input.run_in_background {
        return execute_background(cwd, session_id, input);
    }
    execute_foreground(cwd, input)
}

fn execute_background(
    cwd: &Path,
    session_id: &Uuid,
    input: ClaudeBashInput,
) -> Result<ClaudeBashExecution> {
    let output_dir = shell_output_dir(cwd)?;
    let pending_output_file =
        output_dir.join(format!("shell-pending-{}.log", unique_output_nonce()));
    let stdout = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&pending_output_file)
        .with_context(|| format!("failed to create {}", pending_output_file.display()))?;
    let stderr = stdout
        .try_clone()
        .with_context(|| format!("failed to clone {}", pending_output_file.display()))?;
    let mut child = Command::new(puffer_tools::detected_shell())
        .arg("-lc")
        .arg(&input.command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| {
            format!(
                "failed to start background bash command in {}",
                cwd.display()
            )
        })?;
    let pid = child.id();
    let task_id = format!("shell-{}", pid);
    let subject = tool_description(&input);
    let output_file = shell_output_path(cwd, pid)?;
    let _ = fs::rename(&pending_output_file, &output_file);
    super::workflow::register_background_shell_task(
        cwd,
        session_id,
        &task_id,
        &subject,
        &input.command,
        pid,
        &output_file,
    )?;

    // Spawn a reaper thread that calls wait() on the child process.
    // Without this, the child becomes a zombie after exit because nobody
    // collects its exit status.  The reaper also marks the task as completed
    // in the persistent store so that status queries see accurate state
    // instead of relying on `kill -0` (which returns true for zombies).
    let reaper_cwd = cwd.to_path_buf();
    let reaper_session_id = *session_id;
    let reaper_task_id = task_id.clone();
    thread::spawn(move || {
        let exit_status = child.wait();
        let exit_code = exit_status.ok().and_then(|s| s.code());
        // Best-effort: mark the stored task as completed.
        let _ = super::workflow::mark_shell_task_completed(
            &reaper_cwd,
            &reaper_session_id,
            &reaper_task_id,
            exit_code,
        );
    });

    Ok(ClaudeBashExecution {
        success: true,
        output: ClaudeBashOutput {
            stdout: String::new(),
            stderr: String::new(),
            interrupted: false,
            background_task_id: Some(task_id),
            output_file: Some(output_file.display().to_string()),
            backgrounded_by_user: Some(false),
            assistant_auto_backgrounded: Some(false),
            dangerously_disable_sandbox: Some(input.dangerously_disable_sandbox),
            return_code_interpretation: None,
            no_output_expected: Some(true),
        },
    })
}

fn execute_foreground(cwd: &Path, input: ClaudeBashInput) -> Result<ClaudeBashExecution> {
    let timeout_ms = input
        .timeout
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1, MAX_TIMEOUT_MS);
    let command = input.command.clone();
    let timed = run_bash_command(cwd, &command, timeout_ms)?;
    let mut stderr = String::from_utf8_lossy(&timed.output.stderr).to_string();
    if timed.timed_out {
        if !stderr.trim().is_empty() {
            stderr.push('\n');
        }
        stderr.push_str(&format!("command timed out after {timeout_ms}ms"));
    }
    let stdout = truncate_output(String::from_utf8_lossy(&timed.output.stdout).to_string());
    let success = timed.output.status.success() && !timed.timed_out;
    let no_output_expected = success && stdout.trim().is_empty() && stderr.trim().is_empty();
    Ok(ClaudeBashExecution {
        success,
        output: ClaudeBashOutput {
            stdout,
            stderr,
            interrupted: timed.timed_out,
            background_task_id: None,
            output_file: None,
            backgrounded_by_user: None,
            assistant_auto_backgrounded: None,
            dangerously_disable_sandbox: Some(input.dangerously_disable_sandbox),
            return_code_interpretation: classify_return_code(timed.output.status.code()),
            no_output_expected: Some(no_output_expected),
        },
    })
}

fn classify_return_code(code: Option<i32>) -> Option<String> {
    match code {
        Some(130) => Some("interrupted_by_signal".to_string()),
        Some(137) => Some("killed".to_string()),
        _ => None,
    }
}

struct TimedCommandOutput {
    output: Output,
    timed_out: bool,
}

fn unique_output_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn shell_output_dir(cwd: &Path) -> Result<std::path::PathBuf> {
    let dir = cwd
        .join(".puffer")
        .join("runtime")
        .join("claude_workflow")
        .join("shell_outputs");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}

fn shell_output_path(cwd: &Path, pid: u32) -> Result<std::path::PathBuf> {
    Ok(shell_output_dir(cwd)?.join(format!("shell-{pid}.log")))
}

fn run_bash_command(cwd: &Path, command: &str, timeout_ms: u64) -> Result<TimedCommandOutput> {
    let mut process = Command::new(puffer_tools::detected_shell());
    process
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_foreground_process_group(&mut process);
    let mut child = process
        .spawn()
        .with_context(|| format!("failed to execute bash command in {}", cwd.display()))?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if child
            .try_wait()
            .with_context(|| format!("failed to poll bash command in {}", cwd.display()))?
            .is_some()
        {
            let output = child
                .wait_with_output()
                .with_context(|| format!("failed to collect bash output in {}", cwd.display()))?;
            return Ok(TimedCommandOutput {
                output,
                timed_out: false,
            });
        }
        if Instant::now() >= deadline {
            terminate_foreground_process_tree(&mut child);
            let output = child.wait_with_output().with_context(|| {
                format!(
                    "failed to collect timed-out bash output in {}",
                    cwd.display()
                )
            })?;
            return Ok(TimedCommandOutput {
                output,
                timed_out: true,
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn configure_foreground_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_foreground_process_group(_command: &mut Command) {}

fn terminate_foreground_process_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let process_group = -(child.id() as i32);
        let _ = send_signal(process_group, SIGTERM);
        for _ in 0..10 {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = send_signal(process_group, SIGKILL);
        return;
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

#[cfg(unix)]
const SIGTERM: i32 = 15;

#[cfg(unix)]
const SIGKILL: i32 = 9;

#[cfg(unix)]
fn send_signal(pid: i32, signal: i32) -> i32 {
    unsafe { kill(pid, signal) }
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

const MAX_OUTPUT_CHARS: usize = 30_000;

/// Truncates large output using a middle-truncation strategy (Codex pattern):
/// keeps the first half and last half of the budget so that both the initial
/// context and the trailing error messages / results are preserved.
fn truncate_output(output: String) -> String {
    let chars: Vec<char> = output.chars().collect();
    if chars.len() <= MAX_OUTPUT_CHARS {
        return output;
    }
    let head_len = MAX_OUTPUT_CHARS / 2;
    let tail_len = MAX_OUTPUT_CHARS - head_len;
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[chars.len() - tail_len..].iter().collect();
    let omitted = chars.len() - MAX_OUTPUT_CHARS;
    format!("{head}\n\n[…{omitted} chars truncated…]\n\n{tail}")
}

/// Builds a human-readable summary line for UI/status displays.
pub fn summary_line(input: &ClaudeBashInput) -> Result<String> {
    if input.command.trim().is_empty() {
        return Err(anyhow!("Bash command cannot be empty"));
    }
    Ok(tool_description(input))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn test_session_id() -> Uuid {
        Uuid::nil()
    }

    #[test]
    fn description_uses_fallback_when_omitted() {
        let input = ClaudeBashInput {
            command: "echo hi".to_string(),
            timeout: None,
            description: None,
            run_in_background: false,
            dangerously_disable_sandbox: false,
        };
        assert_eq!(tool_description(&input), "Run shell command");
    }

    #[test]
    fn description_prefers_explicit_value() {
        let input = ClaudeBashInput {
            command: "echo hi".to_string(),
            timeout: None,
            description: Some("Show greeting".to_string()),
            run_in_background: false,
            dangerously_disable_sandbox: false,
        };
        assert_eq!(tool_description(&input), "Show greeting");
    }

    #[test]
    fn execute_foreground_returns_stdout() {
        let temp = tempfile::tempdir().unwrap();
        let result = execute(
            temp.path(),
            &test_session_id(),
            ClaudeBashInput {
                command: "printf 'hello'".to_string(),
                timeout: Some(1_000),
                description: None,
                run_in_background: false,
                dangerously_disable_sandbox: false,
            },
        )
        .unwrap();
        assert!(result.success);
        assert_eq!(result.output.stdout, "hello");
        assert!(!result.output.interrupted);
        assert_eq!(result.output.dangerously_disable_sandbox, Some(false));
    }

    #[test]
    fn execute_timeout_marks_interrupted() {
        let temp = tempfile::tempdir().unwrap();
        let result = execute(
            temp.path(),
            &test_session_id(),
            ClaudeBashInput {
                command: "sleep 0.2".to_string(),
                timeout: Some(20),
                description: None,
                run_in_background: false,
                dangerously_disable_sandbox: true,
            },
        )
        .unwrap();
        assert!(!result.success);
        assert!(result.output.interrupted);
        assert!(result.output.stderr.contains("timed out after"));
        assert_eq!(result.output.dangerously_disable_sandbox, Some(true));
    }

    #[test]
    fn execute_timeout_collects_after_descendant_keeps_pipe_open() {
        let temp = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let result = execute(
            temp.path(),
            &test_session_id(),
            ClaudeBashInput {
                command: "sh -c 'sleep 2'".to_string(),
                timeout: Some(20),
                description: None,
                run_in_background: false,
                dangerously_disable_sandbox: false,
            },
        )
        .unwrap();

        assert!(!result.success);
        assert!(result.output.interrupted);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn execute_background_returns_task_id() {
        let temp = tempfile::tempdir().unwrap();
        let result = execute(
            temp.path(),
            &test_session_id(),
            ClaudeBashInput {
                command: "sleep 0.1".to_string(),
                timeout: Some(1_000),
                description: None,
                run_in_background: true,
                dangerously_disable_sandbox: false,
            },
        )
        .unwrap();
        assert!(result.success);
        assert!(result.output.background_task_id.is_some());
        assert_eq!(result.output.backgrounded_by_user, Some(false));
        assert_eq!(result.output.assistant_auto_backgrounded, Some(false));
    }

    #[test]
    fn execute_background_persists_shell_task() {
        let temp = tempfile::tempdir().unwrap();
        let result = execute(
            temp.path(),
            &test_session_id(),
            ClaudeBashInput {
                command: "sleep 0.1".to_string(),
                timeout: Some(1_000),
                description: Some("Sleep briefly".to_string()),
                run_in_background: true,
                dangerously_disable_sandbox: false,
            },
        )
        .unwrap();

        let task_id = result.output.background_task_id.as_deref().unwrap();
        let tasks_path = temp
            .path()
            .join(".puffer")
            .join("runtime")
            .join("claude_workflow")
            .join("sessions")
            .join(test_session_id().to_string())
            .join("tasks.json");
        let payload: Value =
            serde_json::from_str(&fs::read_to_string(tasks_path).unwrap()).unwrap();
        let tasks = payload.get("tasks").and_then(Value::as_array).unwrap();
        let stored = tasks
            .iter()
            .find(|task| task.get("task_id").and_then(Value::as_str) == Some(task_id))
            .unwrap();
        assert_eq!(
            stored.get("task_type").and_then(Value::as_str),
            Some("shell")
        );
        assert_eq!(
            stored.get("status").and_then(Value::as_str),
            Some("running")
        );
        assert_eq!(
            stored.get("command").and_then(Value::as_str),
            Some("sleep 0.1")
        );
        assert_eq!(
            stored.get("subject").and_then(Value::as_str),
            Some("Sleep briefly")
        );
    }

    #[test]
    fn execute_from_value_parses_claude_field_names() {
        let temp = tempfile::tempdir().unwrap();
        let input = json!({
            "command": "printf ok",
            "timeout": 1000,
            "description": "Print test token",
            "run_in_background": false,
            "dangerouslyDisableSandbox": true
        });
        let result = execute_from_value(temp.path(), &test_session_id(), input).unwrap();
        assert!(result.success);
        assert_eq!(result.output.stdout, "ok");
        assert_eq!(result.output.dangerously_disable_sandbox, Some(true));
    }

    #[test]
    fn summary_line_rejects_empty_commands() {
        let input = ClaudeBashInput {
            command: "   ".to_string(),
            timeout: None,
            description: None,
            run_in_background: false,
            dangerously_disable_sandbox: false,
        };
        let error = summary_line(&input).unwrap_err();
        assert!(error.to_string().contains("cannot be empty"));
    }

    #[test]
    fn truncate_output_preserves_short_output() {
        let short = "hello world".to_string();
        assert_eq!(truncate_output(short.clone()), short);
    }

    #[test]
    fn truncate_output_preserves_exact_limit() {
        let exact: String = "x".repeat(MAX_OUTPUT_CHARS);
        assert_eq!(truncate_output(exact.clone()), exact);
    }

    #[test]
    fn truncate_output_uses_middle_truncation() {
        // Build output: "AAAA...BBB..." where head is A's and tail is B's
        let head_marker = "HEAD_START";
        let tail_marker = "TAIL_END!!";
        let filler_len = MAX_OUTPUT_CHARS + 10_000;
        let mut big = String::with_capacity(filler_len + 20);
        big.push_str(head_marker);
        for _ in 0..(filler_len - head_marker.len() - tail_marker.len()) {
            big.push('.');
        }
        big.push_str(tail_marker);

        let result = truncate_output(big);
        // Head preserved
        assert!(result.starts_with(head_marker), "head must be preserved");
        // Tail preserved
        assert!(result.ends_with(tail_marker), "tail must be preserved");
        // Truncation marker present
        assert!(result.contains("[…"), "must contain truncation marker");
        assert!(result.contains("chars truncated…]"), "must show char count");
    }

    #[test]
    fn truncate_output_handles_unicode() {
        // 50k Chinese chars — well above 30k limit
        let chinese: String = "测试".repeat(25_000);
        let result = truncate_output(chinese);
        assert!(result.contains("[…"), "must contain truncation marker");
        // Head should start with 测试
        assert!(result.starts_with("测试"), "head must preserve Chinese");
        // Tail should end with 测试
        assert!(result.ends_with("测试"), "tail must preserve Chinese");
    }

    #[test]
    fn truncate_output_middle_truncation_real_bash() {
        // Simulate large output: head=AAAAAA..., tail=ZZZZZZ...
        let temp = tempfile::tempdir().unwrap();
        // Emit 20000 A chars, then 20000 Z chars. This avoids non-portable
        // helpers such as `jot` while still exercising real shell output.
        let result = execute(
            temp.path(),
            &test_session_id(),
            ClaudeBashInput {
                command: "awk 'BEGIN { for (i = 0; i < 20000; i++) printf \"A\"; for (i = 0; i < 20000; i++) printf \"Z\" }'".to_string(),
                timeout: Some(5_000),
                description: None,
                run_in_background: false,
                dangerously_disable_sandbox: false,
            },
        )
        .unwrap();
        assert!(result.success, "command failed: {}", result.output.stderr);
        let stdout = &result.output.stdout;
        // 40000 chars > 30000 limit, must be truncated
        assert!(
            stdout.contains("[…"),
            "large output must be middle-truncated"
        );
        // Head preserved (starts with A's)
        assert!(stdout.starts_with('A'), "head must start with A");
        // Tail preserved (ends with Z's)
        assert!(stdout.ends_with('Z'), "tail must end with Z");
    }
}
