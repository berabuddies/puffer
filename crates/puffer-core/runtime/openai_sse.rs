use super::TurnStreamEvent;
use anyhow::{bail, Context, Result};
use puffer_provider_openai::OpenAIResponseToolCall;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::io::{BufRead, BufReader};

/// Typed result from streaming response parsing (SSE or WebSocket) — eliminates
/// the need to re-serialize/re-parse the raw `Value` into
/// `OpenAIResponsesResponse`.
pub(super) struct OpenAISseResult {
    pub(super) response_id: Option<String>,
    pub(super) input_tokens: Option<usize>,
    pub(super) output_tokens: Option<usize>,
    pub(super) cached_tokens: Option<usize>,
    pub(super) assistant_text: String,
    pub(super) tool_calls: Vec<OpenAIResponseToolCall>,
    pub(super) emitted_tool_call_ids: HashSet<String>,
    /// Reasoning items captured from `response.output_item.done` events when
    /// the request enabled `include: ["reasoning.encrypted_content"]`. These
    /// need to be re-emitted in the next turn's `input` so the model can
    /// resume its prior thought process — aligned with Codex behavior.
    pub(super) reasoning_items: Vec<Value>,
    /// Raw response Value, kept for backward-compat callers that still need it.
    pub(super) raw_response: Value,
}

/// Returns true when an OpenAI response payload is encoded as SSE.
pub(super) fn is_event_stream(content_type: Option<&str>, text: &str) -> bool {
    content_type.is_some_and(|value| value.starts_with("text/event-stream"))
        || text.trim_start().starts_with("event:")
}

/// Parses a complete OpenAI SSE response body into a reconstructed JSON payload.
/// Used by `parse_http_json_response` as fallback when non-streaming requests
/// unexpectedly receive SSE responses.
pub(super) fn parse_openai_sse_response(stream: &str) -> Result<Value> {
    let mut on_event = |_| {};
    let state = parse_openai_sse_reader_core(BufReader::new(stream.as_bytes()), &mut on_event)?;
    Ok(state.build_raw_response())
}

/// Parses an OpenAI SSE response body and emits incremental text delta events.
/// Used only in tests — production streaming uses `parse_openai_sse_reader_typed`.
#[cfg(test)]
pub(super) fn parse_openai_sse_response_streaming<F>(
    stream: &str,
    on_event: &mut F,
) -> Result<Value>
where
    F: FnMut(TurnStreamEvent),
{
    let result = parse_openai_sse_reader_core(BufReader::new(stream.as_bytes()), on_event)?;
    Ok(result.build_raw_response())
}

/// Core SSE parsing loop — returns the accumulated state for callers to convert.
fn parse_openai_sse_reader_core<R, F>(
    reader: BufReader<R>,
    on_event: &mut F,
) -> Result<OpenAISseState>
where
    R: std::io::Read,
    F: FnMut(TurnStreamEvent),
{
    let mut state = OpenAISseState::default();
    let mut reader = reader;
    let mut line = String::new();
    let mut data_lines = Vec::new();
    let mut last_event_name: Option<String> = None;
    let mut trace = openai_sse_trace_file();

    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        trace_openai_sse_line(&mut trace, read, &line);
        if read == 0 {
            flush_sse_event(&data_lines, &mut state, on_event)?;
            if !state.terminal && is_terminal_event_name(last_event_name.as_deref()) {
                state.terminal = true;
                trace_openai_sse_message(
                    &mut trace,
                    "EOF after terminal event header without data — treating as terminal",
                );
            }
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
        } else if let Some(name) = trimmed.strip_prefix("event:") {
            last_event_name = Some(name.trim().to_string());
        }
    }

    Ok(state)
}

fn is_terminal_event_name(name: Option<&str>) -> bool {
    matches!(
        name,
        Some(
            "response.completed"
                | "response.done"
                | "response.incomplete"
                | "response.cancelled"
        )
    )
}

/// Parses an OpenAI SSE stream and returns typed result with tool calls,
/// assistant text, and metadata — no post-parse roundtrip needed.
pub(super) fn parse_openai_sse_reader_typed<R, F>(
    reader: BufReader<R>,
    on_event: &mut F,
) -> Result<OpenAISseResult>
where
    R: std::io::Read,
    F: FnMut(TurnStreamEvent),
{
    parse_openai_sse_reader_core(reader, on_event).map(OpenAISseState::into_typed_result)
}

fn openai_sse_trace_file() -> Option<File> {
    let path = std::env::var("PUFFER_OPENAI_SSE_TRACE_PATH").ok()?;
    OpenOptions::new().create(true).append(true).open(path).ok()
}

fn trace_now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn trace_openai_sse_line(trace: &mut Option<File>, read: usize, line: &str) {
    if let Some(trace) = trace {
        let _ = writeln!(trace, "{} READ {read}: {:?}", trace_now_ms(), line);
    }
}

fn trace_openai_sse_message(trace: &mut Option<File>, message: &str) {
    if let Some(trace) = trace {
        let _ = writeln!(trace, "{} TRACE: {message}", trace_now_ms());
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
    process_openai_event(&event, state, on_event)
}

/// Processes a single parsed OpenAI streaming event and updates accumulator
/// state. Returns `Ok(true)` when a terminal event is received (the caller
/// should stop reading). This function is shared between the SSE and WebSocket
/// transports — both produce identical event schemas.
pub(super) fn process_openai_event<F>(
    event: &Value,
    state: &mut OpenAISseState,
    on_event: &mut F,
) -> Result<bool>
where
    F: FnMut(TurnStreamEvent),
{
    match event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "response.created" => {
            state.update_response(event.get("response"));
        }
        "response.output_item.added" => {
            if let Some(item) = event.get("item") {
                upsert_output_item(&mut state.output, item.clone());
            }
        }
        "response.output_item.done" => {
            if let Some(item) = event.get("item") {
                upsert_output_item(&mut state.output, item.clone());
                // Eagerly emit tool call as soon as each function_call item completes,
                // so the TUI can display pending tool calls without waiting for the
                // full response to finish.
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    if let (Some(name), Some(arguments_str)) = (
                        item.get("name").and_then(Value::as_str),
                        item.get("arguments").and_then(Value::as_str),
                    ) {
                        let call_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let already_emitted = !call_id.is_empty()
                            && !state.emitted_tool_call_ids.insert(call_id.to_string());
                        if !already_emitted {
                            on_event(TurnStreamEvent::ToolCallsRequested(vec![
                                super::ToolCallRequest {
                                    call_id: call_id.to_string(),
                                    tool_id: name.to_string(),
                                    input: arguments_str.to_string(),
                                },
                            ]));
                        }
                        // Accumulate typed tool call — avoids later re-parse.
                        // On invalid JSON, preserve the raw string so downstream
                        // can report a proper error instead of silently running
                        // the tool with empty arguments (Codex parity).
                        let arguments = serde_json::from_str::<Value>(arguments_str)
                            .unwrap_or_else(|_| Value::String(arguments_str.to_string()));
                        state.tool_calls.push(OpenAIResponseToolCall {
                            item_id: item.get("id").and_then(Value::as_str).map(str::to_string),
                            status: item
                                .get("status")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            call_id: call_id.to_string(),
                            name: name.to_string(),
                            arguments,
                        });
                    }
                }
            }
        }
        "response.reasoning_summary_text.delta" => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                on_event(TurnStreamEvent::ThinkingDelta(delta.to_string()));
            }
        }
        "response.output_text.delta" => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                state.assistant_text.push_str(delta);
                on_event(TurnStreamEvent::TextDelta(delta.to_string()));
            }
        }
        "response.completed" | "response.done" | "response.incomplete" | "response.cancelled" => {
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

/// Accumulator state for OpenAI streaming events (SSE or WebSocket).
///
/// Collects response metadata, output items, assistant text deltas, and
/// typed tool calls as they arrive. Once a terminal event is received
/// (`response.completed`, `response.done`, `response.incomplete`, or
/// `response.cancelled`), the accumulated state is converted into an
/// [`OpenAISseResult`] via [`into_typed_result`](Self::into_typed_result).
#[derive(Default)]
pub(super) struct OpenAISseState {
    response: Option<Value>,
    output: Vec<Value>,
    pub(super) terminal: bool,
    emitted_tool_call_ids: HashSet<String>,
    // Typed fields — accumulated during SSE to avoid post-parse roundtrip.
    response_id: Option<String>,
    input_tokens: Option<usize>,
    output_tokens: Option<usize>,
    cached_tokens: Option<usize>,
    assistant_text: String,
    tool_calls: Vec<OpenAIResponseToolCall>,
}

impl OpenAISseState {
    fn update_response(&mut self, response: Option<&Value>) {
        if let Some(response) = response {
            // Extract typed fields from the response envelope.
            if let Some(id) = response.get("id").and_then(Value::as_str) {
                self.response_id = Some(id.to_string());
            }
            if let Some(tokens) = response
                .pointer("/usage/input_tokens")
                .and_then(Value::as_u64)
            {
                self.input_tokens = Some(tokens as usize);
            }
            if let Some(tokens) = response
                .pointer("/usage/output_tokens")
                .and_then(Value::as_u64)
            {
                self.output_tokens = Some(tokens as usize);
            }
            if let Some(tokens) = response
                .pointer("/usage/input_tokens_details/cached_tokens")
                .and_then(Value::as_u64)
            {
                self.cached_tokens = Some(tokens as usize);
            }
            self.response = Some(response.clone());
        }
    }

    fn build_raw_response(&self) -> Value {
        let mut response = self
            .response
            .clone()
            .unwrap_or_else(|| json!({ "id": Value::Null, "output": Value::Array(Vec::new()) }));
        if response.get("output").is_none() || !self.output.is_empty() {
            response["output"] = Value::Array(self.output.clone());
        }
        response
    }

    /// Consumes the accumulated SSE/WS state and produces a typed result.
    ///
    /// This avoids re-serializing the raw response into
    /// `OpenAIResponsesResponse` — the typed fields (`response_id`,
    /// `input_tokens`, `tool_calls`, etc.) are already populated during
    /// streaming. A raw `Value` is still built for backward-compat callers.
    pub(super) fn into_typed_result(self) -> OpenAISseResult {
        let raw_response = self.build_raw_response();
        // If no text was accumulated from deltas, fall back to extracting from
        // completed message output items.
        let assistant_text = if self.assistant_text.is_empty() {
            extract_text_from_output_items(&self.output)
        } else {
            self.assistant_text
        };
        // Collect reasoning items in the order they appeared in `output`. These
        // must be replayed in the next turn's `input` so the model can continue
        // its prior reasoning chain — otherwise high-effort reasoning restarts
        // from scratch every turn.
        let reasoning_items: Vec<Value> = self
            .output
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
            .cloned()
            .collect();
        OpenAISseResult {
            response_id: self.response_id,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cached_tokens: self.cached_tokens,
            assistant_text,
            tool_calls: self.tool_calls,
            emitted_tool_call_ids: self.emitted_tool_call_ids,
            reasoning_items,
            raw_response,
        }
    }
}

/// Extracts assistant text from completed output items (fallback when no deltas received).
fn extract_text_from_output_items(items: &[Value]) -> String {
    let mut parts = Vec::new();
    for item in items {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        if let Some(content) = item.get("content").and_then(Value::as_array) {
            for block in content {
                if matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("output_text" | "text")
                ) {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        parts.push(text);
                    }
                }
            }
        }
    }
    parts.join("")
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
    use crate::runtime::TurnStreamEvent;
    use serde_json::json;
    use std::io::BufReader;

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
    fn parse_openai_sse_response_treats_eof_after_terminal_event_header_as_terminal() {
        // Real-world proxy bug: the upstream sends `event: response.completed\n`
        // and then closes the connection before the matching `data:` line lands.
        // Without recovery, every tool-call turn through such a proxy bails with
        // "stream closed before response.completed" and the request is retried,
        // even though the user already saw the assistant text streamed.
        let stream = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\"}}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n\n",
            "event: response.completed\n",
        );

        let parsed = parse_openai_sse_response(stream).unwrap();
        assert_eq!(parsed["id"], json!("resp_123"));
        assert_eq!(parsed["output"][0]["type"], json!("message"));
    }

    #[test]
    fn parse_openai_sse_response_does_not_recover_from_eof_after_failed_event_header() {
        // We only recover non-failure terminal events on header-only EOF, since a
        // failed response without its data line lacks the error message we need
        // to surface to the user. Falling back to "stream closed" preserves the
        // retry path for genuine failures.
        let stream = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\"}}\n\n",
            "event: response.failed\n",
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

    #[test]
    fn parse_openai_sse_reader_tracks_emitted_tool_call_ids() {
        let stream = concat!(
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_123\",\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"Cargo.toml\\\"}\"}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"status\":\"completed\"}}\n\n"
        );
        let mut requests = Vec::new();

        let parsed =
            super::parse_openai_sse_reader_typed(BufReader::new(stream.as_bytes()), &mut |event| {
                if let super::TurnStreamEvent::ToolCallsRequested(tool_calls) = event {
                    requests.extend(tool_calls);
                }
            })
            .unwrap();

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].tool_id, "read_file");
        assert!(parsed.emitted_tool_call_ids.contains("call_123"));
        // Typed path: verify tool_calls directly instead of raw Value.
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].call_id, "call_123");
        assert_eq!(parsed.tool_calls[0].name, "read_file");
        // Raw response still contains the output item.
        assert_eq!(
            parsed.raw_response["output"][0]["call_id"],
            json!("call_123")
        );
    }

    #[test]
    fn typed_result_accumulates_tool_calls_and_text() {
        let stream = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_456\"}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Let me \"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"read that.\"}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Let me read that.\"}]}}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"status\":\"completed\",\"call_id\":\"call_A\",\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"src/main.rs\\\"}\"}}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_2\",\"status\":\"completed\",\"call_id\":\"call_B\",\"name\":\"list_dir\",\"arguments\":\"{\\\"path\\\":\\\".\\\",\\\"depth\\\":2}\"}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_456\",\"status\":\"completed\",\"usage\":{\"input_tokens\":1500,\"output_tokens\":200}}}\n\n"
        );

        let result =
            super::parse_openai_sse_reader_typed(BufReader::new(stream.as_bytes()), &mut |_| {})
                .unwrap();

        // Typed fields populated directly from SSE events.
        assert_eq!(result.response_id.as_deref(), Some("resp_456"));
        assert_eq!(result.input_tokens, Some(1500));
        assert_eq!(result.assistant_text, "Let me read that.");
        assert_eq!(result.tool_calls.len(), 2);
        assert_eq!(result.tool_calls[0].call_id, "call_A");
        assert_eq!(result.tool_calls[0].name, "read_file");
        assert_eq!(result.tool_calls[0].arguments["path"], "src/main.rs");
        assert_eq!(result.tool_calls[1].call_id, "call_B");
        assert_eq!(result.tool_calls[1].name, "list_dir");
        assert_eq!(result.tool_calls[1].arguments["depth"], 2);
        // Dedup tracking.
        assert!(result.emitted_tool_call_ids.contains("call_A"));
        assert!(result.emitted_tool_call_ids.contains("call_B"));
        // Raw response still available for backward compat.
        assert_eq!(result.raw_response["id"], json!("resp_456"));
    }

    #[test]
    fn typed_result_extracts_text_from_output_items_when_no_deltas() {
        // Some providers may not send text delta events. Verify fallback to
        // output_item.done message content extraction.
        let stream = concat!(
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"fallback text\"}]}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_789\",\"status\":\"completed\"}}\n\n"
        );

        let result =
            super::parse_openai_sse_reader_typed(BufReader::new(stream.as_bytes()), &mut |_| {})
                .unwrap();

        assert_eq!(result.assistant_text, "fallback text");
    }

    #[test]
    fn typed_result_preserves_invalid_json_arguments() {
        // When arguments contain invalid JSON, the raw string should be
        // preserved as Value::String so downstream can report a proper error
        // instead of silently executing with empty arguments.
        let stream = concat!(
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"status\":\"completed\",\"call_id\":\"call_X\",\"name\":\"Bash\",\"arguments\":\"not valid json\"}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_bad\",\"status\":\"completed\"}}\n\n"
        );

        let result =
            super::parse_openai_sse_reader_typed(BufReader::new(stream.as_bytes()), &mut |_| {})
                .unwrap();

        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "Bash");
        // Invalid JSON preserved as string, not silently replaced with {}.
        assert_eq!(result.tool_calls[0].arguments, json!("not valid json"));
    }

    #[test]
    fn process_openai_event_handles_response_cancelled() {
        let stream = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            "event: response.cancelled\n",
            "data: {\"type\":\"response.cancelled\",\"response\":{\"id\":\"resp_cancel\",\"status\":\"cancelled\"}}\n\n"
        );
        let mut deltas = Vec::new();
        let result =
            super::parse_openai_sse_reader_typed(BufReader::new(stream.as_bytes()), &mut |evt| {
                if let TurnStreamEvent::TextDelta(d) = evt {
                    deltas.push(d);
                }
            })
            .unwrap();
        assert_eq!(deltas, vec!["partial"]);
        assert_eq!(result.response_id.as_deref(), Some("resp_cancel"));
    }
}
