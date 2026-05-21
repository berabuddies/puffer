use super::*;
use crate::approval_overlay::ApprovalOverlay;
use crate::permission_prompt_flow::handle_permission_prompt_key;
use crate::state::{PendingPermissionRequest, PendingSubmit, PendingSubmitEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
            transcript_persisted_len: 0,
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
fn permission_prompt_response_preserves_composer_draft() {
    let request = PermissionPromptRequest {
        tool_id: "Bash".to_string(),
        summary: "cat <<'EOF'".to_string(),
        reason: Some("pasted shell input requires approval".to_string()),
    };
    let (response_tx, response_rx) = mpsc::channel();
    let draft = "next message [Pasted text #1 +2 lines]".to_string();
    let pending_pastes = vec![(
        "[Pasted text #1 +2 lines]".to_string(),
        "first pasted line\nsecond pasted line".to_string(),
    )];
    let mut tui = TuiState {
        input: draft.clone(),
        cursor: draft.len(),
        slash_selection: 2,
        overlay: Some(OverlayState::PermissionPrompt {
            overlay: ApprovalOverlay::new(request),
        }),
        pending_permission_request: Some(PendingPermissionRequest { response_tx }),
        pending_pastes: pending_pastes.clone(),
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
    assert_eq!(tui.input, draft);
    assert_eq!(tui.cursor, tui.input.len());
    assert_eq!(tui.pending_pastes, pending_pastes);
    assert_eq!(tui.slash_selection, 0);
}

#[test]
fn permission_prompt_ctrl_c_denies_and_closes_overlay() {
    let request = PermissionPromptRequest {
        tool_id: "Bash".to_string(),
        summary: "git push origin master".to_string(),
        reason: Some("shell command matches sandbox exclusion `git push`".to_string()),
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
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &mut tui
    ));
    assert_eq!(
        response_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        PermissionPromptAction::Deny
    );
    assert!(tui.overlay.is_none());
    assert!(tui.pending_permission_request.is_none());
}

#[test]
fn permission_prompt_ctrl_c_interrupts_pending_turn() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    let commands = supported_commands();
    let request = PermissionPromptRequest {
        tool_id: "Bash".to_string(),
        summary: "git push origin master".to_string(),
        reason: Some("shell command matches sandbox exclusion `git push`".to_string()),
    };
    let (_event_tx, event_rx) = mpsc::channel();
    let (response_tx, response_rx) = mpsc::channel();
    let cancel = puffer_core::CancelToken::new();
    let cancel_handle = cancel.clone();
    let mut tui = TuiState {
        pending_submit: Some(PendingSubmit {
            prompt: "hi".to_string(),
            receiver: event_rx,
            transcript_persisted_len: 0,
            rendered_tool_invocations: 0,
            pending_tool_calls: Vec::new(),
            started_at: std::time::Instant::now(),
            thinking_active: false,
            status_hint: None,
            cancel,
        }),
        overlay: Some(OverlayState::PermissionPrompt {
            overlay: ApprovalOverlay::new(request),
        }),
        pending_permission_request: Some(PendingPermissionRequest { response_tx }),
        ..TuiState::default()
    };

    handle_key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
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

    assert!(cancel_handle.is_cancelled());
    assert!(!tui.has_pending_submit());
    assert!(tui.overlay.is_none());
    assert!(tui.pending_permission_request.is_none());
    assert!(response_rx.try_recv().is_err());
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::System && message.text == "Interrupted by user."
    }));
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
