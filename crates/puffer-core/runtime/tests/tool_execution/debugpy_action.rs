use super::*;
use crate::runtime::lambda_gate::{LambdaFact, LambdaGateState, LambdaHostEnv};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::thread;

#[test]
fn debugpy_attach_guard_commits_attached_fact_after_dap_handshake() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        for command in ["initialize", "attach", "configurationDone"] {
            let request = read_dap_message(&mut stream);
            assert_eq!(
                request.get("command").and_then(Value::as_str),
                Some(command)
            );
            let request_seq = request.get("seq").and_then(Value::as_u64).unwrap();
            write_dap_response(&mut stream, request_seq, command);
        }
    });

    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["net_r","net_w"],"domains":[],"tools":[{"name":"dap_client","params":[{"name":"port","ty":"PortNum{debugpy_listening(p)}"}],"result":"unit{attached(p)}","effects":["net_r","net_w"],"registers":[{"pred":"attached","args":["port"]}],"contextReq":null,"concreteTools":["DebugpyAction"],"concreteInputContracts":{"DebugpyAction":{"action":"attach","port":{"$int_arg":"port"}}}}]}"#,
    )
    .unwrap();
    let mut gate = LambdaGateState::with_host_caps(host);
    gate.add_fact(LambdaFact::new("debugpy_listening", vec![port.to_string()]));
    state.lambda_gate = Some(gate);
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            loaded_tool(
                "DebugpyAction",
                "Debugpy action",
                "runtime:workflow:debugpy_action",
            ),
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();

    let admitted = execute_tool_call(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &registry,
        "gpt-5",
        &cwd,
        ToolExecutionBackend::OpenAi {
            request_config: &request_config,
            structured_output: None,
        },
        None,
        "LambdaHostCall",
        json!({
            "host_tool": "dap_client",
            "args": {"port": port},
            "tool": "DebugpyAction",
            "input": {"action": "attach", "port": port},
        }),
    )
    .unwrap();
    assert!(admitted.success);

    let attached = execute_tool_call(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &registry,
        "gpt-5",
        &cwd,
        ToolExecutionBackend::OpenAi {
            request_config: &request_config,
            structured_output: None,
        },
        None,
        "DebugpyAction",
        json!({"action": "attach", "port": port}),
    )
    .unwrap();
    assert!(
        attached.success,
        "attach failed: stdout={} stderr={}",
        attached.output.stdout, attached.output.stderr
    );
    assert!(state.pending_lambda_host_call.is_none());
    assert!(state
        .lambda_gate
        .as_ref()
        .unwrap()
        .facts()
        .contains(&LambdaFact::new("attached", vec![port.to_string()])));
    server.join().unwrap();
}

#[test]
fn debugpy_attach_guard_rejects_without_listening_fact() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let host = LambdaHostEnv::from_json_str(
        r#"{"effects":["net_r","net_w"],"domains":[],"tools":[{"name":"dap_client","params":[{"name":"port","ty":"PortNum{debugpy_listening(p)}"}],"result":"unit{attached(p)}","effects":["net_r","net_w"],"registers":[{"pred":"attached","args":["port"]}],"contextReq":null,"concreteTools":["DebugpyAction"],"concreteInputContracts":{"DebugpyAction":{"action":"attach","port":{"$int_arg":"port"}}}}]}"#,
    )
    .unwrap();
    state.lambda_gate = Some(LambdaGateState::with_host_caps(host));
    let resources = LoadedResources {
        tools: vec![
            loaded_tool(
                "LambdaHostCall",
                "Admit Lambda host call",
                "runtime:lambda_host_call",
            ),
            loaded_tool(
                "DebugpyAction",
                "Debugpy action",
                "runtime:workflow:debugpy_action",
            ),
        ],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let providers = empty_providers();
    let request_config = test_openai_request_config();
    let result = execute_tool_call(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &registry,
        "gpt-5",
        &cwd,
        ToolExecutionBackend::OpenAi {
            request_config: &request_config,
            structured_output: None,
        },
        None,
        "LambdaHostCall",
        json!({
            "host_tool": "dap_client",
            "args": {"port": 5678},
            "tool": "DebugpyAction",
            "input": {"action": "attach", "port": 5678},
        }),
    )
    .unwrap();

    assert!(!result.success);
    assert!(result.output.stdout.contains("debugpy_listening"));
    assert!(state.pending_lambda_host_call.is_none());
}

fn read_dap_message(stream: &mut TcpStream) -> Value {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
    }
    let header = std::str::from_utf8(&header).unwrap();
    let length = header
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length:"))
        .unwrap()
        .trim()
        .parse::<usize>()
        .unwrap();
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn write_dap_response(stream: &mut TcpStream, request_seq: u64, command: &str) {
    let body = serde_json::to_vec(&json!({
        "seq": request_seq + 100,
        "type": "response",
        "request_seq": request_seq,
        "success": true,
        "command": command
    }))
    .unwrap();
    write!(stream, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stream.write_all(&body).unwrap();
}
