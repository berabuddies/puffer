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
use std::time::Duration;
use thiserror::Error;

/// Liveness reply from [`ToolRunner::ping`]. Returned after a successful
/// round-trip to the runner backend (in-process or remote).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerPing {
    /// `CARGO_PKG_VERSION` reported by the runner.
    pub version: String,
    /// Time since the runner instance was constructed.
    pub uptime: Duration,
}

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

/// SDK-free DTO mirroring the on-disk `PersistedTokens` payload from
/// `puffer-mcp-oauth`. This lets `puffer-cli` push a freshly minted token
/// bundle to a runner over `ToolRunner::push_oauth_tokens` without dragging
/// rmcp / oauth2 types across the trait boundary.
///
/// The fields are 1:1 with `puffer_mcp_oauth::PersistedTokens`; the
/// runner's `LocalToolRunner` impl reconstructs that type and writes it
/// through the existing file-backed credential store, so the on-disk shape
/// is identical to what `puffer mcp login` used to write directly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthTokensPayload {
    pub server_id: String,
    pub server_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub access_token: String,
    pub token_type: String,
    pub refresh_token: Option<String>,
    pub scopes: Vec<String>,
    /// Wall-clock expiry as `SystemTime` epoch milliseconds. Optional —
    /// some auth servers omit `expires_in` entirely (treated as
    /// "long-lived; refresh on demand from a 401" by the manager).
    pub expires_at_ms: Option<u64>,
}

/// Read-only summary of the runner's stored OAuth state for a server,
/// returned from [`ToolRunner::oauth_status`]. The CLI uses this to render
/// `puffer mcp login-status` without hitting the underlying token file
/// directly (which is the runner's concern, not the CLI's).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OAuthStatus {
    /// The runner has no stored credentials for this server.
    Absent,
    /// The runner has a stored bundle. The full secret material is never
    /// returned over the trait — only the metadata callers need to render
    /// human-friendly status output.
    Present {
        expires_at_ms: Option<u64>,
        has_refresh: bool,
        scopes: Vec<String>,
    },
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

    /// HTTP MCP server requires OAuth and no usable token is on disk.
    ///
    /// `authorization_url` is `Some` when discovery has run and the runner
    /// can hand the orchestrator a URL to redirect the user at; it's
    /// `None` for the silent-resolve path that surfaced this error
    /// before discovery completed (the caller should kick off
    /// `puffer mcp login <server_id>` to drive discovery + DCR + URL
    /// minting in one shot).
    #[error("OAuth required for MCP server `{server_id}`")]
    OAuthRequired {
        server_id: String,
        authorization_url: Option<String>,
    },

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

/// One server-initiated elicitation request, surfaced to the runner so the
/// configured [`ElicitationHandler`] can collect a user response.
///
/// `schema` is the JSON Schema (per MCP) describing the expected shape of the
/// `Accept` payload. `mode` distinguishes the two MCP elicitation styles
/// (form vs. URL); URL-mode requests carry the URL the user should visit and
/// an opaque `elicitation_id` minted by the server for correlation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationRequest {
    /// MCP server id (matches `McpServerInfo::id`).
    pub server: String,
    /// Tool that triggered the elicitation, when known. Empty when the
    /// originating call is not a `tools/call` (today never observed; kept
    /// for forward compatibility).
    pub tool: String,
    /// Server's prompt to the user.
    pub message: String,
    /// Form vs. URL elicitation style.
    #[serde(default)]
    pub mode: ElicitationMode,
    /// JSON Schema describing the expected Accept payload (form mode only).
    /// `Value::Null` for URL-mode requests.
    pub schema: serde_json::Value,
    /// URL the user is asked to visit (URL mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Server-minted correlation id for URL elicitations. `None` for form mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elicitation_id: Option<String>,
}

/// MCP elicitation style — see [`ElicitationRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationMode {
    /// Server expects a structured response matching `schema`.
    #[default]
    Form,
    /// Server is directing the user to a URL; the response only conveys
    /// accept/decline/cancel without any payload.
    Url,
}

/// User's reply to an [`ElicitationRequest`]. The runner relays this back
/// to the MCP server through rmcp.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum ElicitationResponse {
    /// User provided the requested value (must conform to `schema`).
    Accept { content: serde_json::Value },
    /// User refused but allows the operation to continue.
    Decline,
    /// User asked to cancel the entire operation.
    Cancel,
}

impl ElicitationResponse {
    /// Convenience constructor for `Accept` with the given JSON payload.
    pub fn accept(content: serde_json::Value) -> Self {
        ElicitationResponse::Accept { content }
    }
}

/// Strategy for fielding [`ElicitationRequest`]s. Runners hold an
/// `Arc<dyn ElicitationHandler>` configured at construction (default
/// [`DeclineAllElicitations`]); the connection manager invokes
/// [`ElicitationHandler::elicit`] whenever an MCP server sends an
/// `elicitation/create` mid-call.
///
/// The handler is **synchronous on purpose**. The rmcp adapter calls it from
/// `tokio::task::spawn_blocking`, which lets puffer's UI plumbing relay the
/// request to the user (TUI / web / etc.) without forcing every UI surface
/// to expose an async API. Implementations that need to block on a tokio
/// future should construct a fresh `current_thread` runtime; do **not** call
/// `block_on` on the manager's runtime.
pub trait ElicitationHandler: Send + Sync + std::fmt::Debug {
    /// Handles one elicitation request. The runner has already serialized
    /// the rmcp payload into the puffer-shaped DTO; the implementation only
    /// has to produce a response.
    fn elicit(&self, request: ElicitationRequest) -> ElicitationResponse;
}

/// Default `ElicitationHandler` that responds `Decline` to every request.
/// Mirrors rmcp's default `ClientHandler::create_elicitation` behavior.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeclineAllElicitations;

impl ElicitationHandler for DeclineAllElicitations {
    fn elicit(&self, _request: ElicitationRequest) -> ElicitationResponse {
        ElicitationResponse::Decline
    }
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
pub trait ToolRunner: Send + Sync + std::fmt::Debug {
    /// Cheap liveness probe. For the local backend this is purely
    /// in-memory; for the gRPC backend it issues a `Ping` RPC. Used by
    /// startup gates that wait for a runner to become reachable.
    fn ping(&self) -> Result<RunnerPing, RunnerError>;

    /// Returns the runner's self-reported capabilities.
    fn capabilities(&self) -> RunnerCapabilities;

    // --- Tool execution (model-facing) -------------------------------------

    /// Executes one tool call. Permission gating is the caller's
    /// responsibility — the runner has no permission concept.
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
    fn read_mcp_resource(&self, server: &str, uri: &str)
        -> Result<McpResourceContent, RunnerError>;
    fn list_mcp_prompts(&self, server: &str) -> Result<Vec<McpPrompt>, RunnerError>;
    fn get_mcp_prompt(
        &self,
        server: &str,
        name: &str,
        args: serde_json::Value,
    ) -> Result<McpPromptContent, RunnerError>;

    // --- OAuth credential push (puffer-cli drives the browser flow) -------
    //
    // These are no-ops by default so existing runners that don't care about
    // OAuth (test fakes, etc.) don't have to implement them. The local +
    // gRPC runners override them to write through to the file-backed
    // credential store / forward to the gRPC server respectively.

    /// Persist a freshly-minted OAuth token bundle for `server`. Called by
    /// `puffer-cli`'s `mcp login` command after the local interactive
    /// authorization-code flow completes. The runner persists the bundle
    /// to its on-disk credential store; subsequent MCP calls authenticated
    /// against `server` will use these tokens transparently.
    ///
    /// Default impl returns `Unsupported` so impls that don't speak OAuth
    /// can opt out.
    fn push_oauth_tokens(
        &self,
        _server: &str,
        _tokens: OAuthTokensPayload,
    ) -> Result<(), RunnerError> {
        Err(RunnerError::Unsupported(
            "push_oauth_tokens is not implemented for this runner".into(),
        ))
    }

    /// Returns whether the runner has stored credentials for `server`,
    /// plus enough non-secret metadata for the CLI to render
    /// `puffer mcp login-status` (expiry timestamp, presence of refresh
    /// token, granted scopes). Never returns the access token itself.
    fn oauth_status(&self, _server: &str) -> Result<OAuthStatus, RunnerError> {
        Err(RunnerError::Unsupported(
            "oauth_status is not implemented for this runner".into(),
        ))
    }

    /// Forget any stored OAuth credentials for `server`. Idempotent: when
    /// no credentials exist, returns `Ok(())`.
    fn clear_oauth_tokens(&self, _server: &str) -> Result<(), RunnerError> {
        Err(RunnerError::Unsupported(
            "clear_oauth_tokens is not implemented for this runner".into(),
        ))
    }
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

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

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
    fn decline_all_handler_returns_decline() {
        let handler = DeclineAllElicitations;
        let response = handler.elicit(ElicitationRequest {
            server: "stub".into(),
            tool: "ask".into(),
            message: "ok?".into(),
            mode: ElicitationMode::Form,
            schema: serde_json::json!({ "type": "object" }),
            url: None,
            elicitation_id: None,
        });
        assert!(matches!(response, ElicitationResponse::Decline));
    }

    #[test]
    fn elicitation_response_roundtrips_through_json() {
        let cases = [
            ElicitationResponse::Accept {
                content: serde_json::json!({ "x": 1 }),
            },
            ElicitationResponse::Decline,
            ElicitationResponse::Cancel,
        ];
        for resp in cases {
            let json = serde_json::to_value(&resp).unwrap();
            let back: ElicitationResponse = serde_json::from_value(json).unwrap();
            // Prove round-trip preserves the variant — use Debug since the
            // type is not Eq.
            assert_eq!(format!("{resp:?}"), format!("{back:?}"));
        }
    }

    #[test]
    fn oauth_tokens_payload_roundtrips_through_json() {
        let payload = OAuthTokensPayload {
            server_id: "github".into(),
            server_url: "https://mcp.example.com/v1".into(),
            client_id: "client-123".into(),
            client_secret: Some("secret".into()),
            access_token: "at-1".into(),
            token_type: "Bearer".into(),
            refresh_token: Some("rt-1".into()),
            scopes: vec!["read".into(), "write".into()],
            expires_at_ms: Some(1_700_000_000_000),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let back: OAuthTokensPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload, back);
    }

    #[test]
    fn oauth_status_present_roundtrips_through_json() {
        let cases = [
            OAuthStatus::Absent,
            OAuthStatus::Present {
                expires_at_ms: Some(1_700_000_000_000),
                has_refresh: true,
                scopes: vec!["repo".into()],
            },
        ];
        for status in cases {
            let json = serde_json::to_string(&status).unwrap();
            let back: OAuthStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn default_runner_oauth_methods_return_unsupported() {
        // Sanity-check that a runner that doesn't override the OAuth
        // methods bubbles up `Unsupported` rather than panicking.
        #[derive(Debug)]
        struct Stub;
        impl ToolRunner for Stub {
            fn ping(&self) -> Result<RunnerPing, RunnerError> {
                unimplemented!()
            }
            fn capabilities(&self) -> RunnerCapabilities {
                Default::default()
            }
            fn execute_tool(
                &self,
                _: ToolRequest,
                _: &mut dyn ChunkSink,
            ) -> Result<ToolResult, RunnerError> {
                unimplemented!()
            }
            fn read_file(&self, _: &Path) -> Result<Vec<u8>, RunnerError> {
                unimplemented!()
            }
            fn list_dir(&self, _: &Path) -> Result<Vec<DirEntry>, RunnerError> {
                unimplemented!()
            }
            fn glob(&self, _: &Path, _: &str) -> Result<Vec<PathBuf>, RunnerError> {
                unimplemented!()
            }
            fn list_mcp_servers(&self) -> Result<Vec<McpServerInfo>, RunnerError> {
                unimplemented!()
            }
            fn list_mcp_tools(&self, _: &str) -> Result<Vec<McpTool>, RunnerError> {
                unimplemented!()
            }
            fn call_mcp_tool(
                &self,
                _: &str,
                _: &str,
                _: serde_json::Value,
                _: &mut dyn ChunkSink,
            ) -> Result<McpResult, RunnerError> {
                unimplemented!()
            }
            fn list_mcp_resources(
                &self,
                _: Option<&str>,
            ) -> Result<Vec<McpResourceRecord>, RunnerError> {
                unimplemented!()
            }
            fn read_mcp_resource(
                &self,
                _: &str,
                _: &str,
            ) -> Result<McpResourceContent, RunnerError> {
                unimplemented!()
            }
            fn list_mcp_prompts(&self, _: &str) -> Result<Vec<McpPrompt>, RunnerError> {
                unimplemented!()
            }
            fn get_mcp_prompt(
                &self,
                _: &str,
                _: &str,
                _: serde_json::Value,
            ) -> Result<McpPromptContent, RunnerError> {
                unimplemented!()
            }
        }
        let stub = Stub;
        let payload = OAuthTokensPayload {
            server_id: "x".into(),
            server_url: "https://x".into(),
            client_id: "c".into(),
            client_secret: None,
            access_token: "a".into(),
            token_type: "Bearer".into(),
            refresh_token: None,
            scopes: vec![],
            expires_at_ms: None,
        };
        assert!(matches!(
            stub.push_oauth_tokens("x", payload).unwrap_err(),
            RunnerError::Unsupported(_)
        ));
        assert!(matches!(
            stub.oauth_status("x").unwrap_err(),
            RunnerError::Unsupported(_)
        ));
        assert!(matches!(
            stub.clear_oauth_tokens("x").unwrap_err(),
            RunnerError::Unsupported(_)
        ));
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
