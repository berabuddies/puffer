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
    spawn_server_on_port(runner, port)
}

fn spawn_server_on_port(runner: Arc<dyn ToolRunner>, port: u16) -> ServerHandle {
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

/// Drives the MCP RPCs that target the built-in `filesystem` transport
/// through both backends. The filesystem stub still walks the workspace
/// root in-process, so its outputs must stay structurally equivalent
/// across `LocalToolRunner` and `RemoteToolRunner`. Tools / prompts
/// remain `Unsupported` for the filesystem stub itself; subprocess MCP
/// servers are exercised separately by `cross_backend_real_mcp_*`.
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
        vec![McpServerSpec {
            id: "filesystem".into(),
            display_name: "Filesystem".into(),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: "builtin:filesystem".into(),
            description: "Workspace filesystem stub".into(),
        }]
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
    assert_eq!(local_servers.len(), 1);
    assert_eq!(local_servers.len(), remote_servers.len());
    let local_ids: Vec<_> = local_servers.iter().map(|s| s.id.clone()).collect();
    let remote_ids: Vec<_> = remote_servers.iter().map(|s| s.id.clone()).collect();
    assert_eq!(local_ids, remote_ids);

    // 2. list_mcp_resources walks the filesystem stub's workspace root.
    let local_resources = local_runner.list_mcp_resources(None).expect("local resources");
    let remote_resources = remote.list_mcp_resources(None).expect("remote resources");
    assert_eq!(local_resources.len(), remote_resources.len());
    assert!(local_resources.iter().any(|r| r.uri == "mcp://filesystem/hello.md"));
    assert!(remote_resources.iter().any(|r| r.uri == "mcp://filesystem/hello.md"));

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

    // 6. tools / prompts on the built-in filesystem stub still surface a
    //    deterministic Unsupported on both backends.
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

    // 7. Unknown server is reported as NotFound, not Unsupported.
    let unknown_local = local_runner.list_mcp_tools("missing").unwrap_err();
    let unknown_remote = remote.list_mcp_tools("missing").unwrap_err();
    assert!(matches!(unknown_local, RunnerError::NotFound(_)));
    assert!(matches!(unknown_remote, RunnerError::NotFound(_)));

    drop(remote);
    drop(server);
}

/// Locates `puffer-mcp-stub-server` next to the running test binary. Cargo
/// only exposes `CARGO_BIN_EXE_*` to integration tests inside the package
/// that owns the bin (i.e. `puffer-core`), so this peer crate has to walk
/// `current_exe` up to the build dir manually — and invoke cargo to build
/// the bin if it doesn't already exist (typical on a clean checkout).
fn locate_stub_binary() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    let mut dir = exe.parent().expect("test bin parent").to_path_buf();
    let bin_name = if cfg!(windows) {
        "puffer-mcp-stub-server.exe"
    } else {
        "puffer-mcp-stub-server"
    };
    // `current_exe` is `<target>/<profile>/deps/grpc_e2e-XXX`; the stub
    // lives one directory up at `<target>/<profile>/puffer-mcp-stub-server`.
    if dir.file_name().and_then(|s| s.to_str()) == Some("deps") {
        dir.pop();
    }
    let candidate = dir.join(bin_name);
    if !candidate.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = std::process::Command::new(cargo)
            .args(["build", "-p", "puffer-core", "--bin", "puffer-mcp-stub-server"])
            .status()
            .expect("build puffer-mcp-stub-server");
        assert!(status.success(), "cargo build of stub bin failed");
    }
    assert!(
        candidate.exists(),
        "stub binary missing at {} after build attempt",
        candidate.display()
    );
    candidate
}

/// Drives `tools/list` and `tools/call` through both backends against the
/// real `puffer-mcp-stub-server` binary and asserts the results round-trip
/// byte-equal between local and remote.
#[test]
fn cross_backend_real_mcp_tools() {
    use puffer_resources::McpServerSpec;

    let stub_bin = locate_stub_binary();
    let manifest = || -> Vec<McpServerSpec> {
        vec![McpServerSpec {
            id: "stub".into(),
            display_name: "Stub".into(),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: format!("'{}' --marker puffer-mcp-grpc-cross-backend", stub_bin.display()),
            description: "Integration-test stub MCP server".into(),
        }]
    };

    let local_runner = LocalToolRunner::new().with_mcp_servers(manifest());
    let server_runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new().with_mcp_servers(manifest()));
    let server = spawn_server(server_runner);
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN))
        .expect("connect remote runner");

    // tools/list — both backends agree on names and order.
    let local_tools = local_runner.list_mcp_tools("stub").expect("local tools");
    let remote_tools = remote.list_mcp_tools("stub").expect("remote tools");
    let local_names: Vec<_> = local_tools.iter().map(|t| t.name.clone()).collect();
    let remote_names: Vec<_> = remote_tools.iter().map(|t| t.name.clone()).collect();
    assert_eq!(local_names, remote_names);
    assert!(local_names.contains(&"echo".to_string()));
    assert!(local_names.contains(&"slow_echo".to_string()));

    // tools/call echo — byte-equal payloads.
    let mut sink = NullChunkSink;
    let local_echo = local_runner
        .call_mcp_tool("stub", "echo", serde_json::json!({"text": "ping"}), &mut sink)
        .expect("local echo");
    let remote_echo = remote
        .call_mcp_tool("stub", "echo", serde_json::json!({"text": "ping"}), &mut sink)
        .expect("remote echo");
    assert_eq!(local_echo.success, remote_echo.success);
    assert_eq!(local_echo.stdout, remote_echo.stdout);
    assert_eq!(local_echo.stdout, "ping");

    // tools/call slow_echo — same equivalence with a small delay.
    let local_slow = local_runner
        .call_mcp_tool(
            "stub",
            "slow_echo",
            serde_json::json!({"text": "delayed", "delay_ms": 30}),
            &mut sink,
        )
        .expect("local slow_echo");
    let remote_slow = remote
        .call_mcp_tool(
            "stub",
            "slow_echo",
            serde_json::json!({"text": "delayed", "delay_ms": 30}),
            &mut sink,
        )
        .expect("remote slow_echo");
    assert_eq!(local_slow.stdout, remote_slow.stdout);
    assert_eq!(local_slow.stdout, "delayed");

    drop(remote);
    drop(server);
}

/// Drives `resources/list`, `resources/read` (text + blob),
/// `prompts/list`, and `prompts/get` through both backends against the
/// real `puffer-mcp-stub-server`. Asserts byte-equal results between the
/// in-process and gRPC paths.
#[test]
fn cross_backend_real_mcp_resources_and_prompts() {
    use puffer_resources::McpServerSpec;

    let stub_bin = locate_stub_binary();
    let manifest = || -> Vec<McpServerSpec> {
        vec![McpServerSpec {
            id: "stub".into(),
            display_name: "Stub".into(),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: format!(
                "'{}' --marker puffer-mcp-grpc-cross-backend-resources",
                stub_bin.display()
            ),
            description: "Integration-test stub MCP server".into(),
        }]
    };

    let local_runner = LocalToolRunner::new().with_mcp_servers(manifest());
    let server_runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new().with_mcp_servers(manifest()));
    let server = spawn_server(server_runner);
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN))
        .expect("connect remote runner");

    // resources/list — same URIs, names, mime types in the same order.
    let local_resources = local_runner.list_mcp_resources(Some("stub")).expect("local list_resources");
    let remote_resources = remote.list_mcp_resources(Some("stub")).expect("remote list_resources");
    let local_uris: Vec<_> = local_resources.iter().map(|r| r.uri.clone()).collect();
    let remote_uris: Vec<_> = remote_resources.iter().map(|r| r.uri.clone()).collect();
    assert_eq!(local_uris, remote_uris);
    assert!(local_uris.contains(&"stub://hello.txt".to_string()));
    assert!(local_uris.contains(&"stub://binary.bin".to_string()));

    // resources/read text — payload byte-equal across backends.
    let local_text = local_runner
        .read_mcp_resource("stub", "stub://hello.txt")
        .expect("local read text");
    let remote_text = remote
        .read_mcp_resource("stub", "stub://hello.txt")
        .expect("remote read text");
    match (local_text.parts.first(), remote_text.parts.first()) {
        (
            Some(McpResourceContentPart::Text { text: l, .. }),
            Some(McpResourceContentPart::Text { text: r, .. }),
        ) => {
            assert_eq!(l, r);
            assert_eq!(l, "hello from stub");
        }
        other => panic!("expected text/text parts, got {other:?}"),
    }

    // resources/read blob — bytes byte-equal across backends.
    let local_blob = local_runner
        .read_mcp_resource("stub", "stub://binary.bin")
        .expect("local read blob");
    let remote_blob = remote
        .read_mcp_resource("stub", "stub://binary.bin")
        .expect("remote read blob");
    match (local_blob.parts.first(), remote_blob.parts.first()) {
        (
            Some(McpResourceContentPart::Blob { bytes: l, .. }),
            Some(McpResourceContentPart::Blob { bytes: r, .. }),
        ) => {
            assert_eq!(l, r);
            assert_eq!(l, &vec![0xde, 0xad, 0xbe]);
        }
        other => panic!("expected blob/blob parts, got {other:?}"),
    }

    // prompts/list — same prompt names + arguments.
    let local_prompts = local_runner.list_mcp_prompts("stub").expect("local list_prompts");
    let remote_prompts = remote.list_mcp_prompts("stub").expect("remote list_prompts");
    let local_names: Vec<_> = local_prompts.iter().map(|p| p.name.clone()).collect();
    let remote_names: Vec<_> = remote_prompts.iter().map(|p| p.name.clone()).collect();
    assert_eq!(local_names, remote_names);
    assert_eq!(local_names, vec!["greet".to_string()]);

    // prompts/get — rendered text identical across backends.
    let local_get = local_runner
        .get_mcp_prompt("stub", "greet", serde_json::json!({"name": "remote"}))
        .expect("local get_prompt");
    let remote_get = remote
        .get_mcp_prompt("stub", "greet", serde_json::json!({"name": "remote"}))
        .expect("remote get_prompt");
    assert_eq!(local_get.messages.len(), remote_get.messages.len());
    assert_eq!(local_get.messages[0].text, remote_get.messages[0].text);
    assert_eq!(local_get.messages[0].text, "Hello, remote!");
    assert_eq!(local_get.messages[0].role, remote_get.messages[0].role);

    drop(remote);
    drop(server);
}

/// Recording sink used by the cross-backend progress test below. Keeps
/// every `event` call so the assertion can confirm the gRPC server-side
/// bridge forwarded `notifications/progress` envelopes through the
/// streaming response.
#[derive(Default, Clone)]
struct GrpcRecordingSink {
    events: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl ChunkSink for GrpcRecordingSink {
    fn stdout(&mut self, _chunk: &[u8]) {}
    fn stderr(&mut self, _chunk: &[u8]) {}
    fn event(&mut self, event: serde_json::Value) {
        self.events.lock().unwrap().push(event);
    }
}

#[test]
fn cross_backend_progress_notifications_round_trip() {
    use puffer_resources::McpServerSpec;

    let stub_bin = locate_stub_binary();
    let manifest = || -> Vec<McpServerSpec> {
        vec![McpServerSpec {
            id: "stub".into(),
            display_name: "Stub".into(),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: format!(
                "'{}' --marker puffer-mcp-grpc-cross-backend-progress",
                stub_bin.display()
            ),
            description: "Integration-test stub MCP server".into(),
        }]
    };

    let server_runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new().with_mcp_servers(manifest()));
    let server = spawn_server(server_runner);
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN))
        .expect("connect remote runner");

    let sink = GrpcRecordingSink::default();
    let events_handle = sink.events.clone();
    let mut sink = sink;
    let result = remote
        .call_mcp_tool(
            "stub",
            "slow_with_progress",
            serde_json::json!({"text": "remote-progress", "delay_ms": 25}),
            &mut sink,
        )
        .expect("slow_with_progress");
    assert!(result.success);
    assert_eq!(result.stdout, "remote-progress");
    let events = events_handle.lock().unwrap();
    assert!(
        !events.is_empty(),
        "expected at least one progress event over gRPC, got none"
    );
    for event in events.iter() {
        assert_eq!(event.get("kind").and_then(|v| v.as_str()), Some("mcp/progress"));
    }

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

/// Mirrors the backoff sequence baked into `select_tool_runner`. Kept
/// inline here so the resilience tests don't reach across crates.
fn ping_until_alive(runner: &dyn ToolRunner, deadline: Duration) -> bool {
    let start = std::time::Instant::now();
    let mut delay = Duration::from_millis(50);
    while start.elapsed() < deadline {
        if runner.ping().is_ok() {
            return true;
        }
        std::thread::sleep(delay);
        delay = std::cmp::min(delay * 2, Duration::from_millis(500));
    }
    false
}

#[test]
fn ping_returns_version() {
    let server_runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let server = spawn_server(server_runner);
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN)).expect("connect");

    let ping = remote.ping().expect("ping ok");
    assert!(!ping.version.is_empty(), "version should be non-empty");
    // The server has just started; uptime is bounded by the test runtime.
    assert!(ping.uptime < Duration::from_secs(60), "uptime sanity");

    drop(remote);
    drop(server);
}

#[test]
fn connect_retries_until_runner_ready() {
    // Pick a port up front, build the runner against an offline endpoint,
    // and spawn the server only after a delay. The lazy channel + Ping
    // retry loop must reach the runner once it comes up.
    let port = pick_free_port();
    let endpoint = format!("http://127.0.0.1:{port}");

    let server_slot: Arc<Mutex<Option<ServerHandle>>> = Arc::new(Mutex::new(None));
    let slot = server_slot.clone();
    let starter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        let server_runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
        let handle = spawn_server_on_port(server_runner, port);
        *slot.lock().unwrap() = Some(handle);
    });

    let remote = RemoteToolRunner::connect(&endpoint, Some(TEST_TOKEN))
        .expect("connect (lazy) returns immediately");
    assert!(
        ping_until_alive(&remote, Duration::from_secs(3)),
        "ping never succeeded within 3s"
    );

    starter.join().expect("starter thread");
    drop(remote);
    drop(server_slot);
}

#[test]
fn survives_runner_restart_mid_session() {
    // First boot.
    let port = pick_free_port();
    let server_runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let server = spawn_server_on_port(server_runner, port);
    let endpoint = server.endpoint.clone();
    let remote = RemoteToolRunner::connect(&endpoint, Some(TEST_TOKEN)).expect("connect");
    let workspace = tempdir().unwrap();

    let first = remote
        .execute_tool(
            make_request(
                "Bash",
                workspace.path(),
                serde_json::json!({"command": "echo hello"}),
            ),
            &mut NullChunkSink,
        )
        .expect("first Bash");
    assert!(first.success);
    assert!(first.stdout.contains("hello"));

    // Tear the server down and wait for the port to free up.
    drop(server);
    // Give the OS a moment to release the bound port.
    std::thread::sleep(Duration::from_millis(200));

    // Re-bind on the same port. If this fails because of TIME_WAIT, the
    // test exits cleanly with an explanation rather than flaking.
    let restart_runner: Arc<dyn ToolRunner> = Arc::new(LocalToolRunner::new());
    let server2 = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        spawn_server_on_port(restart_runner, port)
    })) {
        Ok(handle) => handle,
        Err(_) => {
            eprintln!(
                "survives_runner_restart_mid_session: could not rebind port {port} \
                 immediately after shutdown; treating as flake-skip"
            );
            return;
        }
    };

    // Wait for the new server to answer Ping. The lazy channel will
    // reconnect under the hood, and the per-call `Unavailable` retry
    // covers the brief window where the connection is half-open.
    assert!(
        ping_until_alive(&remote, Duration::from_secs(3)),
        "remote runner never became reachable after restart"
    );

    let second = remote
        .execute_tool(
            make_request(
                "Bash",
                workspace.path(),
                serde_json::json!({"command": "echo world"}),
            ),
            &mut NullChunkSink,
        )
        .expect("second Bash after restart");
    assert!(second.success);
    assert!(second.stdout.contains("world"));

    drop(remote);
    drop(server2);
}
