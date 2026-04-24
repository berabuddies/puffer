use anyhow::Result;
use puffer_core::{apply_model_preferences, AppState, MessageRole};
use puffer_provider_registry::AuthStore;
use puffer_session_store::SessionStore;
use std::path::Path;

use crate::flow::emit_system_message;
use crate::state::TuiState;

/// Returns true when the help transcript is visible without an active overlay.
pub(crate) fn help_pane_active_without_overlay(state: &AppState, tui: &TuiState) -> bool {
    tui.overlay.is_none()
        && state.transcript.last().is_some_and(|message| {
            message.role == MessageRole::System && message.text.starts_with("Supported commands:")
        })
}

/// Returns true when Puffer should use the content-sized bottom viewport.
pub(crate) fn should_use_inline_viewport(no_alt_screen: bool) -> bool {
    no_alt_screen
}

/// Applies the selected model, effort, and fast-mode choices to the current session.
pub(crate) fn apply_model_selection_preferences(
    state: &mut AppState,
    auth_store: &AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    provider_id: &str,
    model_id: &str,
    effort: &str,
    fast_mode: bool,
) -> Result<()> {
    let _ = auth_store;
    let _ = auth_path;
    apply_model_preferences(state, provider_id, model_id, effort, fast_mode)?;
    session_store.append_event(state.session.id, state.snapshot_event())?;
    emit_system_message(
        state,
        session_store,
        format!(
            "Active model set to {provider_id}/{model_id}.\nEffort level set to {effort}.\nFast mode is now {}.",
            if fast_mode { "on" } else { "off" }
        ),
    )
}

/// Applies a direct model change while preserving the current effort and fast-mode values.
pub(crate) fn apply_selected_model(
    state: &mut AppState,
    session_store: &SessionStore,
    provider_id: &str,
    model_id: &str,
) -> Result<()> {
    let current_effort = state.effort_level.clone();
    let current_fast_mode = state.fast_mode;
    apply_model_preferences(
        state,
        provider_id,
        model_id,
        &current_effort,
        current_fast_mode,
    )?;
    session_store.append_event(state.session.id, state.snapshot_event())?;
    emit_system_message(
        state,
        session_store,
        format!(
            "Active model set to {provider_id}/{model_id}.\nEffort level set to {}.\nFast mode is now {}.",
            state.effort_level,
            if state.fast_mode { "on" } else { "off" }
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::should_use_inline_viewport;

    #[test]
    fn inline_viewport_tracks_no_alt_screen_setting() {
        assert!(should_use_inline_viewport(true));
        assert!(!should_use_inline_viewport(false));
    }
}
