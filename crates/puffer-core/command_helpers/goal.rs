//! `/goal` slash command — thin wrapper over `runtime::goals`.
//!
//! All state mutation lives in `runtime::goals`; this module owns
//! string parsing of the slash-command arg and rendering the
//! resulting status as a system message. Sharing the underlying
//! module with the model-facing `get_goal` / `create_goal` /
//! `update_goal` tools is what keeps the lifecycle (4-state
//! machine, token accounting, plan-mode exclusion, optimistic
//! concurrency goal_id) consistent regardless of which surface
//! initiated the mutation. Codex applies the same separation
//! (`core/src/goals.rs` vs `core/src/tools/handlers/goal.rs` vs
//! `app-server/src/codex_message_processor/thread_goal_handlers.rs`).

use super::emit_system;
use crate::runtime::goals;
use crate::state::{GoalStatus, SessionGoal};
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
        let message = if goals::slash_clear_goal(state) {
            "Session goal cleared.".to_string()
        } else {
            "No goal was set.".to_string()
        };
        return emit_system(state, session_store, message);
    }

    if let Some(rest) = lower.strip_prefix("budget") {
        return apply_budget(state, session_store, rest.trim());
    }

    // Anything else: treat as the new goal text. `slash_set_goal`
    // preserves the prior budget and goal_id when the goal is
    // active.
    let prior_existed = state.session_goal.is_some();
    let result = goals::slash_set_goal(state, trimmed.to_string(), None);
    let message = match result {
        Ok(_) => {
            let action = if prior_existed { "updated" } else { "set" };
            format!("Goal {action}.\n{}", render_goal_status(state))
        }
        Err(error) => format!("Could not set goal: {error}"),
    };
    emit_system(state, session_store, message)
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
    match goals::slash_set_budget(state, parsed) {
        Ok(_) => emit_system(
            state,
            session_store,
            format!(
                "Token budget set to {parsed}.\n{}",
                render_goal_status(state)
            ),
        ),
        Err(error) => emit_system(state, session_store, error.to_string()),
    }
}

fn render_goal_status(state: &AppState) -> String {
    let Some(goal) = goals::get_goal(state) else {
        return "No goal set. Use `/goal <objective>` to set one.".to_string();
    };
    let mut lines = vec![format!("Goal: {}", goal.objective)];
    lines.push(format!("Status: {}", status_label(goal.status)));
    if let Some(line) = budget_line(goal) {
        lines.push(line);
    }
    if let Some(elapsed) = elapsed_human(goal.created_at_ms) {
        lines.push(format!("Active for: {elapsed}"));
    }
    lines.join("\n")
}

/// Human-friendly status label. Keeps codex-flavored vocabulary so
/// `/status` and tool JSON output line up.
fn status_label(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::Active => "active",
        GoalStatus::Paused => "paused",
        GoalStatus::BudgetLimited => "budget_limited",
        GoalStatus::Complete => "complete",
    }
}

/// Render `Tokens: USED / BUDGET (REMAINING left)` when a budget is
/// attached, else `Tokens: USED`. Returned `None` when both are 0
/// — keeps the status panel clean for fresh goals.
fn budget_line(goal: &SessionGoal) -> Option<String> {
    match (goal.token_budget, goal.tokens_used) {
        (None, 0) => None,
        (None, used) => Some(format!("Tokens: {used}")),
        (Some(budget), used) => Some(format!(
            "Tokens: {used} / {budget} ({} left)",
            (budget as u64).saturating_sub(used)
        )),
    }
}

fn elapsed_human(set_at_ms: u128) -> Option<String> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
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
    fn setting_text_creates_active_goal() {
        let (mut state, store, _tmp) = make_state();
        handle_goal_command(&mut state, &store, "ship the kimi-v17 trial").unwrap();
        let goal = state.session_goal.as_ref().expect("goal set");
        assert_eq!(goal.objective, "ship the kimi-v17 trial");
        assert!(goal.token_budget.is_none());
        assert_eq!(goal.status, GoalStatus::Active);
        assert!(goal.created_at_ms > 0);
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
