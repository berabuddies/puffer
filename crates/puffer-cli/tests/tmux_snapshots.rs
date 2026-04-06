use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_session_store::SessionStore;
use puffer_test_support::{
    assert_normalized_snapshot, send_tmux_keys, start_tmux_command, temp_workspace, tmux_available,
    wait_for_tmux_text,
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
        &normalize_tmux_capture(&capture),
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
    let capture = wait_for_tmux_text(&session, "anthropic", Duration::from_secs(10)).unwrap();
    assert_normalized_snapshot(
        &normalize_tmux_capture(&capture),
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
    let lines = capture
        .lines()
        .map(normalize_tmux_line)
        .map(strip_tmux_chrome)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    if lines
        .iter()
        .any(|line| line.contains("Supported commands:"))
    {
        return lines
            .into_iter()
            .filter(|line| {
                line.starts_with("Puffer Code ·")
                    || line.contains("Supported commands:")
                    || line.starts_with('/')
                    || line.starts_with("Resource counts:")
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    lines
        .into_iter()
        .filter(|line| {
            matches!(
                line.as_str(),
                "Puffer Code"
                    | "Welcome to Puffer Code"
                    | "Clawd on duty"
                    | "anthropic/claude-sonnet-4-5"
                    | "workspace"
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_tmux_line(line: &str) -> String {
    let line = replace_session_marker(line);
    let line = replace_id_line(&line);
    replace_hex_runs(&line)
}

fn strip_tmux_chrome(line: String) -> String {
    let collapsed = line
        .chars()
        .map(|ch| match ch {
            '│' | '╭' | '╮' | '╰' | '╯' | '─' => ' ',
            other => other,
        })
        .collect::<String>();
    collapsed.split_whitespace().collect::<Vec<_>>().join(" ")
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
