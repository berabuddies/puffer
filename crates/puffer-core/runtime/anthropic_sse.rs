//! Anthropic SSE streaming parser for the Messages API.
//!
//! Handles event types: message_start, content_block_start,
//! content_block_delta, content_block_stop, message_delta, message_stop.

use super::{TurnStreamEvent, TurnUsageReport};
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
                // `message_start` carries the initial usage block:
                // `input_tokens`, `cache_read_input_tokens`,
                // `cache_creation_input_tokens`, plus a placeholder
                // `output_tokens` (usually 1). Capture all four — output
                // is incremented by `message_delta.usage.output_tokens`
                // events for the streaming totals.
                if let Some(usage) = msg.get("usage") {
                    state.usage_input_tokens =
                        usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                    state.usage_cache_read_tokens = usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    state.usage_cache_creation_tokens = usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    state.usage_output_tokens = usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    state.saw_usage = true;
                }
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
            // Anthropic streams the running output token count in
            // `message_delta.usage.output_tokens`. This is the
            // authoritative final value (the `message_start` placeholder
            // is just `1`). Treat the last seen value as the total —
            // some relays emit cumulative, others only the terminal
            // delta, so taking the max of (start, delta) is robust.
            if let Some(usage) = event.get("usage") {
                if let Some(output) = usage.get("output_tokens").and_then(Value::as_u64) {
                    state.usage_output_tokens = state.usage_output_tokens.max(output);
                    state.saw_usage = true;
                }
                // `message_delta.usage` may also restate input / cache
                // counts; honor them when present (defensive — matches
                // pi-mono's "last write wins" treatment).
                if let Some(input) = usage.get("input_tokens").and_then(Value::as_u64) {
                    state.usage_input_tokens = state.usage_input_tokens.max(input);
                }
                if let Some(cache_read) =
                    usage.get("cache_read_input_tokens").and_then(Value::as_u64)
                {
                    state.usage_cache_read_tokens =
                        state.usage_cache_read_tokens.max(cache_read);
                }
                if let Some(cache_creation) = usage
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                {
                    state.usage_cache_creation_tokens =
                        state.usage_cache_creation_tokens.max(cache_creation);
                }
            }
        }
        "message_stop" => {
            // Emit the per-turn usage report exactly once. Without
            // this, `agent_loop::run_streaming_loop` never sees a
            // `TurnStreamEvent::Usage` for Claude users and the goal
            // budget accounting hook (`account_token_usage`) is a
            // no-op for the entire Anthropic provider — meaning
            // `tokens_used` stays at 0 and `/goal` budgets never trip.
            if state.saw_usage {
                on_event(TurnStreamEvent::Usage(TurnUsageReport {
                    input_tokens: state.usage_input_tokens,
                    output_tokens: state.usage_output_tokens,
                    cache_read_tokens: state.usage_cache_read_tokens,
                    cache_creation_tokens: state.usage_cache_creation_tokens,
                }));
            }
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
    /// Per-turn token usage, accumulated across `message_start`
    /// (initial input + cache counts) and `message_delta`
    /// (running / final output count). Emitted as a single
    /// `TurnStreamEvent::Usage` on `message_stop` so the agent loop's
    /// goal-accounting hook fires for Anthropic streams the same way
    /// it does for OpenAI streams.
    usage_input_tokens: u64,
    usage_output_tokens: u64,
    usage_cache_read_tokens: u64,
    usage_cache_creation_tokens: u64,
    saw_usage: bool,
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

    /// Without an emitted `TurnStreamEvent::Usage` the agent loop's
    /// `account_token_usage` hook is a no-op for Anthropic — meaning
    /// `/goal` budgets never trip for Claude users (the headline bug
    /// for PR #84). This test wires synthetic SSE that carries usage
    /// in `message_start` (input + cache hits) and `message_delta`
    /// (running output count) and asserts exactly one Usage event
    /// fires with the accumulated numbers.
    #[test]
    fn emits_usage_event_on_message_stop() {
        let stream = concat!(
            "event:message_start\n",
            "data:{\"type\":\"message_start\",\"message\":{\"id\":\"msg_u\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":1234,\"output_tokens\":1,\"cache_read_input_tokens\":900,\"cache_creation_input_tokens\":50}}}\n\n",
            "event:content_block_start\n",
            "data:{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event:content_block_delta\n",
            "data:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
            "event:content_block_stop\n",
            "data:{\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event:message_delta\n",
            "data:{\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":42}}\n\n",
            "event:message_stop\n",
            "data:{\"type\":\"message_stop\"}\n\n",
        );

        let mut usages = Vec::new();
        let mut other_events = 0;
        parse_anthropic_sse(stream.as_bytes(), &mut |event| match event {
            TurnStreamEvent::Usage(u) => usages.push(u),
            _ => other_events += 1,
        })
        .unwrap();

        assert_eq!(usages.len(), 1, "expected exactly one Usage event");
        let u = &usages[0];
        assert_eq!(u.input_tokens, 1234);
        assert_eq!(u.output_tokens, 42);
        assert_eq!(u.cache_read_tokens, 900);
        assert_eq!(u.cache_creation_tokens, 50);
        assert!(other_events > 0, "expected at least one TextDelta event");
    }

    /// Streams that omit usage entirely (legacy upstream, mocked
    /// fixtures, etc.) must NOT synthesize a zero-token Usage event —
    /// that would falsely tick the goal budget by 0 tokens (harmless)
    /// but also make the OTel cache-hit-ratio path divide by zero.
    #[test]
    fn no_usage_event_when_stream_omits_usage_block() {
        let stream = concat!(
            "event:message_start\n",
            "data:{\"type\":\"message_start\",\"message\":{\"id\":\"msg_n\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
            "event:content_block_start\n",
            "data:{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event:content_block_delta\n",
            "data:{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
            "event:content_block_stop\n",
            "data:{\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event:message_stop\n",
            "data:{\"type\":\"message_stop\"}\n\n",
        );

        let mut saw_usage = false;
        parse_anthropic_sse(stream.as_bytes(), &mut |event| {
            if matches!(event, TurnStreamEvent::Usage(_)) {
                saw_usage = true;
            }
        })
        .unwrap();
        assert!(!saw_usage, "must not emit Usage when the stream omits it");
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
