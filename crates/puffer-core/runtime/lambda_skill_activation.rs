use super::lambda_gate::{gate_for_verified_skill, LambdaGateState};
use super::{build_request_tool_filter, RequestToolFilter};
use anyhow::{anyhow, bail, Result};
use puffer_resources::SkillSpec;

const LAMBDA_HOST_CALL_TOOL_ID: &str = "LambdaHostCall";
const LAMBDA_INTERNAL_TOOL_ID: &str = "LambdaInternal";

/// Returns true when a loaded skill carries Lambda Skill verification metadata.
pub(crate) fn is_lambda_verified_skill(skill: &SkillSpec) -> bool {
    skill
        .verification
        .as_ref()
        .is_some_and(|verification| verification.system == "lambda-skill")
}

/// Builds the runtime gate installed when a skill becomes active.
pub(crate) fn gate_for_verified_skill_activation(
    skill: &SkillSpec,
) -> Result<Option<LambdaGateState>> {
    let mut gate = gate_for_verified_skill(skill)?;
    if !is_lambda_verified_skill(skill) {
        return Ok(gate);
    }
    let Some(active_gate) = gate.as_mut() else {
        bail!(
            "verified Lambda Skill requires a precompiled host catalogue; set host_catalogue_subpath in the lambda_skill_libraries manifest"
        );
    };
    active_gate.set_request_tool_filter(request_tool_filter_for_verified_skill(skill)?);
    if let Some(verification) = skill.verification.as_ref() {
        active_gate.set_require_concrete_tool_approval(verification.require_approval);
    }
    Ok(gate)
}

/// Returns the concrete tool selectors used while a verified Lambda Skill runs.
pub(crate) fn allowed_tools_for_verified_skill(skill: &SkillSpec) -> Result<Vec<String>> {
    let mut allowed_tools = skill.allowed_tools.clone();
    if !is_lambda_verified_skill(skill) {
        return Ok(allowed_tools);
    }
    if allowed_tools.is_empty() {
        bail!(
            "verified Lambda Skill requires non-empty allowed_tools in the lambda_skill_libraries manifest"
        );
    }
    if !allowed_tools
        .iter()
        .any(|tool| tool.eq_ignore_ascii_case(LAMBDA_HOST_CALL_TOOL_ID))
    {
        allowed_tools.push(LAMBDA_HOST_CALL_TOOL_ID.to_string());
    }
    if !allowed_tools
        .iter()
        .any(|tool| tool.eq_ignore_ascii_case(LAMBDA_INTERNAL_TOOL_ID))
    {
        allowed_tools.push(LAMBDA_INTERNAL_TOOL_ID.to_string());
    }
    Ok(allowed_tools)
}

fn request_tool_filter_for_verified_skill(skill: &SkillSpec) -> Result<RequestToolFilter> {
    let allowed_tools = allowed_tools_for_verified_skill(skill)?;
    build_request_tool_filter(&allowed_tools)?
        .ok_or_else(|| anyhow!("verified Lambda Skill requires a non-empty concrete tool scope"))
}
