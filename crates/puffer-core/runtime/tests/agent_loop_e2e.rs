//! End-to-end coverage for the `agent_loop` migration.
//!
//! Each test stands up an in-process `TcpListener` that pretends to
//! be the upstream provider, scripts a multi-turn exchange (turn 1
//! returns a tool call, turn 2 returns a final text answer), and
//! asserts on the resulting `TurnExecution` plus the emitted wire
//! requests. Together they verify:
//!
//! - the agent_loop driver runs through a tool round-trip and a final
//!   text turn for each provider session implementation,
//! - per-tool execution still flows through the real `ToolRegistry`
//!   (so reflection / permissions / hooks remain reachable),
//! - OpenAI Responses propagates `previous_response_id` on the second
//!   request (the `continuation_start` threading optimization),
//! - streaming providers actually surface `TextDelta` events.
//!
//! Anthropic is already exercised end-to-end by `iteration_behavior.rs`
//! (9-round tool loop), so we focus here on the freshly-migrated
//! OpenAI Responses + Chat Completions paths.

use super::*;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// RAII helper that restores an env var on drop. Use under
/// `refresh_env_lock()` so concurrent tests don't race the value.
struct EnvGuard {
    name: &'static str,
    prior: Option<String>,
}

impl EnvGuard {
    fn set(name: &'static str, value: &str) -> Self {
        let prior = std::env::var(name).ok();
        std::env::set_var(name, value);
        EnvGuard { name, prior }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prior.take() {
            Some(prior) => std::env::set_var(self.name, prior),
            None => std::env::remove_var(self.name),
        }
    }
}

fn session_for(cwd: &std::path::Path) -> SessionMetadata {
    SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        generated_title: None,
        cwd: cwd.to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    }
}
/// Scripted HTTP server: serves `expected_requests` mock responses,
/// recording each raw request body so tests can assert on wire shape.
fn spawn_server<F>(
    content_type: &'static str,
    expected_requests: usize,
    response_body: F,
) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>)
where
    F: Fn(usize) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut handled = 0_usize;
        while handled < expected_requests && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    let mut buffer = vec![0_u8; 65_536];
                    let bytes = stream.read(&mut buffer).unwrap();
                    let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
                    request_log.lock().unwrap().push(request);
                    let body = response_body(handled);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                    handled += 1;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("listener accept failed: {error}"),
            }
        }
    });
    (format!("http://{address}"), requests, server)
}

fn extract_request_body(raw: &str) -> &str {
    raw.split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("")
}

// ---------------------------------------------------------------------------
// OpenAI Responses — non-streaming, 2 turns (tool → text).
// ---------------------------------------------------------------------------

fn openai_responses_tool_turn() -> String {
    json!({
        "id": "resp_1",
        "output": [{
            "type": "function_call",
            "call_id": "call_1",
            "name": "read_file",
            "arguments": "{\"path\":\"fixture.txt\"}"
        }],
        "usage": {
            "input_tokens": 100,
            "output_tokens": 5,
            "input_tokens_details": { "cached_tokens": 0 }
        }
    })
    .to_string()
}

fn openai_responses_final_turn() -> String {
    json!({
        "id": "resp_2",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": "all set"
            }]
        }],
        "output_text": "all set",
        "usage": {
            "input_tokens": 110,
            "output_tokens": 3,
            "input_tokens_details": { "cached_tokens": 5 }
        }
    })
    .to_string()
}

#[test]
fn openai_responses_agent_loop_runs_tool_then_text() {
    // Note: threading (previous_response_id) is exercised by the
    // dedicated `openai_responses_threading_*` test below under an
    // env_lock. This test focuses on the loop's end-to-end shape:
    // tool round-trip, FunctionCallOutput append, final text turn.
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("fixture.txt"), "fixture-contents").unwrap();
    let (base_url, requests, server) = spawn_server("application/json", 2, |index| {
        if index == 0 {
            openai_responses_tool_turn()
        } else {
            openai_responses_final_turn()
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(base_url));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");

    let mut state = AppState::new(
        PufferConfig::default(),
        temp.path().to_path_buf(),
        session_for(temp.path()),
    );
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    state.session_allow_all = true;

    let resources = LoadedResources {
        tools: vec![loaded_tool("read_file", "Read a file", "read_file")],
        ..LoadedResources::default()
    };

    let turn = execute_user_prompt(
        &mut state,
        &resources,
        &registry,
        &mut auth_store,
        "please read fixture.txt",
    )
    .unwrap();

    server.join().unwrap();

    assert_eq!(turn.assistant_text, "all set");
    assert_eq!(turn.tool_invocations.len(), 1);
    assert_eq!(turn.tool_invocations[0].tool_id, "read_file");
    assert_eq!(turn.tool_invocations[0].call_id, "call_1");
    assert!(turn.tool_invocations[0].success);
    assert!(turn.tool_invocations[0].output.contains("fixture-contents"));

    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2, "expected exactly two upstream requests");

    // Turn 2 must include the tool call's FunctionCallOutput — this
    // verifies the loop appends FunctionCallOutput items after tool
    // execution and the session re-serializes them on the next request.
    let body2 = extract_request_body(&captured[1]);
    let body2_json: Value = serde_json::from_str(body2).unwrap_or(Value::Null);
    let input_arr = body2_json
        .get("input")
        .and_then(Value::as_array)
        .expect("input array on second request");
    let has_function_output = input_arr.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("function_call_output")
            && item.get("call_id").and_then(Value::as_str) == Some("call_1")
    });
    assert!(
        has_function_output,
        "second request input must contain function_call_output for call_1: {body2}"
    );
}

// ---------------------------------------------------------------------------
// OpenAI Responses — streaming SSE, 2 turns (tool → text).
// ---------------------------------------------------------------------------

fn openai_responses_tool_sse() -> String {
    concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
        "event: response.output_item.added\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read_file\",\"arguments\":\"\"}}\n\n",
        "event: response.function_call_arguments.delta\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"path\\\":\\\"fixture.txt\\\"}\"}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"fixture.txt\\\"}\"}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":100,\"output_tokens\":5,\"input_tokens_details\":{\"cached_tokens\":0}}}}\n\n"
    ).to_string()
}

fn openai_responses_final_sse() -> String {
    concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_2\"}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"all \"}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"set\"}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"all set\"}]}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_2\",\"status\":\"completed\",\"usage\":{\"input_tokens\":110,\"output_tokens\":3,\"input_tokens_details\":{\"cached_tokens\":5}}}}\n\n"
    ).to_string()
}

#[test]
fn openai_responses_streaming_agent_loop_runs_tool_then_text() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("fixture.txt"), "fixture-contents").unwrap();
    let (base_url, requests, server) = spawn_server("text/event-stream", 2, |index| {
        if index == 0 {
            openai_responses_tool_sse()
        } else {
            openai_responses_final_sse()
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(base_url));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");

    let mut state = AppState::new(
        PufferConfig::default(),
        temp.path().to_path_buf(),
        session_for(temp.path()),
    );
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    state.session_allow_all = true;

    let resources = LoadedResources {
        tools: vec![loaded_tool("read_file", "Read a file", "read_file")],
        ..LoadedResources::default()
    };

    let mut text_deltas = Vec::new();
    let mut tool_calls_seen = 0_usize;
    let mut tool_invocations_seen = 0_usize;

    let turn = execute_user_prompt_streaming(
        &mut state,
        &resources,
        &registry,
        &mut auth_store,
        "please read fixture.txt",
        |event| match event {
            TurnStreamEvent::TextDelta(delta) => text_deltas.push(delta),
            TurnStreamEvent::ToolCallsRequested(calls) => tool_calls_seen += calls.len(),
            TurnStreamEvent::ToolInvocations(invocations) => {
                tool_invocations_seen += invocations.len()
            }
            _ => {}
        },
    )
    .unwrap();

    server.join().unwrap();

    assert_eq!(turn.assistant_text, "all set");
    assert_eq!(turn.tool_invocations.len(), 1);
    assert_eq!(turn.tool_invocations[0].tool_id, "read_file");
    assert!(turn.tool_invocations[0].success);
    assert!(turn.tool_invocations[0].output.contains("fixture-contents"));
    assert_eq!(tool_invocations_seen, 1, "loop must emit ToolInvocations");
    assert_eq!(
        text_deltas,
        vec!["all ".to_string(), "set".to_string()],
        "streaming TextDelta events should mirror the SSE tokens"
    );
    // The SSE parser may surface tool_call_start events; the loop's
    // dedupe should NOT re-emit ToolCallsRequested for the same id.
    assert!(
        tool_calls_seen <= 1,
        "agent_loop must dedupe ToolCallsRequested when SSE already surfaced the call"
    );

    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    let body2 = extract_request_body(&captured[1]);
    let body2_json: Value = serde_json::from_str(body2).unwrap_or(Value::Null);
    let input_arr = body2_json
        .get("input")
        .and_then(Value::as_array)
        .expect("input array on second streaming request");
    let has_function_output = input_arr.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("function_call_output")
            && item.get("call_id").and_then(Value::as_str) == Some("call_1")
    });
    assert!(
        has_function_output,
        "streaming second request must contain function_call_output for call_1: {body2}"
    );
}

// ---------------------------------------------------------------------------
// OpenAI Responses — streaming SSE with reasoning summary deltas.
// Verifies that `response.reasoning_summary_text.delta` events round-trip
// from the SSE parser through the agent_loop session out to the
// `on_event` callback as `TurnStreamEvent::ThinkingDelta`. Locks in the
// hanbbq-relay reasoning visibility regression: the relay returns
// 80+ reasoning summary deltas per turn but the TUI was showing no
// thinking block, suggesting the chain was being broken somewhere.
// ---------------------------------------------------------------------------

fn openai_responses_reasoning_sse() -> String {
    concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_r\"}}\n\n",
        "event: response.output_item.added\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[]}}\n\n",
        "event: response.reasoning_summary_part.added\n",
        "data: {\"type\":\"response.reasoning_summary_part.added\",\"output_index\":0,\"summary_index\":0,\"part\":{\"type\":\"summary_text\",\"text\":\"\"}}\n\n",
        "event: response.reasoning_summary_text.delta\n",
        "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"Step 1.\",\"output_index\":0,\"summary_index\":0}\n\n",
        "event: response.reasoning_summary_text.delta\n",
        "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\" Step 2.\",\"output_index\":0,\"summary_index\":0}\n\n",
        "event: response.reasoning_summary_text.done\n",
        "data: {\"type\":\"response.reasoning_summary_text.done\",\"output_index\":0,\"summary_index\":0,\"text\":\"Step 1. Step 2.\"}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Step 1. Step 2.\"}]}}\n\n",
        "event: response.output_item.added\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"message\",\"id\":\"msg_1\",\"role\":\"assistant\"}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Yes\"}\n\n",
        "event: response.output_text.done\n",
        "data: {\"type\":\"response.output_text.done\",\"text\":\"Yes\"}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Yes\"}]}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_r\",\"status\":\"completed\",\"usage\":{\"input_tokens\":50,\"output_tokens\":10,\"input_tokens_details\":{\"cached_tokens\":0}}}}\n\n"
    ).to_string()
}

#[test]
fn openai_responses_streaming_emits_thinking_for_reasoning_summary() {
    let temp = tempfile::tempdir().unwrap();
    let (base_url, _requests, server) =
        spawn_server("text/event-stream", 1, |_| openai_responses_reasoning_sse());

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(base_url));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");

    let mut state = AppState::new(
        PufferConfig::default(),
        temp.path().to_path_buf(),
        session_for(temp.path()),
    );
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    state.session_allow_all = true;

    let resources = LoadedResources::default();

    let mut thinking_deltas: Vec<String> = Vec::new();
    let mut text_deltas: Vec<String> = Vec::new();
    let turn = execute_user_prompt_streaming(
        &mut state,
        &resources,
        &registry,
        &mut auth_store,
        "is 17 prime?",
        |event| match event {
            TurnStreamEvent::ThinkingDelta(d) => thinking_deltas.push(d),
            TurnStreamEvent::TextDelta(d) => text_deltas.push(d),
            _ => {}
        },
    )
    .unwrap();

    server.join().unwrap();

    assert_eq!(turn.assistant_text, "Yes");
    assert!(
        !thinking_deltas.is_empty(),
        "ThinkingDelta MUST fire for response.reasoning_summary_text.delta events; \
         got 0 deltas — the SSE→on_event chain is broken"
    );
    assert_eq!(
        thinking_deltas
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["Step 1.", " Step 2."],
        "ThinkingDelta payloads must mirror the SSE delta tokens 1:1"
    );
    assert_eq!(text_deltas, vec!["Yes".to_string()]);
}

// ---------------------------------------------------------------------------
// OpenAI Chat Completions — 2 turns (tool → text).
// ---------------------------------------------------------------------------

fn openai_completions_provider(base_url: String) -> ProviderDescriptor {
    let mut descriptor = openai_provider(base_url);
    descriptor.id = "openai-completions-test".to_string();
    descriptor.default_api = "openai-completions".to_string();
    for model in &mut descriptor.models {
        model.provider = "openai-completions-test".to_string();
        model.api = "openai-completions".to_string();
    }
    descriptor
}

fn openai_completions_tool_turn() -> String {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_chat_1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"fixture.txt\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    })
    .to_string()
}

fn openai_completions_final_turn() -> String {
    json!({
        "id": "chatcmpl-2",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "completions ok"
            },
            "finish_reason": "stop"
        }]
    })
    .to_string()
}

/// Reasoning-capable Chat Completions providers (Moonshot Kimi
/// `k2p5`, Deepseek, OpenRouter relays) return a `reasoning_content`
/// (or `reasoning`) field on the assistant message. Agent_loop must
/// surface this as a `TurnStreamEvent::ThinkingDelta` so the TUI's
/// thinking block stays populated.
#[test]
fn openai_completions_agent_loop_emits_thinking_for_reasoning_content() {
    let temp = tempfile::tempdir().unwrap();
    let (base_url, _requests, server) = spawn_server("application/json", 1, |_| {
        json!({
            "id": "chatcmpl-rsn",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "the visible answer",
                    "reasoning_content": "step 1, step 2, step 3"
                },
                "finish_reason": "stop"
            }]
        })
        .to_string()
    });
    let mut registry = ProviderRegistry::new();
    registry.register(openai_completions_provider(base_url));
    let mut auth = AuthStore::default();
    auth.set_api_key("openai-completions-test", "sk-x");
    let mut state = AppState::new(
        PufferConfig::default(),
        temp.path().to_path_buf(),
        session_for(temp.path()),
    );
    state.current_provider = Some("openai-completions-test".to_string());
    state.current_model = Some("openai-completions-test/gpt-5".to_string());
    state.session_allow_all = true;
    let resources = LoadedResources::default();

    let mut thinking_deltas = Vec::new();
    let mut text_deltas = Vec::new();
    let turn = execute_user_prompt_streaming(
        &mut state,
        &resources,
        &registry,
        &mut auth,
        "ask",
        |event| match event {
            TurnStreamEvent::ThinkingDelta(d) => thinking_deltas.push(d),
            TurnStreamEvent::TextDelta(d) => text_deltas.push(d),
            _ => {}
        },
    )
    .unwrap();
    server.join().unwrap();

    assert_eq!(turn.assistant_text, "the visible answer");
    assert_eq!(
        thinking_deltas,
        vec!["step 1, step 2, step 3".to_string()],
        "ThinkingDelta must fire with reasoning_content payload"
    );
    assert_eq!(
        text_deltas,
        vec!["the visible answer".to_string()],
        "TextDelta must fire with the visible content"
    );
}

#[test]
fn openai_completions_agent_loop_runs_tool_then_text() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("fixture.txt"), "fixture-contents").unwrap();
    let (base_url, requests, server) = spawn_server("application/json", 2, |index| {
        if index == 0 {
            openai_completions_tool_turn()
        } else {
            openai_completions_final_turn()
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_completions_provider(base_url));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai-completions-test", "sk-openai");

    let mut state = AppState::new(
        PufferConfig::default(),
        temp.path().to_path_buf(),
        session_for(temp.path()),
    );
    state.current_provider = Some("openai-completions-test".to_string());
    state.current_model = Some("openai-completions-test/gpt-5".to_string());
    state.session_allow_all = true;

    let resources = LoadedResources {
        tools: vec![loaded_tool("read_file", "Read a file", "read_file")],
        ..LoadedResources::default()
    };

    let turn = execute_user_prompt(
        &mut state,
        &resources,
        &registry,
        &mut auth_store,
        "please read fixture.txt",
    )
    .unwrap();

    server.join().unwrap();

    assert_eq!(turn.assistant_text, "completions ok");
    assert_eq!(turn.tool_invocations.len(), 1);
    assert_eq!(turn.tool_invocations[0].tool_id, "read_file");
    assert_eq!(turn.tool_invocations[0].call_id, "call_chat_1");
    assert!(turn.tool_invocations[0].success);
    assert!(turn.tool_invocations[0].output.contains("fixture-contents"));

    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2, "expected exactly two upstream requests");

    // Verify the second request includes the tool result message
    // (Chat Completions has no previous_response_id; the tool output
    // must round-trip via the messages array).
    let body2 = extract_request_body(&captured[1]);
    let body2_json: Value = serde_json::from_str(body2).unwrap_or(Value::Null);
    let messages = body2_json
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages array on second request");
    let has_tool_result = messages.iter().any(|m| {
        m.get("role").and_then(Value::as_str) == Some("tool")
            && m.get("tool_call_id").and_then(Value::as_str) == Some("call_chat_1")
    });
    assert!(
        has_tool_result,
        "second request must replay the tool result for call_chat_1: {body2}"
    );
}

// ---------------------------------------------------------------------------
// Cross-provider behavior: same prompt + same tool, both Anthropic and
// OpenAI Responses produce semantically equivalent end states (one tool
// invocation, same final text). Locks in the agent_loop's "the loop is
// the same regardless of provider" promise.
// ---------------------------------------------------------------------------

#[test]
fn anthropic_and_openai_agent_loop_share_outcome_for_same_tool_round() {
    let temp_anthropic = tempfile::tempdir().unwrap();
    let temp_openai = tempfile::tempdir().unwrap();
    std::fs::write(temp_anthropic.path().join("fixture.txt"), "shared-text").unwrap();
    std::fs::write(temp_openai.path().join("fixture.txt"), "shared-text").unwrap();

    // Anthropic: 2-turn tool → text.
    let (a_url, _a_req, a_server) = spawn_server("application/json", 2, |index| {
        if index == 0 {
            json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "call_anthropic",
                    "name": "read_file",
                    "input": { "path": "fixture.txt" }
                }],
                "stop_reason": "tool_use"
            })
            .to_string()
        } else {
            json!({
                "id": "msg_2",
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "text", "text": "shared outcome" }],
                "stop_reason": "end_turn"
            })
            .to_string()
        }
    });

    // OpenAI Responses: 2-turn tool → text.
    let (o_url, _o_req, o_server) = spawn_server("application/json", 2, |index| {
        if index == 0 {
            openai_responses_tool_turn()
        } else {
            json!({
                "id": "resp_2",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "shared outcome" }]
                }],
                "output_text": "shared outcome",
                "usage": {
                    "input_tokens": 110,
                    "output_tokens": 3,
                    "input_tokens_details": { "cached_tokens": 5 }
                }
            })
            .to_string()
        }
    });

    let resources = LoadedResources {
        tools: vec![loaded_tool("read_file", "Read a file", "read_file")],
        ..LoadedResources::default()
    };

    // Anthropic side.
    let mut a_descriptor = provider();
    a_descriptor.id = "local-anthropic".to_string();
    a_descriptor.base_url = a_url;
    a_descriptor.auth_modes.clear();
    a_descriptor.models[0].provider = "local-anthropic".to_string();
    let mut a_registry = ProviderRegistry::new();
    a_registry.register(a_descriptor);
    let mut a_state = AppState::new(
        PufferConfig::default(),
        temp_anthropic.path().to_path_buf(),
        session_for(temp_anthropic.path()),
    );
    a_state.current_provider = Some("local-anthropic".to_string());
    a_state.current_model = Some("local-anthropic/claude-sonnet-4-5".to_string());
    a_state.session_allow_all = true;
    let a_turn = execute_user_prompt(
        &mut a_state,
        &resources,
        &a_registry,
        &mut AuthStore::default(),
        "please read fixture.txt",
    )
    .unwrap();
    a_server.join().unwrap();

    // OpenAI side.
    let mut o_registry = ProviderRegistry::new();
    o_registry.register(openai_provider(o_url));
    let mut o_auth = AuthStore::default();
    o_auth.set_api_key("openai", "sk-openai");
    let mut o_state = AppState::new(
        PufferConfig::default(),
        temp_openai.path().to_path_buf(),
        session_for(temp_openai.path()),
    );
    o_state.current_provider = Some("openai".to_string());
    o_state.current_model = Some("openai/gpt-5".to_string());
    o_state.session_allow_all = true;
    let o_turn = execute_user_prompt(
        &mut o_state,
        &resources,
        &o_registry,
        &mut o_auth,
        "please read fixture.txt",
    )
    .unwrap();
    o_server.join().unwrap();

    assert_eq!(a_turn.assistant_text, o_turn.assistant_text);
    assert_eq!(a_turn.tool_invocations.len(), o_turn.tool_invocations.len());
    assert_eq!(
        a_turn.tool_invocations[0].tool_id,
        o_turn.tool_invocations[0].tool_id
    );
    assert_eq!(a_turn.tool_invocations[0].success, true);
    assert_eq!(o_turn.tool_invocations[0].success, true);
    assert!(a_turn.tool_invocations[0].output.contains("shared-text"));
    assert!(o_turn.tool_invocations[0].output.contains("shared-text"));
}

// ---------------------------------------------------------------------------
// OpenAI Responses threading: with `previous_response_id` enabled, turn 2
// must (a) carry `previous_response_id: "resp_1"`, and (b) send only the
// continuation slice (FunctionCall + FunctionCallOutput) — NOT the full
// transcript. Held under env_lock so concurrent tests' threading-disable
// flags don't short-circuit the check inside `setup_responses_session`.
// ---------------------------------------------------------------------------

#[test]
fn openai_responses_agent_loop_threads_previous_response_id() {
    let _env_lock = refresh_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    // Clear the disable flags first; setup_responses_session checks
    // DISABLE_* before ENABLE_* so a stale "1" would trump our enable.
    let _disable1 = EnvGuard::set("PUFFER_OPENAI_DISABLE_RESPONSE_THREADING", "");
    let _disable2 = EnvGuard::set("PUFFER_OPENAI_DISABLE_PREVIOUS_RESPONSE_ID", "");
    let _enable = EnvGuard::set("PUFFER_OPENAI_ENABLE_CUSTOM_RESPONSE_THREADING", "1");

    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("fixture.txt"), "fixture-contents").unwrap();
    let (base_url, requests, server) = spawn_server("application/json", 2, |index| {
        if index == 0 {
            openai_responses_tool_turn()
        } else {
            openai_responses_final_turn()
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(base_url));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");

    let mut state = AppState::new(
        PufferConfig::default(),
        temp.path().to_path_buf(),
        session_for(temp.path()),
    );
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    state.session_allow_all = true;

    let resources = LoadedResources {
        tools: vec![loaded_tool("read_file", "Read a file", "read_file")],
        ..LoadedResources::default()
    };

    let turn = execute_user_prompt(
        &mut state,
        &resources,
        &registry,
        &mut auth_store,
        "please read fixture.txt",
    )
    .unwrap();
    server.join().unwrap();

    assert_eq!(turn.assistant_text, "all set");
    assert_eq!(turn.tool_invocations.len(), 1);

    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2);

    let body2 = extract_request_body(&captured[1]);
    let body2_json: Value = serde_json::from_str(body2).unwrap_or(Value::Null);

    // (a) previous_response_id wired through.
    assert_eq!(
        body2_json
            .get("previous_response_id")
            .and_then(Value::as_str),
        Some("resp_1"),
        "second request should carry previous_response_id from first response: {body2}"
    );

    // (b) Continuation slice only — NOT the full transcript.
    let input_arr = body2_json
        .get("input")
        .and_then(Value::as_array)
        .expect("input array on second request");
    let types: Vec<&str> = input_arr
        .iter()
        .filter_map(|item| item.get("type").and_then(Value::as_str))
        .collect();
    assert_eq!(
        types,
        vec!["function_call", "function_call_output"],
        "with threading enabled, turn 2 must send only [FunctionCall, FunctionCallOutput], got: {types:?}"
    );
    let call_id_match = input_arr
        .iter()
        .all(|item| item.get("call_id").and_then(Value::as_str) == Some("call_1"));
    assert!(
        call_id_match,
        "all continuation items must reference call_1: {body2}"
    );
}

// ---------------------------------------------------------------------------
// Anthropic multi-turn thinking round-trip — locks in the
// `kimi-coding/k2p5` regression. Turn 1 returns
// `[thinking{signature}, tool_use]`; Turn 2's wire body must replay the
// thinking block (with signature) BEFORE the tool_use block within the
// assistant message. Without this, providers like Kimi reject with
// "reasoning_content is missing in assistant tool call message".
// ---------------------------------------------------------------------------

#[test]
fn anthropic_agent_loop_replays_thinking_signature_on_next_turn() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("fixture.txt"), "shared-text").unwrap();

    let (base_url, requests, server) = spawn_server("application/json", 2, |index| {
        if index == 0 {
            // Turn 1: upstream produced a thinking block (with
            // signature) followed by a tool_use block — exactly the
            // shape Anthropic / Kimi return when extended thinking is
            // enabled and the model decided to call a tool.
            json!({
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [
                    {
                        "type": "thinking",
                        "thinking": "I should read fixture.txt to answer.",
                        "signature": "sig-multi-turn-test"
                    },
                    {
                        "type": "tool_use",
                        "id": "call_kimi",
                        "name": "read_file",
                        "input": { "path": "fixture.txt" }
                    }
                ],
                "stop_reason": "tool_use"
            })
            .to_string()
        } else {
            json!({
                "id": "msg_2",
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "text", "text": "shared outcome" }],
                "stop_reason": "end_turn"
            })
            .to_string()
        }
    });

    let resources = LoadedResources {
        tools: vec![loaded_tool("read_file", "Read a file", "read_file")],
        ..LoadedResources::default()
    };

    let mut descriptor = provider();
    descriptor.id = "local-anthropic".to_string();
    descriptor.base_url = base_url;
    descriptor.auth_modes.clear();
    descriptor.models[0].provider = "local-anthropic".to_string();
    let mut registry = ProviderRegistry::new();
    registry.register(descriptor);

    let mut state = AppState::new(
        PufferConfig::default(),
        temp.path().to_path_buf(),
        session_for(temp.path()),
    );
    state.current_provider = Some("local-anthropic".to_string());
    state.current_model = Some("local-anthropic/claude-sonnet-4-5".to_string());
    state.session_allow_all = true;

    let turn = execute_user_prompt(
        &mut state,
        &resources,
        &registry,
        &mut AuthStore::default(),
        "please read fixture.txt",
    )
    .unwrap();
    server.join().unwrap();

    assert_eq!(turn.assistant_text, "shared outcome");
    assert_eq!(turn.tool_invocations.len(), 1);
    assert_eq!(turn.tool_invocations[0].call_id, "call_kimi");
    assert!(turn.tool_invocations[0].output.contains("shared-text"));

    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2, "expected exactly two upstream requests");

    let body2 = extract_request_body(&captured[1]);
    let body2_json: Value = serde_json::from_str(body2).unwrap_or(Value::Null);
    let messages = body2_json
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages array on second request");

    // Find the assistant message that wraps the tool_use replay.
    let assistant_msg = messages
        .iter()
        .find(|m| {
            m.get("role").and_then(Value::as_str) == Some("assistant")
                && m.get("content")
                    .and_then(Value::as_array)
                    .map(|blocks| {
                        blocks
                            .iter()
                            .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                    })
                    .unwrap_or(false)
        })
        .expect("assistant message containing tool_use must be present in turn 2 wire body");

    let blocks = assistant_msg["content"]
        .as_array()
        .expect("assistant message content must be a block array");

    let kinds: Vec<&str> = blocks
        .iter()
        .filter_map(|b| b.get("type").and_then(Value::as_str))
        .collect();
    assert!(
        kinds.first() == Some(&"thinking"),
        "first content block must be `thinking` (Anthropic ordering rule), got: {kinds:?}\nbody: {body2}"
    );
    assert!(
        kinds.contains(&"tool_use"),
        "tool_use must be present in the assistant message, got: {kinds:?}"
    );
    let thinking_block = &blocks[0];
    assert_eq!(
        thinking_block["thinking"],
        "I should read fixture.txt to answer."
    );
    assert_eq!(thinking_block["signature"], "sig-multi-turn-test");

    // Tool-use block must keep its original call_id so the upstream's
    // own pairing logic still validates.
    let tool_use_block = blocks
        .iter()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
        .expect("tool_use block expected");
    assert_eq!(tool_use_block["id"], "call_kimi");
    assert_eq!(tool_use_block["name"], "read_file");
}

// ---------------------------------------------------------------------------
// Cancellation: when a `CancelToken` is passed via the public API and
// flipped before the loop runs the next iteration, the loop returns
// `Err("cancelled")` instead of completing the second turn.
// Pi-mono parity: `agent-loop.ts` checks `signal` between turns.
// ---------------------------------------------------------------------------

#[test]
fn cancel_token_aborts_agent_loop_between_turns() {
    use crate::runtime::CancelToken;

    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("fixture.txt"), "fixture-contents").unwrap();

    // The mock server is told to expect 2 requests but the cancellation
    // mid-flight should keep the second one from happening. We pre-flip
    // the token AFTER the first response, so the loop's "before turn 2"
    // boundary check trips. Use a request counter to flip the token in
    // the response builder thread.
    let cancel = CancelToken::new();
    let cancel_for_server = cancel.clone();
    let (base_url, requests, server) = spawn_server("application/json", 2, move |index| {
        if index == 0 {
            // Trip cancel right after responding to turn 1, so the
            // loop's "before next provider call" check fires.
            let body = openai_responses_tool_turn();
            cancel_for_server.cancel();
            body
        } else {
            // We never expect to reach this — the second mock response
            // is here just so the server thread doesn't deadlock when
            // it accepts a stray retry.
            openai_responses_final_turn()
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(base_url));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");

    let mut state = AppState::new(
        PufferConfig::default(),
        temp.path().to_path_buf(),
        session_for(temp.path()),
    );
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    state.session_allow_all = true;

    let resources = LoadedResources {
        tools: vec![loaded_tool("read_file", "Read a file", "read_file")],
        ..LoadedResources::default()
    };

    let result = crate::runtime::execute_user_prompt_streaming_with_cancel(
        &mut state,
        &resources,
        &registry,
        &mut auth_store,
        "please read fixture.txt",
        &cancel,
        |_| {},
    );

    // The server thread expects 2 connections — once we cancel, we
    // never make the 2nd. Drop the listener bound state by joining
    // (it'll exit when its accept loop times out).
    drop(server);

    let err = result.expect_err("cancel must abort the loop");
    let msg = err.to_string();
    assert!(
        msg.contains("cancelled"),
        "expected 'cancelled' in error, got: {msg}"
    );

    // Exactly one upstream request was made (turn 1) — turn 2 was
    // skipped by the cancel boundary check.
    let captured = requests.lock().unwrap();
    assert_eq!(
        captured.len(),
        1,
        "cancel should stop after turn 1; got {} requests",
        captured.len()
    );
}
