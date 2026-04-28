use crate::evolve::{
    should_promote, EvalComparisonReport, MutationCandidate, MutationStatus, MutationType,
};
use crate::ids::MutationId;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct PromotionRecord {
    pub(crate) mutation_id: MutationId,
    pub(crate) mutation_type: MutationType,
    pub(crate) target: String,
    pub(crate) status: MutationStatus,
    pub(crate) decision: PromotionDecision,
    pub(crate) comparison: EvalComparisonReport,
    pub(crate) rollback_pointer: Option<Value>,
    pub(crate) artifact: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PromotionDecision {
    Reject,
    KeepCandidate,
    EnterShadow,
    EnterCanary,
    Promote,
}

pub(crate) fn evaluate_candidate(
    candidate: &MutationCandidate,
    comparison: EvalComparisonReport,
) -> PromotionRecord {
    let decision = if should_promote(&comparison) {
        PromotionDecision::EnterShadow
    } else {
        PromotionDecision::Reject
    };
    let status = match decision {
        PromotionDecision::EnterShadow => MutationStatus::Shadow,
        PromotionDecision::Reject => MutationStatus::Rejected,
        _ => MutationStatus::Evaluated,
    };
    promotion_record(candidate, comparison, decision, status, None)
}

pub(crate) fn advance_promotion(record: &PromotionRecord) -> PromotionRecord {
    let (decision, status) = match record.status.clone() {
        MutationStatus::Shadow => (PromotionDecision::EnterCanary, MutationStatus::Canary),
        MutationStatus::Canary => (PromotionDecision::Promote, MutationStatus::Promoted),
        MutationStatus::Promoted => (PromotionDecision::Promote, MutationStatus::Promoted),
        MutationStatus::Rejected => (PromotionDecision::Reject, MutationStatus::Rejected),
        _ => {
            if should_promote(&record.comparison) {
                (PromotionDecision::EnterShadow, MutationStatus::Shadow)
            } else {
                (PromotionDecision::Reject, MutationStatus::Rejected)
            }
        }
    };
    promotion_record(
        &MutationCandidate {
            mutation_id: record.mutation_id,
            mutation_type: record.mutation_type.clone(),
            target: record.target.clone(),
            patch: record.artifact["patch"].clone(),
            rationale: record.artifact["rationale"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            source_trace_ids: Vec::new(),
            status: status.clone(),
            estimated_risk: record
                .artifact
                .get("estimated_risk")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or(crate::contract::RiskLevel::Low),
        },
        record.comparison.clone(),
        decision,
        status,
        record.rollback_pointer.clone(),
    )
}

pub(crate) fn write_promotion_record(
    workspace_root: &Path,
    record: &PromotionRecord,
) -> Result<PathBuf> {
    let dir = promotion_dir(workspace_root);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create promotion dir {}", dir.display()))?;
    let path = dir.join(format!("{}.json", record.mutation_id.0));
    fs::write(&path, serde_json::to_string_pretty(record)?)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub(crate) fn promotion_dir(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".puffer")
        .join("burbot")
        .join("promotions")
}

fn promotion_record(
    candidate: &MutationCandidate,
    comparison: EvalComparisonReport,
    decision: PromotionDecision,
    status: MutationStatus,
    rollback_pointer: Option<Value>,
) -> PromotionRecord {
    PromotionRecord {
        mutation_id: candidate.mutation_id,
        mutation_type: candidate.mutation_type.clone(),
        target: candidate.target.clone(),
        status,
        decision,
        comparison,
        rollback_pointer,
        artifact: json!({
            "patch": candidate.patch,
            "rationale": candidate.rationale,
            "estimated_risk": candidate.estimated_risk,
            "promotion_boundary": "burbot_local_overlay_only",
        }),
    }
}

pub(crate) fn candidate_from_json(value: Value) -> Result<MutationCandidate> {
    if let Some(array) = value.as_array() {
        let Some(first) = array.first() else {
            anyhow::bail!("candidate array is empty");
        };
        return serde_json::from_value(first.clone()).context("failed to parse mutation candidate");
    }
    serde_json::from_value(value).context("failed to parse mutation candidate")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::RiskLevel;
    use crate::evolve::MutationType;

    fn candidate() -> MutationCandidate {
        MutationCandidate {
            mutation_id: MutationId::new(),
            mutation_type: MutationType::ContractPatch,
            target: "puffer.tools".to_string(),
            patch: json!({"op": "add_failure_mode"}),
            rationale: "test".to_string(),
            source_trace_ids: Vec::new(),
            status: MutationStatus::Candidate,
            estimated_risk: RiskLevel::Low,
        }
    }

    #[test]
    fn evaluation_enters_shadow_when_gate_passes() {
        let record = evaluate_candidate(
            &candidate(),
            EvalComparisonReport {
                success_delta: 0.05,
                cost_multiplier: 1.0,
                latency_multiplier: 1.0,
                ..EvalComparisonReport::default()
            },
        );

        assert_eq!(record.status, MutationStatus::Shadow);
        assert_eq!(record.decision, PromotionDecision::EnterShadow);
        assert_eq!(
            record.artifact["promotion_boundary"],
            "burbot_local_overlay_only"
        );
    }

    #[test]
    fn evaluation_rejects_safety_violations() {
        let record = evaluate_candidate(
            &candidate(),
            EvalComparisonReport {
                safety_violations: 1,
                success_delta: 1.0,
                cost_multiplier: 1.0,
                latency_multiplier: 1.0,
                ..EvalComparisonReport::default()
            },
        );

        assert_eq!(record.status, MutationStatus::Rejected);
        assert_eq!(record.decision, PromotionDecision::Reject);
    }
}
