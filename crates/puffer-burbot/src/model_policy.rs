use crate::contract::{ActionContract, SideEffectClass};
use crate::graph::ActionScores;
use crate::planner::CompletionRole;
use crate::semantics::{read_only_side_effect, semantic_symbol};
use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::{error::Error as StdError, fmt};

pub(crate) const MAX_MODEL_EXECUTABLE_ARG_CHARS: usize = 16_384;
pub(crate) const MAX_MODEL_EDIT_REPLACEMENT_ARG_LINES: usize = 64;
pub(crate) const MAX_MODEL_FOREGROUND_TIMEOUT_MS: u64 = 600_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct ModelProposalViolation {
    pub(crate) code: &'static str,
    pub(crate) path: String,
    pub(crate) tool_id: Option<String>,
    pub(crate) actual: Option<u64>,
    pub(crate) limit: Option<u64>,
    pub(crate) repair_hint: &'static str,
}

#[derive(Debug)]
pub(crate) struct ModelPolicyError {
    message: String,
    violations: Vec<ModelProposalViolation>,
}

impl ModelPolicyError {
    fn single(message: String, violation: ModelProposalViolation) -> Self {
        Self {
            message,
            violations: vec![violation],
        }
    }

    /// Returns the structured policy violations carried by this error.
    pub(crate) fn violations(&self) -> &[ModelProposalViolation] {
        &self.violations
    }
}

impl fmt::Display for ModelPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl StdError for ModelPolicyError {}

/// Extracts structured model policy violations from an error chain.
pub(crate) fn model_policy_violations(error: &anyhow::Error) -> Vec<ModelProposalViolation> {
    error
        .chain()
        .find_map(|cause| {
            cause
                .downcast_ref::<ModelPolicyError>()
                .map(|error| error.violations().to_vec())
        })
        .unwrap_or_default()
}

/// Returns the input schema Burbot accepts from model-originated proposals.
pub(crate) fn model_proposal_args_schema(action: &ActionContract) -> Value {
    let mut schema = action.input_schema.clone();
    let executable_args = executable_arg_names(action);
    if executable_args.is_empty() {
        return schema;
    }
    constrain_executable_arg_schema(&mut schema, &executable_args);
    schema
}

/// Validates structural model-proposal bounds that are stricter than tool contracts.
pub(crate) fn validate_model_tool_args(
    action: &ActionContract,
    args: &Value,
    role: Option<CompletionRole>,
) -> Result<()> {
    validate_structural_action_args(action, args)?;
    let executable_args = executable_arg_names(action);
    if executable_args.is_empty() {
        validate_completion_role_side_effect(action, role)?;
        return Ok(());
    }
    let Some(object) = args.as_object() else {
        return Ok(());
    };
    for arg in &executable_args {
        let Some(text) = object.get(arg).and_then(Value::as_str) else {
            continue;
        };
        let chars = text.chars().count();
        if chars > MAX_MODEL_EXECUTABLE_ARG_CHARS {
            let message = format!(
                "model proposal args.{arg} exceeds executable argument limit: {chars} > {MAX_MODEL_EXECUTABLE_ARG_CHARS}; decompose generated programs or long command payloads into separate file/artifact actions and bounded execution"
            );
            return Err(ModelPolicyError::single(
                message,
                ModelProposalViolation {
                    code: "executable_arg_char_limit_exceeded",
                    path: format!("args.{arg}"),
                    tool_id: Some(action.name.clone()),
                    actual: Some(chars as u64),
                    limit: Some(MAX_MODEL_EXECUTABLE_ARG_CHARS as u64),
                    repair_hint: "decompose generated programs or long command payloads into file/artifact actions and bounded execution",
                },
            )
            .into());
        }
    }
    if foreground_timeout_ms(args).is_some_and(|timeout| timeout > MAX_MODEL_FOREGROUND_TIMEOUT_MS)
    {
        let timeout = foreground_timeout_ms(args).unwrap_or_default();
        let message = format!(
            "model proposal args.timeout exceeds foreground timeout limit: {timeout} > {MAX_MODEL_FOREGROUND_TIMEOUT_MS}; decompose long-running work into bounded actions"
        );
        return Err(ModelPolicyError::single(
            message,
            ModelProposalViolation {
                code: "foreground_timeout_limit_exceeded",
                path: "args.timeout".to_string(),
                tool_id: Some(action.name.clone()),
                actual: Some(timeout),
                limit: Some(MAX_MODEL_FOREGROUND_TIMEOUT_MS),
                repair_hint: "decompose long-running work into bounded actions",
            },
        )
        .into());
    }
    if role == Some(CompletionRole::Terminal) && run_in_background(args) {
        return Err(ModelPolicyError::single(
            "model proposal cannot use run_in_background=true with completion_role=terminal"
                .to_string(),
            ModelProposalViolation {
                code: "terminal_background_execution_forbidden",
                path: "args.run_in_background".to_string(),
                tool_id: Some(action.name.clone()),
                actual: None,
                limit: None,
                repair_hint: "use support or repair role for background work, then add an explicit verification or terminal candidate",
            },
        )
        .into());
    }
    validate_completion_role_side_effect(action, role)?;
    Ok(())
}

fn validate_structural_action_args(action: &ActionContract, args: &Value) -> Result<()> {
    let Some((old_arg, new_arg)) = exact_replacement_arg_names(action) else {
        return Ok(());
    };
    let Some(object) = args.as_object() else {
        return Ok(());
    };
    for arg in [old_arg, new_arg] {
        let Some(text) = object.get(arg).and_then(Value::as_str) else {
            continue;
        };
        let lines = text.lines().count();
        if lines > MAX_MODEL_EDIT_REPLACEMENT_ARG_LINES {
            let message = format!(
                "model proposal args.{arg} exceeds edit replacement line limit: {lines} > {MAX_MODEL_EDIT_REPLACEMENT_ARG_LINES}; use whole-file write for complete rewrites or a smaller exact edit for local changes"
            );
            return Err(ModelPolicyError::single(
                message,
                ModelProposalViolation {
                    code: "edit_replacement_line_limit_exceeded",
                    path: format!("args.{arg}"),
                    tool_id: Some(action.name.clone()),
                    actual: Some(lines as u64),
                    limit: Some(MAX_MODEL_EDIT_REPLACEMENT_ARG_LINES as u64),
                    repair_hint: "use Write for complete file rewrites, or submit a smaller exact Edit for local changes",
                },
            )
            .into());
        }
    }
    Ok(())
}

fn exact_replacement_arg_names(action: &ActionContract) -> Option<(&'static str, &'static str)> {
    let properties = action
        .input_schema
        .get("properties")
        .and_then(Value::as_object)?;
    (properties.contains_key("old_string") && properties.contains_key("new_string"))
        .then_some(("old_string", "new_string"))
}

fn validate_completion_role_side_effect(
    action: &ActionContract,
    role: Option<CompletionRole>,
) -> Result<()> {
    match role {
        Some(CompletionRole::Support) if action.side_effect_class == SideEffectClass::Unknown => {
            Err(ModelPolicyError::single(
                "model proposal cannot use unknown-side-effect tools with completion_role=support; use read-only tools for support, or mark bounded execution as repair, verification, or terminal"
                    .to_string(),
                ModelProposalViolation {
                    code: "unknown_side_effect_support_forbidden",
                    path: "completion_role".to_string(),
                    tool_id: Some(action.name.clone()),
                    actual: None,
                    limit: None,
                    repair_hint: "use read-only tools for support, or mark bounded execution as repair, verification, or terminal",
                },
            )
            .into())
        }
        Some(CompletionRole::Support) if !read_only_side_effect(&action.side_effect_class) => {
            Err(ModelPolicyError::single(
                "model proposal cannot use side-effecting tools with completion_role=support; use read-only tools for support, or mark bounded state-changing work as repair, verification, or terminal"
                    .to_string(),
                ModelProposalViolation {
                    code: "support_side_effect_forbidden",
                    path: "completion_role".to_string(),
                    tool_id: Some(action.name.clone()),
                    actual: None,
                    limit: None,
                    repair_hint: "use read-only tools for support, or use repair, verification, or terminal for bounded state-changing work",
                },
            )
            .into())
        }
        Some(CompletionRole::Repair) if read_only_side_effect(&action.side_effect_class) => {
            Err(ModelPolicyError::single(
                "model proposal cannot use read-only tools with completion_role=repair; use support or verification for observations, or choose a state-changing repair tool"
                    .to_string(),
                ModelProposalViolation {
                    code: "repair_read_only_forbidden",
                    path: "completion_role".to_string(),
                    tool_id: Some(action.name.clone()),
                    actual: None,
                    limit: None,
                    repair_hint: "use support or verification for read-only observations, or choose a state-changing repair tool",
                },
            )
            .into())
        }
        Some(CompletionRole::Verification)
            if matches!(
                action.side_effect_class,
                SideEffectClass::LocalWrite
                    | SideEffectClass::ExternalWrite
                    | SideEffectClass::DestructiveWrite
            ) =>
        {
            Err(ModelPolicyError::single(
                "model proposal cannot use write tools with completion_role=verification; use read-only checks or bounded executable verification"
                    .to_string(),
                ModelProposalViolation {
                    code: "verification_write_forbidden",
                    path: "completion_role".to_string(),
                    tool_id: Some(action.name.clone()),
                    actual: None,
                    limit: None,
                    repair_hint: "use read-only checks or bounded executable verification instead of write tools",
                },
            )
            .into())
        }
        Some(CompletionRole::Verification)
            if !read_only_side_effect(&action.side_effect_class)
                && action.side_effect_class != SideEffectClass::Unknown =>
        {
            Err(ModelPolicyError::single(
                "model proposal cannot use side-effecting tools with completion_role=verification; use read-only checks or bounded executable verification"
                    .to_string(),
                ModelProposalViolation {
                    code: "verification_side_effect_forbidden",
                    path: "completion_role".to_string(),
                    tool_id: Some(action.name.clone()),
                    actual: None,
                    limit: None,
                    repair_hint: "use read-only tools or bounded unknown-side-effect execution for verification",
                },
            )
            .into())
        }
        _ => Ok(()),
    }
}

/// Adds structural latency/cost uncertainty for model-originated executable payloads.
pub(crate) fn adjust_model_action_scores(
    action: &ActionContract,
    args: &Value,
    scores: &mut ActionScores,
) {
    let executable_args = executable_arg_names(action);
    if executable_args.is_empty() {
        return;
    }
    let longest_ratio = executable_args
        .iter()
        .filter_map(|arg| args.get(arg).and_then(Value::as_str))
        .map(|text| text.chars().count() as f64 / MAX_MODEL_EXECUTABLE_ARG_CHARS as f64)
        .fold(0.0_f64, f64::max)
        .clamp(0.0, 1.0);
    scores.cost_penalty += 0.25 * longest_ratio;
    scores.uncertainty_penalty += 0.35 * longest_ratio;

    if let Some(timeout) = foreground_timeout_ms(args) {
        let timeout_ratio =
            (timeout as f64 / MAX_MODEL_FOREGROUND_TIMEOUT_MS as f64).clamp(0.0, 1.0);
        scores.latency_penalty += 0.9 * timeout_ratio;
        scores.uncertainty_penalty += 0.25 * timeout_ratio;
    }
    if run_in_background(args) {
        scores.latency_penalty += 0.4;
        scores.uncertainty_penalty += 0.4;
    }
}

fn constrain_executable_arg_schema(schema: &mut Value, executable_args: &BTreeSet<String>) {
    let Some(properties) = schema
        .as_object_mut()
        .and_then(|object| object.get_mut("properties"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for arg in executable_args {
        if let Some(property) = properties.get_mut(arg) {
            cap_integer_property(property, "maxLength", MAX_MODEL_EXECUTABLE_ARG_CHARS as u64);
        }
    }
    if let Some(property) = properties.get_mut("timeout") {
        cap_integer_property(property, "maximum", MAX_MODEL_FOREGROUND_TIMEOUT_MS);
    }
}

fn cap_integer_property(property: &mut Value, key: &str, cap: u64) {
    let Some(object) = property.as_object_mut() else {
        return;
    };
    let value = object
        .get(key)
        .and_then(Value::as_u64)
        .map(|current| current.min(cap))
        .unwrap_or(cap);
    object.insert(key.to_string(), json!(value));
}

fn executable_arg_names(action: &ActionContract) -> BTreeSet<String> {
    let mut args = BTreeSet::new();
    for intent in &action.semantic_intents {
        if semantic_symbol(&intent.intent) != "shell_command" {
            continue;
        }
        args.extend(intent.slots.values().cloned());
        args.extend(intent.optional_slots.values().cloned());
    }
    args
}

fn foreground_timeout_ms(args: &Value) -> Option<u64> {
    if run_in_background(args) {
        return None;
    }
    args.get("timeout").and_then(Value::as_u64)
}

fn run_in_background(args: &Value) -> bool {
    args.get("run_in_background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}
