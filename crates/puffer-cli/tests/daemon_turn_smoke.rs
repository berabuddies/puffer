use serde_json::{json, Value};
use std::io::{Read, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::{connect, Message, WebSocket};
use url::Url;

#[test]
fn daemon_accepts_desktop_alias_and_completes_mock_turn() {
    let mock = MockOpenAiServer::start("Puffer smoke reply");
    let tempdir = tempfile::tempdir().expect("tempdir");
    let workspace = tempdir.path().join("workspace");
    let puffer_home = tempdir.path().join("home");
    let puffer_config = puffer_home.join(".puffer");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&puffer_config).expect("puffer config");
    std::fs::write(
        puffer_config.join("auth.json"),
        json!({
            "format_version": 1,
            "providers": {
                "openai": { "kind": "api_key", "key": "sk-test" }
            }
        })
        .to_string(),
    )
    .expect("auth store");
    let discovery_cache = tempdir.path().join("discovery.json");
    std::fs::write(&discovery_cache, discovery_cache_json()).expect("discovery cache");

    let mut daemon = DaemonProcess::start(&workspace, &puffer_home, &discovery_cache);
    let mut client = DaemonClient::connect(&daemon.handshake);

    client.rpc(
        "update_config",
        json!({
            "openaiBaseUrl": mock.base_url,
            "defaultProvider": "openai",
            "defaultModel": "openai/gpt-5",
        }),
    );
    let session = client.rpc(
        "create_session",
        json!({
            "cwd": workspace.display().to_string(),
            "providerId": "codex",
            "modelId": "codex/gpt-5",
        }),
    );
    assert_eq!(session["providerId"], "openai");
    assert_eq!(session["modelId"], "gpt-5");
    let session_id = session["sessionId"].as_str().expect("session id");

    let turn = client.rpc(
        "run_agent_turn",
        json!({
            "sessionId": session_id,
            "message": "Say exactly: Puffer smoke reply",
            "providerId": "codex",
            "modelId": "codex/gpt-5",
            "permissionMode": "read-only",
        }),
    );
    let turn_id = turn["turnId"].as_str().expect("turn id");
    let complete = client.wait_for_event(|message| {
        message["event"] == format!("session:{session_id}:event")
            && message["payload"]["type"] == "turn-complete"
    });
    assert_eq!(complete["payload"]["turnId"], turn_id);
    assert_eq!(complete["payload"]["assistantText"], "Puffer smoke reply");

    let detail = client.rpc("load_session_detail", json!({ "sessionId": session_id }));
    let timeline = detail["timeline"].as_array().expect("timeline array");
    assert!(timeline.iter().any(|item| {
        item["kind"] == "assistant_message" && item["text"] == "Puffer smoke reply"
    }));
    assert_eq!(mock.responses_calls.load(Ordering::SeqCst), 1);
    assert!(
        mock.last_responses_body()
            .contains("Say exactly: Puffer smoke reply"),
        "provider request body should include the user instruction"
    );

    daemon.stop();
}

#[test]
fn daemon_uses_desktop_alias_defaults_for_new_turns() {
    let mock = MockOpenAiServer::start("Alias default reply");
    let tempdir = tempfile::tempdir().expect("tempdir");
    let workspace = tempdir.path().join("workspace");
    let puffer_home = tempdir.path().join("home");
    let puffer_config = puffer_home.join(".puffer");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&puffer_config).expect("puffer config");
    std::fs::write(
        puffer_config.join("auth.json"),
        json!({
            "format_version": 1,
            "providers": {
                "openai": { "kind": "api_key", "key": "sk-test" }
            }
        })
        .to_string(),
    )
    .expect("auth store");
    let discovery_cache = tempdir.path().join("discovery.json");
    std::fs::write(&discovery_cache, discovery_cache_json()).expect("discovery cache");

    let mut daemon = DaemonProcess::start(&workspace, &puffer_home, &discovery_cache);
    let mut client = DaemonClient::connect(&daemon.handshake);

    client.rpc(
        "update_config",
        json!({
            "openaiBaseUrl": mock.base_url,
            "defaultProvider": "codex",
            "defaultModel": "codex/gpt-5",
        }),
    );
    let session = client.rpc(
        "create_session",
        json!({
            "cwd": workspace.display().to_string(),
        }),
    );
    let session_id = session["sessionId"].as_str().expect("session id");

    let turn = client.rpc(
        "run_agent_turn",
        json!({
            "sessionId": session_id,
            "message": "Say exactly: Alias default reply",
            "permissionMode": "read-only",
        }),
    );
    let turn_id = turn["turnId"].as_str().expect("turn id");
    let complete = client.wait_for_event(|message| {
        message["event"] == format!("session:{session_id}:event")
            && message["payload"]["type"] == "turn-complete"
    });
    assert_eq!(complete["payload"]["turnId"], turn_id);
    assert_eq!(complete["payload"]["assistantText"], "Alias default reply");
    assert_eq!(mock.responses_calls.load(Ordering::SeqCst), 1);

    daemon.stop();
}

#[test]
fn daemon_uses_session_routing_when_turn_omits_provider_options() {
    let mock = MockOpenAiServer::start("Session routed reply");
    let tempdir = tempfile::tempdir().expect("tempdir");
    let workspace = tempdir.path().join("workspace");
    let puffer_home = tempdir.path().join("home");
    let puffer_config = puffer_home.join(".puffer");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&puffer_config).expect("puffer config");
    std::fs::write(
        puffer_config.join("auth.json"),
        json!({
            "format_version": 1,
            "providers": {
                "openai": { "kind": "api_key", "key": "sk-test" }
            }
        })
        .to_string(),
    )
    .expect("auth store");
    let discovery_cache = tempdir.path().join("discovery.json");
    std::fs::write(&discovery_cache, discovery_cache_json()).expect("discovery cache");

    let mut daemon = DaemonProcess::start(&workspace, &puffer_home, &discovery_cache);
    let mut client = DaemonClient::connect(&daemon.handshake);

    client.rpc(
        "update_config",
        json!({
            "openaiBaseUrl": mock.base_url,
            "defaultProvider": "anthropic",
            "defaultModel": "anthropic/claude-sonnet-4-5",
        }),
    );
    let session = client.rpc(
        "create_session",
        json!({
            "cwd": workspace.display().to_string(),
            "displayName": "Session routed OpenAI",
            "providerId": "codex",
        }),
    );
    assert_eq!(session["providerId"], "openai");
    assert_eq!(session["modelId"], "gpt-5");
    let session_id = session["sessionId"].as_str().expect("session id");

    let turn = client.rpc(
        "run_agent_turn",
        json!({
            "sessionId": session_id,
            "message": "Say exactly: Session routed reply",
            "permissionMode": "read-only",
        }),
    );
    let turn_id = turn["turnId"].as_str().expect("turn id");
    let complete = client.wait_for_event(|message| {
        message["event"] == format!("session:{session_id}:event")
            && message["payload"]["type"] == "turn-complete"
    });
    assert_eq!(complete["payload"]["turnId"], turn_id);
    assert_eq!(complete["payload"]["assistantText"], "Session routed reply");
    assert_eq!(mock.responses_calls.load(Ordering::SeqCst), 1);

    daemon.stop();
}

struct DaemonProcess {
    child: Child,
    handshake: Value,
    stderr: Arc<Mutex<String>>,
}

impl DaemonProcess {
    fn start(workspace: &Path, puffer_home: &Path, discovery_cache: &Path) -> Self {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("cli crate parent")
            .parent()
            .expect("repo root");
        let mut child = Command::new(env!("CARGO_BIN_EXE_puffer"))
            .args([
                "daemon",
                "--bind",
                "127.0.0.1:0",
                "--token",
                "smoke-token",
                "--print-handshake",
                "--no-browser",
                "--disable-auto-title",
            ])
            .current_dir(workspace)
            .env("PUFFER_HOME", puffer_home)
            .env("PUFFER_BUILTIN_RESOURCES_DIR", repo_root.join("resources"))
            .env("PUFFER_DISCOVERY_CACHE_PATH", discovery_cache)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn daemon");

        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_thread = Arc::clone(&stderr);
        let mut err = child.stderr.take().expect("daemon stderr");
        thread::spawn(move || {
            let mut buf = String::new();
            let _ = err.read_to_string(&mut buf);
            *stderr_thread.lock().unwrap() = buf;
        });

        let mut stdout = child.stdout.take().expect("daemon stdout");
        let handshake = read_handshake_line(&mut stdout, &mut child, &stderr);
        Self {
            child,
            handshake,
            stderr,
        }
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let stderr = self.stderr.lock().unwrap();
        if !stderr.is_empty() {
            eprintln!("daemon stderr:\n{stderr}");
        }
    }
}

struct DaemonClient {
    socket: WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    next_id: u64,
    backlog: Vec<Value>,
}

impl DaemonClient {
    fn connect(handshake: &Value) -> Self {
        let mut url = Url::parse(handshake["url"].as_str().expect("daemon url")).expect("url");
        url.query_pairs_mut()
            .append_pair("token", handshake["token"].as_str().expect("token"));
        let (socket, _) = connect(url.as_str()).expect("connect daemon websocket");
        Self {
            socket,
            next_id: 1,
            backlog: Vec::new(),
        }
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id.to_string();
        self.next_id += 1;
        self.socket
            .send(Message::Text(
                json!({ "id": id, "method": method, "params": params })
                    .to_string()
                    .into(),
            ))
            .expect("send daemon request");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(Instant::now() < deadline, "{method} timed out");
            let message = self.read_message();
            if message["id"].as_str() == Some(id.as_str()) {
                if !message["error"].is_null() {
                    panic!("{method} failed: {}", message["error"]);
                }
                return message["result"].clone();
            }
            self.backlog.push(message);
        }
    }

    fn wait_for_event(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        if let Some(index) = self.backlog.iter().position(&predicate) {
            return self.backlog.remove(index);
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(Instant::now() < deadline, "event timed out");
            let message = self.read_message();
            if predicate(&message) {
                return message;
            }
            self.backlog.push(message);
        }
    }

    fn read_message(&mut self) -> Value {
        loop {
            let message = self.socket.read().expect("read daemon message");
            if let Message::Text(text) = message {
                return serde_json::from_str(&text).expect("daemon message json");
            }
        }
    }
}

struct MockOpenAiServer {
    base_url: String,
    responses_calls: Arc<AtomicUsize>,
    last_body: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockOpenAiServer {
    fn start(reply: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock openai");
        listener.set_nonblocking(true).expect("nonblocking mock");
        let address = listener.local_addr().expect("mock address");
        let stop = Arc::new(AtomicBool::new(false));
        let responses_calls = Arc::new(AtomicUsize::new(0));
        let last_body = Arc::new(Mutex::new(String::new()));
        let thread_stop = Arc::clone(&stop);
        let thread_calls = Arc::clone(&responses_calls);
        let thread_body = Arc::clone(&last_body);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        handle_mock_openai_stream(stream, reply, &thread_calls, &thread_body);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept mock openai request: {error}"),
                }
            }
        });
        Self {
            base_url: format!("http://{address}"),
            responses_calls,
            last_body,
            stop,
            handle: Some(handle),
        }
    }

    fn last_responses_body(&self) -> String {
        self.last_body.lock().unwrap().clone()
    }
}

impl Drop for MockOpenAiServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut stream) = TcpStream::connect(self.base_url.trim_start_matches("http://")) {
            let _ = stream.write_all(b"GET /shutdown HTTP/1.1\r\nHost: localhost\r\n\r\n");
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_handshake_line(
    stdout: &mut impl Read,
    child: &mut Child,
    stderr: &Arc<Mutex<String>>,
) -> Value {
    let mut line = String::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buf = [0_u8; 1];
    while Instant::now() < deadline {
        match stdout.read(&mut buf) {
            Ok(0) => {
                if let Some(status) = child.try_wait().expect("daemon status") {
                    panic!(
                        "daemon exited before handshake: {status}\n{}",
                        stderr.lock().unwrap()
                    );
                }
                thread::sleep(Duration::from_millis(10));
            }
            Ok(_) if buf[0] == b'\n' => break,
            Ok(_) => line.push(buf[0] as char),
            Err(error) => panic!("read daemon handshake: {error}"),
        }
    }
    assert!(!line.is_empty(), "daemon handshake timed out");
    serde_json::from_str(&line).expect("handshake json")
}

fn handle_mock_openai_stream(
    mut stream: TcpStream,
    reply: &str,
    responses_calls: &AtomicUsize,
    last_body: &Mutex<String>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut buf = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buf).expect("read mock request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buf[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            let content_length = parse_content_length(&request).unwrap_or(0);
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
                .unwrap_or(request.len());
            while request.len() < header_end + content_length {
                let read = stream.read(&mut buf).expect("read mock body");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..read]);
            }
            break;
        }
    }
    let text = String::from_utf8_lossy(&request);
    let path = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    match path {
        "/v1/models" => write_http_json(
            &mut stream,
            json!({ "data": [{ "id": "gpt-5", "name": "GPT 5 smoke" }] }),
        ),
        "/v1/responses" => {
            responses_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(body) = text.split("\r\n\r\n").nth(1) {
                *last_body.lock().unwrap() = body.to_string();
            }
            write_http_json(
                &mut stream,
                json!({
                    "id": "resp_smoke",
                    "status": "completed",
                    "output_text": reply,
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": reply }]
                    }],
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 4,
                        "input_tokens_details": { "cached_tokens": 0 }
                    }
                }),
            );
        }
        _ => write_http_response(&mut stream, 404, "text/plain", b"not found"),
    }
}

fn parse_content_length(request: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(request);
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            value.trim().parse().ok()
        } else {
            None
        }
    })
}

fn write_http_json(stream: &mut TcpStream, value: Value) {
    let body = value.to_string();
    write_http_response(stream, 200, "application/json", body.as_bytes());
}

fn write_http_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).expect("write header");
    stream.write_all(body).expect("write body");
}

fn discovery_cache_json() -> String {
    let now = 1_700_000_000_000_u64;
    json!({
        "entries": {
            "llama-cpp": { "models": [], "cached_at_ms": now },
            "lmstudio": { "models": [], "cached_at_ms": now },
            "ollama": { "models": [], "cached_at_ms": now },
            "vllm": { "models": [], "cached_at_ms": now }
        }
    })
    .to_string()
}
