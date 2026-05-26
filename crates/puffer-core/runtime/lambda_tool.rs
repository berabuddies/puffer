use super::claude_tools::bash::ClaudeBashInput;
use super::lambda_gate::{admitted_host_call_metadata, LambdaGateVerdict, PendingLambdaHostCall};
use super::tool_executor::blocked_runtime_tool;
use super::RequestToolFilter;
use crate::permissions::ToolPermissionBehavior;
use crate::AppState;
use puffer_tools::{ToolDefinition, ToolExecutionResult, ToolOutput, ToolRegistry};
use serde::Deserialize;
use serde_json::{json, Map, Number, Value};
use std::path::Path;

pub(super) const LAMBDA_HOST_CALL_TOOL_ID: &str = "LambdaHostCall";
const SKILL_TOOL_ID: &str = "Skill";

/// Rejects concrete tool calls that do not match the active Lambda Skill gate.
pub(super) fn reject_lambda_skill_gate_preflight(
    state: &AppState,
    registry: &ToolRegistry,
    cwd: &Path,
    tool_id: &str,
    input: &Value,
) -> Option<ToolExecutionResult> {
    if let Some(pending) = state.pending_lambda_host_call.as_ref() {
        let canonical_input = registry
            .definition(tool_id)
            .map(|definition| lambda_pending_input_for_tool(definition, input))
            .unwrap_or_else(|| input.clone());
        if pending.permits_concrete_call(tool_id, &canonical_input) {
            return None;
        }
        return Some(lambda_skill_pending_bridge_denial(
            tool_id,
            pending.concrete_tool(),
            format!(
                "pending formal host call {} requires next concrete tool {} with the declared input",
                pending.host_tool(),
                pending.concrete_tool()
            ),
        ));
    }
    if tool_id == SKILL_TOOL_ID {
        return None;
    }
    if let Some(gate) = state.lambda_gate.as_ref() {
        let Some(filter) = gate.request_tool_filter() else {
            return Some(lambda_skill_bridge_required_denial(
                tool_id,
                "active Lambda Skill requires LambdaHostCall before concrete tool calls"
                    .to_string(),
            ));
        };
        let Some(definition) = registry.definition(tool_id) else {
            return None;
        };
        match filter.allows_call(definition, cwd, input) {
            Ok(true) => {
                return Some(lambda_skill_bridge_required_denial(
                    tool_id,
                    "active Lambda Skill requires LambdaHostCall before concrete tool calls"
                        .to_string(),
                ));
            }
            Ok(false) => return None,
            Err(error) => {
                return Some(lambda_skill_bridge_required_denial(
                    tool_id,
                    format!("active Lambda Skill tool-scope check failed: {error}"),
                ));
            }
        }
    }
    None
}

/// Returns true when an active Lambda gate should scope this concrete call.
pub(super) fn lambda_skill_gate_scopes_tool_call(
    state: &AppState,
    registry: &ToolRegistry,
    cwd: &Path,
    tool_id: &str,
    input: &Value,
) -> bool {
    if state.pending_lambda_host_call.is_some() || tool_id == SKILL_TOOL_ID {
        return true;
    }
    let Some(gate) = state.lambda_gate.as_ref() else {
        return false;
    };
    let Some(filter) = gate.request_tool_filter() else {
        return true;
    };
    let Some(definition) = registry.definition(tool_id) else {
        return false;
    };
    filter.allows_call(definition, cwd, input).unwrap_or(true)
}

/// Returns true when an exact verified concrete call should skip user approval.
pub(super) fn lambda_skill_skips_concrete_approval(
    state: &AppState,
    registry: &ToolRegistry,
    tool_id: &str,
    input: &Value,
) -> bool {
    let Some(pending) = state.pending_lambda_host_call.as_ref() else {
        return false;
    };
    if pending.requires_approval() {
        return false;
    }
    let canonical_input = registry
        .definition(tool_id)
        .map(|definition| lambda_pending_input_for_tool(definition, input))
        .unwrap_or_else(|| input.clone());
    pending.permits_concrete_call(tool_id, &canonical_input)
}

/// Commits the Lambda Skill gate transition after the concrete tool succeeds.
pub(super) fn commit_successful_lambda_skill_gate_call(
    state: &mut AppState,
    tool_id: &str,
    output: &ToolOutput,
) -> std::result::Result<Option<Value>, ToolExecutionResult> {
    if let Some(pending) = state.pending_lambda_host_call.as_ref().cloned() {
        let Some(gate) = state.lambda_gate.as_mut() else {
            return Err(lambda_skill_bridge_required_denial(
                tool_id,
                "pending formal host call has no active Lambda Skill gate".to_string(),
            ));
        };
        let result = lambda_result_value(output);
        let metadata = gate.committed_host_call_metadata(
            pending.host_tool(),
            Some(pending.host_args()),
            Some(pending.metadata_host_args()),
            Some(pending.concrete_tool()),
            Some(&result),
        );
        return match gate.step_call_with_args_and_result(
            pending.host_tool(),
            pending.host_args(),
            &result,
        ) {
            LambdaGateVerdict::Accept => {
                state.pending_lambda_host_call = None;
                Ok(Some(metadata))
            }
            LambdaGateVerdict::Reject(reason) => Err(lambda_skill_gate_denial(tool_id, reason)),
        };
    }
    Ok(None)
}

fn lambda_result_value(output: &ToolOutput) -> Value {
    let stdout = output.stdout.trim_end_matches(['\r', '\n']).to_string();
    let parsed = serde_json::from_str(&stdout).unwrap_or_else(|_| Value::String(stdout.clone()));
    if let Some(inner_stdout) = parsed
        .as_object()
        .and_then(|object| object.get("stdout"))
        .and_then(Value::as_str)
    {
        let inner_stdout = inner_stdout.trim_end_matches(['\r', '\n']).to_string();
        return serde_json::from_str(&inner_stdout).unwrap_or(Value::String(inner_stdout));
    }
    parsed
}

/// Prepares a verified formal host-call bridge for the next concrete tool call.
pub(super) fn prepare_lambda_host_call(
    state: &mut AppState,
    registry: &ToolRegistry,
    cwd: &Path,
    tool_filter: Option<&RequestToolFilter>,
    tool_id: &str,
    input: Value,
) -> ToolExecutionResult {
    let parsed = match serde_json::from_value::<LambdaHostCallInput>(input) {
        Ok(parsed) => parsed,
        Err(error) => {
            return blocked_runtime_tool(
                tool_id,
                ToolPermissionBehavior::Deny,
                Some(format!("invalid LambdaHostCall input: {error}")),
            );
        }
    };
    if parsed.host_tool.trim().is_empty() {
        return blocked_runtime_tool(
            tool_id,
            ToolPermissionBehavior::Deny,
            Some("LambdaHostCall requires a non-empty host_tool".to_string()),
        );
    }
    if parsed.tool.trim().is_empty() {
        return blocked_runtime_tool(
            tool_id,
            ToolPermissionBehavior::Deny,
            Some("LambdaHostCall requires a non-empty concrete tool".to_string()),
        );
    }
    let Some(gate) = state.lambda_gate.as_ref() else {
        return blocked_runtime_tool(
            tool_id,
            ToolPermissionBehavior::Deny,
            Some("LambdaHostCall requires an active Lambda Skill gate".to_string()),
        );
    };
    if let Some(pending) = state.pending_lambda_host_call.as_ref() {
        return blocked_runtime_tool(
            tool_id,
            ToolPermissionBehavior::Deny,
            Some(format!(
                "pending formal host call {} must be completed before admitting another host call",
                pending.host_tool()
            )),
        );
    }
    if parsed.tool == LAMBDA_HOST_CALL_TOOL_ID {
        return blocked_runtime_tool(
            tool_id,
            ToolPermissionBehavior::Deny,
            Some("LambdaHostCall cannot target itself".to_string()),
        );
    }
    let Some(definition) = registry.definition(&parsed.tool) else {
        return blocked_runtime_tool(
            tool_id,
            ToolPermissionBehavior::Deny,
            Some(format!(
                "LambdaHostCall target tool {} is not available",
                parsed.tool
            )),
        );
    };
    if let Some(filter) = tool_filter {
        if !filter.allows_definition(definition) {
            return blocked_runtime_tool(
                tool_id,
                ToolPermissionBehavior::Deny,
                Some(format!(
                    "LambdaHostCall target tool {} is outside the active skill tool scope",
                    parsed.tool
                )),
            );
        }
    }
    match gate.admit_call_with_args(&parsed.host_tool, &parsed.args) {
        LambdaGateVerdict::Accept => {
            if let LambdaGateVerdict::Reject(reason) =
                gate.admit_concrete_tool_binding(&parsed.host_tool, &parsed.tool)
            {
                return lambda_skill_gate_denial(tool_id, reason);
            }
            let materialized_input = match gate.materialize_concrete_input_binding(
                &parsed.host_tool,
                &parsed.args,
                &parsed.tool,
            ) {
                Ok(input) => input,
                Err(reason) => return lambda_skill_gate_denial(tool_id, reason),
            };
            let concrete_input = parsed.input.unwrap_or_else(|| materialized_input.clone());
            let contract_input =
                lambda_contract_input_for_tool(definition, &concrete_input, &materialized_input);
            if let LambdaGateVerdict::Reject(reason) = gate.admit_concrete_input_binding(
                &parsed.host_tool,
                &parsed.args,
                &parsed.tool,
                &contract_input,
            ) {
                return lambda_skill_gate_denial(tool_id, reason);
            }
            let pending_concrete_input = lambda_pending_input_for_tool(definition, &contract_input);
            if let Some(filter) = tool_filter {
                match filter.allows_call(definition, cwd, &pending_concrete_input) {
                    Ok(true) => {}
                    Ok(false) => {
                        return blocked_runtime_tool(
                            tool_id,
                            ToolPermissionBehavior::Deny,
                            Some(format!(
                                "LambdaHostCall target tool {} is outside the active skill tool scope",
                                parsed.tool
                            )),
                        );
                    }
                    Err(error) => {
                        return blocked_runtime_tool(
                            tool_id,
                            ToolPermissionBehavior::Deny,
                            Some(format!(
                                "LambdaHostCall target tool {} failed skill tool-scope check: {error}",
                                parsed.tool
                            )),
                        );
                    }
                }
            }
            let host_tool = parsed.host_tool.clone();
            let host_args = parsed.args.clone();
            let concrete_tool = parsed.tool.clone();
            let metadata_host_args = redacted_lambda_metadata_value(&host_args, definition);
            let metadata_concrete_input =
                redacted_lambda_metadata_value(&pending_concrete_input, definition);
            let metadata = admitted_host_call_metadata(
                &host_tool,
                metadata_host_args.clone(),
                &concrete_tool,
                metadata_concrete_input.clone(),
            );
            state.pending_lambda_host_call = Some(PendingLambdaHostCall::new(
                parsed.host_tool,
                parsed.args,
                metadata_host_args,
                parsed.tool,
                pending_concrete_input.clone(),
                gate.require_concrete_tool_approval(),
            ));
            let concrete_input_text = serde_json::to_string(&pending_concrete_input)
                .unwrap_or_else(|_| "null".to_string());
            successful_runtime_tool_with_metadata(
                tool_id,
                format!(
                    "Lambda host call admitted: {host_tool}. Next call must be {concrete_tool} with this exact input: {concrete_input_text}"
                ),
                metadata,
            )
        }
        LambdaGateVerdict::Reject(reason) => lambda_skill_gate_denial(tool_id, reason),
    }
}

fn lambda_contract_input_for_tool(
    definition: &ToolDefinition,
    input: &Value,
    expected: &Value,
) -> Value {
    if is_bash_tool(definition) {
        return canonical_bash_contract_input(input, expected).unwrap_or_else(|| input.clone());
    }
    input.clone()
}

fn lambda_pending_input_for_tool(definition: &ToolDefinition, input: &Value) -> Value {
    if is_bash_tool(definition) {
        return canonical_bash_pending_input(input).unwrap_or_else(|| input.clone());
    }
    input.clone()
}

fn is_bash_tool(definition: &ToolDefinition) -> bool {
    definition.id.eq_ignore_ascii_case("bash")
        || definition.handler == "bash"
        || definition.handler == "runtime:claude_bash"
}

fn canonical_bash_contract_input(input: &Value, expected: &Value) -> Option<Value> {
    let parsed = serde_json::from_value::<ClaudeBashInput>(input.clone()).ok()?;
    let expected = expected.as_object()?;
    let mut object = Map::new();
    object.insert("command".to_string(), Value::String(parsed.command));
    if expected.contains_key("timeout") {
        let timeout = parsed.timeout?;
        object.insert("timeout".to_string(), Value::Number(Number::from(timeout)));
    }
    if expected.contains_key("run_in_background") {
        object.insert(
            "run_in_background".to_string(),
            Value::Bool(parsed.run_in_background),
        );
    }
    if expected.contains_key("tty") {
        object.insert("tty".to_string(), Value::Bool(parsed.tty));
    }
    Some(Value::Object(object))
}

fn canonical_bash_pending_input(input: &Value) -> Option<Value> {
    let parsed = serde_json::from_value::<ClaudeBashInput>(input.clone()).ok()?;
    let mut object = Map::new();
    object.insert("command".to_string(), Value::String(parsed.command));
    if let Some(timeout) = parsed.timeout {
        object.insert("timeout".to_string(), Value::Number(Number::from(timeout)));
    }
    object.insert(
        "run_in_background".to_string(),
        Value::Bool(parsed.run_in_background),
    );
    object.insert("tty".to_string(), Value::Bool(parsed.tty));
    Some(Value::Object(object))
}

fn redacted_lambda_metadata_value(value: &Value, definition: &ToolDefinition) -> Value {
    let mut redacted = value.clone();
    for path in secret_input_paths(definition) {
        redact_json_path(&mut redacted, &path);
    }
    redacted
}

fn secret_input_paths(definition: &ToolDefinition) -> Vec<String> {
    let schema = definition.input_schema.as_json_schema();
    let mut paths = schema
        .get("x-puffer-secret-paths")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            let is_secret = property
                .get("x-puffer-secret")
                .or_else(|| property.get("x-secret"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if is_secret {
                paths.push(name.clone());
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

fn redact_json_path(value: &mut Value, path: &str) {
    let parts = path
        .split('.')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return;
    }
    redact_json_path_parts(value, &parts);
}

fn redact_json_path_parts(value: &mut Value, parts: &[&str]) {
    let Some((head, tail)) = parts.split_first() else {
        return;
    };
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let Some(child) = object.get_mut(*head) else {
        return;
    };
    if tail.is_empty() {
        *child = Value::String("[redacted]".to_string());
    } else {
        redact_json_path_parts(child, tail);
    }
}

#[derive(Debug, Deserialize)]
struct LambdaHostCallInput {
    host_tool: String,
    args: Value,
    tool: String,
    #[serde(default)]
    input: Option<Value>,
}

fn lambda_skill_gate_denial(tool_id: &str, reason: String) -> ToolExecutionResult {
    lambda_skill_recoverable_denial(
        tool_id,
        reason,
        LAMBDA_HOST_CALL_TOOL_ID,
        "Retry by calling LambdaHostCall with the formal host_tool, formal args, and target concrete tool. You may omit input; Puffer will materialize the exact concrete input from the verified contract. After LambdaHostCall is admitted, call the declared concrete tool once with the exact input returned by LambdaHostCall. Puffer will then run normal user approval for that concrete tool if approval is required.",
    )
}

fn lambda_skill_bridge_required_denial(tool_id: &str, reason: String) -> ToolExecutionResult {
    lambda_skill_recoverable_denial(
        tool_id,
        reason,
        LAMBDA_HOST_CALL_TOOL_ID,
        "Retry by calling LambdaHostCall before this concrete tool call. Include the formal host_tool, formal args, and this concrete tool name; omit input so Puffer materializes the exact concrete input from the verified contract. Puffer will ask the user to approve the concrete tool later if approval is required.",
    )
}

fn lambda_skill_pending_bridge_denial(
    tool_id: &str,
    next_tool: &str,
    reason: String,
) -> ToolExecutionResult {
    lambda_skill_recoverable_denial(
        tool_id,
        reason,
        next_tool,
        "A LambdaHostCall bridge is already pending. Retry by calling the pending concrete tool with the exact input declared by that LambdaHostCall; do not call LambdaHostCall again until the pending bridge completes.",
    )
}

fn lambda_skill_recoverable_denial(
    tool_id: &str,
    reason: String,
    retry_tool: &str,
    retry_advice: &str,
) -> ToolExecutionResult {
    ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: false,
        output: ToolOutput {
            stdout: format!(
                "Lambda Skill gate rejected call: {reason}\nRecoverable: {retry_advice}"
            ),
            stderr: String::new(),
            metadata: json!({
                "lambda_skill": {
                    "event": "gate_rejected",
                    "recoverable": true,
                    "rejected_tool": tool_id,
                    "retry_tool": retry_tool,
                    "reason": reason,
                    "approval_path": "normal permission approval runs after LambdaHostCall admits the concrete tool"
                }
            }),
        },
    }
}

fn successful_runtime_tool_with_metadata(
    tool_id: &str,
    stdout: String,
    metadata: Value,
) -> ToolExecutionResult {
    ToolExecutionResult {
        tool_id: tool_id.to_string(),
        success: true,
        output: ToolOutput {
            stdout,
            stderr: String::new(),
            metadata,
        },
    }
}
