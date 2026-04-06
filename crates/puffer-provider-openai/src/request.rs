use crate::auth::OpenAIAuth;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A minimal OpenAI Responses API request payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAIResponsesRequest {
    pub model: String,
    pub input: String,
}

/// A minimal OpenAI Chat Completions API request payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIChatCompletionsRequest {
    pub model: String,
    pub messages: Vec<OpenAIChatMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OpenAIChatCompletionTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<OpenAIResponsesToolChoiceMode>,
}

/// A message item accepted by the OpenAI Chat Completions API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OpenAIChatToolCall>,
}

/// A tool-call item emitted or replayed through Chat Completions messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAIChatToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OpenAIChatFunctionCall,
}

/// A function-call payload nested under a Chat Completions tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAIChatFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// A tool definition accepted by the OpenAI Chat Completions API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIChatCompletionTool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: OpenAIChatCompletionToolFunction,
}

/// A function definition nested under a Chat Completions tool payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIChatCompletionToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// A tool-enabled OpenAI Responses API request payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIResponsesToolRequest {
    pub model: String,
    pub input: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OpenAIResponsesTool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<OpenAIResponsesToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
}

/// A tool definition accepted by the OpenAI Responses API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIResponsesTool {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub parameters: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_location: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_web_access: Option<bool>,
}

/// A tool selection directive for the OpenAI Responses API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum OpenAIResponsesToolChoice {
    Mode(OpenAIResponsesToolChoiceMode),
    Named(OpenAIResponsesNamedToolChoice),
}

/// A simple tool-choice mode for the OpenAI Responses API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIResponsesToolChoiceMode {
    Auto,
    None,
    Required,
}

/// A named tool-choice directive for the OpenAI Responses API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAIResponsesNamedToolChoice {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
}

/// A tool-result item accepted by the OpenAI Responses API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAIResponsesFunctionCallOutput {
    #[serde(rename = "type")]
    pub kind: String,
    pub call_id: String,
    pub output: String,
}

/// Runtime request configuration for the OpenAI provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAIRequestConfig {
    pub base_url: String,
    pub version: String,
    pub auth: OpenAIAuth,
}

/// An ordered HTTP request representation for tests and execution adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltOpenAIRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Builds a minimal OpenAI Responses API request with ordered headers.
pub(crate) fn build_responses_request(
    config: &OpenAIRequestConfig,
    request: &OpenAIResponsesRequest,
) -> anyhow::Result<BuiltOpenAIRequest> {
    build_request(config, request)
}

/// Builds a tool-enabled OpenAI Responses API request with ordered headers.
pub(crate) fn build_tool_responses_request(
    config: &OpenAIRequestConfig,
    request: &OpenAIResponsesToolRequest,
) -> anyhow::Result<BuiltOpenAIRequest> {
    build_request(config, request)
}

/// Builds an ordered OpenAI Chat Completions API request.
pub(crate) fn build_chat_completions_request(
    config: &OpenAIRequestConfig,
    request: &OpenAIChatCompletionsRequest,
) -> anyhow::Result<BuiltOpenAIRequest> {
    build_request_to_path(config, request, "/v1/chat/completions")
}

fn build_request<T: Serialize>(
    config: &OpenAIRequestConfig,
    request: &T,
) -> anyhow::Result<BuiltOpenAIRequest> {
    build_request_to_path(config, request, "/v1/responses")
}

fn build_request_to_path<T: Serialize>(
    config: &OpenAIRequestConfig,
    request: &T,
    path: &str,
) -> anyhow::Result<BuiltOpenAIRequest> {
    let mut headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        (
            "User-Agent".to_string(),
            format!("puffer-code/{}", config.version),
        ),
    ];
    match &config.auth {
        OpenAIAuth::None => {}
        OpenAIAuth::ApiKey(key) | OpenAIAuth::OAuthBearer(key) => {
            headers.push(("Authorization".to_string(), format!("Bearer {key}")));
        }
    }
    let normalized_path = normalized_path(&config.base_url, path);
    Ok(BuiltOpenAIRequest {
        method: "POST",
        url: format!(
            "{}{}",
            config.base_url.trim_end_matches('/'),
            normalized_path
        ),
        headers,
        body: serde_json::to_string(request)?,
    })
}

fn normalized_path(base_url: &str, path: &str) -> String {
    if base_url.trim_end_matches('/').ends_with("/v1") && path.starts_with("/v1/") {
        path[3..].to_string()
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn api_key_uses_bearer_auth() {
        let request = build_responses_request(
            &OpenAIRequestConfig {
                base_url: "https://api.openai.com".to_string(),
                version: "0.1.0".to_string(),
                auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            },
            &OpenAIResponsesRequest {
                model: "gpt-5".to_string(),
                input: "hello".to_string(),
            },
        )
        .unwrap();
        assert_eq!(
            request.headers[2],
            ("Authorization".to_string(), "Bearer sk-test".to_string())
        );
    }

    #[test]
    fn none_auth_omits_authorization_header() {
        let request = build_responses_request(
            &OpenAIRequestConfig {
                base_url: "http://127.0.0.1:11434/v1".to_string(),
                version: "0.1.0".to_string(),
                auth: OpenAIAuth::None,
            },
            &OpenAIResponsesRequest {
                model: "llama3.1:8b".to_string(),
                input: "hello".to_string(),
            },
        )
        .unwrap();
        assert!(!request
            .headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("authorization")));
    }

    #[test]
    fn tool_request_serializes_tools_and_choice() {
        let request = build_tool_responses_request(
            &OpenAIRequestConfig {
                base_url: "https://api.openai.com".to_string(),
                version: "0.1.0".to_string(),
                auth: OpenAIAuth::OAuthBearer("oauth-token".to_string()),
            },
            &OpenAIResponsesToolRequest {
                model: "gpt-5".to_string(),
                input: json!("inspect Cargo.toml"),
                tools: vec![OpenAIResponsesTool {
                    kind: "function".to_string(),
                    name: "read_file".to_string(),
                    description: "Reads a file from disk.".to_string(),
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
            },
        )
        .unwrap();

        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["model"], json!("gpt-5"));
        assert_eq!(body["input"], json!("inspect Cargo.toml"));
        assert_eq!(body["tools"][0]["name"], json!("read_file"));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["previous_response_id"], json!("resp_123"));
        assert_eq!(
            request.headers[2],
            (
                "Authorization".to_string(),
                "Bearer oauth-token".to_string()
            )
        );
    }

    #[test]
    fn chat_completions_request_uses_chat_endpoint_and_tools() {
        let request = build_chat_completions_request(
            &OpenAIRequestConfig {
                base_url: "https://openrouter.ai/api/v1".to_string(),
                version: "0.1.0".to_string(),
                auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            },
            &OpenAIChatCompletionsRequest {
                model: "demo-model".to_string(),
                messages: vec![OpenAIChatMessage {
                    role: "user".to_string(),
                    content: Some(json!("hello")),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                }],
                tools: vec![OpenAIChatCompletionTool {
                    kind: "function".to_string(),
                    function: OpenAIChatCompletionToolFunction {
                        name: "read_file".to_string(),
                        description: "Reads a file.".to_string(),
                        parameters: json!({"type": "object", "properties": {}}),
                    },
                }],
                tool_choice: Some(OpenAIResponsesToolChoiceMode::Auto),
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
    fn tool_request_preserves_rich_schema_and_name() {
        let request = build_tool_responses_request(
            &OpenAIRequestConfig {
                base_url: "https://api.openai.com".to_string(),
                version: "0.1.0".to_string(),
                auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            },
            &OpenAIResponsesToolRequest {
                model: "gpt-5".to_string(),
                input: json!("inspect"),
                tools: vec![OpenAIResponsesTool {
                    kind: "function".to_string(),
                    name: "workspace/search:text.v2".to_string(),
                    description: "Searches workspace text.".to_string(),
                    parameters: json!({
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "query": {"type": "string"},
                                    "path": {"type": ["string", "null"]}
                                },
                                "required": ["query"],
                                "additionalProperties": false
                            },
                            {"type": "boolean"}
                        ]
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
            },
        )
        .unwrap();

        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["tools"][0]["name"], json!("workspace/search:text.v2"));
        assert_eq!(
            body["tools"][0]["parameters"]["oneOf"][0]["properties"]["path"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            body["tools"][0]["parameters"]["oneOf"][1]["type"],
            json!("boolean")
        );
    }

    #[test]
    fn tool_request_allows_non_function_style_tool_payload() {
        let request = build_tool_responses_request(
            &OpenAIRequestConfig {
                base_url: "https://api.openai.com".to_string(),
                version: "0.1.0".to_string(),
                auth: OpenAIAuth::ApiKey("sk-test".to_string()),
            },
            &OpenAIResponsesToolRequest {
                model: "gpt-5".to_string(),
                input: json!("search"),
                tools: vec![OpenAIResponsesTool {
                    kind: "web_search".to_string(),
                    name: String::new(),
                    description: String::new(),
                    parameters: Value::Null,
                    filters: Some(json!({
                        "allowed_domains": ["openai.com"]
                    })),
                    user_location: Some(json!({
                        "type": "approximate",
                        "country": "US",
                    })),
                    external_web_access: Some(true),
                }],
                include: vec!["web_search_call.action.sources".to_string()],
                tool_choice: None,
                previous_response_id: None,
            },
        )
        .unwrap();

        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["tools"][0]["type"], json!("web_search"));
        assert_eq!(
            body["tools"][0]["filters"]["allowed_domains"],
            json!(["openai.com"])
        );
        assert_eq!(body["include"], json!(["web_search_call.action.sources"]));
    }
}
