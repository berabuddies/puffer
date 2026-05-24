use super::lambda_gate::{admitted_host_call_metadata, LambdaGateVerdict, PendingLambdaHostCall};
use super::tool_executor::blocked_runtime_tool;
use super::RequestToolFilter;
use crate::permissions::ToolPermissionBehavior;
use crate::AppState;
use puffer_tools::{ToolExecutionResult, ToolOutput, ToolRegistry};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

pub(super) const LAMBDA_HOST_CALL_TOOL_ID: &str = "LambdaHostCall";
const SKILL_TOOL_ID: &str = "Skill";

/// Rejects concrete tool calls that do not match the active Lambda Skill gate.
pub(super) fn reject_lambda_skill_gate_preflight(
    state: &AppState,
    tool_id: &str,
    input: &Value,
) -> Option<ToolExecutionResult> {
    if tool_id == SKILL_TOOL_ID {
        return None;
    }
    if let Some(pending) = state.pending_lambda_host_call.as_ref() {
        if pending.permits_concrete_call(tool_id, input) {
            return None;
        }
        return Some(lambda_skill_gate_denial(
            tool_id,
            format!(
                "pending formal host call {} requires next concrete tool {} with the declared input",
                pending.host_tool(),
                pending.concrete_tool()
            ),
        ));
    }
    if state.lambda_gate.is_some() {
        return Some(lambda_skill_gate_denial(
            tool_id,
            "active Lambda Skill requires LambdaHostCall before concrete tool calls".to_string(),
        ));
    }
    None
}

/// Commits the Lambda Skill gate transition after the concrete tool succeeds permission checks.
pub(super) fn commit_lambda_skill_gate_call(
    state: &mut AppState,
    tool_id: &str,
) -> std::result::Result<Option<Value>, ToolExecutionResult> {
    if state.pending_lambda_host_call.is_some() {
        let Some(pending) = state.pending_lambda_host_call.take() else {
            return Ok(None);
        };
        let Some(gate) = state.lambda_gate.as_mut() else {
            return Err(lambda_skill_gate_denial(
                tool_id,
                "pending formal host call has no active Lambda Skill gate".to_string(),
            ));
        };
        let metadata = gate.committed_host_call_metadata(
            pending.host_tool(),
            Some(pending.host_args()),
            Some(pending.concrete_tool()),
        );
        return match gate.step_call(pending.host_tool()) {
            LambdaGateVerdict::Accept => Ok(Some(metadata)),
            LambdaGateVerdict::Reject(reason) => Err(lambda_skill_gate_denial(tool_id, reason)),
        };
    }
    if state.lambda_gate.is_none() {
        return Ok(None);
    }
    if tool_id == SKILL_TOOL_ID {
        return Ok(None);
    }
    Err(lambda_skill_gate_denial(
        tool_id,
        "active Lambda Skill requires LambdaHostCall before concrete tool calls".to_string(),
    ))
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
        match filter.allows_call(definition, cwd, &parsed.input) {
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
    match gate.admit_call_with_args(&parsed.host_tool, &parsed.args) {
        LambdaGateVerdict::Accept => {
            let host_tool = parsed.host_tool.clone();
            let host_args = parsed.args.clone();
            let concrete_tool = parsed.tool.clone();
            let metadata = admitted_host_call_metadata(
                &host_tool,
                host_args.clone(),
                &concrete_tool,
                parsed.input.clone(),
            );
            state.pending_lambda_host_call = Some(PendingLambdaHostCall::new(
                parsed.host_tool,
                parsed.args,
                parsed.tool,
                parsed.input,
            ));
            successful_runtime_tool_with_metadata(
                tool_id,
                format!(
                    "Lambda host call admitted: {host_tool}. Next call must be {concrete_tool} with the declared input."
                ),
                metadata,
            )
        }
        LambdaGateVerdict::Reject(reason) => lambda_skill_gate_denial(tool_id, reason),
    }
}

#[derive(Debug, Deserialize)]
struct LambdaHostCallInput {
    host_tool: String,
    args: Value,
    tool: String,
    input: Value,
}

fn lambda_skill_gate_denial(tool_id: &str, reason: String) -> ToolExecutionResult {
    blocked_runtime_tool(
        tool_id,
        ToolPermissionBehavior::Deny,
        Some(format!("Lambda Skill gate rejected call: {reason}")),
    )
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
