use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use puffer_core::PermissionPromptAction;

use crate::flow::respond_to_permission_prompt;
use crate::{OverlayState, TuiState};

/// Handles key input for the active permission approval overlay.
pub(crate) fn handle_permission_prompt_key(key: KeyEvent, tui: &mut TuiState) -> bool {
    let Some(active_overlay) = tui.overlay.as_ref() else {
        return false;
    };
    if !matches!(active_overlay, OverlayState::PermissionPrompt { .. }) {
        return false;
    }

    let action = match key.code {
        KeyCode::Esc => Some(PermissionPromptAction::Deny),
        KeyCode::Enter => active_overlay.selected_permission_action(),
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
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(PermissionPromptAction::Deny)
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => tui
            .overlay
            .as_mut()
            .and_then(|overlay| overlay.permission_shortcut_action(ch)),
        _ => None,
    };

    if let Some(action) = action {
        respond_to_permission_prompt(tui, action);
        return true;
    }

    true
}
