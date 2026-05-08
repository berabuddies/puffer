//! `/goal` slash command — session-scoped objective tracker.
//!
//! Borrows the public-facing shape of Codex's per-thread goal feature
//! (`codex/codex-rs/core/src/goals.rs`, `codex-rs/tui/src/slash_command.rs`):
//! a free-form `objective` plus an optional `token_budget`. Codex
//! exposes the goal to the model via three tools (`get_goal`,
//! `create_goal`, `update_goal`) so the model can self-track; that
//! piece is left as a follow-up — this MVP is the user-side surface
//! only (set / show / clear / budget).
//!
//! Persistence is in-memory on `AppState.session_goal` and lives for
//! the duration of the session. Surfacing in `/status` is a
//! follow-up; for now the handler echoes the current state on every
//! mutation so the user has an immediate confirmation.

use super::emit_system;
use crate::state::SessionGoal;
use crate::AppState;
use anyhow::Result;
use puffer_session_store::SessionStore;
use std::time::{SystemTime, UNIX_EPOCH};

const USAGE: &str = "Usage: /goal [<objective> | clear | budget <N> | status | help]\n\
Sets or shows the session-scoped goal. With no argument, shows the current goal.\n\
With text, replaces the current goal. `clear` removes it. `budget <N>` attaches a\n\
token budget to the active goal.";

pub(crate) fn handle_goal_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let trimmed = args.trim();
    let lower = trimmed.to_ascii_lowercase();

    if trimmed.is_empty() || matches!(lower.as_str(), "status" | "show" | "current" | "info") {
        return emit_system(state, session_store, render_goal_status(state));
    }

    if matches!(lower.as_str(), "help" | "--help" | "-h") {
        return emit_system(state, session_store, USAGE.to_string());
    }

    if matches!(lower.as_str(), "clear" | "reset" | "remove" | "off") {
        let message = if state.session_goal.is_some() {
            state.session_goal = None;
            "Session goal cleared.".to_string()
        } else {
            "No goal was set.".to_string()
        };
        return emit_system(state, session_store, message);
    }

    if let Some(rest) = lower.strip_prefix("budget") {
        return apply_budget(state, session_store, rest.trim());
    }

    // Anything else is treated as the new goal text.
    let now_ms = unix_time_ms();
    let prior = state.session_goal.clone();
    state.session_goal = Some(SessionGoal {
        objective: trimmed.to_string(),
        token_budget: prior.as_ref().and_then(|g| g.token_budget),
        set_at_ms: now_ms,
    });
    let action = if prior.is_some() { "updated" } else { "set" };
    emit_system(
        state,
        session_store,
        format!("Goal {action}.\n{}", render_goal_status(state)),
    )
}

fn apply_budget(state: &mut AppState, session_store: &SessionStore, arg: &str) -> Result<()> {
    if arg.is_empty() {
        return emit_system(
            state,
            session_store,
            "Usage: /goal budget <N>  — N must be a positive token budget.".to_string(),
        );
    }
    let parsed = match arg.parse::<u32>() {
        Ok(value) if value > 0 => value,
        _ => {
            return emit_system(
                state,
                session_store,
                format!("Invalid token budget `{arg}`. Expected a positive integer."),
            );
        }
    };
    let Some(goal) = state.session_goal.as_mut() else {
        return emit_system(
            state,
            session_store,
            "No active goal. Set one with `/goal <objective>` first.".to_string(),
        );
    };
    goal.token_budget = Some(parsed);
    let message = format!(
        "Token budget set to {parsed}.\n{}",
        render_goal_status(state)
    );
    emit_system(state, session_store, message)
}

fn render_goal_status(state: &AppState) -> String {
    let Some(goal) = state.session_goal.as_ref() else {
        return "No goal set. Use `/goal <objective>` to set one.".to_string();
    };
    let mut lines = vec![format!("Goal: {}", goal.objective)];
    if let Some(budget) = goal.token_budget {
        lines.push(format!("Token budget: {budget}"));
    }
    if let Some(elapsed) = elapsed_human(goal.set_at_ms) {
        lines.push(format!("Set: {elapsed} ago"));
    }
    lines.join("\n")
}

fn elapsed_human(set_at_ms: u128) -> Option<String> {
    let now_ms = unix_time_ms();
    if now_ms <= set_at_ms {
        return None;
    }
    let elapsed_secs = (now_ms - set_at_ms) / 1000;
    if elapsed_secs < 60 {
        return Some(format!("{elapsed_secs}s"));
    }
    let minutes = elapsed_secs / 60;
    if minutes < 60 {
        return Some(format!("{minutes}m"));
    }
    let hours = minutes / 60;
    let remaining_minutes = minutes % 60;
    if hours < 24 {
        if remaining_minutes == 0 {
            return Some(format!("{hours}h"));
        }
        return Some(format!("{hours}h{remaining_minutes}m"));
    }
    let days = hours / 24;
    let remaining_hours = hours % 24;
    if remaining_hours == 0 {
        Some(format!("{days}d"))
    } else {
        Some(format!("{days}d{remaining_hours}h"))
    }
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
    use tempfile::tempdir;

    fn make_state() -> (AppState, SessionStore, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let paths = ConfigPaths::discover(tmp.path());
        ensure_workspace_dirs(&paths).unwrap();
        let store = SessionStore::from_paths(&paths).unwrap();
        let session = store.create_session(tmp.path().to_path_buf()).unwrap();
        let state = AppState::new(PufferConfig::default(), tmp.path().to_path_buf(), session);
        (state, store, tmp)
    }

    #[test]
    fn empty_args_renders_no_goal_message() {
        let (mut state, store, _tmp) = make_state();
        handle_goal_command(&mut state, &store, "").unwrap();
        assert!(state.session_goal.is_none());
    }

    #[test]
    fn setting_text_creates_goal() {
        let (mut state, store, _tmp) = make_state();
        handle_goal_command(&mut state, &store, "ship the kimi-v17 trial").unwrap();
        let goal = state.session_goal.as_ref().expect("goal set");
        assert_eq!(goal.objective, "ship the kimi-v17 trial");
        assert!(goal.token_budget.is_none());
        assert!(goal.set_at_ms > 0);
    }

    #[test]
    fn setting_text_again_updates_goal_and_preserves_budget() {
        let (mut state, store, _tmp) = make_state();
        handle_goal_command(&mut state, &store, "first goal").unwrap();
        handle_goal_command(&mut state, &store, "budget 50000").unwrap();
        handle_goal_command(&mut state, &store, "second goal").unwrap();
        let goal = state.session_goal.as_ref().expect("goal still set");
        assert_eq!(goal.objective, "second goal");
        assert_eq!(goal.token_budget, Some(50_000));
    }

    #[test]
    fn clear_removes_goal() {
        let (mut state, store, _tmp) = make_state();
        handle_goal_command(&mut state, &store, "to be cleared").unwrap();
        assert!(state.session_goal.is_some());
        handle_goal_command(&mut state, &store, "clear").unwrap();
        assert!(state.session_goal.is_none());
    }

    #[test]
    fn budget_without_goal_emits_message_and_does_not_set_anything() {
        let (mut state, store, _tmp) = make_state();
        handle_goal_command(&mut state, &store, "budget 10000").unwrap();
        assert!(state.session_goal.is_none());
    }

    #[test]
    fn budget_with_goal_attaches_budget() {
        let (mut state, store, _tmp) = make_state();
        handle_goal_command(&mut state, &store, "ship something").unwrap();
        handle_goal_command(&mut state, &store, "budget 100000").unwrap();
        assert_eq!(
            state.session_goal.as_ref().unwrap().token_budget,
            Some(100_000)
        );
    }

    #[test]
    fn budget_rejects_zero_and_non_numeric() {
        let (mut state, store, _tmp) = make_state();
        handle_goal_command(&mut state, &store, "ship something").unwrap();
        handle_goal_command(&mut state, &store, "budget 0").unwrap();
        assert!(state.session_goal.as_ref().unwrap().token_budget.is_none());
        handle_goal_command(&mut state, &store, "budget banana").unwrap();
        assert!(state.session_goal.as_ref().unwrap().token_budget.is_none());
    }
}
