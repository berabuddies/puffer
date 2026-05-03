use super::*;
use crate::approval_overlay::ApprovalOverlay;
use crate::permission_prompt_flow::handle_permission_prompt_key;
use crate::state::{PendingPermissionRequest, PendingSubmit, PendingSubmitEvent};
use crossterm::event::{KeyCode, KeyEvent};
use puffer_core::{PermissionPromptAction, PermissionPromptRequest};
use ratatui::backend::TestBackend;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn poll_pending_submit_opens_permission_prompt_overlay() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let auth_path = paths.user_config_dir.join("auth.json");

    let request = PermissionPromptRequest {
        tool_id: "Bash".to_string(),
        summary: "git push origin master".to_string(),
        reason: Some("shell command matches sandbox exclusion `git push`".to_string()),
    };
    let (event_tx, event_rx) = mpsc::channel();
    let (response_tx, _response_rx) = mpsc::channel();
    event_tx
        .send(PendingSubmitEvent::PermissionRequest(
            request.clone(),
            response_tx,
        ))
        .unwrap();

    let mut tui = TuiState {
        pending_submit: Some(PendingSubmit {
            prompt: "hi".to_string(),
            receiver: event_rx,
            rendered_tool_invocations: 0,
            pending_tool_calls: Vec::new(),
            started_at: std::time::Instant::now(),
            thinking_active: false,
            status_hint: None,
            cancel: puffer_core::CancelToken::new(),
        }),
        ..TuiState::default()
    };
    let mut state = sample_state();
    let mut auth_store = sample_auth_store();

    let completed = poll_pending_submit(
        &mut state,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
    )
    .unwrap();

    assert!(!completed);
    assert!(tui.pending_permission_request.is_some());
    assert_eq!(
        tui.overlay,
        Some(OverlayState::PermissionPrompt {
            overlay: ApprovalOverlay::new(request),
        })
    );
}

#[test]
fn permission_prompt_shortcuts_send_response() {
    let request = PermissionPromptRequest {
        tool_id: "Config".to_string(),
        summary: "Set theme to \"dark\"".to_string(),
        reason: Some("config writes require approval".to_string()),
    };
    let (response_tx, response_rx) = mpsc::channel();
    let mut tui = TuiState {
        overlay: Some(OverlayState::PermissionPrompt {
            overlay: ApprovalOverlay::new(request),
        }),
        pending_permission_request: Some(PendingPermissionRequest { response_tx }),
        ..TuiState::default()
    };

    assert!(handle_permission_prompt_key(
        KeyEvent::from(KeyCode::Char('a')),
        &mut tui
    ));
    assert_eq!(
        response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        PermissionPromptAction::AllowSession
    );
    assert!(tui.overlay.is_none());
    assert!(tui.pending_permission_request.is_none());
}

#[test]
fn render_permission_prompt_shows_codex_style_options() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    let overlay = OverlayState::PermissionPrompt {
        overlay: ApprovalOverlay::new(PermissionPromptRequest {
            tool_id: "Bash".to_string(),
            summary: "git push origin master".to_string(),
            reason: Some("shell command matches sandbox exclusion `git push`".to_string()),
        }),
    };

    terminal
        .draw(|frame| {
            render::set_active_overlay(Some(overlay.clone()));
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
            );
            render::set_active_overlay(None);
        })
        .unwrap();
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains("Would you like to grant these permissions?"));
    assert!(rendered.contains("Yes, grant these permissions"));
    assert!(rendered.contains("Yes, grant these permissions for this session"));
    assert!(rendered.contains("Yes, allow ALL tools for this session"));
    assert!(rendered.contains("No, continue without permissions"));
}
