use super::*;
use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

const ANTHROPIC_TOOL_ROUNDS_BEFORE_FINAL: usize = 9;

fn session_for(cwd: &std::path::Path) -> SessionMetadata {
    SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        cwd: cwd.to_path_buf(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    }
}

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
                    let mut buffer = [0_u8; 32_768];
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

fn anthropic_tool_response(index: usize) -> String {
    json!({
        "id": format!("msg_{index}"),
        "type": "message",
        "role": "assistant",
        "content": [{
            "type": "tool_use",
            "id": format!("call_{index}"),
            "name": "read_file",
            "input": { "path": "fixture.txt" }
        }],
        "stop_reason": "tool_use"
    })
    .to_string()
}

fn anthropic_final_response() -> String {
    json!({
        "id": "msg_done",
        "type": "message",
        "role": "assistant",
        "content": [{
            "type": "text",
            "text": "done"
        }],
        "stop_reason": "end_turn"
    })
    .to_string()
}

fn anthropic_tool_sse(index: usize) -> String {
    format!(
        concat!(
            "event: message_start\n",
            "data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_{index}\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}}}\n\n",
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"call_{index}\",\"name\":\"read_file\"}}}}\n\n",
            "event: content_block_delta\n",
            "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{{\\\"path\\\":\\\"fixture.txt\\\"}}\"}}}}\n\n",
            "event: content_block_stop\n",
            "data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n",
            "event: message_delta\n",
            "data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}}}}\n\n",
            "event: message_stop\n",
            "data: {{\"type\":\"message_stop\"}}\n\n"
        ),
        index = index
    )
}

fn anthropic_final_sse() -> String {
    concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_done\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"done\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    )
    .to_string()
}

#[test]
fn execute_user_prompt_allows_anthropic_tool_iterations_beyond_eight() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("fixture.txt"), "fixture").unwrap();
    let total_requests = ANTHROPIC_TOOL_ROUNDS_BEFORE_FINAL + 1;
    let (base_url, requests, server) = spawn_server("application/json", total_requests, |index| {
        if index < ANTHROPIC_TOOL_ROUNDS_BEFORE_FINAL {
            anthropic_tool_response(index)
        } else {
            anthropic_final_response()
        }
    });

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
    let resources = LoadedResources {
        tools: vec![loaded_tool("read_file", "Read a file", "read_file")],
        ..LoadedResources::default()
    };

    let turn = execute_user_prompt(
        &mut state,
        &resources,
        &registry,
        &mut AuthStore::default(),
        "loop",
    )
    .unwrap();

    assert_eq!(turn.assistant_text, "done");
    assert_eq!(
        turn.tool_invocations.len(),
        ANTHROPIC_TOOL_ROUNDS_BEFORE_FINAL
    );
    server.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), total_requests);
}

#[test]
fn execute_user_prompt_streaming_allows_anthropic_tool_iterations_beyond_eight() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("fixture.txt"), "fixture").unwrap();
    let total_requests = ANTHROPIC_TOOL_ROUNDS_BEFORE_FINAL + 1;
    let (base_url, requests, server) = spawn_server("text/event-stream", total_requests, |index| {
        if index < ANTHROPIC_TOOL_ROUNDS_BEFORE_FINAL {
            anthropic_tool_sse(index)
        } else {
            anthropic_final_sse()
        }
    });

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
    let resources = LoadedResources {
        tools: vec![loaded_tool("read_file", "Read a file", "read_file")],
        ..LoadedResources::default()
    };
    let mut deltas = Vec::new();

    let turn = execute_user_prompt_streaming(
        &mut state,
        &resources,
        &registry,
        &mut AuthStore::default(),
        "loop",
        |event| {
            if let TurnStreamEvent::TextDelta(delta) = event {
                deltas.push(delta);
            }
        },
    )
    .unwrap();

    assert_eq!(deltas, vec!["done".to_string()]);
    assert_eq!(turn.assistant_text, "done");
    assert_eq!(
        turn.tool_invocations.len(),
        ANTHROPIC_TOOL_ROUNDS_BEFORE_FINAL
    );
    server.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), total_requests);
}
