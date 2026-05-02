use crate::model_policy::ModelProposalViolation;
use crate::runtime::model_loop_support::compact_history_value;
use crate::runtime::model_loop_support::ModelCandidateProposalOutcome;
use serde_json::{json, Value};

/// Builds retry context after a model submitted invalid structural candidates.
pub(super) fn invalid_candidate_context(
    previous_context: &Value,
    stage: &str,
    violations: &[ModelProposalViolation],
) -> Value {
    json!({
        "phase": "after_invalid_model_candidates",
        "previous_context": compact_history_value(previous_context),
        "invalid_prior_candidates": invalid_candidate_feedback(stage, violations),
    })
}

/// Builds recovery detail after invalid structural candidates exhausted retries.
pub(super) fn invalid_candidate_detail(
    previous_context: Value,
    stage: &str,
    violations: &[ModelProposalViolation],
) -> Value {
    json!({
        "previous_context": compact_history_value(&previous_context),
        "invalid_prior_candidates": invalid_candidate_feedback(stage, violations),
    })
}

fn invalid_candidate_feedback(stage: &str, violations: &[ModelProposalViolation]) -> Vec<Value> {
    if violations.is_empty() {
        return vec![json!({
            "stage": stage,
            "code": "candidate_structural_validation_failed",
            "repair_hint": "return a raw JSON object only, with no markdown fences or prose, containing valid contract-gated candidates with bounded executable args",
        })];
    }
    violations
        .iter()
        .map(|violation| {
            json!({
                "stage": stage,
                "code": violation.code,
                "path": violation.path,
                "tool_id": violation.tool_id,
                "actual": violation.actual,
                "limit": violation.limit,
                "repair_hint": format!(
                    "{}; return a raw JSON object only, with no markdown fences or prose",
                    violation.repair_hint
                ),
            })
        })
        .collect()
}

/// Builds retry context after a model submitted candidates that added no new work.
pub(super) fn non_advancing_candidate_context(
    previous_context: &Value,
    stage: &str,
    outcome: &ModelCandidateProposalOutcome,
) -> Value {
    json!({
        "phase": "after_non_advancing_model_candidates",
        "previous_context": compact_history_value(previous_context),
        "non_advancing_prior_candidates": non_advancing_candidate_feedback(stage, outcome),
    })
}

/// Marks a model context as requiring candidates that can advance beyond support.
pub(super) fn advancing_candidate_role_context(previous_context: &Value, reason: &str) -> Value {
    json!({
        "phase": "after_support_exhausted",
        "previous_context": compact_history_value(previous_context),
        "required_completion_roles": ["repair", "verification", "terminal"],
        "forbidden_completion_roles": ["support"],
        "role_requirement_reason": reason,
        "repair_hint": "support observations have not produced new executable work in this recovery context; propose repair, verification, or terminal candidates with explicit roles",
    })
}

/// Marks a model context as requiring state-changing repair or terminal completion.
pub(super) fn advancing_state_candidate_role_context(
    previous_context: &Value,
    reason: &str,
) -> Value {
    json!({
        "phase": "after_observation_saturated",
        "previous_context": compact_history_value(previous_context),
        "required_completion_roles": ["repair", "terminal"],
        "forbidden_completion_roles": ["support", "verification"],
        "role_requirement_reason": reason,
        "repair_hint": "recent model-authored observation/probe actions did not change state or create artifacts; propose a concrete repair action that changes local state, or a terminal action if existing evidence is enough",
    })
}

/// Builds recovery detail after duplicate or otherwise non-advancing candidates.
pub(super) fn non_advancing_candidate_detail(
    previous_context: Value,
    stage: &str,
    outcome: &ModelCandidateProposalOutcome,
) -> Value {
    json!({
        "previous_context": compact_history_value(&previous_context),
        "non_advancing_prior_candidates": non_advancing_candidate_feedback(stage, outcome),
    })
}

fn non_advancing_candidate_feedback(
    stage: &str,
    outcome: &ModelCandidateProposalOutcome,
) -> Vec<Value> {
    vec![json!({
        "stage": stage,
        "code": "no_new_model_candidates",
        "proposed": outcome.proposed,
        "added": outcome.added,
        "skipped_wait": outcome.skipped_wait,
        "skipped_duplicate": outcome.skipped_duplicate,
        "skipped_dependency": outcome.skipped_dependency,
        "skipped_policy": outcome.skipped_policy,
        "duplicate_candidates": outcome.skipped_duplicate_candidates.clone(),
        "policy_skipped_candidates": outcome.skipped_policy_candidates.clone(),
        "repair_hint": "propose only actions absent from frontier/history, or return a verification/terminal candidate if existing evidence is enough; do not repeat successful checks unless state changed",
    })]
}
