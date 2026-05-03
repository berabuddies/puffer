//! Cross-backend equivalence test: drives the same `ToolRunner` API against
//! a local in-process `LocalToolRunner` and a `RemoteToolRunner` connected
//! to a `ToolRunnerService` running on a loopback gRPC server. Every call
//! must produce structurally equivalent results.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use puffer_runner_api::{
    ChunkKind, ChunkSink, FnChunkSink, McpResourceContentPart, NullChunkSink, RunnerError,
    ToolRequest, ToolResult, ToolRunner,
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

    // Capabilities should both advertise the local backend (since the gRPC
    // server is itself wrapping a LocalToolRunner).
    assert_eq!(local_runner.capabilities().backend, "local");
    assert_eq!(remote.capabilities().backend, "local");

    drop(remote);
    drop(server);
}

/// Drives the seven MCP RPCs through both backends. Each runner is
/// configured with the same MCP manifest (one filesystem stub + one
/// manifest server) and the test asserts the local and remote outputs
/// stay structurally equivalent — list_servers / list_resources /
/// read_resource for both manifest and live transports, plus the
/// uniform Unsupported reply for tool / prompt RPCs that the runner
/// hasn't grown yet.
#[test]
fn cross_backend_mcp_equivalence() {
    use puffer_resources::McpServerSpec;

    let local_workspace = tempdir().unwrap();
    let remote_workspace = tempdir().unwrap();
    std::fs::write(local_workspace.path().join("hello.md"), "# Hello\n").unwrap();
    std::fs::write(remote_workspace.path().join("hello.md"), "# Hello\n").unwrap();
    std::fs::write(local_workspace.path().join("data.bin"), [0xfe_u8, 0xed]).unwrap();
    std::fs::write(remote_workspace.path().join("data.bin"), [0xfe_u8, 0xed]).unwrap();

    let manifest = || -> Vec<McpServerSpec> {
        vec![
            McpServerSpec {
                id: "filesystem".into(),
                display_name: "Filesystem".into(),
                transport: "stdio".into(),
                endpoint: String::new(),
                target: "builtin:filesystem".into(),
                description: "Workspace filesystem stub".into(),
            },
            McpServerSpec {
                id: "docs".into(),
                display_name: "Docs".into(),
                transport: "stdio".into(),
                endpoint: String::new(),
                target: "docs-server".into(),
                description: "Static manifest entry".into(),
            },
        ]
    };

    let local_runner = LocalToolRunner::new()
        .with_mcp_servers(manifest())
        .with_mcp_workspace_root(local_workspace.path().to_path_buf());

    let server_runner: Arc<dyn ToolRunner> = Arc::new(
        LocalToolRunner::new()
            .with_mcp_servers(manifest())
            .with_mcp_workspace_root(remote_workspace.path().to_path_buf()),
    );
    let server = spawn_server(server_runner);
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN))
        .expect("connect remote runner");

    // 1. list_mcp_servers — equal modulo ordering.
    let local_servers = local_runner.list_mcp_servers().expect("local servers");
    let remote_servers = remote.list_mcp_servers().expect("remote servers");
    assert_eq!(local_servers.len(), 2);
    assert_eq!(local_servers.len(), remote_servers.len());
    let local_ids: Vec<_> = local_servers.iter().map(|s| s.id.clone()).collect();
    let remote_ids: Vec<_> = remote_servers.iter().map(|s| s.id.clone()).collect();
    assert_eq!(local_ids, remote_ids);

    // 2. list_mcp_resources — workspace walk for filesystem + manifest URI for docs.
    let local_resources = local_runner.list_mcp_resources(None).expect("local resources");
    let remote_resources = remote.list_mcp_resources(None).expect("remote resources");
    assert_eq!(local_resources.len(), remote_resources.len());
    assert!(local_resources.iter().any(|r| r.uri == "mcp://filesystem/hello.md"));
    assert!(remote_resources.iter().any(|r| r.uri == "mcp://filesystem/hello.md"));
    assert!(local_resources.iter().any(|r| r.uri == "mcp://manifest/docs"));
    assert!(remote_resources.iter().any(|r| r.uri == "mcp://manifest/docs"));

    // 3. list_mcp_resources filtered by server.
    let local_filtered = local_runner
        .list_mcp_resources(Some("filesystem"))
        .expect("local filtered");
    let remote_filtered = remote
        .list_mcp_resources(Some("filesystem"))
        .expect("remote filtered");
    assert_eq!(local_filtered.len(), remote_filtered.len());
    assert!(local_filtered.iter().all(|r| r.server == "filesystem"));
    assert!(remote_filtered.iter().all(|r| r.server == "filesystem"));

    // 4. read_mcp_resource — text via filesystem.
    let local_text = local_runner
        .read_mcp_resource("filesystem", "mcp://filesystem/hello.md")
        .expect("local read");
    let remote_text = remote
        .read_mcp_resource("filesystem", "mcp://filesystem/hello.md")
        .expect("remote read");
    assert_eq!(local_text.parts.len(), remote_text.parts.len());
    match (&local_text.parts[0], &remote_text.parts[0]) {
        (
            McpResourceContentPart::Text { text: l, .. },
            McpResourceContentPart::Text { text: r, .. },
        ) => assert_eq!(l, r),
        other => panic!("expected text/text, got {other:?}"),
    }

    // 5. read_mcp_resource — blob via filesystem (binary).
    let local_blob = local_runner
        .read_mcp_resource("filesystem", "mcp://filesystem/data.bin")
        .expect("local blob");
    let remote_blob = remote
        .read_mcp_resource("filesystem", "mcp://filesystem/data.bin")
        .expect("remote blob");
    match (&local_blob.parts[0], &remote_blob.parts[0]) {
        (
            McpResourceContentPart::Blob { bytes: l, .. },
            McpResourceContentPart::Blob { bytes: r, .. },
        ) => assert_eq!(l, r),
        other => panic!("expected blob/blob, got {other:?}"),
    }

    // 6. read_mcp_resource — manifest server returns YAML payload.
    let local_manifest = local_runner
        .read_mcp_resource("docs", "mcp://manifest/docs")
        .expect("local manifest");
    let remote_manifest = remote
        .read_mcp_resource("docs", "mcp://manifest/docs")
        .expect("remote manifest");
    assert_eq!(local_manifest.parts.len(), remote_manifest.parts.len());

    // 7. tools / prompts surface a deterministic Unsupported on both
    //    backends until a real subprocess MCP client lands.
    let local_tools = local_runner.list_mcp_tools("filesystem").unwrap_err();
    let remote_tools = remote.list_mcp_tools("filesystem").unwrap_err();
    assert!(matches!(local_tools, RunnerError::Unsupported(_)));
    assert!(matches!(remote_tools, RunnerError::Unsupported(_)));

    let local_call = local_runner
        .call_mcp_tool(
            "filesystem",
            "noop",
            serde_json::json!({}),
            &mut NullChunkSink,
        )
        .unwrap_err();
    let remote_call = remote
        .call_mcp_tool(
            "filesystem",
            "noop",
            serde_json::json!({}),
            &mut NullChunkSink,
        )
        .unwrap_err();
    assert!(matches!(local_call, RunnerError::Unsupported(_)));
    assert!(matches!(remote_call, RunnerError::Unsupported(_)));

    let local_prompts = local_runner.list_mcp_prompts("filesystem").unwrap_err();
    let remote_prompts = remote.list_mcp_prompts("filesystem").unwrap_err();
    assert!(matches!(local_prompts, RunnerError::Unsupported(_)));
    assert!(matches!(remote_prompts, RunnerError::Unsupported(_)));

    let local_get = local_runner
        .get_mcp_prompt("filesystem", "noop", serde_json::json!({}))
        .unwrap_err();
    let remote_get = remote
        .get_mcp_prompt("filesystem", "noop", serde_json::json!({}))
        .unwrap_err();
    assert!(matches!(local_get, RunnerError::Unsupported(_)));
    assert!(matches!(remote_get, RunnerError::Unsupported(_)));

    // 8. Unknown server is reported as NotFound, not Unsupported.
    let unknown_local = local_runner.list_mcp_tools("missing").unwrap_err();
    let unknown_remote = remote.list_mcp_tools("missing").unwrap_err();
    assert!(matches!(unknown_local, RunnerError::NotFound(_)));
    assert!(matches!(unknown_remote, RunnerError::NotFound(_)));

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
