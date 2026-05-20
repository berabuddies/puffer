use super::*;
use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

#[test]
fn execute_user_prompt_streaming_surfaces_anthropic_error_type() {
    let temp = tempfile::tempdir().unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() >= deadline {
                panic!("mock anthropic server: no client connected");
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    let mut buffer = [0_u8; 32_768];
                    let _ = stream.read(&mut buffer).unwrap();
                    let body = concat!(
                        "event: message_start\n",
                        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_x\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
                        "event: error\n",
                        "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Service is temporarily overloaded\"}}\n\n",
                    );
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                    return;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("listener accept failed: {error}"),
            }
        }
    });

    let mut descriptor = provider();
    descriptor.id = "local-anthropic".to_string();
    descriptor.base_url = format!("http://{address}");
    descriptor.auth_modes.clear();
    descriptor.models[0].provider = "local-anthropic".to_string();

    let mut registry = ProviderRegistry::new();
    registry.register(descriptor);
    let mut state = AppState::new(
        PufferConfig::default(),
        temp.path().to_path_buf(),
        SessionMetadata {
            id: Uuid::new_v4(),
            display_name: None,
            generated_title: None,
            cwd: temp.path().to_path_buf(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        },
    );
    state.current_provider = Some("local-anthropic".to_string());
    state.current_model = Some("local-anthropic/claude-sonnet-4-5".to_string());

    let resources = LoadedResources::default();
    let result = execute_user_prompt_streaming(
        &mut state,
        &resources,
        &registry,
        &mut AuthStore::default(),
        "ping",
        |_| {},
    );

    server.join().unwrap();

    let err = result.expect_err("mock server emits error event; turn must fail");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("[overloaded_error]"),
        "PR #91 contract: bail message must surface error.type. got: {msg}"
    );
    assert!(
        msg.contains("Service is temporarily overloaded"),
        "bail message must preserve human-readable text. got: {msg}"
    );
}
