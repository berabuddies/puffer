use super::*;
use crate::state::{PendingSubmit, PendingSubmitEvent, PendingUserQuestionRequest};
use crate::user_question_flow::handle_user_question_key;
use crate::user_question_overlay::UserQuestionOverlay;
use crossterm::event::{KeyCode, KeyEvent};
use puffer_core::UserQuestionPromptRequest;
use ratatui::backend::TestBackend;
use serde_json::json;
use std::sync::mpsc;
use std::time::Duration;

fn sample_question_payload() -> serde_json::Value {
    json!([
        {
            "header": "Mode",
            "question": "Pick one",
            "options": [
                {"label": "Fast", "description": "Prioritize speed"},
                {"label": "Careful", "description": "Prioritize review"}
            ]
        }
    ])
}

fn sample_multi_select_payload() -> serde_json::Value {
    json!([
        {
            "header": "Review",
            "question": "Choose checks",
            "multiSelect": true,
            "options": [
                {"label": "Tests", "description": "Run focused tests"},
                {"label": "Format", "description": "Check formatting"}
            ]
        }
    ])
}

fn sample_preview_payload() -> serde_json::Value {
    json!([
        {
            "header": "Mode",
            "question": "Pick one",
            "options": [
                {
                    "label": "Fast",
                    "description": "Prioritize speed",
                    "preview": "**Fast** path\nskips broad tests"
                },
                {
                    "label": "Careful",
                    "description": "Prioritize review",
                    "preview": "Careful path\nruns focused tests"
                }
            ]
        }
    ])
}

#[test]
fn poll_pending_submit_opens_user_question_overlay() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let auth_path = paths.user_config_dir.join("auth.json");

    let request = UserQuestionPromptRequest {
        questions: sample_question_payload(),
    };
    let (event_tx, event_rx) = mpsc::channel();
    let (response_tx, _response_rx) = mpsc::channel();
    event_tx
        .send(PendingSubmitEvent::UserQuestionRequest(
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
    assert!(tui.pending_user_question_request.is_some());
    assert_eq!(
        tui.overlay,
        Some(OverlayState::UserQuestionPrompt {
            overlay: UserQuestionOverlay::from_value(request.questions).unwrap(),
        })
    );
}

#[test]
fn user_question_enter_sends_selected_answer() {
    let (response_tx, response_rx) = mpsc::channel();
    let mut tui = TuiState {
        overlay: Some(OverlayState::UserQuestionPrompt {
            overlay: UserQuestionOverlay::from_value(sample_question_payload()).unwrap(),
        }),
        pending_user_question_request: Some(PendingUserQuestionRequest { response_tx }),
        ..TuiState::default()
    };

    assert!(handle_user_question_key(
        KeyEvent::from(KeyCode::Down),
        &mut tui
    ));
    assert!(handle_user_question_key(
        KeyEvent::from(KeyCode::Enter),
        &mut tui
    ));
    let response = response_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(response.answers["Pick one"], json!("Careful"));
    assert!(response.annotations.is_empty());
    assert!(tui.overlay.is_none());
    assert!(tui.pending_user_question_request.is_none());
}

#[test]
fn user_question_response_preserves_composer_draft() {
    let (response_tx, response_rx) = mpsc::channel();
    let draft = "keep this draft [Pasted text #1 +2 lines]".to_string();
    let pending_pastes = vec![(
        "[Pasted text #1 +2 lines]".to_string(),
        "remembered pasted text\nwith another line".to_string(),
    )];
    let mut tui = TuiState {
        input: draft.clone(),
        cursor: draft.len(),
        slash_selection: 3,
        overlay: Some(OverlayState::UserQuestionPrompt {
            overlay: UserQuestionOverlay::from_value(sample_question_payload()).unwrap(),
        }),
        pending_user_question_request: Some(PendingUserQuestionRequest { response_tx }),
        pending_pastes: pending_pastes.clone(),
        ..TuiState::default()
    };

    assert!(handle_user_question_key(
        KeyEvent::from(KeyCode::Enter),
        &mut tui
    ));
    let response = response_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(response.answers["Pick one"], json!("Fast"));
    assert!(response.annotations.is_empty());
    assert!(tui.overlay.is_none());
    assert!(tui.pending_user_question_request.is_none());
    assert_eq!(tui.input, draft);
    assert_eq!(tui.cursor, tui.input.len());
    assert_eq!(tui.pending_pastes, pending_pastes);
    assert_eq!(tui.slash_selection, 0);
}

#[test]
fn user_question_number_shortcut_sends_single_select_answer() {
    let (response_tx, response_rx) = mpsc::channel();
    let mut tui = TuiState {
        overlay: Some(OverlayState::UserQuestionPrompt {
            overlay: UserQuestionOverlay::from_value(sample_question_payload()).unwrap(),
        }),
        pending_user_question_request: Some(PendingUserQuestionRequest { response_tx }),
        ..TuiState::default()
    };

    assert!(handle_user_question_key(
        KeyEvent::from(KeyCode::Char('2')),
        &mut tui
    ));
    let response = response_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(response.answers["Pick one"], json!("Careful"));
    assert!(tui.overlay.is_none());
    assert!(tui.pending_user_question_request.is_none());
}

#[test]
fn user_question_number_shortcut_toggles_multi_select_answer() {
    let (response_tx, response_rx) = mpsc::channel();
    let mut tui = TuiState {
        overlay: Some(OverlayState::UserQuestionPrompt {
            overlay: UserQuestionOverlay::from_value(sample_multi_select_payload()).unwrap(),
        }),
        pending_user_question_request: Some(PendingUserQuestionRequest { response_tx }),
        ..TuiState::default()
    };

    assert!(handle_user_question_key(
        KeyEvent::from(KeyCode::Char('2')),
        &mut tui
    ));
    assert!(response_rx.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(handle_user_question_key(
        KeyEvent::from(KeyCode::Enter),
        &mut tui
    ));
    let response = response_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(response.answers["Choose checks"], json!(["Format"]));
    assert!(tui.overlay.is_none());
    assert!(tui.pending_user_question_request.is_none());
}

#[test]
fn render_user_question_shows_list_options() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    let overlay = OverlayState::UserQuestionPrompt {
        overlay: UserQuestionOverlay::from_value(sample_question_payload()).unwrap(),
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
    assert!(rendered.contains("Mode: Pick one"));
    assert!(rendered.contains("Fast  Prioritize speed"));
    assert!(rendered.contains("Careful  Prioritize review"));
}

#[test]
fn user_question_selected_preview_tracks_selection() {
    let mut overlay = UserQuestionOverlay::from_value(sample_preview_payload()).unwrap();

    assert_eq!(
        overlay.selected_preview(),
        Some("**Fast** path\nskips broad tests")
    );
    overlay.select_next();
    assert_eq!(
        overlay.selected_preview(),
        Some("Careful path\nruns focused tests")
    );
}

#[test]
fn render_user_question_shows_selected_preview() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    let overlay = OverlayState::UserQuestionPrompt {
        overlay: UserQuestionOverlay::from_value(sample_preview_payload()).unwrap(),
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
    assert!(rendered.contains("Preview"));
    assert!(rendered.contains("Fast path"));
    assert!(rendered.contains("skips broad tests"));
    assert!(!rendered.contains("Careful path"));
}
