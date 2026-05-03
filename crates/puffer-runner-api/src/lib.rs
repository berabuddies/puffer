//! Trait surface and DTOs for the puffer tool runner.
//!
//! A `ToolRunner` abstracts everything that historically required direct
//! filesystem / process / MCP access from inside the puffer runtime. There
//! are two concrete implementations:
//!
//! * `LocalToolRunner` (in `puffer-runner-local`) — runs in-process and
//!   delegates to the existing claude-parity executors.
//! * `RemoteToolRunner` (planned, in `puffer-runner-grpc`) — forwards every
//!   call to a remote `puffer-tool-runner` server over gRPC.
//!
//! All DTOs are owned (no borrowed references to runtime state) so the same
//! types can cross a gRPC boundary unchanged.
//!
//! Note: this is the Phase 0 trait extraction. Call-site refactors that make
//! the runtime hold an `Arc<dyn ToolRunner>` instead of calling concrete
//! functions directly are not yet wired in — see the crate-level README and
//! task notes for the remaining phases.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Coarse capability flags reported by a runner so the client can adapt.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunnerCapabilities {
    /// Stable identifier for the implementation (e.g. `"local"`, `"grpc"`).
    pub backend: String,
    /// Human-readable build/version string.
    pub version: String,
    /// Set of tool ids the runner is willing to execute. Empty means
    /// "the standard claude-parity 10".
    pub supported_tools: Vec<String>,
    /// True when the runner brokers MCP server lifecycle.
    pub mcp_supported: bool,
    /// True when the runner can forward permission prompts back to the
    /// caller via `request_permission`.
    pub permission_relay_supported: bool,
}

/// Identifies one tool execution attempt for transport / logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequest {
    /// Tool id (e.g. `"Bash"`, `"Read"`, `"Edit"`).
    pub tool_id: String,
    /// Working directory for the call.
    pub cwd: PathBuf,
    /// Additional roots that the sandbox treats as in-bounds for path
    /// resolution (the session's `working_dirs` list).
    #[serde(default)]
    pub working_dirs: Vec<PathBuf>,
    /// When true, path-sandbox checks are bypassed (`danger-full-access`).
    #[serde(default)]
    pub allow_all_paths: bool,
    /// Tool input as a JSON value (matches the tool's input schema).
    pub input: serde_json::Value,
    /// Optional opaque session token used for tying related calls together
    /// (e.g. background bash processes share a session id).
    pub session_id: Option<String>,
}

/// One read-state-relevant fact reported by `ToolRunner::execute_tool`.
///
/// The runner is a pure function of (request, filesystem) → result and does
/// not own per-session staleness tracking. After a successful call the
/// dispatcher applies these updates to whatever in-memory map it keeps for
/// the running session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadStateUpdate {
    pub path: PathBuf,
    pub timestamp_ms: u128,
    pub is_partial_view: bool,
}

/// Final, non-streaming summary returned by a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_id: String,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub metadata: serde_json::Value,
    /// Read-state-relevant facts the dispatcher should apply after a
    /// successful call. Empty for tools that don't touch tracked files.
    #[serde(default)]
    pub read_state_updates: Vec<ReadStateUpdate>,
}

/// One entry returned by `ToolRunner::list_dir`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
}

/// Lightweight description of one MCP server known to the runner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub id: String,
    pub display_name: String,
    pub transport: String,
    pub target: String,
    pub description: String,
}

/// Lightweight description of one MCP tool exposed by a server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Option<serde_json::Value>,
}

/// One MCP resource record (URI + display metadata).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceRecord {
    pub server: String,
    pub uri: String,
    pub name: String,
    pub mime_type: Option<String>,
    pub description: Option<String>,
}

/// One piece of content returned by an MCP `resources/read` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpResourceContentPart {
    Text {
        uri: String,
        mime_type: Option<String>,
        text: String,
    },
    Blob {
        uri: String,
        mime_type: Option<String>,
        #[serde(with = "serde_bytes_compat")]
        bytes: Vec<u8>,
    },
}

/// Container for one or more `McpResourceContentPart`s.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceContent {
    pub server: String,
    pub uri: String,
    pub parts: Vec<McpResourceContentPart>,
}

/// Description of one MCP prompt template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<McpPromptArgument>,
}

/// One declared argument to an MCP prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgument {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
}

/// Rendered MCP prompt content (one or more messages).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptContent {
    pub server: String,
    pub name: String,
    pub messages: Vec<McpPromptMessage>,
}

/// One message in a rendered MCP prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptMessage {
    pub role: String,
    pub text: String,
}

/// Final result of an MCP tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResult {
    pub server: String,
    pub tool: String,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub metadata: serde_json::Value,
}

/// Permission-prompt request sent from the runner back to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub tool_id: String,
    pub cwd: PathBuf,
    pub input: serde_json::Value,
    pub reason: Option<String>,
}

/// Outcome the client returns to the runner for a permission prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    AllowOnce,
    AllowSession,
    AllowAllSession,
    Deny,
}

/// Typed errors returned from runner methods. Designed to round-trip cleanly
/// over a gRPC status code.
#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("requested operation is unsupported by this runner: {0}")]
    Unsupported(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("tool execution failed: {0}")]
    Execution(String),

    #[error("internal runner error: {0}")]
    Other(String),
}

impl RunnerError {
    pub fn other<E: std::fmt::Display>(error: E) -> Self {
        RunnerError::Other(error.to_string())
    }

    pub fn execution<E: std::fmt::Display>(error: E) -> Self {
        RunnerError::Execution(error.to_string())
    }

    pub fn mcp<E: std::fmt::Display>(error: E) -> Self {
        RunnerError::Mcp(error.to_string())
    }
}

/// Per-session snapshot used by the dispatcher's pre-flight staleness check.
///
/// Mirrors the runtime's `ClaudeReadState` exactly so the dispatcher can
/// hand its in-memory map to the helper without copying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadStateSnapshot {
    pub timestamp_ms: u128,
    pub is_partial_view: bool,
}

/// Reasons the dispatcher's pre-flight staleness gate may reject a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StalenessRejection {
    /// The path was never (fully) read in this session.
    NotRead,
    /// The path was read, but the on-disk mtime has advanced since.
    StaleRead,
}

impl StalenessRejection {
    pub const NOT_READ_MESSAGE: &'static str =
        "File has not been read yet. Read it first before writing to it.";
    pub const STALE_READ_MESSAGE: &'static str = "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it.";

    pub fn message(&self) -> &'static str {
        match self {
            StalenessRejection::NotRead => Self::NOT_READ_MESSAGE,
            StalenessRejection::StaleRead => Self::STALE_READ_MESSAGE,
        }
    }
}

/// Validates that a tool requiring a prior full-file Read may proceed.
///
/// `current_mtime_ms` is the on-disk mtime of `path` at dispatch time;
/// callers compute it from `std::fs::metadata` (or the runner's
/// `read_file` future equivalent).
pub fn check_read_freshness(
    snapshot: Option<&ReadStateSnapshot>,
    current_mtime_ms: u128,
) -> Result<(), StalenessRejection> {
    let Some(snapshot) = snapshot else {
        return Err(StalenessRejection::NotRead);
    };
    if snapshot.is_partial_view {
        return Err(StalenessRejection::NotRead);
    }
    if current_mtime_ms > snapshot.timestamp_ms {
        return Err(StalenessRejection::StaleRead);
    }
    Ok(())
}

/// Streaming sink for partial output from long-running tools.
///
/// Implemented in-process by an adapter wrapping a closure; remotely by the
/// gRPC server-streaming response.
pub trait ChunkSink: Send {
    /// Append a chunk to the tool's stdout stream.
    fn stdout(&mut self, chunk: &[u8]);
    /// Append a chunk to the tool's stderr stream.
    fn stderr(&mut self, chunk: &[u8]);
    /// Optional: emit a JSON event (e.g. progress / partial MCP content).
    fn event(&mut self, _event: serde_json::Value) {}
}

/// A no-op `ChunkSink` for callers that only need the final result.
pub struct NullChunkSink;

impl ChunkSink for NullChunkSink {
    fn stdout(&mut self, _chunk: &[u8]) {}
    fn stderr(&mut self, _chunk: &[u8]) {}
}

/// Adapter `ChunkSink` backed by an in-process closure.
pub struct FnChunkSink<F: FnMut(ChunkKind, &[u8]) + Send> {
    callback: F,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    Stdout,
    Stderr,
}

impl<F: FnMut(ChunkKind, &[u8]) + Send> FnChunkSink<F> {
    pub fn new(callback: F) -> Self {
        Self { callback }
    }
}

impl<F: FnMut(ChunkKind, &[u8]) + Send> ChunkSink for FnChunkSink<F> {
    fn stdout(&mut self, chunk: &[u8]) {
        (self.callback)(ChunkKind::Stdout, chunk);
    }
    fn stderr(&mut self, chunk: &[u8]) {
        (self.callback)(ChunkKind::Stderr, chunk);
    }
}

/// The unified tool runner trait. Instances are wrapped in `Arc<dyn ToolRunner>`
/// at runtime startup; the rest of the codebase doesn't care whether the
/// underlying implementation is local or remote.
pub trait ToolRunner: Send + Sync {
    /// Returns the runner's self-reported capabilities.
    fn capabilities(&self) -> RunnerCapabilities;

    // --- Tool execution (model-facing) -------------------------------------

    /// Executes one tool call. Permission gating is the caller's
    /// responsibility unless `permission_relay_supported` is set.
    fn execute_tool(
        &self,
        req: ToolRequest,
        sink: &mut dyn ChunkSink,
    ) -> Result<ToolResult, RunnerError>;

    // --- Raw filesystem (runtime-internal, no model formatting) ------------

    /// Reads raw bytes from `path`. No permission prompt; intended for
    /// loading AGENTS.md / CLAUDE.md / SKILL.md / mcp.json.
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, RunnerError>;

    /// Returns one level of directory entries.
    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, RunnerError>;

    /// Resolves a glob pattern relative to `root`.
    fn glob(&self, root: &Path, pattern: &str) -> Result<Vec<PathBuf>, RunnerError>;

    // --- MCP --------------------------------------------------------------

    fn list_mcp_servers(&self) -> Result<Vec<McpServerInfo>, RunnerError>;
    fn list_mcp_tools(&self, server: &str) -> Result<Vec<McpTool>, RunnerError>;
    fn call_mcp_tool(
        &self,
        server: &str,
        tool: &str,
        args: serde_json::Value,
        sink: &mut dyn ChunkSink,
    ) -> Result<McpResult, RunnerError>;
    fn list_mcp_resources(
        &self,
        server: Option<&str>,
    ) -> Result<Vec<McpResourceRecord>, RunnerError>;
    fn read_mcp_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> Result<McpResourceContent, RunnerError>;
    fn list_mcp_prompts(&self, server: &str) -> Result<Vec<McpPrompt>, RunnerError>;
    fn get_mcp_prompt(
        &self,
        server: &str,
        name: &str,
        args: serde_json::Value,
    ) -> Result<McpPromptContent, RunnerError>;

    // --- Permission relay -------------------------------------------------

    /// Asks the upstream caller (UI / TUI / human operator) to approve a
    /// pending tool call. Local runners may delegate to an in-process
    /// permission decider; remote runners proxy this over the bidirectional
    /// `PermissionChannel` stream.
    fn request_permission(
        &self,
        req: PermissionRequest,
    ) -> Result<PermissionDecision, RunnerError>;
}

/// Bytes serializer that round-trips through both bincode-style and JSON
/// transports. Uses base64 in JSON, a raw byte sequence elsewhere.
mod serde_bytes_compat {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            // Crude inline base64 — keeps the dependency surface small.
            base64_encode(bytes).serialize(serializer)
        } else {
            serializer.serialize_bytes(bytes)
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let encoded = String::deserialize(deserializer)?;
            base64_decode(&encoded).map_err(serde::de::Error::custom)
        } else {
            <Vec<u8>>::deserialize(deserializer)
        }
    }

    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn base64_encode(input: &[u8]) -> String {
        let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
        for chunk in input.chunks(3) {
            let b0 = chunk[0];
            let b1 = chunk.get(1).copied().unwrap_or(0);
            let b2 = chunk.get(2).copied().unwrap_or(0);
            out.push(ALPHABET[(b0 >> 2) as usize] as char);
            out.push(ALPHABET[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize] as char);
            if chunk.len() > 1 {
                out.push(ALPHABET[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(ALPHABET[(b2 & 0b111111) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
        let trimmed = input.trim_end_matches('=');
        let mut decoded_bits: Vec<u8> = Vec::with_capacity(trimmed.len() * 3 / 4);
        let mut buffer: u32 = 0;
        let mut bits: u8 = 0;
        for ch in trimmed.bytes() {
            let value = ALPHABET
                .iter()
                .position(|c| *c == ch)
                .ok_or_else(|| format!("invalid base64 character: {ch:#x}"))?
                as u32;
            buffer = (buffer << 6) | value;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                decoded_bits.push((buffer >> bits) as u8 & 0xff);
            }
        }
        Ok(decoded_bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_chunk_sink_swallows_data() {
        let mut sink = NullChunkSink;
        sink.stdout(b"hello");
        sink.stderr(b"world");
    }

    #[test]
    fn fn_chunk_sink_routes_kinds() {
        let mut buf: Vec<(ChunkKind, Vec<u8>)> = Vec::new();
        {
            let mut sink = FnChunkSink::new(|kind, chunk| buf.push((kind, chunk.to_vec())));
            sink.stdout(b"out");
            sink.stderr(b"err");
        }
        assert_eq!(buf.len(), 2);
        assert_eq!(buf[0].0, ChunkKind::Stdout);
        assert_eq!(buf[1].0, ChunkKind::Stderr);
    }

    #[test]
    fn runner_error_roundtrip_messages() {
        let err = RunnerError::execution("boom");
        assert_eq!(err.to_string(), "tool execution failed: boom");
    }

    #[test]
    fn mcp_resource_content_blob_roundtrips_through_json() {
        let content = McpResourceContent {
            server: "filesystem".into(),
            uri: "mcp://filesystem/x".into(),
            parts: vec![McpResourceContentPart::Blob {
                uri: "mcp://filesystem/x".into(),
                mime_type: Some("application/octet-stream".into()),
                bytes: vec![0xde, 0xad, 0xbe, 0xef],
            }],
        };
        let json = serde_json::to_string(&content).unwrap();
        let back: McpResourceContent = serde_json::from_str(&json).unwrap();
        match &back.parts[0] {
            McpResourceContentPart::Blob { bytes, .. } => {
                assert_eq!(bytes, &vec![0xde, 0xad, 0xbe, 0xef]);
            }
            _ => panic!("expected blob"),
        }
    }
}
