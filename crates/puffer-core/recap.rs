//! Session recap (away-summary) feature.
//!
//! Mirrors Anthropic claude-code's `/recap` semantics (RE'd at v2.1.142):
//! a short, model-generated one-liner that surfaces in the UI when the
//! user comes back after stepping away. Two entrypoints:
//!
//! 1. `/recap` slash command — always runs (subject to `enabled`).
//! 2. Auto-trigger from TUI idle timer or GUI window-blur events — runs
//!    only when `auto=true` and the [`should_auto_trigger`] skip checks
//!    pass.
//!
//! The generated text is rendered as a transient system message in the
//! transcript: it is pushed into `state.transcript` for immediate display
//! but **not** written to `session.jsonl`, so `/resume` does not surface
//! historical recaps and the model never sees past recaps in its context.
//!
//! Constants are inherited from claude-code's reverse-engineered defaults
//! (`on8=180s`, `Ls3=3`, `ks3=2`).

use anyhow::Result;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;

use crate::state::{AppState, MessageRole};

/// Prompt sent to the model. Lifted verbatim from claude-code's `tP5`
/// constant so the linguistic shape matches what Anthropic ships and so
/// the model's "Skip ... em-dash tangents" anti-pattern hint carries over.
pub(crate) const RECAP_PROMPT: &str = "The user stepped away and is coming back. Recap in under 40 words, 1-2 plain sentences, no markdown. Lead with the overall goal and current task, then the one next action. Skip root-cause narrative, fix internals, secondary to-dos, and em-dash tangents.";

/// Visible-but-quiet prefix the TUI / GUI use to render the recap line
/// with a distinct symbol so users can tell auto-recap apart from a
/// regular system note. Matches claude-code's `nwq` reference symbol.
pub const RECAP_DISPLAY_PREFIX: &str = "\u{203B} recap: ";

/// Runs one synchronous recap side-turn against the currently selected
/// provider and model. Tool-filter is empty, so no tools are reachable;
/// the model sees the in-memory transcript (via `state.clone()`) and
/// returns a short summary as plain assistant text.
///
/// Returns the trimmed assistant text on success. Errors propagate so
/// callers can decide whether to surface the failure to the user
/// (slash command path) or swallow it (auto-trigger path).
pub fn run_recap_side_turn(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
) -> Result<String> {
    let mut side_state = state.clone();
    let filter = crate::runtime::build_request_tool_filter(&[])?;
    let turn = crate::runtime::execute_user_prompt_with_tool_filter(
        &mut side_state,
        resources,
        providers,
        auth_store,
        RECAP_PROMPT,
        filter.as_ref(),
    )?;
    Ok(turn.assistant_text.trim().to_string())
}

/// Renders a successful recap into the live transcript without writing
/// it to `session.jsonl`. The recap is `MessageRole::System` so existing
/// renderers display it; the [`RECAP_DISPLAY_PREFIX`] sentinel lets TUI /
/// GUI styling pick it out for italic+dim treatment.
pub fn render_recap(state: &mut AppState, text: &str) {
    let mut line = String::with_capacity(RECAP_DISPLAY_PREFIX.len() + text.len());
    line.push_str(RECAP_DISPLAY_PREFIX);
    line.push_str(text);
    state.push_message(MessageRole::System, line);
}

/// Skip-check decisions for the auto-trigger path. Returns `Ok(true)` if
/// a recap should fire, `Ok(false)` if any guard rejects it. The slash
/// command path bypasses these checks.
///
/// `last_recap_user_msg_count` is the count of user messages observed at
/// the time the last recap fired (or `None` if no recap has fired this
/// session). `draft_input` is the user's unsent input buffer (TUI's
/// editor contents or GUI's input field text).
pub fn should_auto_trigger(
    state: &AppState,
    last_recap_user_msg_count: Option<usize>,
    draft_input: &str,
) -> bool {
    if !state.config.recap.enabled || !state.config.recap.auto {
        return false;
    }
    if !draft_input.trim().is_empty() {
        return false;
    }
    let user_count = count_user_messages(state);
    if user_count < state.config.recap.min_user_messages {
        return false;
    }
    if let Some(last) = last_recap_user_msg_count {
        if user_count.saturating_sub(last) < state.config.recap.cooldown_messages {
            return false;
        }
    }
    true
}

/// Counts real user messages in the in-memory transcript. Used by the
/// auto-trigger skip checks to enforce `Ls3` (min user messages) and
/// `ks3` (cooldown between recaps).
pub fn count_user_messages(state: &AppState) -> usize {
    state
        .transcript
        .iter()
        .filter(|message| matches!(message.role, MessageRole::User))
        .count()
}

/// Whether the recap feature is enabled at all (gating both the slash
/// command and the auto-trigger).
pub fn enabled(state: &AppState) -> bool {
    state.config.recap.enabled
}

/// Whether the auto-trigger specifically is enabled. Slash command runs
/// regardless when [`enabled`] is true.
pub fn auto_enabled(state: &AppState) -> bool {
    state.config.recap.enabled && state.config.recap.auto
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::PufferConfig;
    use puffer_session_store::SessionMetadata;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn fresh_state() -> AppState {
        let session = SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: PathBuf::from("/tmp"),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        AppState::new(PufferConfig::default(), PathBuf::from("/tmp"), session)
    }

    fn push_user(state: &mut AppState, text: &str) {
        state.push_message(MessageRole::User, text.to_string());
    }

    #[test]
    fn skip_when_disabled() {
        let mut state = fresh_state();
        state.config.recap.enabled = false;
        for _ in 0..10 {
            push_user(&mut state, "u");
        }
        assert!(!should_auto_trigger(&state, None, ""));
    }

    #[test]
    fn skip_when_auto_disabled_only() {
        let mut state = fresh_state();
        state.config.recap.enabled = true;
        state.config.recap.auto = false;
        for _ in 0..10 {
            push_user(&mut state, "u");
        }
        // enabled() stays true (slash command still works) but auto is off.
        assert!(enabled(&state));
        assert!(!auto_enabled(&state));
        assert!(!should_auto_trigger(&state, None, ""));
    }

    #[test]
    fn skip_when_draft_non_empty() {
        let mut state = fresh_state();
        for _ in 0..5 {
            push_user(&mut state, "u");
        }
        assert!(!should_auto_trigger(&state, None, "typing..."));
        // Whitespace-only is treated as empty (trim) — does NOT block.
        assert!(should_auto_trigger(&state, None, "  "));
    }

    #[test]
    fn skip_below_min_user_messages() {
        let mut state = fresh_state();
        push_user(&mut state, "one");
        push_user(&mut state, "two");
        // default min_user_messages = 3, two is below threshold
        assert!(!should_auto_trigger(&state, None, ""));
        push_user(&mut state, "three");
        assert!(should_auto_trigger(&state, None, ""));
    }

    #[test]
    fn skip_during_cooldown() {
        let mut state = fresh_state();
        for _ in 0..3 {
            push_user(&mut state, "u");
        }
        // Last recap fired when user_count == 3; cooldown = 2 default.
        // Still 3 now → 0 new messages, cooldown blocks.
        assert!(!should_auto_trigger(&state, Some(3), ""));
        // One new user message — still inside cooldown (need 2).
        push_user(&mut state, "u");
        assert!(!should_auto_trigger(&state, Some(3), ""));
        // Two new — cooldown satisfied.
        push_user(&mut state, "u");
        assert!(should_auto_trigger(&state, Some(3), ""));
    }

    #[test]
    fn render_recap_uses_prefix() {
        let mut state = fresh_state();
        render_recap(&mut state, "Hello world.");
        let last = state.transcript.last().expect("recap message pushed");
        assert!(matches!(last.role, MessageRole::System));
        assert!(last.text.starts_with(RECAP_DISPLAY_PREFIX));
        assert!(last.text.ends_with("Hello world."));
    }
}
