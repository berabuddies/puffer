use puffer_test_support::{
    send_tmux_keys, start_tmux_command, temp_workspace, tmux_available, wait_for_tmux_text,
};
use std::fs;
use std::time::Duration;

#[test]
fn tmux_smoke_renders_help_output() {
    if !tmux_available() {
        return;
    }

    let (_tempdir, workspace) = temp_workspace().unwrap();
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

    let binary = env!("CARGO_BIN_EXE_puffer");
    let session = start_tmux_command(
        "sh",
        &[
            "-lc",
            &format!("HOME='{}' '{}'", workspace.display(), binary),
        ],
        Some(workspace.as_path()),
    )
    .unwrap();
    wait_for_tmux_text(&session, "Puffer Code", Duration::from_secs(15)).unwrap();
    send_tmux_keys(&session, &["/help", "Enter"]).unwrap();
    let capture =
        wait_for_tmux_text(&session, "Supported commands:", Duration::from_secs(15)).unwrap();
    assert!(capture.contains("/add-dir"));
    assert!(capture.contains("Supported commands:"));
}
