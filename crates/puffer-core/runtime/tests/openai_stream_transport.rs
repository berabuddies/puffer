use super::*;

#[test]
fn execute_user_prompt_streaming_parses_headerless_sse() {
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
