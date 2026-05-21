use crate::{run_command_capture, CommandOutput, TerminalSize};
use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use uuid::Uuid;

const REQUIRE_TMUX_ENV: &str = "PUFFER_REQUIRE_TMUX_TESTS";
const REQUIRE_TMUX_ENV_LEGACY: &str = "PUFFER_REQUIRE_TMUX";

/// Describes tmux availability and optional version metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxInfo {
    pub available: bool,
    pub version: Option<String>,
}

/// Owns one temporary tmux session created for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxSession {
    pub name: String,
    pub socket_name: String,
}

/// Returns true when tmux appears to be installed and runnable.
pub fn tmux_available() -> bool {
    detect_tmux().available
}

/// Returns true when a tmux-backed test may continue, or skips with a clear gate.
pub fn require_tmux_or_skip(test_name: &str) -> bool {
    let info = detect_tmux();
    if info.available {
        return true;
    }

    let message = format!(
        "tmux is not available; `{test_name}` did not exercise the TUI. Install tmux or unset {REQUIRE_TMUX_ENV} to allow local skips."
    );
    if require_tmux_tests() {
        panic!("{message}");
    }
    eprintln!("skipping `{test_name}`: {message}");
    false
}

/// Probes the local system for tmux and captures its version when available.
pub fn detect_tmux() -> TmuxInfo {
    match run_command_capture("tmux", &["-V"], None) {
        Ok(output) if output.status_code == 0 => TmuxInfo {
            available: true,
            version: Some(output.stdout.trim().to_string()),
        },
        _ => TmuxInfo {
            available: false,
            version: None,
        },
    }
}

fn require_tmux_tests() -> bool {
    [REQUIRE_TMUX_ENV, REQUIRE_TMUX_ENV_LEGACY]
        .iter()
        .any(|name| {
            std::env::var(name)
                .map(|value| tmux_require_env_value(Some(&value)))
                .unwrap_or(false)
        })
}

fn tmux_require_env_value(value: Option<&str>) -> bool {
    value
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Starts a detached tmux session that runs the provided program and arguments.
pub fn start_tmux_command(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<TmuxSession> {
    start_tmux_command_with_size(program, args, cwd, TerminalSize::default())
}

/// Starts a detached tmux session using an isolated server and explicit pane size.
pub fn start_tmux_command_with_size(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    size: TerminalSize,
) -> Result<TmuxSession> {
    let session = TmuxSession {
        name: format!("puffer-{}", Uuid::new_v4().simple()),
        socket_name: format!("puffer-test-{}", Uuid::new_v4().simple()),
    };
    let mut command = Command::new("tmux");
    command
        .arg("-L")
        .arg(&session.socket_name)
        .arg("-f")
        .arg("/dev/null")
        .arg("new-session")
        .arg("-d")
        .arg("-x")
        .arg(size.cols.to_string())
        .arg("-y")
        .arg(size.rows.to_string())
        .arg("-s")
        .arg(&session.name);
    if let Some(cwd) = cwd {
        command.arg("-c").arg(cwd);
    }
    command.arg(shell_command(program, args));
    let status = command.status()?;
    if !status.success() {
        return Err(anyhow!("failed to start tmux session {}", session.name));
    }
    let output = run_tmux_command(
        &session,
        &["set-option", "-t", &session.name, "remain-on-exit", "on"],
    )?;
    if output.status_code != 0 {
        return Err(anyhow!(
            "failed to set remain-on-exit for {}: {}",
            session.name,
            output.stderr.trim()
        ));
    }
    Ok(session)
}

/// Captures the current pane contents for a tmux session.
pub fn capture_tmux_pane(session: &TmuxSession) -> Result<String> {
    let target = format!("{}:0.0", session.name);
    let output = run_tmux_command(session, &["capture-pane", "-p", "-S", "-", "-t", &target])?;
    if output.status_code != 0 {
        return Err(anyhow!(
            "failed to capture tmux pane for {}: {}",
            session.name,
            output.stderr.trim()
        ));
    }
    Ok(output.stdout)
}

/// Captures only the visible pane viewport for a tmux session.
pub fn capture_tmux_visible_pane(session: &TmuxSession) -> Result<String> {
    let target = format!("{}:0.0", session.name);
    let output = run_tmux_command(session, &["capture-pane", "-p", "-t", &target])?;
    if output.status_code != 0 {
        return Err(anyhow!(
            "failed to capture visible tmux pane for {}: {}",
            session.name,
            output.stderr.trim()
        ));
    }
    Ok(output.stdout)
}

/// Sends tmux key presses to the target session pane.
pub fn send_tmux_keys(session: &TmuxSession, keys: &[&str]) -> Result<()> {
    for key in keys {
        if tmux_key_name(key) {
            let output =
                run_tmux_command(session, &["send-keys", "-t", session.name.as_str(), key])?;
            if output.status_code != 0 {
                return Err(anyhow!(
                    "failed to send tmux keys to {}: {}",
                    session.name,
                    output.stderr.trim()
                ));
            }
            continue;
        }

        let output = run_tmux_command(
            session,
            &["send-keys", "-l", "-t", session.name.as_str(), key],
        )?;
        if output.status_code != 0 {
            return Err(anyhow!(
                "failed to send tmux keys to {}: {}",
                session.name,
                output.stderr.trim()
            ));
        }
    }
    Ok(())
}

fn tmux_key_name(key: &str) -> bool {
    matches!(
        key,
        "Enter"
            | "Escape"
            | "Esc"
            | "BSpace"
            | "Backspace"
            | "Delete"
            | "Up"
            | "Down"
            | "Left"
            | "Right"
            | "Tab"
            | "Space"
    ) || key.starts_with("C-")
        || key.starts_with("M-")
        || key.starts_with("S-")
}

/// Waits until the tmux pane contains the requested text or the timeout expires.
pub fn wait_for_tmux_text(
    session: &TmuxSession,
    needle: &str,
    timeout: Duration,
) -> Result<String> {
    wait_for_tmux_capture(session, needle, timeout, capture_tmux_pane)
}

/// Waits until the visible tmux pane contains the requested text.
pub fn wait_for_tmux_visible_text(
    session: &TmuxSession,
    needle: &str,
    timeout: Duration,
) -> Result<String> {
    wait_for_tmux_capture(session, needle, timeout, capture_tmux_visible_pane)
}

fn wait_for_tmux_capture(
    session: &TmuxSession,
    needle: &str,
    timeout: Duration,
    capture: fn(&TmuxSession) -> Result<String>,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let pane = capture(session)?;
        if pane.contains(needle) {
            return Ok(pane);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting for `{needle}` in tmux session {}\nlast pane capture:\n{}",
                session.name,
                pane
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Kills a tmux session created for testing.
pub fn kill_tmux_session(session: &TmuxSession) -> Result<()> {
    let output = run_tmux_command(session, &["kill-server"])?;
    if output.status_code != 0 {
        return Err(anyhow!(
            "failed to kill tmux session {}: {}",
            session.name,
            output.stderr.trim()
        ));
    }
    Ok(())
}

impl Drop for TmuxSession {
    fn drop(&mut self) {
        let _ = run_tmux_command(self, &["kill-server"]);
    }
}

fn run_tmux_command(session: &TmuxSession, args: &[&str]) -> Result<CommandOutput> {
    let owned = std::iter::once("-L".to_string())
        .chain(std::iter::once(session.socket_name.clone()))
        .chain(args.iter().map(|value| (*value).to_string()))
        .collect::<Vec<_>>();
    let refs = owned.iter().map(String::as_str).collect::<Vec<_>>();
    run_command_capture("tmux", &refs, None)
}

fn shell_command(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .map(shell_escape)
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_tmux_never_panics() {
        let _ = detect_tmux();
    }

    #[test]
    fn tmux_command_round_trips_output_when_available() {
        if !require_tmux_or_skip("tmux_command_round_trips_output_when_available") {
            return;
        }
        let session =
            start_tmux_command("sh", &["-lc", "printf 'hello from tmux'; sleep 2"], None).unwrap();
        let capture =
            wait_for_tmux_text(&session, "hello from tmux", Duration::from_secs(3)).unwrap();
        assert!(capture.contains("hello from tmux"));
    }

    #[test]
    fn tmux_command_respects_requested_size() {
        if !require_tmux_or_skip("tmux_command_respects_requested_size") {
            return;
        }
        let session = start_tmux_command_with_size(
            "sh",
            &["-lc", "sleep 2"],
            None,
            TerminalSize { rows: 24, cols: 80 },
        )
        .unwrap();
        let target = format!("{}:0.0", session.name);
        let output = run_tmux_command(
            &session,
            &[
                "display-message",
                "-p",
                "-t",
                &target,
                "#{pane_width}x#{pane_height}",
            ],
        )
        .unwrap();
        assert_eq!(output.stdout.trim(), "80x24");
    }

    #[test]
    fn tmux_require_env_values_are_explicit() {
        assert!(!tmux_require_env_value(None));
        assert!(!tmux_require_env_value(Some("")));
        assert!(!tmux_require_env_value(Some("0")));
        assert!(!tmux_require_env_value(Some("false")));
        assert!(tmux_require_env_value(Some("1")));
        assert!(tmux_require_env_value(Some("true")));
        assert!(tmux_require_env_value(Some("YES")));
        assert!(tmux_require_env_value(Some(" on ")));
    }
}
