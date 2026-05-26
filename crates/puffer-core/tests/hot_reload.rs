//! End-to-end coverage of skill and MCP hot-reload.
//!
//! Verifies that:
//! - A new skill file dropped on disk shows up in `LoadedResources.skills`
//!   after [`puffer_core::reload_runtime_resources`].
//! - A removed skill file disappears after the same reload.
//! - A new MCP manifest is added to the live `LocalToolRunner`'s MCP host
//!   and an existing one is removed when its manifest is deleted.
//! - The cross-thread reload signal raised by [`puffer_core::ResourceWatcher`]
//!   participates in `AppState::take_reload_request()`.
//!
//! Skill content reloads are covered by writing the SKILL.md frontmatter
//! `description` field and verifying the new value reaches
//! `LoadedResources.skills` after reload.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_core::runner_adapter::LocalToolRunner;
use puffer_core::{reload_runtime_resources, AppState, ResourceWatcher};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::{load_resources, skill_by_name};
use puffer_runner_api::ToolRunner;
use puffer_session_store::SessionMetadata;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

/// Builds a workspace rooted at `temp`. Returns the workspace `cwd` plus the
/// resolved `ConfigPaths` so individual tests can lay out resources under
/// the matching layer (workspace `.puffer/resources/...`).
fn make_workspace() -> (tempfile::TempDir, ConfigPaths) {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    // Build ConfigPaths with disjoint roots so this test never touches the
    // developer's real ~/.puffer or the in-repo `resources/`.
    let paths = ConfigPaths {
        workspace_root: workspace.clone(),
        workspace_config_dir: workspace.join(".puffer"),
        user_config_dir: temp.path().join(".home/.puffer"),
        builtin_resources_dir: temp.path().join("builtin-resources"),
    };
    ensure_workspace_dirs(&paths).unwrap();
    std::fs::create_dir_all(&paths.builtin_resources_dir).unwrap();
    (temp, paths)
}

/// Builds an `AppState` rooted at the workspace with the local tool runner
/// already configured for the resource layout. Returns the state plus the
/// initial `LoadedResources`. Mirrors production by syncing the MCP roster
/// from the loaded resources into the runner so the runner starts with the
/// same view as `puffer_runner_local::local_runner_from_resources`.
fn make_state_with_resources(
    paths: &ConfigPaths,
    workspace_cwd: PathBuf,
) -> (AppState, puffer_resources::LoadedResources) {
    let runner = LocalToolRunner::new();
    let initial =
        load_resources(paths, &runner).expect("initial load_resources should succeed for empty fs");
    let mut servers: Vec<puffer_resources::McpServerSpec> = initial
        .mcp_servers
        .iter()
        .map(|item| item.value.clone())
        .collect();
    for (_plugin, spec) in puffer_resources::plugin_mcp_servers(&initial) {
        if servers
            .iter()
            .any(|existing| existing.id.eq_ignore_ascii_case(&spec.id))
        {
            continue;
        }
        servers.push(spec.clone());
    }
    let runner = runner.with_mcp_servers(servers);
    let session = SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        generated_title: None,
        cwd: workspace_cwd.clone(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let state = AppState::new(PufferConfig::default(), workspace_cwd, session)
        .with_tool_runner(Arc::new(runner));
    (state, initial)
}

#[test]
fn reload_picks_up_newly_added_skill() {
    let (_temp, paths) = make_workspace();
    let (state, mut resources) = make_state_with_resources(&paths, paths.workspace_root.clone());
    assert!(
        skill_by_name(&resources, "review-helper").is_none(),
        "skill should not exist before it is written"
    );

    // Drop a workspace-layer skill file. The resource loader watches the
    // `<workspace>/.puffer/resources/skills/<name>/SKILL.md` layout.
    let skill_dir = paths
        .workspace_config_dir
        .join("resources/skills/review-helper");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review-helper\ndescription: Hot-added skill\n---\nBody\n",
    )
    .unwrap();

    let mut providers = ProviderRegistry::new();
    let auth_store = AuthStore::default();
    let _ = reload_runtime_resources(&state, &mut resources, &mut providers, &auth_store)
        .expect("reload after adding a skill should succeed");

    let found = skill_by_name(&resources, "review-helper")
        .expect("skill should be visible after hot-reload");
    assert_eq!(found.value.description, "Hot-added skill");
}

#[test]
fn reload_drops_removed_skill() {
    let (_temp, paths) = make_workspace();
    // Seed the skill on disk before constructing state so it appears in
    // the initial load.
    let skill_dir = paths
        .workspace_config_dir
        .join("resources/skills/old-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: old-skill\ndescription: Will be deleted\n---\nBody\n",
    )
    .unwrap();

    let (state, mut resources) = make_state_with_resources(&paths, paths.workspace_root.clone());
    assert!(skill_by_name(&resources, "old-skill").is_some());

    // Delete the skill directory and force a reload.
    std::fs::remove_dir_all(&skill_dir).unwrap();
    let mut providers = ProviderRegistry::new();
    let auth_store = AuthStore::default();
    let _ = reload_runtime_resources(&state, &mut resources, &mut providers, &auth_store)
        .expect("reload after deleting a skill should succeed");

    assert!(
        skill_by_name(&resources, "old-skill").is_none(),
        "skill should be gone from the loaded inventory after hot-reload"
    );
}

#[test]
fn reload_picks_up_modified_skill_description() {
    let (_temp, paths) = make_workspace();
    let skill_dir = paths.workspace_config_dir.join("resources/skills/reviewer");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: reviewer\ndescription: Original description\n---\nBody\n",
    )
    .unwrap();

    let (state, mut resources) = make_state_with_resources(&paths, paths.workspace_root.clone());
    assert_eq!(
        skill_by_name(&resources, "reviewer")
            .unwrap()
            .value
            .description,
        "Original description"
    );

    // Edit the skill file in place.
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: reviewer\ndescription: Updated description\n---\nUpdated body\n",
    )
    .unwrap();

    let mut providers = ProviderRegistry::new();
    let auth_store = AuthStore::default();
    let _ = reload_runtime_resources(&state, &mut resources, &mut providers, &auth_store)
        .expect("reload after editing a skill should succeed");

    let skill = skill_by_name(&resources, "reviewer").unwrap();
    assert_eq!(skill.value.description, "Updated description");
    assert!(skill.value.content.contains("Updated body"));
}

#[test]
fn reload_adds_mcp_server_to_local_runner() {
    let (_temp, paths) = make_workspace();
    let (state, mut resources) = make_state_with_resources(&paths, paths.workspace_root.clone());

    // Initially the runner's MCP host should hold no manifest entries.
    let initial = state.tool_runner.list_mcp_servers().expect("list before");
    assert!(
        !initial.iter().any(|s| s.id == "hotload"),
        "the hot-load fixture should not exist yet, got {initial:?}"
    );

    // Drop a workspace-layer manifest and force a reload.
    let mcp_dir = paths.workspace_config_dir.join("resources/mcp_servers");
    std::fs::create_dir_all(&mcp_dir).unwrap();
    std::fs::write(
        mcp_dir.join("hotload.yaml"),
        "id: hotload\n\
display_name: Hot-loaded MCP\n\
transport: stdio\n\
endpoint: \"\"\n\
target: /nonexistent/binary\n\
description: Verifies hot-reload of MCP roster\n\
enabled: true\n",
    )
    .unwrap();

    let mut providers = ProviderRegistry::new();
    let auth_store = AuthStore::default();
    let _ = reload_runtime_resources(&state, &mut resources, &mut providers, &auth_store)
        .expect("reload after adding an MCP manifest should succeed");

    let after = state.tool_runner.list_mcp_servers().expect("list after");
    assert!(
        after.iter().any(|s| s.id == "hotload"),
        "the hot-loaded MCP server should be visible on the live runner, got {after:?}"
    );
    assert!(
        resources
            .mcp_servers
            .iter()
            .any(|s| s.value.id == "hotload"),
        "resources.mcp_servers should also reflect the addition"
    );
}

#[test]
fn reload_removes_mcp_server_from_local_runner() {
    let (_temp, paths) = make_workspace();
    let mcp_dir = paths.workspace_config_dir.join("resources/mcp_servers");
    std::fs::create_dir_all(&mcp_dir).unwrap();
    std::fs::write(
        mcp_dir.join("doomed.yaml"),
        "id: doomed\n\
display_name: Doomed MCP\n\
transport: stdio\n\
endpoint: \"\"\n\
target: /nonexistent/binary\n\
description: Will be deleted\n\
enabled: true\n",
    )
    .unwrap();

    let (state, mut resources) = make_state_with_resources(&paths, paths.workspace_root.clone());
    let initial = state.tool_runner.list_mcp_servers().expect("list before");
    assert!(initial.iter().any(|s| s.id == "doomed"));

    std::fs::remove_file(mcp_dir.join("doomed.yaml")).unwrap();
    let mut providers = ProviderRegistry::new();
    let auth_store = AuthStore::default();
    let _ = reload_runtime_resources(&state, &mut resources, &mut providers, &auth_store)
        .expect("reload after deleting an MCP manifest should succeed");

    let after = state.tool_runner.list_mcp_servers().expect("list after");
    assert!(
        !after.iter().any(|s| s.id == "doomed"),
        "deleted MCP manifest should be removed from the live runner roster"
    );
}

#[test]
fn watcher_signal_participates_in_take_reload_request() {
    let (_temp, paths) = make_workspace();
    let (mut state, _resources) = make_state_with_resources(&paths, paths.workspace_root.clone());
    assert!(!state.take_reload_request());

    // Manually flip the cross-thread signal as if a watcher event arrived.
    state.reload_signal().store(true, Ordering::Release);
    assert!(state.take_reload_request());
    assert!(
        !state.take_reload_request(),
        "take_reload_request must clear the signal after observing it"
    );
}

#[test]
fn watcher_raises_signal_for_files_under_workspace_resources() {
    let (_temp, paths) = make_workspace();
    let (state, _resources) = make_state_with_resources(&paths, paths.workspace_root.clone());

    // Use the production helper so the watcher hooks the same roots as
    // it does in run_app.
    let watcher = ResourceWatcher::start(&paths, state.reload_signal()).unwrap();
    assert!(!watcher.watched_roots().is_empty());

    // Write a skill under the workspace layer; the watcher should flip
    // the signal within a couple of seconds.
    let skill_dir = paths
        .workspace_config_dir
        .join("resources/skills/picked-up");
    std::fs::create_dir_all(&skill_dir).unwrap();
    // Give notify a moment after attachment before generating events.
    std::thread::sleep(Duration::from_millis(150));
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: picked-up\ndescription: Watcher should see this\n---\nBody\n",
    )
    .unwrap();

    let raised = wait_for(&state.reload_signal(), Duration::from_secs(3));
    assert!(raised, "watcher should raise the reload signal");
}

#[test]
fn replace_mcp_roster_preserves_oauth_token_dir() {
    // Regression: an early version of `replace_mcp_roster` built the
    // replacement via `McpHost::with_elicitation(...)` and silently
    // forwarded `None` for `oauth_token_dir`, so any prior
    // `with_oauth_token_dir(custom)` pin was dropped on the first
    // hot-reload — every subsequent OAuth op then fell back to the
    // default `<config>/puffer/mcp-tokens` location.
    let temp_token_dir = tempfile::tempdir().unwrap();
    let runner = LocalToolRunner::new().with_oauth_token_dir(temp_token_dir.path().to_path_buf());
    let host = runner.mcp_host();
    assert_eq!(host.oauth_token_dir(), Some(temp_token_dir.path()));
    drop(host);

    runner.replace_mcp_roster(Vec::new(), None);

    let host_after = runner.mcp_host();
    assert_eq!(
        host_after.oauth_token_dir(),
        Some(temp_token_dir.path()),
        "hot-swap must preserve the custom OAuth token dir set via with_oauth_token_dir"
    );
}

#[test]
fn replace_mcp_roster_drops_old_entries() {
    // Direct test of `LocalToolRunner::replace_mcp_roster` independent of
    // the resource loader: confirms the hot-swap surface exposed to
    // `reload_runtime_resources` is correct.
    let mut servers = Vec::new();
    servers.push(puffer_resources::McpServerSpec {
        id: "alpha".into(),
        display_name: "Alpha".into(),
        transport: "stdio".into(),
        endpoint: String::new(),
        target: "/bin/nonexistent-alpha".into(),
        description: String::new(),
        env: Default::default(),
        inherit_env: true,
        timeout: None,
        connect_timeout: None,
        headers: Default::default(),
        oauth: None,
    });
    let runner = LocalToolRunner::new().with_mcp_servers(servers.clone());
    let before = runner.list_mcp_servers().unwrap();
    assert!(before.iter().any(|s| s.id == "alpha"));

    // Swap the roster wholesale.
    let new_servers = vec![puffer_resources::McpServerSpec {
        id: "beta".into(),
        display_name: "Beta".into(),
        transport: "stdio".into(),
        endpoint: String::new(),
        target: "/bin/nonexistent-beta".into(),
        description: String::new(),
        env: Default::default(),
        inherit_env: true,
        timeout: None,
        connect_timeout: None,
        headers: Default::default(),
        oauth: None,
    }];
    runner.replace_mcp_roster(new_servers, None);
    let after = runner.list_mcp_servers().unwrap();
    assert!(!after.iter().any(|s| s.id == "alpha"));
    assert!(after.iter().any(|s| s.id == "beta"));
}

/// Polls the atomic signal in 25 ms chunks up to `timeout`.
fn wait_for(signal: &Arc<AtomicBool>, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if signal.load(Ordering::Acquire) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    signal.load(Ordering::Acquire)
}

#[test]
fn end_to_end_watcher_drives_reload_and_runner_picks_up_new_mcp_server() {
    // Full-pipeline integration: spin up `ResourceWatcher` against the
    // workspace and user resource roots, drop an MCP server manifest on
    // disk, wait for the watcher to flip the cross-thread signal, then
    // run the same reload path the TUI uses. The live local tool
    // runner must end up holding the new manifest.
    let (_temp, paths) = make_workspace();
    let (mut state, mut resources) =
        make_state_with_resources(&paths, paths.workspace_root.clone());

    // No watcher yet — the runner should not see the manifest we'll
    // drop in a moment.
    let initial = state.tool_runner.list_mcp_servers().expect("initial list");
    assert!(!initial.iter().any(|s| s.id == "e2e-hotload"));

    // Wire up the watcher exactly like `puffer-tui::run_app` does.
    let _watcher =
        ResourceWatcher::start(&paths, state.reload_signal()).expect("watcher should start");

    // Give notify a moment to attach before we generate events on
    // platforms where attach is asynchronous.
    std::thread::sleep(Duration::from_millis(150));

    // Drop a new MCP manifest at the workspace layer.
    let mcp_dir = paths.workspace_config_dir.join("resources/mcp_servers");
    std::fs::create_dir_all(&mcp_dir).unwrap();
    std::fs::write(
        mcp_dir.join("e2e-hotload.yaml"),
        "id: e2e-hotload\n\
display_name: End-to-End Hot-Loaded\n\
transport: stdio\n\
endpoint: \"\"\n\
target: /bin/echo\n\
description: integration test\n\
enabled: true\n",
    )
    .unwrap();

    // Watcher should raise the signal within a few seconds.
    assert!(
        wait_for(&state.reload_signal(), Duration::from_secs(3)),
        "watcher should raise the reload signal after dropping the manifest"
    );
    assert!(state.take_reload_request());

    // Now run the same reload the TUI runs in `maybe_apply_requested_reload`.
    let mut providers = ProviderRegistry::new();
    let auth_store = AuthStore::default();
    let _ =
        puffer_core::reload_runtime_resources(&state, &mut resources, &mut providers, &auth_store)
            .expect("reload should succeed");

    let after = state.tool_runner.list_mcp_servers().expect("after list");
    assert!(
        after.iter().any(|s| s.id == "e2e-hotload"),
        "the hot-loaded MCP server must be visible on the live runner after reload, got {after:?}"
    );
    assert!(
        resources
            .mcp_servers
            .iter()
            .any(|s| s.value.id == "e2e-hotload"),
        "resources.mcp_servers should also reflect the addition"
    );
}

#[test]
fn end_to_end_watcher_drives_reload_for_skill_removal() {
    // Full-pipeline integration for skill removal: seed a skill on
    // disk, start the watcher, delete the file, wait for the signal,
    // reload, verify the skill is gone from `LoadedResources`.
    let (_temp, paths) = make_workspace();

    let skill_dir = paths
        .workspace_config_dir
        .join("resources/skills/e2e-doomed");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_file = skill_dir.join("SKILL.md");
    std::fs::write(
        &skill_file,
        "---\nname: e2e-doomed\ndescription: integration\n---\nBody\n",
    )
    .unwrap();

    let (mut state, mut resources) =
        make_state_with_resources(&paths, paths.workspace_root.clone());
    assert!(skill_by_name(&resources, "e2e-doomed").is_some());

    let _watcher =
        ResourceWatcher::start(&paths, state.reload_signal()).expect("watcher should start");
    std::thread::sleep(Duration::from_millis(150));

    std::fs::remove_file(&skill_file).unwrap();
    // Also drop the now-empty directory so the next loader sweep
    // observes no candidate `SKILL.md` for this skill.
    let _ = std::fs::remove_dir(&skill_dir);

    assert!(
        wait_for(&state.reload_signal(), Duration::from_secs(3)),
        "watcher should raise the signal after deleting the skill"
    );
    assert!(state.take_reload_request());

    let mut providers = ProviderRegistry::new();
    let auth_store = AuthStore::default();
    let _ =
        puffer_core::reload_runtime_resources(&state, &mut resources, &mut providers, &auth_store)
            .expect("reload should succeed");

    assert!(
        skill_by_name(&resources, "e2e-doomed").is_none(),
        "deleted skill should be gone from resources after watcher-driven reload"
    );
}
