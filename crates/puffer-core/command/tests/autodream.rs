use super::*;
use std::fs;

fn autodream_fixture() -> (
    tempfile::TempDir,
    puffer_config::PufferHomeOverride,
    SessionStore,
    AppState,
) {
    let tempdir = tempdir().unwrap();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    let home_override = ScopedPufferHome::set(&home)._override;
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let state = AppState::new(PufferConfig::default(), workspace, session);
    (tempdir, home_override, session_store, state)
}

fn dispatch_autodream(state: &mut AppState, session_store: &SessionStore, args: &str) {
    let command = if args.is_empty() {
        "/autodream".to_string()
    } else {
        format!("/autodream {args}")
    };
    dispatch_command(
        state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        session_store,
        &command,
    )
    .unwrap();
}

#[test]
fn autodream_help_does_not_run_review() {
    let _lock = lock_puffer_home();
    let (_tempdir, _home, session_store, mut state) = autodream_fixture();

    dispatch_autodream(&mut state, &session_store, "help");

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("Usage: /autodream [on|off|status|suggestions|help]"));
    assert!(!text.contains("AutoDream complete."));
}

#[test]
fn autodream_unknown_arg_returns_usage_without_running_review() {
    let _lock = lock_puffer_home();
    let (_tempdir, _home, session_store, mut state) = autodream_fixture();

    dispatch_autodream(&mut state, &session_store, "verbose");

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("Unknown AutoDream command: verbose"));
    assert!(text.contains("Usage: /autodream [on|off|status|suggestions|help]"));
    assert!(!text.contains("AutoDream complete."));
}

#[test]
fn autodream_status_aliases_report_scheduler_without_running_review() {
    let _lock = lock_puffer_home();
    let (_tempdir, _home, session_store, mut state) = autodream_fixture();

    for arg in ["status", "show", "check"] {
        dispatch_autodream(&mut state, &session_store, arg);
        let text = &state.transcript.last().unwrap().text;
        assert!(text.contains("enabled="));
        assert!(text.contains("project_memory="));
        assert!(!text.contains("AutoDream complete."));
    }
}

#[test]
fn autodream_suggestion_aliases_report_empty_queue() {
    let _lock = lock_puffer_home();
    let (_tempdir, _home, session_store, mut state) = autodream_fixture();

    for arg in ["suggestions", "queue", "genskill"] {
        dispatch_autodream(&mut state, &session_store, arg);
        assert_eq!(
            state.transcript.last().unwrap().text,
            "AutoDream GenSkill suggestions: none"
        );
    }
}

#[test]
fn autodream_off_persists_user_config() {
    let _lock = lock_puffer_home();
    let (_tempdir, _home, session_store, mut state) = autodream_fixture();

    dispatch_autodream(&mut state, &session_store, "off");

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("AutoDream is off."));
    assert!(!state.config.memory.autodream_enabled);
    let paths = ConfigPaths::discover(&state.cwd);
    let raw = fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(raw.contains("autodream_enabled = false"));
}

#[test]
fn autodream_on_persists_user_config() {
    let _lock = lock_puffer_home();
    let (_tempdir, _home, session_store, mut state) = autodream_fixture();
    state.config.memory.autodream_enabled = false;

    dispatch_autodream(&mut state, &session_store, "on");

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("AutoDream is on."));
    assert!(state.config.memory.autodream_enabled);
    let paths = ConfigPaths::discover(&state.cwd);
    let raw = fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(raw.contains("autodream_enabled = true"));
}

#[test]
fn config_command_can_toggle_autodream_enabled() {
    let _lock = lock_puffer_home();
    let (_tempdir, _home, session_store, mut state) = autodream_fixture();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/config set autodreamEnabled false",
    )
    .unwrap();

    assert!(!state.config.memory.autodream_enabled);
    let paths = ConfigPaths::discover(&state.cwd);
    let raw = fs::read_to_string(paths.user_config_file()).unwrap();
    assert!(raw.contains("autodream_enabled = false"));
}
