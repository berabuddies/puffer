use super::model_loop_support::{compact_history_value, compact_value, failure_summary_for_action};
use crate::contract::{ContractRegistry, SideEffectClass};
use crate::graph::{PlanEdgeKind, PlanGraph, PlanNodeKind};
use crate::ids::NodeId;
use crate::semantics::read_only_side_effect;
use serde_json::{json, Value};

const MAX_GENERATED_ARTIFACTS: usize = 8;

/// Builds structured model context for prior model-generated artifact actions.
pub(super) fn generated_artifact_context(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
    excluded_action: Option<NodeId>,
) -> Value {
    let mut artifacts = graph
        .nodes
        .iter()
        .filter_map(|(id, node)| {
            if excluded_action == Some(*id) || node.kind != PlanNodeKind::Action {
                return None;
            }
            let action_ref = node.action_ref.as_ref()?;
            let action = contracts.get_action(&action_ref.contract_id, &action_ref.action_name)?;
            if !model_proposed(graph, *id) || read_only_side_effect(&action.side_effect_class) {
                return None;
            }
            let output = observation_output_for_action(graph, *id);
            if !artifact_like_action(&action.side_effect_class, output.as_ref()) {
                return None;
            }
            Some(json!({
                "action_id": id.0,
                "status": node.status,
                "action_ref": action_ref,
                "completion_role": completion_role_for_node(graph, *id),
                "args": compact_value(&node.payload),
                "output": output.as_ref().map(compact_history_value),
                "failure": failure_summary_for_action(graph, *id).map(|value| compact_history_value(&value)),
            }))
        })
        .collect::<Vec<_>>();
    if artifacts.len() > MAX_GENERATED_ARTIFACTS {
        let keep_from = artifacts.len().saturating_sub(MAX_GENERATED_ARTIFACTS);
        artifacts = artifacts.into_iter().skip(keep_from).collect();
    }
    json!({
        "items": artifacts,
        "truncated_to_recent": MAX_GENERATED_ARTIFACTS,
    })
}

fn artifact_like_action(side_effect_class: &SideEffectClass, output: Option<&Value>) -> bool {
    match side_effect_class {
        SideEffectClass::LocalWrite | SideEffectClass::DestructiveWrite => true,
        SideEffectClass::Unknown => output.is_some_and(output_has_workspace_witness),
        _ => false,
    }
}

fn output_has_workspace_witness(output: &Value) -> bool {
    let Some(witness) = output.get("filesystem_witness") else {
        return false;
    };
    ["created", "modified", "removed"].iter().any(|key| {
        witness
            .get(*key)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    })
}

fn model_proposed(graph: &PlanGraph, action_id: NodeId) -> bool {
    graph
        .edges
        .iter()
        .find(|edge| edge.target == action_id && edge.kind == PlanEdgeKind::Supports)
        .and_then(|edge| edge.payload.get("source"))
        .and_then(Value::as_str)
        .is_some_and(|source| {
            matches!(
                source,
                "model_proposal" | "model_observation_proposal" | "model_goal_verifier"
            )
        })
}

fn completion_role_for_node(graph: &PlanGraph, action_id: NodeId) -> Option<String> {
    graph
        .edges
        .iter()
        .find(|edge| edge.target == action_id && edge.kind == PlanEdgeKind::Supports)
        .and_then(|edge| edge.payload.get("completion_role"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn observation_output_for_action(graph: &PlanGraph, action_id: NodeId) -> Option<Value> {
    graph
        .edges
        .iter()
        .find(|edge| edge.source == action_id && edge.kind == PlanEdgeKind::Produces)
        .and_then(|edge| graph.nodes.get(&edge.target))
        .map(|node| node.payload.clone())
}
