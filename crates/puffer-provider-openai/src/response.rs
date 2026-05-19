use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// A minimal OpenAI Responses API payload needed by the runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIResponsesResponse {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub output_text: Option<String>,
    #[serde(default)]
    pub output: Vec<OpenAIResponsesOutputItem>,
}

/// A single output item from the OpenAI Responses API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIResponsesOutputItem {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub arguments: Option<Value>,
    #[serde(default)]
    pub content: Vec<OpenAIResponsesContentItem>,
    /// Present when `kind == "reasoning"` — thinking-chain summary items.
    /// Aligned with Codex `ResponseItem::Reasoning.summary`.
    #[serde(default)]
    pub summary: Vec<Value>,
    /// Present when `kind == "reasoning"` — opaque encrypted thinking chain
    /// returned by the Responses API when `include` contains the
    /// `reasoning.encryptedcontent` selector. Aligned with Codex
    /// `ResponseItem::Reasoning.encrypted_content`.
    #[serde(default)]
    pub encrypted_content: Option<String>,
}

/// A content fragment nested under an OpenAI assistant message output item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAIResponsesContentItem {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
}

/// A minimal OpenAI Chat Completions payload needed by the runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIChatCompletionsResponse {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<OpenAIChatChoice>,
}

/// One completion choice returned by the Chat Completions API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIChatChoice {
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(default)]
    pub finish_reason: Option<String>,
    pub message: OpenAIChatChoiceMessage,
}

/// An assistant message returned by the Chat Completions API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenAIChatChoiceMessage {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<Value>,
    /// Reasoning chain emitted by reasoning-capable Chat-Completions
    /// providers (Moonshot Kimi, Deepseek, OpenRouter, etc.). The
    /// canonical key is `reasoning_content`; we accept `reasoning` as
    /// an alias because some compatible relays use the shorter name.
    /// Populated for non-streaming responses.
    #[serde(default, alias = "reasoning")]
    pub reasoning_content: Option<String>,
    #[serde(default, deserialize_with = "deserialize_tool_calls")]
    pub tool_calls: Vec<OpenAIChatToolCall>,
}

/// Convenience accessor: returns the reasoning chain text when the
/// message includes one, or `None` for non-reasoning models.
///
/// Tries three sources, in order:
/// 1. The dedicated `reasoning_content` field (canonical Moonshot Kimi
///    / Deepseek shape; also accepted as `reasoning` via serde alias
///    used by OpenRouter and others).
/// 2. The `reasoning` JSON object (some providers nest summary text
///    under `message.reasoning.summary` or `message.reasoning.content`).
/// 3. A `<think>…</think>` block inside `message.content` (the
///    open-source / DeepSeek-R1 distill convention).
pub fn extract_chat_completions_reasoning(
    response: &OpenAIChatCompletionsResponse,
) -> Option<String> {
    let message = response.choices.first().map(|choice| &choice.message)?;

    // (1) explicit reasoning_content field.
    if let Some(value) = message
        .reasoning_content
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(value.to_string());
    }

    // (3) <think>…</think> inside content. Catches DeepSeek-R1-style
    // distill outputs and several Moonshot / Kimi K2 modes that pack
    // the chain-of-thought into the visible content stream.
    if let Some(content) = message.content.as_ref() {
        let raw = content_to_text(content);
        if let Some(thinking) = extract_think_block(&raw) {
            return Some(thinking);
        }
    }

    None
}

/// Returns the visible-to-user portion of the assistant message,
/// stripped of any `<think>…</think>` block. Used by the agent loop
/// after `extract_chat_completions_reasoning` so the same content
/// doesn't appear twice (once in the thinking card, once in the
/// answer card). Falls back to the raw text when no think block is
/// present.
pub fn extract_chat_completions_visible_text(response: &OpenAIChatCompletionsResponse) -> String {
    response
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_ref())
        .map(content_to_text)
        .map(|raw| strip_think_block(&raw).into_owned())
        .unwrap_or_default()
}

/// Lightweight conversion that mirrors `extract_chat_content_text` but
/// is reachable from the reasoning helpers. Kept separate to avoid
/// re-exporting the private fn.
fn content_to_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    if let Some(array) = content.as_array() {
        return array
            .iter()
            .filter_map(|item| {
                let kind = item.get("type").and_then(Value::as_str)?;
                if kind == "text" || kind == "output_text" {
                    item.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

/// Returns the inner text of the FIRST `<think>...</think>` block, or
/// `None` if no such block exists. Tolerant of leading whitespace and
/// case (some providers emit `<Think>` or `<THINK>`).
fn extract_think_block(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let start_tag = lower.find("<think>")?;
    let after_open = start_tag + "<think>".len();
    let close_offset = lower[after_open..].find("</think>")?;
    let inner = &text[after_open..after_open + close_offset];
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Returns `text` with the FIRST `<think>...</think>` block removed.
/// Used to compute the "visible answer" when reasoning is embedded
/// inline in the content stream.
fn strip_think_block(text: &str) -> std::borrow::Cow<'_, str> {
    let lower = text.to_ascii_lowercase();
    let Some(start_tag) = lower.find("<think>") else {
        return std::borrow::Cow::Borrowed(text);
    };
    let after_open = start_tag + "<think>".len();
    let Some(close_offset) = lower[after_open..].find("</think>") else {
        return std::borrow::Cow::Borrowed(text);
    };
    let close_end = after_open + close_offset + "</think>".len();
    let mut out = String::with_capacity(text.len() - (close_end - start_tag));
    out.push_str(&text[..start_tag]);
    out.push_str(&text[close_end..]);
    std::borrow::Cow::Owned(out.trim().to_string())
}

/// A tool-call item nested under a Chat Completions assistant message.
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
    pub arguments: Value,
}

/// A parsed tool call emitted by the OpenAI Responses API.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenAIResponseToolCall {
    pub item_id: Option<String>,
    pub status: Option<String>,
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

fn deserialize_tool_calls<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<OpenAIChatToolCall>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<Vec<OpenAIChatToolCall>>::deserialize(deserializer)?.unwrap_or_default())
}

/// Parses a serialized OpenAI Responses API payload.
pub(crate) fn parse_responses_response(payload: &str) -> Result<OpenAIResponsesResponse> {
    serde_json::from_str(payload).context("failed to parse OpenAI Responses payload")
}

/// Parses a serialized OpenAI Chat Completions payload.
pub(crate) fn parse_chat_completions_response(
    payload: &str,
) -> Result<OpenAIChatCompletionsResponse> {
    serde_json::from_str(payload).context("failed to parse OpenAI Chat Completions payload")
}

/// Extracts assistant text from a parsed OpenAI Responses payload.
pub(crate) fn extract_responses_text(response: &OpenAIResponsesResponse) -> String {
    if let Some(text) = response.output_text.as_ref() {
        if !text.trim().is_empty() {
            return text.clone();
        }
    }

    response
        .output
        .iter()
        .filter(|item| item.kind == "message")
        .flat_map(|item| item.content.iter())
        .filter(|item| item.kind == "output_text")
        .filter_map(|item| item.text.as_deref())
        .collect::<Vec<_>>()
        .join("")
}

/// Extracts tool calls from a parsed OpenAI Responses payload.
pub(crate) fn extract_responses_tool_calls(
    response: &OpenAIResponsesResponse,
) -> Result<Vec<OpenAIResponseToolCall>> {
    response
        .output
        .iter()
        .filter(|item| item.kind == "function_call")
        .map(parse_tool_call)
        .collect()
}

/// Extracts assistant text from a parsed OpenAI Chat Completions payload.
pub(crate) fn extract_chat_completions_text(response: &OpenAIChatCompletionsResponse) -> String {
    response
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_ref())
        .map(extract_chat_content_text)
        .unwrap_or_default()
}

/// Extracts tool calls from a parsed OpenAI Chat Completions payload.
pub(crate) fn extract_chat_completions_tool_calls(
    response: &OpenAIChatCompletionsResponse,
) -> Result<Vec<OpenAIResponseToolCall>> {
    response
        .choices
        .first()
        .into_iter()
        .flat_map(|choice| choice.message.tool_calls.iter())
        .map(|tool_call| {
            let arguments = parse_tool_arguments(&tool_call.function.arguments, &tool_call.id)
                .with_context(|| {
                    format!(
                        "failed to parse OpenAI Chat Completions tool arguments for call {}",
                        tool_call.id
                    )
                })?;
            Ok(OpenAIResponseToolCall {
                item_id: None,
                status: None,
                call_id: tool_call.id.clone(),
                name: tool_call.function.name.clone(),
                arguments,
            })
        })
        .collect()
}

fn extract_chat_content_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("text")
                .and_then(Value::as_str)
                .or_else(|| item.get("content").and_then(Value::as_str))
        })
        .collect::<Vec<_>>()
        .join("")
}

fn parse_tool_call(item: &OpenAIResponsesOutputItem) -> Result<OpenAIResponseToolCall> {
    let call_id = item
        .call_id
        .as_ref()
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| anyhow!("OpenAI tool call item is missing call_id"))?;
    let name = item
        .name
        .as_ref()
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| anyhow!("OpenAI tool call item is missing name"))?;
    let raw_arguments = item
        .arguments
        .as_ref()
        .ok_or_else(|| anyhow!("OpenAI tool call item is missing arguments"))?;
    let arguments = parse_tool_arguments(raw_arguments, &call_id)
        .with_context(|| format!("failed to parse OpenAI tool arguments for call {call_id}"))?;

    Ok(OpenAIResponseToolCall {
        item_id: item.id.clone(),
        status: item.status.clone(),
        call_id,
        name,
        arguments,
    })
}

fn parse_tool_arguments(raw_arguments: &Value, call_id: &str) -> Result<Value> {
    match raw_arguments {
        Value::String(encoded) => {
            if encoded.is_empty() {
                return Err(anyhow!("OpenAI tool call item is missing arguments"));
            }
            serde_json::from_str(encoded)
                .with_context(|| format!("invalid JSON string arguments for call {call_id}"))
        }
        other => Ok(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_text_from_message_content() {
        let response = parse_responses_response(
            r#"{
                "id": "resp_123",
                "model": "gpt-5",
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "alpha " },
                            { "type": "output_text", "text": "beta" }
                        ]
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(extract_responses_text(&response), "alpha beta");
        assert!(extract_responses_tool_calls(&response).unwrap().is_empty());
    }

    #[test]
    fn prefers_top_level_output_text_when_present() {
        let response = parse_responses_response(
            r#"{
                "id": "resp_123",
                "output_text": "top-level text",
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "nested text" }
                        ]
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(extract_responses_text(&response), "top-level text");
    }

    #[test]
    fn extracts_function_calls_from_output_items() {
        let response = parse_responses_response(
            r#"{
                "id": "resp_123",
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            { "type": "output_text", "text": "Let me inspect that." }
                        ]
                    },
                    {
                        "type": "function_call",
                        "id": "fc_123",
                        "status": "completed",
                        "call_id": "call_123",
                        "name": "read_file",
                        "arguments": "{\"path\":\"Cargo.toml\"}"
                    }
                ]
            }"#,
        )
        .unwrap();

        let calls = extract_responses_tool_calls(&response).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].item_id.as_deref(), Some("fc_123"));
        assert_eq!(calls[0].status.as_deref(), Some("completed"));
        assert_eq!(calls[0].call_id, "call_123");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments, json!({ "path": "Cargo.toml" }));
    }

    #[test]
    fn rejects_invalid_tool_argument_json() {
        let response = parse_responses_response(
            r#"{
                "output": [
                    {
                        "type": "function_call",
                        "call_id": "call_123",
                        "name": "read_file",
                        "arguments": "{not-json}"
                    }
                ]
            }"#,
        )
        .unwrap();

        let error = extract_responses_tool_calls(&response).unwrap_err();
        assert!(error.to_string().contains("call_123"));
    }

    #[test]
    fn accepts_structured_responses_tool_arguments() {
        let response = parse_responses_response(
            r#"{
                "output": [
                    {
                        "type": "function_call",
                        "call_id": "call_456",
                        "name": "read_file",
                        "arguments": {"path":"Cargo.toml","line":3}
                    }
                ]
            }"#,
        )
        .unwrap();

        let calls = extract_responses_tool_calls(&response).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["path"], "Cargo.toml");
        assert_eq!(calls[0].arguments["line"], 3);
    }

    #[test]
    fn extracts_text_and_tool_calls_from_chat_completions() {
        let response = parse_chat_completions_response(
            r#"{
                "id": "chatcmpl_123",
                "choices": [
                    {
                        "message": {
                            "role": "assistant",
                            "content": "Let me inspect that.",
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
        .unwrap();

        assert_eq!(
            extract_chat_completions_text(&response),
            "Let me inspect that."
        );
        let calls = extract_chat_completions_tool_calls(&response).unwrap();
        assert_eq!(calls[0].call_id, "call_123");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments, json!({ "path": "Cargo.toml" }));
    }

    #[test]
    fn accepts_structured_chat_completions_tool_arguments() {
        let response = parse_chat_completions_response(
            r#"{
                "choices": [
                    {
                        "message": {
                            "role": "assistant",
                            "tool_calls": [
                                {
                                    "id": "call_obj",
                                    "type": "function",
                                    "function": {
                                        "name": "search_text",
                                        "arguments": {
                                            "query": "tool schema",
                                            "path": "."
                                        }
                                    }
                                }
                            ]
                        }
                    }
                ]
            }"#,
        )
        .unwrap();

        let calls = extract_chat_completions_tool_calls(&response).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search_text");
        assert_eq!(calls[0].arguments["query"], "tool schema");
    }
}
