//! Server side of the puffer gRPC tool runner.
//!
//! [`ToolRunnerService`] is a thin adapter: every RPC forwards to the
//! supplied `Arc<dyn ToolRunner>`. There is no business logic here; if a
//! tool's behaviour needs to change, fix it in the underlying runner.
//!
//! The `puffer-tool-runner` binary constructs one of these wrapping a
//! [`puffer_runner_api::ToolRunner`] (typically `LocalToolRunner`) and hands
//! it to `tonic::transport::Server`. Integration tests do the same in-process.

use std::sync::Arc;

use puffer_runner_api::{
    ChunkKind, ChunkSink, FnChunkSink, NullChunkSink, RunnerError, ToolRunner,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::convert::{
    from_proto_tool_request, runner_error_to_status, to_proto_capabilities, to_proto_dir_entry,
    to_proto_mcp_prompt, to_proto_mcp_prompt_content, to_proto_mcp_resource_content,
    to_proto_mcp_resource_record, to_proto_mcp_result, to_proto_mcp_server, to_proto_mcp_tool,
    to_proto_tool_completed,
};
use crate::proto;
use crate::AUTH_METADATA_KEY;

pub use proto::tool_runner_server::ToolRunnerServer;

/// Adapter from a synchronous `Arc<dyn ToolRunner>` to the generated tonic
/// service trait. All RPCs forward to the runner; blocking work runs on a
/// `spawn_blocking` thread to avoid stalling the tonic worker pool.
#[derive(Clone)]
pub struct ToolRunnerService {
    runner: Arc<dyn ToolRunner>,
    auth_token: Option<Arc<String>>,
}

impl ToolRunnerService {
    /// Wraps `runner` in a tonic-ready service. No auth token.
    pub fn new(runner: Arc<dyn ToolRunner>) -> Self {
        Self {
            runner,
            auth_token: None,
        }
    }

    /// Requires every RPC to carry `authorization: Bearer <token>` metadata
    /// matching `token`. Calls without the header are rejected with
    /// `Unauthenticated`.
    pub fn with_auth_token(mut self, token: Option<String>) -> Self {
        self.auth_token = token.map(Arc::new);
        self
    }

    fn check_auth<T>(&self, req: &Request<T>) -> Result<(), Status> {
        let Some(expected) = self.auth_token.as_deref() else {
            return Ok(());
        };
        let Some(value) = req.metadata().get(AUTH_METADATA_KEY) else {
            return Err(Status::unauthenticated("missing authorization metadata"));
        };
        let raw = value
            .to_str()
            .map_err(|_| Status::unauthenticated("non-ASCII authorization metadata"))?;
        let token = raw.strip_prefix("Bearer ").unwrap_or(raw);
        if token == expected.as_str() {
            Ok(())
        } else {
            Err(Status::unauthenticated("invalid bearer token"))
        }
    }
}

#[tonic::async_trait]
impl proto::tool_runner_server::ToolRunner for ToolRunnerService {
    type ExecuteToolStream = ReceiverStream<Result<proto::ToolEvent, Status>>;
    type CallMcpToolStream = ReceiverStream<Result<proto::McpToolEvent, Status>>;

    async fn capabilities(
        &self,
        req: Request<proto::Empty>,
    ) -> Result<Response<proto::RunnerCapabilities>, Status> {
        self.check_auth(&req)?;
        let runner = self.runner.clone();
        let caps = tokio::task::spawn_blocking(move || runner.capabilities())
            .await
            .map_err(internal_join_error)?;
        Ok(Response::new(to_proto_capabilities(&caps)))
    }

    async fn execute_tool(
        &self,
        req: Request<proto::ToolRequest>,
    ) -> Result<Response<Self::ExecuteToolStream>, Status> {
        self.check_auth(&req)?;
        let proto_req = req.into_inner();
        let request = from_proto_tool_request(proto_req).map_err(|e| runner_error_to_status(&e))?;

        let (event_tx, event_rx) = mpsc::channel::<Result<proto::ToolEvent, Status>>(32);
        let runner = self.runner.clone();
        let event_tx_for_blocking = event_tx.clone();

        tokio::task::spawn_blocking(move || {
            let mut sink = ChannelChunkSink::new(event_tx_for_blocking.clone());
            match runner.execute_tool(request, &mut sink) {
                Ok(result) => {
                    let _ = event_tx_for_blocking.blocking_send(Ok(proto::ToolEvent {
                        payload: Some(proto::tool_event::Payload::Completed(
                            to_proto_tool_completed(&result),
                        )),
                    }));
                }
                Err(err) => {
                    let _ = event_tx_for_blocking.blocking_send(Ok(proto::ToolEvent {
                        payload: Some(proto::tool_event::Payload::Failed(proto::ToolFailed {
                            code: runner_error_code(&err).to_string(),
                            message: err.to_string(),
                        })),
                    }));
                }
            }
        });
        drop(event_tx);

        Ok(Response::new(ReceiverStream::new(event_rx)))
    }

    async fn read_file(
        &self,
        req: Request<proto::ReadFileRequest>,
    ) -> Result<Response<proto::FileContent>, Status> {
        self.check_auth(&req)?;
        let path = std::path::PathBuf::from(req.into_inner().path);
        let runner = self.runner.clone();
        let bytes = tokio::task::spawn_blocking(move || runner.read_file(&path))
            .await
            .map_err(internal_join_error)?
            .map_err(|e| runner_error_to_status(&e))?;
        Ok(Response::new(proto::FileContent { data: bytes }))
    }

    async fn list_dir(
        &self,
        req: Request<proto::ListDirRequest>,
    ) -> Result<Response<proto::DirListing>, Status> {
        self.check_auth(&req)?;
        let path = std::path::PathBuf::from(req.into_inner().path);
        let runner = self.runner.clone();
        let entries = tokio::task::spawn_blocking(move || runner.list_dir(&path))
            .await
            .map_err(internal_join_error)?
            .map_err(|e| runner_error_to_status(&e))?;
        Ok(Response::new(proto::DirListing {
            entries: entries.iter().map(to_proto_dir_entry).collect(),
        }))
    }

    async fn glob(
        &self,
        req: Request<proto::GlobRequest>,
    ) -> Result<Response<proto::GlobResponse>, Status> {
        self.check_auth(&req)?;
        let inner = req.into_inner();
        let root = std::path::PathBuf::from(inner.root);
        let pattern = inner.pattern;
        let runner = self.runner.clone();
        let paths = tokio::task::spawn_blocking(move || runner.glob(&root, &pattern))
            .await
            .map_err(internal_join_error)?
            .map_err(|e| runner_error_to_status(&e))?;
        Ok(Response::new(proto::GlobResponse {
            paths: paths.iter().map(|p| p.display().to_string()).collect(),
        }))
    }

    async fn list_mcp_servers(
        &self,
        req: Request<proto::Empty>,
    ) -> Result<Response<proto::McpServerList>, Status> {
        self.check_auth(&req)?;
        let runner = self.runner.clone();
        let servers = tokio::task::spawn_blocking(move || runner.list_mcp_servers())
            .await
            .map_err(internal_join_error)?
            .map_err(|e| runner_error_to_status(&e))?;
        Ok(Response::new(proto::McpServerList {
            servers: servers.iter().map(to_proto_mcp_server).collect(),
        }))
    }

    async fn list_mcp_tools(
        &self,
        req: Request<proto::McpServerRef>,
    ) -> Result<Response<proto::McpToolList>, Status> {
        self.check_auth(&req)?;
        let server = req.into_inner().server;
        let runner = self.runner.clone();
        let tools = tokio::task::spawn_blocking(move || runner.list_mcp_tools(&server))
            .await
            .map_err(internal_join_error)?
            .map_err(|e| runner_error_to_status(&e))?;
        Ok(Response::new(proto::McpToolList {
            tools: tools.iter().map(to_proto_mcp_tool).collect(),
        }))
    }

    async fn call_mcp_tool(
        &self,
        req: Request<proto::McpToolCall>,
    ) -> Result<Response<Self::CallMcpToolStream>, Status> {
        self.check_auth(&req)?;
        let inner = req.into_inner();
        let args: serde_json::Value = if inner.args_json.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&inner.args_json)
                .map_err(|e| Status::invalid_argument(format!("args_json: {e}")))?
        };

        let (tx, rx) = mpsc::channel::<Result<proto::McpToolEvent, Status>>(32);
        let runner = self.runner.clone();
        let tx_blocking = tx.clone();
        tokio::task::spawn_blocking(move || {
            let mut sink = NullChunkSink;
            // We don't yet stream stdout/stderr from MCP through the wire; the
            // server-side runner provides it inside the final `McpResult`.
            match runner.call_mcp_tool(&inner.server, &inner.tool, args, &mut sink) {
                Ok(result) => {
                    let _ = tx_blocking.blocking_send(Ok(proto::McpToolEvent {
                        payload: Some(proto::mcp_tool_event::Payload::Completed(
                            to_proto_mcp_result(&result),
                        )),
                    }));
                }
                Err(err) => {
                    let _ = tx_blocking.blocking_send(Ok(proto::McpToolEvent {
                        payload: Some(proto::mcp_tool_event::Payload::Failed(proto::ToolFailed {
                            code: runner_error_code(&err).to_string(),
                            message: err.to_string(),
                        })),
                    }));
                }
            }
        });
        drop(tx);

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn list_mcp_resources(
        &self,
        req: Request<proto::McpResourceQuery>,
    ) -> Result<Response<proto::McpResourceList>, Status> {
        self.check_auth(&req)?;
        let server = req.into_inner().server;
        let runner = self.runner.clone();
        let records = tokio::task::spawn_blocking(move || runner.list_mcp_resources(server.as_deref()))
            .await
            .map_err(internal_join_error)?
            .map_err(|e| runner_error_to_status(&e))?;
        Ok(Response::new(proto::McpResourceList {
            resources: records.iter().map(to_proto_mcp_resource_record).collect(),
        }))
    }

    async fn read_mcp_resource(
        &self,
        req: Request<proto::McpResourceRef>,
    ) -> Result<Response<proto::McpResourceContent>, Status> {
        self.check_auth(&req)?;
        let inner = req.into_inner();
        let runner = self.runner.clone();
        let content = tokio::task::spawn_blocking(move || {
            runner.read_mcp_resource(&inner.server, &inner.uri)
        })
        .await
        .map_err(internal_join_error)?
        .map_err(|e| runner_error_to_status(&e))?;
        Ok(Response::new(to_proto_mcp_resource_content(&content)))
    }

    async fn list_mcp_prompts(
        &self,
        req: Request<proto::McpServerRef>,
    ) -> Result<Response<proto::McpPromptList>, Status> {
        self.check_auth(&req)?;
        let server = req.into_inner().server;
        let runner = self.runner.clone();
        let prompts = tokio::task::spawn_blocking(move || runner.list_mcp_prompts(&server))
            .await
            .map_err(internal_join_error)?
            .map_err(|e| runner_error_to_status(&e))?;
        Ok(Response::new(proto::McpPromptList {
            prompts: prompts.iter().map(to_proto_mcp_prompt).collect(),
        }))
    }

    async fn get_mcp_prompt(
        &self,
        req: Request<proto::McpPromptRequest>,
    ) -> Result<Response<proto::McpPromptContent>, Status> {
        self.check_auth(&req)?;
        let inner = req.into_inner();
        let args: serde_json::Value = if inner.args_json.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&inner.args_json)
                .map_err(|e| Status::invalid_argument(format!("args_json: {e}")))?
        };
        let runner = self.runner.clone();
        let content = tokio::task::spawn_blocking(move || {
            runner.get_mcp_prompt(&inner.server, &inner.name, args)
        })
        .await
        .map_err(internal_join_error)?
        .map_err(|e| runner_error_to_status(&e))?;
        Ok(Response::new(to_proto_mcp_prompt_content(&content)))
    }

}

fn internal_join_error(e: tokio::task::JoinError) -> Status {
    Status::internal(format!("worker join: {e}"))
}

fn runner_error_code(err: &RunnerError) -> &'static str {
    match err {
        RunnerError::NotFound(_) => "not_found",
        RunnerError::PermissionDenied(_) => "permission_denied",
        RunnerError::Unsupported(_) => "unsupported",
        RunnerError::InvalidArgument(_) => "invalid_argument",
        RunnerError::Transport(_) => "transport",
        RunnerError::Mcp(_) => "mcp",
        RunnerError::Execution(_) => "execution",
        RunnerError::Other(_) => "other",
    }
}

/// Bridge from synchronous `ChunkSink` writes to an mpsc of `ToolEvent`.
struct ChannelChunkSink {
    tx: mpsc::Sender<Result<proto::ToolEvent, Status>>,
}

impl ChannelChunkSink {
    fn new(tx: mpsc::Sender<Result<proto::ToolEvent, Status>>) -> Self {
        Self { tx }
    }

    fn send_chunk(&self, kind: ChunkKind, bytes: &[u8]) {
        let payload = match kind {
            ChunkKind::Stdout => proto::tool_event::Payload::Stdout(proto::StreamChunk {
                data: bytes.to_vec(),
            }),
            ChunkKind::Stderr => proto::tool_event::Payload::Stderr(proto::StreamChunk {
                data: bytes.to_vec(),
            }),
        };
        let _ = self
            .tx
            .blocking_send(Ok(proto::ToolEvent { payload: Some(payload) }));
    }
}

impl ChunkSink for ChannelChunkSink {
    fn stdout(&mut self, chunk: &[u8]) {
        self.send_chunk(ChunkKind::Stdout, chunk);
    }
    fn stderr(&mut self, chunk: &[u8]) {
        self.send_chunk(ChunkKind::Stderr, chunk);
    }
}

// FnChunkSink is unused here but kept in the public api for symmetry; silence
// the "unused import" warning when the module compiles standalone.
#[allow(dead_code)]
fn _force_link_fn_chunk_sink<F: FnMut(ChunkKind, &[u8]) + Send>(_: FnChunkSink<F>) {}
