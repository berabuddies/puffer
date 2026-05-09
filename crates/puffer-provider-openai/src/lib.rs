//! Canonical public surface for the OpenAI provider crate.
//!
//! The crate root is the only supported public entrypoint. The internal
//! `auth` and `request` modules remain private implementation details so other
//! crates consume a stable, curated API from one place.

mod auth;
mod codex;
pub mod compat;
mod request;
mod response;
mod usage;

pub use auth::OpenAIAuth;
pub use auth::OpenAIOAuthConfig;
pub use auth::OpenAIOAuthCredentials;
pub use auth::OpenAIPkce;
pub use auth::OPENAI_AUTHORIZE_URL;
pub use auth::OPENAI_CODEX_CLIENT_ID;
pub use auth::OPENAI_REDIRECT_URI;
pub use auth::OPENAI_SCOPE;
pub use auth::OPENAI_TOKEN_URL;
pub use codex::codex_user_agent;
pub use compat::CacheControlFormat;
pub use compat::MaxTokensField;
pub use compat::OpenAICompat;
pub use compat::ThinkingFormat;
pub use request::BuiltOpenAIRequest;
pub use request::OpenAIChatCompletionTool;
pub use request::OpenAIChatCompletionToolFunction;
pub use request::OpenAIChatCompletionsRequest;
pub use request::OpenAIChatFunctionCall;
pub use request::OpenAIChatMessage;
pub use request::OpenAIChatResponseFormat;
pub use request::OpenAIChatResponseJsonSchema;
pub use request::OpenAIChatToolCall;
pub use request::OpenAIRequestConfig;
pub use request::OpenAIResponsesFunctionCallOutput;
pub use request::OpenAIResponsesNamedToolChoice;
pub use request::OpenAIResponsesRequest;
pub use request::OpenAIResponsesTextConfig;
pub use request::OpenAIResponsesTextFormat;
pub use request::OpenAIResponsesTool;
pub use request::OpenAIResponsesToolChoice;
pub use request::OpenAIResponsesToolChoiceMode;
pub use request::OpenAIResponsesToolRequest;
pub use response::OpenAIChatChoice;
pub use response::OpenAIChatChoiceMessage;
pub use response::OpenAIChatCompletionsResponse;
pub use response::OpenAIResponseToolCall;
pub use response::OpenAIResponsesContentItem;
pub use response::OpenAIResponsesOutputItem;
pub use response::OpenAIResponsesResponse;
pub use usage::fetch_usage_summary;
pub use usage::OpenAIUsageError;
pub use usage::OpenAIUsageSummary;

/// Generates a PKCE verifier, challenge, and state for the OpenAI OAuth flow.
pub fn generate_pkce() -> OpenAIPkce {
    auth::generate_pkce()
}

/// Builds the OpenAI OAuth authorization URL for the provided flow settings.
pub fn build_authorization_url(config: &OpenAIOAuthConfig) -> String {
    auth::build_authorization_url(config)
}

/// Extracts an authorization code and optional state from pasted user input.
pub fn parse_authorization_input(input: &str) -> (Option<String>, Option<String>) {
    auth::parse_authorization_input(input)
}

/// Exchanges an OAuth authorization code for OpenAI bearer credentials.
pub fn exchange_authorization_code(
    code: &str,
    verifier: &str,
    redirect_uri: Option<&str>,
) -> anyhow::Result<OpenAIOAuthCredentials> {
    auth::exchange_authorization_code(code, verifier, redirect_uri)
}

/// Refreshes OpenAI bearer credentials from a stored refresh token.
pub fn refresh_oauth_token(refresh_token: &str) -> anyhow::Result<OpenAIOAuthCredentials> {
    auth::refresh_oauth_token(refresh_token)
}

/// Builds an ordered OpenAI Responses API request for execution or testing.
pub fn build_responses_request(
    config: &OpenAIRequestConfig,
    request: &OpenAIResponsesRequest,
) -> anyhow::Result<BuiltOpenAIRequest> {
    request::build_responses_request(config, request)
}

/// Builds an ordered OpenAI Responses API request with tool definitions.
pub fn build_tool_responses_request(
    config: &OpenAIRequestConfig,
    request: &OpenAIResponsesToolRequest,
) -> anyhow::Result<BuiltOpenAIRequest> {
    request::build_tool_responses_request(config, request)
}

/// Builds an ordered OpenAI-compatible Chat Completions request.
pub fn build_chat_completions_request(
    config: &OpenAIRequestConfig,
    request: &OpenAIChatCompletionsRequest,
) -> anyhow::Result<BuiltOpenAIRequest> {
    request::build_chat_completions_request(config, request)
}

/// Builds an ordered JSON POST request for OpenAI-compatible endpoints.
pub fn build_json_post_request(
    config: &OpenAIRequestConfig,
    path: &str,
    body: &serde_json::Value,
) -> anyhow::Result<BuiltOpenAIRequest> {
    request::build_json_post_request(config, path, body)
}

/// Parses a serialized OpenAI Responses API payload into typed response data.
pub fn parse_responses_response(payload: &str) -> anyhow::Result<OpenAIResponsesResponse> {
    response::parse_responses_response(payload)
}

/// Parses a serialized OpenAI-compatible Chat Completions payload.
pub fn parse_chat_completions_response(
    payload: &str,
) -> anyhow::Result<OpenAIChatCompletionsResponse> {
    response::parse_chat_completions_response(payload)
}

/// Extracts assistant text from a parsed OpenAI Responses API payload.
pub fn extract_responses_text(response: &OpenAIResponsesResponse) -> String {
    response::extract_responses_text(response)
}

/// Extracts assistant text from a parsed OpenAI-compatible Chat Completions payload.
pub fn extract_chat_completions_text(response: &OpenAIChatCompletionsResponse) -> String {
    response::extract_chat_completions_text(response)
}

/// Extracts the reasoning / thinking chain from a parsed OpenAI-compatible
/// Chat Completions payload. Non-reasoning models (or providers that omit
/// the field) return `None`. Used by the agent loop to surface a
/// `ThinkingDelta` event so the TUI's thinking block stays populated for
/// reasoning-capable Chat Completions providers (Moonshot Kimi, Deepseek,
/// OpenRouter relays). Tries the dedicated `reasoning_content` /
/// `reasoning` field first, then falls back to a `<think>…</think>`
/// block inside `content` (DeepSeek-R1 distill convention).
pub fn extract_chat_completions_reasoning(
    response: &OpenAIChatCompletionsResponse,
) -> Option<String> {
    response::extract_chat_completions_reasoning(response)
}

/// Returns the visible-to-user portion of the assistant message — same
/// as `extract_chat_completions_text` but additionally strips a leading
/// `<think>…</think>` block when one is embedded in `content`. Pair
/// with `extract_chat_completions_reasoning` so the same prose doesn't
/// land in both the thinking card and the answer card.
pub fn extract_chat_completions_visible_text(response: &OpenAIChatCompletionsResponse) -> String {
    response::extract_chat_completions_visible_text(response)
}

/// Extracts tool calls from a parsed OpenAI Responses API payload.
pub fn extract_responses_tool_calls(
    response: &OpenAIResponsesResponse,
) -> anyhow::Result<Vec<OpenAIResponseToolCall>> {
    response::extract_responses_tool_calls(response)
}

/// Extracts tool calls from a parsed OpenAI-compatible Chat Completions payload.
pub fn extract_chat_completions_tool_calls(
    response: &OpenAIChatCompletionsResponse,
) -> anyhow::Result<Vec<OpenAIResponseToolCall>> {
    response::extract_chat_completions_tool_calls(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn crate_root_builds_authorization_url() {
        let url = build_authorization_url(&OpenAIOAuthConfig {
            state: "state-1".to_string(),
            code_challenge: "challenge-1".to_string(),
            redirect_uri: OPENAI_REDIRECT_URI.to_string(),
            originator: "puffer".to_string(),
        });
        assert!(url.contains("state=state-1"));
        assert!(url.contains("code_challenge=challenge-1"));
    }

    #[test]
    fn crate_root_builds_request() {
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
        .expect("request should build");
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "https://api.openai.com/v1/responses");
    }

    #[test]
    fn crate_root_builds_tool_request() {
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
                input: json!("use tools"),
                tools: vec![OpenAIResponsesTool {
                    kind: "function".to_string(),
                    name: "read_file".to_string(),
                    description: "Reads a file from disk.".to_string(),
                    strict: false,
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" }
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
        .expect("request should build");

        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["tools"][0]["name"], json!("read_file"));
        assert_eq!(body["tool_choice"], json!("auto"));
    }

    #[test]
    fn crate_root_builds_chat_completions_request() {
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
                model: "auto".to_string(),
                messages: vec![OpenAIChatMessage {
                    role: "user".to_string(),
                    content: Some(json!("hello")),
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
        .expect("request should build");
        assert_eq!(request.url, "https://openrouter.ai/api/v1/chat/completions");
    }

    #[test]
    fn crate_root_parses_tool_calls() {
        let response = parse_responses_response(
            r#"{
                "output": [
                    {
                        "type": "function_call",
                        "call_id": "call_123",
                        "name": "read_file",
                        "arguments": "{\"path\":\"Cargo.toml\"}"
                    }
                ]
            }"#,
        )
        .expect("response should parse");

        let calls = extract_responses_tool_calls(&response).expect("tool calls should parse");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
    }

    #[test]
    fn crate_root_parses_chat_completions_tool_calls() {
        let response = parse_chat_completions_response(
            r#"{
                "choices": [
                    {
                        "message": {
                            "role": "assistant",
                            "content": "Inspecting",
                            "tool_calls": [
                                {
                                    "id": "call_123",
                                    "type": "function",
                                    "function": {
                                        "name": "read_file",
                                        "arguments": "{\"path\":\"Cargo.toml\"}"
                                    }
                                }
                            ]
                        }
                    }
                ]
            }"#,
        )
        .expect("response should parse");
        let calls =
            extract_chat_completions_tool_calls(&response).expect("tool calls should parse");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "call_123");
    }
}
