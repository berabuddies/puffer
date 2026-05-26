//! Workflow handlers for the model-facing goal tools.
//!
//! Three thin shells — `get_goal` / `create_goal` / `update_goal` —
//! each delegating to `runtime::goals` for state mutation. Mirrors
//! codex's `core/src/tools/handlers/goal.rs` shape: tool inputs are
//! parsed into typed structs, errors are surfaced as `RespondToModel`
//! (model can recover) rather than fatal aborts.
//!
//! The slash command `/goal` shares the same `runtime::goals` module
//! so all three surfaces (slash, model tools, future external API)
//! agree on the 4-state machine, token accounting, plan-mode
//! exclusion, and goal_id continuity.

use crate::runtime::goals::{self, GoalToolEnvelope};
use crate::AppState;
use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

/// `get_goal` is read-only — no fields. Defined as `#[serde(deny_unknown_fields)]`
/// so a model that adds spurious args gets an early, actionable error
/// (codex parity).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GetGoalArgs {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateGoalArgs {
    objective: String,
    #[serde(default)]
    token_budget: Option<u32>,
}

/// `update_goal` accepts only `status: "complete"`. The serde
/// deserializer enforces this by deserializing `status` to a marker
/// enum with a single variant — the JSON schema also restricts it,
/// so this is defense-in-depth (codex parity).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateGoalArgs {
    status: UpdateGoalStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum UpdateGoalStatus {
    Complete,
}

pub fn execute_get_goal(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    parse::<GetGoalArgs>(&input, "get_goal")?;
    let envelope = goals::get_goal(state)
        .map(|goal| goals::tool_envelope(goal, None))
        .unwrap_or_else(|| GoalToolEnvelope {
            goal: None,
            remaining_tokens: None,
            completion_budget_report: None,
        });
    Ok(serde_json::to_string_pretty(&envelope)?)
}

pub fn execute_create_goal(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let args = parse::<CreateGoalArgs>(&input, "create_goal")?;
    let goal = goals::create_goal(state, args.objective, args.token_budget)?;
    let envelope = goals::tool_envelope(goal, None);
    Ok(serde_json::to_string_pretty(&envelope)?)
}

pub fn execute_update_goal(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let args = parse::<UpdateGoalArgs>(&input, "update_goal")?;
    // The enum guarantees `status: complete` — pattern-match
    // exhaustively so the day someone adds a variant we get a
    // compile-time nudge to revisit the contract.
    match args.status {
        UpdateGoalStatus::Complete => {
            let report = goals::completion_budget_report(state);
            let goal = goals::mark_complete(state)?;
            let envelope = goals::tool_envelope(goal, report);
            Ok(serde_json::to_string_pretty(&envelope)?)
        }
    }
}

fn parse<T: for<'de> Deserialize<'de>>(input: &Value, tool: &str) -> Result<T> {
    serde_json::from_value::<T>(if input.is_null() {
        json!({})
    } else {
        input.clone()
    })
    .map_err(|error| anyhow!("invalid {tool} input: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::goals;
    use crate::state::GoalStatus;
    use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
    use puffer_session_store::SessionStore;
    use serde_json::json;
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

    #[test]
    fn get_goal_returns_null_envelope_when_no_goal() {
        let (mut state, tmp) = make_state();
        let result = execute_get_goal(&mut state, tmp.path(), Value::Null).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["goal"], Value::Null);
    }

    #[test]
    fn create_goal_returns_envelope_with_active_status() {
        let (mut state, tmp) = make_state();
        let result = execute_create_goal(
            &mut state,
            tmp.path(),
            json!({"objective": "ship the goal feature", "token_budget": 50000}),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["goal"]["objective"], "ship the goal feature");
        assert_eq!(parsed["goal"]["status"], "active");
        assert_eq!(parsed["goal"]["tokenBudget"], 50000);
        assert_eq!(parsed["goal"]["tokensUsed"], 0);
        assert_eq!(parsed["remainingTokens"], 50000);
        assert_eq!(parsed.get("completionBudgetReport"), None); // omitted via skip_serializing
    }

    #[test]
    fn create_goal_fails_when_one_already_active() {
        let (mut state, tmp) = make_state();
        execute_create_goal(&mut state, tmp.path(), json!({"objective": "first"})).unwrap();
        let err = execute_create_goal(&mut state, tmp.path(), json!({"objective": "second"}))
            .unwrap_err();
        assert!(err.to_string().contains("already has a goal"));
    }

    #[test]
    fn update_goal_marks_complete_and_emits_budget_report() {
        let (mut state, tmp) = make_state();
        execute_create_goal(
            &mut state,
            tmp.path(),
            json!({"objective": "x", "token_budget": 1_000}),
        )
        .unwrap();
        // Pretend we used some tokens.
        let usage = crate::runtime::TurnUsageReport {
            input_tokens: 200,
            output_tokens: 50,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        goals::account_token_usage(&mut state, &usage);

        let result =
            execute_update_goal(&mut state, tmp.path(), json!({"status": "complete"})).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["goal"]["status"], "complete");
        assert!(parsed["completionBudgetReport"]
            .as_str()
            .unwrap()
            .contains("250 of 1000"));
    }

    #[test]
    fn update_goal_no_budget_report_when_no_budget() {
        let (mut state, tmp) = make_state();
        execute_create_goal(&mut state, tmp.path(), json!({"objective": "x"})).unwrap();
        let result =
            execute_update_goal(&mut state, tmp.path(), json!({"status": "complete"})).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["goal"]["status"], "complete");
        assert!(parsed.get("completionBudgetReport").is_none());
    }

    #[test]
    fn update_goal_rejects_non_complete_status_via_serde() {
        let (mut state, tmp) = make_state();
        execute_create_goal(&mut state, tmp.path(), json!({"objective": "x"})).unwrap();
        let err =
            execute_update_goal(&mut state, tmp.path(), json!({"status": "active"})).unwrap_err();
        assert!(err
            .to_string()
            .to_ascii_lowercase()
            .contains("invalid update_goal input"));
    }

    #[test]
    fn create_goal_rejects_unknown_field() {
        let (mut state, tmp) = make_state();
        let err = execute_create_goal(
            &mut state,
            tmp.path(),
            json!({"objective": "x", "deadline": "tomorrow"}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid create_goal input"));
    }

    #[test]
    fn get_goal_status_after_budget_trip() {
        let (mut state, tmp) = make_state();
        execute_create_goal(
            &mut state,
            tmp.path(),
            json!({"objective": "x", "token_budget": 100}),
        )
        .unwrap();
        let usage = crate::runtime::TurnUsageReport {
            input_tokens: 200,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        };
        goals::account_token_usage(&mut state, &usage);
        // Verify status flipped, then look it up via the tool.
        assert_eq!(
            state.session_goal.as_ref().unwrap().status,
            GoalStatus::BudgetLimited
        );
        let result = execute_get_goal(&mut state, tmp.path(), Value::Null).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["goal"]["status"], "budget_limited");
        assert_eq!(parsed["remainingTokens"], 0);
    }
}
