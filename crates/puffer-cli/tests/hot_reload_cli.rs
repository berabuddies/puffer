//! CLI-level integration tests for the hot-reload feature.
//!
//! Drives the real `puffer` binary against a tempdir workspace. The CLI
//! reloads resources from disk on every invocation (it doesn't share the
//! long-lived TUI's watcher), so these tests cover the precondition the
//! hot-reload pipeline depends on: that freshly-written manifests on the
//! correct disk paths are picked up by the resource loader.
//!
//! End-to-end coverage of the watcher → signal → reload pipeline lives in
//! `crates/puffer-core/tests/hot_reload.rs` as in-process tests; this file
//! is the binary-level sanity check.

use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn mcp_list_reflects_dropped_workspace_manifest() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();

    // Baseline — no `cli-hotload` manifest yet.
    let before = run_puffer(&workspace, &puffer_home, &["mcp", "list"]);
    assert!(
        !before.contains("cli-hotload"),
        "baseline should not include test manifest, got:\n{before}"
    );

    // Drop a workspace-layer MCP manifest the way the user would.
    let mcp_dir = workspace.join(".puffer/resources/mcp_servers");
    fs::create_dir_all(&mcp_dir).expect("mkdir mcp_servers");
    fs::write(
        mcp_dir.join("cli-hotload.yaml"),
        "id: cli-hotload\n\
display_name: CLI Hot Loaded\n\
transport: stdio\n\
endpoint: \"\"\n\
target: /bin/echo\n\
description: integration test\n\
enabled: true\n",
    )
    .expect("write manifest");

    let after_add = run_puffer(&workspace, &puffer_home, &["mcp", "list"]);
    assert!(
        after_add.contains("cli-hotload"),
        "manifest dropped on disk should appear in `puffer mcp list`, got:\n{after_add}"
    );

    fs::remove_file(mcp_dir.join("cli-hotload.yaml")).expect("rm manifest");
    let after_remove = run_puffer(&workspace, &puffer_home, &["mcp", "list"]);
    assert!(
        !after_remove.contains("cli-hotload"),
        "removed manifest should disappear from `puffer mcp list`, got:\n{after_remove}"
    );
}

#[test]
fn doctor_skill_count_grows_after_dropping_workspace_skill() {
    // `puffer doctor` reports `skills=<N>`. Dropping a fresh skill file
    // under `.puffer/resources/skills/<name>/SKILL.md` should bump the
    // count by exactly one, proving the static loader picks up new
    // skill resources at the workspace layer (the watcher uses the same
    // loader at hot-reload time).
    let (_tempdir, workspace, puffer_home) = configured_workspace();

    let before = run_puffer(&workspace, &puffer_home, &["doctor"]);
    let before_count = extract_skill_count(&before)
        .unwrap_or_else(|| panic!("expected skills=N in doctor output, got:\n{before}"));

    let skill_dir = workspace.join(".puffer/resources/skills/cli-skill-fresh");
    fs::create_dir_all(&skill_dir).expect("mkdir skill");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: cli-skill-fresh\ndescription: Hot-dropped via CLI test\n---\nBody\n",
    )
    .expect("write skill");

    let after = run_puffer(&workspace, &puffer_home, &["doctor"]);
    let after_count = extract_skill_count(&after)
        .unwrap_or_else(|| panic!("expected skills=N in doctor output, got:\n{after}"));
    assert_eq!(
        after_count,
        before_count + 1,
        "skill count should grow by 1 after dropping a workspace skill on disk\nbefore: {before_count}\nafter: {after_count}\noutput: {after}"
    );

    // Removing the skill should bring the count back down.
    fs::remove_dir_all(&skill_dir).expect("rm skill");
    let after_remove = run_puffer(&workspace, &puffer_home, &["doctor"]);
    let after_remove_count = extract_skill_count(&after_remove).expect("skill count");
    assert_eq!(
        after_remove_count, before_count,
        "skill count should fall back to baseline after deletion"
    );
}

fn extract_skill_count(doctor_output: &str) -> Option<usize> {
    doctor_output
        .lines()
        .find_map(|line| line.trim().strip_prefix("skills="))
        .and_then(|n| n.trim().parse().ok())
}

#[test]
fn agents_list_reflects_dropped_workspace_agent_manifest() {
    // Same precondition check for the `agents` resource type — proves
    // that any of the 11 watched resource subdirectories pick up new
    // files via the static loader (the hot-reload watcher uses the same
    // load path).
    let (_tempdir, workspace, puffer_home) = configured_workspace();

    let agents_dir = workspace.join(".puffer/resources/agents");
    fs::create_dir_all(&agents_dir).expect("mkdir agents");
    fs::write(
        agents_dir.join("cli-fresh-agent.yaml"),
        "id: cli-fresh-agent\n\
description: Hot-dropped agent from CLI test\n\
prompt: You are a hot-dropped agent.\n",
    )
    .expect("write agent");

    let output = run_puffer(&workspace, &puffer_home, &["agents"]);
    assert!(
        output.contains("cli-fresh-agent"),
        "newly-dropped agent manifest should appear in `puffer agents`, got:\n{output}"
    );
}

fn configured_workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let workspace = tempdir.path().join("workspace");
    let puffer_home = tempdir.path().join("puffer-home");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(&puffer_home).expect("puffer-home");
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).expect("dirs");
    // Symlink the repo `resources/` so the loader still finds bundled
    // tools/agents/prompts during the test — the embedded copy is
    // loaded too, but mirroring the production layout keeps the test
    // exercising the same code paths real users hit.
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate parent")
        .parent()
        .expect("repo root");
    let _ = std::os::unix::fs::symlink(repo_root.join("resources"), workspace.join("resources"));
    (tempdir, workspace, puffer_home)
}

fn run_puffer(workspace: &Path, puffer_home: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_puffer"))
        .args(args)
        .current_dir(workspace)
        .env("PUFFER_HOME", puffer_home)
        .output()
        .expect("puffer process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}{stderr}")
}
