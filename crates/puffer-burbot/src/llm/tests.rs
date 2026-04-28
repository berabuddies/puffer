use super::*;
use crate::contract::{
    ApprovalSpec, ContractStatus, Idempotency, Reversibility, RiskLevel, SideEffectClass,
    TrustLevel, VerificationSpec,
};
use puffer_provider_registry::OAuthCredential;
use serde_json::json;

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
                    "timeout": {"type": "integer", "minimum": 1}
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
fn api_key_credential_uses_v1_responses() {
    let credential = OpenAiCredential {
        auth: OpenAIAuth::ApiKey("sk-test".to_string()),
        auth_source: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
        account_id: None,
        refresh_token: None,
        custom_headers: Vec::new(),
        query_params: Vec::new(),
    };
    let request = build_probe_request(&request_config(&credential), "gpt-5.5", "hi").unwrap();
    assert_eq!(request.url, "https://api.openai.com/v1/responses");
    assert!(request.body.contains("gpt-5.5"));
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
    let credential = OpenAiCredential {
        auth: OpenAIAuth::ApiKey("sk-test".to_string()),
        auth_source: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
        account_id: None,
        refresh_token: None,
        custom_headers: Vec::new(),
        query_params: Vec::new(),
    };
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
fn candidate_list_parser_preserves_more_than_eight_candidates() {
    let contract = tool_contract();
    let candidates = (0..12)
        .map(|index| {
            json!({
                "tool_id": "Bash",
                "args": {"command": format!("printf {index}")},
                "completion_role": "support",
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
fn model_normalization_removes_provider_prefix() {
    assert_eq!(normalize_openai_model("openai/gpt-5.5"), "gpt-5.5");
    assert_eq!(normalize_openai_model("gpt-5.5"), "gpt-5.5");
}

#[test]
fn response_format_rejection_is_retryable_as_plain_json() {
    let error = anyhow!(
        "OpenAI request failed with status 400 Bad Request: Invalid schema for response_format"
    );
    assert!(response_format_schema_rejected(&error));
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
