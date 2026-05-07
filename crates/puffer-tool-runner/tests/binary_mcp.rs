//! In-process integration test for `puffer-tool-runner`'s MCP wiring.
//!
//! Exercises the same `build_service_from_cwd` path the binary uses to
//! confirm that an MCP server spec dropped into a workspace's
//! `.puffer/resources/mcp_servers/` directory is discovered, hydrated onto
//! the underlying `LocalToolRunner`, and surfaced via the `list_mcp_servers`
//! RPC over a real tonic loopback server.
//!
//! We deliberately avoid shelling out to the binary; doing so would slow
//! the test suite down and obscure where a regression actually lives. The
//! wiring under test is the library helper that `main.rs` itself calls.

use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::time::Duration;

use puffer_runner_api::ToolRunner;
use puffer_runner_grpc::server::ToolRunnerServer;
use puffer_runner_grpc::RemoteToolRunner;
use puffer_tool_runner::build_service_from_cwd;
use tempfile::tempdir;
use tokio::sync::oneshot;

const TEST_TOKEN: &str = "binary-mcp-test-token";

fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Mirrors the loopback-server pattern from `puffer-runner-grpc`'s
/// `grpc_e2e` suite: stand a tonic server up on a worker thread driven by
/// a dedicated runtime, signal readiness once the listener binds, and let
/// the `Drop` impl clean both halves up at the end of the test.
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

fn spawn_server(service: puffer_runner_grpc::ToolRunnerService) -> ServerHandle {
    let port = pick_free_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let endpoint = format!("http://{addr}");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("puffer-tool-runner-binary-mcp-server")
        .build()
        .expect("server runtime");

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let (ready_tx, ready_rx) = oneshot::channel::<()>();

    let handle = runtime.handle().clone();
    let server_thread = std::thread::Builder::new()
        .name("puffer-tool-runner-binary-mcp-server-thread".into())
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

/// Drops a minimal MCP server YAML into the workspace's
/// `.puffer/resources/mcp_servers/` directory, builds the tool-runner
/// service from that workspace exactly the way the binary does, and
/// asserts the server surfaces through the gRPC `list_mcp_servers` RPC.
#[test]
fn discovers_workspace_mcp_servers_at_startup() {
    let workspace = tempdir().expect("workspace tempdir");
    let mcp_dir = workspace
        .path()
        .join(".puffer")
        .join("resources")
        .join("mcp_servers");
    fs::create_dir_all(&mcp_dir).expect("create mcp_servers dir");

    // Mirrors the canonical workspace fixture in
    // `resources/mcp_servers/filesystem.yaml` — the simplest spec the
    // loader accepts: id + display_name + transport + target.
    fs::write(
        mcp_dir.join("foo.yaml"),
        "id: foo\n\
         display_name: Foo MCP\n\
         transport: stdio\n\
         target: builtin:filesystem\n\
         description: Test fixture for puffer-tool-runner MCP discovery.\n",
    )
    .expect("write foo.yaml");

    let (service, mcp_count) = build_service_from_cwd(workspace.path(), Some(TEST_TOKEN.into()))
        .expect("build_service_from_cwd");
    assert!(
        mcp_count >= 1,
        "expected at least one MCP server discovered, got {mcp_count}"
    );

    let server = spawn_server(service);
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN))
        .expect("connect remote runner");

    let servers = remote.list_mcp_servers().expect("list_mcp_servers");
    assert!(
        servers.iter().any(|s| s.id.eq_ignore_ascii_case("foo")),
        "expected the workspace `foo` MCP server in {servers:?}"
    );
}
