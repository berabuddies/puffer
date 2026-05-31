use super::*;
use serde_json::json;

#[test]
fn api_key_uses_bearer_auth() {
    let request = build_responses_request(
        &OpenAIRequestConfig {
            base_url: "https://api.openai.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        },
        &OpenAIResponsesRequest {
            model: "gpt-5".to_string(),
            input: "hello".to_string(),
            text: None,
        },
    )
    .unwrap();
    assert!(request
        .headers
        .iter()
        .any(|(key, value)| key == "Authorization" && value == "Bearer sk-test"));
}

#[test]
fn none_auth_omits_authorization_header() {
    let request = build_responses_request(
        &OpenAIRequestConfig {
            base_url: "http://127.0.0.1:11434/v1".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::None,
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        },
        &OpenAIResponsesRequest {
            model: "llama3.1:8b".to_string(),
            input: "hello".to_string(),
            text: None,
        },
    )
    .unwrap();
    assert!(!request
        .headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("authorization")));
}

#[test]
fn realtime_client_secret_request_targets_realtime_endpoint() {
    let request = build_realtime_client_secret_request(
        &OpenAIRequestConfig {
            base_url: "https://api.openai.com/v1".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("test-api-key".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        },
        &OpenAIRealtimeClientSecretRequest {
            session: json!({
                "type": "realtime",
                "model": "gpt-realtime-2",
                "audio": {"output": {"voice": "marin"}}
            }),
        },
    )
    .unwrap();
    let body: Value = serde_json::from_str(&request.body).unwrap();

    assert_eq!(request.method, "POST");
    assert_eq!(
        request.url,
        "https://api.openai.com/v1/realtime/client_secrets"
    );
    assert!(request
        .headers
        .iter()
        .any(|(key, value)| key == "Authorization" && value == "Bearer test-api-key"));
    assert_eq!(body["session"]["type"], "realtime");
    assert_eq!(body["session"]["model"], "gpt-realtime-2");
    assert_eq!(body["session"]["audio"]["output"]["voice"], "marin");
}

#[test]
fn tool_request_serializes_tools_and_choice() {
    let request = build_tool_responses_request(
        &OpenAIRequestConfig {
            base_url: "https://api.openai.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::OAuthBearer("oauth-token".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: Some("session-123".to_string()),
            account_id: Some("account-123".to_string()),
            custom_headers: vec![("version".to_string(), "0.1.0".to_string())],
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        },
        &OpenAIResponsesToolRequest {
            model: "gpt-5".to_string(),
            input: json!("inspect Cargo.toml"),
            tools: vec![OpenAIResponsesTool {
                kind: "function".to_string(),
                name: "read_file".to_string(),
                description: "Reads a file from disk.".to_string(),
                strict: false,
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string"
                        }
                    },
                    "required": ["path"]
                }),
                filters: None,
                user_location: None,
                external_web_access: None,
            }],
            include: Vec::new(),
            tool_choice: Some(OpenAIResponsesToolChoice::Mode(
                OpenAIResponsesToolChoiceMode::Auto,
            )),
            previous_response_id: Some("resp_123".to_string()),
            text: None,
        },
    )
    .unwrap();

    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["model"], json!("gpt-5"));
    assert_eq!(body["input"], json!("inspect Cargo.toml"));
    assert_eq!(body["tools"][0]["name"], json!("read_file"));
    assert_eq!(body["tool_choice"], json!("auto"));
    assert_eq!(body["previous_response_id"], json!("resp_123"));
    assert!(request
        .headers
        .iter()
        .any(|(key, value)| key == "session_id" && value == "session-123"));
    assert!(request
        .headers
        .iter()
        .any(|(key, value)| key == "ChatGPT-Account-ID" && value == "account-123"));
    assert!(request
        .headers
        .iter()
        .any(|(key, value)| key == "version" && value == "0.1.0"));
    assert!(request
        .headers
        .iter()
        .any(|(key, value)| { key == "Authorization" && value == "Bearer oauth-token" }));
}

#[test]
fn chat_completions_request_uses_chat_endpoint_and_tools() {
    let request = build_chat_completions_request(
        &OpenAIRequestConfig {
            base_url: "https://openrouter.ai/api/v1".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        },
        &OpenAIChatCompletionsRequest {
            model: "demo-model".to_string(),
            messages: vec![OpenAIChatMessage {
                role: "user".to_string(),
                content: Some(json!("hello")),
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning_content: None,
            }],
            tools: vec![OpenAIChatCompletionTool {
                kind: "function".to_string(),
                function: OpenAIChatCompletionToolFunction {
                    name: "read_file".to_string(),
                    description: "Reads a file.".to_string(),
                    parameters: json!({"type": "object", "properties": {}}),
                    strict: false,
                },
            }],
            tool_choice: Some(OpenAIResponsesToolChoiceMode::Auto),
            response_format: None,
            reasoning_effort: None,
            reasoning: None,
            thinking: None,
            enable_thinking: None,
            chat_template_kwargs: None,
        },
    )
    .unwrap();

    assert_eq!(request.url, "https://openrouter.ai/api/v1/chat/completions");
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["messages"][0]["role"], json!("user"));
    assert_eq!(body["tools"][0]["function"]["name"], json!("read_file"));
    assert_eq!(body["tool_choice"], json!("auto"));
}

#[test]
fn chat_completions_request_serializes_response_format() {
    let request = build_chat_completions_request(
        &OpenAIRequestConfig {
            base_url: "https://api.openai.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        },
        &OpenAIChatCompletionsRequest {
            model: "gpt-5".to_string(),
            messages: vec![OpenAIChatMessage {
                role: "user".to_string(),
                content: Some(json!("hello")),
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning_content: None,
            }],
            tools: Vec::new(),
            tool_choice: None,
            reasoning_effort: None,
            reasoning: None,
            thinking: None,
            enable_thinking: None,
            chat_template_kwargs: None,
            response_format: Some(OpenAIChatResponseFormat {
                kind: "json_schema".to_string(),
                json_schema: OpenAIChatResponseJsonSchema {
                    name: "answer".to_string(),
                    description: Some("Structured answer".to_string()),
                    schema: json!({
                        "type": "object",
                        "properties": {
                            "value": { "type": "string" }
                        },
                        "required": ["value"]
                    }),
                    strict: true,
                },
            }),
        },
    )
    .unwrap();

    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["response_format"]["type"], json!("json_schema"));
    assert_eq!(
        body["response_format"]["json_schema"]["name"],
        json!("answer")
    );
}

#[test]
fn responses_request_serializes_text_format() {
    let request = build_tool_responses_request(
        &OpenAIRequestConfig {
            base_url: "https://api.openai.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        },
        &OpenAIResponsesToolRequest {
            model: "gpt-5".to_string(),
            input: json!("hello"),
            tools: Vec::new(),
            include: Vec::new(),
            tool_choice: None,
            previous_response_id: None,
            text: Some(OpenAIResponsesTextConfig {
                verbosity: None,
                format: Some(OpenAIResponsesTextFormat {
                    kind: "json_schema".to_string(),
                    name: "answer".to_string(),
                    description: Some("Structured answer".to_string()),
                    schema: json!({
                        "type": "object",
                        "properties": {
                            "value": { "type": "string" }
                        },
                        "required": ["value"]
                    }),
                    strict: true,
                }),
            }),
        },
    )
    .unwrap();

    let body: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["text"]["format"]["type"], json!("json_schema"));
    assert_eq!(body["text"]["format"]["name"], json!("answer"));
}

#[test]
fn json_post_request_supports_codex_backend_paths() {
    let request = build_json_post_request(
        &OpenAIRequestConfig {
            base_url: "https://chatgpt.com/backend-api/codex".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::OAuthBearer("oauth-token".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: Some("session-123".to_string()),
            account_id: Some("account-123".to_string()),
            custom_headers: Vec::new(),
            query_params: vec![("api-version".to_string(), "2025-01-01".to_string())],
            chat_completions_path: None,
            responses_path: None,
        },
        "/responses",
        &json!({
            "model": "gpt-5",
            "stream": true,
        }),
    )
    .unwrap();

    assert_eq!(
        request.url,
        "https://chatgpt.com/backend-api/codex/responses?api-version=2025-01-01"
    );
    assert!(request
        .headers
        .iter()
        .any(|(key, value)| key == "ChatGPT-Account-ID" && value == "account-123"));
    assert!(request
        .headers
        .iter()
        .any(|(key, value)| key == "originator" && value == "codex_cli_rs"));
    assert!(request
        .headers
        .iter()
        .any(|(key, value)| key == "Accept" && value == "text/event-stream"));
}

#[test]
fn json_post_request_omits_sse_accept_for_non_streaming_body() {
    let request = build_json_post_request(
        &OpenAIRequestConfig {
            base_url: "https://api.openai.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        },
        "/v1/responses",
        &json!({
            "model": "gpt-5",
            "stream": false,
        }),
    )
    .unwrap();

    assert!(!request.headers.iter().any(|(key, _)| key == "Accept"));
}

#[test]
fn tool_request_omits_false_strict_fields() {
    let request = build_tool_responses_request(
        &OpenAIRequestConfig {
            base_url: "http://84.32.32.146:8317/v1".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        },
        &OpenAIResponsesToolRequest {
            model: "gpt-5.4".to_string(),
            input: Value::String("hello".to_string()),
            tools: vec![OpenAIResponsesTool {
                kind: "function".to_string(),
                name: "read_file".to_string(),
                description: "Reads a file.".to_string(),
                strict: false,
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    }
                }),
                filters: None,
                user_location: None,
                external_web_access: None,
            }],
            include: Vec::new(),
            tool_choice: Some(OpenAIResponsesToolChoice::Mode(
                OpenAIResponsesToolChoiceMode::Auto,
            )),
            previous_response_id: None,
            text: None,
        },
    )
    .unwrap();

    let body: Value = serde_json::from_str(&request.body).unwrap();
    assert!(body["tools"][0].get("strict").is_none());
}

/// Custom-headers from a workspace provider yaml must override the
/// per-crate defaults instead of being appended after them. Kimi
/// For Coding gates on the *first* `User-Agent` header it sees and
/// rejects the default `codex_cli_rs/...`; users supply
/// `User-Agent: claude-code/1.0` in the yaml to satisfy the gate.
/// Sending both headers (the previous behavior) made Kimi still
/// see our default first and 403 the request.
#[test]
fn custom_header_overrides_default() {
    let request = build_chat_completions_request(
        &OpenAIRequestConfig {
            base_url: "https://api.example.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: vec![("User-Agent".to_string(), "claude-code/1.0".to_string())],
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        },
        &OpenAIChatCompletionsRequest {
            model: "k2p5".to_string(),
            messages: vec![OpenAIChatMessage {
                role: "user".to_string(),
                content: Some(json!("hi")),
                tool_call_id: None,
                tool_calls: Vec::new(),
                reasoning_content: None,
            }],
            tools: Vec::new(),
            tool_choice: None,
            response_format: None,
            reasoning_effort: None,
            reasoning: None,
            thinking: None,
            enable_thinking: None,
            chat_template_kwargs: None,
        },
    )
    .unwrap();

    // Exactly one User-Agent header, value taken from custom_headers.
    let user_agents: Vec<&str> = request
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("User-Agent"))
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(user_agents.len(), 1, "headers: {:?}", request.headers);
    assert_eq!(user_agents[0], "claude-code/1.0");
}

fn minimal_chat_request() -> OpenAIChatCompletionsRequest {
    OpenAIChatCompletionsRequest {
        model: "demo".to_string(),
        messages: vec![OpenAIChatMessage {
            role: "user".to_string(),
            content: Some(json!("hi")),
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning_content: None,
        }],
        tools: Vec::new(),
        tool_choice: None,
        response_format: None,
        reasoning_effort: None,
        reasoning: None,
        thinking: None,
        enable_thinking: None,
        chat_template_kwargs: None,
    }
}

/// When `chat_completions_path` is unset, the canonical OpenAI
/// path `/v1/chat/completions` is appended to `base_url`.
#[test]
fn default_chat_completions_path_when_unset() {
    let request = build_chat_completions_request(
        &OpenAIRequestConfig {
            base_url: "https://api.openai.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: None,
        },
        &minimal_chat_request(),
    )
    .unwrap();
    assert_eq!(request.url, "https://api.openai.com/v1/chat/completions");
}

/// A custom `chat_completions_path` overrides the default,
/// preserving the existing `base_url` end-to-end.
#[test]
fn custom_chat_completions_path_overrides_default() {
    let request = build_chat_completions_request(
        &OpenAIRequestConfig {
            base_url: "https://relay.example.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: Some("/api/openai/chat/completions".to_string()),
            responses_path: None,
        },
        &minimal_chat_request(),
    )
    .unwrap();
    assert_eq!(
        request.url,
        "https://relay.example.com/api/openai/chat/completions"
    );
}

/// Zhipu's `base_url` already encodes a versioned prefix
/// (`/api/paas/v4`); pairing it with `/chat/completions` must
/// produce `/api/paas/v4/chat/completions` (NO `/v1/`!) so the
/// relay actually accepts the call.
#[test]
fn zhipu_style_path_constructs_correctly() {
    let request = build_chat_completions_request(
        &OpenAIRequestConfig {
            base_url: "https://open.bigmodel.cn/api/paas/v4".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: Some("/chat/completions".to_string()),
            responses_path: None,
        },
        &minimal_chat_request(),
    )
    .unwrap();
    assert_eq!(
        request.url,
        "https://open.bigmodel.cn/api/paas/v4/chat/completions"
    );
    assert!(
        !request.url.contains("/v1/"),
        "URL must not contain /v1/: {}",
        request.url
    );
}

/// The same override mechanism works for the Responses API path,
/// so relays that proxy `/v1/responses` under a different prefix
/// don't get double-versioned URLs.
#[test]
fn custom_responses_path_overrides_default() {
    let request = build_responses_request(
        &OpenAIRequestConfig {
            base_url: "https://relay.example.com".to_string(),
            version: "0.1.0".to_string(),
            auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
            chat_completions_path: None,
            responses_path: Some("/api/openai/responses".to_string()),
        },
        &OpenAIResponsesRequest {
            model: "gpt-5".to_string(),
            input: "hi".to_string(),
            text: None,
        },
    )
    .unwrap();
    assert_eq!(
        request.url,
        "https://relay.example.com/api/openai/responses"
    );
}
