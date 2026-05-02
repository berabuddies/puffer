use crate::belief::BeliefGraph;
use crate::contract::ContractRegistry;
use crate::failure::FailureKind;
use crate::graph::{ActionRef, PlanEdgeKind, PlanGraph, PlanNode, PlanStatus};
use crate::ids::NodeId;
use crate::planner::{CandidateSource, CompletionRole};
use crate::puffer_tools::PUFFER_TOOLS_CONTRACT_ID;
use serde_json::{json, Value};
use std::path::PathBuf;

use super::{RunOptions, VerificationOutcome};

pub(super) fn plan_key(source: &CandidateSource) -> &'static str {
    match source {
        CandidateSource::ExplicitUserTool => "explicit_user_tool",
        CandidateSource::ModelProposal => "model_proposal",
        CandidateSource::ModelObservationProposal => "model_observation_proposal",
        CandidateSource::ModelGoalVerifier => "model_goal_verifier",
        CandidateSource::ContractGoal => "contract_goal",
        CandidateSource::CheapObservation => "observe_first",
        CandidateSource::GuardedSubstitution => "guarded_substitution",
        CandidateSource::Repair => "repair_observation",
        CandidateSource::Verification => "verification",
        CandidateSource::SymbolicProposal => "symbolic_proposal",
        CandidateSource::SymbolicCritic => "symbolic_critic",
        CandidateSource::SymbolicVerifier => "symbolic_verifier",
        CandidateSource::SynthesizedPlan => "synthesized_plan",
    }
}

pub(super) fn plan_label(source: &CandidateSource) -> &'static str {
    match source {
        CandidateSource::ExplicitUserTool => "explicit tool request",
        CandidateSource::ModelProposal => "model proposal",
        CandidateSource::ModelObservationProposal => "model observation proposal",
        CandidateSource::ModelGoalVerifier => "model goal verifier",
        CandidateSource::ContractGoal => "contract-declared goal action",
        CandidateSource::CheapObservation => "observe before acting",
        CandidateSource::GuardedSubstitution => "guarded safer substitute",
        CandidateSource::Repair => "repair observation",
        CandidateSource::Verification => "verify before completion",
        CandidateSource::SymbolicProposal => "symbolic proposal",
        CandidateSource::SymbolicCritic => "symbolic critique",
        CandidateSource::SymbolicVerifier => "symbolic verification",
        CandidateSource::SynthesizedPlan => "symbolic worker reviewed plan",
    }
}

pub(super) fn plan_rationale(source: &CandidateSource) -> &'static str {
    match source {
        CandidateSource::ExplicitUserTool => "caller supplied a concrete Puffer tool invocation",
        CandidateSource::ModelProposal => "model supplied a validated structural tool proposal",
        CandidateSource::ModelObservationProposal => {
            "model supplied a validated structural follow-up proposal from observations"
        }
        CandidateSource::ModelGoalVerifier => {
            "model supplied a validated structural proposal after goal verification"
        }
        CandidateSource::ContractGoal => "contract goal rules supplied this candidate",
        CandidateSource::CheapObservation => "low-risk observations can reduce uncertainty first",
        CandidateSource::GuardedSubstitution => {
            "dedicated read-only tools can replace matching shell observations"
        }
        CandidateSource::Repair => "failed actions should be diagnosed before retry",
        CandidateSource::Verification => "declared contracts require evidence before completion",
        CandidateSource::SymbolicProposal => "symbolic worker proposed guarded plan alternatives",
        CandidateSource::SymbolicCritic => "symbolic worker critiqued plan risk and verification",
        CandidateSource::SymbolicVerifier => "symbolic worker verified plan metadata",
        CandidateSource::SynthesizedPlan => {
            "proposal, critique, and verification workers gate action selection"
        }
    }
}

pub(super) fn satisfy_dependent_preconditions(
    contracts: &dyn ContractRegistry,
    graph: &PlanGraph,
    beliefs: &mut BeliefGraph,
    observation_action_id: NodeId,
    output: &Value,
) {
    for edge in graph.edges.iter().filter(|edge| {
        edge.source == observation_action_id && edge.kind == PlanEdgeKind::PreconditionFor
    }) {
        let Some(action_ref) = graph
            .nodes
            .get(&edge.target)
            .and_then(|node| node.action_ref.as_ref())
        else {
            continue;
        };
        let Some(action) = contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
        else {
            continue;
        };
        for precondition in action.preconditions {
            beliefs.satisfy_precondition(
                precondition,
                "supporting observation satisfied dependent action precondition",
                output.clone(),
            );
        }
    }
}

/// Satisfies selected action preconditions from already-executed graph precondition edges.
pub(super) fn satisfy_selected_preconditions_from_graph(
    contracts: &dyn ContractRegistry,
    graph: &PlanGraph,
    selected: &PlanNode,
    beliefs: &mut BeliefGraph,
) {
    if !graph.edges.iter().any(|edge| {
        edge.target == selected.id
            && edge.kind == PlanEdgeKind::PreconditionFor
            && graph
                .nodes
                .get(&edge.source)
                .is_some_and(precondition_source_satisfied)
    }) {
        return;
    }
    let Some(action_ref) = selected.action_ref.as_ref() else {
        return;
    };
    let Some(action) = contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
    else {
        return;
    };
    for precondition in action.preconditions {
        beliefs.satisfy_precondition(
            precondition,
            "executed graph precondition satisfies selected action precondition",
            serde_json::json!({"action": action.name}),
        );
    }
}

fn precondition_source_satisfied(node: &PlanNode) -> bool {
    matches!(node.status, PlanStatus::Executed | PlanStatus::Satisfied)
}

pub(super) fn parse_completion_role(value: &str) -> Option<CompletionRole> {
    match value {
        "terminal" => Some(CompletionRole::Terminal),
        "support" => Some(CompletionRole::Support),
        "verification" => Some(CompletionRole::Verification),
        "repair" => Some(CompletionRole::Repair),
        _ => None,
    }
}

pub(super) fn explicit_puffer_action(action_ref: &ActionRef, options: &RunOptions) -> bool {
    action_ref.contract_id == PUFFER_TOOLS_CONTRACT_ID
        && options.puffer_tool.as_deref() == Some(action_ref.action_name.as_str())
        && options.puffer_tool_source == CandidateSource::ExplicitUserTool
}

pub(super) fn failed_terminal_completion_allowed(
    graph: &PlanGraph,
    node_id: NodeId,
    options: &RunOptions,
) -> bool {
    options.allow_failed_terminal_completion
        && graph
            .edges
            .iter()
            .find(|edge| edge.target == node_id && edge.kind == PlanEdgeKind::Supports)
            .and_then(|edge| edge.payload.get("completion_role").and_then(Value::as_str))
            .and_then(parse_completion_role)
            == Some(CompletionRole::Terminal)
}

pub(super) fn failure_completion_artifact(
    action_ref: &ActionRef,
    output: &Value,
    failure_kind: &FailureKind,
) -> Value {
    json!({
        "completed": true,
        "expected_failure": true,
        "contract_id": action_ref.contract_id,
        "action_name": action_ref.action_name,
        "failure_mode": failure_kind.as_str(),
        "verification_required": true,
        "verification_passed": true,
        "verification_evidence": output,
        "output": output,
    })
}

pub(super) fn success_artifact(
    action_ref: &ActionRef,
    output: &Value,
    verification: &VerificationOutcome,
) -> Value {
    json!({
        "completed": true,
        "contract_id": action_ref.contract_id,
        "action_name": action_ref.action_name,
        "verification_required": verification.required,
        "verification_passed": verification.passed,
        "verification_evidence": verification.required.then(|| output.clone()),
        "output": output,
    })
}

pub(crate) fn default_trace_dir(workspace_root: PathBuf) -> PathBuf {
    workspace_root.join(".puffer").join("burbot").join("traces")
}
