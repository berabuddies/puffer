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
//! ## Connection model
//!
//! The channel is built with [`Endpoint::connect_lazy`]: construction is
//! infallible and does not touch the network. The first RPC opens the
//! connection on demand, and subsequent transient failures are reconnected
//! transparently by the underlying tower stack. See [`retry_unavailable`]
//! for the per-call resilience layer that wraps each unary RPC.
//!
//! The runtime is owned by the runner; on `Drop` it is shut down with a short
//! grace period.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use puffer_runner_api::{
    ChunkSink, DeclineAllElicitations, DirEntry, ElicitationHandler, ElicitationMode,
    ElicitationRequest, ElicitationResponse, McpPrompt, McpPromptContent, McpResourceContent,
    McpResourceRecord, McpResult, McpServerInfo, McpTool, OAuthStatus, OAuthTokensPayload,
    RunnerCapabilities, RunnerError, RunnerPing, ToolRequest, ToolResult, ToolRunner,
};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
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
    elicitation: Arc<dyn ElicitationHandler>,
}

impl std::fmt::Debug for RemoteToolRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteToolRunner")
            .field("endpoint", &self.endpoint)
            .field("elicitation", &self.elicitation)
            .finish()
    }
}

impl RemoteToolRunner {
    /// Builds a runner pointing at `endpoint`. This call performs **no
    /// network I/O**: the underlying tonic channel is constructed with
    /// [`Endpoint::connect_lazy`], so the first real RPC opens the
    /// connection and subsequent transport failures trigger a transparent
    /// reconnect. Use [`ToolRunner::ping`] to actively wait until the
    /// remote is reachable.
    pub fn connect(endpoint: &str, auth_token: Option<&str>) -> Result<Self, RunnerError> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("puffer-runner-grpc")
                .build()
                .map_err(|e| RunnerError::Transport(format!("tokio runtime: {e}")))?,
        );

        let token_meta = match auth_token {
            None => None,
            Some(t) => {
                let value: MetadataValue<tonic::metadata::Ascii> = format!("Bearer {t}")
                    .parse()
                    .map_err(|e| RunnerError::InvalidArgument(format!("auth token: {e}")))?;
                Some(Arc::new(value))
            }
        };

        // `connect_lazy` constructs a hyper connector that registers with
        // the ambient tokio reactor at build time, so we run it inside the
        // worker runtime. The call still does no network I/O — the lazy
        // channel only opens the connection on first RPC.
        let endpoint_owned = endpoint.to_string();
        let channel = runtime.block_on(async move {
            Endpoint::from_shared(endpoint_owned)
                .map_err(|e| RunnerError::InvalidArgument(format!("endpoint: {e}")))
                .map(|ep| {
                    ep.connect_timeout(Duration::from_secs(5))
                        .timeout(Duration::from_secs(60))
                        .connect_lazy()
                })
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
            elicitation: Arc::new(DeclineAllElicitations),
        })
    }

    /// Installs the elicitation handler invoked when the remote runner
    /// forwards a server-initiated `elicitation/create` request through a
    /// `CallMcpTool` bidi stream. Defaults to
    /// [`puffer_runner_api::DeclineAllElicitations`].
    pub fn with_elicitation_handler(
        mut self,
        handler: Arc<dyn ElicitationHandler>,
    ) -> Self {
        self.elicitation = handler;
        self
    }

    /// The endpoint this runner was constructed with. Useful for logs and
    /// orchestrator-side health probes.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn run<F, T>(&self, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.runtime.block_on(fut)
    }
}

/// Retries `f` once when it fails with [`tonic::Code::Unavailable`]. All
/// other status codes propagate immediately. The double-call is the entire
/// retry budget per RPC — long bounded retry loops belong at the call site
/// (e.g. the startup `Ping` gate in `select_tool_runner`).
async fn retry_unavailable<T, F, Fut>(mut f: F) -> Result<T, Status>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Status>>,
{
    match f().await {
        Err(s) if s.code() == tonic::Code::Unavailable => f().await,
        other => other,
    }
}

impl ToolRunner for RemoteToolRunner {
    fn ping(&self) -> Result<RunnerPing, RunnerError> {
        let client = self.client.clone();
        let resp = self
            .run(async move {
                retry_unavailable(|| {
                    let mut client = client.clone();
                    async move { client.ping(proto::Empty {}).await }
                })
                .await
            })
            .map_err(status_to_runner_error)?;
        let inner = resp.into_inner();
        Ok(RunnerPing {
            version: inner.version,
            uptime: Duration::from_secs(inner.uptime_seconds),
        })
    }

    fn capabilities(&self) -> RunnerCapabilities {
        let client = self.client.clone();
        let result = self.run(async move {
            retry_unavailable(|| {
                let mut client = client.clone();
                async move { client.capabilities(proto::Empty {}).await }
            })
            .await
        });
        match result {
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
        let client = self.client.clone();
        self.run(async move {
            // Only retry the initial open of the server-streaming call;
            // once chunks are flowing we propagate any mid-stream error.
            let stream = retry_unavailable(|| {
                let mut client = client.clone();
                let proto_req = proto_req.clone();
                async move { client.execute_tool(proto_req).await }
            })
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
                        return Err(runner_error_from_code(&f.code, f.message));
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
        let client = self.client.clone();
        let req = proto::ReadFileRequest {
            path: path.display().to_string(),
        };
        self.run(async move {
            retry_unavailable(|| {
                let mut client = client.clone();
                let req = req.clone();
                async move { client.read_file(req).await }
            })
            .await
        })
        .map(|resp| resp.into_inner().data)
        .map_err(status_to_runner_error)
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, RunnerError> {
        let client = self.client.clone();
        let req = proto::ListDirRequest {
            path: path.display().to_string(),
        };
        let resp = self
            .run(async move {
                retry_unavailable(|| {
                    let mut client = client.clone();
                    let req = req.clone();
                    async move { client.list_dir(req).await }
                })
                .await
            })
            .map_err(status_to_runner_error)?;
        Ok(resp
            .into_inner()
            .entries
            .into_iter()
            .map(from_proto_dir_entry)
            .collect())
    }

    fn glob(&self, root: &Path, pattern: &str) -> Result<Vec<PathBuf>, RunnerError> {
        let client = self.client.clone();
        let req = proto::GlobRequest {
            root: root.display().to_string(),
            pattern: pattern.to_string(),
        };
        let resp = self
            .run(async move {
                retry_unavailable(|| {
                    let mut client = client.clone();
                    let req = req.clone();
                    async move { client.glob(req).await }
                })
                .await
            })
            .map_err(status_to_runner_error)?;
        Ok(resp.into_inner().paths.into_iter().map(PathBuf::from).collect())
    }

    fn list_mcp_servers(&self) -> Result<Vec<McpServerInfo>, RunnerError> {
        let client = self.client.clone();
        let resp = self
            .run(async move {
                retry_unavailable(|| {
                    let mut client = client.clone();
                    async move { client.list_mcp_servers(proto::Empty {}).await }
                })
                .await
            })
            .map_err(status_to_runner_error)?;
        Ok(resp
            .into_inner()
            .servers
            .into_iter()
            .map(crate::convert::from_proto_mcp_server)
            .collect())
    }

    fn list_mcp_tools(&self, server: &str) -> Result<Vec<McpTool>, RunnerError> {
        let client = self.client.clone();
        let req = proto::McpServerRef {
            server: server.to_string(),
        };
        let resp = self
            .run(async move {
                retry_unavailable(|| {
                    let mut client = client.clone();
                    let req = req.clone();
                    async move { client.list_mcp_tools(req).await }
                })
                .await
            })
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
        let client = self.client.clone();
        let elicitation = Arc::clone(&self.elicitation);
        let call = proto::McpToolCall {
            server: server.to_string(),
            tool: tool.to_string(),
            args_json: args.to_string(),
        };
        self.run(async move {
            // Outbound channel for client→server messages. We send the
            // initial `Call` envelope and any subsequent
            // `ElicitationResponse` envelopes through this. Buffer of 8
            // is plenty: at most one response per outstanding elicitation
            // plus the initial Call.
            let (out_tx, out_rx) = mpsc::channel::<proto::McpToolMessage>(8);
            // Seed the stream with the Call envelope.
            out_tx
                .send(proto::McpToolMessage {
                    payload: Some(proto::mcp_tool_message::Payload::Call(call)),
                })
                .await
                .map_err(|_| RunnerError::Transport("CallMcpTool send dropped".into()))?;
            let outbound = ReceiverStream::new(out_rx);

            // Open the bidi stream. Note: tonic's bidi RPCs return a
            // `Streaming<Response>` directly; we don't use the unary
            // retry_unavailable wrapper because it would require restarting
            // the stream from scratch.
            let mut client_for_call = client.clone();
            let inbound = client_for_call
                .call_mcp_tool(outbound)
                .await
                .map_err(status_to_runner_error)?;
            let mut inbound = inbound.into_inner();

            let mut result: Option<McpResult> = None;
            while let Some(msg) = inbound
                .message()
                .await
                .map_err(status_to_runner_error)?
            {
                match msg.payload {
                    Some(proto::mcp_tool_message::Payload::Stdout(c)) => sink.stdout(&c.data),
                    Some(proto::mcp_tool_message::Payload::Stderr(c)) => sink.stderr(&c.data),
                    Some(proto::mcp_tool_message::Payload::Completed(c)) => {
                        result = Some(crate::convert::from_proto_mcp_result(c)?);
                    }
                    Some(proto::mcp_tool_message::Payload::Failed(f)) => {
                        return Err(runner_error_from_code(&f.code, f.message));
                    }
                    Some(proto::mcp_tool_message::Payload::EventJson(json)) => {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
                            sink.event(value);
                        }
                    }
                    Some(proto::mcp_tool_message::Payload::ElicitationRequest(req)) => {
                        // Run the (sync) handler off the tokio worker so
                        // it can block waiting on the user without
                        // stalling the gRPC reader.
                        let handler = Arc::clone(&elicitation);
                        let dto = ElicitationRequest {
                            server: req.server,
                            tool: req.tool,
                            message: req.message,
                            mode: match req.mode.as_str() {
                                "url" => ElicitationMode::Url,
                                _ => ElicitationMode::Form,
                            },
                            schema: if req.schema_json.is_empty() {
                                serde_json::Value::Null
                            } else {
                                serde_json::from_str(&req.schema_json)
                                    .unwrap_or(serde_json::Value::Null)
                            },
                            url: req.url,
                            elicitation_id: req.elicitation_id,
                        };
                        let response = tokio::task::spawn_blocking(move || handler.elicit(dto))
                            .await
                            .map_err(|e| {
                                RunnerError::Other(format!("elicitation handler join: {e}"))
                            })?;
                        let envelope = encode_elicitation_response(&req.request_id, response);
                        if out_tx.send(envelope).await.is_err() {
                            // Stream dropped — nothing more we can do; the
                            // server will see `Cancel` semantics by virtue
                            // of the closed receive side.
                            break;
                        }
                    }
                    Some(proto::mcp_tool_message::Payload::ElicitationResponse(_))
                    | Some(proto::mcp_tool_message::Payload::Call(_))
                    | None => {}
                }
            }
            result.ok_or_else(|| RunnerError::Mcp("call_mcp_tool stream ended early".into()))
        })
    }

    fn list_mcp_resources(
        &self,
        server: Option<&str>,
    ) -> Result<Vec<McpResourceRecord>, RunnerError> {
        let client = self.client.clone();
        let req = proto::McpResourceQuery {
            server: server.map(|s| s.to_string()),
        };
        let resp = self
            .run(async move {
                retry_unavailable(|| {
                    let mut client = client.clone();
                    let req = req.clone();
                    async move { client.list_mcp_resources(req).await }
                })
                .await
            })
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
        let client = self.client.clone();
        let req = proto::McpResourceRef {
            server: server.to_string(),
            uri: uri.to_string(),
        };
        let resp = self
            .run(async move {
                retry_unavailable(|| {
                    let mut client = client.clone();
                    let req = req.clone();
                    async move { client.read_mcp_resource(req).await }
                })
                .await
            })
            .map_err(status_to_runner_error)?;
        crate::convert::from_proto_mcp_resource_content(resp.into_inner())
    }

    fn list_mcp_prompts(&self, server: &str) -> Result<Vec<McpPrompt>, RunnerError> {
        let client = self.client.clone();
        let req = proto::McpServerRef {
            server: server.to_string(),
        };
        let resp = self
            .run(async move {
                retry_unavailable(|| {
                    let mut client = client.clone();
                    let req = req.clone();
                    async move { client.list_mcp_prompts(req).await }
                })
                .await
            })
            .map_err(status_to_runner_error)?;
        Ok(resp
            .into_inner()
            .prompts
            .into_iter()
            .map(crate::convert::from_proto_mcp_prompt)
            .collect())
    }

    fn push_oauth_tokens(
        &self,
        server: &str,
        tokens: OAuthTokensPayload,
    ) -> Result<(), RunnerError> {
        let client = self.client.clone();
        let req = proto::PushOAuthTokensRequest {
            server: server.to_string(),
            tokens: Some(crate::convert::oauth_payload_to_proto(&tokens)),
        };
        self.run(async move {
            retry_unavailable(|| {
                let mut client = client.clone();
                let req = req.clone();
                async move { client.push_o_auth_tokens(req).await }
            })
            .await
        })
        .map(|_| ())
        .map_err(status_to_runner_error)
    }

    fn oauth_status(&self, server: &str) -> Result<OAuthStatus, RunnerError> {
        let client = self.client.clone();
        let req = proto::OAuthServerRef {
            server_id: server.to_string(),
        };
        let resp = self
            .run(async move {
                retry_unavailable(|| {
                    let mut client = client.clone();
                    let req = req.clone();
                    async move { client.query_o_auth_status(req).await }
                })
                .await
            })
            .map_err(status_to_runner_error)?;
        Ok(crate::convert::oauth_status_from_proto(resp.into_inner()))
    }

    fn clear_oauth_tokens(&self, server: &str) -> Result<(), RunnerError> {
        let client = self.client.clone();
        let req = proto::OAuthServerRef {
            server_id: server.to_string(),
        };
        self.run(async move {
            retry_unavailable(|| {
                let mut client = client.clone();
                let req = req.clone();
                async move { client.clear_o_auth_tokens(req).await }
            })
            .await
        })
        .map(|_| ())
        .map_err(status_to_runner_error)
    }

    fn get_mcp_prompt(
        &self,
        server: &str,
        name: &str,
        args: serde_json::Value,
    ) -> Result<McpPromptContent, RunnerError> {
        let client = self.client.clone();
        let req = proto::McpPromptRequest {
            server: server.to_string(),
            name: name.to_string(),
            args_json: args.to_string(),
        };
        let resp = self
            .run(async move {
                retry_unavailable(|| {
                    let mut client = client.clone();
                    let req = req.clone();
                    async move { client.get_mcp_prompt(req).await }
                })
                .await
            })
            .map_err(status_to_runner_error)?;
        Ok(crate::convert::from_proto_mcp_prompt_content(
            resp.into_inner(),
        ))
    }
}

/// Reconstructs a typed [`RunnerError`] from the `code` string emitted by
/// the server in a `Failed` envelope. Mirrors `runner_error_code` on the
/// server side so an `Unsupported` MCP call round-trips with the right
/// variant on the client.
fn runner_error_from_code(code: &str, message: String) -> RunnerError {
    match code {
        "not_found" => RunnerError::NotFound(message),
        "permission_denied" => RunnerError::PermissionDenied(message),
        "unsupported" => RunnerError::Unsupported(message),
        "invalid_argument" => RunnerError::InvalidArgument(message),
        "transport" => RunnerError::Transport(message),
        "mcp" => RunnerError::Mcp(message),
        "execution" => RunnerError::Execution(message),
        _ => RunnerError::Other(message),
    }
}

/// Convenience: build a [`RemoteToolRunner`] and return it as a trait
/// object suitable for `AppState::with_tool_runner(...)`. Construction is
/// infallible w.r.t. the network — the channel is lazy. Callers that need
/// a confirmed-live runner should follow up with [`ToolRunner::ping`].
pub fn connect_runner(
    endpoint: &str,
    auth_token: Option<&str>,
) -> Result<std::sync::Arc<dyn ToolRunner>, RunnerError> {
    let runner = RemoteToolRunner::connect(endpoint, auth_token)?;
    Ok(std::sync::Arc::new(runner))
}

/// Serializes an [`ElicitationResponse`] into the bidi `McpToolMessage`
/// envelope expected by the server (`request_id` correlates with the
/// originating `McpElicitationRequest`).
fn encode_elicitation_response(
    request_id: &str,
    response: ElicitationResponse,
) -> proto::McpToolMessage {
    let (action, content_json) = match response {
        ElicitationResponse::Accept { content } => ("accept".to_string(), content.to_string()),
        ElicitationResponse::Decline => ("decline".to_string(), String::new()),
        ElicitationResponse::Cancel => ("cancel".to_string(), String::new()),
    };
    proto::McpToolMessage {
        payload: Some(proto::mcp_tool_message::Payload::ElicitationResponse(
            proto::McpElicitationResponse {
                request_id: request_id.to_string(),
                action,
                content_json,
            },
        )),
    }
}
