use anyhow::{Context, Result};
use puffer_core::{execute_remote_tool, remote_tool_runner_capabilities};
use puffer_remote_tools::proto::tool_runner_server::{ToolRunner, ToolRunnerServer};
use puffer_remote_tools::proto::{
    DescribeCapabilitiesRequest, DescribeCapabilitiesResponse, ExecuteToolCompleted,
    ExecuteToolEvent, ExecuteToolFailed, ExecuteToolRequest, LoadProjectResourcesRequest,
    LoadProjectResourcesResponse, ProjectResourceFile, StreamChunk,
};
use puffer_remote_tools::RemoteToolRequest;
use puffer_resources::collect_project_resource_files;
use rand::RngCore;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status};

/// Carries the startup handshake parameters for one tool-runner process.
#[derive(Debug, Clone)]
pub struct ToolRunnerOptions {
    pub endpoint: String,
    pub token: String,
    pub handshake_file: Option<PathBuf>,
    pub print_stdout: bool,
}

#[derive(Debug, Clone)]
struct ToolRunnerAuth {
    token: Arc<String>,
}

/// Serves the remote tool-runner gRPC API.
#[derive(Debug, Clone)]
pub struct ToolRunnerService {
    auth: ToolRunnerAuth,
}

impl ToolRunnerService {
    /// Builds a new gRPC tool-runner service protected by a bearer token.
    pub fn new(token: String) -> Self {
        Self {
            auth: ToolRunnerAuth {
                token: Arc::new(token),
            },
        }
    }

    /// Converts the service into the tonic server type.
    pub fn into_service(self) -> ToolRunnerServer<Self> {
        ToolRunnerServer::new(self)
    }
}

type ExecuteToolStream =
    Pin<Box<dyn Stream<Item = Result<ExecuteToolEvent, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl ToolRunner for ToolRunnerService {
    type ExecuteToolStream = ExecuteToolStream;

    async fn describe_capabilities(
        &self,
        request: Request<DescribeCapabilitiesRequest>,
    ) -> std::result::Result<Response<DescribeCapabilitiesResponse>, Status> {
        authorize(&self.auth, &request)?;
        let capabilities = remote_tool_runner_capabilities();
        Ok(Response::new(DescribeCapabilitiesResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
            supported_tools: capabilities.supported_tools,
            streams_stdout_stderr: capabilities.streams_stdout_stderr,
        }))
    }

    async fn execute_tool(
        &self,
        request: Request<ExecuteToolRequest>,
    ) -> std::result::Result<Response<Self::ExecuteToolStream>, Status> {
        authorize(&self.auth, &request)?;
        let request = RemoteToolRequest::try_from(request.into_inner())
            .map_err(|error| Status::invalid_argument(error.to_string()))?;
        let (tx, rx) = mpsc::channel(32);
        std::thread::spawn(move || {
            let result = execute_remote_tool(&request, |chunk| {
                let payload = match chunk.stream {
                    puffer_remote_tools::RemoteToolChunkStream::Stdout => ExecuteToolEvent {
                        payload: Some(
                            puffer_remote_tools::proto::execute_tool_event::Payload::Stdout(
                                StreamChunk { text: chunk.text },
                            ),
                        ),
                    },
                    puffer_remote_tools::RemoteToolChunkStream::Stderr => ExecuteToolEvent {
                        payload: Some(
                            puffer_remote_tools::proto::execute_tool_event::Payload::Stderr(
                                StreamChunk { text: chunk.text },
                            ),
                        ),
                    },
                };
                let _ = tx.blocking_send(Ok(payload));
            });
            let event = match result {
                Ok(execution) => {
                    let metadata_json = serde_json::to_string(&execution.output.metadata)
                        .unwrap_or_else(|_| "null".to_string());
                    ExecuteToolEvent {
                        payload: Some(
                            puffer_remote_tools::proto::execute_tool_event::Payload::Completed(
                                ExecuteToolCompleted {
                                    tool_id: execution.tool_id,
                                    success: execution.success,
                                    stdout: execution.output.stdout,
                                    stderr: execution.output.stderr,
                                    metadata_json,
                                },
                            ),
                        ),
                    }
                }
                Err(error) => ExecuteToolEvent {
                    payload: Some(
                        puffer_remote_tools::proto::execute_tool_event::Payload::Failed(
                            ExecuteToolFailed {
                                message: format!("{error:#}"),
                            },
                        ),
                    ),
                },
            };
            let _ = tx.blocking_send(Ok(event));
        });
        Ok(Response::new(
            Box::pin(ReceiverStream::new(rx).map(|item| item)) as Self::ExecuteToolStream,
        ))
    }

    async fn load_project_resources(
        &self,
        request: Request<LoadProjectResourcesRequest>,
    ) -> std::result::Result<Response<LoadProjectResourcesResponse>, Status> {
        authorize(&self.auth, &request)?;
        let project_root = request.into_inner().project_root;
        if project_root.trim().is_empty() {
            return Err(Status::invalid_argument("project_root must not be empty"));
        }
        let project_root = PathBuf::from(project_root);
        let files = collect_project_resource_files(&project_root)
            .map_err(|error| Status::internal(error.to_string()))?;
        Ok(Response::new(LoadProjectResourcesResponse {
            files: files
                .into_iter()
                .map(|file| ProjectResourceFile {
                    relative_path: file.relative_path.to_string_lossy().replace('\\', "/"),
                    content: file.content,
                })
                .collect(),
        }))
    }
}

/// Loads the configured token or generates a random fallback token.
pub fn load_or_generate_token(configured: Option<&str>) -> String {
    if let Some(token) = configured.map(str::trim).filter(|value| !value.is_empty()) {
        return token.to_string();
    }
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex_encode(&bytes)
}

/// Writes and optionally prints the startup handshake JSON.
pub fn print_handshake(options: &ToolRunnerOptions) -> Result<()> {
    let handshake = RunnerHandshake {
        endpoint: options.endpoint.clone(),
        token: options.token.clone(),
    };
    let json = serde_json::to_string(&handshake)?;
    if let Some(path) = &options.handshake_file {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(path, &json).with_context(|| format!("failed to write {}", path.display()))?;
    }
    if options.print_stdout {
        println!("{json}");
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct RunnerHandshake {
    endpoint: String,
    token: String,
}

fn authorize<T>(auth: &ToolRunnerAuth, request: &Request<T>) -> Result<(), Status> {
    let Some(value) = request.metadata().get("authorization") else {
        return Err(Status::unauthenticated("missing authorization metadata"));
    };
    let value = value
        .to_str()
        .map_err(|_| Status::unauthenticated("invalid authorization metadata"))?;
    let expected = format!("Bearer {}", auth.token);
    if value != expected {
        return Err(Status::unauthenticated("invalid tool runner token"));
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        output.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_remote_tools::proto::execute_tool_event::Payload;
    use tempfile::tempdir;
    use tokio_stream::StreamExt;
    use tonic::metadata::MetadataValue;

    fn authorized_request<T>(message: T, token: &str) -> Request<T> {
        let mut request = Request::new(message);
        request.metadata_mut().insert(
            "authorization",
            MetadataValue::try_from(format!("Bearer {token}")).unwrap(),
        );
        request
    }

    #[test]
    fn load_or_generate_token_prefers_configured_value() {
        assert_eq!(load_or_generate_token(Some("secret")), "secret");
    }

    #[test]
    fn load_or_generate_token_generates_hex_token() {
        let token = load_or_generate_token(None);
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn print_handshake_writes_json_file() {
        let path =
            std::env::temp_dir().join(format!("puffer-tool-runner-{}.json", rand::random::<u64>()));
        print_handshake(&ToolRunnerOptions {
            endpoint: "http://127.0.0.1:7777".to_string(),
            token: "secret".to_string(),
            handshake_file: Some(path.clone()),
            print_stdout: false,
        })
        .unwrap();
        let json = fs::read_to_string(&path).unwrap();
        let handshake: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(handshake["endpoint"], "http://127.0.0.1:7777");
        assert_eq!(handshake["token"], "secret");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn describe_capabilities_requires_authorization() {
        let service = ToolRunnerService::new("secret".to_string());
        let error = service
            .describe_capabilities(Request::new(DescribeCapabilitiesRequest {}))
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unauthenticated);

        let response = service
            .describe_capabilities(authorized_request(DescribeCapabilitiesRequest {}, "secret"))
            .await
            .unwrap()
            .into_inner();
        assert!(response.supported_tools.iter().any(|tool| tool == "Bash"));
        assert!(response.streams_stdout_stderr);
    }

    #[tokio::test]
    async fn execute_tool_emits_completed_event_for_sleep() {
        let service = ToolRunnerService::new("secret".to_string());
        let cwd = std::env::temp_dir();
        let response = service
            .execute_tool(authorized_request(
                ExecuteToolRequest {
                    session_id: "00000000-0000-0000-0000-000000000000".to_string(),
                    call_id: "call-1".to_string(),
                    tool_id: "Sleep".to_string(),
                    input_json: r#"{"duration_ms":1,"reason":"test"}"#.to_string(),
                    cwd: cwd.display().to_string(),
                    working_dirs: vec![cwd.display().to_string()],
                    sandbox_mode: "workspace-write".to_string(),
                    execution_context_json: String::new(),
                },
                "secret",
            ))
            .await
            .unwrap();

        let mut stream = response.into_inner();
        let mut completed = None;
        while let Some(event) = stream.next().await {
            match event.unwrap().payload {
                Some(Payload::Completed(event)) => {
                    completed = Some(event);
                    break;
                }
                Some(Payload::Failed(event)) => panic!("unexpected failure: {}", event.message),
                Some(Payload::Stdout(_)) | Some(Payload::Stderr(_)) => {}
                None => panic!("missing tool event payload"),
            }
        }

        let completed = completed.expect("completion event");
        assert_eq!(completed.tool_id, "Sleep");
        assert!(completed.success);
        assert!(completed.stdout.contains("\"completed\": true"));
    }

    #[tokio::test]
    async fn load_project_resources_returns_workspace_files() {
        let temp = tempdir().unwrap();
        let project = temp.path();
        fs::create_dir_all(project.join("resources/prompts")).unwrap();
        fs::create_dir_all(project.join(".puffer/resources/skills/reviewer")).unwrap();
        fs::write(
            project.join("resources/prompts/review.yaml"),
            "id: review\ndescription: Review\ntemplate: remote\n",
        )
        .unwrap();
        fs::write(
            project.join(".puffer/resources/skills/reviewer/SKILL.md"),
            "---\nname: reviewer\n---\nRemote body\n",
        )
        .unwrap();

        let service = ToolRunnerService::new("secret".to_string());
        let response = service
            .load_project_resources(authorized_request(
                LoadProjectResourcesRequest {
                    project_root: project.display().to_string(),
                },
                "secret",
            ))
            .await
            .unwrap()
            .into_inner();

        assert!(response
            .files
            .iter()
            .any(|file| file.relative_path == "resources/prompts/review.yaml"));
        assert!(response
            .files
            .iter()
            .any(|file| file.relative_path == ".puffer/resources/skills/reviewer/SKILL.md"));
    }
}
