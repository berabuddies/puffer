use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// gRPC proto bindings for the remote tool-runner service.
pub mod proto {
    tonic::include_proto!("puffer.remote_tools.v1");
}

/// Carries one tool-execution request from the local runtime to the remote runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteToolRequest {
    pub session_id: String,
    pub call_id: String,
    pub tool_id: String,
    pub input_json: String,
    pub cwd: String,
    pub working_dirs: Vec<String>,
    pub sandbox_mode: String,
    pub execution_context_json: Option<String>,
}

/// Carries provider-specific execution context for remote-only tool behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "request", rename_all = "snake_case")]
pub enum RemoteToolExecutionContext {
    WebSearch(RemoteWebSearchRequest),
}

/// Identifies which response parser one remote WebSearch request needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteWebSearchBackend {
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "anthropic")]
    Anthropic,
}

/// Stores one fully built HTTP request for remote WebSearch execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteWebSearchRequest {
    pub backend: RemoteWebSearchBackend,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Identifies which output stream emitted one chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteToolChunkStream {
    Stdout,
    Stderr,
}

/// Carries one streamed stdout/stderr chunk from the remote runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteToolChunk {
    pub stream: RemoteToolChunkStream,
    pub text: String,
}

/// Describes the capabilities exposed by one remote tool-runner instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteToolCapabilities {
    pub version: String,
    pub supported_tools: Vec<String>,
    pub streams_stdout_stderr: bool,
}

/// Carries one remote project resource file returned by the tool runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteProjectResourceFile {
    pub relative_path: String,
    pub content: String,
}
impl From<RemoteToolRequest> for proto::ExecuteToolRequest {
    fn from(value: RemoteToolRequest) -> Self {
        Self {
            session_id: value.session_id,
            call_id: value.call_id,
            tool_id: value.tool_id,
            input_json: value.input_json,
            cwd: value.cwd,
            working_dirs: value.working_dirs,
            sandbox_mode: value.sandbox_mode,
            execution_context_json: value.execution_context_json.unwrap_or_default(),
        }
    }
}

impl TryFrom<proto::ExecuteToolRequest> for RemoteToolRequest {
    type Error = anyhow::Error;

    fn try_from(value: proto::ExecuteToolRequest) -> Result<Self> {
        if value.tool_id.trim().is_empty() {
            return Err(anyhow!("tool_id must not be empty"));
        }
        if value.cwd.trim().is_empty() {
            return Err(anyhow!("cwd must not be empty"));
        }
        Ok(Self {
            session_id: value.session_id,
            call_id: value.call_id,
            tool_id: value.tool_id,
            input_json: value.input_json,
            cwd: value.cwd,
            working_dirs: value.working_dirs,
            sandbox_mode: value.sandbox_mode,
            execution_context_json: (!value.execution_context_json.trim().is_empty())
                .then_some(value.execution_context_json),
        })
    }
}

impl From<proto::DescribeCapabilitiesResponse> for RemoteToolCapabilities {
    fn from(value: proto::DescribeCapabilitiesResponse) -> Self {
        Self {
            version: value.version,
            supported_tools: value.supported_tools,
            streams_stdout_stderr: value.streams_stdout_stderr,
        }
    }
}

impl From<proto::ProjectResourceFile> for RemoteProjectResourceFile {
    fn from(value: proto::ProjectResourceFile) -> Self {
        Self {
            relative_path: value.relative_path,
            content: value.content,
        }
    }
}
