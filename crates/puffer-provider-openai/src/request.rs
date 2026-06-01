use crate::auth::OpenAIAuth;
use crate::codex::codex_user_agent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod responses_include;

fn is_false(value: &bool) -> bool {
    !*value
}

/// A minimal OpenAI Responses API request payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIResponsesRequest {
    pub model: String,
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<OpenAIResponsesTextConfig>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<OpenAIChatResponseFormat>,
    /// Top-level `reasoning_effort` (canonical OpenAI shape, also
    /// honored by Moonshot Kimi and DeepSeek V4 alongside their own
    /// thinking flag). Maps to one of the puffer effort levels (or
    /// the per-model `reasoning_effort_map` override).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// OpenRouter-style nested reasoning param: `{ "effort": "..." }`.
    /// Mutually exclusive with `reasoning_effort` in practice; both
    /// are emitted only when the model's compat says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Value>,
    /// DeepSeek V4 style `thinking: { type: "enabled" }` plus
    /// `reasoning_effort` at the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
    /// Z.ai / Qwen style `enable_thinking: true` at the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
    /// Qwen via vLLM / chat-template style
    /// `chat_template_kwargs: { enable_thinking: true }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_template_kwargs: Option<Value>,
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
    /// Empty `reasoning_content` injected on assistant messages when
    /// the provider's compat specifies
    /// `requires_reasoning_content_on_assistant_messages: true`.
    /// DeepSeek V4 rejects multi-turn requests without this. Other
    /// providers ignore the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
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
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict: bool,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<OpenAIResponsesTextConfig>,
}

/// Top-level `text` block on the OpenAI Responses API. Carries
/// either a structured-output `format` (used for JSON-schema
/// coercion) or a `verbosity` knob, or both. Codex CLI sets
/// `verbosity` to one of `low` / `medium` / `high` to control how
/// terse the assistant's prose is. Pi-mono parity:
/// `pi-mono/packages/ai/src/providers/openai-codex-responses.ts:328`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OpenAIResponsesTextConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<OpenAIResponsesTextFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<String>,
}

/// One structured output format accepted by the OpenAI Responses API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIResponsesTextFormat {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub schema: Value,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict: bool,
}

/// Structured output configuration for OpenAI Chat Completions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIChatResponseFormat {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "json_schema")]
    pub json_schema: OpenAIChatResponseJsonSchema,
}

/// JSON Schema payload nested under a Chat Completions response format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIChatResponseJsonSchema {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub schema: Value,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict: bool,
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
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict: bool,
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

/// A Realtime API client-secret minting request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIRealtimeClientSecretRequest {
    pub session: Value,
}

/// Runtime request configuration for the OpenAI provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAIRequestConfig {
    pub base_url: String,
    pub version: String,
    pub auth: OpenAIAuth,
    pub originator: String,
    pub session_id: Option<String>,
    pub account_id: Option<String>,
    pub custom_headers: Vec<(String, String)>,
    pub query_params: Vec<(String, String)>,
    /// Override for the Chat Completions endpoint path. When `None`,
    /// defaults to `/v1/chat/completions`. Non-OpenAI relays whose base
    /// URL already encodes a versioned prefix (e.g. Zhipu's
    /// `https://open.bigmodel.cn/api/paas/v4`) need to set this to
    /// `/chat/completions` so we don't construct
    /// `…/v4/v1/chat/completions` and 404 the call.
    pub chat_completions_path: Option<String>,
    /// Override for the Responses endpoint path. When `None`, defaults
    /// to `/v1/responses`. Same rationale as `chat_completions_path`
    /// for relays that already include a versioned prefix in
    /// `base_url`.
    pub responses_path: Option<String>,
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
    let path = config
        .chat_completions_path
        .as_deref()
        .unwrap_or("/v1/chat/completions");
    build_request_to_path(config, request, path, false)
}

/// Builds an ordered JSON POST request for OpenAI-compatible endpoints.
pub(crate) fn build_json_post_request(
    config: &OpenAIRequestConfig,
    path: &str,
    body: &Value,
) -> anyhow::Result<BuiltOpenAIRequest> {
    build_request_to_path(config, body, path, wants_event_stream(body))
}

/// Builds an ordered Realtime API client-secret request.
pub(crate) fn build_realtime_client_secret_request(
    config: &OpenAIRequestConfig,
    request: &OpenAIRealtimeClientSecretRequest,
) -> anyhow::Result<BuiltOpenAIRequest> {
    build_request_to_path(config, request, "/v1/realtime/client_secrets", false)
}

fn build_request<T: Serialize>(
    config: &OpenAIRequestConfig,
    request: &T,
) -> anyhow::Result<BuiltOpenAIRequest> {
    let path = config.responses_path.as_deref().unwrap_or("/v1/responses");
    build_request_to_path(config, request, path, false)
}

fn build_request_to_path<T: Serialize>(
    config: &OpenAIRequestConfig,
    request: &T,
    path: &str,
    accept_event_stream: bool,
) -> anyhow::Result<BuiltOpenAIRequest> {
    let normalized_path = normalized_path(&config.base_url, path);
    // Build the workspace yaml's `headers:` overrides first so we can
    // skip any default that would collide. A relay like Kimi For Coding
    // gates on the *first* `User-Agent` header it sees and rejects
    // `codex_cli_rs/...`; the user supplies `User-Agent: claude-code/1.0`
    // in the provider yaml to satisfy the gate. Sending both headers
    // (the previous behavior) made the relay still see our default
    // first and 403 the request.
    let custom_keys: std::collections::HashSet<String> = config
        .custom_headers
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect();
    let mut headers = Vec::new();
    let push_default = |headers: &mut Vec<(String, String)>, name: &str, value: String| {
        if !custom_keys.contains(&name.to_ascii_lowercase()) {
            headers.push((name.to_string(), value));
        }
    };
    push_default(&mut headers, "Content-Type", "application/json".to_string());
    push_default(
        &mut headers,
        "User-Agent",
        codex_user_agent(&config.version, &config.originator),
    );
    push_default(&mut headers, "originator", config.originator.clone());
    if normalized_path.ends_with("/responses") && accept_event_stream {
        push_default(&mut headers, "Accept", "text/event-stream".to_string());
    }
    if let Some(session_id) = config.session_id.as_deref() {
        push_default(&mut headers, "session_id", session_id.to_string());
        if normalized_path.ends_with("/responses") {
            push_default(&mut headers, "x-client-request-id", session_id.to_string());
        }
    }
    if let Some(account_id) = config.account_id.as_deref() {
        push_default(&mut headers, "ChatGPT-Account-ID", account_id.to_string());
    }
    headers.extend(config.custom_headers.iter().cloned());
    match &config.auth {
        OpenAIAuth::None => {}
        OpenAIAuth::ApiKey(key) | OpenAIAuth::OAuthBearer(key) => {
            headers.push(("Authorization".to_string(), format!("Bearer {key}")));
        }
    }
    let mut url = format!(
        "{}{}",
        config.base_url.trim_end_matches('/'),
        normalized_path
    );
    if !config.query_params.is_empty() {
        let mut parsed = url::Url::parse(&url)?;
        {
            let mut pairs = parsed.query_pairs_mut();
            for (key, value) in &config.query_params {
                pairs.append_pair(key, value);
            }
        }
        url = parsed.to_string();
    }
    let mut body = serde_json::to_value(request)?;
    if normalized_path.ends_with("/responses") {
        responses_include::normalize_responses_include(&mut body);
    }

    Ok(BuiltOpenAIRequest {
        method: "POST",
        url,
        headers,
        body: serde_json::to_string(&body)?,
    })
}

fn wants_event_stream(body: &Value) -> bool {
    body.get("stream").and_then(Value::as_bool).unwrap_or(false)
}

fn normalized_path(base_url: &str, path: &str) -> String {
    if base_url.trim_end_matches('/').ends_with("/v1") && path.starts_with("/v1/") {
        path[3..].to_string()
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests;
