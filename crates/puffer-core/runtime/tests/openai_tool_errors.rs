use super::*;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn spawn_tool_error_server() -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let responses = [
            json!({
                "id": "resp_1",
                "output": [{
                    "type": "function_call",
                    "call_id": "call_missing",
                    "name": "MissingTool",
                    "arguments": "{}"
                }]
            })
            .to_string(),
            json!({
                "id": "resp_2",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "recovered after failed tool"
                    }]
                }],
                "output_text": "recovered after failed tool"
            })
            .to_string(),
        ];
        let mut handled = 0_usize;
        while handled < responses.len() && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    let mut buffer = vec![0_u8; 65_536];
                    let bytes = stream.read(&mut buffer).unwrap();
                    request_log
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&buffer[..bytes]).to_string());
                    let body = &responses[handled];
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
fn execute_openai_tool_calls_surfaces_tool_errors_as_results() {
    let missing_path = std::env::current_dir()
        .unwrap()
        .join("definitely-missing-read-target.txt");
    let resources = LoadedResources {
        tools: vec![loaded_tool("Read", "Read a file", "read")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("https://api.openai.com".to_string()));
    let mut state = state();
    let result = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &[OpenAIResponseToolCall {
            item_id: None,
            status: None,
            call_id: "call_1".to_string(),
            name: "Read".to_string(),
            arguments: json!({ "file_path": missing_path }),
        }],
        &registry,
        std::env::current_dir().unwrap().as_path(),
        &test_openai_request_config(),
        "gpt-5",
        None,
        None,
    )
    .unwrap();
    assert_eq!(result.invocations.len(), 1);
    assert!(!result.invocations[0].success);
    assert!(result.invocations[0]
        .output
        .contains("Tool execution failed:"));
}

#[test]
fn agent_loop_continues_after_serial_tool_execution_error() {
    let (base_url, requests, server) = spawn_tool_error_server();
    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(base_url));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    let mut state = state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let turn = execute_user_prompt(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "trigger a missing tool",
    )
    .unwrap();

    server.join().unwrap();

    assert_eq!(turn.assistant_text, "recovered after failed tool");
    assert_eq!(turn.tool_invocations.len(), 1);
    assert_eq!(turn.tool_invocations[0].tool_id, "MissingTool");
    assert!(!turn.tool_invocations[0].success);
    assert!(turn.tool_invocations[0]
        .output
        .contains("Tool execution failed: unknown tool MissingTool"));

    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    let second_body = request_json_body(&captured[1]);
    let input = second_body
        .get("input")
        .and_then(Value::as_array)
        .expect("second request should include input");
    let output = input
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
        .expect("second request should include function call output");
    assert_eq!(
        output.get("call_id").and_then(Value::as_str),
        Some("call_missing")
    );
    assert!(output
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .contains("Tool execution failed: unknown tool MissingTool"));
}
