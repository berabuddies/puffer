use crate::contract::{ContractRegistry, RiskLevel, SideEffectClass};
use crate::graph::{
    scores_for_action, ActionRef, PlanEdgeKind, PlanGraph, PlanNode, PlanNodeKind, PlanStatus,
};
use crate::ids::NodeId;
use crate::planner::{CandidateSource, CompletionRole};
use crate::semantics::{payload_for_intent, read_only_side_effect, NormalizedIntent};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

pub(crate) struct RuleContext<'a> {
    pub(crate) contracts: &'a dyn ContractRegistry,
    pub(crate) repeated_failures: &'a HashMap<String, u64>,
    pub(crate) state_epoch: u64,
    pub(crate) approved_nodes: &'a HashSet<NodeId>,
}

pub(crate) struct RewriteResult {
    pub(crate) changes: usize,
}

pub(crate) trait RewriteRule {
    fn name(&self) -> &'static str;
    fn apply(&self, graph: &mut PlanGraph, ctx: &RuleContext<'_>) -> Result<RewriteResult>;
}

pub(crate) struct RewriteEngine {
    rules: Vec<Box<dyn RewriteRule>>,
}

pub(crate) struct UnknownSideEffectsIncreaseRisk;
pub(crate) struct ObserveBeforeRiskyAction;
pub(crate) struct RequireApprovalForExternalWrite;
pub(crate) struct VerifyBeforeCompletion;
pub(crate) struct BlockRepeatedFailure;
pub(crate) struct InsertRepairCandidatesFromContract;

impl RewriteEngine {
    pub(crate) fn universal() -> Self {
        Self {
            rules: vec![
                Box::new(UnknownSideEffectsIncreaseRisk),
                Box::new(ObserveBeforeRiskyAction),
                Box::new(RequireApprovalForExternalWrite),
                Box::new(VerifyBeforeCompletion),
                Box::new(BlockRepeatedFailure),
                Box::new(InsertRepairCandidatesFromContract),
            ],
        }
    }

    pub(crate) fn apply_all(
        &self,
        graph: &mut PlanGraph,
        ctx: &RuleContext<'_>,
    ) -> Result<Vec<(&'static str, usize)>> {
        let mut results = Vec::new();
        for rule in &self.rules {
            let result = rule.apply(graph, ctx)?;
            results.push((rule.name(), result.changes));
        }
        Ok(results)
    }
}

impl RewriteRule for UnknownSideEffectsIncreaseRisk {
    fn name(&self) -> &'static str {
        "UnknownSideEffectsIncreaseRisk"
    }

    fn apply(&self, graph: &mut PlanGraph, ctx: &RuleContext<'_>) -> Result<RewriteResult> {
        let ids = graph.frontier_actions();
        let mut changes = 0;
        for id in ids {
            let Some(action_ref) = graph.node(id)?.action_ref.clone() else {
                continue;
            };
            let Some(action) = ctx
                .contracts
                .get_action(&action_ref.contract_id, &action_ref.action_name)
            else {
                continue;
            };
            if action.side_effect_class == SideEffectClass::Unknown {
                let node = graph.node_mut(id)?;
                if node.scores.uncertainty_penalty < 0.9 {
                    node.scores.uncertainty_penalty += 0.5;
                    node.scores.risk_penalty += 0.25;
                    changes += 1;
                }
            }
        }
        Ok(RewriteResult { changes })
    }
}

impl RewriteRule for ObserveBeforeRiskyAction {
    fn name(&self) -> &'static str {
        "ObserveBeforeRiskyAction"
    }

    fn apply(&self, graph: &mut PlanGraph, ctx: &RuleContext<'_>) -> Result<RewriteResult> {
        let ids = graph.frontier_actions();
        let mut changes = 0;
        for id in ids {
            let Some(action_ref) = graph.node(id)?.action_ref.clone() else {
                continue;
            };
            let Some(action) = ctx
                .contracts
                .get_action(&action_ref.contract_id, &action_ref.action_name)
            else {
                continue;
            };
            if action.risk_level < RiskLevel::Medium || action.preconditions.is_empty() {
                continue;
            }
            if graph
                .edges
                .iter()
                .any(|edge| edge.target == id && edge.kind == PlanEdgeKind::PreconditionFor)
            {
                continue;
            }
            let Some((observe_ref, observe_payload, observe_scores)) =
                precondition_observation_action(ctx.contracts)
            else {
                continue;
            };
            let observe_id =
                existing_observation_node(graph, &observe_ref, &observe_payload).unwrap_or_else(
                    || {
                        let id = graph.add_node(PlanNode {
                            id: NodeId(0),
                            kind: PlanNodeKind::Action,
                            status: PlanStatus::Open,
                            label: format!("{}.{}", observe_ref.contract_id, observe_ref.action_name),
                            payload: observe_payload,
                            action_ref: Some(observe_ref),
                            scores: observe_scores,
                        });
                        graph.add_edge(
                            NodeId(0),
                            id,
                            PlanEdgeKind::Supports,
                            json!({
                                "source": CandidateSource::CheapObservation,
                                "completion_role": CompletionRole::Support,
                                "rationale": "contract-declared read-only observation before risky action",
                            }),
                        );
                        id
                    },
                );
            graph.add_edge(observe_id, id, PlanEdgeKind::DependsOn, json!({}));
            graph.add_edge(observe_id, id, PlanEdgeKind::PreconditionFor, json!({}));
            changes += 1;
        }
        Ok(RewriteResult { changes })
    }
}

impl RewriteRule for RequireApprovalForExternalWrite {
    fn name(&self) -> &'static str {
        "RequireApprovalForExternalWrite"
    }

    fn apply(&self, graph: &mut PlanGraph, ctx: &RuleContext<'_>) -> Result<RewriteResult> {
        let ids = graph.frontier_actions();
        let mut changes = 0;
        for id in ids {
            let Some(action_ref) = graph.node(id)?.action_ref.clone() else {
                continue;
            };
            let Some(action) = ctx
                .contracts
                .get_action(&action_ref.contract_id, &action_ref.action_name)
            else {
                continue;
            };
            if !action.approval.required {
                continue;
            }
            if ctx.approved_nodes.contains(&id) {
                continue;
            }
            graph.node_mut(id)?.status = PlanStatus::Blocked;
            if !graph
                .edges
                .iter()
                .any(|edge| edge.target == id && edge.kind == PlanEdgeKind::ApprovalRequiredFor)
            {
                let approval_id = graph.add_node(PlanNode {
                    id: NodeId(0),
                    kind: PlanNodeKind::Approval,
                    status: PlanStatus::Open,
                    label: format!("Approval required for {}", action.name),
                    payload: json!({"reason": action.approval.reason}),
                    action_ref: None,
                    scores: Default::default(),
                });
                graph.add_edge(
                    approval_id,
                    id,
                    PlanEdgeKind::ApprovalRequiredFor,
                    json!({}),
                );
            }
            changes += 1;
        }
        Ok(RewriteResult { changes })
    }
}

fn existing_observation_node(
    graph: &PlanGraph,
    action_ref: &ActionRef,
    payload: &Value,
) -> Option<NodeId> {
    graph.nodes.iter().find_map(|(id, node)| {
        (node.kind == PlanNodeKind::Action
            && node.status != PlanStatus::Failed
            && node.action_ref.as_ref() == Some(action_ref)
            && node.payload == *payload)
            .then_some(*id)
    })
}

fn precondition_observation_action(
    contracts: &dyn ContractRegistry,
) -> Option<(ActionRef, Value, crate::graph::ActionScores)> {
    let mut slots = BTreeMap::new();
    slots.insert("pattern".to_string(), "*".to_string());
    let intent = NormalizedIntent {
        intent: "glob_paths".to_string(),
        slots,
    };
    for contract in contracts.active_contracts() {
        let contract_id = contract.contract_id.clone();
        for action in contract.actions {
            if !read_only_side_effect(&action.side_effect_class) {
                continue;
            }
            let Some(args) = payload_for_intent(&action, &intent) else {
                continue;
            };
            let mut scores = scores_for_action(&action);
            scores.information_gain += 0.8;
            scores.expected_progress += 0.1;
            return Some((
                ActionRef {
                    contract_id,
                    action_name: action.name,
                },
                args,
                scores,
            ));
        }
    }
    None
}

impl RewriteRule for VerifyBeforeCompletion {
    fn name(&self) -> &'static str {
        "VerifyBeforeCompletion"
    }

    fn apply(&self, graph: &mut PlanGraph, ctx: &RuleContext<'_>) -> Result<RewriteResult> {
        let ids = graph.frontier_actions();
        let mut changes = 0;
        for id in ids {
            let Some(action_ref) = graph.node(id)?.action_ref.clone() else {
                continue;
            };
            let Some(action) = ctx
                .contracts
                .get_action(&action_ref.contract_id, &action_ref.action_name)
            else {
                continue;
            };
            if !action.verification.required_before_completion {
                continue;
            }
            if graph
                .edges
                .iter()
                .any(|edge| edge.target == id && edge.kind == PlanEdgeKind::Verifies)
            {
                continue;
            }
            let verification_id = graph.add_node(PlanNode {
                id: NodeId(0),
                kind: PlanNodeKind::Verification,
                status: PlanStatus::Open,
                label: format!("Verify {}", action.name),
                payload: json!({"methods": action.verification.methods}),
                action_ref: None,
                scores: Default::default(),
            });
            graph.add_edge(verification_id, id, PlanEdgeKind::Verifies, json!({}));
            changes += 1;
        }
        Ok(RewriteResult { changes })
    }
}

impl RewriteRule for BlockRepeatedFailure {
    fn name(&self) -> &'static str {
        "BlockRepeatedFailure"
    }

    fn apply(&self, graph: &mut PlanGraph, ctx: &RuleContext<'_>) -> Result<RewriteResult> {
        let ids = graph.frontier_actions();
        let mut changes = 0;
        for id in ids {
            let node = graph.node(id)?.clone();
            let Some(action_ref) = node.action_ref else {
                continue;
            };
            if ctx
                .repeated_failures
                .get(&action_key(&action_ref, &node.payload))
                .is_some_and(|failed_epoch| *failed_epoch == ctx.state_epoch)
            {
                let node = graph.node_mut(id)?;
                node.status = PlanStatus::Blocked;
                node.scores.repeated_failure_penalty += 1.0;
                changes += 1;
            }
        }
        Ok(RewriteResult { changes })
    }
}

impl RewriteRule for InsertRepairCandidatesFromContract {
    fn name(&self) -> &'static str {
        "InsertRepairCandidatesFromContract"
    }

    fn apply(&self, graph: &mut PlanGraph, ctx: &RuleContext<'_>) -> Result<RewriteResult> {
        let failure_nodes = graph
            .nodes
            .iter()
            .filter_map(|(id, node)| (node.kind == PlanNodeKind::Failure).then_some(*id))
            .collect::<Vec<_>>();
        let mut changes = 0;
        for id in failure_nodes {
            if graph
                .edges
                .iter()
                .any(|edge| edge.source == id && edge.kind == PlanEdgeKind::Repairs)
            {
                continue;
            }
            let failure = graph.node(id)?.clone();
            let Some(action_ref) = action_ref_from_payload(&failure.payload) else {
                continue;
            };
            let Some(action) = ctx
                .contracts
                .get_action(&action_ref.contract_id, &action_ref.action_name)
            else {
                continue;
            };
            let classified_mode = failure
                .payload
                .get("failure_mode")
                .and_then(serde_json::Value::as_str);
            for mode in action.failure_modes {
                if classified_mode.is_some_and(|classified| classified != mode.name) {
                    continue;
                }
                let repair_id = graph.add_node(PlanNode {
                    id: NodeId(0),
                    kind: PlanNodeKind::Repair,
                    status: PlanStatus::Open,
                    label: format!("Repair {} via {}", action.name, mode.name),
                    payload: json!({
                        "failure_mode": mode.name,
                        "repair_strategy": mode.repair_strategy,
                    }),
                    action_ref: None,
                    scores: Default::default(),
                });
                graph.add_edge(id, repair_id, PlanEdgeKind::Repairs, json!({}));
                changes += 1;
            }
        }
        Ok(RewriteResult { changes })
    }
}

pub(crate) fn action_key(action_ref: &ActionRef, args: &serde_json::Value) -> String {
    format!(
        "{}.{}:{}",
        action_ref.contract_id,
        action_ref.action_name,
        serde_json::to_string(args).unwrap_or_default()
    )
}

fn action_ref_from_payload(payload: &serde_json::Value) -> Option<ActionRef> {
    Some(ActionRef {
        contract_id: payload.get("contract_id")?.as_str()?.to_string(),
        action_name: payload.get("action_name")?.as_str()?.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ActionContract, ApprovalSpec, ArgumentPatternSpec, ArgumentSafetySpec, ContractStatus,
        Idempotency, InMemoryContractRegistry, Reversibility, TrustLevel, VerificationSpec,
    };
    use crate::graph::{scores_for_action, ActionScores};

    fn registry_with(action: ActionContract) -> InMemoryContractRegistry {
        let mut registry = InMemoryContractRegistry::default();
        registry
            .register(crate::contract::CapabilityContract {
                contract_id: "puffer.tools".to_string(),
                version: "0.1.0".to_string(),
                status: ContractStatus::Active,
                trust_level: TrustLevel::Sandboxed,
                description: "Puffer tools".to_string(),
                actions: vec![action],
                global_constraints: Vec::new(),
                forbidden_uses: Vec::new(),
                local_rules: Vec::new(),
                examples: Vec::new(),
                contract_hash: None,
            })
            .unwrap();
        registry
    }

    fn action() -> ActionContract {
        ActionContract {
            name: "Bash".to_string(),
            description: "run shell command through Puffer".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            side_effect_class: SideEffectClass::Unknown,
            reversibility: Reversibility::Unknown,
            idempotency: Idempotency::Unknown,
            risk_level: RiskLevel::Medium,
            preconditions: vec!["command reviewed".to_string()],
            postconditions: Vec::new(),
            verification: VerificationSpec {
                methods: vec!["exit code zero".to_string()],
                templates: Vec::new(),
                required_before_completion: true,
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
        }
    }

    #[test]
    fn unknown_side_effects_raise_penalties() {
        let action = action();
        let registry = registry_with(action.clone());
        let mut graph = PlanGraph::from_goal("run".to_string());
        let id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Open,
            label: action.name.clone(),
            payload: json!({"command": "pwd"}),
            action_ref: Some(ActionRef {
                contract_id: "puffer.tools".to_string(),
                action_name: action.name.clone(),
            }),
            scores: scores_for_action(&action),
        });
        let ctx = RuleContext {
            contracts: &registry,
            repeated_failures: &HashMap::new(),
            state_epoch: 0,
            approved_nodes: &HashSet::new(),
        };
        UnknownSideEffectsIncreaseRisk
            .apply(&mut graph, &ctx)
            .unwrap();
        assert!(graph.node(id).unwrap().scores.uncertainty_penalty > 0.4);
    }

    #[test]
    fn repeated_failure_blocks_matching_action() {
        let action = action();
        let registry = registry_with(action.clone());
        let mut graph = PlanGraph::from_goal("run".to_string());
        let action_ref = ActionRef {
            contract_id: "puffer.tools".to_string(),
            action_name: action.name,
        };
        let args = json!({"command": "false"});
        let id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Open,
            label: "Bash".to_string(),
            payload: args.clone(),
            action_ref: Some(action_ref.clone()),
            scores: ActionScores::default(),
        });
        let repeated = HashMap::from([(action_key(&action_ref, &args), 0)]);
        let ctx = RuleContext {
            contracts: &registry,
            repeated_failures: &repeated,
            state_epoch: 0,
            approved_nodes: &HashSet::new(),
        };
        BlockRepeatedFailure.apply(&mut graph, &ctx).unwrap();
        assert_eq!(graph.node(id).unwrap().status, PlanStatus::Blocked);
    }

    #[test]
    fn repeated_failure_allows_retry_after_state_changes() {
        let action = action();
        let registry = registry_with(action.clone());
        let mut graph = PlanGraph::from_goal("run".to_string());
        let action_ref = ActionRef {
            contract_id: "puffer.tools".to_string(),
            action_name: action.name,
        };
        let args = json!({"command": "false"});
        let id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Open,
            label: "Bash".to_string(),
            payload: args.clone(),
            action_ref: Some(action_ref.clone()),
            scores: ActionScores::default(),
        });
        let repeated = HashMap::from([(action_key(&action_ref, &args), 0)]);
        let ctx = RuleContext {
            contracts: &registry,
            repeated_failures: &repeated,
            state_epoch: 1,
            approved_nodes: &HashSet::new(),
        };

        BlockRepeatedFailure.apply(&mut graph, &ctx).unwrap();

        assert_eq!(graph.node(id).unwrap().status, PlanStatus::Open);
    }

    #[test]
    fn approval_rule_blocks_explicit_contract_approval_even_with_safe_args() {
        let mut action = action();
        action.name = "Write".to_string();
        action.side_effect_class = SideEffectClass::LocalWrite;
        action.approval.required = true;
        action.argument_safety = vec![ArgumentSafetySpec {
            source_arg: "file_path".to_string(),
            blocked_patterns: vec![ArgumentPatternSpec {
                name: "system_path".to_string(),
                pattern: r"^/(proc|sys|dev)(/|$)".to_string(),
                reason: None,
                confidence: 0.9,
            }],
            approval_patterns: vec![ArgumentPatternSpec {
                name: "git_metadata".to_string(),
                pattern: r"(^|/)\.git(/|$)".to_string(),
                reason: None,
                confidence: 0.9,
            }],
        }];
        let registry = registry_with(action.clone());
        let mut graph = PlanGraph::from_goal("write".to_string());
        let id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Open,
            label: "Write".to_string(),
            payload: json!({"file_path": "/app/output.txt"}),
            action_ref: Some(ActionRef {
                contract_id: "puffer.tools".to_string(),
                action_name: action.name,
            }),
            scores: ActionScores::default(),
        });
        let ctx = RuleContext {
            contracts: &registry,
            repeated_failures: &HashMap::new(),
            state_epoch: 0,
            approved_nodes: &HashSet::new(),
        };

        RequireApprovalForExternalWrite
            .apply(&mut graph, &ctx)
            .unwrap();

        assert_eq!(graph.node(id).unwrap().status, PlanStatus::Blocked);
    }
}
