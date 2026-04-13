use super::*;
use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

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

fn spawn_json_server<F>(
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
        while handled < crate::runtime::tool_loop::MAX_TOOL_ITERATIONS_PER_TURN
            && Instant::now() < deadline
        {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0_u8; 32_768];
                    let bytes = stream.read(&mut buffer).unwrap();
                    let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
                    request_log.lock().unwrap().push(request);
                    let body = response_body(handled);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
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

#[test]
fn execute_user_prompt_limits_anthropic_tool_iterations() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("fixture.txt"), "fixture").unwrap();
    let (base_url, requests, server) = spawn_json_server(|index| {
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

    let error = execute_user_prompt(
        &mut state,
        &resources,
        &registry,
        &mut AuthStore::default(),
        "loop",
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "anthropic tool loop exceeded iteration limit"
    );
    server.join().unwrap();
    assert_eq!(
        requests.lock().unwrap().len(),
        crate::runtime::tool_loop::MAX_TOOL_ITERATIONS_PER_TURN
    );
}

#[test]
fn execute_user_prompt_limits_openai_tool_iterations() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("fixture.txt"), "fixture").unwrap();
    let (base_url, requests, server) = spawn_json_server(|index| {
        json!({
            "id": format!("resp_{index}"),
            "output": [{
                "type": "function_call",
                "call_id": format!("call_{index}"),
                "name": "read_file",
                "arguments": { "path": "fixture.txt" }
            }]
        })
        .to_string()
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
    let resources = LoadedResources {
        tools: vec![loaded_tool("read_file", "Read a file", "read_file")],
        ..LoadedResources::default()
    };

    let error = execute_user_prompt(&mut state, &resources, &registry, &mut auth_store, "loop")
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "openai tool loop exceeded iteration limit"
    );
    server.join().unwrap();
    assert_eq!(
        requests.lock().unwrap().len(),
        crate::runtime::tool_loop::MAX_TOOL_ITERATIONS_PER_TURN
    );
}
