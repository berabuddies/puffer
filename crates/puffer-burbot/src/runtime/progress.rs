use crate::contract::{ActionContract, SideEffectClass};
use crate::graph::{PlanEdgeKind, PlanGraph};
use crate::ids::NodeId;
use crate::planner::CompletionRole;
use crate::semantics::read_only_side_effect;
use serde_json::Value;

pub(super) struct ProgressEvidence {
    pub(super) changes_state: bool,
    pub(super) has_state_witness: bool,
    pub(super) has_structural_witness: bool,
    pub(super) has_output_witness: bool,
}

/// Classifies structural progress evidence without inspecting natural-language command text.
pub(super) fn progress_evidence(
    action: Option<&ActionContract>,
    output: &Value,
) -> ProgressEvidence {
    let has_declared_output_witness = explicit_output_witness(output);
    let has_structural_witness = workspace_witness_has_changes(output)
        || value_present_deep(output, "backgroundTaskId")
        || value_present_deep(output, "outputFile")
        || has_declared_output_witness;
    let has_state_witness = workspace_witness_has_changes(output)
        || value_present_deep(output, "backgroundTaskId")
        || value_present_deep(output, "outputFile")
        || value_present_at_any_path(
            output,
            &[
                &["filePath"],
                &["file_path"],
                &["file", "filePath"],
                &["file", "file_path"],
                &["structured_output", "filePath"],
                &["structured_output", "file_path"],
                &["structured_output", "file", "filePath"],
                &["structured_output", "file", "file_path"],
            ],
        );
    let has_output_witness = has_declared_output_witness || has_structural_witness;
    let changes_state = action.is_some_and(|action| {
        if read_only_side_effect(&action.side_effect_class) {
            return false;
        }
        has_state_witness
    });
    ProgressEvidence {
        changes_state,
        has_state_witness,
        has_structural_witness,
        has_output_witness,
    }
}

/// Returns true when a model-generated unknown-side-effect completion action made no progress.
pub(super) fn model_unknown_terminal_without_progress(
    action: Option<&ActionContract>,
    completion_role: CompletionRole,
    model_proposed: bool,
    progress: &ProgressEvidence,
) -> bool {
    action.is_some_and(|action| action.side_effect_class == SideEffectClass::Unknown)
        && model_proposed
        && match completion_role {
            CompletionRole::Repair => !progress.has_structural_witness,
            CompletionRole::Terminal => {
                !progress.has_structural_witness && !progress.has_output_witness
            }
            CompletionRole::Support | CompletionRole::Verification => false,
        }
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

fn workspace_witness_has_changes(output: &Value) -> bool {
    let Some(witness) = value_at(output, &["filesystem_witness"]) else {
        return false;
    };
    ["created", "modified", "removed"].iter().any(|key| {
        witness
            .get(*key)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    })
}

fn value_present_deep(output: &Value, key: &str) -> bool {
    value_at(output, &[key]).is_some_and(concrete_value)
        || value_at(output, &["structured_output", key]).is_some_and(concrete_value)
}

fn value_present_at_any_path(output: &Value, paths: &[&[&str]]) -> bool {
    paths
        .iter()
        .any(|path| value_at(output, path).is_some_and(concrete_value))
}

fn explicit_output_witness(output: &Value) -> bool {
    let paths: &[&[&str]] = &[
        &["produced_output"],
        &["progress_kind"],
        &["artifact_ids"],
        &["verification_evidence"],
        &["structured_output", "produced_output"],
        &["structured_output", "progress_kind"],
        &["structured_output", "artifact_ids"],
        &["structured_output", "verification_evidence"],
    ];
    paths
        .iter()
        .any(|path| value_at(output, path).is_some_and(explicit_witness_value))
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

fn explicit_witness_value(value: &Value) -> bool {
    match value {
        Value::Null | Value::String(_) => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
    }
}
