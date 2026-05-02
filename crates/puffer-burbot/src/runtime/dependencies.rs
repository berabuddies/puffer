use crate::graph::{PlanEdgeKind, PlanGraph, PlanNodeKind, PlanStatus};
use crate::ids::{NodeId, RunId};
use crate::trace::{trace_event, TraceEvent, TraceEventType};
use serde_json::{json, Value};

/// Blocks open actions whose required dependency can no longer satisfy them.
pub(super) fn block_actions_with_dead_dependencies(
    run_id: RunId,
    graph: &mut PlanGraph,
) -> Vec<TraceEvent> {
    let blocked = graph
        .frontier_actions()
        .into_iter()
        .filter_map(|node_id| {
            let dependencies = dead_blocking_dependencies(graph, node_id);
            (!dependencies.is_empty()).then_some((node_id, dependencies))
        })
        .collect::<Vec<_>>();
    let mut events = Vec::new();
    for (node_id, dependencies) in blocked {
        if let Ok(node) = graph.node_mut(node_id) {
            node.status = PlanStatus::Blocked;
        }
        events.push(trace_event(
            run_id,
            TraceEventType::RewriteApplied,
            Some(node_id),
            json!({"rule": "BlockActionsWithDeadDependencies"}),
            json!({"blocked_dependencies": dependencies}),
        ));
    }
    events
}

fn dead_blocking_dependencies(graph: &PlanGraph, target: NodeId) -> Vec<Value> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.target == target && blocking_edge_kind(&edge.kind))
        .filter_map(|edge| {
            let source = graph.nodes.get(&edge.source)?;
            (source.kind == PlanNodeKind::Action && dead_dependency_status(&source.status)).then(
                || {
                    json!({
                        "source": edge.source.0,
                        "kind": edge.kind,
                        "status": source.status,
                    })
                },
            )
        })
        .collect()
}

fn blocking_edge_kind(kind: &PlanEdgeKind) -> bool {
    matches!(kind, PlanEdgeKind::DependsOn | PlanEdgeKind::Requires)
}

fn dead_dependency_status(status: &PlanStatus) -> bool {
    matches!(
        status,
        PlanStatus::Blocked | PlanStatus::Failed | PlanStatus::Pruned
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{ActionRef, ActionScores, PlanEdgeKind, PlanNode};
    use serde_json::json;

    fn action_node(id: u64, status: PlanStatus, command: &str) -> PlanNode {
        PlanNode {
            id: NodeId(id),
            kind: PlanNodeKind::Action,
            status,
            label: "action".to_string(),
            payload: json!({"command": command}),
            action_ref: Some(ActionRef {
                contract_id: "tools".to_string(),
                action_name: "Bash".to_string(),
            }),
            scores: ActionScores::default(),
        }
    }

    #[test]
    fn blocks_open_action_when_dependency_failed() {
        let mut graph = PlanGraph::from_goal("goal".to_string());
        graph.add_node(action_node(1, PlanStatus::Failed, "first"));
        graph.add_node(action_node(2, PlanStatus::Open, "second"));
        graph.add_edge(NodeId(1), NodeId(2), PlanEdgeKind::DependsOn, json!({}));

        let events = block_actions_with_dead_dependencies(RunId::new(), &mut graph);

        assert_eq!(events.len(), 1);
        assert_eq!(graph.node(NodeId(2)).unwrap().status, PlanStatus::Blocked);
        assert_eq!(
            events[0].output["blocked_dependencies"][0]["source"],
            json!(1)
        );
    }

    #[test]
    fn leaves_open_action_waiting_on_open_dependency() {
        let mut graph = PlanGraph::from_goal("goal".to_string());
        graph.add_node(action_node(1, PlanStatus::Open, "first"));
        graph.add_node(action_node(2, PlanStatus::Open, "second"));
        graph.add_edge(NodeId(1), NodeId(2), PlanEdgeKind::DependsOn, json!({}));

        let events = block_actions_with_dead_dependencies(RunId::new(), &mut graph);

        assert!(events.is_empty());
        assert_eq!(graph.node(NodeId(2)).unwrap().status, PlanStatus::Open);
    }
}
