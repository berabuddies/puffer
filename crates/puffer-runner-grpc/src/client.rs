//! Client side of the puffer gRPC tool runner.
//!
//! `RemoteToolRunner` is a synchronous [`puffer_runner_api::ToolRunner`]
//! implementation that internally owns a long-lived multi-threaded tokio
//! runtime and a tonic gRPC channel. Each trait method blocks on the runtime
//! to drive the corresponding RPC. We chose a long-lived runtime over per-call
//! `new_current_thread` builders because:
//!
//! * A `RemoteToolRunner` is constructed once at startup and reused for every
//!   tool call; spinning a fresh runtime per call would dominate the latency
//!   of cheap RPCs (e.g. `read_file`, `glob`).
//! * Server-streaming RPCs (`execute_tool`) need to interleave receiving
//!   chunks with forwarding them to the user-supplied `ChunkSink`, which is
//!   easiest to express as a single `block_on` over an async loop.
//!
//! The runtime is owned by the runner; on `Drop` it is shut down with a short
//! grace period.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use puffer_runner_api::{
    ChunkSink, DirEntry, McpPrompt, McpPromptContent, McpResourceContent, McpResourceRecord,
    McpResult, McpServerInfo, McpTool, PermissionDecision, PermissionRequest, RunnerCapabilities,
    RunnerError, ToolRequest, ToolResult, ToolRunner,
};
use tokio::runtime::Runtime;
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Status};

use crate::convert::{
    from_proto_capabilities, from_proto_dir_entry, from_proto_tool_completed,
    status_to_runner_error, to_proto_tool_request,
};
use crate::proto;
use crate::AUTH_METADATA_KEY;

type RunnerClient =
    proto::tool_runner_client::ToolRunnerClient<tonic::service::interceptor::InterceptedService<Channel, AuthInterceptor>>;

#[derive(Clone)]
struct AuthInterceptor {
    token: Option<Arc<MetadataValue<tonic::metadata::Ascii>>>,
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        if let Some(token) = &self.token {
            req.metadata_mut()
                .insert(AUTH_METADATA_KEY, token.as_ref().clone());
        }
        Ok(req)
    }
}

pub struct RemoteToolRunner {
    endpoint: String,
    runtime: Arc<Runtime>,
    client: RunnerClient,
}

impl std::fmt::Debug for RemoteToolRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteToolRunner")
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

impl RemoteToolRunner {
    /// Connects to a `puffer-tool-runner` server.
    pub fn connect(endpoint: &str, auth_token: Option<&str>) -> Result<Self, RunnerError> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("puffer-runner-grpc")
                .build()
                .map_err(|e| RunnerError::Transport(format!("tokio runtime: {e}")))?,
        );

        let endpoint_owned = endpoint.to_string();
        let token_meta = match auth_token {
            None => None,
            Some(t) => {
                let value: MetadataValue<tonic::metadata::Ascii> = format!("Bearer {t}")
                    .parse()
                    .map_err(|e| RunnerError::InvalidArgument(format!("auth token: {e}")))?;
                Some(Arc::new(value))
            }
        };

        let channel = runtime
            .block_on(async {
                Endpoint::from_shared(endpoint_owned.clone())
                    .map_err(|e| RunnerError::InvalidArgument(format!("endpoint: {e}")))?
                    .connect_timeout(Duration::from_secs(5))
                    .timeout(Duration::from_secs(60))
                    .connect()
                    .await
                    .map_err(|e| RunnerError::Transport(format!("connect {endpoint_owned}: {e}")))
            })?;

        let interceptor = AuthInterceptor { token: token_meta };
        let client = proto::tool_runner_client::ToolRunnerClient::with_interceptor(
            channel,
            interceptor,
        );

        Ok(Self {
            endpoint: endpoint.to_string(),
            runtime,
            client,
        })
    }

    fn run<F, T>(&self, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.runtime.block_on(fut)
    }
}

impl ToolRunner for RemoteToolRunner {
    fn capabilities(&self) -> RunnerCapabilities {
        let mut client = self.client.clone();
        match self.run(async move { client.capabilities(proto::Empty {}).await }) {
            Ok(resp) => from_proto_capabilities(resp.into_inner()),
            Err(_) => RunnerCapabilities::default(),
        }
    }

    fn execute_tool(
        &self,
        req: ToolRequest,
        sink: &mut dyn ChunkSink,
    ) -> Result<ToolResult, RunnerError> {
        let proto_req = to_proto_tool_request(&req);
        let mut client = self.client.clone();
        self.run(async move {
            let stream = client
                .execute_tool(proto_req)
                .await
                .map_err(status_to_runner_error)?;
            let mut stream = stream.into_inner();
            let mut completed: Option<ToolResult> = None;
            while let Some(event) = stream
                .message()
                .await
                .map_err(status_to_runner_error)?
            {
                match event.payload {
                    Some(proto::tool_event::Payload::Stdout(chunk)) => sink.stdout(&chunk.data),
                    Some(proto::tool_event::Payload::Stderr(chunk)) => sink.stderr(&chunk.data),
                    Some(proto::tool_event::Payload::Completed(c)) => {
                        completed = Some(from_proto_tool_completed(c)?);
                    }
                    Some(proto::tool_event::Payload::Failed(f)) => {
                        return Err(RunnerError::Execution(format!("{}: {}", f.code, f.message)));
                    }
                    None => {}
                }
            }
            completed.ok_or_else(|| {
                RunnerError::Other("execute_tool stream ended without a Completed event".into())
            })
        })
    }

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, RunnerError> {
        let mut client = self.client.clone();
        let req = proto::ReadFileRequest {
            path: path.display().to_string(),
        };
        self.run(async move { client.read_file(req).await })
            .map(|resp| resp.into_inner().data)
            .map_err(status_to_runner_error)
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, RunnerError> {
        let mut client = self.client.clone();
        let req = proto::ListDirRequest {
            path: path.display().to_string(),
        };
        let resp = self
            .run(async move { client.list_dir(req).await })
            .map_err(status_to_runner_error)?;
        Ok(resp
            .into_inner()
            .entries
            .into_iter()
            .map(from_proto_dir_entry)
            .collect())
    }

    fn glob(&self, root: &Path, pattern: &str) -> Result<Vec<PathBuf>, RunnerError> {
        let mut client = self.client.clone();
        let req = proto::GlobRequest {
            root: root.display().to_string(),
            pattern: pattern.to_string(),
        };
        let resp = self
            .run(async move { client.glob(req).await })
            .map_err(status_to_runner_error)?;
        Ok(resp.into_inner().paths.into_iter().map(PathBuf::from).collect())
    }

    fn list_mcp_servers(&self) -> Result<Vec<McpServerInfo>, RunnerError> {
        let mut client = self.client.clone();
        let resp = self
            .run(async move { client.list_mcp_servers(proto::Empty {}).await })
            .map_err(status_to_runner_error)?;
        Ok(resp
            .into_inner()
            .servers
            .into_iter()
            .map(crate::convert::from_proto_mcp_server)
            .collect())
    }

    fn list_mcp_tools(&self, server: &str) -> Result<Vec<McpTool>, RunnerError> {
        let mut client = self.client.clone();
        let req = proto::McpServerRef {
            server: server.to_string(),
        };
        let resp = self
            .run(async move { client.list_mcp_tools(req).await })
            .map_err(status_to_runner_error)?;
        let mut out = Vec::new();
        for t in resp.into_inner().tools {
            out.push(crate::convert::from_proto_mcp_tool(t)?);
        }
        Ok(out)
    }

    fn call_mcp_tool(
        &self,
        server: &str,
        tool: &str,
        args: serde_json::Value,
        sink: &mut dyn ChunkSink,
    ) -> Result<McpResult, RunnerError> {
        let mut client = self.client.clone();
        let req = proto::McpToolCall {
            server: server.to_string(),
            tool: tool.to_string(),
            args_json: args.to_string(),
        };
        self.run(async move {
            let stream = client.call_mcp_tool(req).await.map_err(status_to_runner_error)?;
            let mut stream = stream.into_inner();
            let mut result: Option<McpResult> = None;
            while let Some(event) = stream
                .message()
                .await
                .map_err(status_to_runner_error)?
            {
                match event.payload {
                    Some(proto::mcp_tool_event::Payload::Stdout(c)) => sink.stdout(&c.data),
                    Some(proto::mcp_tool_event::Payload::Stderr(c)) => sink.stderr(&c.data),
                    Some(proto::mcp_tool_event::Payload::Completed(c)) => {
                        result = Some(crate::convert::from_proto_mcp_result(c)?);
                    }
                    Some(proto::mcp_tool_event::Payload::Failed(f)) => {
                        return Err(RunnerError::Mcp(format!("{}: {}", f.code, f.message)));
                    }
                    None => {}
                }
            }
            result.ok_or_else(|| RunnerError::Mcp("call_mcp_tool stream ended early".into()))
        })
    }

    fn list_mcp_resources(
        &self,
        server: Option<&str>,
    ) -> Result<Vec<McpResourceRecord>, RunnerError> {
        let mut client = self.client.clone();
        let req = proto::McpResourceQuery {
            server: server.map(|s| s.to_string()),
        };
        let resp = self
            .run(async move { client.list_mcp_resources(req).await })
            .map_err(status_to_runner_error)?;
        Ok(resp
            .into_inner()
            .resources
            .into_iter()
            .map(crate::convert::from_proto_mcp_resource_record)
            .collect())
    }

    fn read_mcp_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> Result<McpResourceContent, RunnerError> {
        let mut client = self.client.clone();
        let req = proto::McpResourceRef {
            server: server.to_string(),
            uri: uri.to_string(),
        };
        let resp = self
            .run(async move { client.read_mcp_resource(req).await })
            .map_err(status_to_runner_error)?;
        crate::convert::from_proto_mcp_resource_content(resp.into_inner())
    }

    fn list_mcp_prompts(&self, server: &str) -> Result<Vec<McpPrompt>, RunnerError> {
        let mut client = self.client.clone();
        let req = proto::McpServerRef {
            server: server.to_string(),
        };
        let resp = self
            .run(async move { client.list_mcp_prompts(req).await })
            .map_err(status_to_runner_error)?;
        Ok(resp
            .into_inner()
            .prompts
            .into_iter()
            .map(crate::convert::from_proto_mcp_prompt)
            .collect())
    }

    fn get_mcp_prompt(
        &self,
        server: &str,
        name: &str,
        args: serde_json::Value,
    ) -> Result<McpPromptContent, RunnerError> {
        let mut client = self.client.clone();
        let req = proto::McpPromptRequest {
            server: server.to_string(),
            name: name.to_string(),
            args_json: args.to_string(),
        };
        let resp = self
            .run(async move { client.get_mcp_prompt(req).await })
            .map_err(status_to_runner_error)?;
        Ok(crate::convert::from_proto_mcp_prompt_content(
            resp.into_inner(),
        ))
    }

    fn request_permission(
        &self,
        req: PermissionRequest,
    ) -> Result<PermissionDecision, RunnerError> {
        // Phase 2 stub: open the bidi stream, send one PermissionRequest,
        // and return whichever decision (or Unsupported) the server replies
        // with. Phase 3 will replace this with a true relay loop.
        let mut client = self.client.clone();
        let id = format!("p-{}", rand_id());
        let proto_req =
            proto::PermissionMessage {
                payload: Some(proto::permission_message::Payload::Request(
                    crate::convert::to_proto_permission_request(&req, &id),
                )),
            };
        self.run(async move {
            let (tx, rx) = tokio::sync::mpsc::channel::<proto::PermissionMessage>(4);
            tx.send(proto_req)
                .await
                .map_err(|e| RunnerError::Transport(format!("permission send: {e}")))?;
            let outbound = tokio_stream::wrappers::ReceiverStream::new(rx);
            let resp = client
                .permission_channel(outbound)
                .await
                .map_err(status_to_runner_error)?;
            drop(tx);
            let mut stream = resp.into_inner();
            while let Some(msg) = stream
                .message()
                .await
                .map_err(status_to_runner_error)?
            {
                match msg.payload {
                    Some(proto::permission_message::Payload::Decision(d)) if d.id == id => {
                        return crate::convert::permission_decision_from_str(&d.decision);
                    }
                    Some(proto::permission_message::Payload::Unsupported(u)) => {
                        return Err(RunnerError::Unsupported(u.message));
                    }
                    _ => continue,
                }
            }
            Err(RunnerError::Other(
                "permission_channel closed without a decision".into(),
            ))
        })
    }
}

/// Convenience: connect a [`RemoteToolRunner`] and return it as a trait
/// object suitable for `AppState::with_tool_runner(...)`. Construction is
/// fallible (the gRPC handshake happens immediately); callers should fall
/// back to a `LocalToolRunner` if the remote is unreachable.
pub fn connect_runner(
    endpoint: &str,
    auth_token: Option<&str>,
) -> Result<std::sync::Arc<dyn ToolRunner>, RunnerError> {
    let runner = RemoteToolRunner::connect(endpoint, auth_token)?;
    Ok(std::sync::Arc::new(runner))
}

fn rand_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}
