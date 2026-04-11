use super::TurnStreamEvent;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::Write as _;
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
    let mut state = OpenAISseState::default();
    let mut reader = reader;
    let mut line = String::new();
    let mut data_lines = Vec::new();
    let mut trace = openai_sse_trace_file();

    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        trace_openai_sse_line(&mut trace, read, &line);
        if read == 0 {
            flush_sse_event(&data_lines, &mut state, on_event)?;
            if !state.terminal {
                trace_openai_sse_message(&mut trace, "EOF before terminal event");
                bail!("stream closed before response.completed");
            }
            trace_openai_sse_message(&mut trace, "EOF after terminal event");
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            if flush_sse_event(&data_lines, &mut state, on_event)? {
                trace_openai_sse_message(&mut trace, "terminal event observed");
                data_lines.clear();
                break;
            }
            data_lines.clear();
            continue;
        }
        if let Some(data) = trimmed.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }

    Ok(state.into_response())
}

fn openai_sse_trace_file() -> Option<File> {
    let path = std::env::var("PUFFER_OPENAI_SSE_TRACE_PATH").ok()?;
    OpenOptions::new().create(true).append(true).open(path).ok()
}

fn trace_openai_sse_line(trace: &mut Option<File>, read: usize, line: &str) {
    if let Some(trace) = trace {
        let _ = writeln!(trace, "READ {read}: {:?}", line);
    }
}

fn trace_openai_sse_message(trace: &mut Option<File>, message: &str) {
    if let Some(trace) = trace {
        let _ = writeln!(trace, "TRACE: {message}");
    }
}

fn flush_sse_event<F>(
    data_lines: &[String],
    state: &mut OpenAISseState,
    on_event: &mut F,
) -> Result<bool>
where
    F: FnMut(TurnStreamEvent),
{
    let data = data_lines.join("\n");
    if data.is_empty() || data == "[DONE]" {
        return Ok(false);
    }
    let event: Value =
        serde_json::from_str(&data).with_context(|| format!("invalid SSE payload: {data}"))?;
    match event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "response.created" => {
            state.update_response(event.get("response"));
        }
        "response.output_item.added" | "response.output_item.done" => {
            if let Some(item) = event.get("item") {
                upsert_output_item(&mut state.output, item.clone());
            }
        }
        "response.output_text.delta" => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                on_event(TurnStreamEvent::TextDelta(delta.to_string()));
            }
        }
        "response.completed" | "response.done" | "response.incomplete" => {
            state.update_response(event.get("response"));
            state.terminal = true;
            return Ok(true);
        }
        "response.failed" => {
            bail!("{}", format_openai_sse_error(&event));
        }
        _ => {}
    }
    Ok(false)
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

#[derive(Default)]
struct OpenAISseState {
    response: Option<Value>,
    output: Vec<Value>,
    terminal: bool,
}

impl OpenAISseState {
    fn update_response(&mut self, response: Option<&Value>) {
        if let Some(response) = response {
            self.response = Some(response.clone());
        }
    }

    fn into_response(self) -> Value {
        let mut response = self
            .response
            .unwrap_or_else(|| json!({ "id": Value::Null, "output": Value::Array(Vec::new()) }));
        if response.get("output").is_none() || !self.output.is_empty() {
            response["output"] = Value::Array(self.output);
        }
        response
    }
}

fn format_openai_sse_error(event: &Value) -> String {
    let status = event
        .pointer("/response/status")
        .and_then(Value::as_str)
        .unwrap_or("failed");
    let code = event
        .pointer("/response/error/code")
        .and_then(Value::as_str);
    let message = event
        .pointer("/response/error/message")
        .and_then(Value::as_str);
    let reason = event
        .pointer("/response/incomplete_details/reason")
        .and_then(Value::as_str);
    match (code, message, reason) {
        (_, Some(message), _) if !message.is_empty() => match code {
            Some(code) if !code.is_empty() => format!("{status}: {code}: {message}"),
            _ => format!("{status}: {message}"),
        },
        (_, _, Some(reason)) if !reason.is_empty() => format!("{status}: {reason}"),
        _ => format!("OpenAI stream failed with status {status}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_openai_sse_response, parse_openai_sse_response_streaming};
    use serde_json::json;

    #[test]
    fn parse_openai_sse_response_stops_at_terminal_event() {
        let stream = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\"}}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"status\":\"completed\"}}\n\n",
            "event: response.failed\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"message\":\"should not be read\"}}}\n\n"
        );

        let parsed = parse_openai_sse_response(stream).unwrap();
        assert_eq!(parsed["id"], json!("resp_123"));
        assert_eq!(parsed["status"], json!("completed"));
        assert_eq!(parsed["output"][0]["type"], json!("message"));
    }

    #[test]
    fn parse_openai_sse_response_requires_terminal_event() {
        let stream = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\"}}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n\n"
        );

        let error = parse_openai_sse_response(stream).unwrap_err();
        assert!(error
            .to_string()
            .contains("stream closed before response.completed"));
    }

    #[test]
    fn parse_openai_sse_response_surfaces_failed_events() {
        let stream = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\"}}\n\n",
            "event: response.failed\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"too many requests\"}}}\n\n"
        );

        let error = parse_openai_sse_response(stream).unwrap_err();
        assert!(error
            .to_string()
            .contains("failed: rate_limit_exceeded: too many requests"));
    }

    #[test]
    fn parse_openai_sse_response_accepts_incomplete_terminal_event() {
        let stream = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            "event: response.incomplete\n",
            "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_123\",\"status\":\"incomplete\"}}\n\n"
        );
        let mut deltas = Vec::new();

        let parsed = parse_openai_sse_response_streaming(stream, &mut |event| {
            if let super::TurnStreamEvent::TextDelta(delta) = event {
                deltas.push(delta);
            }
        })
        .unwrap();

        assert_eq!(deltas, vec!["partial".to_string()]);
        assert_eq!(parsed["id"], json!("resp_123"));
        assert_eq!(parsed["status"], json!("incomplete"));
    }
}
