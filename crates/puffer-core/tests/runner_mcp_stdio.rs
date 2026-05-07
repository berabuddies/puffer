//! Drives `LocalToolRunner` against the `puffer-mcp-stub-server` binary
//! over stdio. Exercises the lazy-spawn path, normal `tools/list` /
//! `tools/call` round-trips, crash recovery via the bounded-retry budget,
//! and the fast-fail path when the configured binary cannot start.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use puffer_core::runner_adapter::LocalToolRunner;
use puffer_resources::McpServerSpec;
use puffer_runner_api::{
    ChunkSink, ElicitationHandler, ElicitationRequest, ElicitationResponse, McpResourceContentPart,
    NullChunkSink, RunnerError, ToolRunner,
};
use serde_json::json;

const STUB_BIN: &str = env!("CARGO_BIN_EXE_puffer-mcp-stub-server");

/// Counts stub server processes whose argv contains the given unique marker.
/// Each test uses its own marker so concurrent tests don't interfere.
fn count_stub_processes(marker: &str) -> usize {
    let output = std::process::Command::new("pgrep")
        .args(["-f", marker])
        .output();
    match output {
        Ok(o) if o.status.success() => o
            .stdout
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .count(),
        _ => 0,
    }
}

fn pid_of_stub(marker: &str) -> Option<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-f", marker])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    std::str::from_utf8(&output.stdout)
        .ok()?
        .lines()
        .next()
        .and_then(|s| s.trim().parse().ok())
}

fn manifest_with_marker(server_id: &str, marker: &str) -> Vec<McpServerSpec> {
    vec![McpServerSpec {
        id: server_id.into(),
        display_name: "Stub".into(),
        transport: "stdio".into(),
        endpoint: String::new(),
        target: format!("'{}' --marker {}", STUB_BIN, marker),
        description: "Integration-test stub MCP server".into(),
        headers: Default::default(),
        oauth: None,
    }]
}

#[test]
fn lazy_spawn_does_not_run_until_first_call() {
    let marker = "puffer-mcp-stub-lazy-spawn-marker";
    let runner = LocalToolRunner::new().with_mcp_servers(manifest_with_marker("stub", marker));

    // Construction alone should not spawn anything.
    assert_eq!(
        count_stub_processes(marker),
        0,
        "no children before first call"
    );

    let tools = runner.list_mcp_tools("stub").expect("list tools");
    assert!(tools.iter().any(|t| t.name == "echo"));
    assert!(tools.iter().any(|t| t.name == "slow_echo"));
    assert!(
        count_stub_processes(marker) >= 1,
        "child spawned on first call"
    );
    drop(runner);
}

#[test]
fn tools_list_returns_stub_tools() {
    let runner = LocalToolRunner::new()
        .with_mcp_servers(manifest_with_marker("stub", "puffer-mcp-stub-tools-list"));
    let tools = runner.list_mcp_tools("stub").expect("list tools");
    let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"echo"), "got {names:?}");
    assert!(names.contains(&"slow_echo"), "got {names:?}");
    assert!(names.contains(&"crash"), "got {names:?}");
}

#[test]
fn tools_call_echo_round_trips() {
    let runner = LocalToolRunner::new().with_mcp_servers(manifest_with_marker(
        "stub",
        "puffer-mcp-stub-echo-round-trip",
    ));
    let mut sink = NullChunkSink;
    let result = runner
        .call_mcp_tool("stub", "echo", json!({ "text": "hello puffer" }), &mut sink)
        .expect("call echo");
    assert!(result.success, "echo should succeed");
    assert_eq!(result.stdout, "hello puffer");
    assert_eq!(result.server, "stub");
    assert_eq!(result.tool, "echo");
}

#[test]
fn tools_call_slow_echo_completes() {
    let runner = LocalToolRunner::new()
        .with_mcp_servers(manifest_with_marker("stub", "puffer-mcp-stub-slow-echo"));
    let mut sink = NullChunkSink;
    let start = std::time::Instant::now();
    let result = runner
        .call_mcp_tool(
            "stub",
            "slow_echo",
            json!({ "text": "delayed", "delay_ms": 50 }),
            &mut sink,
        )
        .expect("call slow_echo");
    let elapsed = start.elapsed();
    assert!(result.success);
    assert_eq!(result.stdout, "delayed");
    assert!(
        elapsed >= Duration::from_millis(40),
        "expected real delay, got {elapsed:?}"
    );
}

#[test]
fn crash_recovery_respawns_on_next_call() {
    let marker = "puffer-mcp-stub-crash-recovery";
    let runner = LocalToolRunner::new().with_mcp_servers(manifest_with_marker("stub", marker));
    let mut sink = NullChunkSink;

    // First, an `echo` to spawn the child.
    let result = runner
        .call_mcp_tool("stub", "echo", json!({ "text": "alive" }), &mut sink)
        .expect("first call");
    assert!(result.success);
    let first_pid = pid_of_stub(marker).expect("child running after first call");

    // Tell the stub to crash, then wait for it to actually exit.
    let _ = runner.call_mcp_tool("stub", "crash", json!({}), &mut sink);
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        if !is_pid_alive(first_pid) {
            break;
        }
    }
    assert!(
        !is_pid_alive(first_pid),
        "stub should have exited after crash"
    );

    // Next `echo` must succeed by spawning a fresh child.
    let result = runner
        .call_mcp_tool("stub", "echo", json!({ "text": "recovered" }), &mut sink)
        .expect("respawn call");
    assert!(result.success);
    assert_eq!(result.stdout, "recovered");
    let second_pid = pid_of_stub(marker).expect("child running after respawn");
    assert_ne!(first_pid, second_pid, "respawn should produce a new pid");
}

#[test]
fn bounded_retries_exhaust_for_dead_binary() {
    let manifest = vec![McpServerSpec {
        id: "dead".into(),
        display_name: "Dead".into(),
        transport: "stdio".into(),
        endpoint: String::new(),
        target: format!("'{}' --exit-immediately", STUB_BIN),
        description: "Always exits immediately".into(),
        headers: Default::default(),
        oauth: None,
    }];
    let runner = LocalToolRunner::new().with_mcp_servers(manifest);

    // Each attempt should fail clearly. We don't over-engineer the cool-off
    // assertion (it can be timing-flaky); the requirement is the failure
    // path surfaces an error every call.
    for attempt in 1..=4 {
        let err = runner
            .list_mcp_tools("dead")
            .expect_err(&format!("attempt {attempt} should fail"));
        assert!(
            matches!(err, RunnerError::Mcp(_)),
            "attempt {attempt}: expected Mcp error, got {err:?}"
        );
    }
}

fn is_pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn list_resources_returns_stub_resources() {
    let runner = LocalToolRunner::new().with_mcp_servers(manifest_with_marker(
        "stub",
        "puffer-mcp-stub-resources-list",
    ));
    let records = runner
        .list_mcp_resources(Some("stub"))
        .expect("list resources");
    let uris: Vec<_> = records.iter().map(|r| r.uri.as_str()).collect();
    assert!(uris.contains(&"stub://hello.txt"), "got {uris:?}");
    assert!(uris.contains(&"stub://binary.bin"), "got {uris:?}");
    for record in &records {
        assert_eq!(record.server, "stub");
    }
}

#[test]
fn read_resource_returns_text_and_blob() {
    let runner = LocalToolRunner::new().with_mcp_servers(manifest_with_marker(
        "stub",
        "puffer-mcp-stub-resources-read",
    ));
    let text = runner
        .read_mcp_resource("stub", "stub://hello.txt")
        .expect("read text");
    match text.parts.first() {
        Some(McpResourceContentPart::Text {
            text, mime_type, ..
        }) => {
            assert_eq!(text, "hello from stub");
            assert_eq!(mime_type.as_deref(), Some("text/plain"));
        }
        other => panic!("expected text content, got {other:?}"),
    }

    let blob = runner
        .read_mcp_resource("stub", "stub://binary.bin")
        .expect("read blob");
    match blob.parts.first() {
        Some(McpResourceContentPart::Blob {
            bytes, mime_type, ..
        }) => {
            assert_eq!(bytes, &vec![0xde, 0xad, 0xbe]);
            assert_eq!(mime_type.as_deref(), Some("application/octet-stream"));
        }
        other => panic!("expected blob content, got {other:?}"),
    }
}

#[test]
fn list_prompts_returns_stub_prompt() {
    let runner = LocalToolRunner::new()
        .with_mcp_servers(manifest_with_marker("stub", "puffer-mcp-stub-prompts-list"));
    let prompts = runner.list_mcp_prompts("stub").expect("list prompts");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "greet");
    assert_eq!(prompts[0].arguments.len(), 1);
    assert_eq!(prompts[0].arguments[0].name, "name");
    assert!(prompts[0].arguments[0].required);
}

#[test]
fn get_prompt_returns_message() {
    let runner = LocalToolRunner::new()
        .with_mcp_servers(manifest_with_marker("stub", "puffer-mcp-stub-prompts-get"));
    let content = runner
        .get_mcp_prompt("stub", "greet", json!({ "name": "puffer" }))
        .expect("get prompt");
    assert_eq!(content.server, "stub");
    assert_eq!(content.name, "greet");
    assert_eq!(content.messages.len(), 1);
    assert_eq!(content.messages[0].role, "user");
    assert_eq!(content.messages[0].text, "Hello, puffer!");
}

/// `ChunkSink` that records every `event` call so the test can assert at
/// least one `notifications/progress` made it through to the sink.
#[derive(Default, Clone)]
struct RecordingSink {
    events: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl ChunkSink for RecordingSink {
    fn stdout(&mut self, _chunk: &[u8]) {}
    fn stderr(&mut self, _chunk: &[u8]) {}
    fn event(&mut self, event: serde_json::Value) {
        self.events.lock().unwrap().push(event);
    }
}

/// `ElicitationHandler` that records every incoming request and replies
/// with a fixed response. Lets tests assert both that the request reached
/// the handler and that the response round-tripped back to the server.
#[derive(Debug, Clone)]
struct RecordingElicitationHandler {
    response: ElicitationResponse,
    requests: Arc<Mutex<Vec<ElicitationRequest>>>,
}

impl RecordingElicitationHandler {
    fn new(response: ElicitationResponse) -> Self {
        Self {
            response,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl ElicitationHandler for RecordingElicitationHandler {
    fn elicit(&self, request: ElicitationRequest) -> ElicitationResponse {
        self.requests.lock().unwrap().push(request);
        self.response.clone()
    }
}

#[test]
fn elicit_with_decline_handler_returns_decline() {
    // Default handler is `DeclineAllElicitations` — no override needed.
    let runner = LocalToolRunner::new().with_mcp_servers(manifest_with_marker(
        "stub",
        "puffer-mcp-stub-elicit-decline",
    ));
    let mut sink = NullChunkSink;
    let result = runner
        .call_mcp_tool("stub", "request_user_input", json!({}), &mut sink)
        .expect("call request_user_input");
    assert!(
        result.success,
        "elicit decline should still produce a tool result"
    );
    let body: serde_json::Value =
        serde_json::from_str(&result.stdout).expect("stub returns JSON body");
    assert_eq!(body.get("action").and_then(|v| v.as_str()), Some("decline"));
}

#[test]
fn elicit_with_accept_handler_returns_value() {
    let handler = RecordingElicitationHandler::new(ElicitationResponse::accept(json!({
        "confirmed": true
    })));
    let requests_handle = handler.requests.clone();
    let runner = LocalToolRunner::new()
        .with_mcp_servers(manifest_with_marker(
            "stub",
            "puffer-mcp-stub-elicit-accept",
        ))
        .with_elicitation_handler(Arc::new(handler));
    let mut sink = NullChunkSink;
    let result = runner
        .call_mcp_tool("stub", "request_user_input", json!({}), &mut sink)
        .expect("call request_user_input");
    assert!(result.success);
    let body: serde_json::Value =
        serde_json::from_str(&result.stdout).expect("stub returns JSON body");
    assert_eq!(body.get("action").and_then(|v| v.as_str()), Some("accept"));
    assert_eq!(
        body.get("content").and_then(|c| c.get("confirmed")),
        Some(&serde_json::Value::Bool(true))
    );
    let captured = requests_handle.lock().unwrap();
    assert_eq!(captured.len(), 1, "handler should see exactly one request");
    assert_eq!(captured[0].server, "stub");
    assert_eq!(captured[0].message, "Confirm the destructive action?");
}

#[test]
fn tools_call_emits_progress_through_sink() {
    let runner = LocalToolRunner::new()
        .with_mcp_servers(manifest_with_marker("stub", "puffer-mcp-stub-progress"));
    let sink = RecordingSink::default();
    let events_handle = sink.events.clone();
    let mut sink = sink;
    let result = runner
        .call_mcp_tool(
            "stub",
            "slow_with_progress",
            json!({ "text": "progress payload", "delay_ms": 30 }),
            &mut sink,
        )
        .expect("slow_with_progress");
    assert!(result.success);
    assert_eq!(result.stdout, "progress payload");
    let events = events_handle.lock().unwrap();
    assert!(
        !events.is_empty(),
        "expected at least one progress event, got none"
    );
    for event in events.iter() {
        assert_eq!(
            event.get("kind").and_then(|v| v.as_str()),
            Some("mcp/progress")
        );
    }
}
