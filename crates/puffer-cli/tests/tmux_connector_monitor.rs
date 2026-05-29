use anyhow::{Context, Result};
use puffer_test_support::{
    capture_tmux_pane, require_tmux_or_skip, send_tmux_keys, start_tmux_command_with_size,
    temp_workspace, wait_for_tmux_text, TerminalSize,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const TMUX_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

#[test]
fn tmux_connect_serve_connectors_write_workspace_config() {
    if !require_tmux_or_skip("tmux_connect_serve_connectors_write_workspace_config") {
        return;
    }

    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let session = start_puffer_tui(&workspace, &puffer_home);

    submit_command(&session, "/connect discord-bot discord-e2e");
    answer_input(&session, "Discord bot token", "test-discord-token");
    wait_for_tmux_text(&session, "connector: discord-bot", TMUX_WAIT_TIMEOUT).unwrap();

    submit_command(&session, "/connect matrix-bot matrix-e2e");
    answer_input(
        &session,
        "Matrix homeserver URL",
        "https://matrix.example.test",
    );
    answer_input(&session, "Matrix username", "@puffer:example.test");
    answer_input(&session, "Matrix password", "test-matrix-password");
    wait_for_tmux_text(&session, "connector: matrix-bot", TMUX_WAIT_TIMEOUT).unwrap();

    let config = read_toml(workspace.join(".puffer/connectors.toml"));
    assert_eq!(
        config["connectors"]["discord"]["display_name"].as_str(),
        Some("discord-e2e")
    );
    assert_eq!(
        config["connectors"]["discord"]["token"].as_str(),
        Some("test-discord-token")
    );
    assert_eq!(
        config["connectors"]["matrix"]["display_name"].as_str(),
        Some("matrix-e2e")
    );
    assert_eq!(
        config["connectors"]["matrix"]["homeserver_url"].as_str(),
        Some("https://matrix.example.test")
    );
}

#[test]
fn tmux_connect_email_then_monitor_email_creates_workflow() {
    if !require_tmux_or_skip("tmux_connect_email_then_monitor_email_creates_workflow") {
        return;
    }

    let (_tempdir, workspace, puffer_home) = configured_workspace();
    write_workspace_email_manifest(&workspace);
    let session = start_puffer_tui(&workspace, &puffer_home);

    submit_command(&session, "/connect email email-e2e");
    answer_input(&session, "IMAP host", "imap.example.test");
    answer_input(&session, "IMAP port", "993");
    answer_input(&session, "SMTP host", "smtp.example.test");
    answer_input(&session, "SMTP port", "465");
    answer_input(&session, "email username", "puffer@example.test");
    answer_input(
        &session,
        "email password or app password",
        "test-email-password",
    );
    answer_input(&session, "from address", "puffer@example.test");
    wait_for_tmux_text(&session, "connector: email", TMUX_WAIT_TIMEOUT).unwrap();

    wait_for_file_text(
        &workspace.join(".puffer/connections.json"),
        "email-e2e",
        TMUX_WAIT_TIMEOUT,
    )
    .unwrap();
    wait_for_file_text(
        &workspace.join(".puffer/subscribers/email/state/config.toml"),
        "imap.example.test",
        TMUX_WAIT_TIMEOUT,
    )
    .unwrap();

    submit_command(&session, "/monitor email-e2e");
    wait_for_tmux_text(&session, "Monitor setup", TMUX_WAIT_TIMEOUT).unwrap();
    wait_for_tmux_text(&session, "workflow=monitor-email-e2e", TMUX_WAIT_TIMEOUT).unwrap();

    let monitor_memory = workspace.join(".puffer/runtime/monitors/email-e2e.md");
    let memory = fs::read_to_string(&monitor_memory)
        .with_context(|| format!("read {}", monitor_memory.display()))
        .unwrap();
    assert!(memory.contains("Connector: email"));

    let subscriptions = read_json(workspace.join(".puffer/subscriptions.json"));
    assert!(
        subscriptions["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|subscription| subscription["slug"] == "monitor-email-e2e"
                && subscription["connection_slug"] == "email-e2e"),
        "workflow binding missing monitor-email-e2e: {subscriptions}"
    );
}

#[test]
fn tmux_connect_slack_app_live_when_credentials_present() {
    if !require_tmux_or_skip("tmux_connect_slack_app_live_when_credentials_present") {
        return;
    }
    let Some(bot_token) = optional_live_secret("PUFFER_E2E_SLACK_BOT_TOKEN") else {
        return;
    };
    let Some(app_token) = optional_live_secret("PUFFER_E2E_SLACK_APP_TOKEN") else {
        return;
    };

    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let session = start_puffer_tui(&workspace, &puffer_home);

    submit_command(&session, "/connect slack-app slack-live");
    answer_secret_input(&session, "Slack bot token", &bot_token);
    answer_secret_input(&session, "Slack app-level token", &app_token);
    wait_for_tmux_text_redacted(&session, "connector: slack-app", TMUX_WAIT_TIMEOUT).unwrap();
    wait_for_tmux_text_redacted(&session, "connection record: created", TMUX_WAIT_TIMEOUT).unwrap();

    let credential =
        read_json(workspace.join(".puffer/slack-accounts/slack-live/credentials.json"));
    assert_eq!(credential["connection_slug"].as_str(), Some("slack-live"));
    assert_eq!(credential["connector_slug"].as_str(), Some("slack-app"));
    assert_eq!(credential["auth"]["kind"].as_str(), Some("app"));
    assert!(
        credential["workspace_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "Slack auth.test did not hydrate workspace_id"
    );
    assert!(
        credential["user_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "Slack auth.test did not hydrate user_id"
    );

    let connections = read_json(workspace.join(".puffer/connections.json"));
    assert!(
        connections["connections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|connection| {
                connection["slug"] == "slack-live"
                    && connection["connector_slug"] == "slack-app"
                    && connection["state"] == "authenticated"
            }),
        "Slack connection record missing authenticated slack-live: {connections}"
    );

    submit_command(&session, "/monitor slack-live");
    wait_for_tmux_text_redacted(
        &session,
        "no event-capable connections are available",
        TMUX_WAIT_TIMEOUT,
    )
    .unwrap();
}

fn configured_workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (tempdir, workspace) = temp_workspace().unwrap();
    let puffer_home = workspace.clone();
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
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
    store_dummy_provider_key(&workspace, &puffer_home);
    (tempdir, workspace, puffer_home)
}

fn write_workspace_email_manifest(workspace: &Path) {
    let dir = workspace.join(".puffer/subscribers/email");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("manifest.toml"),
        r#"manifest_version = 1
id = "email"
kind = "subscriber"
topic = "email"
display_name = "Email (IMAP / SMTP)"

[run]
cmd = ["puffer", "__subscriber", "email"]

[state]
dir = "state"
"#,
    )
    .unwrap();
}

fn start_puffer_tui(workspace: &Path, puffer_home: &Path) -> puffer_test_support::TmuxSession {
    let binary = Path::new(env!("CARGO_BIN_EXE_puffer"));
    let binary_dir = binary.parent().unwrap();
    let session = start_tmux_command_with_size(
        "sh",
        &[
            "-lc",
            &format!(
                "PATH='{}':$PATH PUFFER_HOME='{}' HOME='{}' '{}'",
                binary_dir.display(),
                puffer_home.display(),
                workspace.display(),
                binary.display()
            ),
        ],
        Some(workspace),
        TerminalSize {
            cols: 140,
            rows: 40,
        },
    )
    .unwrap();
    wait_for_tmux_text(&session, "Puffer Code", TMUX_WAIT_TIMEOUT).unwrap();
    session
}

fn submit_command(session: &puffer_test_support::TmuxSession, command: &str) {
    send_tmux_keys(session, &[command]).unwrap();
    wait_for_prompt_input(session, command, Duration::from_secs(3)).unwrap();
    send_tmux_keys(session, &["Enter"]).unwrap();
}

fn wait_for_prompt_input(
    session: &puffer_test_support::TmuxSession,
    command: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        let pane = capture_tmux_pane(session).unwrap_or_default();
        if pane
            .lines()
            .any(|line| line.trim_start().starts_with('❯') && line.contains(command))
        {
            return Ok(());
        }
        last = pane;
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("timed out waiting for prompt input `{command}`\n{last}");
}

fn answer_input(session: &puffer_test_support::TmuxSession, question: &str, answer: &str) {
    wait_for_tmux_text(session, question, TMUX_WAIT_TIMEOUT)
        .with_context(|| {
            let pane = capture_tmux_pane(session).unwrap_or_default();
            format!("waiting for `{question}` before answering `{answer}`\n{pane}")
        })
        .unwrap();
    send_tmux_keys(session, &[answer, "Enter"]).unwrap();
}

fn answer_secret_input(session: &puffer_test_support::TmuxSession, question: &str, answer: &str) {
    wait_for_tmux_text_redacted(session, question, TMUX_WAIT_TIMEOUT).unwrap();
    send_tmux_keys(session, &[answer, "Enter"]).unwrap();
}

fn optional_live_secret(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => {
            eprintln!("skipping live Slack TUI e2e because {name} is not set");
            None
        }
    }
}

fn store_dummy_provider_key(workspace: &Path, puffer_home: &Path) {
    let binary = Path::new(env!("CARGO_BIN_EXE_puffer"));
    let output = Command::new(binary)
        .args(["auth", "set-api-key", "anthropic", "dummy-anthropic-key"])
        .env("PUFFER_HOME", puffer_home)
        .env("HOME", workspace)
        .current_dir(workspace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "failed to store dummy provider key: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn redact_secrets(value: &str) -> String {
    value
        .split_whitespace()
        .map(|token| {
            if token.starts_with("xapp-") || token.starts_with("xox") {
                "<redacted>"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn wait_for_tmux_text_redacted(
    session: &puffer_test_support::TmuxSession,
    needle: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        let pane = capture_tmux_pane(session).unwrap_or_default();
        if pane.contains(needle) {
            return Ok(());
        }
        last = redact_secrets(&pane);
        std::thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("timed out waiting for `{needle}` in tmux pane\n{last}");
}

fn wait_for_file_text(path: &Path, needle: &str, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        match fs::read_to_string(path) {
            Ok(text) if text.contains(needle) => return Ok(text),
            Ok(text) => last = text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!(
        "{} did not contain `{}` before timeout. Last content:\n{}",
        path.display(),
        needle,
        last
    );
}

fn read_toml(path: PathBuf) -> toml::Value {
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))
        .unwrap();
    toml::from_str(&raw)
        .with_context(|| format!("parse {}", path.display()))
        .unwrap()
}

fn read_json(path: PathBuf) -> Value {
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))
        .unwrap();
    serde_json::from_str(&raw)
        .with_context(|| format!("parse {}", path.display()))
        .unwrap()
}
