use crate::contract::{ActionContract, SideEffectClass};
use crate::graph::{PlanEdgeKind, PlanGraph};
use crate::ids::NodeId;
use crate::planner::CompletionRole;
use crate::semantics::read_only_side_effect;
use serde_json::Value;

pub(super) struct ProgressEvidence {
    pub(super) changes_state: bool,
    pub(super) has_structural_witness: bool,
    pub(super) has_output_witness: bool,
}

/// Classifies structural progress evidence without inspecting natural-language command text.
pub(super) fn progress_evidence(
    action: Option<&ActionContract>,
    output: &Value,
) -> ProgressEvidence {
    let has_structural_witness = structured_object_has_witness(output)
        || non_empty_object_at(output, &["metadata"])
        || value_present_deep(output, "backgroundTaskId")
        || value_present_deep(output, "outputFile");
    let has_output_witness = string_at(output, &["stdout"]).is_some_and(non_empty_text)
        || string_at(output, &["stderr"]).is_some_and(non_empty_text)
        || string_at(output, &["error"]).is_some_and(non_empty_text)
        || has_structural_witness;
    let changes_state = action.is_some_and(|action| {
        if read_only_side_effect(&action.side_effect_class) {
            return false;
        }
        match action.side_effect_class {
            SideEffectClass::Unknown => has_structural_witness,
            _ => true,
        }
    });
    ProgressEvidence {
        changes_state,
        has_structural_witness,
        has_output_witness,
    }
}

/// Returns true when a model-generated unknown-side-effect terminal action made no progress.
pub(super) fn model_unknown_terminal_without_progress(
    action: Option<&ActionContract>,
    completion_role: CompletionRole,
    model_proposed: bool,
    progress: &ProgressEvidence,
) -> bool {
    action.is_some_and(|action| action.side_effect_class == SideEffectClass::Unknown)
        && model_proposed
        && matches!(
            completion_role,
            CompletionRole::Terminal | CompletionRole::Repair
        )
        && !progress.has_structural_witness
        && !progress.has_output_witness
}

/// Returns true when the selected node came from a structural model proposal source.
pub(super) fn model_proposed_node(graph: &PlanGraph, node_id: NodeId) -> bool {
    graph
        .edges
        .iter()
        .find(|edge| edge.target == node_id && edge.kind == PlanEdgeKind::Supports)
        .and_then(|edge| edge.payload.get("source"))
        .and_then(Value::as_str)
        .is_some_and(|source| {
            matches!(
                source,
                "model_proposal" | "model_observation_proposal" | "model_goal_verifier"
            )
        })
}

fn structured_object_has_witness(output: &Value) -> bool {
    value_at(output, &["structured_output"])
        .and_then(Value::as_object)
        .is_some_and(|object| {
            object.iter().any(|(key, value)| {
                !matches!(
                    key.as_str(),
                    "noOutputExpected" | "interrupted" | "success" | "exit_code"
                ) && concrete_value(value)
            })
        })
}

fn non_empty_object_at(output: &Value, path: &[&str]) -> bool {
    value_at(output, path)
        .and_then(Value::as_object)
        .is_some_and(|object| !object.is_empty())
}

fn value_present_deep(output: &Value, key: &str) -> bool {
    value_at(output, &[key]).is_some_and(concrete_value)
        || value_at(output, &["structured_output", key]).is_some_and(concrete_value)
        || value_at(output, &["metadata", key]).is_some_and(concrete_value)
}

fn string_at<'a>(output: &'a Value, path: &[&str]) -> Option<&'a str> {
    value_at(output, path).and_then(Value::as_str)
}

fn value_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}

fn concrete_value(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => non_empty_text(text),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

fn non_empty_text(text: &str) -> bool {
    !text.trim().is_empty()
}
