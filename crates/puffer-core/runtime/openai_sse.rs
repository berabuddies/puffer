use super::TurnStreamEvent;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

/// Returns true when an OpenAI response payload is encoded as SSE.
pub(super) fn is_event_stream(content_type: Option<&str>, text: &str) -> bool {
    content_type.is_some_and(|value| value.starts_with("text/event-stream"))
        || text.trim_start().starts_with("event:")
}

/// Parses a complete OpenAI SSE response body into a reconstructed JSON payload.
pub(super) fn parse_openai_sse_response(stream: &str) -> Result<Value> {
    parse_openai_sse_response_streaming(stream, &mut |_| {})
}

/// Parses an OpenAI SSE response body and emits incremental text delta events.
pub(super) fn parse_openai_sse_response_streaming<F>(
    stream: &str,
    on_event: &mut F,
) -> Result<Value>
where
    F: FnMut(TurnStreamEvent),
{
    parse_openai_sse_reader(BufReader::new(stream.as_bytes()), on_event)
}

/// Parses an OpenAI SSE stream from any buffered reader.
pub(super) fn parse_openai_sse_reader<R, F>(reader: BufReader<R>, on_event: &mut F) -> Result<Value>
where
    R: std::io::Read,
    F: FnMut(TurnStreamEvent),
{
    let mut response_id = None;
    let mut output = Vec::new();
    let mut reader = reader;
    let mut line = String::new();
    let mut data_lines = Vec::new();

    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            flush_sse_event(&data_lines, &mut response_id, &mut output, on_event)?;
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            flush_sse_event(&data_lines, &mut response_id, &mut output, on_event)?;
            data_lines.clear();
            continue;
        }
        if let Some(data) = trimmed.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }

    Ok(json!({
        "id": response_id,
        "output": output,
    }))
}

fn flush_sse_event<F>(
    data_lines: &[String],
    response_id: &mut Option<String>,
    output: &mut Vec<Value>,
    on_event: &mut F,
) -> Result<()>
where
    F: FnMut(TurnStreamEvent),
{
    let data = data_lines.join("\n");
    if data.is_empty() || data == "[DONE]" {
        return Ok(());
    }
    let event: Value =
        serde_json::from_str(&data).with_context(|| format!("invalid SSE payload: {data}"))?;
    match event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "response.created" | "response.completed" => {
            if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
                *response_id = Some(id.to_string());
            }
        }
        "response.output_item.added" | "response.output_item.done" => {
            if let Some(item) = event.get("item") {
                upsert_output_item(output, item.clone());
            }
        }
        "response.output_text.delta" => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                on_event(TurnStreamEvent::TextDelta(delta.to_string()));
            }
        }
        _ => {}
    }
    Ok(())
}

fn upsert_output_item(output: &mut Vec<Value>, item: Value) {
    if let Some(item_id) = item.get("id").and_then(Value::as_str) {
        if let Some(existing) = output
            .iter_mut()
            .find(|existing| existing.get("id").and_then(Value::as_str) == Some(item_id))
        {
            *existing = item;
            return;
        }
    }
    output.push(item);
}
