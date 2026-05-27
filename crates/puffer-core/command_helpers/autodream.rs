//! Handler for the `/autodream` slash command.

use crate::AppState;
use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;

/// Runs or reports AutoDream state for the current session.
pub(crate) fn handle_autodream_command(
    state: &AppState,
    session_store: &SessionStore,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    args: &str,
) -> Result<String> {
    let trimmed = args.trim();
    if matches!(trimmed, "suggestions" | "queue" | "genskill") {
        return Ok(crate::autodream_suggestions_with_store(session_store));
    }
    if matches!(trimmed, "status" | "show" | "check" | "?" | "-h" | "--help") {
        return Ok(crate::autodream_status_with_store(state, session_store));
    }

    let outcome = crate::run_autodream_review(state, resources, providers, auth_store)?;
    let suggestion = if state.autodream_genskill_suggestions_enabled() && outcome.genskill_suggested
    {
        "\n\nAutoDream thinks this trace is skill-worthy. Run `/genskill` after reviewing the current conversation if you want to create a reusable skill."
    } else {
        ""
    };
    Ok(format!(
        "AutoDream complete. tool_calls={} genskill_suggested={}\n\n{}{}",
        outcome.tool_invocations.len(),
        outcome.genskill_suggested,
        outcome.assistant_text,
        suggestion,
    ))
}
