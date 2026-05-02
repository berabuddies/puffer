use super::parse::{
    parse_candidate_list_value, parse_goal_verification_value, parse_tool_call_json,
    validate_model_puffer_tool_call,
};
use super::schema::{apply_chat_compatibility_overrides, CHAT_JSON_MAX_TOKENS};
use super::{
    extract_chat_completions_payload_text, send_and_read_success, GoalVerificationResult,
    ModelCandidateProposal, PufferToolCallProposal,
};
use crate::contract::{CapabilityContract, SideEffectClass};
use crate::planner::CompletionRole;
use anyhow::{anyhow, Context, Result};
use puffer_provider_openai::{
    build_json_post_request, extract_chat_completions_tool_calls, parse_chat_completions_response,
    BuiltOpenAIRequest, OpenAIChatCompletionTool, OpenAIChatCompletionToolFunction,
    OpenAIRequestConfig, OpenAIResponseToolCall,
};
use serde_json::{json, Value};

pub(super) fn build_chat_tool_call_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    contract: &CapabilityContract,
) -> Result<BuiltOpenAIRequest> {
    let tools = contract
        .actions
        .iter()
        .map(|action| OpenAIChatCompletionTool {
            kind: "function".to_string(),
            function: OpenAIChatCompletionToolFunction {
                name: action.name.clone(),
                description: action.description.clone(),
                parameters: crate::model_policy::model_proposal_args_schema(action),
                strict: false,
            },
        })
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "You are Burbot's Puffer-tool proposal operator. Return a tool call, not prose."
            },
            {
                "role": "user",
                "content": prompt
            }
        ],
        "tools": tools,
        "tool_choice": "required",
        "max_tokens": CHAT_JSON_MAX_TOKENS,
        "stream": false
    });
    apply_chat_compatibility_overrides(config, &mut body);
    build_json_post_request(config, "/v1/chat/completions", &body)
}

pub(super) fn send_and_parse_chat_tool_call(
    request: &BuiltOpenAIRequest,
    contract: &CapabilityContract,
) -> Result<PufferToolCallProposal> {
    let text = send_and_read_success(request)?;
    let parsed = parse_chat_completions_response(&text)?;
    let calls = extract_chat_completions_tool_calls(&parsed)?;
    if let Some(call) = calls.into_iter().next() {
        return validate_model_puffer_tool_call(&call.name, call.arguments, contract, None);
    }
    let text = extract_chat_completions_payload_text(&text)?;
    parse_tool_call_json(&text, contract)
}

pub(super) fn send_and_parse_chat_candidate_list(
    request: &BuiltOpenAIRequest,
    contract: &CapabilityContract,
    expected_tool_name: &str,
) -> Result<Vec<ModelCandidateProposal>> {
    let value = send_and_parse_chat_structured_payload(
        request,
        contract,
        expected_tool_name,
        DirectToolFallback {
            read_only: CompletionRole::Support,
            mutating: CompletionRole::Repair,
            payload: DirectToolPayload::CandidateList,
        },
    )?;
    parse_candidate_list_value(value, contract)
}

pub(super) fn send_and_parse_chat_goal_verification(
    request: &BuiltOpenAIRequest,
    contract: &CapabilityContract,
    expected_tool_name: &str,
) -> Result<GoalVerificationResult> {
    let value = send_and_parse_chat_structured_payload(
        request,
        contract,
        expected_tool_name,
        DirectToolFallback {
            read_only: CompletionRole::Verification,
            mutating: CompletionRole::Repair,
            payload: DirectToolPayload::GoalVerification,
        },
    )?;
    parse_goal_verification_value(value, contract)
}

#[derive(Debug, Clone)]
struct DirectToolFallback {
    read_only: CompletionRole,
    mutating: CompletionRole,
    payload: DirectToolPayload,
}

#[derive(Debug, Clone, Copy)]
enum DirectToolPayload {
    CandidateList,
    GoalVerification,
}

fn send_and_parse_chat_structured_payload(
    request: &BuiltOpenAIRequest,
    contract: &CapabilityContract,
    expected_tool_name: &str,
    direct_fallback: DirectToolFallback,
) -> Result<Value> {
    let text = send_and_read_success(request)?;
    let parsed = parse_chat_completions_response(&text)?;
    let calls = extract_chat_completions_tool_calls(&parsed)?;
    if let Some(call) = calls.iter().find(|call| call.name == expected_tool_name) {
        return Ok(call.arguments.clone());
    }
    if let Some(candidates) = direct_tool_call_candidates(calls, contract, &direct_fallback)? {
        return Ok(wrap_direct_tool_candidates(
            candidates,
            direct_fallback.payload,
        ));
    }
    let text = extract_chat_completions_payload_text(&text)?;
    serde_json::from_str(text.trim()).with_context(|| {
        format!("failed to parse OpenAI Chat Completions structural assistant JSON: {text}")
    })
}

fn direct_tool_call_candidates(
    calls: Vec<OpenAIResponseToolCall>,
    contract: &CapabilityContract,
    direct_fallback: &DirectToolFallback,
) -> Result<Option<Vec<ModelCandidateProposal>>> {
    if calls.is_empty() {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    let mut unexpected = Vec::new();
    for call in calls {
        let Some(action) = contract
            .actions
            .iter()
            .find(|action| action.name == call.name)
        else {
            unexpected.push(call.name);
            continue;
        };
        let role = direct_completion_role(&action.side_effect_class, direct_fallback);
        let proposal = validate_model_puffer_tool_call(
            &call.name,
            call.arguments.clone(),
            contract,
            Some(role.clone()),
        )?;
        candidates.push(ModelCandidateProposal {
            id: call.item_id.or(Some(call.call_id)),
            tool_id: proposal.tool_id,
            args: proposal.args,
            completion_role: role,
            depends_on: Vec::new(),
            rationale: "provider emitted a direct structural contract tool call".to_string(),
        });
    }
    if !candidates.is_empty() {
        normalize_direct_tool_dependencies(&mut candidates, &direct_fallback.read_only);
        return Ok(Some(candidates));
    }
    Err(anyhow!(
        "OpenAI Chat Completions returned unexpected tools instead of structured payload: {}",
        unexpected.join(", ")
    ))
}

fn normalize_direct_tool_dependencies(
    candidates: &mut [ModelCandidateProposal],
    read_only_role: &CompletionRole,
) {
    if candidates.len() <= 1
        || candidates
            .iter()
            .all(|candidate| &candidate.completion_role == read_only_role)
    {
        return;
    }
    for (index, candidate) in candidates.iter_mut().enumerate() {
        if candidate.id.as_deref().is_none_or(str::is_empty) {
            candidate.id = Some(format!("direct-tool-call-{index}"));
        }
    }
    for index in 1..candidates.len() {
        let Some(previous_id) = candidates[index - 1].id.clone() else {
            continue;
        };
        if !candidates[index].depends_on.contains(&previous_id) {
            candidates[index].depends_on.push(previous_id);
        }
    }
}

fn direct_completion_role(
    side_effect_class: &SideEffectClass,
    direct_fallback: &DirectToolFallback,
) -> CompletionRole {
    match side_effect_class {
        SideEffectClass::PureObservation
        | SideEffectClass::LocalRead
        | SideEffectClass::ExternalRead => direct_fallback.read_only.clone(),
        SideEffectClass::Unknown => CompletionRole::Verification,
        _ => direct_fallback.mutating.clone(),
    }
}

fn wrap_direct_tool_candidates(
    candidates: Vec<ModelCandidateProposal>,
    payload: DirectToolPayload,
) -> Value {
    match payload {
        DirectToolPayload::CandidateList => json!({ "candidates": candidates }),
        DirectToolPayload::GoalVerification => json!({
            "satisfied": false,
            "confidence": 0.0,
            "missing_evidence": ["provider emitted direct structural follow-up tool calls"],
            "suggested_candidates": candidates,
        }),
    }
}
