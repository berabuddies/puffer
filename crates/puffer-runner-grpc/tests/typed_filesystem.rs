use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use puffer_runner_api::{
    ChunkSink, DirEntry, McpPrompt, McpPromptContent, McpResourceContent, McpResourceRecord,
    McpResult, McpServerInfo, McpTool, OAuthStatus, OAuthTokensPayload, RunnerError, ToolRequest,
    ToolResult, ToolRunner,
};
use puffer_runner_grpc::proto;
use puffer_runner_grpc::server::ToolRunnerServer;
use puffer_runner_grpc::{RemoteToolRunner, ToolRunnerService, AUTH_METADATA_KEY};
use puffer_runner_local::LocalToolRunner;
use tempfile::tempdir;
use tokio::sync::oneshot;
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::transport::Channel;
use tonic::{Request, Status};

const TEST_TOKEN: &str = "test-token-typed-fs";

#[derive(Clone)]
struct TestAuthInterceptor {
    token: MetadataValue<tonic::metadata::Ascii>,
}

impl Interceptor for TestAuthInterceptor {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        req.metadata_mut()
            .insert(AUTH_METADATA_KEY, self.token.clone());
        Ok(req)
    }
}

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

fn bind_loopback_listener_or_skip() -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => Some(listener),
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => None,
        Err(err) => panic!("bind ephemeral port: {err}"),
    }
}

fn spawn_server(runner: Arc<dyn ToolRunner>) -> Option<ServerHandle> {
    let listener = bind_loopback_listener_or_skip()?;
    listener
        .set_nonblocking(true)
        .expect("configure listener nonblocking");
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    let endpoint = format!("http://{addr}");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("puffer-runner-grpc-typed-fs-test")
        .build()
        .expect("server runtime");
    let service = ToolRunnerService::new(runner).with_auth_token(Some(TEST_TOKEN.to_string()));
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let (ready_tx, ready_rx) = oneshot::channel::<()>();
    let handle = runtime.handle().clone();
    let server_thread = std::thread::Builder::new()
        .name("puffer-runner-grpc-typed-fs-server".into())
        .spawn(move || {
            handle.block_on(async move {
                let listener =
                    tokio::net::TcpListener::from_std(listener).expect("adopt tonic listener");
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
    Some(ServerHandle {
        endpoint,
        shutdown: Some(shutdown_tx),
        runtime: Some(runtime),
        server_thread: Some(server_thread),
    })
}

fn make_request(tool_id: &str, cwd: &Path, input: serde_json::Value) -> ToolRequest {
    ToolRequest {
        tool_id: tool_id.to_string(),
        cwd: cwd.to_path_buf(),
        working_dirs: Vec::new(),
        filesystem: puffer_runner_api::FilesystemExecutionPolicy {
            sandbox_mode: puffer_runner_api::FilesystemSandboxMode::ReadOnly,
        },
        input,
        session_id: None,
    }
}

#[test]
fn remote_runner_executes_typed_filesystem_request() {
    let workspace = tempdir().unwrap();
    let file = workspace.path().join("file.txt");
    std::fs::write(&file, "hello from typed fs").unwrap();
    let Some(server) = spawn_server(Arc::new(LocalToolRunner::new())) else {
        return;
    };
    let remote = RemoteToolRunner::connect(&server.endpoint, Some(TEST_TOKEN))
        .expect("connect remote runner");

    let result = remote
        .execute_tool(
            make_request(
                "Read",
                workspace.path(),
                serde_json::json!({"file_path": file}),
            ),
            &mut puffer_runner_api::NullChunkSink,
        )
        .expect("typed filesystem request should succeed");

    assert!(result.success);
    assert!(result.stdout.contains("hello from typed fs"));
}

#[test]
fn missing_filesystem_field_returns_invalid_argument() {
    let Some(server) = spawn_server(Arc::new(LocalToolRunner::new())) else {
        return;
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("client runtime");
    let endpoint = server.endpoint.clone();
    let status = runtime.block_on(async move {
        let channel = Channel::from_shared(endpoint)
            .expect("endpoint")
            .connect()
            .await
            .expect("connect raw grpc client");
        let token: MetadataValue<tonic::metadata::Ascii> = format!("Bearer {TEST_TOKEN}")
            .parse()
            .expect("bearer token metadata");
        let interceptor = TestAuthInterceptor { token };
        let mut client =
            proto::tool_runner_client::ToolRunnerClient::with_interceptor(channel, interceptor);
        client
            .execute_tool(proto::ToolRequest {
                tool_id: "Read".into(),
                cwd: "/workspace".into(),
                working_dirs: Vec::new(),
                input_json: "{}".into(),
                session_id: None,
                filesystem: None,
            })
            .await
            .expect_err("missing filesystem should fail")
    });

    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("filesystem: missing"));
}
