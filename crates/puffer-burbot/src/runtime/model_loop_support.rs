use crate::contract::{ActionContract, ContractRegistry};
use crate::graph::{PlanEdgeKind, PlanGraph, PlanNode, PlanNodeKind, PlanStatus};
use crate::ids::NodeId;
use crate::llm::ModelCandidateProposal;
use crate::semantics::{read_only_side_effect, semantic_symbol};
use serde_json::{json, Map, Value};

const MAX_MODEL_STRING_CHARS: usize = 6_000;
const MAX_MODEL_ARRAY_ITEMS: usize = 20;

pub(super) const AWAIT_ASYNC_PROGRESS_INTENT: &str = "await_async_progress";
pub(super) const CREATES_ASYNC_PROGRESS_INTENT: &str = "creates_async_progress";

pub(super) struct ModelProposalAttempt {
    pub(super) proposals: Vec<ModelCandidateProposal>,
    pub(super) retryable_error: Option<String>,
}

pub(super) struct SingleModelProposalAttempt {
    pub(super) proposal: Option<ModelCandidateProposal>,
    pub(super) retryable_error: Option<String>,
}

/// Returns whether the graph has any open executable action.
pub(super) fn graph_has_executable_frontier(graph: &PlanGraph) -> bool {
    !graph.executable_frontier_actions().is_empty()
}

/// Returns whether an executed action has declared asynchronous progress.
pub(super) fn graph_has_async_progress_source(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
) -> bool {
    graph.nodes.values().any(|node| {
        matches!(node.status, PlanStatus::Executed | PlanStatus::Satisfied)
            && node.kind == PlanNodeKind::Action
            && node
                .action_ref
                .as_ref()
                .and_then(|action_ref| {
                    contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
                })
                .is_some_and(|action| action_has_intent(&action, CREATES_ASYNC_PROGRESS_INTENT))
    })
}

/// Returns whether the action declares a normalized semantic intent.
pub(super) fn action_has_intent(action: &ActionContract, intent: &str) -> bool {
    action
        .semantic_intents
        .iter()
        .any(|declared| semantic_symbol(&declared.intent) == intent)
}

/// Builds a compact summary of current frontier actions for model context.
pub(super) fn frontier_summary(graph: &PlanGraph) -> Vec<Value> {
    graph
        .frontier_actions()
        .into_iter()
        .filter_map(|id| graph.node(id).ok())
        .take(20)
        .map(|node| {
            json!({
                "id": node.id.0,
                "label": node.label,
                "action_ref": node.action_ref,
                "args": compact_value(&node.payload),
            })
        })
        .collect()
}

/// Builds a compact action history while preserving goal-relevant older actions.
pub(super) fn action_history_summary(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
) -> Vec<Value> {
    let actions = graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, (id, node))| {
            (node.kind == PlanNodeKind::Action).then(|| {
                (
                    index,
                    preserves_goal_relevant_history(graph, contracts, *id, node),
                    json!({
                        "id": id.0,
                        "status": node.status,
                        "action_ref": node.action_ref,
                        "args": compact_value(&node.payload),
                        "output": observation_output_for_action(graph, *id).map(|value| compact_value(&value)),
                    }),
                )
            })
        })
        .collect::<Vec<_>>();
    let recent_start = actions.len().saturating_sub(40);
    actions
        .into_iter()
        .filter_map(|(index, goal_relevant, value)| {
            (index >= recent_start || goal_relevant).then_some(value)
        })
        .collect()
}

/// Accumulates image-like observations for multimodal model requests.
pub(super) fn accumulated_image_context(graph: &PlanGraph, current: Option<&Value>) -> Value {
    let mut values = Vec::new();
    if let Some(current) = current {
        values.push(current.clone());
    }
    for node in graph.nodes.values() {
        if node.kind == PlanNodeKind::Observation {
            values.push(node.payload.clone());
        }
    }
    Value::Array(values)
}

/// Compacts large JSON values before inserting them into model context.
pub(super) fn compact_value(value: &Value) -> Value {
    match value {
        Value::String(text) if text.chars().count() > MAX_MODEL_STRING_CHARS => {
            let prefix = text
                .chars()
                .take(MAX_MODEL_STRING_CHARS)
                .collect::<String>();
            json!({
                "truncated_string_prefix": prefix,
                "original_chars": text.chars().count(),
            })
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(MAX_MODEL_ARRAY_ITEMS)
                .map(compact_value)
                .collect(),
        ),
        Value::Object(object) => {
            if let Some(image) = compact_image_metadata(object) {
                return image;
            }
            let mut compact = Map::new();
            for (key, value) in object.iter().take(MAX_MODEL_ARRAY_ITEMS) {
                compact.insert(key.clone(), compact_value(value));
            }
            Value::Object(compact)
        }
        other => other.clone(),
    }
}

/// Finds the supporting proposal edge for an action node.
pub(super) fn model_support_edge_for(
    graph: &PlanGraph,
    action_id: NodeId,
) -> Option<(NodeId, String)> {
    graph
        .edges
        .iter()
        .find(|edge| edge.target == action_id && edge.kind == PlanEdgeKind::Supports)
        .and_then(|edge| {
            edge.payload
                .get("source")
                .and_then(Value::as_str)
                .map(|source| (edge.source, source.to_string()))
        })
}

fn preserves_goal_relevant_history(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
    node_id: NodeId,
    node: &PlanNode,
) -> bool {
    let completion_role = graph
        .edges
        .iter()
        .find(|edge| edge.target == node_id && edge.kind == PlanEdgeKind::Supports)
        .and_then(|edge| edge.payload.get("completion_role"))
        .and_then(Value::as_str)
        .unwrap_or("terminal");
    if matches!(completion_role, "terminal" | "repair") {
        return true;
    }
    node.action_ref
        .as_ref()
        .and_then(|action_ref| {
            contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
        })
        .is_some_and(|action| {
            !read_only_side_effect(&action.side_effect_class)
                && !matches!(completion_role, "verification" | "support")
        })
}

fn observation_output_for_action(graph: &PlanGraph, action_id: NodeId) -> Option<Value> {
    graph
        .edges
        .iter()
        .find(|edge| edge.source == action_id && edge.kind == PlanEdgeKind::Produces)
        .and_then(|edge| graph.nodes.get(&edge.target))
        .map(|node| node.payload.clone())
}

fn compact_image_metadata(object: &Map<String, Value>) -> Option<Value> {
    let mime = string_field(object, &["type", "mime_type", "media_type"])?;
    if !mime.starts_with("image/") {
        return None;
    }
    let _ = string_field(object, &["base64", "data"])?;
    Some(json!({
        "type": "image_observation",
        "mime_type": mime,
        "base64_attached_to_model": true,
        "original_size": object.get("originalSize")
            .or_else(|| object.get("original_size"))
            .cloned()
            .unwrap_or(Value::Null),
    }))
}

fn string_field<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
}
