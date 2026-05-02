use super::{GoalVerificationResult, ModelCandidateProposal, PufferToolCallProposal};
use crate::contract::{ActionContract, CapabilityContract, SideEffectClass};
use crate::model_policy::{
    model_policy_violations, validate_model_tool_args, ModelProposalViolation,
};
use crate::planner::CompletionRole;
use crate::semantics::read_only_side_effect;
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error as StdError,
    fmt,
};

struct ParsedCandidateArray {
    accepted: Vec<ModelCandidateProposal>,
    rejected: Vec<String>,
    violations: Vec<ModelProposalViolation>,
}

#[derive(Debug)]
pub(crate) struct ModelCandidateParseError {
    source: String,
    messages: Vec<String>,
    violations: Vec<ModelProposalViolation>,
}

impl ModelCandidateParseError {
    fn new(source: &str, messages: Vec<String>, violations: Vec<ModelProposalViolation>) -> Self {
        Self {
            source: source.to_string(),
            messages,
            violations,
        }
    }

    /// Returns structured policy violations from invalid candidate entries.
    pub(crate) fn violations(&self) -> &[ModelProposalViolation] {
        &self.violations
    }
}

impl fmt::Display for ModelCandidateParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {} entries: {}",
            self.source,
            self.messages.join("; ")
        )
    }
}

impl StdError for ModelCandidateParseError {}

/// Extracts structured model-proposal violations from a parser error chain.
pub(crate) fn model_proposal_violations(error: &anyhow::Error) -> Vec<ModelProposalViolation> {
    if let Some(error) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ModelCandidateParseError>())
    {
        return error.violations().to_vec();
    }
    model_policy_violations(error)
}

pub(crate) fn validate_puffer_tool_call(
    tool_id: &str,
    args: Value,
    contract: &CapabilityContract,
) -> Result<PufferToolCallProposal> {
    if tool_id.is_empty() {
        return Err(anyhow!("Puffer tool-call proposal missing `tool_id`"));
    }
    let action = action_for_tool_id(contract, tool_id)?;
    validate_tool_args_object(tool_id, &args)?;
    validate_tool_args_schema(action, &args)?;
    Ok(PufferToolCallProposal {
        tool_id: tool_id.to_string(),
        args,
    })
}

pub(super) fn validate_model_puffer_tool_call(
    tool_id: &str,
    args: Value,
    contract: &CapabilityContract,
    role: Option<CompletionRole>,
) -> Result<PufferToolCallProposal> {
    let action = action_for_tool_id(contract, tool_id)?;
    let proposal = validate_puffer_tool_call(tool_id, args, contract)?;
    validate_model_tool_args(action, &proposal.args, role)?;
    Ok(proposal)
}

pub(super) fn parse_tool_call_json(
    text: &str,
    contract: &CapabilityContract,
) -> Result<PufferToolCallProposal> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|error| {
        strict_json_parse_error("LLM tool-call proposal", error, text)
    })?;
    validate_tool_call_value(value, contract)
}

pub(super) fn parse_candidate_list_json(
    text: &str,
    contract: &CapabilityContract,
) -> Result<Vec<ModelCandidateProposal>> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|error| {
        strict_json_parse_error("LLM candidate proposal", error, text)
    })?;
    parse_candidate_list_value(value, contract)
}

pub(super) fn parse_candidate_list_value(
    value: Value,
    contract: &CapabilityContract,
) -> Result<Vec<ModelCandidateProposal>> {
    let Some(candidates) = candidate_array(&value) else {
        return parse_single_candidate_value(&value, contract).map(|candidate| vec![candidate]);
    };
    if candidates.is_empty() {
        return Err(anyhow!("LLM candidate proposal contained no candidates"));
    }
    let parsed = parse_model_candidate_array(candidates, contract, "candidate proposal");
    if !parsed.rejected.is_empty() || parsed.accepted.is_empty() {
        return Err(ModelCandidateParseError::new(
            "candidate proposal",
            parsed.rejected,
            parsed.violations,
        )
        .into());
    }
    Ok(parsed.accepted)
}

pub(super) fn parse_goal_verification_json(
    text: &str,
    contract: &CapabilityContract,
) -> Result<GoalVerificationResult> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|error| {
        strict_json_parse_error("LLM goal verification", error, text)
    })?;
    parse_goal_verification_value(value, contract)
}

pub(super) fn parse_goal_verification_value(
    value: Value,
    contract: &CapabilityContract,
) -> Result<GoalVerificationResult> {
    let satisfied = value
        .get("satisfied")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow!("LLM goal verification missing bool `satisfied`"))?;
    let confidence = value
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let missing_evidence = value
        .get("missing_evidence")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let suggested_candidate_values = value
        .get("suggested_candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let parsed_suggestions = parse_model_candidate_array_lossy(
        &suggested_candidate_values,
        contract,
        "goal verification suggested_candidates",
    );
    if (!satisfied || !missing_evidence.is_empty()) && !parsed_suggestions.rejected.is_empty() {
        return Err(ModelCandidateParseError::new(
            "goal verification suggested_candidates",
            parsed_suggestions.rejected,
            parsed_suggestions.violations,
        )
        .into());
    }
    if (!satisfied || !missing_evidence.is_empty()) && parsed_suggestions.accepted.is_empty() {
        let rejected = if parsed_suggestions.rejected.is_empty() {
            String::new()
        } else {
            format!(
                " after rejecting invalid entries: {}",
                parsed_suggestions.rejected.join("; ")
            )
        };
        return Err(anyhow!(
            "LLM goal verification marked the goal unsatisfied without structural follow-up candidates{rejected}"
        ));
    }
    Ok(GoalVerificationResult {
        satisfied,
        confidence,
        missing_evidence,
        suggested_candidates: parsed_suggestions.accepted,
        rejected_suggested_candidates: parsed_suggestions.rejected,
    })
}

fn parse_model_candidate_array(
    candidates: &[Value],
    contract: &CapabilityContract,
    source: &str,
) -> ParsedCandidateArray {
    parse_model_candidate_array_impl(candidates, contract, source, true)
}

fn parse_model_candidate_array_lossy(
    candidates: &[Value],
    contract: &CapabilityContract,
    source: &str,
) -> ParsedCandidateArray {
    parse_model_candidate_array_impl(candidates, contract, source, false)
}

fn parse_model_candidate_array_impl(
    candidates: &[Value],
    contract: &CapabilityContract,
    source: &str,
    strict_dependencies: bool,
) -> ParsedCandidateArray {
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    let mut violations = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        match parse_model_candidate(candidate, contract) {
            Ok(candidate) => accepted.push(candidate),
            Err(error) => {
                rejected.push(format!("{index}: {error}"));
                violations.extend(indexed_policy_violations(index, &error));
            }
        }
    }
    filter_candidate_dependencies(&mut accepted, &mut rejected, source, strict_dependencies);
    ParsedCandidateArray {
        accepted,
        rejected,
        violations,
    }
}

fn indexed_policy_violations(index: usize, error: &anyhow::Error) -> Vec<ModelProposalViolation> {
    model_policy_violations(error)
        .into_iter()
        .map(|mut violation| {
            violation.path = format!("candidates[{index}].{}", violation.path);
            violation
        })
        .collect()
}

fn strict_json_parse_error(source: &str, error: serde_json::Error, text: &str) -> anyhow::Error {
    anyhow!(
        "failed to parse {source} as strict JSON object: {error}; input_bytes={}",
        text.len()
    )
}

fn filter_candidate_dependencies(
    candidates: &mut Vec<ModelCandidateProposal>,
    rejected: &mut Vec<String>,
    source: &str,
    strict: bool,
) {
    let duplicate_ids = duplicate_candidate_ids(candidates);
    let dependency_refs = candidate_dependency_refs(candidates);
    let mut deduped = Vec::new();
    let mut invalid = false;
    for mut candidate in std::mem::take(candidates) {
        if let Some(id) = candidate.id.as_deref() {
            if duplicate_ids.contains(id) {
                if dependency_refs.contains(id) {
                    rejected.push(format!("{source}: duplicate candidate id `{id}`"));
                    invalid = true;
                    continue;
                }
                candidate.id = None;
            }
        }
        deduped.push(candidate);
    }
    *candidates = deduped;
    loop {
        let ids = candidates
            .iter()
            .filter_map(|candidate| candidate.id.clone())
            .collect::<BTreeSet<_>>();
        let before = candidates.len();
        candidates.retain(|candidate| {
            let invalid_dependency = candidate.depends_on.iter().find(|dependency| {
                candidate.id.as_deref() == Some(dependency.as_str())
                    || !ids.contains(dependency.as_str())
            });
            if let Some(dependency) = invalid_dependency {
                rejected.push(format!(
                    "{source}: candidate dependency `{dependency}` is unavailable"
                ));
                invalid = true;
                return false;
            }
            true
        });
        if candidates.len() == before {
            break;
        }
    }
    if has_candidate_dependency_cycle(candidates) {
        rejected.push(format!("{source}: candidate dependency cycle"));
        invalid = true;
        candidates.retain(|candidate| candidate.depends_on.is_empty());
    }
    if strict && invalid {
        candidates.clear();
    }
}

fn duplicate_candidate_ids(candidates: &[ModelCandidateProposal]) -> BTreeSet<String> {
    let mut counts = BTreeMap::new();
    for id in candidates
        .iter()
        .filter_map(|candidate| candidate.id.as_deref())
    {
        *counts.entry(id.to_string()).or_insert(0usize) += 1;
    }
    counts
        .into_iter()
        .filter_map(|(id, count)| (count > 1).then_some(id))
        .collect()
}

fn candidate_dependency_refs(candidates: &[ModelCandidateProposal]) -> BTreeSet<String> {
    candidates
        .iter()
        .flat_map(|candidate| candidate.depends_on.iter().cloned())
        .collect()
}

pub(super) fn parse_model_candidate(
    value: &Value,
    contract: &CapabilityContract,
) -> Result<ModelCandidateProposal> {
    parse_single_candidate_value(value, contract)
}

fn parse_single_candidate_value(
    value: &Value,
    contract: &CapabilityContract,
) -> Result<ModelCandidateProposal> {
    if let Some(wrapped) = pure_wrapped_candidate(value) {
        return parse_single_candidate_value(wrapped, contract);
    }
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("model candidate must be a JSON object"))?;
    let tool_id = tool_id_from_object(object, "model candidate")?;
    let args = tool_args_from_object(object, "model candidate")?;
    let role = string_from_object_or_payloads(object, "completion_role")
        .ok_or_else(|| anyhow!("model candidate missing `completion_role`"))
        .and_then(parse_completion_role)?;
    let action = action_for_tool_id(contract, &tool_id)?;
    let role = contract_compatible_completion_role(action, role);
    let id = optional_candidate_id(object)?;
    let depends_on = optional_candidate_dependencies(object)?;
    let rationale = string_from_object_or_payloads(object, "rationale")
        .unwrap_or("model proposed this contract-gated action")
        .to_string();
    let proposal = validate_model_puffer_tool_call(&tool_id, args, contract, Some(role.clone()))?;
    Ok(ModelCandidateProposal {
        id,
        tool_id: proposal.tool_id,
        args: proposal.args,
        completion_role: role,
        depends_on,
        rationale,
    })
}

fn contract_compatible_completion_role(
    action: &ActionContract,
    role: CompletionRole,
) -> CompletionRole {
    match role {
        CompletionRole::Support if action.side_effect_class == SideEffectClass::Unknown => {
            CompletionRole::Verification
        }
        CompletionRole::Support if !read_only_side_effect(&action.side_effect_class) => {
            CompletionRole::Repair
        }
        CompletionRole::Repair if read_only_side_effect(&action.side_effect_class) => {
            CompletionRole::Support
        }
        CompletionRole::Verification
            if matches!(
                action.side_effect_class,
                SideEffectClass::LocalWrite
                    | SideEffectClass::ExternalWrite
                    | SideEffectClass::DestructiveWrite
            ) =>
        {
            CompletionRole::Repair
        }
        CompletionRole::Verification
            if !read_only_side_effect(&action.side_effect_class)
                && action.side_effect_class != SideEffectClass::Unknown =>
        {
            CompletionRole::Repair
        }
        other => other,
    }
}

fn candidate_array(value: &Value) -> Option<&[Value]> {
    let object = value.as_object()?;
    [
        "candidates",
        "actions",
        "tool_calls",
        "proposals",
        "steps",
        "suggested_candidates",
    ]
    .into_iter()
    .find_map(|key| object.get(key).and_then(Value::as_array).map(Vec::as_slice))
}

fn wrapped_candidate(value: &Value) -> Option<&Value> {
    let object = value.as_object()?;
    [
        "candidate",
        "action",
        "tool_call",
        "proposal",
        "next_action",
        "next_candidate",
        "step",
    ]
    .into_iter()
    .find_map(|key| object.get(key).filter(|value| value.is_object()))
}

fn pure_wrapped_candidate(value: &Value) -> Option<&Value> {
    let object = value.as_object()?;
    let mut wrapped = None;
    for (key, item) in object {
        if !is_candidate_wrapper_key(key) {
            return None;
        }
        if item.is_object() {
            if wrapped.is_some() {
                return None;
            }
            wrapped = Some(item);
        }
    }
    wrapped
}

fn is_candidate_wrapper_key(key: &str) -> bool {
    matches!(
        key,
        "candidate"
            | "action"
            | "tool_call"
            | "proposal"
            | "next_action"
            | "next_candidate"
            | "step"
    )
}

fn optional_candidate_id(object: &serde_json::Map<String, Value>) -> Result<Option<String>> {
    object.get("id").map(parse_candidate_id).transpose()
}

fn optional_candidate_dependencies(object: &serde_json::Map<String, Value>) -> Result<Vec<String>> {
    object
        .get("depends_on")
        .map(parse_candidate_dependencies)
        .transpose()
        .map(Option::unwrap_or_default)
}

fn parse_candidate_dependencies(value: &Value) -> Result<Vec<String>> {
    let items = value
        .as_array()
        .ok_or_else(|| anyhow!("candidate `depends_on` must be an array of candidate ids"))?;
    items.iter().map(parse_candidate_id).collect()
}

fn parse_candidate_id(value: &Value) -> Result<String> {
    let id = value
        .as_str()
        .ok_or_else(|| anyhow!("candidate `id` must be a string"))?
        .trim();
    if id.is_empty() {
        return Err(anyhow!("candidate `id` must not be empty"));
    }
    Ok(id.to_string())
}

fn has_candidate_dependency_cycle(candidates: &[ModelCandidateProposal]) -> bool {
    let by_id = candidates
        .iter()
        .filter_map(|candidate| candidate.id.as_deref().map(|id| (id, candidate)))
        .collect::<BTreeMap<_, _>>();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    by_id
        .keys()
        .any(|id| candidate_dep_cycle_from(id, &by_id, &mut visiting, &mut visited))
}

fn candidate_dep_cycle_from<'a>(
    id: &'a str,
    by_id: &BTreeMap<&'a str, &'a ModelCandidateProposal>,
    visiting: &mut BTreeSet<&'a str>,
    visited: &mut BTreeSet<&'a str>,
) -> bool {
    if visited.contains(id) {
        return false;
    }
    if !visiting.insert(id) {
        return true;
    }
    let cyclic = by_id.get(id).is_some_and(|candidate| {
        candidate
            .depends_on
            .iter()
            .any(|dependency| candidate_dep_cycle_from(dependency, by_id, visiting, visited))
    });
    visiting.remove(id);
    visited.insert(id);
    cyclic
}

fn parse_completion_role(value: &str) -> Result<CompletionRole> {
    match value {
        "terminal" => Ok(CompletionRole::Terminal),
        "support" | "observation" => Ok(CompletionRole::Support),
        "verification" => Ok(CompletionRole::Verification),
        "repair" => Ok(CompletionRole::Repair),
        _ => Err(anyhow!("unknown model candidate completion_role `{value}`")),
    }
}

fn validate_tool_call_value(
    value: Value,
    contract: &CapabilityContract,
) -> Result<PufferToolCallProposal> {
    if let Some(wrapped) = pure_wrapped_candidate(&value) {
        return validate_tool_call_value(wrapped.clone(), contract);
    }
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("LLM tool-call proposal must be a JSON object"))?;
    for key in object.keys() {
        if !is_tool_call_payload_key(key) && !is_tool_call_metadata_key(key) {
            return Err(anyhow!(
                "LLM tool-call proposal contained unsupported field `{key}`"
            ));
        }
    }
    let tool_id = tool_id_from_object(object, "LLM tool-call proposal")?;
    let args = tool_args_from_object(object, "LLM tool-call proposal")?;
    validate_model_puffer_tool_call(&tool_id, args, contract, None)
}

fn is_tool_call_payload_key(key: &str) -> bool {
    matches!(
        key,
        "tool_id"
            | "tool"
            | "tool_name"
            | "action"
            | "action_name"
            | "function"
            | "function_name"
            | "name"
            | "parameters"
            | "args"
            | "arguments"
            | "input"
            | "object"
    )
}

fn is_tool_call_metadata_key(key: &str) -> bool {
    matches!(
        key,
        "id" | "type"
            | "candidate"
            | "proposal"
            | "next_action"
            | "next_candidate"
            | "step"
            | "completion_role"
            | "rationale"
            | "confidence"
            | "depends_on"
    )
}

fn tool_id_from_object(object: &serde_json::Map<String, Value>, source: &str) -> Result<String> {
    let mut present = Vec::new();
    collect_tool_ids(object, false, &mut present);
    present.sort();
    present.dedup();
    match present.as_slice() {
        [] => Err(anyhow!(
            "{source} missing `tool_id` (object keys: {})",
            object_keys(object)
        )),
        [value] => Ok(value.trim().to_string()),
        _ => Err(anyhow!(
            "{source} contains multiple tool identifiers: {}",
            present.join(", ")
        )),
    }
}

fn tool_args_from_object(object: &serde_json::Map<String, Value>, source: &str) -> Result<Value> {
    let mut present = Vec::new();
    collect_tool_args(object, &mut present)?;
    present.sort_by(|left, right| left.0.cmp(&right.0));
    present.dedup_by(|left, right| left.1 == right.1);
    match present.as_slice() {
        [] => Err(anyhow!(
            "{source} missing object `args` (object keys: {})",
            object_keys(object)
        )),
        [(_, value)] => Ok(value.clone()),
        _ => Err(anyhow!("{source} contains multiple argument objects")),
    }
}

fn collect_tool_ids(
    object: &serde_json::Map<String, Value>,
    include_name_alias: bool,
    present: &mut Vec<String>,
) {
    for key in [
        "tool_id",
        "tool",
        "tool_name",
        "action_name",
        "action",
        "function_name",
    ] {
        if let Some(value) = object.get(key).and_then(Value::as_str) {
            present.push(value.trim().to_string());
        }
    }
    if include_name_alias || object_is_structural_tool_payload(object) {
        if let Some(value) = object.get("name").and_then(Value::as_str) {
            present.push(value.trim().to_string());
        }
    }
    for (key, payload) in nested_tool_payloads(object) {
        collect_tool_ids(payload, nested_payload_supports_name(key), present);
    }
}

fn collect_tool_args(
    object: &serde_json::Map<String, Value>,
    present: &mut Vec<(String, Value)>,
) -> Result<()> {
    for key in ["args", "arguments", "input", "object", "parameters"] {
        if let Some(value) = object.get(key) {
            present.push((key.to_string(), parse_tool_args_value(key, value)?));
        }
    }
    for (_, payload) in nested_tool_payloads(object) {
        collect_tool_args(payload, present)?;
    }
    Ok(())
}

fn parse_tool_args_value(key: &str, value: &Value) -> Result<Value> {
    if value.is_object() {
        return Ok(value.clone());
    }
    if key == "arguments" {
        if let Some(text) = value.as_str() {
            let parsed: Value = serde_json::from_str(text)
                .with_context(|| "tool-call `arguments` string must contain a JSON object")?;
            if parsed.is_object() {
                return Ok(parsed);
            }
        }
    }
    Err(anyhow!("tool-call `{key}` must be a JSON object"))
}

fn nested_tool_payloads(
    object: &serde_json::Map<String, Value>,
) -> Vec<(&str, &serde_json::Map<String, Value>)> {
    [
        "candidate",
        "action",
        "tool_call",
        "proposal",
        "next_action",
        "next_candidate",
        "step",
        "function",
        "tool",
    ]
    .into_iter()
    .filter_map(|key| {
        object
            .get(key)
            .and_then(Value::as_object)
            .map(|payload| (key, payload))
    })
    .collect()
}

fn nested_payload_supports_name(key: &str) -> bool {
    matches!(key, "function" | "tool" | "tool_call" | "action")
}

fn object_is_structural_tool_payload(object: &serde_json::Map<String, Value>) -> bool {
    object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| matches!(value, "tool_use" | "function"))
        || ["args", "arguments", "input", "object", "parameters"]
            .into_iter()
            .any(|key| object.contains_key(key))
}

fn string_from_object_or_payloads<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str).or_else(|| {
        nested_tool_payloads(object)
            .into_iter()
            .find_map(|(_, payload)| string_from_object_or_payloads(payload, key))
    })
}

fn object_keys(object: &serde_json::Map<String, Value>) -> String {
    let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
    keys.sort_unstable();
    keys.join(", ")
}

fn action_for_tool_id<'a>(
    contract: &'a CapabilityContract,
    tool_id: &str,
) -> Result<&'a ActionContract> {
    contract
        .actions
        .iter()
        .find(|action| action.name == tool_id)
        .ok_or_else(|| {
            anyhow!("Puffer tool-call proposal selected unavailable Puffer tool `{tool_id}`")
        })
}

fn validate_tool_args_object(tool_id: &str, args: &Value) -> Result<()> {
    if args.is_object() {
        Ok(())
    } else {
        Err(anyhow!(
            "Puffer tool-call proposal args for `{tool_id}` must be a JSON object"
        ))
    }
}

fn validate_tool_args_schema(action: &ActionContract, args: &Value) -> Result<()> {
    let validator = jsonschema::validator_for(&action.input_schema)
        .with_context(|| format!("invalid input_schema for Puffer tool {}", action.name))?;
    let errors = validator
        .iter_errors(args)
        .take(5)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "Puffer tool-call proposal args for `{}` do not match input_schema: {}",
            action.name,
            errors.join(", ")
        ))
    }
}
