use super::flow_loop::*;
use super::*;
use crate::state::LoopKind;
use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_session_store::SessionMetadata;
use tempfile::tempdir;

fn sample_state(session: SessionMetadata, cwd: &Path) -> AppState {
    AppState::new(PufferConfig::default(), cwd.to_path_buf(), session)
}

#[test]
fn provider_prompt_detection_matches_interactive_surface() {
    assert!(is_provider_prompt_input("henlo"));
    assert!(is_provider_prompt_input(" review this diff "));
    assert!(!is_provider_prompt_input(""));
    assert!(!is_provider_prompt_input("/help"));
    assert!(!is_provider_prompt_input("!pwd"));
    assert!(!is_provider_prompt_input("login openai"));
    assert!(!is_provider_prompt_input("/logout"));
}

#[test]
fn handle_prompt_submit_starts_async_provider_turn_and_polls_result() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "henlo".to_string(),
        true,
    )
    .unwrap();

    assert!(tui.has_pending_submit());
    assert!(matches!(state.transcript.first(), Some(message) if message.text == "henlo"));

    let mut completed = false;
    for _ in 0..20 {
        if poll_pending_submit(
            &mut state,
            &mut auth_store,
            &auth_path,
            &session_store,
            &mut tui,
        )
        .unwrap()
        {
            completed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(completed);
    assert!(!tui.has_pending_submit());
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::System && message.text.starts_with("Provider request failed:")
    }));
}

#[test]
fn handle_prompt_submit_queues_prompt_while_turn_is_running() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "first".to_string(),
        true,
    )
    .unwrap();
    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "second".to_string(),
        true,
    )
    .unwrap();

    assert!(tui.has_pending_submit());
    assert_eq!(tui.queued_prompts.len(), 1);
    assert_eq!(
        tui.queued_prompts.front().map(String::as_str),
        Some("second")
    );
    assert!(matches!(state.transcript.first(), Some(message) if message.text == "first"));
}

#[test]
fn slash_plan_with_arguments_starts_async_turn_after_local_handling() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "/plan stabilize slash-command parity".to_string(),
        true,
    )
    .unwrap();
    submit_queued_prompt_if_ready(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(state.plan_mode);
    assert!(tui.has_pending_submit());
    assert!(state
        .transcript
        .iter()
        .any(|message| message.text == "Enabled plan mode"));
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::User && message.text == "stabilize slash-command parity"
    }));
}

#[test]
fn cancel_pending_submit_records_interrupt_and_starts_next_queued_prompt() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "first".to_string(),
        true,
    )
    .unwrap();
    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "second".to_string(),
        true,
    )
    .unwrap();

    assert!(cancel_pending_submit(&mut state, &session_store, &mut tui).unwrap());
    assert!(!tui.has_pending_submit());
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::System && message.text == "Interrupted by user."
    }));

    assert!(submit_next_queued_prompt(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap());
    assert!(tui.has_pending_submit());
    assert!(tui.queued_prompts.is_empty());
    assert!(state
        .transcript
        .iter()
        .any(|message| { message.role == MessageRole::User && message.text == "second" }));
}

// ---------------------------------------------------------------------------
// Loop / Maximize / Minimize tests
// ---------------------------------------------------------------------------

#[test]
fn parse_loop_args_extracts_interval_and_prompt() {
    let (d, p) = parse_loop_args("5m check deploy");
    assert_eq!(d, std::time::Duration::from_secs(300));
    assert_eq!(p, "check deploy");

    let (d, p) = parse_loop_args("30s ping server");
    assert_eq!(d, std::time::Duration::from_secs(30));
    assert_eq!(p, "ping server");

    let (d, p) = parse_loop_args("2h run maintenance");
    assert_eq!(d, std::time::Duration::from_secs(7200));
    assert_eq!(p, "run maintenance");

    // No interval → defaults to 10 minutes, whole input is prompt.
    let (d, p) = parse_loop_args("check deploy");
    assert_eq!(d, std::time::Duration::from_secs(600));
    assert_eq!(p, "check deploy");
}

#[test]
fn parse_duration_handles_all_suffixes() {
    assert_eq!(
        parse_duration("10s"),
        Some(std::time::Duration::from_secs(10))
    );
    assert_eq!(
        parse_duration("5m"),
        Some(std::time::Duration::from_secs(300))
    );
    assert_eq!(
        parse_duration("1h"),
        Some(std::time::Duration::from_secs(3600))
    );
    assert_eq!(
        parse_duration("2d"),
        Some(std::time::Duration::from_secs(172800))
    );
    assert_eq!(parse_duration("abc"), None);
    assert_eq!(parse_duration(""), None);
}

#[test]
fn extract_metric_value_parses_marker() {
    let text = "I improved the test suite.\n[[METRIC:accuracy=0.85]]\nDone.";
    assert_eq!(extract_metric_value(text, "accuracy"), Some(0.85));

    let text = "[[METRIC:latency = 12.5 ]]";
    assert_eq!(extract_metric_value(text, "latency"), Some(12.5));

    let text = "No metric here";
    assert_eq!(extract_metric_value(text, "accuracy"), None);
}

#[test]
fn has_converged_detects_plateau() {
    assert!(!has_converged(&[]));
    assert!(!has_converged(&[1.0, 2.0]));
    assert!(!has_converged(&[1.0, 2.0, 3.0]));
    assert!(has_converged(&[3.0, 3.0, 3.0]));
    assert!(has_converged(&[1.0, 2.0, 3.0, 3.0, 3.0]));
}

#[test]
fn build_optimization_prompt_includes_context() {
    let prompt = build_optimization_prompt("fix tests", "accuracy", true, 3, &[0.5, 0.7]);
    assert!(prompt.contains("maximize"));
    assert!(prompt.contains("accuracy"));
    assert!(prompt.contains("iteration 3/"));
    assert!(prompt.contains("0.5000"));
    assert!(prompt.contains("0.7000"));
    assert!(prompt.contains("fix tests"));
    assert!(prompt.contains("[[METRIC:accuracy="));
    // User prompt should come before optimization context
    let user_pos = prompt.find("fix tests").unwrap();
    let ctx_pos = prompt.find("Optimization context").unwrap();
    assert!(
        user_pos < ctx_pos,
        "user prompt should precede optimization context"
    );
}

#[test]
fn try_handle_loop_command_creates_loop_state() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut tui = TuiState::default();

    // Non-loop command returns false.
    assert!(!try_handle_loop_command(&mut state, &session_store, &mut tui, "/help").unwrap());

    // Loop command creates state and enqueues prompt.
    assert!(
        try_handle_loop_command(&mut state, &session_store, &mut tui, "/loop 10s echo hi").unwrap()
    );
    assert!(tui.active_loop.is_some());
    let ls = tui.active_loop.as_ref().unwrap();
    assert!(matches!(ls.kind, LoopKind::Loop));
    assert_eq!(ls.prompt, "echo hi");
    assert_eq!(ls.interval, Some(std::time::Duration::from_secs(10)));
    assert_eq!(tui.queued_prompts.len(), 1);
}

#[test]
fn try_handle_loop_command_creates_maximize_state() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut tui = TuiState::default();

    assert!(try_handle_loop_command(
        &mut state,
        &session_store,
        &mut tui,
        "/maximize accuracy run bench"
    )
    .unwrap());
    assert!(tui.active_loop.is_some());
    let ls = tui.active_loop.as_ref().unwrap();
    assert!(matches!(ls.kind, LoopKind::Maximize(ref m) if m == "accuracy"));
    assert_eq!(ls.prompt, "run bench");
    assert_eq!(tui.queued_prompts.len(), 1);
    let enqueued = &tui.queued_prompts[0];
    assert!(enqueued.contains("maximize"));
    assert!(enqueued.contains("[[METRIC:accuracy="));
}

#[test]
fn stop_command_clears_active_loop() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut tui = TuiState::default();

    try_handle_loop_command(&mut state, &session_store, &mut tui, "/loop 10s echo hi").unwrap();
    assert!(tui.active_loop.is_some());

    try_handle_loop_command(&mut state, &session_store, &mut tui, "/loop stop").unwrap();
    assert!(tui.active_loop.is_none());
}
