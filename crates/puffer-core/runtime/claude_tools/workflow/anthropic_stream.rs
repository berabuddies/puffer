use crate::AppState;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnthropicStreamInput {
    action: String,
    stream: Value,
}

/// Executes one typed Anthropic stream operation for verified Lambda skills.
pub fn execute_anthropic_stream(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: AnthropicStreamInput =
        serde_json::from_value(input).context("invalid AnthropicStream input")?;
    match parsed.action.as_str() {
        "finalMessage" => {
            let stream = stream_text(&parsed.stream)?;
            let output = final_message_from_stream(&stream)?;
            Ok(serde_json::to_string_pretty(&output)?)
        }
        other => bail!("unsupported AnthropicStream action `{other}`"),
    }
}

fn stream_text(value: &Value) -> Result<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_string());
    }
    if let Some(object) = value.as_object() {
        for key in ["stream", "stdout", "output", "text", "body", "content"] {
            if let Some(text) = object.get(key).and_then(Value::as_str) {
                return Ok(text.to_string());
            }
        }
    }
    Ok(serde_json::to_string(value)?)
}

fn final_message_from_stream(stream: &str) -> Result<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(stream) {
        if let Some(text) = message_text(&value) {
            return Ok(reply(text, 1));
        }
    }

    let mut text = String::new();
    let mut events = 0usize;
    for line in stream.lines() {
        let Some(data) = line.trim_start().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let value = serde_json::from_str::<Value>(data)
            .with_context(|| "Anthropic stream data line was not JSON")?;
        events += 1;
        append_stream_text(&value, &mut text);
    }
    if text.is_empty() {
        bail!("Anthropic stream did not contain a final text message");
    }
    Ok(reply(text, events))
}

fn append_stream_text(value: &Value, text: &mut String) {
    if let Some(delta) = value.get("delta") {
        if let Some(fragment) = delta.get("text").and_then(Value::as_str) {
            text.push_str(fragment);
        }
    }
    if let Some(block) = value.get("content_block") {
        if let Some(fragment) = block.get("text").and_then(Value::as_str) {
            text.push_str(fragment);
        }
    }
    if let Some(fragment) = message_text(value) {
        text.push_str(&fragment);
    }
}

fn message_text(value: &Value) -> Option<String> {
    let content = value.get("content")?.as_array()?;
    let mut text = String::new();
    for block in content {
        if block.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(fragment) = block.get("text").and_then(Value::as_str) {
                text.push_str(fragment);
            }
        }
    }
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn reply(text: String, events: usize) -> Value {
    json!({
        "ok": true,
        "parsed_ok": true,
        "text": text,
        "events": events
    })
}
