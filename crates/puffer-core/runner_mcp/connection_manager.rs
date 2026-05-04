//! Long-lived MCP client multiplexer.
//!
//! Conceptually mirrors `references/codex/codex-rs/codex-mcp/src/connection_manager.rs`,
//! cut down to the pass-1.5a scope:
//!
//! * Stdio transport only.
//! * `tools/list` + `tools/call` only — no resources, prompts, sampling, or
//!   progress streaming through this surface (built-in resources/prompts
//!   continue to flow through [`McpHost`](super::host::McpHost) directly).
//! * Lazy connect: the child process is spawned on the first call to a
//!   given server id. Subsequent calls reuse the connection.
//! * Crash recovery: a respawn is attempted on the next call after the
//!   transport drops. A bounded retry counter (3 attempts in 60 s) protects
//!   against tight respawn loops on broken configs.
//! * Drop semantics: dropping the manager spawns a best-effort `shutdown`
//!   on each live client and lets `kill_on_drop` clean up the children.
//!
//! The manager owns its own tokio runtime (multi-thread). All public methods
//! are sync and `block_on` into that runtime — the trait surface
//! ([`puffer_runner_api::ToolRunner`]) is sync for parity with
//! `RemoteToolRunner`, which is itself sync over a shared tokio runtime.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use puffer_runner_api::{McpResult, McpTool, RunnerError};
use rmcp::model::{CallToolRequestParams, JsonObject, RawContent};
use rmcp::service::{RoleClient, RunningService, ServiceExt};
use rmcp::transport::child_process::TokioChildProcess;
use serde_json::{Map, Value};
use tokio::runtime::Runtime;

use super::launcher::spawn_stdio_child;
use super::transport::{StdioTransportSpec, TransportRecipe};

/// Bounded-retry policy: at most `MAX_RETRIES` failed launches inside the
/// rolling `RETRY_WINDOW`. After that, calls fail fast until the window
/// elapses (the next attempt then resets the counter).
const MAX_RETRIES: u32 = 3;
const RETRY_WINDOW: Duration = Duration::from_secs(60);

/// One configured MCP server known to the manager.
#[derive(Debug, Clone)]
pub struct ConnectionEntry {
    pub server_id: String,
    pub recipe: TransportRecipe,
}

impl ConnectionEntry {
    pub fn new(server_id: impl Into<String>, recipe: TransportRecipe) -> Self {
        Self {
            server_id: server_id.into(),
            recipe,
        }
    }
}

/// Per-server connection state. Lives behind a `Mutex` inside the manager.
struct ServerSlot {
    recipe: TransportRecipe,
    /// Currently live rmcp client, if any. Stored as a fresh handle each
    /// time we (re)connect; `Drop` of the client triggers child shutdown.
    client: Option<Arc<RunningService<RoleClient, ()>>>,
    /// Recent launch attempts, used for the bounded-retry budget.
    failure_history: Vec<Instant>,
}

impl ServerSlot {
    fn new(recipe: TransportRecipe) -> Self {
        Self {
            recipe,
            client: None,
            failure_history: Vec::new(),
        }
    }

    fn record_failure(&mut self) {
        let now = Instant::now();
        self.failure_history.retain(|t| now.duration_since(*t) <= RETRY_WINDOW);
        self.failure_history.push(now);
    }

    fn retries_exhausted(&mut self) -> bool {
        let now = Instant::now();
        self.failure_history.retain(|t| now.duration_since(*t) <= RETRY_WINDOW);
        self.failure_history.len() >= MAX_RETRIES as usize
    }
}

/// Multiplexes MCP server connections behind a synchronous façade.
pub struct McpConnectionManager {
    /// Configured servers. Cloned once at construction; subsequent edits go
    /// through `with_servers` (used only by tests).
    servers: HashMap<String, Mutex<ServerSlot>>,
    /// Lazy tokio runtime that owns every running rmcp client. Reused across
    /// calls so each `block_on` is just a context switch, not a runtime
    /// spin-up. Held in an `Arc` so `Drop` can move it onto a background
    /// thread for orderly shutdown.
    runtime: OnceLock<Arc<Runtime>>,
}

impl std::fmt::Debug for McpConnectionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<&String> = self.servers.keys().collect();
        f.debug_struct("McpConnectionManager")
            .field("servers", &ids)
            .finish()
    }
}

impl Default for McpConnectionManager {
    fn default() -> Self {
        Self {
            servers: HashMap::new(),
            runtime: OnceLock::new(),
        }
    }
}

impl McpConnectionManager {
    /// Builds an empty manager (no MCP servers registered).
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a manager pre-populated with the given server entries.
    pub fn with_servers(entries: impl IntoIterator<Item = ConnectionEntry>) -> Self {
        let mut servers = HashMap::new();
        for entry in entries {
            servers.insert(
                entry.server_id.to_ascii_lowercase(),
                Mutex::new(ServerSlot::new(entry.recipe)),
            );
        }
        Self {
            servers,
            runtime: OnceLock::new(),
        }
    }

    /// Returns true when the manager has any subprocess-style MCP server
    /// registered. Used to decide whether `McpHost` should bother spinning
    /// up the runtime for tools/prompts requests.
    pub fn has_servers(&self) -> bool {
        !self.servers.is_empty()
    }

    /// Public, synchronous surface invoked by `McpHost::list_tools`.
    pub fn list_tools(&self, server: &str) -> Result<Vec<McpTool>, RunnerError> {
        let runtime = self.runtime();
        let client = self.connect(server, &runtime)?;
        let tools = runtime
            .block_on(async move { client.peer().list_all_tools().await })
            .map_err(|e| RunnerError::Mcp(format!("tools/list on `{server}` failed: {e}")))?;
        Ok(tools.into_iter().map(rmcp_tool_to_dto).collect())
    }

    /// Public, synchronous surface invoked by `McpHost::call_tool`.
    pub fn call_tool(
        &self,
        server: &str,
        tool: &str,
        args: Value,
    ) -> Result<McpResult, RunnerError> {
        let arguments = match args {
            Value::Null => None,
            Value::Object(map) => Some(map),
            other => {
                return Err(RunnerError::InvalidArgument(format!(
                    "MCP tool arguments must be a JSON object or null, got {other}",
                )))
            }
        };
        let runtime = self.runtime();
        let client = self.connect(server, &runtime)?;
        let tool_name = tool.to_string();
        let server_label = server.to_string();
        let result = runtime
            .block_on(async move {
                let arguments: Option<JsonObject> = arguments.map(|m| m.into_iter().collect::<Map<_, _>>());
                client
                    .peer()
                    .call_tool(CallToolRequestParams {
                        name: tool_name.into(),
                        arguments,
                        meta: None,
                        task: None,
                    })
                    .await
            })
            .map_err(|e| RunnerError::Mcp(format!("tools/call `{tool}` on `{server}` failed: {e}")))?;

        Ok(result_to_dto(server_label, tool, result))
    }

    /// Looks the server up, lazily (re)spawning the underlying child as
    /// needed. If the previous client dropped because the child exited, a
    /// fresh connection is attempted within the bounded-retry budget.
    fn connect(
        &self,
        server: &str,
        runtime: &Runtime,
    ) -> Result<Arc<RunningService<RoleClient, ()>>, RunnerError> {
        let key = server.to_ascii_lowercase();
        let slot = self
            .servers
            .get(&key)
            .ok_or_else(|| RunnerError::NotFound(format!("MCP server `{server}` not registered")))?;

        // Take the lock while we (a) check for an existing live client and
        // (b) potentially spawn a new one. This serializes connect attempts
        // per server but lets concurrent calls share the resulting client.
        let mut guard = slot.lock().map_err(|_| {
            RunnerError::Mcp(format!("MCP server `{server}` connection mutex poisoned"))
        })?;

        if let Some(client) = guard.client.as_ref() {
            // Detect a transport that has dropped without us noticing —
            // peer().is_transport_closed() reports the rmcp-side flag.
            if !client.peer().is_transport_closed() {
                return Ok(Arc::clone(client));
            }
            // Stale client: drop it before retrying.
            guard.client = None;
        }

        if guard.retries_exhausted() {
            return Err(RunnerError::Mcp(format!(
                "MCP server `{server}` exceeded {MAX_RETRIES} restart attempts within {:?}; \
                 cooling off before another spawn",
                RETRY_WINDOW
            )));
        }

        let recipe = guard.recipe.clone();
        match runtime.block_on(spawn_client(server, recipe)) {
            Ok(client) => {
                let arc = Arc::new(client);
                guard.client = Some(Arc::clone(&arc));
                Ok(arc)
            }
            Err(error) => {
                guard.record_failure();
                Err(error)
            }
        }
    }

    fn runtime(&self) -> Arc<Runtime> {
        Arc::clone(self.runtime.get_or_init(|| {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("puffer-mcp")
                .worker_threads(2)
                .build()
                .expect("build puffer-mcp tokio runtime");
            Arc::new(rt)
        }))
    }
}

impl Drop for McpConnectionManager {
    fn drop(&mut self) {
        // Send a best-effort cancel to every live rmcp client. The clients
        // own a `kill_on_drop` child handle, so the process exits as soon as
        // their `Arc` count reaches zero. Move the runtime onto a detached
        // thread so the synchronous `Drop` returns promptly even if the
        // children take a moment to exit.
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        let mut clients: Vec<Arc<RunningService<RoleClient, ()>>> = Vec::new();
        for (_id, slot) in self.servers.drain() {
            if let Ok(mut slot) = slot.into_inner() {
                if let Some(client) = slot.client.take() {
                    clients.push(client);
                }
            }
        }
        std::thread::spawn(move || {
            runtime.block_on(async move {
                let timeout = Duration::from_secs(2);
                for client in clients {
                    // `RunningService::cancel` is the rmcp-recommended way
                    // to stop the loop; if the underlying Arc still has
                    // outstanding refs we can't cancel — fall back to drop.
                    if let Some(svc) = Arc::into_inner(client) {
                        let _ = tokio::time::timeout(timeout, svc.cancel()).await;
                    }
                }
            });
        });
    }
}

/// Spawns the configured stdio command, hands its pipes to rmcp, and waits
/// for the initialize handshake to complete.
async fn spawn_client(
    server: &str,
    recipe: TransportRecipe,
) -> Result<RunningService<RoleClient, ()>, RunnerError> {
    match recipe {
        TransportRecipe::Stdio(spec) => spawn_stdio_client(server, spec).await,
    }
}

async fn spawn_stdio_client(
    server: &str,
    spec: StdioTransportSpec,
) -> Result<RunningService<RoleClient, ()>, RunnerError> {
    let transport: TokioChildProcess = spawn_stdio_child(server, &spec)
        .map_err(|e| RunnerError::Mcp(format!("spawn `{}`: {e}", spec.program)))?;
    ().serve(transport)
        .await
        .map_err(|e| RunnerError::Mcp(format!("MCP handshake with `{server}` failed: {e}")))
}

fn rmcp_tool_to_dto(tool: rmcp::model::Tool) -> McpTool {
    let input_schema = match (*tool.input_schema).clone() {
        m if m.is_empty() => None,
        m => Some(Value::Object(m.into_iter().collect())),
    };
    McpTool {
        name: tool.name.into_owned(),
        description: tool.description.map(|c| c.into_owned()),
        input_schema,
    }
}

fn result_to_dto(server: String, tool: &str, result: rmcp::model::CallToolResult) -> McpResult {
    let mut stdout_parts: Vec<String> = Vec::new();
    let mut metadata_parts: Vec<Value> = Vec::new();
    for content in &result.content {
        match &content.raw {
            RawContent::Text(text) => stdout_parts.push(text.text.clone()),
            other => {
                metadata_parts.push(serde_json::to_value(other).unwrap_or(Value::Null));
            }
        }
    }
    let mut metadata = Map::new();
    if !metadata_parts.is_empty() {
        metadata.insert("non_text_content".into(), Value::Array(metadata_parts));
    }
    if let Some(structured) = result.structured_content {
        metadata.insert("structured_content".into(), structured);
    }
    let is_error = result.is_error.unwrap_or(false);
    McpResult {
        server,
        tool: tool.to_string(),
        success: !is_error,
        stdout: stdout_parts.join("\n"),
        stderr: String::new(),
        metadata: if metadata.is_empty() {
            Value::Null
        } else {
            Value::Object(metadata)
        },
    }
}

/// Helper used by `McpHost` to translate an `McpServerSpec` into a
/// connection-manager entry. Returns `None` for the built-in filesystem
/// stub or any malformed spec — the caller falls back to the existing
/// in-process behavior in that case.
///
/// `target` parsing follows the documented convention: split on whitespace
/// (shell-words style), the first token is the binary, the rest are argv.
/// Manifests that need richer argv handling can pre-quote with `'...'` or
/// `"..."` per shell-words rules.
pub fn entry_from_spec(spec: &puffer_resources::McpServerSpec) -> Option<ConnectionEntry> {
    if super::host::is_live_filesystem_server(&spec.id, &spec.target) {
        return None;
    }
    let target = spec.target.trim();
    if target.is_empty() {
        return None;
    }
    let tokens = match shell_words::split(target) {
        Ok(tokens) => tokens,
        Err(_) => return None,
    };
    let mut iter = tokens.into_iter();
    let program = iter.next()?;
    let args: Vec<String> = iter.collect();
    let recipe = TransportRecipe::Stdio(StdioTransportSpec {
        program,
        args,
        env: BTreeMap::new(),
        cwd: None,
    });
    Some(ConnectionEntry::new(spec.id.clone(), recipe))
}
