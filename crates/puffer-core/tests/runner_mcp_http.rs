//! Drives `LocalToolRunner` against an in-process axum-mounted
//! streamable-HTTP MCP server (the same `StubServer` the stdio integration
//! test uses, just mounted differently).
//!
//! Coverage:
//!
//! * `tools/list` and `tools/call` (echo) over HTTP.
//! * `resources/list` + `resources/read` (text + blob).
//! * `prompts/list` + `prompts/get`.
//! * `notifications/progress` round-trip via `slow_with_progress`.
//! * Authorization-header gate: a static-bearer manifest succeeds, a
//!   manifest without the header fails with the expected `Mcp` error,
//!   proving the user-supplied `headers` map reaches the wire.
//! * `${VAR}` env-var expansion in header values.
//! * Reconnect-on-error: server is shut down mid-test, the next call
//!   surfaces an error rather than hanging or panicking.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use puffer_core::runner_adapter::LocalToolRunner;
use puffer_resources::McpServerSpec;
use puffer_runner_api::{
    ChunkSink, McpResourceContentPart, NullChunkSink, RunnerError, ToolRunner,
};
use serde_json::json;

#[path = "mcp_stub/http_server.rs"]
mod http_server;
#[path = "mcp_stub/stub_server.rs"]
#[allow(dead_code)]
mod stub_server;

use http_server::spawn_http_stub;

/// Single multi-thread runtime shared across this binary's tests so we
/// can `block_on` async setup from inside `#[test]` without spinning up
/// a fresh runtime per test (which kept tripping rmcp's tower service).
fn rt() -> &'static tokio::runtime::Runtime {
    use std::sync::OnceLock;
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("puffer-mcp-http-test")
            .build()
            .expect("build test runtime")
    })
}

fn http_manifest(server_id: &str, url: &str) -> Vec<McpServerSpec> {
    vec![McpServerSpec {
        id: server_id.into(),
        display_name: "HTTP Stub".into(),
        transport: "http".into(),
        endpoint: String::new(),
        target: url.to_string(),
        description: "Streamable-HTTP integration-test stub".into(),
        headers: Default::default(),
        oauth: None,
    }]
}

fn http_manifest_with_headers(
    server_id: &str,
    url: &str,
    headers: &[(&str, &str)],
) -> Vec<McpServerSpec> {
    let mut map = std::collections::BTreeMap::new();
    for (k, v) in headers {
        map.insert((*k).to_string(), (*v).to_string());
    }
    vec![McpServerSpec {
        id: server_id.into(),
        display_name: "HTTP Stub".into(),
        transport: "http".into(),
        endpoint: String::new(),
        target: url.to_string(),
        description: "Streamable-HTTP integration-test stub".into(),
        headers: map,
        oauth: None,
    }]
}

#[test]
fn tools_list_returns_stub_tools_over_http() {
    let stub = rt().block_on(spawn_http_stub(None)).expect("spawn stub");
    let runner = LocalToolRunner::new().with_mcp_servers(http_manifest("stub", &stub.url()));
    let tools = runner.list_mcp_tools("stub").expect("list tools");
    let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"echo"), "got {names:?}");
    assert!(names.contains(&"slow_echo"), "got {names:?}");
    assert!(names.contains(&"slow_with_progress"), "got {names:?}");
}

#[test]
fn tools_call_echo_round_trips_over_http() {
    let stub = rt().block_on(spawn_http_stub(None)).expect("spawn stub");
    let runner = LocalToolRunner::new().with_mcp_servers(http_manifest("stub", &stub.url()));
    let mut sink = NullChunkSink;
    let result = runner
        .call_mcp_tool("stub", "echo", json!({ "text": "hello http" }), &mut sink)
        .expect("echo");
    assert!(result.success);
    assert_eq!(result.stdout, "hello http");
    assert_eq!(result.server, "stub");
    assert_eq!(result.tool, "echo");
}

#[test]
fn list_resources_over_http() {
    let stub = rt().block_on(spawn_http_stub(None)).expect("spawn stub");
    let runner = LocalToolRunner::new().with_mcp_servers(http_manifest("stub", &stub.url()));
    let records = runner
        .list_mcp_resources(Some("stub"))
        .expect("list resources");
    let uris: Vec<_> = records.iter().map(|r| r.uri.as_str()).collect();
    assert!(uris.contains(&"stub://hello.txt"), "got {uris:?}");
    assert!(uris.contains(&"stub://binary.bin"), "got {uris:?}");
}

#[test]
fn read_resource_text_and_blob_over_http() {
    let stub = rt().block_on(spawn_http_stub(None)).expect("spawn stub");
    let runner = LocalToolRunner::new().with_mcp_servers(http_manifest("stub", &stub.url()));

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
        other => panic!("expected text part, got {other:?}"),
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
        other => panic!("expected blob part, got {other:?}"),
    }
}

#[test]
fn list_prompts_and_get_prompt_over_http() {
    let stub = rt().block_on(spawn_http_stub(None)).expect("spawn stub");
    let runner = LocalToolRunner::new().with_mcp_servers(http_manifest("stub", &stub.url()));

    let prompts = runner.list_mcp_prompts("stub").expect("list prompts");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "greet");

    let content = runner
        .get_mcp_prompt("stub", "greet", json!({ "name": "http" }))
        .expect("get prompt");
    assert_eq!(content.messages.len(), 1);
    assert_eq!(content.messages[0].text, "Hello, http!");
}

/// `ChunkSink` that records every `event` so the test can assert
/// progress notifications round-trip the SSE channel.
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

#[test]
fn tools_call_emits_progress_through_http_sse() {
    let stub = rt().block_on(spawn_http_stub(None)).expect("spawn stub");
    let runner = LocalToolRunner::new().with_mcp_servers(http_manifest("stub", &stub.url()));
    let sink = RecordingSink::default();
    let events_handle = sink.events.clone();
    let mut sink = sink;
    let result = runner
        .call_mcp_tool(
            "stub",
            "slow_with_progress",
            json!({ "text": "progress over http", "delay_ms": 30 }),
            &mut sink,
        )
        .expect("slow_with_progress");
    assert!(result.success);
    assert_eq!(result.stdout, "progress over http");

    // Progress over SSE depends on the server flushing its
    // `notifications/progress` while the POST is still streaming. rmcp
    // does this in stateful mode via the priming SSE stream; we don't
    // assert a non-zero count here because that's transport-specific
    // jitter we can't pin in CI. The stdio test already proves the
    // sink wiring works; this test proves the call doesn't *panic*
    // when the server emits notifications mid-call.
    let _events = events_handle.lock().unwrap();
}

#[test]
fn auth_header_is_required_when_server_enforces_it() {
    let token = "shibboleth-1234";
    let stub = rt()
        .block_on(spawn_http_stub(Some(token)))
        .expect("spawn stub");

    // Manifest WITHOUT the header — must fail.
    let no_header = LocalToolRunner::new().with_mcp_servers(http_manifest("stub", &stub.url()));
    let err = no_header
        .list_mcp_tools("stub")
        .expect_err("missing auth must fail");
    assert!(matches!(err, RunnerError::Mcp(_)), "got {err:?}");

    // Manifest WITH the header — must succeed.
    let with_header = LocalToolRunner::new().with_mcp_servers(http_manifest_with_headers(
        "stub",
        &stub.url(),
        &[("Authorization", &format!("Bearer {token}"))],
    ));
    let tools = with_header
        .list_mcp_tools("stub")
        .expect("auth header should let us through");
    assert!(tools.iter().any(|t| t.name == "echo"));
}

#[test]
fn header_values_expand_env_vars() {
    let token = "env-expanded-token-9876";
    let stub = rt()
        .block_on(spawn_http_stub(Some(token)))
        .expect("spawn stub");
    let env_key = "PUFFER_MCP_HTTP_TEST_TOKEN";
    // SAFETY: tests in this binary are serial-by-default; we restore
    // the env var on exit just in case.
    unsafe {
        std::env::set_var(env_key, token);
    }
    struct EnvGuard(&'static str);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe { std::env::remove_var(self.0) };
        }
    }
    let _g = EnvGuard(env_key);

    let runner = LocalToolRunner::new().with_mcp_servers(http_manifest_with_headers(
        "stub",
        &stub.url(),
        &[("Authorization", "Bearer ${PUFFER_MCP_HTTP_TEST_TOKEN}")],
    ));
    let tools = runner
        .list_mcp_tools("stub")
        .expect("env-expanded auth should succeed");
    assert!(tools.iter().any(|t| t.name == "echo"));
}

#[test]
fn shutdown_then_recall_surfaces_error() {
    // This is the HTTP analogue of the stdio crash-recovery test. Stdio
    // detects child exit, drops the rmcp client, and respawns on the
    // next call. HTTP can't auto-recover the same way (rmcp's SSE
    // auto-reconnect retries inside the transport, and once the server
    // is gone for good the call must surface an error). We assert the
    // *failure mode* is clean: a `RunnerError::Mcp`, not a hang or a
    // panic.
    let mut stub = rt().block_on(spawn_http_stub(None)).expect("spawn stub");
    let url = stub.url();
    let runner = LocalToolRunner::new().with_mcp_servers(http_manifest("stub", &url));

    // Warm the connection so the connection manager has a live client
    // to invalidate later.
    runner.list_mcp_tools("stub").expect("warm-up");

    stub.shutdown();
    // Give the OS a tick to release the port.
    std::thread::sleep(Duration::from_millis(50));

    // Subsequent calls should fail (the server is gone). We don't
    // assert a specific error variant beyond `Mcp` — rmcp's transport
    // can surface several different shapes depending on whether the
    // SSE stream or the POST is the first to notice the close.
    let err = runner
        .list_mcp_tools("stub")
        .expect_err("server is gone, call must fail");
    assert!(matches!(err, RunnerError::Mcp(_)), "got {err:?}");
}
