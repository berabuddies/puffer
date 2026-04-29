use super::{is_codex_backend, supports_responses_api};
use crate::contract::CapabilityContract;
use anyhow::{anyhow, Result};
use puffer_provider_openai::{
    build_json_post_request, build_responses_request, BuiltOpenAIRequest, OpenAIRequestConfig,
    OpenAIResponsesRequest, OpenAIResponsesTextConfig, OpenAIResponsesTextFormat,
};
use serde_json::{json, Value};

pub(super) fn build_tool_call_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    contract: &CapabilityContract,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_structured_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        tool_call_text_config(contract)?,
        "burbot-tool-call-proposal",
        "You are Burbot's Puffer-tool proposal operator. Return only the requested structured JSON object.",
    )
}

pub(super) fn build_tool_call_plain_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_plain_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        "burbot-tool-call-proposal",
        "You are Burbot's Puffer-tool proposal operator. Return only the requested structured JSON object.",
    )
}

pub(super) fn build_observe_act_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    contract: &CapabilityContract,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_structured_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        observe_act_text_config(contract)?,
        "burbot-observe-act-proposal",
        "You are Burbot's observe-act proposal operator. Return only validated structural JSON.",
    )
}

pub(super) fn build_observe_act_plain_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_plain_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        "burbot-observe-act-proposal",
        "You are Burbot's observe-act proposal operator. Return only validated structural JSON.",
    )
}

pub(super) fn build_goal_verification_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    contract: &CapabilityContract,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_structured_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        goal_verification_text_config(contract)?,
        "burbot-goal-verification",
        "You are Burbot's goal verifier. Return only validated structural JSON.",
    )
}

pub(super) fn build_goal_verification_plain_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_plain_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        "burbot-goal-verification",
        "You are Burbot's goal verifier. Return only validated structural JSON.",
    )
}

pub(super) fn observe_act_prompt(
    goal: &str,
    contract: &CapabilityContract,
    observation_context: &Value,
) -> Result<String> {
    Ok(format!(
         "Return exactly this JSON shape: {{\"candidates\":[{{\"tool_id\":\"<available tool id>\",\"args\":{{}},\"completion_role\":\"terminal|support|verification|repair\",\"rationale\":\"why this next action is safe and useful\"}}]}}.\n\
         The `candidates` array must be non-empty. Each candidate must choose one available tool, object args, completion_role, and rationale.\n\
         Propose only actions that materially advance the goal from the given structural context. \
         A structurally valid candidate is still invalid if it only prints, echoes, comments on, or restates future work instead of actually \
         inspecting evidence, creating or changing the required artifact, starting a required service, running a concrete check, or repairing a concrete failure. \
         Do not submit placeholder commands, TODO scaffolds, fake success markers, or commands whose only effect is describing what should be implemented. \
         Prefer cheap observations before risky changes, and include verification candidates when useful.\n\n\
         When the context reports missing evidence, a goal-verifier failure, or invalid prior candidates, propose concrete \
         valid tool calls that directly collect or repair that evidence. Do not repeat checks already shown successful \
         unless the context shows relevant state changed after those checks.\n\n\
         Available tools:\n{}\n\n\
         Goal:\n{goal}\n\n\
         Structural context JSON:\n{}",
        serde_json::to_string_pretty(&available_tool_summaries(contract))?,
        serde_json::to_string_pretty(observation_context)?
    ))
}

pub(super) fn goal_verification_prompt(
    goal: &str,
    contract: &CapabilityContract,
    verification_context: &Value,
) -> Result<String> {
    Ok(format!(
         "Return exactly this JSON shape: {{\"satisfied\":false,\"confidence\":0.0,\"missing_evidence\":[],\"suggested_candidates\":[{{\"tool_id\":\"<available tool id>\",\"args\":{{}},\"completion_role\":\"verification|support|repair\",\"rationale\":\"why this exact follow-up resolves missing evidence\"}}]}}. \
         Decide whether the goal is satisfied by the structural evidence. \
         If evidence is missing, set satisfied=false and include concrete follow-up candidates with tool_id, object args, completion_role, and rationale. \
         Suggested candidates must execute concrete inspection, repair, artifact creation, service startup, or verification work; \
         never suggest placeholder commands, TODO scaffolds, fake success markers, or commands that only describe future work. \
         Do not infer success from a tool exit code alone when the goal requires an artifact or semantic result. \
         For generated code, migrations, ports, reimplementations, protocol services, or other behavioral artifacts, \
         a narrow self-check is not enough: require independent project tests when available, or broad representative checks \
         that exercise stateful/repeated operations, failure paths, edge cases, and final artifacts required by the goal. \
         For interface, protocol, schema, API, CLI, file-format, or wire-format goals, exact declared names are semantic \
         requirements: service names, method names, message names, field names, route names, ports, file names, command names, \
         enum variants, and parameter names must match the goal or source contract. Tests that only use generated clients, \
         wrappers, or helpers from the current artifact do not prove exact contract compatibility unless they also compare \
         the generated definitions or external contract text against the requested names. \
         If the evidence only compares one current input, one happy path, or a single synthetic example, mark the goal unsatisfied \
         and propose stronger verification or repair candidates. If required artifact contents, diffs, test results, or contract \
         comparisons are absent from the evidence, propose concrete read/search/test candidates instead of returning no candidates.\n\n\
         Available tools:\n{}\n\n\
         Goal:\n{goal}\n\n\
         Evidence JSON:\n{}",
        serde_json::to_string_pretty(&available_tool_summaries(contract))?,
        serde_json::to_string_pretty(verification_context)?
    ))
}

fn build_structured_json_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
    text: OpenAIResponsesTextConfig,
    prompt_cache_key: &str,
    instructions: &str,
) -> Result<BuiltOpenAIRequest> {
    if is_codex_backend(&config.base_url) {
        let body = json!({
            "model": model,
            "instructions": instructions,
            "input": [{
                "type": "message",
                "role": "user",
                "content": multimodal_content(prompt, image_data_urls)
            }],
            "tools": [],
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "store": false,
            "stream": false,
            "include": [],
            "text": text,
            "prompt_cache_key": prompt_cache_key
        });
        build_json_post_request(config, "/responses", &body)
    } else if supports_responses_api(&config.base_url) && image_data_urls.is_empty() {
        build_responses_request(
            config,
            &OpenAIResponsesRequest {
                model: model.to_string(),
                input: prompt.to_string(),
                text: Some(text),
            },
        )
    } else if supports_responses_api(&config.base_url) {
        let body = json!({
            "model": model,
            "input": [{
                "role": "user",
                "content": multimodal_content(prompt, image_data_urls)
            }],
            "text": text,
        });
        build_json_post_request(config, "/v1/responses", &body)
    } else {
        build_chat_json_object_request(config, model, prompt, image_data_urls, Some(instructions))
    }
}

fn build_plain_json_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
    prompt_cache_key: &str,
    instructions: &str,
) -> Result<BuiltOpenAIRequest> {
    if is_codex_backend(&config.base_url) {
        let body = json!({
            "model": model,
            "instructions": instructions,
            "input": [{
                "type": "message",
                "role": "user",
                "content": multimodal_content(prompt, image_data_urls)
            }],
            "tools": [],
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "store": false,
            "stream": false,
            "include": [],
            "prompt_cache_key": prompt_cache_key
        });
        build_json_post_request(config, "/responses", &body)
    } else if supports_responses_api(&config.base_url) && image_data_urls.is_empty() {
        build_responses_request(
            config,
            &OpenAIResponsesRequest {
                model: model.to_string(),
                input: prompt.to_string(),
                text: None,
            },
        )
    } else if supports_responses_api(&config.base_url) {
        let body = json!({
            "model": model,
            "input": [{
                "role": "user",
                "content": multimodal_content(prompt, image_data_urls)
            }],
        });
        build_json_post_request(config, "/v1/responses", &body)
    } else {
        build_chat_json_object_request(config, model, prompt, image_data_urls, Some(instructions))
    }
}

fn multimodal_content(prompt: &str, image_data_urls: &[String]) -> Vec<Value> {
    let mut content = vec![json!({"type": "input_text", "text": prompt})];
    content.extend(image_data_urls.iter().map(|image_url| {
        json!({
            "type": "input_image",
            "image_url": image_url,
            "detail": "auto",
        })
    }));
    content
}

fn build_chat_json_object_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
    instructions: Option<&str>,
) -> Result<BuiltOpenAIRequest> {
    let mut messages = Vec::new();
    if let Some(instructions) = instructions {
        messages.push(json!({
            "role": "system",
            "content": instructions
        }));
    }
    messages.push(json!({
        "role": "user",
        "content": chat_content(prompt, image_data_urls)
    }));
    let body = json!({
        "model": model,
        "messages": messages,
        "response_format": {"type": "json_object"},
        "stream": false
    });
    build_json_post_request(config, "/v1/chat/completions", &body)
}

fn chat_content(prompt: &str, image_data_urls: &[String]) -> Value {
    if image_data_urls.is_empty() {
        return Value::String(prompt.to_string());
    }
    let mut content = vec![json!({"type": "text", "text": prompt})];
    content.extend(image_data_urls.iter().map(|image_url| {
        json!({
            "type": "image_url",
            "image_url": {"url": image_url}
        })
    }));
    Value::Array(content)
}

fn tool_call_text_config(contract: &CapabilityContract) -> Result<OpenAIResponsesTextConfig> {
    Ok(OpenAIResponsesTextConfig {
        format: OpenAIResponsesTextFormat {
            kind: "json_schema".to_string(),
            name: "burbot_puffer_tool_call".to_string(),
            description: Some(
                "One validated Puffer tool call proposal for the current Burbot goal.".to_string(),
            ),
            schema: tool_call_response_schema(contract)?,
            strict: true,
        },
    })
}

fn tool_call_response_schema(contract: &CapabilityContract) -> Result<Value> {
    let variants = contract
        .actions
        .iter()
        .map(tool_call_variant_schema)
        .collect::<Vec<_>>();
    if variants.is_empty() {
        return Err(anyhow!("Puffer tool contract has no available actions"));
    }
    Ok(json!({"oneOf": variants}))
}

fn observe_act_text_config(contract: &CapabilityContract) -> Result<OpenAIResponsesTextConfig> {
    Ok(OpenAIResponsesTextConfig {
        format: OpenAIResponsesTextFormat {
            kind: "json_schema".to_string(),
            name: "burbot_observe_act_candidates".to_string(),
            description: Some(
                "Zero or more validated Puffer tool-call candidates for the next Burbot step."
                    .to_string(),
            ),
            schema: candidate_list_response_schema(contract)?,
            strict: true,
        },
    })
}

fn goal_verification_text_config(
    contract: &CapabilityContract,
) -> Result<OpenAIResponsesTextConfig> {
    Ok(OpenAIResponsesTextConfig {
        format: OpenAIResponsesTextFormat {
            kind: "json_schema".to_string(),
            name: "burbot_goal_verification".to_string(),
            description: Some(
                "Goal satisfaction decision with optional validated follow-up candidates."
                    .to_string(),
            ),
            schema: goal_verification_response_schema(contract)?,
            strict: true,
        },
    })
}

fn candidate_list_response_schema(contract: &CapabilityContract) -> Result<Value> {
    Ok(json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "candidates": {
                "type": "array",
                "minItems": 1,
                "items": candidate_response_schema(contract)?
            }
        },
        "required": ["candidates"]
    }))
}

fn goal_verification_response_schema(contract: &CapabilityContract) -> Result<Value> {
    Ok(json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "satisfied": {"type": "boolean"},
            "confidence": {"type": "number", "minimum": 0.0, "maximum": 1.0},
            "missing_evidence": {
                "type": "array",
                "items": {"type": "string"}
            },
            "suggested_candidates": {
                "type": "array",
                "items": candidate_response_schema(contract)?
            }
        },
        "required": ["satisfied", "confidence", "missing_evidence", "suggested_candidates"]
    }))
}

fn candidate_response_schema(contract: &CapabilityContract) -> Result<Value> {
    let variants = contract
        .actions
        .iter()
        .map(candidate_variant_schema)
        .collect::<Vec<_>>();
    if variants.is_empty() {
        return Err(anyhow!("Puffer tool contract has no available actions"));
    }
    Ok(json!({"oneOf": variants}))
}

fn tool_call_variant_schema(action: &crate::contract::ActionContract) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "tool_id": {"type": "string", "enum": [action.name.clone()]},
            "args": action.input_schema.clone(),
        },
        "required": ["tool_id", "args"]
    })
}

fn candidate_variant_schema(action: &crate::contract::ActionContract) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "tool_id": {"type": "string", "enum": [action.name.clone()]},
            "args": action.input_schema.clone(),
            "completion_role": {
                "type": "string",
                "enum": ["terminal", "support", "verification", "repair"]
            },
            "rationale": {"type": "string"}
        },
        "required": ["tool_id", "args", "completion_role", "rationale"]
    })
}

fn available_tool_summaries(contract: &CapabilityContract) -> Vec<Value> {
    contract
        .actions
        .iter()
        .map(|action| {
            json!({
                "tool_id": action.name,
                "description": action.description,
                "input_schema": action.input_schema,
                "side_effect_class": action.side_effect_class,
                "risk_level": action.risk_level,
                "verification_required": action.verification.required_before_completion,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_provider_openai::{OpenAIAuth, OpenAIRequestConfig};

    fn codex_config() -> OpenAIRequestConfig {
        OpenAIRequestConfig {
            base_url: "https://chatgpt.com/backend-api/codex".to_string(),
            version: "0.125.0".to_string(),
            auth: OpenAIAuth::OAuthBearer("token".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
        }
    }

    fn contract() -> CapabilityContract {
        CapabilityContract {
            contract_id: "puffer.tools".to_string(),
            version: "0.1.0".to_string(),
            status: crate::contract::ContractStatus::Active,
            trust_level: crate::contract::TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![crate::contract::ActionContract {
                name: "Read".to_string(),
                description: "read".to_string(),
                input_schema: json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {"file_path": {"type": "string"}},
                    "required": ["file_path"],
                }),
                output_schema: json!({"type": "object"}),
                side_effect_class: crate::contract::SideEffectClass::LocalRead,
                reversibility: crate::contract::Reversibility::Reversible,
                idempotency: crate::contract::Idempotency::Idempotent,
                risk_level: crate::contract::RiskLevel::Low,
                preconditions: Vec::new(),
                postconditions: Vec::new(),
                verification: crate::contract::VerificationSpec {
                    methods: Vec::new(),
                    templates: Vec::new(),
                    required_before_completion: false,
                    confidence: 0.5,
                },
                approval: crate::contract::ApprovalSpec {
                    required: false,
                    reason: None,
                },
                failure_modes: Vec::new(),
                forbidden_uses: Vec::new(),
                argument_safety: Vec::new(),
                semantic_intents: Vec::new(),
                intent_extractors: Vec::new(),
                repair_rules: Vec::new(),
                cost_estimate: None,
                latency_estimate: None,
            }],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        }
    }

    #[test]
    fn goal_verification_request_attaches_images_as_content_items() {
        let image = "data:image/png;base64,aGVsbG8=".to_string();
        let request = build_goal_verification_request(
            &codex_config(),
            "gpt-5.5",
            "verify",
            &contract(),
            &[image.clone()],
        )
        .unwrap();
        let body: Value = serde_json::from_str(&request.body).unwrap();

        assert_eq!(body["text"]["format"]["type"], json!("json_schema"));
        assert_eq!(body["input"][0]["content"][1]["type"], json!("input_image"));
        assert_eq!(body["input"][0]["content"][1]["image_url"], json!(image));
    }

    #[test]
    fn candidate_schemas_do_not_cap_transition_counts() {
        let request =
            build_observe_act_request(&codex_config(), "gpt-5.5", "propose", &contract(), &[])
                .unwrap();
        let body: Value = serde_json::from_str(&request.body).unwrap();

        assert_eq!(
            body["text"]["format"]["schema"]["properties"]["candidates"]["maxItems"],
            Value::Null
        );

        let request =
            build_goal_verification_request(&codex_config(), "gpt-5.5", "verify", &contract(), &[])
                .unwrap();
        let body: Value = serde_json::from_str(&request.body).unwrap();

        assert_eq!(
            body["text"]["format"]["schema"]["properties"]["suggested_candidates"]["maxItems"],
            Value::Null
        );
    }

    #[test]
    fn goal_verification_prompt_requires_strong_behavioral_evidence() {
        let prompt = goal_verification_prompt(
            "Re-implement the service behavior in Python.",
            &contract(),
            &json!({}),
        )
        .unwrap();

        assert!(prompt.contains("narrow self-check is not enough"));
        assert!(prompt.contains("stateful/repeated operations"));
        assert!(prompt.contains("one current input"));
        assert!(prompt.contains("field names"));
        assert!(prompt.contains("parameter names"));
        assert!(prompt.contains("generated clients"));
        assert!(prompt.contains("propose concrete read/search/test candidates"));
        assert!(prompt.contains("\"tool_id\""));
        assert!(prompt.contains("object args"));
    }
}
