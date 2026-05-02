use crate::contract::{
    ActionContract, CapabilityContract, Reversibility, RiskLevel, SideEffectClass,
};
use crate::ids::NodeId;
use anyhow::{anyhow, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanNodeKind {
    Goal,
    Subgoal,
    Action,
    Observation,
    Failure,
    Repair,
    Verification,
    Artifact,
    Constraint,
    Approval,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanStatus {
    Open,
    Blocked,
    Executed,
    Failed,
    Satisfied,
    Pruned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ActionRef {
    pub(crate) contract_id: String,
    pub(crate) action_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub(crate) struct ActionScores {
    pub(crate) expected_progress: f64,
    pub(crate) information_gain: f64,
    pub(crate) verification_value: f64,
    pub(crate) reversibility_bonus: f64,
    pub(crate) cache_reuse_bonus: f64,
    pub(crate) risk_penalty: f64,
    pub(crate) cost_penalty: f64,
    pub(crate) latency_penalty: f64,
    pub(crate) uncertainty_penalty: f64,
    pub(crate) repeated_failure_penalty: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PlanNode {
    pub(crate) id: NodeId,
    pub(crate) kind: PlanNodeKind,
    pub(crate) status: PlanStatus,
    pub(crate) label: String,
    pub(crate) payload: Value,
    pub(crate) action_ref: Option<ActionRef>,
    pub(crate) scores: ActionScores,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanEdgeKind {
    DecomposesTo,
    Requires,
    Produces,
    Supports,
    Contradicts,
    FailedWith,
    Repairs,
    Verifies,
    Blocks,
    CanReplace,
    DependsOn,
    PreconditionFor,
    ApprovalRequiredFor,
    ObservationOf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PlanEdge {
    pub(crate) source: NodeId,
    pub(crate) target: NodeId,
    pub(crate) kind: PlanEdgeKind,
    pub(crate) payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PlanGraph {
    pub(crate) nodes: IndexMap<NodeId, PlanNode>,
    pub(crate) edges: Vec<PlanEdge>,
    pub(crate) next_id: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct CapabilityGraph {
    contracts: Vec<CapabilityContract>,
}

impl PlanGraph {
    pub(crate) fn new() -> Self {
        Self {
            nodes: IndexMap::new(),
            edges: Vec::new(),
            next_id: 1,
        }
    }

    pub(crate) fn from_goal(goal: String) -> Self {
        let mut graph = Self::new();
        graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Goal,
            status: PlanStatus::Open,
            label: goal,
            payload: json!({}),
            action_ref: None,
            scores: ActionScores::default(),
        });
        graph
    }

    pub(crate) fn fresh_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    pub(crate) fn add_node(&mut self, mut node: PlanNode) -> NodeId {
        if node.id.0 == 0 && node.kind != PlanNodeKind::Goal {
            node.id = self.fresh_id();
        }
        let id = node.id;
        self.nodes.insert(id, node);
        id
    }

    pub(crate) fn add_edge(
        &mut self,
        source: NodeId,
        target: NodeId,
        kind: PlanEdgeKind,
        payload: Value,
    ) {
        self.edges.push(PlanEdge {
            source,
            target,
            kind,
            payload,
        });
    }

    pub(crate) fn node(&self, id: NodeId) -> Result<&PlanNode> {
        self.nodes
            .get(&id)
            .ok_or_else(|| anyhow!("missing plan node {:?}", id))
    }

    pub(crate) fn node_mut(&mut self, id: NodeId) -> Result<&mut PlanNode> {
        self.nodes
            .get_mut(&id)
            .ok_or_else(|| anyhow!("missing plan node {:?}", id))
    }

    pub(crate) fn frontier_actions(&self) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter_map(|(id, node)| {
                (node.kind == PlanNodeKind::Action && node.status == PlanStatus::Open)
                    .then_some(*id)
            })
            .collect()
    }

    pub(crate) fn executable_frontier_actions(&self) -> Vec<NodeId> {
        self.frontier_actions()
            .into_iter()
            .filter(|id| self.dependencies_satisfied(*id))
            .collect()
    }

    pub(crate) fn has_edge(&self, source: NodeId, target: NodeId, kind: PlanEdgeKind) -> bool {
        self.edges
            .iter()
            .any(|edge| edge.source == source && edge.target == target && edge.kind == kind)
    }

    fn dependencies_satisfied(&self, target: NodeId) -> bool {
        self.edges
            .iter()
            .filter(|edge| edge.target == target && dependency_edge_blocks(edge.kind.clone()))
            .all(|edge| {
                self.nodes
                    .get(&edge.source)
                    .is_some_and(dependency_source_satisfied)
            })
    }
}

fn dependency_edge_blocks(kind: PlanEdgeKind) -> bool {
    matches!(kind, PlanEdgeKind::DependsOn | PlanEdgeKind::Requires)
}

fn dependency_source_satisfied(node: &PlanNode) -> bool {
    matches!(node.status, PlanStatus::Executed | PlanStatus::Satisfied)
}

impl CapabilityGraph {
    pub(crate) fn from_contracts(contracts: Vec<CapabilityContract>) -> Self {
        Self { contracts }
    }

    pub(crate) fn action_contract(&self, action_ref: &ActionRef) -> Option<ActionContract> {
        self.contracts
            .iter()
            .find(|contract| contract.contract_id == action_ref.contract_id)?
            .actions
            .iter()
            .find(|action| action.name == action_ref.action_name)
            .cloned()
    }
}

pub(crate) fn scores_for_action(action: &ActionContract) -> ActionScores {
    ActionScores {
        expected_progress: 0.5,
        information_gain: match action.side_effect_class {
            SideEffectClass::PureObservation
            | SideEffectClass::LocalRead
            | SideEffectClass::ExternalRead => 0.65,
            _ => 0.25,
        },
        verification_value: if action.verification.required_before_completion {
            0.5
        } else {
            0.1
        },
        reversibility_bonus: match action.reversibility {
            Reversibility::Reversible => 0.4,
            Reversibility::ConditionallyReversible => 0.2,
            _ => 0.0,
        },
        cache_reuse_bonus: 0.0,
        risk_penalty: risk_penalty(&action.risk_level),
        cost_penalty: 0.0,
        latency_penalty: 0.0,
        uncertainty_penalty: if action.side_effect_class == SideEffectClass::Unknown {
            0.4
        } else {
            0.0
        },
        repeated_failure_penalty: 0.0,
    }
}

fn risk_penalty(risk: &RiskLevel) -> f64 {
    match risk {
        RiskLevel::Low => 0.1,
        RiskLevel::Medium => 0.4,
        RiskLevel::High => 1.0,
        RiskLevel::Critical => 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ApprovalSpec, ContractStatus, Idempotency, TrustLevel, VerificationSpec,
    };

    fn action(name: &str, risk_level: RiskLevel) -> ActionContract {
        ActionContract {
            name: name.to_string(),
            description: "read a file".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            side_effect_class: SideEffectClass::LocalRead,
            reversibility: Reversibility::Reversible,
            idempotency: Idempotency::Idempotent,
            risk_level,
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
        }
    }

    #[test]
    fn plan_graph_tracks_frontier_actions() {
        let mut graph = PlanGraph::from_goal("inspect".to_string());
        let id = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Open,
            label: "Read".to_string(),
            payload: json!({}),
            action_ref: None,
            scores: ActionScores::default(),
        });
        assert_eq!(graph.frontier_actions(), vec![id]);
        graph.node_mut(id).unwrap().status = PlanStatus::Executed;
        assert!(graph.frontier_actions().is_empty());
    }

    #[test]
    fn capability_graph_resolves_declared_action_contracts() {
        let contract = CapabilityContract {
            contract_id: "puffer.tools".to_string(),
            version: "0.1.0".to_string(),
            status: ContractStatus::Active,
            trust_level: TrustLevel::Sandboxed,
            description: "Puffer tools".to_string(),
            actions: vec![action("Read", RiskLevel::Low)],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        };
        let graph = CapabilityGraph::from_contracts(vec![contract]);
        let action = graph
            .action_contract(&ActionRef {
                contract_id: "puffer.tools".to_string(),
                action_name: "Read".to_string(),
            })
            .unwrap();
        assert_eq!(action.name, "Read");
    }
}
