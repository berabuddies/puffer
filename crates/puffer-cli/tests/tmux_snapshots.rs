use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_session_store::SessionStore;
use puffer_test_support::{
    assert_normalized_snapshot, capture_tmux_pane, send_tmux_keys, start_tmux_command,
    temp_workspace, tmux_available, wait_for_tmux_text,
};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn tmux_help_matches_snapshot() {
    if !tmux_available() {
        return;
    }

    let (_tempdir, workspace) = configured_workspace();
    let session = start_tmux_with_home(&workspace);
    wait_for_tmux_text(&session, "Puffer Code", Duration::from_secs(10)).unwrap();
    send_tmux_keys(&session, &["/help", "Enter"]).unwrap();
    let capture =
        wait_for_tmux_text(&session, "Supported commands:", Duration::from_secs(10)).unwrap();
    assert_normalized_snapshot(
        &normalize_help_capture(&capture),
        &snapshot_path("tmux_help_snapshot.txt"),
    )
    .unwrap();
}

#[test]
fn tmux_login_overlay_matches_snapshot() {
    if !tmux_available() {
        return;
    }

    let (_tempdir, workspace) = configured_workspace();
    let session = start_tmux_with_home(&workspace);
    wait_for_tmux_text(&session, "Puffer Code", Duration::from_secs(10)).unwrap();
    send_tmux_keys(&session, &["/login", "Enter"]).unwrap();
    wait_for_tmux_text(&session, "Select Provider", Duration::from_secs(10)).unwrap();
    std::thread::sleep(Duration::from_millis(250));
    let capture = capture_tmux_pane(&session).unwrap();
    assert_normalized_snapshot(
        &normalize_login_capture(&capture),
        &snapshot_path("tmux_login_overlay_snapshot.txt"),
    )
    .unwrap();
}

fn configured_workspace() -> (tempfile::TempDir, PathBuf) {
    let (tempdir, workspace) = temp_workspace().unwrap();
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(workspace.join("dockyard"))
        .unwrap();
    session_store
        .rename_session(session.id, "dockyard".to_string())
        .unwrap();
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    std::os::unix::fs::symlink(repo_root.join("resources"), workspace.join("resources")).unwrap();
    fs::write(
        workspace.join(".puffer/config.toml"),
        r#"
app_name = "Puffer Code"
default_provider = "anthropic"
default_model = "anthropic/claude-sonnet-4-5"
theme = "puffer"

[mascot]
id = "clawd"
display_name = "Clawd"
enabled = true

[ui]
no_alt_screen = true
tmux_golden_mode = true
"#,
    )
    .unwrap();
    fs::write(
        workspace.join(".puffer/auth.json"),
        r#"{
  "providers": {
    "anthropic": {
      "kind": "api_key",
      "key": "tmux-test-key"
    }
  }
}"#,
    )
    .unwrap();
    (tempdir, workspace)
}

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(name)
}

fn start_tmux_with_home(workspace: &std::path::Path) -> puffer_test_support::TmuxSession {
    start_tmux_command(
        "sh",
        &[
            "-lc",
            &format!(
                "HOME='{}' '{}'",
                workspace.display(),
                env!("CARGO_BIN_EXE_puffer")
            ),
        ],
        Some(workspace),
    )
    .unwrap()
}

fn normalize_tmux_capture(capture: &str) -> String {
    capture
        .lines()
        .map(normalize_tmux_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_help_capture(capture: &str) -> String {
    let mut lines = Vec::new();
    for line in normalize_tmux_capture(capture).lines() {
        if line.starts_with("Puffer Code") || line.trim() == "· Supported commands:" {
            lines.push(line.trim().to_string());
            continue;
        }
        if let Some(command) = extract_command_line(line) {
            lines.push(command);
            continue;
        }
        if line.trim_start().starts_with("Resource counts:") {
            lines.push(line.trim().to_string());
        }
    }
    lines.join("\n")
}

fn normalize_login_capture(capture: &str) -> String {
    normalize_tmux_capture(capture)
        .lines()
        .filter_map(|line| {
            let trimmed = line
                .trim()
                .trim_matches(|ch: char| matches!(ch, '│' | '╭' | '╰' | '╮' | '╯' | '─' | ' '));
            if trimmed == "Select Provider" {
                return Some(trimmed.to_string());
            }
            if let Some(selector) = trimmed.strip_prefix("anthropic") {
                return Some(format!("anthropic{}", normalize_overlay_suffix(selector)));
            }
            if let Some(selector) = trimmed.strip_prefix("openai") {
                return Some(format!("openai{}", normalize_overlay_suffix(selector)));
            }
            if trimmed == "You can still use /login <provider> for a direct auth hint." {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_overlay_suffix(suffix: &str) -> String {
    let suffix = suffix.split_whitespace().collect::<Vec<_>>().join(" ");
    if suffix.is_empty() {
        String::new()
    } else {
        format!(" {suffix}")
    }
}

fn extract_command_line(line: &str) -> Option<String> {
    let start = line.find('/')?;
    let mut command = String::new();
    for ch in line[start..].chars() {
        if ch.is_ascii_alphanumeric() || ch == '/' || ch == '-' || ch == ':' {
            command.push(ch);
        } else {
            break;
        }
    }
    if command.len() < 2 {
        return None;
    }
    let tail = line[start + command.len()..]
        .split(['│', '╮', '╯', '╭', '╰'])
        .next()
        .unwrap_or_default()
        .replace(['─', '·', '╭', '╰', '╮', '╯', '│'], " ");
    let description = tail.split_whitespace().collect::<Vec<_>>().join(" ");
    if description.is_empty() {
        Some(command)
    } else {
        Some(format!("{command} {description}"))
    }
}

fn normalize_tmux_line(line: &str) -> String {
    let line = replace_session_marker(line);
    let line = replace_id_line(&line);
    replace_hex_runs(&line)
}

fn replace_session_marker(line: &str) -> String {
    for marker in ["session=session-", "│session-"] {
        if let Some(index) = line.find(marker) {
            let start = index + marker.len();
            let mut end = start;
            let bytes = line.as_bytes();
            while end < line.len() {
                let byte = bytes[end];
                if byte.is_ascii_alphanumeric() || byte == b'.' {
                    end += 1;
                } else {
                    break;
                }
            }
            let mut normalized = line.to_string();
            normalized.replace_range(start..end, "<id>");
            return normalized;
        }
    }
    line.to_string()
}

fn replace_id_line(line: &str) -> String {
    let Some(index) = line.find("Id: ") else {
        return line.to_string();
    };
    let start = index + 4;
    let mut end = start;
    let bytes = line.as_bytes();
    while end < line.len() {
        let byte = bytes[end];
        if byte.is_ascii_alphanumeric() {
            end += 1;
        } else {
            break;
        }
    }
    let mut normalized = line.to_string();
    normalized.replace_range(start..end, "<id>");
    normalized
}

fn replace_hex_runs(line: &str) -> String {
    let mut output = String::new();
    let mut hex_run = String::new();

    for ch in line.chars() {
        if ch.is_ascii_hexdigit() {
            hex_run.push(ch);
            continue;
        }

        flush_hex_run(&mut output, &mut hex_run);
        output.push(ch);
    }
    flush_hex_run(&mut output, &mut hex_run);
    output
}

fn flush_hex_run(output: &mut String, hex_run: &mut String) {
    if hex_run.len() >= 6 {
        output.push_str("<id>");
    } else {
        output.push_str(hex_run);
    }
    hex_run.clear();
}
