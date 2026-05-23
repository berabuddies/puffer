//! Drives the real `puffer` TUI under tmux and verifies the hot-reload
//! pipeline picks up filesystem changes mid-session.
//!
//! Why tmux: the resource watcher is wired into `puffer_tui::run_app`,
//! which requires a tty. The tmux test harness already used by
//! `tmux_smoke.rs` / `tmux_snapshots.rs` gives us a real pty so the TUI
//! runs the same code path as a human user.
//!
//! What this proves end-to-end: a long-running puffer process picks up
//! a brand-new workspace skill from disk between turns without any
//! `/reload-plugins` invocation. That covers all three feature layers —
//! the filesystem watcher, the cross-thread reload signal on
//! `AppState`, and the resource reload that reaches `LoadedResources`.

use puffer_test_support::{
    capture_tmux_visible_pane, require_tmux_or_skip, send_tmux_keys, start_tmux_command_with_size,
    temp_workspace, wait_for_tmux_text, wait_for_tmux_visible_text, TerminalSize,
};
use std::fs;
use std::time::Duration;

#[test]
fn tmux_tui_hot_reloads_newly_dropped_skill() {
    if !require_tmux_or_skip("tmux_tui_hot_reloads_newly_dropped_skill") {
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

    let binary = env!("CARGO_BIN_EXE_puffer");
    let session = start_tmux_command_with_size(
        "sh",
        &[
            "-lc",
            &format!("HOME='{}' '{}'", workspace.display(), binary),
        ],
        Some(workspace.as_path()),
        TerminalSize {
            rows: 40,
            cols: 140,
        },
    )
    .unwrap();

    // Wait until the TUI is fully up.
    wait_for_tmux_text(&session, "Puffer Code", Duration::from_secs(15)).unwrap();

    // Sanity: the test skill doesn't exist yet — confirm via /skills.
    submit_skills_command(&session);
    // The /skills overlay surface uses different rendering depending on
    // the build. Give it a beat to render and capture.
    std::thread::sleep(Duration::from_millis(500));
    let before = capture_tmux_visible_pane(&session).unwrap();
    assert!(
        !before.contains("hot-tmux-skill"),
        "baseline /skills should not show test skill, got:\n{before}"
    );

    // Drop the new skill on disk — this is the moment the user's editor
    // would have saved a new SKILL.md in another shell.
    let skill_dir = workspace.join(".puffer/resources/skills/hot-tmux-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: hot-tmux-skill\ndescription: Dropped by tmux integration test\n---\nBody\n",
    )
    .unwrap();

    // Close the overlay so the next /skills invocation re-reads
    // resources after the watcher has had a chance to fire and the
    // main loop has consumed the signal.
    send_tmux_keys(&session, &["Escape"]).unwrap();
    std::thread::sleep(Duration::from_millis(300));

    // Drive a key into the TUI so the main loop ticks and processes
    // the reload signal that the watcher raised. The "i" -> backspace
    // sequence is harmless: it just nudges the loop.
    send_tmux_keys(&session, &["i", "BSpace"]).unwrap();
    wait_for_tmux_text(&session, "Reloaded plugin registry", Duration::from_secs(5)).unwrap();

    // Wait for the reload to have happened, then verify /skills sees it.
    // We try a few times because filesystem-watcher latency + main-loop
    // poll interval add up to a hundred milliseconds or two on macOS.
    let mut found = false;
    for _ in 0..30 {
        submit_skills_command(&session);
        std::thread::sleep(Duration::from_millis(300));
        let after = capture_tmux_visible_pane(&session).unwrap();
        if after.contains("hot-tmux-skill") {
            found = true;
            break;
        }
        // Close the overlay before the next attempt.
        send_tmux_keys(&session, &["Escape"]).unwrap();
        std::thread::sleep(Duration::from_millis(150));
    }
    let final_capture = capture_tmux_visible_pane(&session).unwrap();
    assert!(
        found,
        "hot-reloaded skill should appear in /skills overlay within a few seconds, final capture:\n{final_capture}"
    );
}

fn submit_skills_command(session: &puffer_test_support::TmuxSession) {
    send_tmux_keys(session, &["/skills"]).unwrap();
    wait_for_tmux_visible_text(session, "\u{276f} /skills", Duration::from_secs(15)).unwrap();
    send_tmux_keys(session, &["Enter"]).unwrap();
}
