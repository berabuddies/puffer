//! End-to-end test for `LocalToolRunner`.
//!
//! Drives the runner through a Bash + Read + Write + Edit + Glob + Sleep
//! sequence and a separate dispatcher-style staleness check, mirroring how
//! the runtime will use it once the trait is wired into
//! `runtime::tool_executor`.

use puffer_resources::McpServerSpec;
use puffer_runner_api::{
    check_read_freshness, ChunkKind, ChunkSink, FilesystemExecutionPolicy, FilesystemSandboxMode,
    FnChunkSink, McpResourceContentPart, NullChunkSink, ReadStateSnapshot, ReadStateUpdate,
    RunnerError, StalenessRejection, ToolRequest, ToolResult, ToolRunner,
};
use puffer_runner_local::LocalToolRunner;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use tempfile::tempdir;

fn make_request(tool_id: &str, cwd: &Path, input: serde_json::Value) -> ToolRequest {
    make_request_with_mode(tool_id, cwd, FilesystemSandboxMode::WorkspaceWrite, input)
}

fn make_request_with_mode(
    tool_id: &str,
    cwd: &Path,
    sandbox_mode: FilesystemSandboxMode,
    input: serde_json::Value,
) -> ToolRequest {
    ToolRequest {
        tool_id: tool_id.to_string(),
        cwd: cwd.to_path_buf(),
        working_dirs: Vec::new(),
        filesystem: FilesystemExecutionPolicy { sandbox_mode },
        input,
        session_id: Some(uuid::Uuid::new_v4().to_string()),
    }
}

fn file_mtime_ms(path: &Path) -> u128 {
    fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

#[derive(Debug, Default)]
struct CapturedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn capture_into(buffer: Arc<Mutex<CapturedOutput>>) -> impl ChunkSink {
    FnChunkSink::new(move |kind, bytes| {
        let mut guard = buffer.lock().unwrap();
        match kind {
            ChunkKind::Stdout => guard.stdout.extend_from_slice(bytes),
            ChunkKind::Stderr => guard.stderr.extend_from_slice(bytes),
        }
    })
}

#[test]
fn full_tool_sequence_runs_through_runner() {
    let temp = tempdir().unwrap();
    let runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let mut read_state: HashMap<PathBuf, ReadStateSnapshot> = HashMap::new();

    // 1. Bash — runs a real shell command and captures stdout.
    let buffer = Arc::new(Mutex::new(CapturedOutput::default()));
    let mut sink = capture_into(buffer.clone());
    let bash_result = runner
        .execute_tool(
            make_request(
                "Bash",
                temp.path(),
                serde_json::json!({
                    "command": "echo hello-from-bash",
                }),
            ),
            &mut sink,
        )
        .expect("Bash should succeed");
    assert!(bash_result.success);
    assert!(bash_result.stdout.contains("hello-from-bash"));
    assert!(bash_result.read_state_updates.is_empty());

    // 2. Write — creates a brand-new file, and reports back the mtime.
    let target = temp.path().join("notes.txt");
    let write_result = runner
        .execute_tool(
            make_request(
                "Write",
                temp.path(),
                serde_json::json!({
                    "file_path": target.display().to_string(),
                    "content": "first line\nsecond line\n",
                }),
            ),
            &mut NullChunkSink,
        )
        .expect("Write should succeed");
    assert!(write_result.success);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "first line\nsecond line\n"
    );
    apply_updates(&mut read_state, &write_result);
    assert_state_tracked(&read_state, &target, /* partial */ false);

    // 3. Read — populates the dispatcher's read-state map for later edits.
    let read_result = runner
        .execute_tool(
            make_request(
                "Read",
                temp.path(),
                serde_json::json!({
                    "file_path": target.display().to_string(),
                }),
            ),
            &mut NullChunkSink,
        )
        .expect("Read should succeed");
    assert!(read_result.success);
    assert!(read_result.stdout.contains("first line"));
    apply_updates(&mut read_state, &read_result);
    assert_state_tracked(&read_state, &target, /* partial */ false);

    // 4. Edit — the dispatcher's pre-flight gate sees a fresh read-state
    //    snapshot, lets the call through, and re-records the new mtime.
    let snapshot = read_state.get(&target).cloned();
    check_read_freshness(snapshot.as_ref(), file_mtime_ms(&target))
        .expect("freshness gate should accept a just-Read file");
    let edit_result = runner
        .execute_tool(
            make_request(
                "Edit",
                temp.path(),
                serde_json::json!({
                    "file_path": target.display().to_string(),
                    "old_string": "first line",
                    "new_string": "FIRST LINE",
                }),
            ),
            &mut NullChunkSink,
        )
        .expect("Edit should succeed");
    assert!(edit_result.success);
    assert!(fs::read_to_string(&target)
        .unwrap()
        .starts_with("FIRST LINE"));
    apply_updates(&mut read_state, &edit_result);

    // 5. Glob — pure read-only listing, no read-state updates.
    let glob_result = runner
        .execute_tool(
            make_request(
                "Glob",
                temp.path(),
                serde_json::json!({
                    "pattern": "*.txt",
                }),
            ),
            &mut NullChunkSink,
        )
        .expect("Glob should succeed");
    assert!(glob_result.success);
    assert!(glob_result.stdout.contains("notes.txt"));
    assert!(glob_result.read_state_updates.is_empty());

    // 6. Sleep — keep duration tiny so the test stays fast.
    let sleep_result = runner
        .execute_tool(
            make_request(
                "Sleep",
                temp.path(),
                serde_json::json!({
                    "duration_ms": 1,
                    "reason": "smoke",
                }),
            ),
            &mut NullChunkSink,
        )
        .expect("Sleep should succeed");
    assert!(sleep_result.success);
    assert!(sleep_result.stdout.contains("\"completed\": true"));
}

#[test]
fn dispatcher_rejects_edit_without_prior_read() {
    // Demonstrates the staleness gate: the dispatcher refuses to forward
    // an Edit for a file that was never registered in its read-state map,
    // mirroring the in-process check that used to live inside the tool.
    let temp = tempdir().unwrap();
    let runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let read_state: HashMap<PathBuf, ReadStateSnapshot> = HashMap::new();

    let target = temp.path().join("untracked.txt");
    fs::write(&target, "content\n").unwrap();

    let snapshot = read_state.get(&target).cloned();
    let rejection = check_read_freshness(snapshot.as_ref(), file_mtime_ms(&target))
        .expect_err("Edit on un-tracked file must be rejected");
    assert_eq!(rejection, StalenessRejection::NotRead);

    // The runner itself never saw the call — caller never invoked it.
    let must_not_have_run = runner.capabilities();
    assert_eq!(must_not_have_run.backend, "local");
}

/// Pinned in 2026-05-08: a partial Read (offset/limit) used to surface
/// as `StalenessRejection::NotRead` with the misleading message
/// "File has not been read yet" — confusing the model into retrying
/// the same Edit unchanged. Now distinct as `PartialRead` with a
/// message that tells the model exactly what to do.
/// Trajectory anchor: 2026-04-12 `torch-tensor-parallelism` step 25+.
#[test]
fn partial_read_is_rejected_distinctly_from_not_read() {
    let temp = tempdir().unwrap();
    let target = temp.path().join("partial.txt");
    fs::write(&target, "v1\n").unwrap();

    // Simulate a partial-view snapshot (Read with offset/limit set).
    let mut read_state: HashMap<PathBuf, ReadStateSnapshot> = HashMap::new();
    read_state.insert(
        target.clone(),
        ReadStateSnapshot {
            timestamp_ms: file_mtime_ms(&target),
            is_partial_view: true,
        },
    );

    let snapshot = read_state.get(&target).cloned();
    let rejection = check_read_freshness(snapshot.as_ref(), file_mtime_ms(&target))
        .expect_err("Edit after partial Read must still be rejected");
    assert_eq!(rejection, StalenessRejection::PartialRead);
    assert!(
        rejection
            .message()
            .contains("partially")
            || rejection.message().contains("offset")
            || rejection.message().contains("limit"),
        "PartialRead message must mention partial / offset / limit so the model knows what to do; got: {}",
        rejection.message()
    );
    assert_ne!(
        rejection.message(),
        StalenessRejection::NOT_READ_MESSAGE,
        "PartialRead must not reuse the misleading NotRead message"
    );
}

#[test]
fn dispatcher_rejects_edit_when_file_changed_after_read() {
    let temp = tempdir().unwrap();
    let runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let mut read_state: HashMap<PathBuf, ReadStateSnapshot> = HashMap::new();

    let target = temp.path().join("watched.txt");
    fs::write(&target, "v1\n").unwrap();

    // Simulate a Read first, then an external mutation that bumps mtime
    // (sleep + rewrite — granular enough on every supported OS).
    let read_result = runner
        .execute_tool(
            make_request(
                "Read",
                temp.path(),
                serde_json::json!({"file_path": target.display().to_string()}),
            ),
            &mut NullChunkSink,
        )
        .unwrap();
    apply_updates(&mut read_state, &read_result);

    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(&target, "v2\n").unwrap();

    let snapshot = read_state.get(&target).cloned();
    let rejection = check_read_freshness(snapshot.as_ref(), file_mtime_ms(&target))
        .expect_err("Edit must be rejected after external mutation");
    assert_eq!(rejection, StalenessRejection::StaleRead);
}

#[test]
fn workspace_write_runner_request_rejects_filesystem_escape() {
    let temp = tempdir().unwrap();
    let escaped = PathBuf::from("/__puffer_test_outside_writable_set__/secret.txt");

    let runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let error = runner
        .execute_tool(
            make_request(
                "Read",
                temp.path(),
                serde_json::json!({
                    "file_path": escaped.display().to_string(),
                }),
            ),
            &mut NullChunkSink,
        )
        .unwrap_err()
        .to_string();

    assert!(error.contains("outside the current working directories"));
}

#[test]
fn danger_full_access_runner_request_allows_filesystem_escape() {
    let temp = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let escaped = outside.path().join("secret.txt");
    fs::write(&escaped, "secret\n").unwrap();

    let runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let result = runner
        .execute_tool(
            make_request_with_mode(
                "Read",
                temp.path(),
                FilesystemSandboxMode::DangerFullAccess,
                serde_json::json!({
                    "file_path": escaped.display().to_string(),
                }),
            ),
            &mut NullChunkSink,
        )
        .expect("danger-full-access should allow out-of-workspace paths");

    assert!(result.success);
    assert!(result.stdout.contains("secret"));
}

fn fixture_manifest() -> Vec<McpServerSpec> {
    vec![
        McpServerSpec {
            id: "filesystem".into(),
            display_name: "Filesystem".into(),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: "builtin:filesystem".into(),
            description: "Workspace filesystem stub".into(),
            headers: Default::default(),
            oauth: None,
        },
        McpServerSpec {
            id: "docs".into(),
            display_name: "Docs".into(),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: "docs-server".into(),
            description: "Static manifest entry".into(),
            headers: Default::default(),
            oauth: None,
        },
    ]
}

#[test]
fn list_mcp_servers_returns_configured_manifest() {
    let runner = LocalToolRunner::new().with_mcp_servers(fixture_manifest());
    let servers = runner.list_mcp_servers().unwrap();
    let ids: Vec<_> = servers.iter().map(|s| s.id.clone()).collect();
    assert_eq!(ids, vec!["filesystem".to_string(), "docs".to_string()]);
}

#[test]
fn list_mcp_tools_is_unsupported_until_real_client_lands() {
    let runner = LocalToolRunner::new().with_mcp_servers(fixture_manifest());
    let err = runner.list_mcp_tools("filesystem").unwrap_err();
    assert!(matches!(err, RunnerError::Unsupported(_)));
}

#[test]
fn call_mcp_tool_is_unsupported_until_real_client_lands() {
    let runner = LocalToolRunner::new().with_mcp_servers(fixture_manifest());
    let mut sink = NullChunkSink;
    let err = runner
        .call_mcp_tool("filesystem", "noop", serde_json::json!({}), &mut sink)
        .unwrap_err();
    assert!(matches!(err, RunnerError::Unsupported(_)));
}

#[test]
fn list_mcp_resources_walks_filesystem_workspace() {
    // Subprocess MCP servers now route through the connection manager and
    // answer their own resources, so this test scope is just the built-in
    // filesystem walker. The cross-backend tests in `puffer-runner-grpc`
    // exercise the real subprocess path against `puffer-mcp-stub-server`.
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("guide.md"), "# Guide\n").unwrap();
    fs::create_dir_all(temp.path().join("nested")).unwrap();
    fs::write(temp.path().join("nested/hello.txt"), "hi").unwrap();
    let manifest = vec![McpServerSpec {
        id: "filesystem".into(),
        display_name: "Filesystem".into(),
        transport: "stdio".into(),
        endpoint: String::new(),
        target: "builtin:filesystem".into(),
        description: "Workspace filesystem stub".into(),
        headers: Default::default(),
        oauth: None,
    }];
    let runner = LocalToolRunner::new()
        .with_mcp_servers(manifest)
        .with_mcp_workspace_root(temp.path().to_path_buf());
    let records = runner.list_mcp_resources(None).unwrap();
    assert!(records
        .iter()
        .any(|r| r.uri == "mcp://filesystem/guide.md" && r.server == "filesystem"));
    assert!(records
        .iter()
        .any(|r| r.uri == "mcp://filesystem/nested/hello.txt"));
}

#[test]
fn read_mcp_resource_returns_text_for_workspace_files() {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("guide.md"), "# Guide\n").unwrap();
    let runner = LocalToolRunner::new()
        .with_mcp_servers(fixture_manifest())
        .with_mcp_workspace_root(temp.path().to_path_buf());
    let content = runner
        .read_mcp_resource("filesystem", "mcp://filesystem/guide.md")
        .unwrap();
    assert_eq!(content.parts.len(), 1);
    match &content.parts[0] {
        McpResourceContentPart::Text {
            text, mime_type, ..
        } => {
            assert_eq!(text, "# Guide\n");
            assert_eq!(mime_type.as_deref(), Some("text/markdown"));
        }
        other => panic!("expected text part, got {other:?}"),
    }
}

#[test]
fn read_mcp_resource_unknown_server_is_not_found() {
    let runner = LocalToolRunner::new().with_mcp_servers(fixture_manifest());
    let err = runner
        .read_mcp_resource("missing", "mcp://manifest/docs")
        .unwrap_err();
    assert!(matches!(err, RunnerError::NotFound(_)));
}

fn apply_updates(state: &mut HashMap<PathBuf, ReadStateSnapshot>, result: &ToolResult) {
    for ReadStateUpdate {
        path,
        timestamp_ms,
        is_partial_view,
    } in &result.read_state_updates
    {
        state.insert(
            path.clone(),
            ReadStateSnapshot {
                timestamp_ms: *timestamp_ms,
                is_partial_view: *is_partial_view,
            },
        );
    }
}

fn assert_state_tracked(
    state: &HashMap<PathBuf, ReadStateSnapshot>,
    path: &Path,
    expected_partial: bool,
) {
    let snapshot = state
        .get(path)
        .unwrap_or_else(|| panic!("expected read-state to track {}", path.display()));
    assert_eq!(snapshot.is_partial_view, expected_partial);
    assert!(snapshot.timestamp_ms > 0);
}
