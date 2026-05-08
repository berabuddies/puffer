//! Session goal lifecycle — single source of truth for puffer's
//! `/goal` slash command AND the model-facing `get_goal` /
//! `create_goal` / `update_goal` tools.
//!
//! Borrows the API shape of Codex's `core/src/goals.rs` +
//! `core/src/tools/handlers/goal.rs`, simplified for puffer's
//! single-threaded in-memory model:
//!
//! | Codex                                   | puffer                        |
//! |-----------------------------------------|-------------------------------|
//! | SQLite `thread_goals` table             | `AppState.session_goal`       |
//! | `GoalRuntimeState` + 2 semaphores       | direct `&mut AppState`        |
//! | turn-scoped + wall-clock accounting     | single `tokens_used` counter  |
//! | `apply_goal_resume_runtime_effects`     | (out of scope for MVP)        |
//! | `continue_active_goal_if_idle`          | (out of scope for MVP)        |
//! | continuation suppression `AtomicBool`   | (out of scope for MVP)        |
//! | budget-limit steering message injection | (status flip only, no inject) |
//! | `Active`/`Paused`/`BudgetLimited`/`Complete` | identical 4-state          |
//! | `update_goal` locked to `complete`      | identical (codex parity)      |
//! | Plan mode silently ignores goals        | identical (codex parity)      |
//!
//! Two layers of API:
//!
//! * **Model-facing** (`create_goal`, `mark_complete`, `get_goal`) —
//!   strict semantics. `create_goal` fails if any goal exists;
//!   `mark_complete` is the only transition the model can drive.
//!   These map directly to the three workflow tools.
//!
//! * **User-facing** (`slash_*`) — broader latitude. The user can
//!   replace any goal at any time, attach / clear budgets, drop the
//!   goal entirely. These power the `/goal` slash command's
//!   set-or-update / clear / budget subcommands.

use super::TurnUsageReport;
use crate::state::{GoalStatus, SessionGoal};
use crate::AppState;
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Codex parity: token delta credited to a goal per turn boundary.
/// Mirrors `goal_token_delta_for_usage` (codex `goals.rs:1502-1506`):
/// non-cached input + non-negative output. Cached-input tokens are
/// excluded so a long stable system prompt doesn't burn the budget
/// every turn.
pub fn token_delta_from_usage(usage: &TurnUsageReport) -> u64 {
    let non_cached_input = usage.input_tokens.saturating_sub(usage.cache_read_tokens);
    non_cached_input.saturating_add(usage.output_tokens)
}

/// Read accessor. Cheap, immutable.
pub fn get_goal(state: &AppState) -> Option<&SessionGoal> {
    state.session_goal.as_ref()
}

/// Whether the runtime should silently ignore goal mutations from
/// the model in this state. Mirrors codex's
/// `should_ignore_goal_for_mode` (`goals.rs:1389`) — Plan mode is
/// the sole excluded mode; every other mode participates.
pub fn ignore_goal_for_mode(state: &AppState) -> bool {
    state.plan_mode
}

/// Model-facing: register a fresh goal. Fails when one is already
/// active (any non-terminal status) so `create_goal` can't replace
/// in-flight work — codex enforces the same invariant via state-DB
/// `insert_thread_goal`. Use [`slash_set_goal`] when the user wants
/// the unrestricted replace-anytime semantics.
pub fn create_goal(
    state: &mut AppState,
    objective: String,
    token_budget: Option<u32>,
) -> Result<&SessionGoal> {
    if ignore_goal_for_mode(state) {
        return Err(anyhow!(
            "goals are disabled in plan mode; exit plan mode first"
        ));
    }
    if let Some(existing) = state.session_goal.as_ref() {
        if !existing.status.is_terminal() {
            return Err(anyhow!(
                "cannot create a new goal because this thread already has a goal; \
                use update_goal only when the existing goal is complete"
            ));
        }
    }
    let trimmed = objective.trim().to_string();
    if trimmed.is_empty() {
        return Err(anyhow!("objective must not be empty"));
    }
    if let Some(0) = token_budget {
        return Err(anyhow!("token_budget must be a positive integer"));
    }
    let now = unix_time_ms();
    state.session_goal = Some(SessionGoal {
        goal_id: new_goal_id(),
        objective: trimmed,
        status: GoalStatus::Active,
        token_budget,
        tokens_used: 0,
        created_at_ms: now,
        updated_at_ms: now,
    });
    Ok(state.session_goal.as_ref().unwrap())
}

/// Model-facing: mark the active goal `Complete`. The only transition
/// the `update_goal` tool exposes — codex's analog explicitly rejects
/// every other status (`tools/handlers/goal.rs:156-180`). Idempotent
/// when already `Complete`; errors when no goal exists or the goal
/// is in a non-terminal-non-active state we don't think we should
/// short-circuit (`Paused` / `BudgetLimited`).
pub fn mark_complete(state: &mut AppState) -> Result<&SessionGoal> {
    let Some(goal) = state.session_goal.as_mut() else {
        return Err(anyhow!("no goal to mark complete"));
    };
    match goal.status {
        GoalStatus::Active | GoalStatus::Complete => {
            goal.status = GoalStatus::Complete;
            goal.updated_at_ms = unix_time_ms();
        }
        GoalStatus::BudgetLimited => {
            return Err(anyhow!(
                "goal is budget-limited; the user / runtime must intervene before marking complete"
            ));
        }
        GoalStatus::Paused => {
            return Err(anyhow!("goal is paused; resume it before marking complete"));
        }
    }
    Ok(state.session_goal.as_ref().unwrap())
}

/// Per-turn accounting hook. Increments `tokens_used` and, if the
/// budget is now exhausted, transitions `Active` → `BudgetLimited`.
/// Mirrors codex's atomic SQL UPDATE in `account_thread_goal_usage`
/// (`state/src/runtime/goals.rs:357-381`) but in-memory.
///
/// No-op when no goal exists or the goal is non-active. Plan mode
/// is also a no-op (codex parity).
pub fn account_token_usage(state: &mut AppState, usage: &TurnUsageReport) -> Option<GoalStatus> {
    if ignore_goal_for_mode(state) {
        return None;
    }
    let goal = state.session_goal.as_mut()?;
    if !goal.status.is_active() {
        return None;
    }
    let delta = token_delta_from_usage(usage);
    if delta == 0 {
        return Some(goal.status);
    }
    goal.tokens_used = goal.tokens_used.saturating_add(delta);
    goal.updated_at_ms = unix_time_ms();
    if let Some(budget) = goal.token_budget {
        if goal.tokens_used >= budget as u64 {
            goal.status = GoalStatus::BudgetLimited;
        }
    }
    Some(goal.status)
}

/// User-facing (slash command): replace the current goal. Unlike
/// the model-facing `create_goal`, this *always* succeeds — the user
/// is the boss. Preserves the prior `token_budget` when the new
/// invocation didn't pass one (codex parity for in-thread
/// objective updates: `set_thread_goal` accepts each field as
/// optional, leaving omitted fields untouched).
pub fn slash_set_goal(
    state: &mut AppState,
    objective: String,
    token_budget: Option<u32>,
) -> Result<&SessionGoal> {
    let trimmed = objective.trim().to_string();
    if trimmed.is_empty() {
        return Err(anyhow!("objective must not be empty"));
    }
    let now = unix_time_ms();
    let prior = state.session_goal.clone();
    state.session_goal = Some(SessionGoal {
        // Preserve goal_id when the prior was active so observability
        // pivots (future Langfuse goal-trace tag) don't see a phantom
        // new goal on every text update.
        goal_id: prior
            .as_ref()
            .filter(|g| g.status.is_active())
            .map(|g| g.goal_id.clone())
            .unwrap_or_else(new_goal_id),
        objective: trimmed,
        status: GoalStatus::Active,
        token_budget: token_budget.or_else(|| prior.as_ref().and_then(|g| g.token_budget)),
        tokens_used: prior
            .as_ref()
            .filter(|g| g.status.is_active())
            .map(|g| g.tokens_used)
            .unwrap_or(0),
        created_at_ms: prior.as_ref().map(|g| g.created_at_ms).unwrap_or(now),
        updated_at_ms: now,
    });
    Ok(state.session_goal.as_ref().unwrap())
}

/// User-facing (slash command): attach / replace token budget on
/// the active goal. Errors when no goal exists. Setting a budget
/// below the already-consumed tokens flips status to `BudgetLimited`
/// immediately, matching the live-accounting transition.
pub fn slash_set_budget(state: &mut AppState, budget: u32) -> Result<&SessionGoal> {
    if budget == 0 {
        return Err(anyhow!("token budget must be a positive integer"));
    }
    let Some(goal) = state.session_goal.as_mut() else {
        return Err(anyhow!(
            "no active goal; set one with `/goal <objective>` first"
        ));
    };
    goal.token_budget = Some(budget);
    goal.updated_at_ms = unix_time_ms();
    if goal.status.is_active() && goal.tokens_used >= budget as u64 {
        goal.status = GoalStatus::BudgetLimited;
    }
    Ok(state.session_goal.as_ref().unwrap())
}

/// User-facing (slash command): drop the goal record entirely.
/// Idempotent — returns `false` if there was nothing to clear.
pub fn slash_clear_goal(state: &mut AppState) -> bool {
    state.session_goal.take().is_some()
}

/// Wire-shape returned by `get_goal` / `create_goal` / `update_goal`
/// tool calls. Mirrors codex's `GoalToolResponse`
/// (`core/src/tools/handlers/goal.rs:44-50`):
/// `{ goal, remainingTokens, completionBudgetReport }` (camelCase
/// for parity with codex's serde tag — easier for downstream
/// observability tooling that may consume both formats).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalToolEnvelope {
    pub goal: Option<SessionGoalView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_budget_report: Option<String>,
}

/// Externalized view of `SessionGoal` for the wire — same field
/// surface but with camelCase keys and the status as a string
/// (matches codex's `ThreadGoal` projection in `protocol_goal_from_state`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoalView {
    pub goal_id: String,
    pub objective: String,
    pub status: &'static str,
    pub token_budget: Option<u32>,
    pub tokens_used: u64,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

impl From<&SessionGoal> for SessionGoalView {
    fn from(goal: &SessionGoal) -> Self {
        Self {
            goal_id: goal.goal_id.clone(),
            objective: goal.objective.clone(),
            status: goal.status.slug(),
            token_budget: goal.token_budget,
            tokens_used: goal.tokens_used,
            created_at_ms: goal.created_at_ms,
            updated_at_ms: goal.updated_at_ms,
        }
    }
}

/// Build the wire envelope for a tool response. `completion_report`
/// is set only by `update_goal`'s success path and only when the
/// goal had a budget — codex parity (`CompletionBudgetReport::Include`).
pub fn tool_envelope(goal: &SessionGoal, completion_report: Option<String>) -> GoalToolEnvelope {
    GoalToolEnvelope {
        remaining_tokens: goal.remaining_tokens(),
        goal: Some(SessionGoalView::from(goal)),
        completion_budget_report: completion_report,
    }
}

/// Pre-completion budget snapshot. Called by `update_goal` *before*
/// flipping status so the report reflects "tokens consumed during
/// the active goal". Returns `None` when no goal exists or no
/// budget was attached. Codex's analog is in `goal.rs` near the
/// `CompletionBudgetReport::Include` branch.
pub fn completion_budget_report(state: &AppState) -> Option<String> {
    let goal = state.session_goal.as_ref()?;
    let budget = goal.token_budget?;
    Some(format!(
        "Goal achieved. Report final budget usage to the user: tokens used: {} of {}.",
        goal.tokens_used, budget
    ))
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Hex-encoded random token. Not cryptographically meaningful — this
/// is just an ordering / dedup id, mirroring codex's `goal_id` UUID
/// without the uuid-crate dependency. Consumers should treat it
/// opaquely.
fn new_goal_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = unix_time_ms() as u64;
    format!("goal-{now:x}-{counter:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
    use puffer_session_store::SessionStore;
    use tempfile::tempdir;

    fn make_state() -> (AppState, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let paths = ConfigPaths::discover(tmp.path());
        ensure_workspace_dirs(&paths).unwrap();
        let store = SessionStore::from_paths(&paths).unwrap();
        let session = store.create_session(tmp.path().to_path_buf()).unwrap();
        let state = AppState::new(PufferConfig::default(), tmp.path().to_path_buf(), session);
        (state, tmp)
    }

    fn usage(input: u64, output: u64, cached: u64) -> TurnUsageReport {
        TurnUsageReport {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cached,
            cache_creation_tokens: 0,
        }
    }

    #[test]
    fn token_delta_excludes_cached_input() {
        let u = usage(1000, 50, 800);
        assert_eq!(token_delta_from_usage(&u), 250); // (1000 - 800) + 50
    }

    #[test]
    fn token_delta_handles_zero_cached() {
        let u = usage(100, 25, 0);
        assert_eq!(token_delta_from_usage(&u), 125);
    }

    #[test]
    fn create_goal_rejects_when_active_goal_exists() {
        let (mut state, _tmp) = make_state();
        create_goal(&mut state, "first goal".to_string(), None).unwrap();
        let err = create_goal(&mut state, "second goal".to_string(), None).unwrap_err();
        assert!(err.to_string().contains("already has a goal"));
    }

    #[test]
    fn create_goal_allowed_after_complete() {
        let (mut state, _tmp) = make_state();
        create_goal(&mut state, "first".to_string(), None).unwrap();
        mark_complete(&mut state).unwrap();
        // Should now allow a new goal because previous is terminal.
        let g = create_goal(&mut state, "second".to_string(), Some(50_000)).unwrap();
        assert_eq!(g.objective, "second");
        assert_eq!(g.token_budget, Some(50_000));
        assert_eq!(g.status, GoalStatus::Active);
    }

    #[test]
    fn create_goal_blocked_in_plan_mode() {
        let (mut state, _tmp) = make_state();
        state.plan_mode = true;
        let err = create_goal(&mut state, "x".to_string(), None).unwrap_err();
        assert!(err.to_string().contains("plan mode"));
    }

    #[test]
    fn create_goal_rejects_empty_objective() {
        let (mut state, _tmp) = make_state();
        let err = create_goal(&mut state, "  ".to_string(), None).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("empty"));
    }

    #[test]
    fn create_goal_rejects_zero_budget() {
        let (mut state, _tmp) = make_state();
        let err = create_goal(&mut state, "x".to_string(), Some(0)).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("positive"));
    }

    #[test]
    fn account_token_usage_increments_and_trips_budget() {
        let (mut state, _tmp) = make_state();
        create_goal(&mut state, "fit budget".to_string(), Some(1_000)).unwrap();
        // First turn: 250 tokens used → still active.
        let s1 = account_token_usage(&mut state, &usage(300, 50, 100)).unwrap();
        assert_eq!(s1, GoalStatus::Active);
        assert_eq!(state.session_goal.as_ref().unwrap().tokens_used, 250);
        // Second turn: pushes over the budget → BudgetLimited.
        let s2 = account_token_usage(&mut state, &usage(900, 100, 100)).unwrap();
        assert_eq!(s2, GoalStatus::BudgetLimited);
        let g = state.session_goal.as_ref().unwrap();
        assert_eq!(g.tokens_used, 1_150);
        assert_eq!(g.status, GoalStatus::BudgetLimited);
    }

    #[test]
    fn account_token_usage_noops_when_no_budget() {
        let (mut state, _tmp) = make_state();
        create_goal(&mut state, "open ended".to_string(), None).unwrap();
        let s = account_token_usage(&mut state, &usage(10_000, 5_000, 0)).unwrap();
        assert_eq!(s, GoalStatus::Active);
        assert_eq!(state.session_goal.as_ref().unwrap().tokens_used, 15_000);
    }

    #[test]
    fn account_token_usage_skips_terminal_goals() {
        let (mut state, _tmp) = make_state();
        create_goal(&mut state, "x".to_string(), None).unwrap();
        mark_complete(&mut state).unwrap();
        let s = account_token_usage(&mut state, &usage(1_000, 1_000, 0));
        assert!(s.is_none());
        assert_eq!(state.session_goal.as_ref().unwrap().tokens_used, 0);
    }

    #[test]
    fn account_token_usage_skips_in_plan_mode() {
        let (mut state, _tmp) = make_state();
        // Pre-create the goal before flipping plan_mode (creation
        // fails in plan mode by design).
        create_goal(&mut state, "x".to_string(), None).unwrap();
        state.plan_mode = true;
        assert!(account_token_usage(&mut state, &usage(500, 500, 0)).is_none());
        assert_eq!(state.session_goal.as_ref().unwrap().tokens_used, 0);
    }

    #[test]
    fn mark_complete_rejects_budget_limited() {
        let (mut state, _tmp) = make_state();
        create_goal(&mut state, "x".to_string(), Some(10)).unwrap();
        // Trip the budget.
        account_token_usage(&mut state, &usage(20, 0, 0));
        assert_eq!(
            state.session_goal.as_ref().unwrap().status,
            GoalStatus::BudgetLimited
        );
        let err = mark_complete(&mut state).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("budget"));
    }

    #[test]
    fn slash_set_goal_replaces_active_and_preserves_budget() {
        let (mut state, _tmp) = make_state();
        slash_set_goal(&mut state, "first".to_string(), Some(50_000)).unwrap();
        let id1 = state.session_goal.as_ref().unwrap().goal_id.clone();
        // Update text without budget — budget carries forward.
        slash_set_goal(&mut state, "second".to_string(), None).unwrap();
        let g = state.session_goal.as_ref().unwrap();
        assert_eq!(g.objective, "second");
        assert_eq!(g.token_budget, Some(50_000));
        assert_eq!(
            g.goal_id, id1,
            "goal_id stays stable on active in-place updates"
        );
    }

    #[test]
    fn slash_set_goal_replaces_terminal_with_fresh_id() {
        let (mut state, _tmp) = make_state();
        slash_set_goal(&mut state, "first".to_string(), Some(100)).unwrap();
        let id1 = state.session_goal.as_ref().unwrap().goal_id.clone();
        // Trip budget, then replace.
        account_token_usage(&mut state, &usage(100, 0, 0));
        slash_set_goal(&mut state, "second".to_string(), Some(200)).unwrap();
        let g = state.session_goal.as_ref().unwrap();
        assert_ne!(g.goal_id, id1, "replacing a terminal goal mints fresh id");
        assert_eq!(g.tokens_used, 0, "tokens reset for fresh goal");
    }

    #[test]
    fn slash_set_budget_below_consumed_trips_budget_limited() {
        let (mut state, _tmp) = make_state();
        create_goal(&mut state, "x".to_string(), None).unwrap();
        account_token_usage(&mut state, &usage(500, 0, 0));
        slash_set_budget(&mut state, 100).unwrap();
        assert_eq!(
            state.session_goal.as_ref().unwrap().status,
            GoalStatus::BudgetLimited
        );
    }

    #[test]
    fn slash_clear_goal_idempotent() {
        let (mut state, _tmp) = make_state();
        assert!(!slash_clear_goal(&mut state));
        create_goal(&mut state, "x".to_string(), None).unwrap();
        assert!(slash_clear_goal(&mut state));
        assert!(state.session_goal.is_none());
        assert!(!slash_clear_goal(&mut state));
    }

    #[test]
    fn remaining_tokens_clamps_at_zero() {
        let g = SessionGoal {
            goal_id: "g-test".to_string(),
            objective: "x".to_string(),
            status: GoalStatus::BudgetLimited,
            token_budget: Some(100),
            tokens_used: 200,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        assert_eq!(g.remaining_tokens(), Some(0));
    }
}
