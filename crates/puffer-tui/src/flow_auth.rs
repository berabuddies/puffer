use anyhow::Result;
use puffer_core::{login_provider_credentials, logout_provider_credentials, AppState, MessageRole};
use puffer_provider_registry::AuthStore;
use puffer_session_store::{SessionStore, TranscriptEvent};
use std::io;
use std::path::Path;

/// Handles embedded login and logout commands from the TUI.
pub(crate) fn handle_auth_command(
    state: &mut AppState,
    auth_store: &mut AuthStore,
    _auth_path: &Path,
    session_store: &SessionStore,
    submitted: &str,
    no_alt_screen: bool,
) -> Result<bool> {
    let without_slash = submitted.trim_start_matches('/');
    let (name, args) = without_slash
        .split_once(' ')
        .map(|(name, args)| (name, args.trim()))
        .unwrap_or((without_slash, ""));
    if name == "login" {
        if args.is_empty() {
            return Ok(false);
        }
        let provider = if args.is_empty() {
            state.current_provider.as_deref().unwrap_or("anthropic")
        } else {
            args
        };
        let message = run_embedded_auth_login(state, provider, auth_store, no_alt_screen)?;
        state.push_message(MessageRole::System, message.clone());
        session_store.append_event(
            state.session.id,
            TranscriptEvent::SystemMessage {
                text: message,
                actor: Some(state.system_actor()),
            },
        )?;
        return Ok(true);
    }

    if name != "logout" {
        return Ok(false);
    }

    let provider = if args.is_empty() {
        state.current_provider.as_deref().unwrap_or("anthropic")
    } else {
        args
    }
    .to_string();
    let message = logout_provider_credentials(state, auth_store, &provider)?;
    state.push_message(MessageRole::System, message.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::SystemMessage {
            text: message,
            actor: Some(state.system_actor()),
        },
    )?;
    Ok(true)
}

/// Runs the provider login flow while temporarily restoring the terminal state.
pub(crate) fn run_embedded_auth_login(
    state: &AppState,
    provider: &str,
    auth_store: &mut AuthStore,
    no_alt_screen: bool,
) -> Result<String> {
    if !no_alt_screen {
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    }

    let result = login_provider_credentials(state, auth_store, provider);

    if !no_alt_screen {
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    }

    result
}
