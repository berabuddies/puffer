//! Shared wire protocol for first-party internal tool permission and execution callbacks.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

/// Environment variable containing the localhost permission callback address.
pub const INTERNAL_PERMISSION_ADDR_ENV: &str = "PUFFER_INTERNAL_PERMISSION_ADDR";

/// Environment variable containing the bearer token for permission callbacks.
pub const INTERNAL_PERMISSION_TOKEN_ENV: &str = "PUFFER_INTERNAL_PERMISSION_TOKEN";

/// Environment variable marking internal permission as mandatory for this child.
pub const INTERNAL_PERMISSION_REQUIRED_ENV: &str = "PUFFER_INTERNAL_PERMISSION_REQUIRED";

/// Current version of the internal permission callback protocol.
pub const INTERNAL_PERMISSION_PROTOCOL_VERSION: &str = "1";

/// Describes a structured permission request from an internal tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalToolPermissionRequest {
    /// Stable internal tool id, such as `browser`.
    pub tool_id: String,
    /// Tool-native structured input for the attempted action.
    #[serde(default)]
    pub input: Value,
}

/// Wire envelope sent by internal tools to the parent runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalToolPermissionEnvelope {
    /// Protocol version used by the sender.
    pub protocol_version: String,
    /// Bearer token copied from [`INTERNAL_PERMISSION_TOKEN_ENV`].
    pub token: String,
    /// Structured permission request.
    pub request: InternalToolPermissionRequest,
}

/// Describes a structured execution request from an internal tool CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalToolExecutionRequest {
    /// Stable internal tool id, such as `email` or `telegram`.
    pub tool_id: String,
    /// Tool-native structured input for the attempted action.
    #[serde(default)]
    pub input: Value,
}

/// Wire envelope used when an internal CLI asks the parent runtime to execute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalToolExecutionEnvelope {
    /// Protocol version used by the sender.
    pub protocol_version: String,
    /// Bearer token copied from [`INTERNAL_PERMISSION_TOKEN_ENV`].
    pub token: String,
    /// Structured execution request.
    pub execution: InternalToolExecutionRequest,
}

/// Permission decision returned by the parent runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InternalToolPermissionDecision {
    Allow,
    Deny,
}

/// Response returned by the parent runtime for an internal tool preflight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalToolPermissionResponse {
    /// Final permission decision after policy and any user prompt.
    pub decision: InternalToolPermissionDecision,
    /// Optional denial or diagnostic reason.
    #[serde(default)]
    pub reason: Option<String>,
}

impl InternalToolPermissionResponse {
    /// Builds an allow response.
    pub fn allow() -> Self {
        Self {
            decision: InternalToolPermissionDecision::Allow,
            reason: None,
        }
    }

    /// Builds a deny response with a human-readable reason.
    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            decision: InternalToolPermissionDecision::Deny,
            reason: Some(reason.into()),
        }
    }

    /// Returns true when the response allows execution.
    pub fn is_allowed(&self) -> bool {
        self.decision == InternalToolPermissionDecision::Allow
    }
}

/// Response returned by the parent runtime after an internal execution request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalToolExecutionResponse {
    /// Whether the parent runtime completed the execution successfully.
    pub success: bool,
    /// Tool output, normally a JSON string, when execution succeeded.
    #[serde(default)]
    pub output: Option<String>,
    /// Human-readable denial or failure reason.
    #[serde(default)]
    pub reason: Option<String>,
    /// Optional structured execution diagnostic.
    #[serde(default)]
    pub diagnostic: Option<serde_json::Value>,
}

impl InternalToolExecutionResponse {
    /// Builds a successful execution response.
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            reason: None,
            diagnostic: None,
        }
    }

    /// Builds a failed execution response with a human-readable reason.
    pub fn failure(reason: impl Into<String>) -> Self {
        Self {
            success: false,
            output: None,
            reason: Some(reason.into()),
            diagnostic: None,
        }
    }

    /// Builds a failed execution response with a structured diagnostic.
    pub fn failure_with_diagnostic(
        reason: impl Into<String>,
        diagnostic: serde_json::Value,
    ) -> Self {
        Self {
            success: false,
            output: None,
            reason: Some(reason.into()),
            diagnostic: Some(diagnostic),
        }
    }
}

/// Sends a structured permission request to the parent runtime when available.
///
/// Returns `Ok(None)` when the environment does not contain an internal
/// permission callback endpoint. Direct human CLI invocations use that path.
pub fn request_internal_tool_permission_from_env(
    request: InternalToolPermissionRequest,
) -> Result<Option<InternalToolPermissionResponse>> {
    request_internal_tool_permission_from_env_impl(request, false)
}

/// Sends a structured permission request to the parent runtime and requires it.
pub fn require_internal_tool_permission_from_env(
    request: InternalToolPermissionRequest,
) -> Result<InternalToolPermissionResponse> {
    request_internal_tool_permission_from_env_impl(request, true)?
        .context("internal permission endpoint is required but unavailable")
}

/// Sends an execution request to the parent runtime and requires the callback.
pub fn require_internal_tool_execution_from_env(
    request: InternalToolExecutionRequest,
) -> Result<InternalToolExecutionResponse> {
    let Some(addr) = std::env::var(INTERNAL_PERMISSION_ADDR_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        bail!("internal execution endpoint is required but unavailable");
    };
    let token = std::env::var(INTERNAL_PERMISSION_TOKEN_ENV)
        .context("internal execution endpoint is missing its token")?;
    request_internal_tool_execution(&addr, &token, request)
}

fn request_internal_tool_permission_from_env_impl(
    request: InternalToolPermissionRequest,
    required_by_caller: bool,
) -> Result<Option<InternalToolPermissionResponse>> {
    let required = required_by_caller || internal_permission_required_from_env();
    let Some(addr) = std::env::var(INTERNAL_PERMISSION_ADDR_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        if required {
            bail!("internal permission endpoint is required but unavailable");
        }
        return Ok(None);
    };
    let token = std::env::var(INTERNAL_PERMISSION_TOKEN_ENV)
        .context("internal permission endpoint is missing its token")?;
    request_internal_tool_permission(&addr, &token, request).map(Some)
}

fn internal_permission_required_from_env() -> bool {
    std::env::var(INTERNAL_PERMISSION_REQUIRED_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .is_some_and(|value| !matches!(value.as_str(), "" | "0" | "false" | "no"))
}

/// Sends a structured permission request to a specific callback endpoint.
pub fn request_internal_tool_permission(
    addr: &str,
    token: &str,
    request: InternalToolPermissionRequest,
) -> Result<InternalToolPermissionResponse> {
    let mut stream = TcpStream::connect(addr)
        .with_context(|| format!("connect internal permission endpoint {addr}"))?;
    let envelope = InternalToolPermissionEnvelope {
        protocol_version: INTERNAL_PERMISSION_PROTOCOL_VERSION.to_string(),
        token: token.to_string(),
        request,
    };
    let line = serde_json::to_string(&envelope)?;
    stream
        .write_all(line.as_bytes())
        .context("write internal permission request")?;
    stream
        .write_all(b"\n")
        .context("finish internal permission request")?;
    stream
        .flush()
        .context("flush internal permission request")?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .context("read internal permission response")?;
    serde_json::from_str(response.trim()).context("decode internal permission response")
}

/// Sends a structured execution request to a specific callback endpoint.
pub fn request_internal_tool_execution(
    addr: &str,
    token: &str,
    request: InternalToolExecutionRequest,
) -> Result<InternalToolExecutionResponse> {
    let mut stream = TcpStream::connect(addr)
        .with_context(|| format!("connect internal execution endpoint {addr}"))?;
    let envelope = InternalToolExecutionEnvelope {
        protocol_version: INTERNAL_PERMISSION_PROTOCOL_VERSION.to_string(),
        token: token.to_string(),
        execution: request,
    };
    let line = serde_json::to_string(&envelope)?;
    stream
        .write_all(line.as_bytes())
        .context("write internal execution request")?;
    stream
        .write_all(b"\n")
        .context("finish internal execution request")?;
    stream.flush().context("flush internal execution request")?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .context("read internal execution response")?;
    serde_json::from_str(response.trim()).context("decode internal execution response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn permission_request_round_trips_over_localhost_protocol() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let envelope: InternalToolPermissionEnvelope =
                serde_json::from_str(line.trim()).unwrap();
            assert_eq!(
                envelope.protocol_version,
                INTERNAL_PERMISSION_PROTOCOL_VERSION
            );
            assert_eq!(envelope.token, "test-token");
            assert_eq!(envelope.request.tool_id, "browser");

            let mut stream = reader.into_inner();
            let response = serde_json::to_string(&InternalToolPermissionResponse::allow()).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(b"\n").unwrap();
        });

        let response = request_internal_tool_permission(
            &addr.to_string(),
            "test-token",
            InternalToolPermissionRequest {
                tool_id: "browser".to_string(),
                input: serde_json::json!({"action":"snapshot"}),
            },
        )
        .unwrap();

        assert!(response.is_allowed());
        server.join().unwrap();
    }

    #[test]
    fn execution_request_round_trips_over_localhost_protocol() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let envelope: InternalToolExecutionEnvelope =
                serde_json::from_str(line.trim()).unwrap();
            assert_eq!(
                envelope.protocol_version,
                INTERNAL_PERMISSION_PROTOCOL_VERSION
            );
            assert_eq!(envelope.token, "test-token");
            assert_eq!(envelope.execution.tool_id, "telegram");

            let mut stream = reader.into_inner();
            let response = serde_json::to_string(&InternalToolExecutionResponse::success(
                r#"{"status":"ok"}"#,
            ))
            .unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(b"\n").unwrap();
        });

        let response = request_internal_tool_execution(
            &addr.to_string(),
            "test-token",
            InternalToolExecutionRequest {
                tool_id: "telegram".to_string(),
                input: serde_json::json!({"action":"login_start"}),
            },
        )
        .unwrap();

        assert!(response.success);
        assert_eq!(response.output.as_deref(), Some(r#"{"status":"ok"}"#));
        server.join().unwrap();
    }

    #[test]
    fn execution_failure_serializes_diagnostic() {
        let response = InternalToolExecutionResponse::failure_with_diagnostic(
            "submit failed",
            serde_json::json!({
                "kind": "video",
                "provider": "worldrouter",
                "phase": "submit",
                "error": "low credits"
            }),
        );

        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["success"], false);
        assert_eq!(value["reason"], "submit failed");
        assert_eq!(value["diagnostic"]["provider"], "worldrouter");
    }
}
