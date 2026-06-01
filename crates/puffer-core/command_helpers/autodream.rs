//! Handler for the `/autodream` slash command.

use crate::AppState;
use anyhow::Result;
use puffer_config::ConfigPaths;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;

/// Runs or reports AutoDream state for the current session.
pub(crate) fn handle_autodream_command(
    state: &mut AppState,
    session_store: &SessionStore,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    args: &str,
) -> Result<String> {
    let trimmed = args.trim();
    match trimmed {
        "" => {}
        "on" | "enable" | "enabled" => return set_autodream_enabled(state, true),
        "off" | "disable" | "disabled" => return set_autodream_enabled(state, false),
        "suggestions" | "queue" | "genskill" => {
            return Ok(crate::autodream_suggestions_with_store(session_store));
        }
        "status" | "show" | "check" => {
            return Ok(crate::autodream_status_with_store(state, session_store));
        }
        "help" | "?" | "-h" | "--help" => return Ok(autodream_help_text().to_string()),
        _ => return Ok(unknown_autodream_command_message(trimmed)),
    }

    let bootstrap = crate::ensure_manual_autodream_project_memory(state)?;
    let outcome = crate::run_autodream_review(state, resources, providers, auth_store)?;
    let should_suggest_genskill = crate::should_show_manual_autodream_genskill_suggestion(
        state,
        &bootstrap,
        outcome.genskill_suggested,
    );
    Ok(crate::render_manual_autodream_result(
        &bootstrap,
        &outcome,
        should_suggest_genskill,
    ))
}

fn autodream_help_text() -> &'static str {
    "AutoDream consolidates durable project memory.\n\n\
Usage: /autodream [on|off|status|suggestions|help]\n\n\
  /autodream              Run one memory consolidation pass.\n\
  /autodream on           Enable automatic background consolidation.\n\
  /autodream off          Disable automatic background consolidation.\n\
  /autodream status       Show scheduler and last-run status.\n\
  /autodream suggestions  Show pending skill suggestions.\n\
  /autodream help         Show this help."
}

fn set_autodream_enabled(state: &mut AppState, enabled: bool) -> Result<String> {
    apply_autodream_enabled(state, enabled);
    let paths = ConfigPaths::discover(&state.cwd);
    let path = crate::config_settings::persist_config_setting(&paths, state, "autodreamEnabled")?;
    let status = if enabled { "on" } else { "off" };
    let behavior = if enabled { "enabled" } else { "disabled" };
    Ok(match path {
        Some(path) => format!(
            "AutoDream is {status}.\nBackground memory consolidation is {behavior}.\nSaved to {}.",
            path.display()
        ),
        None => {
            format!("AutoDream is {status}.\nBackground memory consolidation is {behavior}.")
        }
    })
}

fn apply_autodream_enabled(state: &mut AppState, enabled: bool) {
    state.config.memory.autodream_enabled = enabled;
}

fn unknown_autodream_command_message(command: &str) -> String {
    format!(
        "Unknown AutoDream command: {command}\n\n{}",
        autodream_help_text()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageRole;
    use puffer_config::{MemoryConfig, PufferConfig};
    use puffer_session_store::SessionMetadata;
    use std::path::PathBuf;
    use uuid::Uuid;

    const MIN_GENSKILL_SUGGESTION_MESSAGES: usize = 4;

    fn test_state(message_count: usize) -> AppState {
        let mut state = AppState::new(
            PufferConfig {
                memory: MemoryConfig {
                    autodream_genskill_suggestions: true,
                    ..MemoryConfig::default()
                },
                ..PufferConfig::default()
            },
            PathBuf::from("/workspace/demo"),
            SessionMetadata {
                id: Uuid::nil(),
                display_name: None,
                generated_title: None,
                cwd: PathBuf::from("/workspace/demo"),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        );
        for index in 0..message_count {
            let role = if index % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            };
            state.push_message(role, format!("message {index}"));
        }
        state
    }

    #[test]
    fn genskill_suggestion_is_hidden_on_first_bootstrap() {
        let state = test_state(MIN_GENSKILL_SUGGESTION_MESSAGES);
        let bootstrap = crate::ManualAutoDreamBootstrap {
            message: String::new(),
            initialized_project_memory: true,
        };
        assert!(!crate::should_show_manual_autodream_genskill_suggestion(
            &state, &bootstrap, true
        ));
    }

    #[test]
    fn genskill_suggestion_requires_enough_context() {
        let state = test_state(MIN_GENSKILL_SUGGESTION_MESSAGES - 1);
        let bootstrap = crate::ManualAutoDreamBootstrap::default();
        assert!(!crate::should_show_manual_autodream_genskill_suggestion(
            &state, &bootstrap, true
        ));
    }

    #[test]
    fn genskill_suggestion_shows_after_bootstrap_with_context() {
        let state = test_state(MIN_GENSKILL_SUGGESTION_MESSAGES);
        let bootstrap = crate::ManualAutoDreamBootstrap::default();
        assert!(crate::should_show_manual_autodream_genskill_suggestion(
            &state, &bootstrap, true
        ));
    }

    #[test]
    fn autodream_genskill_marker_is_hidden_from_user_output() {
        let text = "Updated memory.\nAUTODREAM_GENSKILL: yes\nDone.";
        assert_eq!(
            crate::visible_autodream_assistant_text(text),
            "Updated memory.\nDone."
        );
    }

    #[test]
    fn manual_autodream_result_hides_debug_counters() {
        let bootstrap = crate::ManualAutoDreamBootstrap::default();
        let outcome = crate::AutoDreamOutcome {
            assistant_text: "Updated memory.".to_string(),
            tool_invocations: Vec::new(),
            genskill_suggested: false,
        };
        let message = crate::render_manual_autodream_result(&bootstrap, &outcome, false);
        assert!(message.contains("AutoDream complete."));
        assert!(!message.contains("tool_calls="));
        assert!(!message.contains("genskill_suggested="));
    }

    #[test]
    fn manual_autodream_result_summarizes_no_change_output() {
        let bootstrap = crate::ManualAutoDreamBootstrap::default();
        let outcome = crate::AutoDreamOutcome {
            assistant_text: "Loaded and verified project memory. Kept the existing AutoDream consolidation workflow entry; no stale conflicts or polluted entries needed changes, and no new durable project facts were supported beyond existing memory.".to_string(),
            tool_invocations: Vec::new(),
            genskill_suggested: false,
        };

        let message = crate::render_manual_autodream_result(&bootstrap, &outcome, false);

        assert_eq!(
            message,
            "AutoDream complete.\nMemory checked. No durable changes needed."
        );
    }

    #[test]
    fn autodream_help_lists_public_subcommands() {
        let help = autodream_help_text();

        assert!(help.contains("Usage: /autodream [on|off|status|suggestions|help]"));
        assert!(help.contains("/autodream on"));
        assert!(help.contains("/autodream off"));
        assert!(help.contains("/autodream status"));
        assert!(help.contains("/autodream suggestions"));
        assert!(help.contains("/autodream help"));
        assert!(!help.contains("show"));
        assert!(!help.contains("check"));
    }

    #[test]
    fn unknown_autodream_command_returns_usage() {
        let message = unknown_autodream_command_message("verbose");

        assert!(message.contains("Unknown AutoDream command: verbose"));
        assert!(message.contains("Usage: /autodream [on|off|status|suggestions|help]"));
    }

    #[test]
    fn autodream_toggle_updates_session_state() {
        let mut state = test_state(0);
        apply_autodream_enabled(&mut state, false);

        assert!(!state.config.memory.autodream_enabled);
    }
}
