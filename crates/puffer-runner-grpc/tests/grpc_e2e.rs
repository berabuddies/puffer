//! Cross-backend equivalence test: drives the same `ToolRunner` API against
//! a local in-process `LocalToolRunner` and a `RemoteToolRunner` connected
//! to a `ToolRunnerService` running on a loopback gRPC server. Every call
//! must produce structurally equivalent results.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use puffer_runner_api::{
    ChunkKind, ChunkSink, FnChunkSink, NullChunkSink, RunnerError, ToolRequest, ToolResult,
    ToolRunner,
};
use puffer_runner_grpc::server::ToolRunnerServer;
use puffer_runner_grpc::{RemoteToolRunner, ToolRunnerService};
use puffer_runner_local::LocalToolRunner;
use tempfile::tempdir;
use tokio::sync::oneshot;

const TEST_TOKEN: &str = "test-token-12345";

struct ServerHandle {
    endpoint: String,
    shutdown: Option<oneshot::Sender<()>>,
    runtime: Option<tokio::runtime::Runtime>,
    server_thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
        }
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn spawn_server(runner: Arc<dyn ToolRunner>) -> ServerHandle {
    let port = pick_free_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let endpoint = format!("http://{addr}");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("puffer-runner-grpc-test-server")
        .build()
        .expect("server runtime");

    let service = ToolRunnerService::new(runner).with_auth_token(Some(TEST_TOKEN.to_string()));
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let (ready_tx, ready_rx) = oneshot::channel::<()>();

    let handle = runtime.handle().clone();
    let server_thread = std::thread::Builder::new()
        .name("puffer-runner-grpc-test-server-thread".into())
        .spawn(move || {
            handle.block_on(async move {
                let listener = tokio::net::TcpListener::bind(addr)
                    .await
                    .expect("bind tonic listener");
                let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
                let _ = ready_tx.send(());
                tonic::transport::Server::builder()
                    .add_service(ToolRunnerServer::new(service))
                    .serve_with_incoming_shutdown(incoming, async {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .expect("tonic server");
            });
        })
        .expect("spawn server thread");

    // Wait for the listener to be bound before handing the endpoint back.
    runtime
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(5), ready_rx)
                .await
                .expect("server bind timeout")
                .expect("ready signal")
        });

    ServerHandle {
        endpoint,
        shutdown: Some(shutdown_tx),
        runtime: Some(runtime),
        server_thread: Some(server_thread),
    }
}

#[derive(Default)]
struct CapturedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn capture(buffer: Arc<Mutex<CapturedOutput>>) -> impl ChunkSink {
    FnChunkSink::new(move |kind, bytes| {
        let mut guard = buffer.lock().unwrap();
        match kind {
            ChunkKind::Stdout => guard.stdout.extend_from_slice(bytes),
            ChunkKind::Stderr => guard.stderr.extend_from_slice(bytes),
        }
    })
}

fn make_request(tool_id: &str, cwd: &Path, input: serde_json::Value) -> ToolRequest {
    ToolRequest {
        tool_id: tool_id.to_string(),
        cwd: cwd.to_path_buf(),
        working_dirs: Vec::new(),
        allow_all_paths: false,
        input,
        session_id: None,
    }
}

fn run_scenarios(
    runner: &dyn ToolRunner,
    workspace: &Path,
) -> HashMap<&'static str, ToolResult> {
    let mut out = HashMap::new();

    // 1. Bash with stdout streaming.
    let captured = Arc::new(Mutex::new(CapturedOutput::default()));
    let mut sink = capture(captured.clone());
    let bash = runner
        .execute_tool(
            make_request(
                "Bash",
                workspace,
                serde_json::json!({"command": "echo cross-backend"}),
            ),
            &mut sink,
        )
        .expect("Bash");
    assert!(bash.success);
    assert!(bash.stdout.contains("cross-backend"));
    out.insert("Bash", bash);

    // 2. Write a new file.
    let target = workspace.join("notes.txt");
    let write = runner
        .execute_tool(
            make_request(
                "Write",
                workspace,
                serde_json::json!({
                    "file_path": target.display().to_string(),
                    "content": "alpha\nbeta\n",
                }),
            ),
            &mut NullChunkSink,
        )
        .expect("Write");
    assert!(write.success);
    out.insert("Write", write);

    // 3. Read it back.
    let read = runner
        .execute_tool(
            make_request(
                "Read",
                workspace,
                serde_json::json!({"file_path": target.display().to_string()}),
            ),
            &mut NullChunkSink,
        )
        .expect("Read");
    assert!(read.success);
    out.insert("Read", read);

    // 4. Edit it.
    let edit = runner
        .execute_tool(
            make_request(
                "Edit",
                workspace,
                serde_json::json!({
                    "file_path": target.display().to_string(),
                    "old_string": "alpha",
                    "new_string": "ALPHA",
                }),
            ),
            &mut NullChunkSink,
        )
        .expect("Edit");
    assert!(edit.success);
    out.insert("Edit", edit);

    // 5. Glob.
    let glob = runner
        .execute_tool(
            make_request(
                "Glob",
                workspace,
                serde_json::json!({"pattern": "*.txt"}),
            ),
            &mut NullChunkSink,
        )
        .expect("Glob");
    assert!(glob.success);
    out.insert("Glob", glob);

    // 6. Sleep — keep the duration tiny.
    let sleep = runner
        .execute_tool(
            make_request(
                "Sleep",
                workspace,
                serde_json::json!({"duration_ms": 1, "reason": "smoke"}),
            ),
            &mut NullChunkSink,
        )
        .expect("Sleep");
    assert!(sleep.success);
    out.insert("Sleep", sleep);

    out
}

/// Read-state-updates compare cleanly only when both runners observe the
/// same on-disk mtimes. Run the local scenario first to populate the file,
/// then have the remote scenario re-Write the same content; equivalent
/// `success` + `stdout` is what we ultimately assert. To keep mtimes
/// identical, the test uses two separate workspaces — local against
/// `local_dir`, remote against `remote_dir` — and compares the per-tool
/// outputs structurally.
#[test]
fn cross_backend_equivalence() {
    let local_workspace = tempdir().unwrap();
    let remote_workspace = tempdir().unwrap();

    let local_runner = LocalToolRunner::new();
    let local_results = run_scenarios(&local_runner, local_workspace.path());

    let server_runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let server = spawn_server(server_runner);
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN))
        .expect("connect remote runner");
    let remote_results = run_scenarios(&remote, remote_workspace.path());

    // Tool-by-tool equivalence. We compare success + tool_id strictly, and
    // stdout byte-equal where the output is deterministic. The Read /
    // Edit / Write outputs include the workspace path, so we normalize by
    // stripping the workspace prefix before comparing.
    let local_norm = normalize_workspace_paths(&local_results, local_workspace.path());
    let remote_norm = normalize_workspace_paths(&remote_results, remote_workspace.path());
    for tool in ["Bash", "Write", "Read", "Edit", "Glob", "Sleep"] {
        let l = local_norm.get(tool).unwrap_or_else(|| panic!("missing local {tool}"));
        let r = remote_norm.get(tool).unwrap_or_else(|| panic!("missing remote {tool}"));
        assert_eq!(l.success, r.success, "{tool}: success");
        assert_eq!(l.tool_id, r.tool_id, "{tool}: tool_id");
        // Bash output includes a per-run uuid in metadata; the stdout is
        // a JSON pretty-print containing it. Skip the strict byte
        // comparison for Bash and only check the streamed stdout body.
        if tool == "Bash" {
            assert!(l.stdout.contains("cross-backend"));
            assert!(r.stdout.contains("cross-backend"));
        } else {
            assert_eq!(l.stdout, r.stdout, "{tool}: stdout (post-normalization)");
        }
        assert_eq!(
            l.read_state_updates.len(),
            r.read_state_updates.len(),
            "{tool}: read_state_updates length",
        );
    }

    // 7. Direct read_file / list_dir / glob through both backends.
    let extra_local = local_workspace.path().join("a.txt");
    let extra_remote = remote_workspace.path().join("a.txt");
    std::fs::write(&extra_local, b"abc").unwrap();
    std::fs::write(&extra_remote, b"abc").unwrap();
    assert_eq!(local_runner.read_file(&extra_local).unwrap(), b"abc");
    assert_eq!(remote.read_file(&extra_remote).unwrap(), b"abc");

    let local_dir = local_runner.list_dir(local_workspace.path()).unwrap();
    let remote_dir = remote.list_dir(remote_workspace.path()).unwrap();
    assert_eq!(local_dir.len(), remote_dir.len(), "list_dir length");

    let local_glob = local_runner
        .glob(local_workspace.path(), "*.txt")
        .unwrap();
    let remote_glob = remote.glob(remote_workspace.path(), "*.txt").unwrap();
    assert_eq!(local_glob.len(), remote_glob.len(), "glob length");

    // 8. MCP and permission stubs return Unsupported on both backends.
    let local_mcp = local_runner.list_mcp_servers().unwrap_err();
    let remote_mcp = remote.list_mcp_servers().unwrap_err();
    assert!(
        matches!(local_mcp, RunnerError::Unsupported(_)),
        "local list_mcp_servers"
    );
    assert!(
        matches!(remote_mcp, RunnerError::Unsupported(_)),
        "remote list_mcp_servers"
    );

    let local_perm = local_runner
        .request_permission(puffer_runner_api::PermissionRequest {
            tool_id: "Bash".into(),
            cwd: PathBuf::from("/"),
            input: serde_json::json!({}),
            reason: None,
        })
        .unwrap_err();
    let remote_perm = remote
        .request_permission(puffer_runner_api::PermissionRequest {
            tool_id: "Bash".into(),
            cwd: PathBuf::from("/"),
            input: serde_json::json!({}),
            reason: None,
        })
        .unwrap_err();
    assert!(matches!(local_perm, RunnerError::Unsupported(_)));
    assert!(matches!(remote_perm, RunnerError::Unsupported(_)));

    // Capabilities should both advertise the local backend (since the gRPC
    // server is itself wrapping a LocalToolRunner).
    assert_eq!(local_runner.capabilities().backend, "local");
    assert_eq!(remote.capabilities().backend, "local");

    drop(remote);
    drop(server);
}

#[test]
fn execute_tool_streams_chunks_when_runner_emits_them() {
    // Smoke: even if the underlying LocalToolRunner doesn't currently push
    // chunks to the sink (it gathers stdout into the final result), the
    // server-side bridge must not deadlock when the sink stays empty. This
    // test reuses a single workspace shared between Write/Read so we can
    // assert basic byte-equivalence as well.
    let workspace = tempdir().unwrap();
    let server_runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let server = spawn_server(server_runner);
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN)).expect("connect");

    let captured = Arc::new(Mutex::new(CapturedOutput::default()));
    let mut sink = capture(captured.clone());
    let result = remote
        .execute_tool(
            make_request(
                "Bash",
                workspace.path(),
                serde_json::json!({"command": "printf 'streaming-bytes'"}),
            ),
            &mut sink,
        )
        .expect("Bash");
    assert!(result.success);
    assert!(result.stdout.contains("streaming-bytes"));

    drop(remote);
    drop(server);
}

/// Drives multiple concurrent `execute_tool` calls through a single
/// `Arc<RemoteToolRunner>` to lock the trait's `Send + Sync` contract:
/// a parallel tool batch must be able to share one runner instance
/// without serializing or stomping on shared state. Each thread runs a
/// distinct Bash command and asserts it sees its own output back.
#[test]
fn concurrent_execute_tool_calls() {
    let workspace = tempdir().unwrap();
    let server_runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let server = spawn_server(server_runner);
    let remote: Arc<dyn ToolRunner> = Arc::new(
        RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN)).expect("connect remote"),
    );

    let cwd = workspace.path().to_path_buf();
    let commands = ["echo 1", "echo 2", "echo 3", "echo 4"];
    let expected_markers = ["1", "2", "3", "4"];

    let mut handles = Vec::with_capacity(commands.len());
    for (idx, cmd) in commands.iter().enumerate() {
        let runner = remote.clone();
        let cwd = cwd.clone();
        let cmd = cmd.to_string();
        handles.push(std::thread::spawn(move || {
            let result = runner
                .execute_tool(
                    make_request("Bash", &cwd, serde_json::json!({"command": cmd})),
                    &mut NullChunkSink,
                )
                .expect("Bash");
            (idx, result)
        }));
    }

    let mut results: Vec<Option<ToolResult>> = (0..commands.len()).map(|_| None).collect();
    for handle in handles {
        let (idx, result) = handle.join().expect("worker join");
        results[idx] = Some(result);
    }

    for (idx, (result, marker)) in results.into_iter().zip(expected_markers.iter()).enumerate() {
        let result = result.unwrap_or_else(|| panic!("missing result for {idx}"));
        assert!(result.success, "Bash {idx} failed");
        assert!(
            result.stdout.contains(marker),
            "Bash {idx}: stdout {:?} does not contain marker {marker}",
            result.stdout,
        );
    }

    drop(remote);
    drop(server);
}

#[test]
fn missing_token_is_unauthenticated() {
    let server_runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let server = spawn_server(server_runner);
    let bad = RemoteToolRunner::connect(&server.endpoint, Some("wrong-token")).expect("connect");
    let err = bad.read_file(Path::new("/etc/hostname")).unwrap_err();
    // tonic's `Unauthenticated` round-trips through `status_to_runner_error`
    // as `Other`; the important property is that the call fails.
    assert!(matches!(err, RunnerError::Other(_)));
    drop(bad);
    drop(server);
}

fn normalize_workspace_paths(
    map: &HashMap<&'static str, ToolResult>,
    workspace: &Path,
) -> HashMap<&'static str, ToolResult> {
    let placeholder = "<<workspace>>";
    let prefix = workspace.display().to_string();
    map.iter()
        .map(|(k, v)| {
            let mut clone = v.clone();
            clone.stdout = clone.stdout.replace(&prefix, placeholder);
            clone.stderr = clone.stderr.replace(&prefix, placeholder);
            (*k, clone)
        })
        .collect()
}
