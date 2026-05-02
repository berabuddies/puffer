use crate::contract::ContractRegistry;
use crate::graph::{ActionRef, PlanEdgeKind, PlanGraph, PlanNodeKind, PlanStatus};
use crate::ids::{NodeId, RunId};
use crate::runtime::model_loop_support::model_support_edge_for;
use crate::semantics::read_only_side_effect;
use crate::trace::{trace_event, TraceEvent, TraceEventType};
use serde_json::{json, Value};

/// Prunes open model verifier/terminal actions made stale by a same-kind failure.
pub(super) fn prune_stale_model_frontier_after_failure(
    run_id: RunId,
    graph: &mut PlanGraph,
    failed_action_id: NodeId,
) -> Vec<TraceEvent> {
    let Some(failed) = failed_model_action_signature(graph, failed_action_id) else {
        return Vec::new();
    };
    let no_progress_repair = failed.completion_role == "repair"
        && failure_mode_for_action(graph, failed_action_id) == Some("no_progress");
    if !matches!(failed.completion_role.as_str(), "terminal" | "verification")
        && !no_progress_repair
    {
        return Vec::new();
    }
    let pruned = graph
        .frontier_actions()
        .into_iter()
        .filter(|node_id| *node_id != failed_action_id)
        .filter(|node_id| stale_model_frontier_match(graph, *node_id, &failed))
        .collect::<Vec<_>>();
    for node_id in &pruned {
        if let Ok(node) = graph.node_mut(*node_id) {
            node.status = PlanStatus::Pruned;
        }
    }
    if pruned.is_empty() {
        return Vec::new();
    }
    vec![trace_event(
        run_id,
        TraceEventType::RewriteApplied,
        Some(failed_action_id),
        json!({"rule": "PruneStaleModelFrontierAfterFailure"}),
        json!({
            "failed_action": failed_action_id.0,
            "source": failed.source,
            "completion_role": failed.completion_role,
            "action_ref": failed.action_ref,
            "pruned": pruned.into_iter().map(|node_id| node_id.0).collect::<Vec<_>>(),
        }),
    )]
}

/// Prunes stale same-batch model actions after an unsatisfied verifier result.
pub(super) fn prune_stale_model_siblings_after_goal_unsatisfied(
    run_id: RunId,
    graph: &mut PlanGraph,
    action_id: NodeId,
) -> Vec<TraceEvent> {
    prune_stale_model_siblings(
        run_id,
        graph,
        action_id,
        "PruneStaleModelSiblingsAfterGoalUnsatisfied",
        |role| matches!(role, "terminal" | "support"),
    )
}

/// Prunes same-batch model actions after a generated artifact is rejected.
pub(super) fn prune_stale_model_siblings_after_artifact_review_failed(
    run_id: RunId,
    graph: &mut PlanGraph,
    contracts: &dyn ContractRegistry,
    action_id: NodeId,
) -> Vec<TraceEvent> {
    prune_stale_model_dependents(
        run_id,
        graph,
        contracts,
        action_id,
        "PruneStaleModelDependentsAfterArtifactReviewFailed",
    )
}

fn prune_stale_model_siblings(
    run_id: RunId,
    graph: &mut PlanGraph,
    action_id: NodeId,
    rule: &str,
    prune_role: impl Fn(&str) -> bool,
) -> Vec<TraceEvent> {
    let Some((support_source, source)) = model_support_edge_for(graph, action_id) else {
        return Vec::new();
    };
    if !model_source(&source) {
        return Vec::new();
    }
    let sibling_ids = graph
        .edges
        .iter()
        .filter(|edge| {
            edge.kind == PlanEdgeKind::Supports
                && edge.source == support_source
                && edge.target != action_id
                && edge.payload.get("source").and_then(Value::as_str) == Some(source.as_str())
                && edge
                    .payload
                    .get("completion_role")
                    .and_then(Value::as_str)
                    .map(&prune_role)
                    .unwrap_or(true)
        })
        .map(|edge| edge.target)
        .collect::<Vec<_>>();
    let mut pruned = 0;
    for sibling_id in sibling_ids {
        if let Ok(node) = graph.node_mut(sibling_id) {
            if node.kind == PlanNodeKind::Action && node.status == PlanStatus::Open {
                node.status = PlanStatus::Pruned;
                pruned += 1;
            }
        }
    }
    if pruned == 0 {
        return Vec::new();
    }
    vec![trace_event(
        run_id,
        TraceEventType::RewriteApplied,
        Some(action_id),
        json!({"rule": rule}),
        json!({
            "pruned": pruned,
            "support_source": support_source.0,
            "source": source,
        }),
    )]
}

fn prune_stale_model_dependents(
    run_id: RunId,
    graph: &mut PlanGraph,
    contracts: &dyn ContractRegistry,
    action_id: NodeId,
    rule: &str,
) -> Vec<TraceEvent> {
    let Some((support_source, source)) = model_support_edge_for(graph, action_id) else {
        return Vec::new();
    };
    if !model_source(&source) {
        return Vec::new();
    }
    let dependent_ids =
        artifact_rejection_prune_targets(graph, contracts, action_id, support_source, &source);
    let mut pruned = Vec::new();
    for dependent_id in dependent_ids {
        if let Ok(node) = graph.node_mut(dependent_id) {
            if node.kind == PlanNodeKind::Action && node.status == PlanStatus::Open {
                node.status = PlanStatus::Pruned;
                pruned.push(dependent_id.0);
            }
        }
    }
    if pruned.is_empty() {
        return Vec::new();
    }
    vec![trace_event(
        run_id,
        TraceEventType::RewriteApplied,
        Some(action_id),
        json!({"rule": rule}),
        json!({
            "pruned": pruned,
            "source": source,
        }),
    )]
}

fn failure_mode_for_action(graph: &PlanGraph, action_id: NodeId) -> Option<&str> {
    graph
        .edges
        .iter()
        .find(|edge| edge.source == action_id && edge.kind == PlanEdgeKind::FailedWith)
        .and_then(|edge| graph.nodes.get(&edge.target))
        .and_then(|node| node.payload.get("failure_mode"))
        .and_then(Value::as_str)
}

fn stale_model_frontier_match(
    graph: &PlanGraph,
    node_id: NodeId,
    failed: &ModelActionSignature,
) -> bool {
    let Ok(node) = graph.node(node_id) else {
        return false;
    };
    node.kind == PlanNodeKind::Action
        && node.status == PlanStatus::Open
        && node.action_ref.as_ref() == Some(&failed.action_ref)
        && failed_model_action_signature(graph, node_id).is_some_and(|candidate| {
            candidate.source == failed.source
                && candidate.completion_role == failed.completion_role
                && candidate.action_ref == failed.action_ref
        })
}

fn failed_model_action_signature(
    graph: &PlanGraph,
    action_id: NodeId,
) -> Option<ModelActionSignature> {
    let node = graph.node(action_id).ok()?;
    let action_ref = node.action_ref.clone()?;
    let support = graph.edges.iter().find(|edge| {
        edge.target == action_id
            && edge.kind == PlanEdgeKind::Supports
            && edge
                .payload
                .get("source")
                .and_then(Value::as_str)
                .is_some_and(model_source)
    })?;
    Some(ModelActionSignature {
        action_ref,
        source: support.payload.get("source")?.as_str()?.to_string(),
        completion_role: support
            .payload
            .get("completion_role")
            .and_then(Value::as_str)
            .unwrap_or("terminal")
            .to_string(),
    })
}

fn model_source(source: &str) -> bool {
    matches!(
        source,
        "model_proposal" | "model_observation_proposal" | "model_goal_verifier"
    )
}

fn artifact_rejection_prune_targets(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
    action_id: NodeId,
    support_source: NodeId,
    source: &str,
) -> Vec<NodeId> {
    let mut targets = explicit_dependency_closure(graph, action_id);
    for edge in &graph.edges {
        if edge.kind != PlanEdgeKind::Supports
            || edge.source != support_source
            || edge.target == action_id
            || edge.payload.get("source").and_then(Value::as_str) != Some(source)
        {
            continue;
        }
        let Some(node) = graph.nodes.get(&edge.target) else {
            continue;
        };
        if node.status != PlanStatus::Open || node.kind != PlanNodeKind::Action {
            continue;
        }
        if action_is_structurally_read_only(contracts, node.action_ref.as_ref()) {
            continue;
        }
        targets.push(edge.target);
    }
    targets.sort_by_key(|id| id.0);
    targets.dedup();
    targets
}

fn explicit_dependency_closure(graph: &PlanGraph, action_id: NodeId) -> Vec<NodeId> {
    let mut result = Vec::new();
    let mut queue = vec![action_id];
    while let Some(source) = queue.pop() {
        for edge in &graph.edges {
            if edge.source != source
                || !matches!(edge.kind, PlanEdgeKind::DependsOn | PlanEdgeKind::Requires)
                || result.contains(&edge.target)
            {
                continue;
            }
            result.push(edge.target);
            queue.push(edge.target);
        }
    }
    result
}

fn action_is_structurally_read_only(
    contracts: &dyn ContractRegistry,
    action_ref: Option<&ActionRef>,
) -> bool {
    let Some(action_ref) = action_ref else {
        return false;
    };
    contracts
        .get_action(&action_ref.contract_id, &action_ref.action_name)
        .is_some_and(|action| read_only_side_effect(&action.side_effect_class))
}

struct ModelActionSignature {
    action_ref: ActionRef,
    source: String,
    completion_role: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        ActionContract, ApprovalSpec, CapabilityContract, ContractStatus, Idempotency,
        InMemoryContractRegistry, Reversibility, RiskLevel, SideEffectClass, TrustLevel,
        VerificationSpec,
    };
    use crate::graph::{ActionScores, PlanNode};

    fn model_action(id: u64, status: PlanStatus) -> PlanNode {
        model_action_with_tool(id, status, "Bash")
    }

    fn model_action_with_tool(id: u64, status: PlanStatus, tool: &str) -> PlanNode {
        PlanNode {
            id: NodeId(id),
            kind: PlanNodeKind::Action,
            status,
            label: format!("puffer.tools.{tool}"),
            payload: json!({"command": format!("command-{id}")}),
            action_ref: Some(ActionRef {
                contract_id: "puffer.tools".to_string(),
                action_name: tool.to_string(),
            }),
            scores: ActionScores::default(),
        }
    }

    fn action_contract(name: &str, side_effect_class: SideEffectClass) -> ActionContract {
        ActionContract {
            name: name.to_string(),
            description: name.to_string(),
            input_schema: json!({"type": "object", "additionalProperties": true}),
            output_schema: json!({"type": "object"}),
            side_effect_class,
            reversibility: Reversibility::Unknown,
            idempotency: Idempotency::Unknown,
            risk_level: RiskLevel::Medium,
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

    fn contract_registry() -> InMemoryContractRegistry {
        let mut registry = InMemoryContractRegistry::default();
        registry
            .register(CapabilityContract {
                contract_id: "puffer.tools".to_string(),
                version: "0.1.0".to_string(),
                status: ContractStatus::Active,
                trust_level: TrustLevel::Sandboxed,
                description: "test tools".to_string(),
                actions: vec![
                    action_contract("Bash", SideEffectClass::Unknown),
                    action_contract("Read", SideEffectClass::LocalRead),
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

    fn add_support(
        graph: &mut PlanGraph,
        source_id: NodeId,
        target: NodeId,
        source: &str,
        role: &str,
    ) {
        graph.add_edge(
            source_id,
            target,
            PlanEdgeKind::Supports,
            json!({"source": source, "completion_role": role}),
        );
    }

    #[test]
    fn prunes_open_same_model_verifiers_after_failure() {
        let mut graph = PlanGraph::from_goal("goal".to_string());
        graph.add_node(model_action(1, PlanStatus::Failed));
        graph.add_node(model_action(2, PlanStatus::Open));
        add_support(
            &mut graph,
            NodeId(0),
            NodeId(1),
            "model_goal_verifier",
            "verification",
        );
        add_support(
            &mut graph,
            NodeId(0),
            NodeId(2),
            "model_goal_verifier",
            "verification",
        );

        let events = prune_stale_model_frontier_after_failure(RunId::new(), &mut graph, NodeId(1));

        assert_eq!(events.len(), 1);
        assert_eq!(graph.node(NodeId(2)).unwrap().status, PlanStatus::Pruned);
        assert_eq!(events[0].output["pruned"], json!([2]));
    }

    #[test]
    fn keeps_support_actions_after_verifier_failure() {
        let mut graph = PlanGraph::from_goal("goal".to_string());
        graph.add_node(model_action(1, PlanStatus::Failed));
        graph.add_node(model_action(2, PlanStatus::Open));
        add_support(
            &mut graph,
            NodeId(0),
            NodeId(1),
            "model_goal_verifier",
            "verification",
        );
        add_support(
            &mut graph,
            NodeId(0),
            NodeId(2),
            "model_goal_verifier",
            "support",
        );

        let events = prune_stale_model_frontier_after_failure(RunId::new(), &mut graph, NodeId(1));

        assert!(events.is_empty());
        assert_eq!(graph.node(NodeId(2)).unwrap().status, PlanStatus::Open);
    }

    #[test]
    fn prunes_open_same_model_repairs_after_no_progress_failure() {
        let mut graph = PlanGraph::from_goal("goal".to_string());
        let mut failed_node = model_action(1, PlanStatus::Failed);
        failed_node.id = NodeId(0);
        let failed = graph.add_node(failed_node);
        let mut sibling_node = model_action(2, PlanStatus::Open);
        sibling_node.id = NodeId(0);
        let sibling = graph.add_node(sibling_node);
        let failure = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Failure,
            status: PlanStatus::Open,
            label: "no progress".to_string(),
            payload: json!({"failure_mode": "no_progress"}),
            action_ref: None,
            scores: ActionScores::default(),
        });
        graph.add_edge(failed, failure, PlanEdgeKind::FailedWith, json!({}));
        add_support(
            &mut graph,
            NodeId(0),
            failed,
            "model_observation_proposal",
            "repair",
        );
        add_support(
            &mut graph,
            NodeId(0),
            sibling,
            "model_observation_proposal",
            "repair",
        );

        let events = prune_stale_model_frontier_after_failure(RunId::new(), &mut graph, failed);

        assert_eq!(events.len(), 1);
        assert_eq!(graph.node(sibling).unwrap().status, PlanStatus::Pruned);
    }

    #[test]
    fn artifact_review_failure_keeps_independent_same_batch_sibling() {
        let registry = contract_registry();
        let mut graph = PlanGraph::from_goal("goal".to_string());
        graph.add_node(model_action(1, PlanStatus::Failed));
        graph.add_node(model_action_with_tool(2, PlanStatus::Open, "Read"));
        add_support(
            &mut graph,
            NodeId(0),
            NodeId(1),
            "model_observation_proposal",
            "repair",
        );
        add_support(
            &mut graph,
            NodeId(0),
            NodeId(2),
            "model_observation_proposal",
            "verification",
        );

        let events = prune_stale_model_siblings_after_artifact_review_failed(
            RunId::new(),
            &mut graph,
            &registry,
            NodeId(1),
        );

        assert!(events.is_empty());
        assert_eq!(graph.node(NodeId(2)).unwrap().status, PlanStatus::Open);
    }

    #[test]
    fn artifact_review_failure_prunes_structural_dependents() {
        let registry = contract_registry();
        let mut graph = PlanGraph::from_goal("goal".to_string());
        graph.add_node(model_action(1, PlanStatus::Failed));
        graph.add_node(model_action(2, PlanStatus::Open));
        add_support(
            &mut graph,
            NodeId(0),
            NodeId(1),
            "model_observation_proposal",
            "repair",
        );
        add_support(
            &mut graph,
            NodeId(0),
            NodeId(2),
            "model_observation_proposal",
            "verification",
        );
        graph.add_edge(
            NodeId(1),
            NodeId(2),
            PlanEdgeKind::DependsOn,
            json!({"source": "model_candidate_dependency"}),
        );

        let events = prune_stale_model_siblings_after_artifact_review_failed(
            RunId::new(),
            &mut graph,
            &registry,
            NodeId(1),
        );

        assert_eq!(events.len(), 1);
        assert_eq!(graph.node(NodeId(2)).unwrap().status, PlanStatus::Pruned);
    }

    #[test]
    fn artifact_review_failure_prunes_same_batch_unknown_sibling_without_dependency() {
        let registry = contract_registry();
        let mut graph = PlanGraph::from_goal("goal".to_string());
        graph.add_node(model_action(1, PlanStatus::Failed));
        graph.add_node(model_action(2, PlanStatus::Open));
        add_support(
            &mut graph,
            NodeId(0),
            NodeId(1),
            "model_observation_proposal",
            "repair",
        );
        add_support(
            &mut graph,
            NodeId(0),
            NodeId(2),
            "model_observation_proposal",
            "verification",
        );

        let events = prune_stale_model_siblings_after_artifact_review_failed(
            RunId::new(),
            &mut graph,
            &registry,
            NodeId(1),
        );

        assert_eq!(events.len(), 1);
        assert_eq!(graph.node(NodeId(2)).unwrap().status, PlanStatus::Pruned);
    }
}
