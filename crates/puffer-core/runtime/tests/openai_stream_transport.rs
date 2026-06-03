use super::*;
use tungstenite::Message;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::test_locks::env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn reset_openai_websocket_fallbacks() {
    crate::runtime::openai::reset_openai_websocket_http_fallbacks();
}

#[test]
fn execute_user_prompt_streaming_parses_headerless_sse() {
    let _guard = env_lock();
    reset_openai_websocket_fallbacks();
    std::env::remove_var("PUFFER_OPENAI_WEBSOCKET");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 8192];
        let bytes = stream.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
        request_log.lock().unwrap().push(request);

        let body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"headerless \"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"headerless ok\"}]}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n\n"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(format!("http://{address}")));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    let mut state = state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let mut deltas = Vec::new();
    let turn = execute_user_prompt_streaming(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "hello",
        |event| {
            if let TurnStreamEvent::TextDelta(delta) = event {
                deltas.push(delta);
            }
        },
    )
    .unwrap();
    server.join().unwrap();

    assert_eq!(turn.assistant_text, "headerless ok");
    assert_eq!(deltas, vec!["headerless ".to_string(), "ok".to_string()]);

    let requests = requests.lock().unwrap();
    let request = requests[0].to_ascii_lowercase();
    assert!(request.contains("accept: text/event-stream"));
    assert!(request.contains("authorization: bearer sk-openai"));
}

#[test]
fn execute_user_prompt_streaming_retries_truncated_responses_sse() {
    let _guard = env_lock();
    reset_openai_websocket_fallbacks();
    std::env::remove_var("PUFFER_OPENAI_WEBSOCKET");
    std::env::set_var("PUFFER_OPENAI_STREAM_MAX_ATTEMPTS", "2");
    std::env::set_var("PUFFER_OPENAI_STREAM_RETRY_DELAY_MS", "0");
    std::env::set_var("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS", "1");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);

    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut accepted = 0;
        while accepted < 2 && std::time::Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("accept failed: {error}"),
            };
            accepted += 1;
            stream.set_nonblocking(false).unwrap();
            let mut buffer = [0_u8; 8192];
            let bytes = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
            request_log.lock().unwrap().push(request);

            let body = if accepted == 1 {
                concat!(
                    "event: response.created\n",
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_truncated\"}}\n\n",
                    "event: response.output_text.delta\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n"
                )
            } else {
                concat!(
                    "event: response.created\n",
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_ok\"}}\n\n",
                    "event: response.output_text.delta\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"pong\"}\n\n",
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_ok\",\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n"
                )
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(format!("http://{address}")));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    let mut state = state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let mut events = Vec::new();
    let result = execute_user_prompt_streaming(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "hello",
        |event| events.push(event),
    );
    server.join().unwrap();
    std::env::remove_var("PUFFER_OPENAI_STREAM_MAX_ATTEMPTS");
    std::env::remove_var("PUFFER_OPENAI_STREAM_RETRY_DELAY_MS");
    std::env::remove_var("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS");

    let turn = result.unwrap();
    assert_eq!(turn.assistant_text, "pong");
    assert_eq!(requests.lock().unwrap().len(), 2);
    assert!(events.iter().any(|event| matches!(
        event,
        TurnStreamEvent::RetryAttempt {
            attempt: 1,
            max_attempts: 2,
            ..
        }
    )));
}

#[test]
fn execute_user_prompt_streaming_retries_incomplete_responses_sse() {
    let _guard = env_lock();
    reset_openai_websocket_fallbacks();
    std::env::remove_var("PUFFER_OPENAI_WEBSOCKET");
    std::env::set_var("PUFFER_OPENAI_STREAM_MAX_ATTEMPTS", "2");
    std::env::set_var("PUFFER_OPENAI_STREAM_RETRY_DELAY_MS", "0");
    std::env::set_var("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS", "1");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);

    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut accepted = 0;
        while accepted < 2 && std::time::Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("accept failed: {error}"),
            };
            accepted += 1;
            stream.set_nonblocking(false).unwrap();
            let mut buffer = [0_u8; 8192];
            let bytes = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
            request_log.lock().unwrap().push(request);

            let body = if accepted == 1 {
                concat!(
                    "event: response.created\n",
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_incomplete\"}}\n\n",
                    "event: response.output_text.delta\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
                    "event: response.incomplete\n",
                    "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_incomplete\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"content_filter\"}}}\n\n"
                )
            } else {
                concat!(
                    "event: response.created\n",
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_ok\"}}\n\n",
                    "event: response.output_text.delta\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"pong\"}\n\n",
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_ok\",\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n"
                )
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(format!("http://{address}")));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    let mut state = state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let mut events = Vec::new();
    let result = execute_user_prompt_streaming(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "hello",
        |event| events.push(event),
    );
    server.join().unwrap();
    std::env::remove_var("PUFFER_OPENAI_STREAM_MAX_ATTEMPTS");
    std::env::remove_var("PUFFER_OPENAI_STREAM_RETRY_DELAY_MS");
    std::env::remove_var("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS");

    let turn = result.unwrap();
    assert_eq!(turn.assistant_text, "pong");
    assert_eq!(requests.lock().unwrap().len(), 2);
    assert!(events.iter().any(|event| matches!(
        event,
        TurnStreamEvent::RetryAttempt {
            attempt: 1,
            max_attempts: 2,
            error,
        } if error.contains("Incomplete response returned, reason: content_filter")
    )));
}

#[test]
fn execute_user_prompt_rejects_non_streaming_incomplete_response() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 8192];
        let bytes = stream.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
        request_log.lock().unwrap().push(request);

        let body = serde_json::json!({
            "id": "resp_incomplete",
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"},
            "output": []
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(format!("http://{address}")));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    let mut state = state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let error = execute_user_prompt(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "hello",
    )
    .unwrap_err()
    .to_string();
    server.join().unwrap();

    assert!(error.contains("Incomplete response returned, reason: max_output_tokens"));
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[test]
fn execute_user_prompt_streaming_retries_incomplete_after_websocket_fallback() {
    let _guard = env_lock();
    reset_openai_websocket_fallbacks();
    std::env::set_var("PUFFER_OPENAI_WEBSOCKET", "1");
    std::env::set_var("PUFFER_OPENAI_STREAM_MAX_ATTEMPTS", "2");
    std::env::set_var("PUFFER_OPENAI_STREAM_RETRY_DELAY_MS", "0");
    std::env::set_var("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS", "1");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);

    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut accepted = 0;
        while accepted < 4 && std::time::Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("accept failed: {error}"),
            };
            accepted += 1;
            stream.set_nonblocking(false).unwrap();
            let mut buffer = [0_u8; 8192];
            let bytes = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
            request_log.lock().unwrap().push(request);

            if accepted == 1 {
                stream
                    .write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\n\r\n")
                    .unwrap();
                continue;
            }

            let body = if accepted == 2 {
                concat!(
                    "event: response.created\n",
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_incomplete\"}}\n\n",
                    "event: response.output_text.delta\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
                    "event: response.incomplete\n",
                    "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_incomplete\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"content_filter\"}}}\n\n"
                )
            } else {
                concat!(
                    "event: response.created\n",
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_ok\"}}\n\n",
                    "event: response.output_text.delta\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"pong\"}\n\n",
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_ok\",\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n"
                )
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(format!("http://{address}")));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    let mut state = state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let mut events = Vec::new();
    let result = execute_user_prompt_streaming(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "hello",
        |event| events.push(event),
    );
    let turn = result.unwrap();
    let second_turn = execute_user_prompt_streaming(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "again",
        |_| {},
    )
    .unwrap();
    server.join().unwrap();
    std::env::remove_var("PUFFER_OPENAI_WEBSOCKET");
    std::env::remove_var("PUFFER_OPENAI_STREAM_MAX_ATTEMPTS");
    std::env::remove_var("PUFFER_OPENAI_STREAM_RETRY_DELAY_MS");
    std::env::remove_var("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS");

    let requests = requests.lock().unwrap();
    assert_eq!(turn.assistant_text, "pong");
    assert_eq!(second_turn.assistant_text, "pong");
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("GET "));
    assert!(requests[1].starts_with("POST "));
    assert!(requests[2].starts_with("POST "));
    assert!(requests[3].starts_with("POST "));
    assert!(events.iter().any(|event| matches!(
        event,
        TurnStreamEvent::RetryAttempt {
            attempt: 1,
            max_attempts: 2,
            error,
        } if error.contains("Incomplete response returned, reason: content_filter")
    )));
}

#[test]
fn execute_user_prompt_streaming_retries_incomplete_over_websocket() {
    let _guard = env_lock();
    reset_openai_websocket_fallbacks();
    std::env::set_var("PUFFER_OPENAI_WEBSOCKET", "1");
    std::env::set_var("PUFFER_OPENAI_WS_MAX_ATTEMPTS", "2");
    std::env::set_var("PUFFER_OPENAI_WS_RETRY_DELAY_MS", "0");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);

    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut accepted = 0;
        while accepted < 2 && std::time::Instant::now() < deadline {
            let (stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("accept failed: {error}"),
            };
            accepted += 1;
            stream.set_nonblocking(false).unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            let request = socket.read().unwrap().into_text().unwrap().to_string();
            request_log.lock().unwrap().push(request);

            if accepted == 1 {
                for event in [
                    serde_json::json!({
                        "type": "response.created",
                        "response": {"id": "resp_incomplete"}
                    }),
                    serde_json::json!({
                        "type": "response.output_text.delta",
                        "delta": "partial"
                    }),
                    serde_json::json!({
                        "type": "response.incomplete",
                        "response": {
                            "id": "resp_incomplete",
                            "status": "incomplete",
                            "incomplete_details": {"reason": "content_filter"}
                        }
                    }),
                ] {
                    socket
                        .send(Message::Text(event.to_string().into()))
                        .unwrap();
                }
                continue;
            }

            for event in [
                serde_json::json!({
                    "type": "response.created",
                    "response": {"id": "resp_ok"}
                }),
                serde_json::json!({
                    "type": "response.output_text.delta",
                    "delta": "pong"
                }),
                serde_json::json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_ok",
                        "status": "completed",
                        "usage": {"input_tokens": 1, "output_tokens": 1}
                    }
                }),
            ] {
                socket
                    .send(Message::Text(event.to_string().into()))
                    .unwrap();
            }
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(format!("http://{address}")));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    let mut state = state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let mut events = Vec::new();
    let turn = execute_user_prompt_streaming(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "hello",
        |event| events.push(event),
    )
    .unwrap();
    server.join().unwrap();
    std::env::remove_var("PUFFER_OPENAI_WEBSOCKET");
    std::env::remove_var("PUFFER_OPENAI_WS_MAX_ATTEMPTS");
    std::env::remove_var("PUFFER_OPENAI_WS_RETRY_DELAY_MS");

    let requests = requests.lock().unwrap();
    assert_eq!(turn.assistant_text, "pong");
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|request| request.contains("\"type\":\"response.create\"")));
    assert!(events.iter().any(|event| matches!(
        event,
        TurnStreamEvent::RetryAttempt {
            attempt: 1,
            max_attempts: 2,
            error,
        } if error.contains("Incomplete response returned, reason: content_filter")
    )));
}

#[test]
fn execute_user_prompt_streaming_does_not_latch_http_fallback_for_ws_incomplete() {
    let _guard = env_lock();
    reset_openai_websocket_fallbacks();
    std::env::set_var("PUFFER_OPENAI_WEBSOCKET", "1");
    std::env::set_var("PUFFER_OPENAI_WS_MAX_ATTEMPTS", "1");
    std::env::set_var("PUFFER_OPENAI_STREAM_RETRY_DELAY_MS", "0");
    std::env::set_var("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS", "1");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);

    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut accepted = 0;
        while accepted < 3 && std::time::Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("accept failed: {error}"),
            };
            accepted += 1;
            stream.set_nonblocking(false).unwrap();

            if accepted == 2 {
                let mut buffer = [0_u8; 8192];
                let bytes = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
                request_log.lock().unwrap().push(request);
                let body = concat!(
                    "event: response.output_text.delta\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"fallback\"}\n\n",
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_sse\",\"status\":\"completed\"}}\n\n"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
                continue;
            }

            let mut socket = tungstenite::accept(stream).unwrap();
            request_log.lock().unwrap().push("WS".to_string());
            let _request = socket.read().unwrap();
            let events = if accepted == 1 {
                vec![serde_json::json!({
                    "type": "response.incomplete",
                    "response": {
                        "id": "resp_incomplete",
                        "status": "incomplete",
                        "incomplete_details": {"reason": "content_filter"}
                    }
                })]
            } else {
                vec![
                    serde_json::json!({
                        "type": "response.output_text.delta",
                        "delta": "websocket"
                    }),
                    serde_json::json!({
                        "type": "response.completed",
                        "response": {"id": "resp_ws", "status": "completed"}
                    }),
                ]
            };
            for event in events {
                socket
                    .send(Message::Text(event.to_string().into()))
                    .unwrap();
            }
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(format!("http://{address}")));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    let mut state = state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let first = execute_user_prompt_streaming(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "hello",
        |_| {},
    )
    .unwrap();
    let second = execute_user_prompt_streaming(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "again",
        |_| {},
    )
    .unwrap();
    server.join().unwrap();
    std::env::remove_var("PUFFER_OPENAI_WEBSOCKET");
    std::env::remove_var("PUFFER_OPENAI_WS_MAX_ATTEMPTS");
    std::env::remove_var("PUFFER_OPENAI_STREAM_RETRY_DELAY_MS");
    std::env::remove_var("PUFFER_OPENAI_HTTP_MAX_ATTEMPTS");

    let requests = requests.lock().unwrap();
    assert_eq!(first.assistant_text, "fallback");
    assert_eq!(second.assistant_text, "websocket");
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0], "WS");
    assert!(requests[1].starts_with("POST "));
    assert_eq!(requests[2], "WS");
}
