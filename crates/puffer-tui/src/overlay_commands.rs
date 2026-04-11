use crate::state::TuiState;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use puffer_core::{AppState, CommandSpec};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;
use std::path::Path;

/// Handles slash-command entry while onboarding overlays are active.
pub(crate) fn handle_onboarding_command_key(
    key: KeyEvent,
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    commands: &[CommandSpec],
    tui: &mut TuiState,
    no_alt_screen: bool,
) -> Result<bool> {
    match key.code {
        KeyCode::Char('/') if tui.input.is_empty() => {
            tui.insert_char('/', commands);
            Ok(true)
        }
        KeyCode::Char(ch)
            if !key.modifiers.contains(KeyModifiers::CONTROL) && tui.input.starts_with('/') =>
        {
            tui.insert_char(ch, commands);
            Ok(true)
        }
        KeyCode::Backspace if tui.input.starts_with('/') => {
            tui.backspace(commands);
            Ok(true)
        }
        KeyCode::Delete if tui.input.starts_with('/') => {
            tui.delete(commands);
            Ok(true)
        }
        KeyCode::Esc if tui.input.starts_with('/') => {
            tui.clear(commands);
            Ok(true)
        }
        KeyCode::Enter if tui.input.starts_with('/') => {
            if tui.complete_on_enter(commands) {
                return Ok(true);
            }
            let current_input = tui.input.clone();
            if super::try_open_overlay(
                state,
                resources,
                providers,
                auth_store,
                session_store,
                tui,
                &current_input,
            )? {
                return Ok(true);
            }
            let submitted = tui.take_input();
            tui.overlay = None;
            super::handle_submit(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                submitted,
                no_alt_screen,
            )?;
            super::submit_queued_prompt_if_ready(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                tui,
                no_alt_screen,
            )?;
            Ok(true)
        }
        _ => Ok(false),
    }
}
