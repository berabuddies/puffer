use crate::contract::ContractRegistry;
use crate::graph::scores_for_action;
use crate::planner::{
    ActionCandidate, ActionPlan, CandidateSource, CompletionRole, PlanDependency,
    PlanDependencyKind,
};
use crate::runtime::RunOptions;
use crate::symbolic::{symbolic_action_ref, BURBOT_SYMBOLIC_CONTRACT_ID};
use serde_json::json;

pub(crate) fn synthesize_candidate_plans(
    contracts: &dyn ContractRegistry,
    goal: &str,
    options: &RunOptions,
    candidates: Vec<ActionCandidate>,
) -> Vec<ActionPlan> {
    if options.puffer_tool.is_some() {
        return candidates.into_iter().map(single_action_plan).collect();
    }

    let cheap_observations = candidates
        .iter()
        .filter(|candidate| candidate.source == CandidateSource::CheapObservation)
        .cloned()
        .collect::<Vec<_>>();
    let terminal_goal_candidates = candidates
        .iter()
        .filter(|candidate| candidate.source == CandidateSource::ContractGoal)
        .cloned()
        .collect::<Vec<_>>();

    let mut plans = Vec::new();
    let symbolic_enabled = options.enable_symbolic_workers;
    let cheap_terminal_exists = candidates.iter().any(|candidate| {
        candidate.source == CandidateSource::CheapObservation
            && candidate.completion_role == CompletionRole::Terminal
    });
    for terminal in &terminal_goal_candidates {
        if symbolic_enabled && cheap_terminal_exists {
            continue;
        }
        if !cheap_observations.is_empty() {
            plans.push(observe_first_plan(&cheap_observations, terminal.clone()));
        }
        if symbolic_enabled {
            if let Some(plan) = symbolic_review_plan(contracts, goal, terminal.clone()) {
                plans.push(plan);
            }
        } else {
            plans.push(single_action_plan(terminal.clone()));
        }
    }

    for candidate in candidates {
        if candidate.source != CandidateSource::CheapObservation
            && candidate.source != CandidateSource::ContractGoal
        {
            plans.push(single_action_plan(candidate));
        } else if candidate.source == CandidateSource::CheapObservation
            && candidate.completion_role == CompletionRole::Terminal
            && symbolic_enabled
        {
            if let Some(plan) = symbolic_review_plan(contracts, goal, candidate) {
                plans.push(plan);
            }
        } else if candidate.source == CandidateSource::CheapObservation
            && candidate.completion_role == CompletionRole::Terminal
        {
            plans.push(single_action_plan(candidate));
        }
    }
    plans
}

pub(crate) fn single_action_plan(candidate: ActionCandidate) -> ActionPlan {
    ActionPlan {
        source: candidate.source.clone(),
        rationale: candidate.rationale.clone(),
        steps: vec![candidate],
        dependencies: Vec::new(),
    }
}

fn observe_first_plan(observations: &[ActionCandidate], terminal: ActionCandidate) -> ActionPlan {
    let mut steps = observations
        .iter()
        .cloned()
        .map(as_support_observation)
        .collect::<Vec<_>>();
    let terminal_index = steps.len();
    steps.push(terminal);
    let dependencies = (0..terminal_index)
        .flat_map(|index| {
            [
                PlanDependency {
                    prerequisite_index: index,
                    dependent_index: terminal_index,
                    kind: PlanDependencyKind::DependsOn,
                },
                PlanDependency {
                    prerequisite_index: index,
                    dependent_index: terminal_index,
                    kind: PlanDependencyKind::PreconditionFor,
                },
            ]
        })
        .collect();
    ActionPlan {
        source: CandidateSource::CheapObservation,
        rationale: "observe low-risk project state before the terminal action".to_string(),
        steps,
        dependencies,
    }
}

fn symbolic_review_plan(
    contracts: &dyn ContractRegistry,
    goal: &str,
    terminal: ActionCandidate,
) -> Option<ActionPlan> {
    let proposal = symbolic_candidate(
        contracts,
        "ProposePlan",
        CandidateSource::SymbolicProposal,
        json!({"goal": goal, "terminal_action": terminal.action_ref.clone()}),
        "symbolically propose guarded alternatives before acting",
        0.35,
    )?;
    let critic = symbolic_candidate(
        contracts,
        "CritiquePlan",
        CandidateSource::SymbolicCritic,
        json!({"goal": goal, "proposal_source": "ProposePlan"}),
        "critique the symbolic plan for risk and verification gaps",
        0.35,
    )?;
    let verifier = symbolic_candidate(
        contracts,
        "VerifyClaim",
        CandidateSource::SymbolicVerifier,
        json!({"claim": "selected terminal action is contract gated and has supporting critique"}),
        "verify symbolic plan metadata before terminal action",
        0.35,
    )?;
    let steps = vec![proposal, critic, verifier, terminal];
    Some(ActionPlan {
        source: CandidateSource::SynthesizedPlan,
        rationale: "proposal, critique, and symbolic verification before the terminal action"
            .to_string(),
        steps,
        dependencies: vec![
            PlanDependency {
                prerequisite_index: 0,
                dependent_index: 1,
                kind: PlanDependencyKind::DependsOn,
            },
            PlanDependency {
                prerequisite_index: 1,
                dependent_index: 2,
                kind: PlanDependencyKind::DependsOn,
            },
            PlanDependency {
                prerequisite_index: 2,
                dependent_index: 3,
                kind: PlanDependencyKind::DependsOn,
            },
            PlanDependency {
                prerequisite_index: 0,
                dependent_index: 3,
                kind: PlanDependencyKind::PreconditionFor,
            },
            PlanDependency {
                prerequisite_index: 1,
                dependent_index: 3,
                kind: PlanDependencyKind::PreconditionFor,
            },
            PlanDependency {
                prerequisite_index: 2,
                dependent_index: 3,
                kind: PlanDependencyKind::PreconditionFor,
            },
        ],
    })
}

fn symbolic_candidate(
    contracts: &dyn ContractRegistry,
    action_name: &str,
    source: CandidateSource,
    args: serde_json::Value,
    rationale: &str,
    expected_progress: f64,
) -> Option<ActionCandidate> {
    let action = contracts.get_action(BURBOT_SYMBOLIC_CONTRACT_ID, action_name)?;
    let mut scores = scores_for_action(&action);
    scores.expected_progress = expected_progress;
    scores.information_gain += 0.4;
    scores.verification_value += 0.2;
    Some(ActionCandidate {
        action_ref: symbolic_action_ref(action_name),
        args,
        source,
        completion_role: CompletionRole::Support,
        rationale: rationale.to_string(),
        scores,
    })
}

fn as_support_observation(mut candidate: ActionCandidate) -> ActionCandidate {
    candidate.completion_role = CompletionRole::Support;
    candidate.rationale = format!("{}; used as precondition evidence", candidate.rationale);
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ActionContract, ApprovalSpec, CapabilityContract, ContractStatus, Idempotency,
        InMemoryContractRegistry, Reversibility, RiskLevel, SideEffectClass, TrustLevel,
        VerificationSpec,
    };
    use crate::planner::{puffer_action, PlanDependencyKind};
    use crate::symbolic::symbolic_contract;

    fn registry() -> InMemoryContractRegistry {
        let mut registry = InMemoryContractRegistry::default();
        registry
            .register(CapabilityContract {
                contract_id: "puffer.tools".to_string(),
                version: "0.1.0".to_string(),
                status: ContractStatus::Active,
                trust_level: TrustLevel::Sandboxed,
                description: "tools".to_string(),
                actions: vec![ActionContract {
                    name: "Bash".to_string(),
                    description: "bash".to_string(),
                    input_schema: json!({"type": "object"}),
                    output_schema: json!({"type": "object"}),
                    side_effect_class: SideEffectClass::Unknown,
                    reversibility: Reversibility::Unknown,
                    idempotency: Idempotency::Unknown,
                    risk_level: RiskLevel::High,
                    preconditions: Vec::new(),
                    postconditions: Vec::new(),
                    verification: VerificationSpec {
                        methods: Vec::new(),
                        observation_checks: Vec::new(),
                        method_templates: Vec::new(),
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
                    structured_argument_safety: Vec::new(),
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
            })
            .unwrap();
        registry.register(symbolic_contract()).unwrap();
        registry
    }

    #[test]
    fn symbolic_review_plan_adds_worker_dependencies() {
        let terminal = ActionCandidate {
            action_ref: puffer_action("Bash"),
            args: json!({"command": "pwd"}),
            source: CandidateSource::ContractGoal,
            completion_role: CompletionRole::Terminal,
            rationale: "terminal".to_string(),
            scores: Default::default(),
        };

        let plan = symbolic_review_plan(&registry(), "inspect", terminal).unwrap();

        assert_eq!(plan.source, CandidateSource::SynthesizedPlan);
        assert_eq!(plan.steps.len(), 4);
        assert!(plan
            .steps
            .iter()
            .any(|step| step.source == CandidateSource::SymbolicCritic));
        assert!(plan
            .dependencies
            .iter()
            .any(|dependency| dependency.kind == PlanDependencyKind::PreconditionFor));
    }
}
