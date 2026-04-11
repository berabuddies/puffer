use crate::AppState;
use anyhow::{bail, Result};
use puffer_provider_openai::OpenAIChatMessage;
use puffer_provider_openai::OpenAIResponsesResponse;
use puffer_transport_anthropic::AnthropicMessage;
use serde_json::{json, Value};

/// Converts the transcript into the JSON message shape used for Anthropic retries.
pub(super) fn transcript_to_anthropic_messages(state: &AppState, input: &str) -> Vec<Value> {
    let mut messages = state
        .transcript
        .iter()
        .map(|message| match message.role {
            crate::MessageRole::User => json!({
                "role": "user",
                "content": message.text,
            }),
            crate::MessageRole::Assistant => json!({
                "role": "assistant",
                "content": message.text,
            }),
            crate::MessageRole::System
            | crate::MessageRole::ToolCall
            | crate::MessageRole::ToolResult => json!({
                "role": "user",
                "content": format!("[system]\n{}", message.text),
            }),
        })
        .collect::<Vec<_>>();
    if messages.is_empty() {
        messages.push(json!({
            "role": "user",
            "content": input,
        }));
    }
    messages
}

/// Converts the transcript into the typed Anthropic request message shape.
pub(super) fn transcript_to_anthropic_request_messages(
    state: &AppState,
    input: &str,
) -> Vec<AnthropicMessage> {
    let mut messages = state
        .transcript
        .iter()
        .map(|message| AnthropicMessage {
            role: match message.role {
                crate::MessageRole::Assistant => "assistant".to_string(),
                crate::MessageRole::User
                | crate::MessageRole::System
                | crate::MessageRole::ToolCall
                | crate::MessageRole::ToolResult => "user".to_string(),
            },
            content: match message.role {
                crate::MessageRole::System
                | crate::MessageRole::ToolCall
                | crate::MessageRole::ToolResult => format!("[system]\n{}", message.text),
                _ => message.text.clone(),
            },
        })
        .collect::<Vec<_>>();
    if messages.is_empty() {
        messages.push(AnthropicMessage {
            role: "user".to_string(),
            content: input.to_string(),
        });
    }
    messages
}

/// Converts the transcript into OpenAI Responses API replay input items.
pub(super) fn transcript_to_openai_input(state: &AppState, input: &str) -> Value {
    if state.transcript.is_empty() {
        return Value::String(input.to_string());
    }

    Value::Array(
        state
            .transcript
            .iter()
            .enumerate()
            .flat_map(|(index, message)| match message.role {
                crate::MessageRole::User => vec![json!({
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": message.text,
                        }
                    ],
                })],
                crate::MessageRole::Assistant => vec![json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": message.text,
                            "annotations": [],
                        }
                    ],
                    "status": "completed",
                    "id": format!("msg_{index}"),
                })],
                crate::MessageRole::System => vec![json!({
                    "role": "system",
                    "content": message.text,
                })],
                crate::MessageRole::ToolCall => {
                    let call_id = message.call_id.clone().unwrap_or_default();
                    let name = message.tool_id.clone().unwrap_or_default();
                    let arguments = message.tool_input.clone().unwrap_or_else(|| "{}".into());
                    vec![json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": name,
                        "arguments": arguments,
                    })]
                }
                crate::MessageRole::ToolResult => {
                    let call_id = message.call_id.clone().unwrap_or_default();
                    vec![json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": message.text,
                    })]
                }
            })
            .collect(),
    )
}

/// Converts the transcript into OpenAI Chat Completions messages.
pub(super) fn transcript_to_openai_chat_messages(
    state: &AppState,
    input: &str,
) -> Vec<OpenAIChatMessage> {
    let mut messages = state
        .transcript
        .iter()
        .map(|message| OpenAIChatMessage {
            role: match message.role {
                crate::MessageRole::User => "user".to_string(),
                crate::MessageRole::Assistant => "assistant".to_string(),
                crate::MessageRole::System
                | crate::MessageRole::ToolCall
                | crate::MessageRole::ToolResult => "system".to_string(),
            },
            content: Some(json!(message.text)),
            tool_call_id: None,
            tool_calls: Vec::new(),
        })
        .collect::<Vec<_>>();
    if messages.is_empty() {
        messages.push(OpenAIChatMessage {
            role: "user".to_string(),
            content: Some(json!(input)),
            tool_call_id: None,
            tool_calls: Vec::new(),
        });
    }
    messages
}

/// Extracts the assistant-visible OpenAI text with a fallback for chat-shaped payloads.
pub(super) fn parse_openai_assistant_text(
    parsed: &OpenAIResponsesResponse,
    response: &Value,
    state: &AppState,
    parse_openai_text: fn(&Value) -> Result<String>,
    extract_responses_text: fn(&OpenAIResponsesResponse) -> String,
) -> Result<String> {
    let text = extract_responses_text(parsed);
    if text.trim().is_empty() {
        parse_openai_text(response).or_else(|_| parse_openai_text_fallback(response, state))
    } else {
        Ok(text)
    }
}

/// Extracts fallback assistant text from chat-shaped OpenAI payloads.
pub(super) fn parse_openai_text_fallback(response: &Value, state: &AppState) -> Result<String> {
    if let Some(text) = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        return Ok(text);
    }
    bail!(
        "provider {} returned an unsupported response shape for session {}",
        state.current_provider.as_deref().unwrap_or("unknown"),
        state.session.id
    )
}
