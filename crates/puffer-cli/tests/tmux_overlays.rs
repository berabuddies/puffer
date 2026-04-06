use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_session_store::SessionStore;
use puffer_test_support::{
    capture_tmux_pane, send_tmux_keys, start_tmux_command, temp_workspace, tmux_available,
    wait_for_tmux_text,
};
use std::fs;
use std::time::Duration;

#[test]
fn tmux_resume_overlay_lists_workspace_sessions() {
    if !tmux_available() {
        return;
    }

    let (_tempdir, workspace) = configured_workspace();
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
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
                "HOME='{}' '{}'",
                workspace.display(),
                env!("CARGO_BIN_EXE_puffer")
            ),
        ],
        Some(workspace.as_path()),
    )
    .unwrap();
    wait_for_tmux_text(&session, "Puffer Code", Duration::from_secs(10)).unwrap();
    send_tmux_keys(&session, &["/resume", "Enter"]).unwrap();
    let capture = wait_for_tmux_text(&session, "Resume Session", Duration::from_secs(10)).unwrap();
    assert!(capture.contains("dockyard"));
}

#[test]
fn tmux_login_overlay_lists_available_providers() {
    if !tmux_available() {
        return;
    }

    let (_tempdir, workspace) = configured_workspace();
    let session = start_tmux_command(
        "sh",
        &[
            "-lc",
            &format!(
                "HOME='{}' '{}'",
                workspace.display(),
                env!("CARGO_BIN_EXE_puffer")
            ),
        ],
        Some(workspace.as_path()),
    )
    .unwrap();
    wait_for_tmux_text(&session, "Puffer Code", Duration::from_secs(10)).unwrap();
    send_tmux_keys(&session, &["/login", "Enter"]).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let capture = capture_tmux_pane(&session).unwrap();
    assert!(capture.contains("anthropic"));
    assert!(capture.contains("openai"));
}

fn configured_workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let (tempdir, workspace) = temp_workspace().unwrap();
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
    (tempdir, workspace)
}
