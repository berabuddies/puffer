use super::*;

#[test]
fn try_open_overlay_builds_status_overlay() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let state = sample_state();
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
        "/status",
    )
    .unwrap();

    assert!(opened);
    assert!(matches!(tui.overlay, Some(OverlayState::Status(..))));
}

#[test]
fn try_open_overlay_builds_usage_overlay() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let state = sample_state();
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
        "/usage",
    )
    .unwrap();

    assert!(opened);
    assert!(matches!(tui.overlay, Some(OverlayState::Usage(..))));
}

#[test]
fn usage_retry_key_is_handled_by_usage_overlay() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let auth_path = paths.user_config_dir.join("auth.json");

    let mut state = sample_state();
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    let mut tui = TuiState {
        overlay: Some(OverlayState::Usage(
            crate::usage::UsageOverlay::error_for_test("network down"),
        )),
        ..TuiState::default()
    };

    handle_overlay_key(
        KeyEvent::from(KeyCode::Char('r')),
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

    assert!(tui.input.is_empty());
    assert!(matches!(tui.overlay, Some(OverlayState::Usage(..))));
}

#[test]
fn usage_overlay_arrow_keys_scroll_usage_body() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let auth_path = paths.user_config_dir.join("auth.json");

    let mut state = sample_state();
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    let usage = crate::usage::UsageOverlay::unavailable_for_test();
    let mut tui = TuiState {
        overlay: Some(OverlayState::Usage(usage.clone())),
        ..TuiState::default()
    };

    handle_overlay_key(
        KeyEvent::from(KeyCode::Down),
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

    assert_eq!(usage.scroll_for_test(), 1);
}

#[test]
fn read_only_status_overlay_ignores_printable_typing() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let auth_path = paths.user_config_dir.join("auth.json");

    let mut state = sample_state();
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    let mut tui = TuiState {
        overlay: Some(crate::status_overlay::StatusOverlay::open(
            &state,
            &resources,
            &providers,
            &auth_store,
        )),
        ..TuiState::default()
    };

    handle_overlay_key(
        KeyEvent::from(KeyCode::Char('x')),
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

    assert!(tui.input.is_empty());
    assert!(matches!(tui.overlay, Some(OverlayState::Status(..))));

    handle_overlay_key(
        KeyEvent::from(KeyCode::Char('/')),
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

    assert_eq!(tui.input, "/");
    assert!(tui.overlay.is_none());
}
