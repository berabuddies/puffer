use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_session_store::SessionStore;
use puffer_test_support::{
    require_tmux_or_skip, send_tmux_keys, start_tmux_command, temp_workspace, wait_for_tmux_text,
    wait_for_tmux_visible_text,
};
use std::fs;
use std::time::Duration;

const TMUX_WAIT_TIMEOUT: Duration = Duration::from_secs(15);

#[test]
fn tmux_resume_overlay_lists_workspace_sessions() {
    if !require_tmux_or_skip("tmux_resume_overlay_lists_workspace_sessions") {
        return;
    }

    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let paths = ConfigPaths {
        workspace_root: workspace.clone(),
        workspace_config_dir: workspace.join(".puffer"),
        user_config_dir: puffer_home.join(".puffer"),
        builtin_resources_dir: workspace.join("resources"),
    };
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    fs::create_dir_all(workspace.join("dockyard")).unwrap();
    let session = session_store
        .create_session(workspace.join("dockyard"))
        .unwrap();
    session_store
        .rename_session(session.id, "dockyard".to_string())
        .unwrap();

    let session = start_tmux_command(
        "sh",
        &[
            "-lc",
            &format!(
                "PUFFER_HOME='{}' HOME='{}' '{}'",
                puffer_home.display(),
                workspace.display(),
                env!("CARGO_BIN_EXE_puffer")
            ),
        ],
        Some(workspace.as_path()),
    )
    .unwrap();
    wait_for_tmux_text(&session, "Puffer Code", TMUX_WAIT_TIMEOUT).unwrap();
    send_tmux_keys(&session, &["/resu"]).unwrap();
    wait_for_tmux_visible_text(
        &session,
        "Resume a previous conversation",
        TMUX_WAIT_TIMEOUT,
    )
    .unwrap();
    send_tmux_keys(&session, &["Enter"]).unwrap();
    wait_for_tmux_visible_text(&session, "\u{276f} /resume", TMUX_WAIT_TIMEOUT).unwrap();
    send_tmux_keys(&session, &["Enter"]).unwrap();
    wait_for_tmux_text(&session, "dockyard", TMUX_WAIT_TIMEOUT).unwrap();
}

#[test]
fn tmux_login_overlay_lists_available_providers() {
    if !require_tmux_or_skip("tmux_login_overlay_lists_available_providers") {
        return;
    }

    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let session = start_tmux_command(
        "sh",
        &[
            "-lc",
            &format!(
                "PUFFER_HOME='{}' HOME='{}' '{}'",
                puffer_home.display(),
                workspace.display(),
                env!("CARGO_BIN_EXE_puffer")
            ),
        ],
        Some(workspace.as_path()),
    )
    .unwrap();
    wait_for_tmux_text(&session, "Puffer Code", TMUX_WAIT_TIMEOUT).unwrap();
    send_tmux_keys(&session, &["/lo"]).unwrap();
    wait_for_tmux_visible_text(&session, "Sign in to a provider", TMUX_WAIT_TIMEOUT).unwrap();
    send_tmux_keys(&session, &["Enter"]).unwrap();
    wait_for_tmux_visible_text(&session, "\u{276f} /login", TMUX_WAIT_TIMEOUT).unwrap();
    send_tmux_keys(&session, &["Enter"]).unwrap();
    let capture = wait_for_tmux_text(&session, "OpenAI", TMUX_WAIT_TIMEOUT).unwrap();
    assert!(capture.contains("anthropic"));
}

fn configured_workspace() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (tempdir, workspace) = temp_workspace().unwrap();
    let puffer_home = workspace.clone();
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
    (tempdir, workspace, puffer_home)
}
