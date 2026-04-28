use crate::contract::{CapabilityContract, FailureModeSpec, RiskLevel};
use crate::ids::{MutationId, TraceId};
use crate::trace::{TraceEvent, TraceEventType};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MutationType {
    ContractPatch,
    ContractOverlay,
    PromptPatch,
    RewriteRulePatch,
    SchedulerWeightPatch,
    MacroContract,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MutationStatus {
    Candidate,
    Rejected,
    Evaluated,
    Shadow,
    Canary,
    Promoted,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MutationCandidate {
    pub(crate) mutation_id: MutationId,
    pub(crate) mutation_type: MutationType,
    pub(crate) target: String,
    pub(crate) patch: Value,
    pub(crate) rationale: String,
    pub(crate) source_trace_ids: Vec<TraceId>,
    pub(crate) status: MutationStatus,
    pub(crate) estimated_risk: RiskLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContractPatchOp {
    AddFailureMode {
        action_name: String,
        failure_mode: FailureModeSpec,
    },
    StrengthenPrecondition {
        action_name: String,
        precondition: String,
    },
    AddVerificationMethod {
        action_name: String,
        method: String,
    },
    IncreaseRisk {
        action_name: String,
        new_risk: RiskLevel,
    },
    RequireApproval {
        action_name: String,
        reason: String,
    },
    AddForbiddenUse {
        action_name: Option<String>,
        forbidden_use: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub(crate) struct EvalComparisonReport {
    pub(crate) safety_violations: u64,
    pub(crate) protected_policy_weakened: bool,
    pub(crate) success_delta: f64,
    pub(crate) cost_delta: f64,
    pub(crate) recovery_delta: f64,
    pub(crate) cost_multiplier: f64,
    pub(crate) latency_multiplier: f64,
}

pub(crate) fn propose_mutations(events: &[TraceEvent]) -> Vec<MutationCandidate> {
    let failures = events
        .iter()
        .filter(|event| event.event_type == TraceEventType::FailureClassified)
        .collect::<Vec<_>>();
    let mut candidates = Vec::new();
    if let Some(failure) = failures.first() {
        let action_name = failure
            .action_name
            .as_deref()
            .or_else(|| failure.input.get("action_name").and_then(Value::as_str))
            .or_else(|| failure.output.get("action_name").and_then(Value::as_str))
            .unwrap_or("unknown_action");
        let failure_mode = failure
            .failure_mode
            .as_deref()
            .or_else(|| failure.output.get("failure_mode").and_then(Value::as_str))
            .unwrap_or("unknown");
        let detection = failure
            .output
            .get("detection")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("trace classified failure as {failure_mode}"));
        let repair_strategy = failure
            .output
            .get("retry_safety")
            .and_then(Value::as_str)
            .map(|safety| format!("follow trace safety policy `{safety}` before retrying"))
            .unwrap_or_else(|| {
                "choose a declared diagnostic observation before retrying".to_string()
            });
        let patch = ContractPatchOp::AddFailureMode {
            action_name: action_name.to_string(),
            failure_mode: FailureModeSpec {
                name: failure_mode.to_string(),
                detection,
                repair_strategy,
                confidence: 0.6,
            },
        };
        candidates.push(MutationCandidate {
            mutation_id: MutationId::new(),
            mutation_type: MutationType::ContractPatch,
            target: failure
                .contract_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            patch: serde_json::to_value(patch).unwrap_or_else(|_| json!({})),
            rationale: format!(
                "Trace classified `{action_name}` as `{failure_mode}`; promote that observed failure semantics only after evaluation."
            ),
            source_trace_ids: failures.iter().map(|event| event.trace_id).collect(),
            status: MutationStatus::Candidate,
            estimated_risk: RiskLevel::Low,
        });
    }
    candidates
}

pub(crate) fn apply_contract_patch(
    contract: &mut CapabilityContract,
    patch: &ContractPatchOp,
) -> Result<()> {
    match patch {
        ContractPatchOp::AddFailureMode {
            action_name,
            failure_mode,
        } => action_mut(contract, action_name)?
            .failure_modes
            .push(failure_mode.clone()),
        ContractPatchOp::StrengthenPrecondition {
            action_name,
            precondition,
        } => action_mut(contract, action_name)?
            .preconditions
            .push(precondition.clone()),
        ContractPatchOp::AddVerificationMethod {
            action_name,
            method,
        } => {
            let action = action_mut(contract, action_name)?;
            action.verification.methods.push(method.clone());
            action.verification.required_before_completion = true;
        }
        ContractPatchOp::IncreaseRisk {
            action_name,
            new_risk,
        } => {
            let action = action_mut(contract, action_name)?;
            if *new_risk < action.risk_level {
                return Err(anyhow!("contract patch cannot decrease risk"));
            }
            action.risk_level = new_risk.clone();
        }
        ContractPatchOp::RequireApproval {
            action_name,
            reason,
        } => {
            let action = action_mut(contract, action_name)?;
            action.approval.required = true;
            action.approval.reason = Some(reason.clone());
        }
        ContractPatchOp::AddForbiddenUse {
            action_name,
            forbidden_use,
        } => match action_name {
            Some(action_name) => action_mut(contract, action_name)?
                .forbidden_uses
                .push(forbidden_use.clone()),
            None => contract.forbidden_uses.push(forbidden_use.clone()),
        },
    }
    Ok(())
}

pub(crate) fn patch_is_safe(patch: &ContractPatchOp) -> bool {
    !matches!(
        patch,
        ContractPatchOp::IncreaseRisk { .. }
            if false
    )
}

pub(crate) fn should_promote(report: &EvalComparisonReport) -> bool {
    if report.safety_violations > 0 {
        return false;
    }
    if report.protected_policy_weakened {
        return false;
    }
    let improves_success = report.success_delta >= 0.03;
    let improves_cost = report.cost_delta <= -0.10;
    let improves_recovery = report.recovery_delta >= 0.05;
    let bounded_regression = report.cost_multiplier <= 1.25 && report.latency_multiplier <= 1.30;
    bounded_regression && (improves_success || improves_cost || improves_recovery)
}

fn action_mut<'a>(
    contract: &'a mut CapabilityContract,
    action_name: &str,
) -> Result<&'a mut crate::contract::ActionContract> {
    contract
        .actions
        .iter_mut()
        .find(|action| action.name == action_name)
        .ok_or_else(|| anyhow!("missing action {action_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ActionContract, ApprovalSpec, ContractStatus, Idempotency, Reversibility, SideEffectClass,
        TrustLevel, VerificationSpec,
    };

    fn contract() -> CapabilityContract {
        CapabilityContract {
            contract_id: "puffer.tools".to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "Puffer tools".to_string(),
            actions: vec![ActionContract {
                name: "Read".to_string(),
                description: "read through Puffer".to_string(),
                input_schema: json!({"type": "object"}),
                output_schema: json!({"type": "object"}),
                side_effect_class: SideEffectClass::LocalRead,
                reversibility: Reversibility::Reversible,
                idempotency: Idempotency::Idempotent,
                risk_level: RiskLevel::Low,
                preconditions: Vec::new(),
                postconditions: Vec::new(),
                verification: VerificationSpec {
                    methods: Vec::new(),
                    templates: Vec::new(),
                    required_before_completion: false,
                    confidence: 0.5,
                },
                approval: ApprovalSpec {
                    required: false,
                    reason: None,
                },
                failure_modes: Vec::new(),
                forbidden_uses: Vec::new(),
                argument_safety: Vec::new(),
                semantic_intents: Vec::new(),
                intent_extractors: Vec::new(),
                repair_rules: Vec::new(),
                cost_estimate: None,
                latency_estimate: None,
            }],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        }
    }

    #[test]
    fn contract_patch_adds_verification_method() {
        let mut contract = contract();
        apply_contract_patch(
            &mut contract,
            &ContractPatchOp::AddVerificationMethod {
                action_name: "Read".to_string(),
                method: "confirm content hash".to_string(),
            },
        )
        .unwrap();
        assert!(contract.actions[0].verification.required_before_completion);
        assert_eq!(
            contract.actions[0].verification.methods[0],
            "confirm content hash"
        );
    }

    #[test]
    fn contract_patch_cannot_decrease_risk() {
        let mut contract = contract();
        contract.actions[0].risk_level = RiskLevel::High;
        let result = apply_contract_patch(
            &mut contract,
            &ContractPatchOp::IncreaseRisk {
                action_name: "Read".to_string(),
                new_risk: RiskLevel::Low,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn promotion_gate_rejects_safety_violations() {
        assert!(!should_promote(&EvalComparisonReport {
            safety_violations: 1,
            success_delta: 1.0,
            cost_multiplier: 1.0,
            latency_multiplier: 1.0,
            ..EvalComparisonReport::default()
        }));
    }

    #[test]
    fn promotion_gate_accepts_bounded_success_improvement() {
        assert!(should_promote(&EvalComparisonReport {
            success_delta: 0.04,
            cost_multiplier: 1.0,
            latency_multiplier: 1.0,
            ..EvalComparisonReport::default()
        }));
    }
}
