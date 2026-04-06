use puffer_test_support::{start_tmux_command, tmux_available, wait_for_tmux_text};
use std::path::PathBuf;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn puffer_help_renders_in_tmux_no_alt_screen() {
    if !tmux_available() {
        return;
    }

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let home = tempdir().unwrap();
    let session = start_tmux_command(
        "sh",
        &[
            "-lc",
            &format!(
                "HOME='{}' {} --no-alt-screen /help; sleep 10",
                home.path().display(),
                env!("CARGO_BIN_EXE_puffer")
            ),
        ],
        Some(&workspace_root),
    )
    .unwrap();
    let capture =
        wait_for_tmux_text(&session, "Supported commands", Duration::from_secs(15)).unwrap();
    assert!(capture.contains("/review"));
    assert!(capture.contains("Supported commands"));
}
