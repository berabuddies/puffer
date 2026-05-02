use super::schema::CHAT_JSON_MAX_TOKENS;
use super::*;
use crate::contract::{
    ActionContract, ApprovalSpec, ContractStatus, Idempotency, Reversibility, RiskLevel,
    SemanticIntentSpec, SideEffectClass, TrustLevel, VerificationSpec,
};
use crate::model_policy::{MAX_MODEL_EXECUTABLE_ARG_CHARS, MAX_MODEL_FOREGROUND_TIMEOUT_MS};
use puffer_provider_registry::OAuthCredential;
use serde_json::json;
use std::collections::BTreeMap;

fn tool_contract() -> CapabilityContract {
    CapabilityContract {
        contract_id: "puffer.tools".to_string(),
        version: "0.1.0".to_string(),
        status: ContractStatus::Active,
        trust_level: TrustLevel::Sandboxed,
        description: "Puffer tools".to_string(),
        actions: vec![ActionContract {
            name: "Bash".to_string(),
            description: "Run a bash command".to_string(),
            input_schema: json!({
                "type": "object",
                    "properties": {
                        "command": {"type": "string"},
                        "timeout": {"type": "integer", "minimum": 1},
                        "run_in_background": {"type": "boolean"}
                    },
                "required": ["command"],
                "additionalProperties": false
            }),
            output_schema: json!({"type": "object"}),
            side_effect_class: SideEffectClass::Unknown,
            reversibility: Reversibility::Unknown,
            idempotency: Idempotency::Unknown,
            risk_level: RiskLevel::High,
            preconditions: Vec::new(),
            postconditions: Vec::new(),
            verification: VerificationSpec {
                methods: Vec::new(),
                observation_checks: Vec::new(),
                method_templates: Vec::new(),
                templates: Vec::new(),
                required_before_completion: false,
                confidence: 0.5,
            },
            approval: ApprovalSpec {
                required: false,
                reason: None,
            },
            failure_modes: Vec::new(),
            forbidden_uses: Vec::new(),
            argument_safety: Vec::new(),
            structured_argument_safety: Vec::new(),
            semantic_intents: shell_command_intent(),
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

fn shell_command_intent() -> Vec<SemanticIntentSpec> {
    let mut slots = BTreeMap::new();
    slots.insert("command".to_string(), "command".to_string());
    vec![SemanticIntentSpec {
        intent: "shell_command".to_string(),
        slots,
        optional_slots: BTreeMap::new(),
        defaults: BTreeMap::new(),
        side_effect_class: Some(SideEffectClass::Unknown),
        slot_kinds: Default::default(),
    }]
}

fn api_key_credential(base_url: &str) -> OpenAiCredential {
    OpenAiCredential {
        auth: OpenAIAuth::ApiKey("sk-test".to_string()),
        auth_source: "test".to_string(),
        base_url: base_url.to_string(),
        account_id: None,
        refresh_token: None,
        custom_headers: Vec::new(),
        query_params: Vec::new(),
    }
}

#[test]
fn api_key_credential_uses_v1_responses() {
    let credential = api_key_credential("https://api.openai.com");
    let request = build_probe_request(&request_config(&credential), "gpt-5.5", "hi").unwrap();
    assert_eq!(request.url, "https://api.openai.com/v1/responses");
    assert!(request.body.contains("gpt-5.5"));
}

#[test]
fn env_openai_base_url_overrides_config_base_url() {
    let resolved = resolve_openai_base_url(
        Some("https://api.openai.com".to_string()),
        Some("https://api.deepseek.com".to_string()),
    );

    assert_eq!(resolved, "https://api.deepseek.com");
}

#[test]
fn deepseek_base_url_uses_chat_completions_plain_json() {
    let credential = api_key_credential("https://api.deepseek.com");
    let request = build_tool_call_request(
        &request_config(&credential),
        "deepseek-v4-pro",
        "run pwd",
        &tool_contract(),
        &[],
    )
    .unwrap();
    let body: Value = serde_json::from_str(&request.body).unwrap();

    assert_eq!(request.url, "https://api.deepseek.com/v1/chat/completions");
    assert_eq!(body["response_format"]["type"], json!("json_object"));
    assert_eq!(body["max_tokens"], json!(CHAT_JSON_MAX_TOKENS));
    assert_eq!(body["thinking"]["type"], json!("disabled"));
    assert!(body.get("reasoning_effort").is_none());
    assert_eq!(body["messages"][0]["role"], json!("system"));
    assert_eq!(body["messages"][1]["role"], json!("user"));
    assert!(body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("first byte of the assistant message must be `{`"));
    assert!(body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("Do not wrap the JSON in markdown fences"));
}

#[test]
fn deepseek_plain_fallback_omits_response_format() {
    let credential = api_key_credential("https://api.deepseek.com");
    let request = build_tool_call_plain_request(
        &request_config(&credential),
        "deepseek-v4-pro",
        "run pwd",
        &[],
    )
    .unwrap();
    let body: Value = serde_json::from_str(&request.body).unwrap();

    assert_eq!(request.url, "https://api.deepseek.com/v1/chat/completions");
    assert_eq!(body["max_tokens"], json!(CHAT_JSON_MAX_TOKENS));
    assert_eq!(body["thinking"]["type"], json!("disabled"));
    assert!(body.get("reasoning_effort").is_none());
    assert!(body.get("response_format").is_none());
    assert!(body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("reply with exactly one raw JSON object"));
}

#[test]
fn deepseek_base_url_uses_structural_json_by_default() {
    assert!(!use_chat_tool_calls("https://api.deepseek.com"));
    assert!(use_chat_tool_calls("https://openrouter.ai/api"));
}

#[test]
fn non_retryable_chat_tool_parse_errors_can_fallback_to_json_output() {
    let error =
        anyhow::anyhow!("failed to parse OpenAI Chat Completions tool arguments for call call_123");

    assert!(chat_tool_call_fallback_allowed(&error));
}

#[test]
fn openai_compatible_base_url_can_build_structural_chat_tool_call() {
    let credential = api_key_credential("https://api.deepseek.com");
    let request = build_chat_tool_call_request(
        &request_config(&credential),
        "deepseek-v4-pro",
        "run pwd",
        &tool_contract(),
    )
    .unwrap();
    let body: Value = serde_json::from_str(&request.body).unwrap();

    assert_eq!(request.url, "https://api.deepseek.com/v1/chat/completions");
    assert_eq!(body["tool_choice"], json!("required"));
    assert_eq!(body["max_tokens"], json!(CHAT_JSON_MAX_TOKENS));
    assert_eq!(body["thinking"]["type"], json!("disabled"));
    assert!(body.get("reasoning_effort").is_none());
    assert_eq!(body["tools"][0]["type"], json!("function"));
    assert_eq!(body["tools"][0]["function"]["name"], json!("Bash"));
    assert!(body.get("response_format").is_none());
}

#[test]
fn openai_compatible_can_build_observe_act_internal_tool_call() {
    let credential = api_key_credential("https://api.deepseek.com");
    let request = build_observe_act_chat_tool_request(
        &request_config(&credential),
        "deepseek-v4-pro",
        "inspect",
        &tool_contract(),
        &[],
    )
    .unwrap();
    let body: Value = serde_json::from_str(&request.body).unwrap();

    assert_eq!(request.url, "https://api.deepseek.com/v1/chat/completions");
    assert_eq!(
        body["tool_choice"]["function"]["name"],
        json!(CHAT_OBSERVE_ACT_TOOL_NAME)
    );
    assert_eq!(
        body["tools"][0]["function"]["name"],
        json!(CHAT_OBSERVE_ACT_TOOL_NAME)
    );
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["properties"]["candidates"]["items"]["oneOf"][0]
            ["properties"]["completion_role"]["enum"],
        json!(["terminal", "verification", "repair"])
    );
    assert_eq!(body["thinking"]["type"], json!("disabled"));
    assert!(body.get("response_format").is_none());
}

#[test]
fn candidate_schema_roles_follow_action_side_effects() {
    let mut contract = tool_contract();
    let mut read = contract.actions[0].clone();
    read.name = "Read".to_string();
    read.side_effect_class = SideEffectClass::LocalRead;
    read.input_schema = json!({
        "type": "object",
        "properties": {"file_path": {"type": "string"}},
        "required": ["file_path"],
        "additionalProperties": false
    });
    read.semantic_intents = Vec::new();
    let mut write = read.clone();
    write.name = "Write".to_string();
    write.side_effect_class = SideEffectClass::LocalWrite;
    contract.actions.push(read);
    contract.actions.push(write);

    let request = build_observe_act_chat_tool_request(
        &request_config(&api_key_credential("https://api.deepseek.com")),
        "deepseek-v4-pro",
        "inspect",
        &contract,
        &[],
    )
    .unwrap();
    let body: Value = serde_json::from_str(&request.body).unwrap();
    let variants =
        &body["tools"][0]["function"]["parameters"]["properties"]["candidates"]["items"]["oneOf"];

    assert_eq!(
        variants[0]["properties"]["completion_role"]["enum"],
        json!(["terminal", "verification", "repair"])
    );
    assert_eq!(
        variants[1]["properties"]["completion_role"]["enum"],
        json!(["terminal", "support", "observation", "verification"])
    );
    assert_eq!(
        variants[2]["properties"]["completion_role"]["enum"],
        json!(["terminal", "repair"])
    );
}

#[test]
fn openai_compatible_can_build_goal_verification_internal_tool_call() {
    let credential = api_key_credential("https://api.deepseek.com");
    let request = build_goal_verification_chat_tool_request(
        &request_config(&credential),
        "deepseek-v4-pro",
        "verify",
        &tool_contract(),
        &[],
    )
    .unwrap();
    let body: Value = serde_json::from_str(&request.body).unwrap();

    assert_eq!(request.url, "https://api.deepseek.com/v1/chat/completions");
    assert_eq!(
        body["tool_choice"]["function"]["name"],
        json!(CHAT_GOAL_VERIFICATION_TOOL_NAME)
    );
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["required"],
        json!([
            "satisfied",
            "confidence",
            "missing_evidence",
            "suggested_candidates"
        ])
    );
    assert_eq!(body["thinking"]["type"], json!("disabled"));
    assert!(body.get("response_format").is_none());
}

#[test]
fn chat_completion_response_text_is_extracted() {
    let text = parse_llm_response_text(
        "https://api.deepseek.com/v1/chat/completions",
        r#"{
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "{\"tool_id\":\"Bash\",\"args\":{\"command\":\"pwd\"}}"
                }
            }]
        }"#,
    )
    .unwrap();

    assert_eq!(text, r#"{"tool_id":"Bash","args":{"command":"pwd"}}"#);
}

#[test]
fn chat_completion_length_response_with_partial_json_is_retryable() {
    let error = parse_llm_response_text(
        "https://api.deepseek.com/v1/chat/completions",
        r#"{
            "choices": [{
                "finish_reason": "length",
                "message": {
                    "role": "assistant",
                    "content": "{\"candidates\":["
                }
            }]
        }"#,
    )
    .unwrap_err();

    assert!(assistant_content_missing(&error));
    assert!(assistant_content_stopped_by_length(&error));
    assert!(retryable_openai_error(&error));
}

#[test]
fn chat_completion_object_content_is_extracted_as_structural_json() {
    let text = parse_llm_response_text(
        "https://api.deepseek.com/v1/chat/completions",
        r#"{
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": {
                        "candidates": [{
                            "tool_id": "Bash",
                            "args": {"command": "pwd"},
                            "completion_role": "support",
                            "depends_on": [],
                            "rationale": "inspect"
                        }]
                    }
                }
            }],
            "usage": {"total_tokens": 10}
        }"#,
    )
    .unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();

    assert_eq!(value["candidates"][0]["tool_id"], json!("Bash"));
}

#[test]
fn oauth_credential_uses_codex_backend() {
    let stored = StoredCredential::OAuth(OAuthCredential {
        access_token: "access".to_string(),
        refresh_token: "refresh".to_string(),
        expires_at_ms: 0,
        account_id: Some("acct".to_string()),
        organization_id: None,
        email: None,
        plan_type: None,
        rate_limit_tier: None,
        scopes: Vec::new(),
        organization_name: None,
        organization_role: None,
        workspace_role: None,
    });
    let credential = credential_from_stored(
        &stored,
        "test",
        Some(CODEX_BACKEND_BASE_URL.to_string()),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let request = build_probe_request(&request_config(&credential), "gpt-5.5", "hi").unwrap();
    assert_eq!(
        request.url,
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert!(request.headers.iter().any(|(name, _)| name == "version"));
}

#[test]
fn tool_call_request_asks_for_structural_json_schema() {
    let credential = api_key_credential("https://api.openai.com");
    let request = build_tool_call_request(
        &request_config(&credential),
        "gpt-5.5",
        "run pwd",
        &tool_contract(),
        &[],
    )
    .unwrap();
    let body: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["text"]["format"]["type"], json!("json_schema"));
    assert_eq!(
        body["text"]["format"]["schema"]["oneOf"][0]["properties"]["tool_id"]["enum"],
        json!(["Bash"])
    );
    assert_eq!(
        body["text"]["format"]["schema"]["oneOf"][0]["additionalProperties"],
        json!(false)
    );
    assert_eq!(body["text"]["format"]["strict"], json!(true));
}

#[test]
fn tool_call_json_parser_accepts_strict_json() {
    let contract = tool_contract();
    let proposal = parse_tool_call_json(
        "{\"tool_id\":\"Bash\",\"args\":{\"command\":\"printf ok\",\"timeout\":1000}}",
        &contract,
    )
    .unwrap();
    assert_eq!(proposal.tool_id, "Bash");
    assert_eq!(proposal.args["command"], "printf ok");
}

#[test]
fn tool_call_json_parser_accepts_structural_args_alias() {
    let contract = tool_contract();
    let proposal = parse_tool_call_json(
        "{\"tool_id\":\"Bash\",\"object\":{\"command\":\"printf ok\"}}",
        &contract,
    )
    .unwrap();

    assert_eq!(proposal.tool_id, "Bash");
    assert_eq!(proposal.args["command"], "printf ok");
}

#[test]
fn tool_call_json_parser_accepts_structural_wrapper_and_tool_alias() {
    let contract = tool_contract();
    let proposal = parse_tool_call_json(
        "{\"tool_call\":{\"action\":\"Bash\",\"object\":{\"command\":\"printf ok\"}}}",
        &contract,
    )
    .unwrap();

    assert_eq!(proposal.tool_id, "Bash");
    assert_eq!(proposal.args["command"], "printf ok");
}

#[test]
fn tool_call_json_parser_accepts_openai_function_shape() {
    let contract = tool_contract();
    let text = json!({
        "id": "call_1",
        "type": "function",
        "function": {
            "name": "Bash",
            "arguments": "{\"command\":\"printf ok\"}"
        }
    })
    .to_string();

    let proposal = parse_tool_call_json(&text, &contract).unwrap();

    assert_eq!(proposal.tool_id, "Bash");
    assert_eq!(proposal.args["command"], "printf ok");
}

#[test]
fn tool_call_json_parser_ignores_non_operational_metadata() {
    let contract = tool_contract();
    let proposal = parse_tool_call_json(
        r#"{
            "id": "call_123",
            "tool_id": "Bash",
            "args": {"command": "printf ok"},
            "rationale": "inspect",
            "confidence": 0.7
        }"#,
        &contract,
    )
    .unwrap();

    assert_eq!(proposal.tool_id, "Bash");
    assert_eq!(proposal.args["command"], "printf ok");
}

#[test]
fn tool_call_json_parser_rejects_unknown_top_level_fields() {
    let contract = tool_contract();
    let error = parse_tool_call_json(
        r#"{
            "tool_id": "Bash",
            "args": {"command": "printf ok"},
            "command": "printf bypass"
        }"#,
        &contract,
    )
    .unwrap_err();

    assert!(error.to_string().contains("unsupported field `command`"));
}

#[test]
fn tool_call_json_parser_rejects_missing_tool_id() {
    let contract = tool_contract();
    let error =
        parse_tool_call_json("{\"args\":{\"command\":\"printf ok\"}}", &contract).unwrap_err();
    assert!(error.to_string().contains("missing `tool_id`"));
}

#[test]
fn tool_call_json_parser_rejects_markdown_wrapped_json() {
    let contract = tool_contract();
    let error = parse_tool_call_json(
        "```json\n{\"tool_id\":\"Bash\",\"args\":{\"command\":\"printf ok\"}}\n```",
        &contract,
    )
    .unwrap_err();
    assert!(error.to_string().contains("strict JSON object"));
}

#[test]
fn strict_json_parse_errors_do_not_echo_model_payload() {
    let contract = tool_contract();
    let secret_payload = format!("not json {}", "x".repeat(32_000));
    let error = parse_tool_call_json(&secret_payload, &contract).unwrap_err();
    let message = error.to_string();

    assert!(message.contains("strict JSON object"));
    assert!(message.contains("input_bytes="));
    assert!(message.len() < 512);
    assert!(!message.contains(&"x".repeat(256)));
}

#[test]
fn tool_call_json_parser_rejects_non_object_top_level() {
    let contract = tool_contract();
    let error = parse_tool_call_json("[]", &contract).unwrap_err();
    assert!(error.to_string().contains("must be a JSON object"));
}

#[test]
fn tool_call_json_parser_rejects_unavailable_tools() {
    let contract = tool_contract();
    let error = parse_tool_call_json(
        "{\"tool_id\":\"Read\",\"args\":{\"file_path\":\"/tmp/x\"}}",
        &contract,
    )
    .unwrap_err();
    assert!(error.to_string().contains("unavailable Puffer tool"));
}

#[test]
fn tool_call_json_parser_rejects_non_object_args() {
    let contract = tool_contract();
    let error = parse_tool_call_json("{\"tool_id\":\"Bash\",\"args\":\"printf ok\"}", &contract)
        .unwrap_err();
    assert!(error.to_string().contains("must be a JSON object"));
}

#[test]
fn tool_call_json_parser_rejects_args_that_fail_contract_schema() {
    let contract = tool_contract();
    let error = parse_tool_call_json("{\"tool_id\":\"Bash\",\"args\":{}}", &contract).unwrap_err();
    assert!(error.to_string().contains("do not match input_schema"));
}

#[test]
fn tool_call_json_parser_rejects_oversized_model_executable_arg() {
    let contract = tool_contract();
    let text = json!({
        "tool_id": "Bash",
        "args": {"command": "x".repeat(MAX_MODEL_EXECUTABLE_ARG_CHARS + 1)}
    })
    .to_string();

    let error = parse_tool_call_json(&text, &contract).unwrap_err();

    assert!(error
        .to_string()
        .contains("exceeds executable argument limit"));
}

#[test]
fn tool_call_json_parser_accepts_multiline_executable_payload_under_char_limit() {
    let contract = tool_contract();
    let command = std::iter::repeat("printf x")
        .take(128)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(command.chars().count() < MAX_MODEL_EXECUTABLE_ARG_CHARS);
    let text = json!({
        "tool_id": "Bash",
        "args": {"command": command}
    })
    .to_string();

    let parsed = parse_tool_call_json(&text, &contract).unwrap();

    assert_eq!(parsed.tool_id, "Bash");
}

#[test]
fn tool_call_json_parser_rejects_long_model_foreground_timeout() {
    let contract = tool_contract();
    let text = json!({
        "tool_id": "Bash",
        "args": {"command": "python solve.py", "timeout": MAX_MODEL_FOREGROUND_TIMEOUT_MS + 1}
    })
    .to_string();

    let error = parse_tool_call_json(&text, &contract).unwrap_err();

    assert!(error
        .to_string()
        .contains("exceeds foreground timeout limit"));
}

#[test]
fn candidate_parser_requires_known_completion_role() {
    let contract = tool_contract();
    for (candidate, expected) in [
        (
            json!({"tool_id":"Bash","args":{"command":"pwd"},"rationale":"inspect"}),
            "missing `completion_role`",
        ),
        (
            json!({"tool_id":"Bash","args":{"command":"pwd"},"completion_role":"done","rationale":"inspect"}),
            "unknown model candidate completion_role",
        ),
    ] {
        let error = parse_model_candidate(&candidate, &contract).unwrap_err();
        assert!(error.to_string().contains(expected));
    }
}

#[test]
fn candidate_parser_treats_observation_role_as_support() {
    let mut contract = tool_contract();
    contract.actions[0].side_effect_class = SideEffectClass::LocalRead;
    let text = json!({
        "candidates": [{
            "tool_id": "Bash",
            "args": {"command": "pwd"},
            "completion_role": "observation",
            "rationale": "inspect"
        }]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed[0].completion_role, CompletionRole::Support);
}

#[test]
fn candidate_parser_rejects_background_terminal_executable_action() {
    let contract = tool_contract();
    let candidate = json!({
        "tool_id": "Bash",
        "args": {"command": "python server.py", "run_in_background": true},
        "completion_role": "terminal",
        "rationale": "start final service"
    });

    let error = parse_model_candidate(&candidate, &contract).unwrap_err();

    assert!(error
        .to_string()
        .contains("run_in_background=true with completion_role=terminal"));
}

#[test]
fn candidate_list_parser_preserves_more_than_eight_candidates() {
    let contract = tool_contract();
    let candidates = (0..12)
        .map(|index| {
            json!({
                "tool_id": "Bash",
                "args": {"command": format!("printf {index}")},
                "completion_role": "verification",
                "rationale": "candidate"
            })
        })
        .collect::<Vec<_>>();
    let text = json!({"candidates": candidates}).to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed.len(), 12);
    assert_eq!(parsed[11].args["command"], "printf 11");
}

#[test]
fn candidate_list_parser_accepts_single_structural_candidate() {
    let contract = tool_contract();
    let text = json!({
        "tool_id": "Bash",
        "object": {"command": "printf ok"},
        "completion_role": "verification",
        "rationale": "candidate"
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].args["command"], "printf ok");
}

#[test]
fn candidate_list_parser_accepts_structural_wrappers_and_aliases() {
    let contract = tool_contract();
    let text = json!({
        "next_action": {
            "action": "Bash",
            "object": {"command": "printf ok"},
            "completion_role": "verification",
            "rationale": "candidate"
        }
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].args["command"], "printf ok");
}

#[test]
fn candidate_list_parser_accepts_nested_action_with_outer_role() {
    let contract = tool_contract();
    let text = json!({
        "candidates": [
            {
                "id": "read-ciphertexts",
                "action": {
                    "tool": "Bash",
                    "input": {"command": "printf ok"}
                },
                "completion_role": "verification",
                "rationale": "read required input data"
            }
        ]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id.as_deref(), Some("read-ciphertexts"));
    assert_eq!(parsed[0].tool_id, "Bash");
    assert_eq!(parsed[0].args["command"], "printf ok");
}

#[test]
fn candidate_list_parser_accepts_openai_function_payload_shape() {
    let contract = tool_contract();
    let text = json!({
        "candidates": [
            {
                "id": "write-script",
                "type": "function",
                "function": {
                    "name": "Bash",
                    "arguments": "{\"command\":\"printf ok\"}"
                },
                "completion_role": "verification",
                "rationale": "write bounded helper script"
            }
        ]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].tool_id, "Bash");
    assert_eq!(parsed[0].args["command"], "printf ok");
}

#[test]
fn candidate_list_parser_accepts_anthropic_tool_use_shape() {
    let contract = tool_contract();
    let text = json!({
        "candidates": [
            {
                "type": "tool_use",
                "name": "Bash",
                "input": {"command": "printf ok"},
                "completion_role": "verification",
                "rationale": "verify output"
            }
        ]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].tool_id, "Bash");
    assert_eq!(parsed[0].args["command"], "printf ok");
}

#[test]
fn candidate_list_parser_treats_unreferenced_duplicate_ids_as_metadata() {
    let contract = tool_contract();
    let text = json!({
        "candidates": [
            {
                "id": "stable-candidate-id",
                "tool_id": "Bash",
                "args": {"command": "printf one"},
                "completion_role": "verification",
                "rationale": "first"
            },
            {
                "id": "stable-candidate-id",
                "tool_id": "Bash",
                "args": {"command": "printf two"},
                "completion_role": "verification",
                "rationale": "second"
            }
        ]
    })
    .to_string();

    let parsed = parse_candidate_list_json(&text, &contract).unwrap();

    assert_eq!(parsed.len(), 2);
    assert!(parsed.iter().all(|candidate| candidate.id.is_none()));
    assert_eq!(parsed[0].args["command"], "printf one");
    assert_eq!(parsed[1].args["command"], "printf two");
}

#[test]
fn candidate_list_parser_rejects_referenced_duplicate_ids() {
    let contract = tool_contract();
    let text = json!({
        "candidates": [
            {
                "id": "ambiguous",
                "tool_id": "Bash",
                "args": {"command": "printf one"},
                "completion_role": "verification",
                "rationale": "first"
            },
            {
                "id": "ambiguous",
                "tool_id": "Bash",
                "args": {"command": "printf two"},
                "completion_role": "verification",
                "rationale": "second"
            },
            {
                "id": "dependent",
                "depends_on": ["ambiguous"],
                "tool_id": "Bash",
                "args": {"command": "printf dependent"},
                "completion_role": "verification",
                "rationale": "dependent"
            }
        ]
    })
    .to_string();

    let error = parse_candidate_list_json(&text, &contract).unwrap_err();

    assert!(error
        .to_string()
        .contains("duplicate candidate id `ambiguous`"));
}

#[test]
fn candidate_list_parser_rejects_partial_invalid_batches() {
    let contract = tool_contract();
    let text = json!({
        "candidates": [
            {
                "tool_id": "Bash",
                "args": {"command": "printf ok"},
                "completion_role": "verification",
                "rationale": "valid"
            },
            {
                "tool_id": "Bash",
                "args": {},
                "completion_role": "verification",
                "rationale": "invalid"
            }
        ]
    })
    .to_string();

    let error = parse_candidate_list_json(&text, &contract).unwrap_err();

    assert!(error
        .to_string()
        .contains("invalid candidate proposal entries"));
    assert!(error.to_string().contains("do not match input_schema"));
}

#[test]
fn candidate_list_parser_rejects_empty_candidates() {
    let contract = tool_contract();
    let error = parse_candidate_list_json("{\"candidates\":[]}", &contract).unwrap_err();

    assert!(error.to_string().contains("contained no candidates"));
}

#[test]
fn goal_verification_parser_preserves_more_than_eight_suggestions() {
    let contract = tool_contract();
    let suggestions = (0..12)
        .map(|index| {
            json!({
                "tool_id": "Bash",
                "args": {"command": format!("printf {index}")},
                "completion_role": "repair",
                "rationale": "suggestion"
            })
        })
        .collect::<Vec<_>>();
    let text = json!({
        "satisfied": false,
        "confidence": 0.1,
        "missing_evidence": [],
        "suggested_candidates": suggestions
    })
    .to_string();

    let parsed = parse_goal_verification_json(&text, &contract).unwrap();

    assert_eq!(parsed.suggested_candidates.len(), 12);
    assert_eq!(parsed.suggested_candidates[11].args["command"], "printf 11");
}

#[test]
fn goal_verification_parser_accepts_unreferenced_duplicate_suggestion_ids() {
    let contract = tool_contract();
    let text = json!({
        "satisfied": false,
        "confidence": 0.1,
        "missing_evidence": ["need repair"],
        "suggested_candidates": [
            {
                "id": "stable-candidate-id",
                "tool_id": "Bash",
                "args": {"command": "printf one"},
                "completion_role": "repair",
                "rationale": "first"
            },
            {
                "id": "stable-candidate-id",
                "tool_id": "Bash",
                "args": {"command": "printf two"},
                "completion_role": "repair",
                "rationale": "second"
            }
        ]
    })
    .to_string();

    let parsed = parse_goal_verification_json(&text, &contract).unwrap();

    assert_eq!(parsed.suggested_candidates.len(), 2);
    assert!(parsed
        .suggested_candidates
        .iter()
        .all(|candidate| candidate.id.is_none()));
    assert!(parsed.rejected_suggested_candidates.is_empty());
}

#[test]
fn goal_verification_parser_rejects_unsatisfied_without_followups() {
    let contract = tool_contract();
    let text = json!({
        "satisfied": false,
        "confidence": 0.2,
        "missing_evidence": ["need stronger evidence"],
        "suggested_candidates": []
    })
    .to_string();

    let error = parse_goal_verification_json(&text, &contract).unwrap_err();

    assert!(error
        .to_string()
        .contains("unsatisfied without structural follow-up candidates"));
}

#[test]
fn goal_verification_parser_rejects_partial_invalid_followups() {
    let contract = tool_contract();
    let text = json!({
        "satisfied": false,
        "confidence": 0.2,
        "missing_evidence": ["need stronger evidence"],
        "suggested_candidates": [
            {
                "tool_id": "Bash",
                "args": {"command": "printf valid"},
                "completion_role": "verification",
                "rationale": "valid"
            },
            {
                "tool_id": "Bash",
                "args": {},
                "completion_role": "verification",
                "rationale": "invalid"
            }
        ]
    })
    .to_string();

    let error = parse_goal_verification_json(&text, &contract).unwrap_err();

    assert!(error
        .to_string()
        .contains("invalid goal verification suggested_candidates entries"));
    assert!(error.to_string().contains("do not match input_schema"));
}

#[test]
fn goal_verification_parser_rejects_when_all_followups_are_oversized() {
    let contract = tool_contract();
    let text = json!({
        "satisfied": false,
        "confidence": 0.2,
        "missing_evidence": ["need bounded repair"],
        "suggested_candidates": [
            {
                "tool_id": "Bash",
                "args": {"command": "x".repeat(MAX_MODEL_EXECUTABLE_ARG_CHARS + 1)},
                "completion_role": "repair",
                "rationale": "oversized generated command"
            }
        ]
    })
    .to_string();

    let error = parse_goal_verification_json(&text, &contract).unwrap_err();

    assert!(error
        .to_string()
        .contains("invalid goal verification suggested_candidates entries"));
    assert!(error
        .to_string()
        .contains("exceeds executable argument limit"));
}

#[test]
fn goal_verification_parser_accepts_multiline_executable_payload_under_char_limit() {
    let contract = tool_contract();
    let command = std::iter::repeat("printf x")
        .take(128)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(command.chars().count() < MAX_MODEL_EXECUTABLE_ARG_CHARS);
    let text = json!({
        "satisfied": false,
        "confidence": 0.2,
        "missing_evidence": ["need bounded repair"],
        "suggested_candidates": [
            {
                "tool_id": "Bash",
                "args": {"command": command},
                "completion_role": "repair",
                "rationale": "oversized generated command"
            }
        ]
    })
    .to_string();

    let parsed = parse_goal_verification_json(&text, &contract).unwrap();

    assert_eq!(parsed.suggested_candidates.len(), 1);
}

#[test]
fn model_normalization_removes_provider_prefix() {
    assert_eq!(normalize_openai_model("openai/gpt-5.5"), "gpt-5.5");
    assert_eq!(normalize_openai_model("gpt-5.5"), "gpt-5.5");
}

#[test]
fn response_format_rejection_is_retryable_as_plain_json() {
    let error: anyhow::Error = OpenAIStatusError {
        status: reqwest::StatusCode::BAD_REQUEST,
        body: json!({"error": {"param": "response_format"}}).to_string(),
    }
    .into();
    assert!(response_format_schema_rejected(&error));
    assert!(should_retry_without_response_format(&error));
}

#[test]
fn empty_assistant_content_is_retryable_as_plain_json() {
    let error: anyhow::Error = OpenAINoStructuralAssistantContent {
        endpoint_kind: "chat_completions",
        finish_reason: Some("length".to_string()),
        message_keys: vec!["role".to_string()],
    }
    .into();

    assert!(should_retry_without_response_format(&error));
    assert!(retryable_openai_error(&error));
    assert!(assistant_content_stopped_by_length(&error));
}

#[test]
fn retryable_openai_status_failures_are_retryable() {
    let error: anyhow::Error = OpenAIStatusError {
        status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
        body: String::new(),
    }
    .into();

    assert!(retryable_openai_error(&error));
}

#[test]
fn wall_timeout_failures_are_retryable() {
    let error: anyhow::Error = OpenAIWallTimeoutError { timeout_secs: 1 }.into();

    assert!(retryable_openai_error(&error));
}

#[test]
fn image_context_extracts_nested_base64_payloads() {
    let urls = image_data_urls_from_context(&json!({
        "structured_output": {
            "file": {
                "base64": "aGVsbG8=",
                "type": "image/png"
            }
        }
    }));

    assert_eq!(urls, vec!["data:image/png;base64,aGVsbG8=".to_string()]);
}
