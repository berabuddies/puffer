//! Drives `LocalToolRunner` against the `puffer-mcp-stub-server` binary
//! over stdio. Exercises the lazy-spawn path, normal `tools/list` /
//! `tools/call` round-trips, crash recovery via the bounded-retry budget,
//! and the fast-fail path when the configured binary cannot start.

use std::time::Duration;

use puffer_core::runner_adapter::LocalToolRunner;
use puffer_resources::McpServerSpec;
use puffer_runner_api::{NullChunkSink, RunnerError, ToolRunner};
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
    }]
}

#[test]
fn lazy_spawn_does_not_run_until_first_call() {
    let marker = "puffer-mcp-stub-lazy-spawn-marker";
    let runner = LocalToolRunner::new().with_mcp_servers(manifest_with_marker("stub", marker));

    // Construction alone should not spawn anything.
    assert_eq!(count_stub_processes(marker), 0, "no children before first call");

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
    let runner = LocalToolRunner::new()
        .with_mcp_servers(manifest_with_marker("stub", "puffer-mcp-stub-echo-round-trip"));
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
    assert!(!is_pid_alive(first_pid), "stub should have exited after crash");

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
