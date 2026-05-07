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

    // EOF: flush any pending data lines that lacked a trailing blank line.
    // Some upstreams (and `BufReader::lines()` swallowing the final newline)
    // can leave the terminal `data: {message_stop}` event in the buffer
    // without a separator. Without this flush we would mis-classify a fully
    // delivered stream as truncated. Pi-mono parity:
    // `pi-mono/.../anthropic.ts:353,371` consume the trailing buffer too.
    if !data_lines.is_empty() {
        let done = flush_anthropic_event(&data_lines, &mut state, on_event)?;
        data_lines.clear();
        if done {
            return Ok(state.into_response());
        }
    }

    // Stream ended without `message_stop`. Pi-mono parity
    // (`pi-mono/packages/ai/src/providers/anthropic.ts` 83592bb2): when
    // we saw `message_start` we know the upstream is a real Anthropic
    // stream — a missing `message_stop` means it was truncated mid-flight
    // (transport drop, gateway timeout, OOM at the relay). Returning the
    // partial state silently would feed a half-built thinking block (no
    // signature, possibly truncated mid-token) into the next turn's
    // replay, where it would either fail upstream verification or drop
    // its chain-of-thought silently. Bail loudly so retry / surfacing
    // can decide what to do.
    if state.response.is_some() {
        bail!("Anthropic SSE stream ended before message_stop");
    }
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
            let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let block = event
                .get("content_block")
                .cloned()
                .unwrap_or_else(|| json!({"type": "text", "text": ""}));
            // Ensure content array is large enough
            while state.content_blocks.len() <= index {
                state
                    .content_blocks
                    .push(json!({"type": "text", "text": ""}));
            }
            state.content_blocks[index] = block;
        }
        "content_block_delta" => {
            let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
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
                        // Thinking blocks — accumulate and emit to UI.
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
                            on_event(TurnStreamEvent::ThinkingDelta(thinking.to_string()));
                        }
                    }
                    "signature_delta" => {
                        // Anthropic emits the thinking block's signature
                        // separately from `thinking_delta`. The signature
                        // is required to round-trip the thinking block on
                        // multi-turn tool flows — without it providers
                        // like `kimi-coding/k2p5` reject with
                        // "reasoning_content is missing in assistant tool
                        // call message".
                        if let Some(signature) = delta.get("signature").and_then(Value::as_str) {
                            if index < state.content_blocks.len() {
                                let existing = state.content_blocks[index]
                                    .get("signature")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                state.content_blocks[index]["signature"] =
                                    Value::String(existing + signature);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        "content_block_stop" => {
            let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
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

    #[test]
    fn parse_thinking_block_accumulates_text_and_signature() {
        // Verifies the SSE parser preserves both the `thinking` text and
        // the `signature` token on the reconstructed thinking block.
        // Without `signature_delta` accumulation the round-trip into the
        // next request would drop the thinking block (no signature →
        // unverifiable replay) and trigger "reasoning_content is missing
        // in assistant tool call message".
        let stream = concat!(
            "event:message_start\n",
            "data:{\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
            "event:content_block_start\n",
            "data:{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event:content_block_delta\n",
            "data:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Step 1.\"}}\n\n",
            "event:content_block_delta\n",
            "data:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\" Step 2.\"}}\n\n",
            "event:content_block_delta\n",
            "data:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-part-A\"}}\n\n",
            "event:content_block_delta\n",
            "data:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"-part-B\"}}\n\n",
            "event:content_block_stop\n",
            "data:{\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event:message_stop\n",
            "data:{\"type\":\"message_stop\"}\n\n",
        );

        let mut thinking_deltas = Vec::new();
        let result = parse_anthropic_sse(stream.as_bytes(), &mut |event| {
            if let TurnStreamEvent::ThinkingDelta(d) = event {
                thinking_deltas.push(d);
            }
        })
        .unwrap();

        assert_eq!(thinking_deltas, vec!["Step 1.", " Step 2."]);
        assert_eq!(result["content"][0]["type"], "thinking");
        assert_eq!(result["content"][0]["thinking"], "Step 1. Step 2.");
        assert_eq!(result["content"][0]["signature"], "sig-part-A-part-B");
    }

    /// Pi-mono parity (`pi-mono/.../anthropic.ts` 83592bb2):
    /// truncated streams that started but never reached `message_stop`
    /// must bail. Previously we silently returned the partial state,
    /// feeding a half-built thinking block (no signature, possibly
    /// truncated mid-token) into the next turn — caller would then
    /// fail upstream signature verification or lose chain-of-thought.
    #[test]
    fn truncated_stream_after_message_start_bails() {
        let stream = concat!(
            "event:message_start\n",
            "data:{\"type\":\"message_start\",\"message\":{\"id\":\"msg_t\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
            "event:content_block_start\n",
            "data:{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event:content_block_delta\n",
            "data:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n",
            // EOF here — gateway dropped the connection. No
            // content_block_stop, no message_delta, no message_stop.
        );

        let result = parse_anthropic_sse(stream.as_bytes(), &mut |_| {});
        let err = result.expect_err("truncated stream must bail");
        assert!(
            err.to_string().contains("ended before message_stop"),
            "expected truncation error, got: {err}"
        );
    }

    /// Streams that never started (no `message_start`) AND have no
    /// content fall through the legacy "no message_stop" branch and
    /// bail — same as before this change. Locks in that we did not
    /// regress the empty-stream path.
    #[test]
    fn empty_stream_with_no_message_start_still_bails() {
        let stream = "";
        let result = parse_anthropic_sse(stream.as_bytes(), &mut |_| {});
        let err = result.expect_err("empty stream must bail");
        assert!(
            err.to_string().contains("without message_stop"),
            "got: {err}"
        );
    }

    /// Pi-mono parity: when the upstream delivers the final
    /// `data: message_stop` event but the trailing blank-line separator
    /// is missing (relay framing quirk, or `BufReader::lines()`
    /// swallowing the last newline), the EOF flush must still observe
    /// the terminal event instead of mis-classifying it as truncation.
    /// Codex review caught this: previously we only flushed on empty
    /// lines, so the last event sat in `data_lines` until EOF and was
    /// then discarded.
    #[test]
    fn message_stop_without_trailing_blank_line_is_accepted() {
        let stream = concat!(
            "event:message_start\n",
            "data:{\"type\":\"message_start\",\"message\":{\"id\":\"msg_e\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
            "event:content_block_start\n",
            "data:{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event:content_block_delta\n",
            "data:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
            "event:content_block_stop\n",
            "data:{\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event:message_stop\n",
            "data:{\"type\":\"message_stop\"}\n",
            // EOF here — no trailing blank line.
        );
        let result = parse_anthropic_sse(stream.as_bytes(), &mut |_| {})
            .expect("trailing message_stop without blank line must succeed");
        assert_eq!(result["content"][0]["text"], "hi");
    }
}
