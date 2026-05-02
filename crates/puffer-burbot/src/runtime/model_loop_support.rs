use crate::contract::{ActionContract, ContractRegistry};
use crate::graph::{PlanEdgeKind, PlanGraph, PlanNode, PlanNodeKind, PlanStatus};
use crate::ids::NodeId;
use crate::llm::ModelCandidateProposal;
use crate::model_policy::ModelProposalViolation;
use crate::semantics::{read_only_side_effect, semantic_symbol};
use serde_json::{json, Map, Value};

const MAX_MODEL_STRING_CHARS: usize = 16_000;
const MAX_MODEL_HISTORY_STRING_CHARS: usize = 4_000;
const MAX_ARTIFACT_REVIEW_HISTORY_STRING_CHARS: usize = 512;
const MAX_MODEL_ARRAY_ITEMS: usize = 20;

pub(super) const AWAIT_ASYNC_PROGRESS_INTENT: &str = "await_async_progress";
pub(super) const CREATES_ASYNC_PROGRESS_INTENT: &str = "creates_async_progress";

pub(super) struct ModelProposalAttempt {
    pub(super) proposals: Vec<ModelCandidateProposal>,
    pub(super) retryable_error: Option<String>,
    pub(super) structural_error: Option<String>,
    pub(super) structural_violations: Vec<ModelProposalViolation>,
}

pub(super) struct SingleModelProposalAttempt {
    pub(super) proposal: Option<ModelCandidateProposal>,
    pub(super) retryable_error: Option<String>,
    pub(super) structural_error: Option<String>,
    pub(super) structural_violations: Vec<ModelProposalViolation>,
}

/// Summarizes how model-proposed candidates changed the plan graph.
pub(super) struct ModelCandidateProposalOutcome {
    pub(super) added: usize,
    pub(super) proposed: usize,
    pub(super) skipped_wait: usize,
    pub(super) skipped_duplicate: usize,
    pub(super) skipped_dependency: usize,
    pub(super) skipped_policy: usize,
    pub(super) skipped_duplicate_candidates: Vec<Value>,
    pub(super) skipped_policy_candidates: Vec<Value>,
}

impl ModelCandidateProposalOutcome {
    /// Returns whether the model proposed actions but none added new executable work.
    pub(super) fn is_non_advancing(&self) -> bool {
        self.added == 0
            && self.proposed > 0
            && (self.skipped_wait > 0
                || self.skipped_duplicate > 0
                || self.skipped_dependency > 0
                || self.skipped_policy > 0)
    }
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

/// Builds structural context for the action proven by a verifier node.
pub(super) fn verified_target_summary(graph: &PlanGraph, verifier_id: NodeId) -> Option<Value> {
    let target_id = graph
        .edges
        .iter()
        .find(|edge| edge.source == verifier_id && edge.kind == PlanEdgeKind::Verifies)
        .map(|edge| edge.target)?;
    let target = graph.node(target_id).ok()?;
    Some(json!({
        "id": target_id.0,
        "status": target.status,
        "action_ref": target.action_ref,
        "args": compact_value(&target.payload),
        "output": observation_output_for_action(graph, target_id).map(|value| compact_value(&value)),
    }))
}

/// Builds a compact action history while preserving goal-relevant older actions.
pub(super) fn action_history_summary(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
) -> Vec<Value> {
    action_history_summary_excluding(graph, contracts, None)
}

/// Builds compact action history while omitting a current action already in context.
pub(super) fn action_history_summary_excluding(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
    excluded_action: Option<NodeId>,
) -> Vec<Value> {
    let actions = graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, (id, node))| {
            if excluded_action == Some(*id) {
                return None;
            }
            (node.kind == PlanNodeKind::Action).then(|| {
                (
                    index,
                    preserves_goal_relevant_history(graph, contracts, *id, node),
                    json!({
                        "id": id.0,
                        "status": node.status,
                        "action_ref": node.action_ref,
                        "args": compact_history_value(&node.payload),
                        "output": observation_output_for_action(graph, *id).map(|value| compact_history_value(&value)),
                        "failure": failure_summary_for_action(graph, *id),
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

/// Builds compact structural history for generated-artifact review.
pub(super) fn artifact_review_history_summary_excluding(
    graph: &PlanGraph,
    contracts: &dyn ContractRegistry,
    excluded_action: Option<NodeId>,
) -> Vec<Value> {
    let actions = graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, (id, node))| {
            if excluded_action == Some(*id) {
                return None;
            }
            (node.kind == PlanNodeKind::Action).then(|| {
                (
                    index,
                    preserves_goal_relevant_history(graph, contracts, *id, node),
                    json!({
                        "id": id.0,
                        "status": node.status,
                        "action_ref": node.action_ref,
                        "args": compact_artifact_review_history_value(&node.payload),
                        "output": observation_output_for_action(graph, *id)
                            .map(|value| compact_artifact_review_history_value(&value)),
                        "failure": failure_summary_for_action(graph, *id)
                            .map(|value| compact_artifact_review_history_value(&value)),
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

/// Builds a compact failure summary for an action, if one has been classified.
pub(super) fn failure_summary_for_action(graph: &PlanGraph, action_id: NodeId) -> Option<Value> {
    graph
        .edges
        .iter()
        .find(|edge| edge.source == action_id && edge.kind == PlanEdgeKind::FailedWith)
        .and_then(|edge| graph.nodes.get(&edge.target))
        .map(|node| compact_history_value(&node.payload))
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
    compact_value_with_limit(value, MAX_MODEL_STRING_CHARS)
}

/// Compacts older history values more aggressively than the current observation.
pub(super) fn compact_history_value(value: &Value) -> Value {
    compact_value_with_limit(value, MAX_MODEL_HISTORY_STRING_CHARS)
}

fn compact_artifact_review_history_value(value: &Value) -> Value {
    compact_metadata_value_with_limit(value, MAX_ARTIFACT_REVIEW_HISTORY_STRING_CHARS)
}

fn compact_value_with_limit(value: &Value, max_string_chars: usize) -> Value {
    match value {
        Value::String(text) if text.chars().count() > max_string_chars => {
            let prefix = text.chars().take(max_string_chars).collect::<String>();
            json!({
                "truncated_string_prefix": prefix,
                "original_chars": text.chars().count(),
            })
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(MAX_MODEL_ARRAY_ITEMS)
                .map(|value| compact_value_with_limit(value, max_string_chars))
                .collect(),
        ),
        Value::Object(object) => {
            if let Some(image) = compact_image_metadata(object) {
                return image;
            }
            if let Some(output) = compact_structured_tool_output(object, max_string_chars) {
                return output;
            }
            let mut compact = Map::new();
            for (key, value) in object.iter().take(MAX_MODEL_ARRAY_ITEMS) {
                compact.insert(
                    key.clone(),
                    compact_value_with_limit(value, max_string_chars),
                );
            }
            Value::Object(compact)
        }
        other => other.clone(),
    }
}

fn compact_metadata_value_with_limit(value: &Value, max_string_chars: usize) -> Value {
    match value {
        Value::String(text) if text.chars().count() > max_string_chars => {
            omitted_string_summary(value)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(MAX_MODEL_ARRAY_ITEMS)
                .map(|value| compact_metadata_value_with_limit(value, max_string_chars))
                .collect(),
        ),
        Value::Object(object) => {
            if let Some(image) = compact_image_metadata(object) {
                return image;
            }
            let mut compact = Map::new();
            for (key, value) in object.iter().take(MAX_MODEL_ARRAY_ITEMS) {
                compact.insert(
                    key.clone(),
                    compact_metadata_value_with_limit(value, max_string_chars),
                );
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
        .and_then(Value::as_str);
    if matches!(completion_role, Some("terminal" | "repair")) {
        return true;
    }
    node.action_ref
        .as_ref()
        .and_then(|action_ref| {
            contracts.get_action(&action_ref.contract_id, &action_ref.action_name)
        })
        .is_some_and(|action| {
            !read_only_side_effect(&action.side_effect_class)
                && !matches!(completion_role, Some("verification" | "support"))
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

fn compact_structured_tool_output(
    object: &Map<String, Value>,
    max_string_chars: usize,
) -> Option<Value> {
    let structured_output = object.get("structured_output")?;
    let trusted_structured_output = object
        .get("structured_output_source")
        .and_then(Value::as_str)
        .is_some();
    if !object.contains_key("stdout") {
        return None;
    }
    let mut compact = Map::new();
    for (key, value) in object {
        match key.as_str() {
            "structured_output" => {}
            "stdout" if empty_string(value) || trusted_structured_output => {}
            "stderr" if empty_string(value) => {}
            _ => {
                compact.insert(
                    key.clone(),
                    compact_value_with_limit(value, max_string_chars),
                );
            }
        }
    }
    compact.insert(
        "structured_output".to_string(),
        compact_structured_output_payload(structured_output, max_string_chars),
    );
    Some(Value::Object(compact))
}

fn compact_structured_output_payload(value: &Value, max_string_chars: usize) -> Value {
    let Some(object) = value.as_object() else {
        return compact_value_with_limit(value, max_string_chars);
    };
    if !looks_like_file_write_echo(object) {
        return compact_value_with_limit(value, max_string_chars);
    }
    let mut compact = Map::new();
    for (key, value) in object {
        match key.as_str() {
            "content" | "gitDiff" | "git_diff" | "structuredPatch" | "structured_patch"
            | "originalFile" | "original_file" => {
                compact.insert(key.clone(), omitted_string_summary(value));
            }
            _ => {
                compact.insert(
                    key.clone(),
                    compact_value_with_limit(value, max_string_chars),
                );
            }
        }
    }
    Value::Object(compact)
}

fn looks_like_file_write_echo(object: &Map<String, Value>) -> bool {
    (object.contains_key("filePath") || object.contains_key("file_path"))
        && object.get("content").is_some_and(Value::is_string)
}

fn omitted_string_summary(value: &Value) -> Value {
    json!({
        "omitted_from_context": true,
        "original_chars": value.as_str().map(|text| text.chars().count()).unwrap_or(0),
    })
}

fn empty_string(value: &Value) -> bool {
    value.as_str().is_some_and(str::is_empty)
}

fn string_field<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
}
