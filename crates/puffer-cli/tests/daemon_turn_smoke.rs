use serde_json::{json, Value};
use std::io::{ErrorKind, Read, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::{connect, stream::MaybeTlsStream, Message, WebSocket};
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

#[test]
fn daemon_accepts_claude_alias_and_completes_mock_anthropic_turn() {
    let mock = MockAnthropicServer::start("Claude smoke reply");
    let tempdir = tempfile::tempdir().expect("tempdir");
    let workspace = tempdir.path().join("workspace");
    let puffer_home = tempdir.path().join("home");
    let puffer_config = puffer_home.join(".puffer");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&puffer_config).expect("puffer config");
    write_anthropic_provider_override(&workspace, &mock.base_url);
    std::fs::write(
        puffer_config.join("auth.json"),
        json!({
            "format_version": 1,
            "providers": {
                "anthropic": { "kind": "api_key", "key": "sk-ant-test" }
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
            "defaultProvider": "claude",
            "defaultModel": "claude/claude-sonnet-4-5",
        }),
    );
    let session = client.rpc(
        "create_session",
        json!({
            "cwd": workspace.display().to_string(),
            "providerId": "claude",
            "modelId": "claude/claude-sonnet-4-5",
        }),
    );
    assert_eq!(session["providerId"], "anthropic");
    assert_eq!(session["modelId"], "claude-sonnet-4-5");
    let session_id = session["sessionId"].as_str().expect("session id");

    let turn = client.rpc(
        "run_agent_turn",
        json!({
            "sessionId": session_id,
            "message": "Say exactly: Claude smoke reply",
            "permissionMode": "read-only",
        }),
    );
    let turn_id = turn["turnId"].as_str().expect("turn id");
    let complete = client.wait_for_event(|message| {
        message["event"] == format!("session:{session_id}:event")
            && message["payload"]["type"] == "turn-complete"
    });
    assert_eq!(complete["payload"]["turnId"], turn_id);
    assert_eq!(complete["payload"]["assistantText"], "Claude smoke reply");
    assert_eq!(mock.messages_calls.load(Ordering::SeqCst), 1);
    let body = mock.last_messages_body();
    assert!(
        body.contains("Say exactly: Claude smoke reply"),
        "provider request body should include the user instruction: {body}"
    );
    assert!(
        body.contains("claude-sonnet-4-5"),
        "provider request body should include the selected Claude model: {body}"
    );

    daemon.stop();
}

#[test]
fn daemon_turn_error_refreshes_workspace_and_marks_session_idle() {
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
    let failing_base_url = closed_http_base_url();

    client.rpc(
        "update_config",
        json!({
            "openaiBaseUrl": failing_base_url,
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
    let session_id = session["sessionId"].as_str().expect("session id");

    let turn = client.rpc(
        "run_agent_turn",
        json!({
            "sessionId": session_id,
            "message": "Trigger a controlled provider error",
            "providerId": "codex",
            "modelId": "codex/gpt-5",
            "permissionMode": "read-only",
        }),
    );
    let turn_id = turn["turnId"].as_str().expect("turn id");
    let error = client.wait_for_event(|message| {
        message["event"] == format!("session:{session_id}:event")
            && message["payload"]["type"] == "turn-error"
    });
    assert_eq!(error["payload"]["turnId"], turn_id);
    let error_text = error["payload"]["error"]
        .as_str()
        .expect("friendly error")
        .to_string();

    let changed = client.wait_for_event(|message| {
        message["event"] == "workspace:sessions:changed"
            && message["payload"]["reason"] == "turn_error"
    });
    assert_eq!(changed["payload"]["sessionId"], session_id);

    let detail = client.rpc("load_session_detail", json!({ "sessionId": session_id }));
    let timeline = detail["timeline"].as_array().expect("timeline array");
    assert!(timeline.iter().any(|item| {
        item["kind"] == "user_message" && item["text"] == "Trigger a controlled provider error"
    }));
    assert!(timeline.iter().any(|item| {
        item["kind"] == "system_message"
            && item["text"].as_str().is_some_and(|text| text == error_text)
    }));

    let groups = client.rpc("list_grouped_sessions", json!({}));
    let status = groups
        .as_array()
        .expect("groups")
        .iter()
        .flat_map(|group| group["sessions"].as_array().into_iter().flatten())
        .find(|item| item["sessionId"] == session_id)
        .and_then(|item| item["activityStatus"].as_str())
        .expect("session activity status");
    assert_eq!(status, "idle");

    daemon.stop();
}

#[test]
fn daemon_cancel_turn_marks_session_idle_before_provider_returns() {
    let mock = MockOpenAiServer::start_with_delay(
        "Reply that should arrive after cancellation",
        Duration::from_secs(2),
    );
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
    let session_id = session["sessionId"].as_str().expect("session id");

    let turn = client.rpc(
        "run_agent_turn",
        json!({
            "sessionId": session_id,
            "message": "Cancel while provider is still responding",
            "providerId": "codex",
            "modelId": "codex/gpt-5",
            "permissionMode": "read-only",
        }),
    );
    let turn_id = turn["turnId"].as_str().expect("turn id");
    wait_until(Duration::from_secs(5), || {
        mock.responses_calls.load(Ordering::SeqCst) > 0
    });

    let cancelled = client.rpc("cancel_turn", json!({ "turnId": turn_id }));
    assert_eq!(cancelled["ok"], true);
    let error = client.wait_for_event(|message| {
        message["event"] == format!("session:{session_id}:event")
            && message["payload"]["type"] == "turn-error"
    });
    assert_eq!(error["payload"]["turnId"], turn_id);
    assert_eq!(error["payload"]["category"], "cancelled");
    assert_eq!(error["payload"]["error"], "Interrupted by user.");

    let detail = client.rpc("load_session_detail", json!({ "sessionId": session_id }));
    let timeline = detail["timeline"].as_array().expect("timeline array");
    assert!(timeline.iter().any(|item| {
        item["kind"] == "user_message"
            && item["text"] == "Cancel while provider is still responding"
    }));
    assert!(timeline.iter().any(|item| {
        item["kind"] == "system_message" && item["text"] == "Interrupted by user."
    }));

    let groups = client.rpc("list_grouped_sessions", json!({}));
    let status = groups
        .as_array()
        .expect("groups")
        .iter()
        .flat_map(|group| group["sessions"].as_array().into_iter().flatten())
        .find(|item| item["sessionId"] == session_id)
        .and_then(|item| item["activityStatus"].as_str())
        .expect("session activity status");
    assert_eq!(status, "idle");

    daemon.stop();
}

#[test]
fn daemon_accepts_new_turn_after_cancel_before_provider_returns() {
    let mock = MockOpenAiServer::start_with_delay(
        "Reply that should arrive after cancellation",
        Duration::from_secs(2),
    );
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
    let session_id = session["sessionId"].as_str().expect("session id");

    let first = client.rpc(
        "run_agent_turn",
        json!({
            "sessionId": session_id,
            "message": "Cancel this before the provider returns",
            "providerId": "codex",
            "modelId": "codex/gpt-5",
            "permissionMode": "read-only",
        }),
    );
    let first_turn_id = first["turnId"].as_str().expect("first turn id");
    wait_until(Duration::from_secs(5), || {
        mock.responses_calls.load(Ordering::SeqCst) > 0
    });

    let cancelled = client.rpc("cancel_turn", json!({ "turnId": first_turn_id }));
    assert_eq!(cancelled["ok"], true);
    let error = client.wait_for_event(|message| {
        message["event"] == format!("session:{session_id}:event")
            && message["payload"]["type"] == "turn-error"
            && message["payload"]["turnId"] == first_turn_id
    });
    assert_eq!(error["payload"]["category"], "cancelled");

    let second = client.rpc(
        "run_agent_turn",
        json!({
            "sessionId": session_id,
            "message": "Start again immediately after cancel",
            "providerId": "codex",
            "modelId": "codex/gpt-5",
            "permissionMode": "read-only",
        }),
    );
    let second_turn_id = second["turnId"].as_str().expect("second turn id");
    assert_ne!(second_turn_id, first_turn_id);

    daemon.stop();
}

#[test]
fn daemon_rejects_concurrent_turn_for_same_session() {
    let mock =
        MockOpenAiServer::start_with_delay("First concurrent turn reply", Duration::from_secs(2));
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
    let session_id = session["sessionId"].as_str().expect("session id");

    let first = client.rpc(
        "run_agent_turn",
        json!({
            "sessionId": session_id,
            "message": "First concurrent turn",
            "providerId": "codex",
            "modelId": "codex/gpt-5",
            "permissionMode": "read-only",
        }),
    );
    let first_turn_id = first["turnId"].as_str().expect("turn id");
    wait_until(Duration::from_secs(5), || {
        mock.responses_calls.load(Ordering::SeqCst) > 0
    });

    let error = client
        .try_rpc(
            "run_agent_turn",
            json!({
                "sessionId": session_id,
                "message": "Second concurrent turn",
                "providerId": "codex",
                "modelId": "codex/gpt-5",
                "permissionMode": "read-only",
            }),
        )
        .expect_err("same-session concurrent turn should be rejected");
    assert_eq!(error["code"], "turn-start-error");
    let error_message = error["message"].as_str().expect("error message");
    assert!(error_message.contains("already has an in-flight turn"));
    assert!(error_message.contains(first_turn_id));
    assert_eq!(mock.responses_calls.load(Ordering::SeqCst), 1);

    let complete = client.wait_for_event(|message| {
        message["event"] == format!("session:{session_id}:event")
            && message["payload"]["type"] == "turn-complete"
    });
    assert_eq!(complete["payload"]["turnId"], first_turn_id);
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
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    backlog: Vec<Value>,
}

impl DaemonClient {
    fn connect(handshake: &Value) -> Self {
        let mut url = Url::parse(handshake["url"].as_str().expect("daemon url")).expect("url");
        url.query_pairs_mut()
            .append_pair("token", handshake["token"].as_str().expect("token"));
        let (socket, _) = connect(url.as_str()).expect("connect daemon websocket");
        set_daemon_socket_read_timeout(&socket, Some(Duration::from_millis(100)));
        Self {
            socket,
            next_id: 1,
            backlog: Vec::new(),
        }
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        match self.try_rpc(method, params) {
            Ok(result) => result,
            Err(error) => panic!("{method} failed: {error}"),
        }
    }

    fn try_rpc(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        let message = self.rpc_response(method, params);
        if message["error"].is_null() {
            Ok(message["result"].clone())
        } else {
            Err(message["error"].clone())
        }
    }

    fn rpc_response(&mut self, method: &str, params: Value) -> Value {
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
            let message = self.read_message_until(deadline);
            if message["id"].as_str() == Some(id.as_str()) {
                return message;
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
            let message = self.read_message_until(deadline);
            if predicate(&message) {
                return message;
            }
            self.backlog.push(message);
        }
    }

    fn read_message_until(&mut self, deadline: Instant) -> Value {
        loop {
            assert!(Instant::now() < deadline, "daemon message timed out");
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str(&text).expect("daemon message json");
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(error) => panic!("read daemon message: {error}"),
            }
        }
    }
}

fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(predicate(), "condition timed out");
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
        Self::start_with_delay(reply, Duration::ZERO)
    }

    fn start_with_delay(reply: &'static str, response_delay: Duration) -> Self {
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
                        handle_mock_openai_stream(
                            stream,
                            reply,
                            response_delay,
                            &thread_calls,
                            &thread_body,
                        );
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

struct MockAnthropicServer {
    base_url: String,
    messages_calls: Arc<AtomicUsize>,
    last_body: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockAnthropicServer {
    fn start(reply: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock anthropic");
        listener.set_nonblocking(true).expect("nonblocking mock");
        let address = listener.local_addr().expect("mock address");
        let stop = Arc::new(AtomicBool::new(false));
        let messages_calls = Arc::new(AtomicUsize::new(0));
        let last_body = Arc::new(Mutex::new(String::new()));
        let thread_stop = Arc::clone(&stop);
        let thread_calls = Arc::clone(&messages_calls);
        let thread_body = Arc::clone(&last_body);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        handle_mock_anthropic_stream(stream, reply, &thread_calls, &thread_body);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept mock anthropic request: {error}"),
                }
            }
        });
        Self {
            base_url: format!("http://{address}"),
            messages_calls,
            last_body,
            stop,
            handle: Some(handle),
        }
    }

    fn last_messages_body(&self) -> String {
        self.last_body.lock().unwrap().clone()
    }
}

impl Drop for MockAnthropicServer {
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
    response_delay: Duration,
    responses_calls: &AtomicUsize,
    last_body: &Mutex<String>,
) {
    let _ = stream.set_nonblocking(false);
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
            thread::sleep(response_delay);
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

fn handle_mock_anthropic_stream(
    mut stream: TcpStream,
    reply: &str,
    messages_calls: &AtomicUsize,
    last_body: &Mutex<String>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let request = read_http_request(&mut stream);
    let text = String::from_utf8_lossy(&request);
    let path = request_path(&text);
    if path.starts_with("/v1/models") {
        write_http_json(
            &mut stream,
            json!({ "data": [{ "id": "claude-sonnet-4-5", "display_name": "Claude Sonnet 4.5" }] }),
        );
    } else if path.starts_with("/v1/messages") {
        messages_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(body) = text.split("\r\n\r\n").nth(1) {
            *last_body.lock().unwrap() = body.to_string();
        }
        write_http_response(
            &mut stream,
            200,
            "text/event-stream",
            anthropic_text_stream(reply).as_bytes(),
        );
    } else {
        write_http_response(&mut stream, 404, "text/plain", b"not found");
    }
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
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
    request
}

fn request_path(text: &str) -> String {
    text.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string()
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

fn anthropic_text_stream(reply: &str) -> String {
    [
        sse_event(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_daemon_smoke",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4-5",
                    "content": [],
                    "usage": {
                        "input_tokens": 10,
                        "cache_read_input_tokens": 0,
                        "cache_creation_input_tokens": 0,
                        "output_tokens": 1
                    }
                }
            }),
        ),
        sse_event(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "" }
            }),
        ),
        sse_event(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": reply }
            }),
        ),
        sse_event(
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 0 }),
        ),
        sse_event(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn" },
                "usage": { "output_tokens": 4 }
            }),
        ),
        sse_event("message_stop", json!({ "type": "message_stop" })),
    ]
    .join("")
}

fn sse_event(event: &str, data: Value) -> String {
    format!("event:{event}\ndata:{data}\n\n")
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

fn write_anthropic_provider_override(workspace: &Path, base_url: &str) {
    let provider_dir = workspace
        .join(".puffer")
        .join("resources")
        .join("providers");
    std::fs::create_dir_all(&provider_dir).expect("workspace provider dir");
    std::fs::write(
        provider_dir.join("anthropic.yaml"),
        format!(
            r#"id: anthropic
display_name: Anthropic
base_url: "{base_url}"
default_api: anthropic-messages
auth_modes:
  - api_key
  - oauth
discovery:
  path: /v1/models
  response: anthropic_models
  api: anthropic-messages
  context_window: 200000
  max_output_tokens: 8192
  supports_reasoning: true
models:
  - id: claude-sonnet-4-5
    display_name: Claude Sonnet 4.5
    provider: anthropic
    api: anthropic-messages
    context_window: 200000
    max_output_tokens: 8192
    supports_reasoning: true
"#
        ),
    )
    .expect("write anthropic provider override");
}

fn closed_http_base_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind closed url");
    let address = listener.local_addr().expect("closed url address");
    drop(listener);
    format!("http://{address}")
}

fn set_daemon_socket_read_timeout(
    socket: &WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Option<Duration>,
) {
    let tcp = match socket.get_ref() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => stream.get_ref(),
        _ => return,
    };
    let _ = tcp.set_read_timeout(timeout);
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
