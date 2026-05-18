//! Integration coverage for the `/recap` slash command.
//!
//! These tests drive the public dispatcher (`dispatch_command`) the same
//! way the TUI does, so the registry registration + handler routing +
//! emit_system fallback paths are all exercised end-to-end. The actual
//! model side-turn is not invoked in unit tests — the recap module's own
//! unit tests cover the prompt and skip-check logic, and the
//! `recap_enabled_without_provider_surfaces_error` test below confirms
//! the handler degrades cleanly when there's no provider to call.

use super::*;
use crate::MessageRole;
use puffer_session_store::SessionMetadata;

#[test]
fn recap_appears_in_supported_commands_as_local() {
    let commands = supported_commands();
    let recap = find_command(&commands, "recap").expect("/recap is registered");
    assert_eq!(recap.kind, CommandKind::Local);
    assert!(
        recap.description.to_lowercase().contains("summarize"),
        "expected description to mention 'summarize'; got {}",
        recap.description
    );
}

#[test]
fn recap_disabled_emits_explanatory_system_message() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    puffer_config::ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();

    let mut config = PufferConfig::default();
    config.recap.enabled = false;
    let mut state = AppState::new(config, tempdir.path().to_path_buf(), session);

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/recap",
    )
    .unwrap();

    let last = state.transcript.last().expect("system message pushed");
    assert!(matches!(last.role, MessageRole::System));
    assert!(
        last.text.to_lowercase().contains("disabled"),
        "expected 'disabled' in message; got {}",
        last.text
    );
    // The recap module's RECAP_DISPLAY_PREFIX must NOT appear — we
    // short-circuited before run_recap_side_turn.
    assert!(!last.text.starts_with("\u{203B} recap: "));
}

#[test]
fn recap_enabled_without_provider_surfaces_error_via_emit_system() {
    // recap.enabled defaults to true. There are no providers in
    // ProviderRegistry::new(), so the side-turn will fail at
    // resolve_provider_and_model. We assert the handler catches the
    // error and emits a "Recap failed: ..." system line instead of
    // bubbling it up — that's the contract slash command callers rely
    // on.
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    puffer_config::ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/recap",
    )
    .expect("dispatch must succeed even when side-turn errors internally");

    let last = state.transcript.last().expect("system message pushed");
    assert!(matches!(last.role, MessageRole::System));
    assert!(
        last.text.to_lowercase().contains("failed")
            || last.text.to_lowercase().contains("error")
            || last.text.to_lowercase().contains("no"),
        "expected the system message to surface a failure reason; got {}",
        last.text
    );
}

#[test]
fn recap_auto_disabled_still_runs_slash_command() {
    // auto=false should NOT block the manual slash command. Verify by
    // dispatching /recap with auto=false and confirming it gets past
    // the gate (lands in the side-turn error path rather than the
    // "disabled" early-return).
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    puffer_config::ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();

    let mut config = PufferConfig::default();
    config.recap.enabled = true;
    config.recap.auto = false;
    let mut state = AppState::new(config, tempdir.path().to_path_buf(), session);

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/recap",
    )
    .unwrap();

    let last = state.transcript.last().expect("system message pushed");
    // We must NOT have hit the "Recap is disabled" early-return. The
    // message is some side-turn error/note instead.
    assert!(
        !last.text.to_lowercase().contains("disabled"),
        "auto=false must not block manual /recap; got {}",
        last.text
    );
}

#[allow(dead_code)]
fn dummy_session_metadata() -> SessionMetadata {
    SessionMetadata {
        id: uuid::Uuid::nil(),
        display_name: None,
        generated_title: None,
        cwd: PathBuf::from("/tmp"),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    }
}
