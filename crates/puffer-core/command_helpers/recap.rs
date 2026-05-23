//! Slash-command handler for `/recap`. See [`crate::recap`] for the
//! underlying side-turn logic and skip-check helpers.

use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;

use crate::command_helpers::emit_system;
use crate::recap::{render_recap, run_recap_side_turn};
use crate::state::AppState;

/// Handles `/recap`. Runs one synchronous recap side-turn against the
/// currently selected provider, then renders the result as a transient
/// system message in the live transcript. Failures surface as a plain
/// `emit_system` so the user can see them.
///
/// The recap is **not** persisted: `render_recap` only pushes to the
/// in-memory transcript, not to `session.jsonl`. `/resume` will not
/// show historical recaps.
pub(crate) fn handle_recap_command(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    session_store: &SessionStore,
    _args: &str,
) -> Result<()> {
    if !state.config.recap.enabled {
        return emit_system(
            state,
            session_store,
            "Recap is disabled in this session's config (`recap.enabled = false`).".to_string(),
        );
    }
    match run_recap_side_turn(state, resources, providers, auth_store) {
        Ok(text) if text.is_empty() => emit_system(
            state,
            session_store,
            "Recap produced no text (model returned empty response).".to_string(),
        ),
        Ok(text) => {
            render_recap(state, &text);
            Ok(())
        }
        Err(error) => emit_system(state, session_store, format!("Recap failed: {error}")),
    }
}
