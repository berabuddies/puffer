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
    ChunkKind, ChunkSink, ElicitationHandler, ElicitationRequest, ElicitationResponse, FnChunkSink,
    McpResourceContentPart, NullChunkSink, OAuthStatus, OAuthTokensPayload, RunnerError,
    ToolRequest, ToolResult, ToolRunner,
};
use puffer_runner_grpc::server::ToolRunnerServer;
use puffer_runner_grpc::{BidiElicitationRouter, RemoteToolRunner, ToolRunnerService};
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

/// Like [`spawn_server`] but installs a custom `BidiElicitationRouter` on
/// the service so the caller can pre-install it on the underlying runner
/// (server-side MCP elicitations route through this instance).
fn spawn_server_with_router(
    runner: Arc<dyn ToolRunner>,
    router: Arc<BidiElicitationRouter>,
) -> ServerHandle {
    let port = pick_free_port();
    spawn_server_on_port_with_router(runner, port, Some(router))
}

fn spawn_server_on_port(runner: Arc<dyn ToolRunner>, port: u16) -> ServerHandle {
    spawn_server_on_port_with_router(runner, port, None)
}

fn spawn_server_on_port_with_router(
    runner: Arc<dyn ToolRunner>,
    port: u16,
    router: Option<Arc<BidiElicitationRouter>>,
) -> ServerHandle {
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let endpoint = format!("http://{addr}");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("puffer-runner-grpc-test-server")
        .build()
        .expect("server runtime");

    let service = match router {
        Some(router) => ToolRunnerService::with_router(runner, router),
        None => ToolRunnerService::new(runner),
    }
    .with_auth_token(Some(TEST_TOKEN.to_string()));
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
    runtime.block_on(async {
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
        filesystem: puffer_runner_api::FilesystemExecutionPolicy {
            sandbox_mode: puffer_runner_api::FilesystemSandboxMode::WorkspaceWrite,
        },
        input,
        session_id: None,
    }
}

fn run_scenarios(runner: &dyn ToolRunner, workspace: &Path) -> HashMap<&'static str, ToolResult> {
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
            make_request("Glob", workspace, serde_json::json!({"pattern": "*.txt"})),
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
        let l = local_norm
            .get(tool)
            .unwrap_or_else(|| panic!("missing local {tool}"));
        let r = remote_norm
            .get(tool)
            .unwrap_or_else(|| panic!("missing remote {tool}"));
        assert_eq!(l.success, r.success, "{tool}: success");
        assert_eq!(l.tool_id, r.tool_id, "{tool}: tool_id");
        // Bash output includes a per-run uuid in metadata; the stdout is
        // a JSON pretty-print containing it. Skip the strict byte
        // comparison for Bash and only check the streamed stdout body.
        if tool == "Bash" {
            assert!(l.stdout.contains("cross-backend"));
            assert!(r.stdout.contains("cross-backend"));
        } else {
            assert_eq!(
                comparable_stdout(&l.stdout),
                comparable_stdout(&r.stdout),
                "{tool}: stdout (post-normalization)"
            );
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

    let local_glob = local_runner.glob(local_workspace.path(), "*.txt").unwrap();
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
            headers: Default::default(),
            oauth: None,
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
    let local_resources = local_runner
        .list_mcp_resources(None)
        .expect("local resources");
    let remote_resources = remote.list_mcp_resources(None).expect("remote resources");
    assert_eq!(local_resources.len(), remote_resources.len());
    assert!(local_resources
        .iter()
        .any(|r| r.uri == "mcp://filesystem/hello.md"));
    assert!(remote_resources
        .iter()
        .any(|r| r.uri == "mcp://filesystem/hello.md"));

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
            .args([
                "build",
                "-p",
                "puffer-core",
                "--bin",
                "puffer-mcp-stub-server",
            ])
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
            target: format!(
                "'{}' --marker puffer-mcp-grpc-cross-backend",
                stub_bin.display()
            ),
            description: "Integration-test stub MCP server".into(),
            headers: Default::default(),
            oauth: None,
        }]
    };

    let local_runner = LocalToolRunner::new().with_mcp_servers(manifest());
    let server_runner: Arc<dyn ToolRunner> =
        Arc::new(LocalToolRunner::new().with_mcp_servers(manifest()));
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
        .call_mcp_tool(
            "stub",
            "echo",
            serde_json::json!({"text": "ping"}),
            &mut sink,
        )
        .expect("local echo");
    let remote_echo = remote
        .call_mcp_tool(
            "stub",
            "echo",
            serde_json::json!({"text": "ping"}),
            &mut sink,
        )
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
            headers: Default::default(),
            oauth: None,
        }]
    };

    let local_runner = LocalToolRunner::new().with_mcp_servers(manifest());
    let server_runner: Arc<dyn ToolRunner> =
        Arc::new(LocalToolRunner::new().with_mcp_servers(manifest()));
    let server = spawn_server(server_runner);
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN))
        .expect("connect remote runner");

    // resources/list — same URIs, names, mime types in the same order.
    let local_resources = local_runner
        .list_mcp_resources(Some("stub"))
        .expect("local list_resources");
    let remote_resources = remote
        .list_mcp_resources(Some("stub"))
        .expect("remote list_resources");
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
    let local_prompts = local_runner
        .list_mcp_prompts("stub")
        .expect("local list_prompts");
    let remote_prompts = remote
        .list_mcp_prompts("stub")
        .expect("remote list_prompts");
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
            headers: Default::default(),
            oauth: None,
        }]
    };

    let server_runner: Arc<dyn ToolRunner> =
        Arc::new(LocalToolRunner::new().with_mcp_servers(manifest()));
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
        assert_eq!(
            event.get("kind").and_then(|v| v.as_str()),
            Some("mcp/progress")
        );
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

fn comparable_stdout(stdout: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(stdout) else {
        return stdout.to_string();
    };
    let Some(object) = value.as_object_mut() else {
        return stdout.to_string();
    };
    object.remove("durationMs");
    serde_json::to_string(&value).unwrap_or_else(|_| stdout.to_string())
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

/// `ElicitationHandler` that records every received request and replies
/// with a fixed response. Mirrors the recording handler in the puffer-core
/// stdio tests so the cross-backend test asserts end-to-end equivalence.
#[derive(Debug, Clone)]
struct CrossBackendRecordingHandler {
    response: ElicitationResponse,
    requests: Arc<Mutex<Vec<ElicitationRequest>>>,
}

impl CrossBackendRecordingHandler {
    fn new(response: ElicitationResponse) -> Self {
        Self {
            response,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl ElicitationHandler for CrossBackendRecordingHandler {
    fn elicit(&self, request: ElicitationRequest) -> ElicitationResponse {
        self.requests.lock().unwrap().push(request);
        self.response.clone()
    }
}

/// End-to-end elicitation round-trip: the local runner and the gRPC stack
/// both invoke `request_user_input` with matching handlers, and the
/// resulting `McpResult` payloads must be byte-identical.
#[test]
fn cross_backend_elicitation_round_trips() {
    use puffer_resources::McpServerSpec;

    let stub_bin = locate_stub_binary();
    let manifest = || -> Vec<McpServerSpec> {
        vec![McpServerSpec {
            id: "stub".into(),
            display_name: "Stub".into(),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: format!(
                "'{}' --marker puffer-mcp-grpc-cross-backend-elicit",
                stub_bin.display()
            ),
            description: "Integration-test stub MCP server".into(),
            headers: Default::default(),
            oauth: None,
        }]
    };

    let accept_payload = serde_json::json!({ "confirmed": true });

    // Local side: install the recording handler directly on
    // LocalToolRunner. The handler is invoked synchronously by the
    // connection manager via spawn_blocking.
    let local_handler =
        CrossBackendRecordingHandler::new(ElicitationResponse::accept(accept_payload.clone()));
    let local_requests = local_handler.requests.clone();
    let local_runner = LocalToolRunner::new()
        .with_mcp_servers(manifest())
        .with_elicitation_handler(Arc::new(local_handler));

    // Remote side: install the bidi router on the SERVER's runner so
    // server-initiated MCP elicitations route into the bidi stream, and
    // install the matching recording handler on the CLIENT
    // (RemoteToolRunner) so the response flows back through gRPC.
    let router = Arc::new(BidiElicitationRouter::default());
    let server_runner_inner = LocalToolRunner::new()
        .with_mcp_servers(manifest())
        .with_elicitation_handler(Arc::clone(&router) as Arc<dyn ElicitationHandler>);
    let server_runner: Arc<dyn ToolRunner> = Arc::new(server_runner_inner);
    let server = spawn_server_with_router(server_runner, router);

    let remote_handler =
        CrossBackendRecordingHandler::new(ElicitationResponse::accept(accept_payload.clone()));
    let remote_requests = remote_handler.requests.clone();
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN))
        .expect("connect remote runner")
        .with_elicitation_handler(Arc::new(remote_handler));

    // Drive the eliciting tool through both backends.
    let mut sink = NullChunkSink;
    let local_result = local_runner
        .call_mcp_tool(
            "stub",
            "request_user_input",
            serde_json::json!({}),
            &mut sink,
        )
        .expect("local request_user_input");
    let remote_result = remote
        .call_mcp_tool(
            "stub",
            "request_user_input",
            serde_json::json!({}),
            &mut sink,
        )
        .expect("remote request_user_input");

    assert!(local_result.success);
    assert!(remote_result.success);
    // Byte-equal stdout — the stub serializes the resolved action +
    // content as JSON, so equivalence here proves the elicitation
    // payload survived both hops untouched.
    assert_eq!(local_result.stdout, remote_result.stdout);
    let body: serde_json::Value =
        serde_json::from_str(&local_result.stdout).expect("stub returns JSON body");
    assert_eq!(body.get("action").and_then(|v| v.as_str()), Some("accept"));
    assert_eq!(body.get("content").cloned(), Some(accept_payload));

    // Both handlers actually saw exactly one request.
    assert_eq!(local_requests.lock().unwrap().len(), 1);
    assert_eq!(remote_requests.lock().unwrap().len(), 1);
    let local_req = local_requests.lock().unwrap()[0].clone();
    let remote_req = remote_requests.lock().unwrap()[0].clone();
    assert_eq!(local_req.message, "Confirm the destructive action?");
    assert_eq!(remote_req.message, local_req.message);
    assert_eq!(remote_req.server, local_req.server);

    drop(remote);
    drop(server);
}

/// Same shape as `cross_backend_real_mcp_tools`, but the underlying MCP
/// transport is HTTP rather than stdio. Both backends point at the same
/// in-process axum-mounted `StreamableHttpService`; results must be
/// byte-equal across local and gRPC paths.
///
/// The stub server here is a small inline copy — it would be nice to
/// share `puffer-core/tests/mcp_stub/stub_server.rs`, but cargo test
/// helpers can't easily reach into another crate's `tests/` tree. The
/// puffer-core HTTP integration test exercises the same StubServer
/// via the same wire format, so any drift between the two would show
/// up there first.
mod http_stub {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::Router;
    use rmcp::handler::server::ServerHandler;
    use rmcp::model::{
        CallToolRequestParams, CallToolResult, Content, GetPromptRequestParams, GetPromptResult,
        Implementation, InitializeResult, ListPromptsResult, ListResourcesResult, ListToolsResult,
        PaginatedRequestParams, Prompt, PromptArgument, PromptMessage, PromptMessageRole,
        ProtocolVersion, RawResource, ReadResourceRequestParams, ReadResourceResult, Resource,
        ResourceContents, ServerCapabilities, Tool,
    };
    use rmcp::service::{NotificationContext, RequestContext, RoleServer};
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::streamable_http_server::tower::StreamableHttpService;
    use rmcp::transport::streamable_http_server::StreamableHttpServerConfig;
    use rmcp::ErrorData;
    use serde_json::{json, Map, Value};
    use tokio_util::sync::CancellationToken;

    #[derive(Clone, Default)]
    pub struct StubServer;

    impl ServerHandler for StubServer {
        fn get_info(&self) -> InitializeResult {
            InitializeResult {
                protocol_version: ProtocolVersion::default(),
                capabilities: ServerCapabilities::builder()
                    .enable_tools()
                    .enable_prompts()
                    .enable_resources()
                    .build(),
                server_info: Implementation {
                    name: "puffer-mcp-grpc-http-stub".into(),
                    title: None,
                    version: env!("CARGO_PKG_VERSION").into(),
                    description: None,
                    icons: None,
                    website_url: None,
                },
                instructions: None,
            }
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _ctx: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            Ok(ListToolsResult {
                tools: vec![Tool::new("echo", "Echo `text` back", echo_schema())],
                next_cursor: None,
                meta: None,
            })
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            _ctx: RequestContext<RoleServer>,
        ) -> Result<CallToolResult, ErrorData> {
            if request.name.as_ref() != "echo" {
                return Err(ErrorData::invalid_params("only `echo` is supported", None));
            }
            let args = request.arguments.unwrap_or_default();
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| ErrorData::invalid_params("missing `text`", None))?;
            Ok(CallToolResult::success(vec![Content::text(text)]))
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _ctx: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            Ok(ListResourcesResult {
                resources: vec![Resource::new(
                    RawResource {
                        uri: "stub://hello.txt".into(),
                        name: "hello".into(),
                        title: None,
                        description: Some("hello stub".into()),
                        mime_type: Some("text/plain".into()),
                        size: None,
                        icons: None,
                        meta: None,
                    },
                    None,
                )],
                next_cursor: None,
                meta: None,
            })
        }

        async fn read_resource(
            &self,
            request: ReadResourceRequestParams,
            _ctx: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResult, ErrorData> {
            if request.uri == "stub://hello.txt" {
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::TextResourceContents {
                        uri: request.uri,
                        mime_type: Some("text/plain".into()),
                        text: "hello from grpc http stub".into(),
                        meta: None,
                    }],
                })
            } else {
                Err(ErrorData::invalid_params("unknown resource", None))
            }
        }

        async fn list_prompts(
            &self,
            _request: Option<PaginatedRequestParams>,
            _ctx: RequestContext<RoleServer>,
        ) -> Result<ListPromptsResult, ErrorData> {
            Ok(ListPromptsResult {
                prompts: vec![Prompt::new(
                    "greet",
                    Some("Greet the named caller"),
                    Some(vec![PromptArgument {
                        name: "name".into(),
                        title: None,
                        description: None,
                        required: Some(true),
                    }]),
                )],
                next_cursor: None,
                meta: None,
            })
        }

        async fn get_prompt(
            &self,
            request: GetPromptRequestParams,
            _ctx: RequestContext<RoleServer>,
        ) -> Result<GetPromptResult, ErrorData> {
            let name = request
                .arguments
                .as_ref()
                .and_then(|m| m.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("world");
            Ok(GetPromptResult {
                description: None,
                messages: vec![PromptMessage::new_text(
                    PromptMessageRole::User,
                    format!("Hello, {name}!"),
                )],
            })
        }

        async fn on_initialized(&self, _ctx: NotificationContext<RoleServer>) {}
    }

    fn echo_schema() -> Arc<Map<String, Value>> {
        let v = json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"],
        });
        match v {
            Value::Object(m) => Arc::new(m),
            _ => Arc::new(Map::new()),
        }
    }

    pub struct Stub {
        pub url: String,
        cancel: CancellationToken,
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            self.cancel.cancel();
        }
    }

    pub async fn spawn() -> anyhow::Result<Stub> {
        let cancel = CancellationToken::new();
        let service: StreamableHttpService<StubServer, LocalSessionManager> =
            StreamableHttpService::new(
                || Ok(StubServer),
                Default::default(),
                StreamableHttpServerConfig {
                    stateful_mode: true,
                    sse_keep_alive: Some(Duration::from_secs(5)),
                    cancellation_token: cancel.child_token(),
                    ..Default::default()
                },
            );
        let router = Router::new().nest_service("/mcp", service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let cancel_for_shutdown = cancel.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { cancel_for_shutdown.cancelled_owned().await })
                .await;
        });
        Ok(Stub {
            url: format!("http://{addr}/mcp"),
            cancel,
        })
    }
}

/// Drives `tools/list`, `tools/call`, `resources/list`, `resources/read`,
/// `prompts/list`, and `prompts/get` over an HTTP MCP transport against
/// both backends. The results must round-trip byte-equal between the
/// in-process LocalToolRunner and the gRPC RemoteToolRunner.
///
/// This proves the HTTP transport plugs into the *same* `ToolRunner`
/// trait surface that stdio uses — the gRPC layer sees no difference
/// between a stdio child and an HTTPS endpoint.
#[test]
fn cross_backend_http_mcp_round_trip() {
    use puffer_resources::McpServerSpec;

    // Build a multi-thread runtime so the axum stub runs alongside the
    // sync test thread. The server stays up as long as `_stub` does.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("test runtime");
    let stub = rt.block_on(http_stub::spawn()).expect("spawn http stub");

    let manifest = || -> Vec<McpServerSpec> {
        vec![McpServerSpec {
            id: "stub".into(),
            display_name: "Stub HTTP".into(),
            transport: "http".into(),
            endpoint: String::new(),
            target: stub.url.clone(),
            description: "Cross-backend HTTP MCP stub".into(),
            headers: Default::default(),
            oauth: None,
        }]
    };

    let local_runner = LocalToolRunner::new().with_mcp_servers(manifest());
    let server_runner: Arc<dyn ToolRunner> =
        Arc::new(LocalToolRunner::new().with_mcp_servers(manifest()));
    let server = spawn_server(server_runner);
    let remote =
        RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN)).expect("connect remote");

    // tools/list — both backends agree.
    let local_tools = local_runner.list_mcp_tools("stub").expect("local tools");
    let remote_tools = remote.list_mcp_tools("stub").expect("remote tools");
    let local_names: Vec<_> = local_tools.iter().map(|t| t.name.clone()).collect();
    let remote_names: Vec<_> = remote_tools.iter().map(|t| t.name.clone()).collect();
    assert_eq!(local_names, remote_names);
    assert_eq!(local_names, vec!["echo".to_string()]);

    // tools/call echo — payloads round-trip byte-equal.
    let mut sink = NullChunkSink;
    let local_echo = local_runner
        .call_mcp_tool(
            "stub",
            "echo",
            serde_json::json!({"text": "ping"}),
            &mut sink,
        )
        .expect("local echo");
    let remote_echo = remote
        .call_mcp_tool(
            "stub",
            "echo",
            serde_json::json!({"text": "ping"}),
            &mut sink,
        )
        .expect("remote echo");
    assert_eq!(local_echo.success, remote_echo.success);
    assert_eq!(local_echo.stdout, remote_echo.stdout);
    assert_eq!(local_echo.stdout, "ping");

    // resources/list — same URIs.
    let local_res = local_runner
        .list_mcp_resources(Some("stub"))
        .expect("local list_resources");
    let remote_res = remote
        .list_mcp_resources(Some("stub"))
        .expect("remote list_resources");
    let local_uris: Vec<_> = local_res.iter().map(|r| r.uri.clone()).collect();
    let remote_uris: Vec<_> = remote_res.iter().map(|r| r.uri.clone()).collect();
    assert_eq!(local_uris, remote_uris);

    // resources/read — text byte-equal.
    let local_text = local_runner
        .read_mcp_resource("stub", "stub://hello.txt")
        .expect("local read_resource");
    let remote_text = remote
        .read_mcp_resource("stub", "stub://hello.txt")
        .expect("remote read_resource");
    match (local_text.parts.first(), remote_text.parts.first()) {
        (
            Some(McpResourceContentPart::Text { text: l, .. }),
            Some(McpResourceContentPart::Text { text: r, .. }),
        ) => {
            assert_eq!(l, r);
            assert_eq!(l, "hello from grpc http stub");
        }
        other => panic!("expected text/text parts, got {other:?}"),
    }

    // prompts/get — rendered text byte-equal.
    let local_get = local_runner
        .get_mcp_prompt("stub", "greet", serde_json::json!({"name": "remote"}))
        .expect("local get_prompt");
    let remote_get = remote
        .get_mcp_prompt("stub", "greet", serde_json::json!({"name": "remote"}))
        .expect("remote get_prompt");
    assert_eq!(local_get.messages.len(), remote_get.messages.len());
    assert_eq!(local_get.messages[0].text, remote_get.messages[0].text);
    assert_eq!(local_get.messages[0].text, "Hello, remote!");

    drop(remote);
    drop(server);
    drop(stub);
    drop(rt);
}

#[test]
fn cross_backend_oauth_token_push_round_trip() {
    // Verifies the new push_oauth_tokens / oauth_status / clear_oauth_tokens
    // RPCs end-to-end. Builds a LocalToolRunner with one HTTP+OAuth MCP
    // server registered and an isolated token dir, fronts it with a gRPC
    // ToolRunnerService, and drives push -> status -> clear -> status from
    // a RemoteToolRunner client.
    use puffer_resources::{McpOAuthDetail, McpOAuthSpec, McpServerSpec};

    let token_dir = tempdir().unwrap();
    let manifest = || -> Vec<McpServerSpec> {
        vec![McpServerSpec {
            id: "stub".into(),
            display_name: "Stub OAuth".into(),
            transport: "http".into(),
            endpoint: String::new(),
            target: "https://mcp.example.com/v1".into(),
            description: "OAuth-gated MCP stub".into(),
            headers: Default::default(),
            oauth: Some(McpOAuthSpec::Detailed(McpOAuthDetail {
                enabled: true,
                scope: String::new(),
                client_name: "puffer-test".into(),
            })),
        }]
    };

    let server_runner: Arc<dyn ToolRunner> = Arc::new(
        LocalToolRunner::new()
            .with_mcp_servers(manifest())
            .with_oauth_token_dir(token_dir.path().to_path_buf()),
    );
    let server = spawn_server(server_runner);
    let remote =
        RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN)).expect("connect remote");

    // 1. Initially no tokens stored.
    let status = remote.oauth_status("stub").expect("status absent");
    assert!(matches!(status, OAuthStatus::Absent), "got {status:?}");

    // 2. Push a synthetic token bundle.
    let payload = OAuthTokensPayload {
        server_id: "stub".into(),
        server_url: "https://mcp.example.com/v1".into(),
        client_id: "client-from-test".into(),
        client_secret: None,
        access_token: "stub-access-42".into(),
        token_type: "Bearer".into(),
        refresh_token: Some("stub-refresh-42".into()),
        scopes: vec!["repo".into(), "user:email".into()],
        expires_at_ms: Some(99_999_999_999_999),
    };
    remote
        .push_oauth_tokens("stub", payload.clone())
        .expect("push_oauth_tokens succeeds");

    // 3. The runner now reports a Present status with the right metadata.
    let status = remote.oauth_status("stub").expect("status present");
    match status {
        OAuthStatus::Present {
            expires_at_ms,
            has_refresh,
            scopes,
        } => {
            assert_eq!(expires_at_ms, Some(99_999_999_999_999));
            assert!(has_refresh);
            assert_eq!(scopes, vec!["repo".to_string(), "user:email".to_string()]);
        }
        other => panic!("expected Present, got {other:?}"),
    }

    // 4. The on-disk file is exactly what `puffer mcp login` would have
    //    written — readable as `PersistedTokens` with the same shape.
    let on_disk = puffer_mcp_oauth::PersistedTokens::read_from(
        token_dir.path(),
        "stub",
        "https://mcp.example.com/v1",
    )
    .expect("read on-disk tokens")
    .expect("tokens persisted");
    assert_eq!(on_disk.access_token, payload.access_token);
    assert_eq!(on_disk.refresh_token, payload.refresh_token);
    assert_eq!(on_disk.client_id, payload.client_id);

    // 5. Clear via remote — status flips back to Absent and the file is
    //    gone.
    remote.clear_oauth_tokens("stub").expect("clear succeeds");
    let status = remote.oauth_status("stub").expect("status after clear");
    assert!(matches!(status, OAuthStatus::Absent), "got {status:?}");
    let on_disk = puffer_mcp_oauth::PersistedTokens::read_from(
        token_dir.path(),
        "stub",
        "https://mcp.example.com/v1",
    )
    .expect("read after clear");
    assert!(on_disk.is_none(), "file should be removed");

    // 6. Clearing again is a no-op (idempotent).
    remote
        .clear_oauth_tokens("stub")
        .expect("second clear is idempotent");

    drop(remote);
    drop(server);
}

/// Pass 1.5g cross-backend: the MCP tool advertising bridge must produce
/// the same qualified registry whether discovery walks an in-process
/// `LocalToolRunner` or a `RemoteToolRunner` over gRPC. Without this the
/// model would only see `mcp__*` entries on one of the two backends.
#[test]
fn cross_backend_mcp_tool_advertising() {
    use puffer_core::mcp_discovery::registry_with_mcp_tools;
    use puffer_resources::{LoadedResources, McpServerSpec};

    let stub_bin = locate_stub_binary();
    let manifest = || -> Vec<McpServerSpec> {
        vec![McpServerSpec {
            id: "stub".into(),
            display_name: "Stub".into(),
            transport: "stdio".into(),
            endpoint: String::new(),
            target: format!(
                "'{}' --marker puffer-mcp-grpc-tool-advertising",
                stub_bin.display()
            ),
            description: "Integration-test stub MCP server".into(),
            headers: Default::default(),
            oauth: None,
        }]
    };

    let local_runner = LocalToolRunner::new().with_mcp_servers(manifest());
    let server_runner: Arc<dyn ToolRunner> =
        Arc::new(LocalToolRunner::new().with_mcp_servers(manifest()));
    let server = spawn_server(server_runner);
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN))
        .expect("connect remote runner");

    let resources = LoadedResources::default();
    let local_registry = registry_with_mcp_tools(&resources, &local_runner);
    let remote_registry = registry_with_mcp_tools(&resources, &remote);

    let qualified_ids = |registry: &puffer_tools::ToolRegistry| -> Vec<String> {
        let mut ids: Vec<String> = registry
            .definitions()
            .filter(|definition| definition.id.starts_with("mcp__"))
            .map(|definition| definition.id.clone())
            .collect();
        ids.sort();
        ids
    };

    let local_ids = qualified_ids(&local_registry);
    let remote_ids = qualified_ids(&remote_registry);
    assert_eq!(
        local_ids, remote_ids,
        "qualified ids must match across backends"
    );
    assert!(local_ids.contains(&"mcp__stub__echo".to_string()));
    assert!(local_ids.contains(&"mcp__stub__slow_echo".to_string()));

    // handler_args is what the runtime executor reads to recover the raw
    // (server, tool) pair; verify both backends agree on it.
    for id in &local_ids {
        let local_def = local_registry.definition(id).expect("local definition");
        let remote_def = remote_registry.definition(id).expect("remote definition");
        assert_eq!(local_def.handler, "runtime:mcp_call");
        assert_eq!(remote_def.handler, "runtime:mcp_call");
        assert_eq!(local_def.handler_args, remote_def.handler_args);
    }

    drop(remote);
    drop(server);
}
