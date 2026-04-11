//! Anthropic SSE streaming parser for the Messages API.
//!
//! Handles event types: message_start, content_block_start,
//! content_block_delta, content_block_stop, message_delta, message_stop.

use super::TurnStreamEvent;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::BufRead;

/// Parse an Anthropic SSE stream, emitting text deltas and reconstructing
/// the final response JSON for tool-call handling.
pub(super) fn parse_anthropic_sse<R, F>(reader: R, on_event: &mut F) -> Result<Value>
where
    R: std::io::Read,
    F: FnMut(TurnStreamEvent),
{
    let buf = std::io::BufReader::new(reader);
    let mut state = AnthropicSseState::default();
    let mut data_lines: Vec<String> = Vec::new();

    for line in buf.lines() {
        let line = line.context("failed to read SSE line")?;
        let line = line.trim_end();

        if line.is_empty() {
            // Empty line = end of event, flush
            if !data_lines.is_empty() {
                let done = flush_anthropic_event(&data_lines, &mut state, on_event)?;
                data_lines.clear();
                if done {
                    return Ok(state.into_response());
                }
            }
            continue;
        }

        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
        // Ignore "event:" lines — we parse type from data payload
    }

    // Stream ended without message_stop
    if state.has_content() {
        Ok(state.into_response())
    } else {
        bail!("Anthropic SSE stream ended without message_stop")
    }
}

fn flush_anthropic_event<F>(
    data_lines: &[String],
    state: &mut AnthropicSseState,
    on_event: &mut F,
) -> Result<bool>
where
    F: FnMut(TurnStreamEvent),
{
    let data = data_lines.join("\n");
    if data.is_empty() {
        return Ok(false);
    }

    let event: Value =
        serde_json::from_str(&data).with_context(|| format!("invalid SSE payload: {data}"))?;

    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match event_type {
        "message_start" => {
            if let Some(msg) = event.get("message") {
                state.response = Some(msg.clone());
            }
        }
        "content_block_start" => {
            let index = event
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let block = event
                .get("content_block")
                .cloned()
                .unwrap_or_else(|| json!({"type": "text", "text": ""}));
            // Ensure content array is large enough
            while state.content_blocks.len() <= index {
                state.content_blocks.push(json!({"type": "text", "text": ""}));
            }
            state.content_blocks[index] = block;
        }
        "content_block_delta" => {
            let index = event
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            if let Some(delta) = event.get("delta") {
                let delta_type = delta.get("type").and_then(Value::as_str).unwrap_or("");
                match delta_type {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            // Emit streaming text delta
                            on_event(TurnStreamEvent::TextDelta(text.to_string()));
                            // Accumulate into content block
                            if index < state.content_blocks.len() {
                                let existing = state.content_blocks[index]
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                state.content_blocks[index]["text"] =
                                    Value::String(existing + text);
                            }
                        }
                    }
                    "input_json_delta" => {
                        // Tool use input delta — accumulate
                        if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                            if index < state.content_blocks.len() {
                                let existing = state.partial_json.entry(index).or_default();
                                existing.push_str(partial);
                            }
                        }
                    }
                    "thinking_delta" => {
                        // Thinking blocks — accumulate silently (not emitted to UI).
                        if let Some(thinking) = delta.get("thinking").and_then(Value::as_str) {
                            if index < state.content_blocks.len() {
                                let existing = state.content_blocks[index]
                                    .get("thinking")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                state.content_blocks[index]["thinking"] =
                                    Value::String(existing + thinking);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        "content_block_stop" => {
            let index = event
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            // Finalize tool_use input from accumulated JSON
            if let Some(json_str) = state.partial_json.remove(&index) {
                if index < state.content_blocks.len() {
                    if let Ok(parsed) = serde_json::from_str::<Value>(&json_str) {
                        state.content_blocks[index]["input"] = parsed;
                    }
                }
            }
        }
        "message_delta" => {
            if let Some(delta) = event.get("delta") {
                if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
                    state.stop_reason = Some(reason.to_string());
                }
            }
        }
        "message_stop" => {
            return Ok(true); // Terminal event
        }
        "error" => {
            let msg = event
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            bail!("Anthropic SSE error: {msg}");
        }
        _ => {} // ping, etc.
    }

    Ok(false)
}

#[derive(Default)]
struct AnthropicSseState {
    response: Option<Value>,
    content_blocks: Vec<Value>,
    partial_json: std::collections::HashMap<usize, String>,
    stop_reason: Option<String>,
}

impl AnthropicSseState {
    fn has_content(&self) -> bool {
        self.response.is_some() || !self.content_blocks.is_empty()
    }

    fn into_response(self) -> Value {
        let mut response = self.response.unwrap_or_else(|| {
            json!({
                "id": Value::Null,
                "type": "message",
                "role": "assistant",
                "content": [],
                "stop_reason": null,
            })
        });
        if !self.content_blocks.is_empty() {
            response["content"] = Value::Array(self.content_blocks);
        }
        if let Some(reason) = self.stop_reason {
            response["stop_reason"] = Value::String(reason);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_text_stream() {
        let stream = concat!(
            "event:message_start\n",
            "data:{\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
            "event:content_block_start\n",
            "data:{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event:content_block_delta\n",
            "data:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            "event:content_block_delta\n",
            "data:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\n",
            "event:content_block_stop\n",
            "data:{\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event:message_delta\n",
            "data:{\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
            "event:message_stop\n",
            "data:{\"type\":\"message_stop\"}\n\n",
        );

        let mut deltas = Vec::new();
        let result = parse_anthropic_sse(stream.as_bytes(), &mut |event| {
            if let TurnStreamEvent::TextDelta(d) = event {
                deltas.push(d);
            }
        })
        .unwrap();

        assert_eq!(deltas, vec!["Hello", " world"]);
        assert_eq!(result["content"][0]["text"], "Hello world");
        assert_eq!(result["stop_reason"], "end_turn");
    }
}
