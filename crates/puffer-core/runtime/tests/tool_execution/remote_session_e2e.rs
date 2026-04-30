use super::remote_runner;
use crate::runtime::{
    execute_user_prompt, execute_user_prompt_streaming_with_permissions, PermissionPromptAction,
    ToolOutputStream, TurnStreamEvent,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

fn request_json_body(raw_request: &str) -> Value {
    let body = raw_request.split("\r\n\r\n").nth(1).unwrap_or_default();
    serde_json::from_str(body).expect("parse request body as json")
}

fn streamed_tool_call_response() -> String {
    concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_remote_round_1\"}}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_remote_bash\",\"status\":\"completed\",\"call_id\":\"call_remote_bash\",\"name\":\"Bash\",\"arguments\":\"{\\\"command\\\":\\\"printf remote-stdout; printf remote-stderr >&2\\\"}\"}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_remote_round_1\",\"status\":\"completed\"}}\n\n"
    )
    .to_string()
}

fn streamed_final_response() -> String {
    concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_remote_round_2\"}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"remote \"}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"roundtrip ok\"}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"remote roundtrip ok\"}]}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_remote_round_2\",\"status\":\"completed\"}}\n\n"
    )
    .to_string()
}

#[test]
fn execute_user_prompt_round_trips_remote_runner_results_back_to_openai_provider() {
    let runner = remote_runner::spawn_remote_tool_runner();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);
    let server = thread::spawn(move || {
        for index in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 65_536];
            let bytes = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
            request_log.lock().unwrap().push(request);
            let body = match index {
                0 => json!({
                    "id": "resp_remote_round_1",
                    "output": [{
                        "type": "function_call",
                        "id": "fc_remote_bash",
                        "status": "completed",
                        "call_id": "call_remote_bash",
                        "name": "Bash",
                        "arguments": "{\"command\":\"printf remote-stdout; printf remote-stderr >&2\"}"
                    }]
                })
                .to_string(),
                _ => json!({
                    "id": "resp_remote_round_2",
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": "remote roundtrip ok"
                        }]
                    }]
                })
                .to_string(),
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut bash = super::super::loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash.value.approval_policy = Some("never".to_string());
    bash.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![bash],
        ..LoadedResources::default()
    };
    let mut providers = ProviderRegistry::new();
    providers.register(super::super::openai_provider(format!(
        "http://{address}/api/codex"
    )));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    let mut state = super::temp_state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    super::configure_remote_tool_runner(&mut state, &runner);

    let mut deltas = Vec::new();
    let mut on_delta = |delta| deltas.push(delta);
    let turn = super::super::super::tool_stream::with_tool_stream_handler(&mut on_delta, || {
        execute_user_prompt(
            &mut state,
            &resources,
            &providers,
            &mut auth_store,
            "run the remote bash tool",
        )
    })
    .unwrap();
    server.join().unwrap();

    assert_eq!(turn.assistant_text, "remote roundtrip ok");
    assert_eq!(turn.tool_invocations.len(), 1);
    assert!(turn.tool_invocations[0].success);
    assert_eq!(turn.tool_invocations[0].tool_id, "Bash");
    assert!(turn.tool_invocations[0].output.contains("remote-stdout"));
    assert!(turn.tool_invocations[0].output.contains("remote-stderr"));
    assert!(deltas.iter().any(|delta| {
        delta.call_id == "call_remote_bash"
            && delta.tool_id == "Bash"
            && delta.stream == ToolOutputStream::Stdout
            && delta.text.contains("remote-stdout")
    }));
    assert!(deltas.iter().any(|delta| {
        delta.call_id == "call_remote_bash"
            && delta.tool_id == "Bash"
            && delta.stream == ToolOutputStream::Stderr
            && delta.text.contains("remote-stderr")
    }));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0]
        .to_ascii_lowercase()
        .contains("post /api/codex/responses"));
    assert!(requests[1]
        .to_ascii_lowercase()
        .contains("post /api/codex/responses"));

    let second_body = request_json_body(&requests[1]);
    let second_input = second_body["input"]
        .as_array()
        .expect("openai continuation input should be an array");
    let tool_call = second_input
        .iter()
        .find(|item| {
            item.get("type") == Some(&json!("function_call"))
                && item.get("call_id") == Some(&json!("call_remote_bash"))
        })
        .expect("second provider round should replay the tool call");
    assert_eq!(tool_call["name"], json!("Bash"));
    assert_eq!(
        tool_call["arguments"],
        json!("{\"command\":\"printf remote-stdout; printf remote-stderr >&2\"}")
    );

    let tool_output = second_input
        .iter()
        .find(|item| {
            item.get("type") == Some(&json!("function_call_output"))
                && item.get("call_id") == Some(&json!("call_remote_bash"))
        })
        .expect("second provider round should include the remote tool result");
    let output = tool_output["output"]
        .as_str()
        .expect("tool output should serialize as text");
    assert!(output.contains("remote-stdout"));
    assert!(output.contains("remote-stderr"));
}

#[test]
fn execute_user_prompt_streaming_emits_remote_runner_events_and_continues_provider_round() {
    let runner = remote_runner::spawn_remote_tool_runner();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);
    let server = thread::spawn(move || {
        for index in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 65_536];
            let bytes = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
            request_log.lock().unwrap().push(request);
            let body = match index {
                0 => streamed_tool_call_response(),
                _ => streamed_final_response(),
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut bash = super::super::loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash.value.approval_policy = Some("never".to_string());
    bash.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![bash],
        ..LoadedResources::default()
    };
    let mut providers = ProviderRegistry::new();
    providers.register(super::super::openai_provider(format!(
        "http://{address}/api/codex"
    )));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    let mut state = super::temp_state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    super::configure_remote_tool_runner(&mut state, &runner);

    let mut text_deltas = Vec::new();
    let mut requested = Vec::new();
    let mut streamed_outputs = Vec::new();
    let mut completed = Vec::new();
    let turn = execute_user_prompt_streaming_with_permissions(
        &mut state,
        &resources,
        &providers,
        &mut auth_store,
        "run the remote bash tool",
        None,
        |event| match event {
            TurnStreamEvent::TextDelta(delta) => text_deltas.push(delta),
            TurnStreamEvent::ToolCallsRequested(calls) => requested.extend(calls),
            TurnStreamEvent::ToolOutputDelta(delta) => streamed_outputs.push(delta),
            TurnStreamEvent::ToolInvocations(invocations) => completed.extend(invocations),
            _ => {}
        },
        |_| PermissionPromptAction::AllowOnce,
    )
    .unwrap();
    server.join().unwrap();

    assert_eq!(turn.assistant_text, "remote roundtrip ok");
    assert_eq!(
        text_deltas,
        vec!["remote ".to_string(), "roundtrip ok".to_string()]
    );
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].call_id, "call_remote_bash");
    assert_eq!(requested[0].tool_id, "Bash");
    assert!(requested[0].input.contains("remote-stdout"));
    assert_eq!(completed.len(), 1);
    assert!(completed[0].success);
    assert!(completed[0].output.contains("remote-stdout"));
    assert!(completed[0].output.contains("remote-stderr"));
    assert!(streamed_outputs.iter().any(|delta| {
        delta.call_id == "call_remote_bash"
            && delta.tool_id == "Bash"
            && delta.stream == ToolOutputStream::Stdout
            && delta.text.contains("remote-stdout")
    }));
    assert!(streamed_outputs.iter().any(|delta| {
        delta.call_id == "call_remote_bash"
            && delta.tool_id == "Bash"
            && delta.stream == ToolOutputStream::Stderr
            && delta.text.contains("remote-stderr")
    }));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let second_body = request_json_body(&requests[1]);
    let second_input = second_body["input"]
        .as_array()
        .expect("openai continuation input should be an array");
    assert!(second_input.iter().any(|item| {
        item.get("type") == Some(&json!("function_call_output"))
            && item.get("call_id") == Some(&json!("call_remote_bash"))
            && item
                .get("output")
                .and_then(Value::as_str)
                .is_some_and(|output| output.contains("remote-stdout"))
    }));
}
