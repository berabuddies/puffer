use super::*;
use crate::state::{LoopKind, LoopState, LoopStatus, PendingSubmit};
use std::sync::mpsc;

fn open_panel(command: &str) -> OverlayState {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let mut state = sample_state();
    state.cwd = tempdir.path().to_path_buf();
    state.session.cwd = tempdir.path().to_path_buf();
    let resources = sample_resources();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &resources,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        command,
    )
    .unwrap();
    assert!(opened);
    tui.overlay.expect("panel overlay")
}

fn set_pending_turn(tui: &mut TuiState) {
    let (_sender, receiver) = mpsc::channel();
    tui.pending_submit = Some(PendingSubmit {
        prompt: "first".to_string(),
        receiver,
        transcript_persisted_len: 0,
        pending_tool_calls: Vec::new(),
        rendered_tool_invocations: 0,
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: None,
        cancel: puffer_core::CancelToken::new(),
    });
}

fn rewind_test_state(tempdir: &tempfile::TempDir, session_store: &SessionStore) -> AppState {
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    state.push_message(MessageRole::User, "first");
    state.push_message(MessageRole::Assistant, "reply");
    state.push_message(MessageRole::User, "second");
    state
}

fn open_command_picker_panel(command: &str) -> (String, Vec<ModelPickerEntry>, usize) {
    match open_panel(command) {
        OverlayState::CommandPicker {
            title,
            entries,
            selection,
        } => (title, entries, selection),
        other => panic!("expected command picker for {command}, got {other:?}"),
    }
}

fn picker_entry<'a>(entries: &'a [ModelPickerEntry], selector: &str) -> &'a ModelPickerEntry {
    entries
        .iter()
        .find(|entry| entry.selector == selector)
        .unwrap_or_else(|| panic!("missing picker entry {selector}"))
}

#[test]
fn try_open_overlay_builds_config_panel() {
    assert!(matches!(open_panel("/config"), OverlayState::Text(..)));
    assert!(matches!(open_panel("/settings"), OverlayState::Text(..)));
}

#[test]
fn try_open_overlay_builds_context_panel() {
    assert!(matches!(open_panel("/context"), OverlayState::Text(..)));
}

#[test]
fn try_open_overlay_builds_fast_mode_picker() {
    match open_panel("/fast") {
        OverlayState::FastModePicker {
            provider_id,
            model_id,
            effort,
            selection,
            entries,
            ..
        } => {
            assert_eq!(provider_id, "anthropic");
            assert_eq!(model_id, "claude-sonnet-4-5");
            assert_eq!(effort, "high");
            assert_eq!(entries[selection].selector, "on");
        }
        other => panic!("expected fast-mode picker, got {other:?}"),
    }
}

#[test]
fn try_open_overlay_builds_permissions_panel() {
    assert!(matches!(open_panel("/permissions"), OverlayState::Text(..)));
}

#[test]
fn try_open_overlay_builds_skills_panel() {
    assert!(matches!(open_panel("/skills"), OverlayState::Text(..)));
}

#[test]
fn try_open_overlay_builds_hooks_panel() {
    let (title, entries, selection) = open_command_picker_panel("/hooks");

    assert_eq!(title, "Hooks");
    assert_eq!(selection, 0);
    assert_eq!(
        picker_entry(&entries, "/hooks path").description,
        "Show hook resource paths and supported events"
    );

    let open = picker_entry(&entries, "/hooks open");
    assert!(open.description.contains("workspace hooks directory"));
    assert!(open.description.contains("resources/hooks"));
}

#[test]
fn try_open_overlay_builds_mcp_panel() {
    assert!(matches!(
        open_panel("/mcp"),
        OverlayState::CommandPicker { .. }
    ));
}

#[test]
fn try_open_overlay_builds_plugin_picker_with_management_actions() {
    let (title, entries, selection) = open_command_picker_panel("/plugin");

    assert_eq!(title, "Plugins");
    assert_eq!(selection, 0);
    assert!(entries.len() >= 10);

    let open = picker_entry(&entries, "/plugin open");
    assert!(open.description.contains("workspace plugin manifest"));
    assert!(open.description.contains("workspace.yaml"));

    assert_eq!(
        picker_entry(&entries, "/reload-plugins").description,
        "Reload plugin changes from disk for this session"
    );
    assert_eq!(
        picker_entry(&entries, "/plugin errors").description,
        "Show plugin-specific resource diagnostics"
    );
    assert_eq!(
        picker_entry(&entries, "/plugin validate").description,
        "Validate loaded plugin manifests or one manifest path"
    );
}

#[test]
fn try_open_overlay_builds_plugin_picker_with_plugin_specific_actions() {
    let (_, entries, _) = open_command_picker_panel("/plugin");

    let disable = picker_entry(&entries, "/plugin disable git");
    assert!(disable.description.contains("git (Git)"));
    assert!(disable.description.contains("[enabled] builtin"));
    assert!(disable.description.contains("commands=1"));
    assert!(disable.description.contains("skills=1"));
    assert!(disable.description.contains("mcp_servers=1"));

    let open = picker_entry(&entries, "/plugin open git");
    assert!(open.description.contains("plugins/git.yaml"));

    assert_eq!(
        picker_entry(&entries, "/plugin validate git").description,
        "Validate plugin git"
    );
}

#[test]
fn try_open_overlay_builds_plugin_alias_pickers() {
    for command in ["/plugins", "/marketplace"] {
        let (title, entries, selection) = open_command_picker_panel(command);
        assert_eq!(title, "Plugins");
        assert_eq!(selection, 0);
        assert!(entries.iter().any(|entry| entry.selector == "/plugin open"));
        assert!(entries
            .iter()
            .any(|entry| entry.selector == "/reload-plugins"));
        assert!(entries
            .iter()
            .any(|entry| entry.selector == "/plugin disable git"));
        assert!(entries
            .iter()
            .any(|entry| entry.selector == "/plugin validate git"));
    }
}

#[test]
fn try_open_overlay_builds_ide_picker_with_manifest_actions() {
    let (title, entries, selection) = open_command_picker_panel("/ide");

    assert_eq!(title, "IDE");
    assert_eq!(selection, 0);
    assert_eq!(
        picker_entry(&entries, "/ide path").description,
        "Show IDE resource paths"
    );

    let open = picker_entry(&entries, "/ide open");
    assert!(open.description.contains("workspace IDE manifest"));
    assert!(open.description.contains("workspace.yaml"));

    let show = picker_entry(&entries, "/ide show vscode");
    assert!(show.description.contains("VS Code"));
    assert!(show.description.contains("builtin"));

    let open_named = picker_entry(&entries, "/ide open vscode");
    assert!(open_named.description.contains("ides/vscode.yaml"));
}

#[test]
fn try_open_overlay_builds_sandbox_picker_with_toggle_actions() {
    let (title, entries, selection) = open_command_picker_panel("/sandbox");

    assert_eq!(title, "Sandbox");
    assert_eq!(selection, 0);
    assert_eq!(
        picker_entry(&entries, "/sandbox workspace-write").description,
        "Sandbox mode: workspace-write (current)"
    );
    assert_eq!(
        picker_entry(&entries, "/sandbox read-only").description,
        "Switch sandbox mode to read-only"
    );
    assert_eq!(
        picker_entry(&entries, "/sandbox auto-allow true").description,
        "Auto-allow tool prompts: off"
    );
    assert_eq!(
        picker_entry(&entries, "/sandbox allow-unsandboxed true").description,
        "Allow unsandboxed Bash fallback: off"
    );
    assert!(picker_entry(&entries, "/sandbox open")
        .description
        .contains("sandbox.toml"));
}

#[test]
fn try_open_overlay_builds_memory_panel() {
    assert!(matches!(
        open_panel("/memory"),
        OverlayState::CommandPicker { .. }
    ));
}

#[test]
fn try_open_overlay_builds_copy_picker_for_code_blocks() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let mut state = sample_state();
    state.cwd = tempdir.path().to_path_buf();
    state.session.cwd = tempdir.path().to_path_buf();
    state.transcript.clear();
    state.push_message(
        MessageRole::Assistant,
        "```rs\nfn main() {}\n```\n```json\n{\"ok\":true}\n```",
    );
    let resources = sample_resources();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &resources,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/copy",
    )
    .unwrap();

    assert!(opened);
    match tui.overlay {
        Some(OverlayState::CommandPicker {
            title,
            entries,
            selection,
        }) => {
            assert_eq!(title, "Copy");
            assert_eq!(selection, 0);
            assert_eq!(entries[0].selector, "Full response");
            assert_eq!(entries[0].command.as_deref(), Some("/copy --full 0"));
            assert_eq!(entries[1].selector, "fn main() {}");
            assert_eq!(entries[1].command.as_deref(), Some("/copy --code 0 0"));
        }
        other => panic!("expected copy picker, got {other:?}"),
    }
}

#[test]
fn try_open_overlay_skips_copy_picker_when_preference_is_enabled() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let mut state = sample_state();
    state.cwd = tempdir.path().to_path_buf();
    state.session.cwd = tempdir.path().to_path_buf();
    state.config.copy_full_response = true;
    state.transcript.clear();
    state.push_message(MessageRole::Assistant, "```rs\nfn main() {}\n```");
    let resources = sample_resources();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &resources,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/copy",
    )
    .unwrap();

    assert!(!opened);
    assert!(tui.overlay.is_none());
}

#[test]
fn try_open_overlay_builds_tasks_panel() {
    let (title, entries, selection) = open_command_picker_panel("/tasks");
    assert_eq!(title, "Background Tasks");
    assert_eq!(selection, 0);
    assert_eq!(entries[0].selector, "dashboard");
    assert_eq!(entries[0].command.as_deref(), Some("/tasks show"));
    assert!(entries.iter().all(|entry| {
        entry
            .command
            .as_deref()
            .is_some_and(|command| !command.starts_with("/tasks output "))
    }));
    let (title, _, _) = open_command_picker_panel("/bashes");
    assert_eq!(title, "Background Tasks");
}

#[test]
fn try_open_overlay_builds_task_dashboard_panel() {
    assert!(matches!(open_panel("/tasks show"), OverlayState::Text(..)));
}

#[test]
fn pending_turn_opens_tasks_overlay_instead_of_queueing() {
    for command in ["/tasks", "/bashes"] {
        let tempdir = tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let mut state = sample_state();
        state.cwd = tempdir.path().to_path_buf();
        state.session.cwd = tempdir.path().to_path_buf();
        let mut resources = sample_resources();
        let mut providers = sample_providers();
        let mut auth_store = sample_auth_store();
        let auth_path = paths.user_config_dir.join("auth.json");
        let commands = supported_commands();
        let mut tui = TuiState::default();
        set_pending_turn(&mut tui);
        tui.input = command.to_string();
        tui.cursor = tui.input.len();

        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut resources,
            &mut providers,
            &mut auth_store,
            &auth_path,
            &session_store,
            &commands,
            &mut tui,
            true,
        )
        .unwrap();

        assert!(matches!(
            tui.overlay,
            Some(OverlayState::CommandPicker {
                ref title,
                ..
            }) if title == "Background Tasks"
        ));
        assert!(tui.queued_prompts.is_empty());
    }
}

#[test]
fn pending_turn_queues_task_stop_instead_of_opening_overlay() {
    for command in ["/tasks stop task-1", "/bashes stop task-1"] {
        let tempdir = tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let mut state = sample_state();
        state.cwd = tempdir.path().to_path_buf();
        state.session.cwd = tempdir.path().to_path_buf();
        let mut resources = sample_resources();
        let mut providers = sample_providers();
        let mut auth_store = sample_auth_store();
        let auth_path = paths.user_config_dir.join("auth.json");
        let commands = supported_commands();
        let mut tui = TuiState::default();
        set_pending_turn(&mut tui);
        tui.input = command.to_string();
        tui.cursor = tui.input.len();

        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut resources,
            &mut providers,
            &mut auth_store,
            &auth_path,
            &session_store,
            &commands,
            &mut tui,
            true,
        )
        .unwrap();

        assert!(tui.overlay.is_none());
        assert_eq!(
            tui.queued_prompts.front().map(String::as_str),
            Some(command)
        );
    }
}

#[test]
fn pending_turn_opens_config_aliases_instead_of_queueing() {
    for command in ["/config", "/settings"] {
        let tempdir = tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let mut state = sample_state();
        state.cwd = tempdir.path().to_path_buf();
        state.session.cwd = tempdir.path().to_path_buf();
        let mut resources = sample_resources();
        let mut providers = sample_providers();
        let mut auth_store = sample_auth_store();
        let auth_path = paths.user_config_dir.join("auth.json");
        let commands = supported_commands();
        let mut tui = TuiState::default();
        set_pending_turn(&mut tui);
        tui.input = command.to_string();
        tui.cursor = tui.input.len();

        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut resources,
            &mut providers,
            &mut auth_store,
            &auth_path,
            &session_store,
            &commands,
            &mut tui,
            true,
        )
        .unwrap();

        assert!(matches!(tui.overlay, Some(OverlayState::Text(..))));
        assert!(tui.queued_prompts.is_empty());
    }
}

#[test]
fn pending_turn_opens_cost_panel_instead_of_mutating_transcript() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let mut state = sample_state();
    state.cwd = tempdir.path().to_path_buf();
    state.session.cwd = tempdir.path().to_path_buf();
    state.push_message(MessageRole::User, "tell me about the code");
    state.push_message(MessageRole::Assistant, "streaming partial");
    let transcript_len = state.transcript.len();
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    let auth_path = paths.user_config_dir.join("auth.json");
    let commands = supported_commands();
    let mut tui = TuiState::default();
    set_pending_turn(&mut tui);
    tui.input = "/cost".to_string();
    tui.cursor = tui.input.len();

    handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &commands,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(matches!(tui.overlay, Some(OverlayState::Text(..))));
    assert!(tui.queued_prompts.is_empty());
    assert_eq!(state.transcript.len(), transcript_len);
    assert_eq!(
        state.transcript.last().map(|message| message.text.as_str()),
        Some("streaming partial")
    );
}

#[test]
fn pending_turn_queues_diff_instead_of_mutating_transcript() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let mut state = sample_state();
    state.cwd = tempdir.path().to_path_buf();
    state.session.cwd = tempdir.path().to_path_buf();
    state.push_message(MessageRole::User, "update the branch");
    state.push_message(MessageRole::Assistant, "streaming partial");
    let transcript_len = state.transcript.len();
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    let auth_path = paths.user_config_dir.join("auth.json");
    let commands = supported_commands();
    let mut tui = TuiState::default();
    set_pending_turn(&mut tui);
    tui.input = "/diff".to_string();
    tui.cursor = tui.input.len();

    handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &commands,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(tui.overlay.is_none());
    assert_eq!(tui.queued_prompts.front().map(String::as_str), Some("/diff"));
    assert_eq!(state.transcript.len(), transcript_len);
    assert_eq!(
        state.transcript.last().map(|message| message.text.as_str()),
        Some("streaming partial")
    );
}

#[test]
fn try_open_overlay_builds_session_summary_panel() {
    let overlay = open_panel("/session");
    assert!(matches!(overlay, OverlayState::Text(..)));

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    render::set_active_overlay(Some(overlay));
    terminal
        .draw(|frame| {
            render::render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &supported_commands(),
            )
        })
        .unwrap();
    render::set_active_overlay(None);
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains("session_id="));
    assert!(rendered.contains("cwd="));
    assert!(!rendered.contains("Not in remote mode"));
}

#[test]
fn try_open_overlay_builds_remote_session_overlay() {
    assert!(matches!(open_panel("/remote"), OverlayState::Session(..)));
}

#[test]
fn try_open_overlay_builds_rewind_picker() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let mut state = sample_state();
    state.cwd = tempdir.path().to_path_buf();
    state.session.cwd = tempdir.path().to_path_buf();
    state.push_message(MessageRole::User, "first");
    state.push_message(MessageRole::Assistant, "reply");
    state.push_message(MessageRole::User, "second");
    let resources = sample_resources();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    for command in ["/rewind", "/checkpoint"] {
        let mut tui = TuiState::default();
        let opened = try_open_overlay(
            &state,
            &resources,
            &mut providers,
            &auth_store,
            &session_store,
            &mut tui,
            command,
        )
        .unwrap();

        assert!(opened);
        assert!(matches!(
            tui.overlay,
            Some(OverlayState::CommandPicker { .. })
        ));
    }
}

#[test]
fn pending_turn_queues_rewind_instead_of_opening_picker() {
    for command in ["/rewind", "/checkpoint"] {
        let tempdir = tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let mut state = rewind_test_state(&tempdir, &session_store);
        let mut resources = sample_resources();
        let mut providers = sample_providers();
        let mut auth_store = sample_auth_store();
        let auth_path = paths.user_config_dir.join("auth.json");
        let commands = supported_commands();
        let mut tui = TuiState::default();
        set_pending_turn(&mut tui);
        tui.input = command.to_string();
        tui.cursor = tui.input.len();

        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut resources,
            &mut providers,
            &mut auth_store,
            &auth_path,
            &session_store,
            &commands,
            &mut tui,
            true,
        )
        .unwrap();

        assert!(tui.overlay.is_none());
        assert_eq!(
            tui.queued_prompts.front().map(String::as_str),
            Some(command)
        );
        assert_eq!(state.transcript.len(), 3);
    }
}

#[test]
fn pending_turn_queues_rewind_picker_selection_instead_of_rewinding() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let mut state = rewind_test_state(&tempdir, &session_store);
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    let auth_path = paths.user_config_dir.join("auth.json");
    let commands = supported_commands();
    let mut tui = TuiState::default();

    assert!(try_open_overlay(
        &state,
        &resources,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/rewind",
    )
    .unwrap());
    set_pending_turn(&mut tui);

    handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &commands,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(tui.overlay.is_none());
    assert_eq!(
        tui.queued_prompts.front().map(String::as_str),
        Some("/rewind")
    );
    assert_eq!(state.transcript.len(), 3);
}

#[test]
fn render_shows_loop_status_box_when_active() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    render::set_active_loop_state(Some(LoopState {
        kind: LoopKind::Maximize("accuracy".to_string()),
        prompt: "improve tests".to_string(),
        iteration: 3,
        max_iterations: 50,
        interval: None,
        next_fire: None,
        target_history: vec![0.72, 0.85, 0.91],
        status: LoopStatus::Running,
    }));
    terminal
        .draw(|frame| {
            render::set_active_overlay(None);
            render::set_pending_submit_state(None, vec![], vec![], None, false, None);
            render::set_tool_details_expanded(false);
            render::set_follow_output(true);
            render::render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &supported_commands(),
            )
        })
        .unwrap();
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains("Optimize"), "should show Optimize title");
    assert!(rendered.contains("accuracy"), "should show metric name");
    assert!(rendered.contains("iter 3/50"), "should show iteration");
    assert!(rendered.contains("Running"), "should show running status");

    // Clean up thread-local state.
    render::set_active_loop_state(None);
}
