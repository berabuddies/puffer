use crate::contract::{ActionContract, ContractRegistry, Idempotency, RiskLevel, SideEffectClass};
use crate::graph::{PlanEdgeKind, PlanGraph};
use crate::ids::NodeId;
use crate::runtime::{RuntimeContext, SafetyGate};
use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ParallelBatch {
    pub(crate) nodes: Vec<NodeId>,
    pub(crate) proofs: Vec<ParallelProof>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ParallelProof {
    pub(crate) node_id: NodeId,
    pub(crate) side_effect_class: SideEffectClass,
    pub(crate) risk_level: RiskLevel,
    pub(crate) idempotency: Idempotency,
    pub(crate) reason: &'static str,
}

pub(crate) fn select_parallel_read_batch(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
    safety_gate: &SafetyGate,
    ctx: &RuntimeContext<'_>,
    selected: NodeId,
    max_batch: usize,
) -> Result<Option<ParallelBatch>> {
    let Some(selected_proof) = parallel_proof(graph, contracts, safety_gate, ctx, selected)? else {
        return Ok(None);
    };
    let mut nodes = vec![selected];
    let mut proofs = vec![selected_proof];
    let mut seen = HashSet::new();
    seen.insert(action_payload_key(graph, selected));

    for candidate in graph.executable_frontier_actions() {
        if candidate == selected || nodes.len() >= max_batch {
            continue;
        }
        if dependent_inside_batch(graph, candidate, &nodes) {
            continue;
        }
        let key = action_payload_key(graph, candidate);
        if !seen.insert(key) {
            continue;
        }
        let Some(proof) = parallel_proof(graph, contracts, safety_gate, ctx, candidate)? else {
            continue;
        };
        nodes.push(candidate);
        proofs.push(proof);
    }

    if nodes.len() > 1 {
        Ok(Some(ParallelBatch { nodes, proofs }))
    } else {
        Ok(None)
    }
}

pub(crate) fn parallel_batch_payload(batch: &ParallelBatch) -> Value {
    json!({
        "nodes": batch.nodes.iter().map(|node| node.0).collect::<Vec<_>>(),
        "proofs": batch.proofs,
    })
}

fn parallel_proof(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
    safety_gate: &SafetyGate,
    ctx: &RuntimeContext<'_>,
    node_id: NodeId,
) -> Result<Option<ParallelProof>> {
    let node = graph.node(node_id)?;
    let Some(action_ref) = &node.action_ref else {
        return Ok(None);
    };
    if safety_gate.blocks(node, ctx)?.is_some() {
        return Ok(None);
    }
    let Some(action) = contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
    else {
        return Ok(None);
    };
    if !contract_proves_parallel_read(&action) {
        return Ok(None);
    }
    Ok(Some(ParallelProof {
        node_id,
        side_effect_class: action.side_effect_class,
        risk_level: action.risk_level,
        idempotency: action.idempotency,
        reason: "contract proves low-risk idempotent read-only action",
    }))
}

fn contract_proves_parallel_read(action: &ActionContract) -> bool {
    matches!(
        action.side_effect_class,
        SideEffectClass::PureObservation
            | SideEffectClass::LocalRead
            | SideEffectClass::ExternalRead
    ) && matches!(action.idempotency, Idempotency::Idempotent)
        && matches!(action.risk_level, RiskLevel::Low | RiskLevel::Medium)
        && !action.approval.required
}

fn dependent_inside_batch(graph: &PlanGraph, candidate: NodeId, batch: &[NodeId]) -> bool {
    graph.edges.iter().any(|edge| {
        dependency_edge(edge.kind.clone())
            && ((edge.source == candidate && batch.contains(&edge.target))
                || (edge.target == candidate && batch.contains(&edge.source)))
    })
}

fn dependency_edge(kind: PlanEdgeKind) -> bool {
    matches!(
        kind,
        PlanEdgeKind::DependsOn | PlanEdgeKind::Requires | PlanEdgeKind::PreconditionFor
    )
}

fn action_payload_key(graph: &PlanGraph, node_id: NodeId) -> String {
    graph
        .node(node_id)
        .ok()
        .and_then(|node| {
            Some(format!(
                "{}:{}",
                node.action_ref.as_ref()?.action_name,
                serde_json::to_string(&node.payload).ok()?
            ))
        })
        .unwrap_or_else(|| format!("node:{}", node_id.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::belief::BeliefGraph;
    use crate::contract::{
        ApprovalSpec, CapabilityContract, ContractStatus, Reversibility, TrustLevel,
        VerificationSpec,
    };
    use crate::graph::{ActionRef, ActionScores, PlanNode, PlanNodeKind, PlanStatus};
    use serde_json::json;

    fn action(name: &str, side_effect_class: SideEffectClass) -> ActionContract {
        ActionContract {
            name: name.to_string(),
            description: name.to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            side_effect_class,
            reversibility: Reversibility::Reversible,
            idempotency: Idempotency::Idempotent,
            risk_level: RiskLevel::Low,
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

    fn registry() -> crate::contract::InMemoryContractRegistry {
        let mut registry = crate::contract::InMemoryContractRegistry::default();
        registry
            .register(CapabilityContract {
                contract_id: "tools".to_string(),
                version: "0.1.0".to_string(),
                status: ContractStatus::Active,
                trust_level: TrustLevel::Sandboxed,
                description: "tools".to_string(),
                actions: vec![
                    action("Read", SideEffectClass::LocalRead),
                    action("Search", SideEffectClass::LocalRead),
                    action("Write", SideEffectClass::LocalWrite),
                ],
                global_constraints: Vec::new(),
                forbidden_uses: Vec::new(),
                local_rules: Vec::new(),
                examples: Vec::new(),
                contract_hash: None,
            })
            .unwrap();
        registry
    }

    fn node(action_name: &str, args: Value) -> PlanNode {
        PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Open,
            label: action_name.to_string(),
            payload: args,
            action_ref: Some(ActionRef {
                contract_id: "tools".to_string(),
                action_name: action_name.to_string(),
            }),
            scores: ActionScores::default(),
        }
    }

    #[test]
    fn batch_includes_only_contract_proven_read_only_nodes() {
        let registry = registry();
        let mut graph = PlanGraph::from_goal("inspect".to_string());
        let read = graph.add_node(node("Read", json!({"path": "a"})));
        graph.add_node(node("Search", json!({"pattern": "x"})));
        graph.add_node(node("Write", json!({"path": "a", "content": "x"})));
        let ctx = RuntimeContext {
            contracts: &registry,
            approvals: crate::runtime::ApprovalStore::default(),
            beliefs: BeliefGraph::new(),
        };

        let batch =
            select_parallel_read_batch(&graph, &registry, &SafetyGate, &ctx, read, 8).unwrap();

        let batch = batch.expect("two read-only actions should batch");
        assert_eq!(batch.nodes.len(), 2);
        assert!(batch
            .proofs
            .iter()
            .all(|proof| matches!(proof.side_effect_class, SideEffectClass::LocalRead)));
    }

    #[test]
    fn dependency_edges_prevent_same_batch_execution() {
        let registry = registry();
        let mut graph = PlanGraph::from_goal("inspect".to_string());
        let first = graph.add_node(node("Read", json!({"path": "a"})));
        let second = graph.add_node(node("Search", json!({"pattern": "x"})));
        graph.add_edge(first, second, PlanEdgeKind::DependsOn, json!({}));
        let ctx = RuntimeContext {
            contracts: &registry,
            approvals: crate::runtime::ApprovalStore::default(),
            beliefs: BeliefGraph::new(),
        };

        let batch =
            select_parallel_read_batch(&graph, &registry, &SafetyGate, &ctx, first, 8).unwrap();

        assert!(batch.is_none());
    }
}
