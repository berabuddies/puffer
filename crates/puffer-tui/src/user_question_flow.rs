use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::flow::respond_to_user_question;
use crate::user_question_overlay::{UserQuestionOverlay, UserQuestionShortcutActivation};
use crate::{OverlayState, TuiState};

/// Handles key input for the active `AskUserQuestion` overlay.
pub(crate) fn handle_user_question_key(key: KeyEvent, tui: &mut TuiState) -> bool {
    let Some(OverlayState::UserQuestionPrompt { .. }) = tui.overlay.as_ref() else {
        return false;
    };

    let response = match key.code {
        KeyCode::Esc => Some(UserQuestionOverlay::empty_response()),
        KeyCode::Enter => tui.overlay.as_mut().and_then(|overlay| match overlay {
            OverlayState::UserQuestionPrompt { overlay } => overlay.confirm_current(),
            _ => None,
        }),
        KeyCode::Up => {
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.select_previous();
            }
            None
        }
        KeyCode::Down => {
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.select_next();
            }
            None
        }
        KeyCode::PageUp => {
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.page_up();
            }
            None
        }
        KeyCode::PageDown => {
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.page_down();
            }
            None
        }
        KeyCode::Backspace => {
            if let Some(OverlayState::UserQuestionPrompt { overlay }) = tui.overlay.as_mut() {
                overlay.backspace_custom_answer();
            }
            None
        }
        KeyCode::Char(' ') => {
            if let Some(OverlayState::UserQuestionPrompt { overlay }) = tui.overlay.as_mut() {
                if overlay.custom_answer_active() || overlay.has_custom_answer() {
                    overlay.insert_custom_char(' ');
                } else {
                    overlay.toggle_current();
                }
            }
            None
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UserQuestionOverlay::empty_response())
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            tui.overlay.as_mut().and_then(|overlay| match overlay {
                OverlayState::UserQuestionPrompt { overlay } => {
                    if overlay.custom_answer_active() || overlay.has_custom_answer() {
                        overlay.insert_custom_char(ch);
                        None
                    } else {
                        match overlay.activate_shortcut(ch) {
                            UserQuestionShortcutActivation::Ignored => {
                                overlay.insert_custom_char(ch);
                                None
                            }
                            UserQuestionShortcutActivation::Pending => None,
                            UserQuestionShortcutActivation::Response(response) => Some(response),
                        }
                    }
                }
                _ => None,
            })
        }
        _ => None,
    };

    if let Some(response) = response {
        respond_to_user_question(tui, response);
    }
    true
}
