//! Long-lived MCP client multiplexer.
//!
//! Conceptually mirrors `references/codex/codex-rs/codex-mcp/src/connection_manager.rs`,
//! cut down to the pass-1.5b scope:
//!
//! * Stdio transport only.
//! * `tools/list`, `tools/call`, `resources/list`, `resources/read`,
//!   `prompts/list`, `prompts/get`. The built-in `filesystem` server keeps
//!   its in-process walker via [`McpHost`](super::host::McpHost); every
//!   other server routes through this manager.
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

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use puffer_runner_api::{
    ChunkSink, DeclineAllElicitations, ElicitationHandler, ElicitationMode, ElicitationRequest,
    ElicitationResponse, McpPrompt, McpPromptArgument, McpPromptContent, McpPromptMessage,
    McpResourceContent, McpResourceContentPart, McpResourceRecord, McpResult, McpTool, RunnerError,
};
use rmcp::handler::client::ClientHandler;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientRequest, CreateElicitationRequestParams,
    CreateElicitationResult, ElicitationAction, ErrorData as McpError, GetPromptRequestParams,
    JsonObject, NumberOrString, ProgressNotificationParam, ProgressToken, PromptMessage,
    PromptMessageContent, PromptMessageRole, RawContent, ReadResourceRequestParams,
    ResourceContents, ServerResult,
};
use rmcp::service::{
    NotificationContext, PeerRequestOptions, RequestContext, RoleClient, RunningService, ServiceExt,
};
use rmcp::transport::child_process::TokioChildProcess;
use serde_json::{Map, Value};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedSender;

use super::http_launcher::{
    build_oauth_streamable_http_transport, build_streamable_http_transport,
};
use super::launcher::spawn_stdio_child;
use super::transport::{
    expand_env, HttpOAuthSpec, HttpTransportSpec, StdioTransportSpec, TransportRecipe,
};
use puffer_mcp_oauth::{default_token_dir, OAuthConfig, OAuthError, OAuthService};
use std::path::PathBuf;

/// Bounded-retry policy: at most `MAX_RETRIES` failed launches inside the
/// rolling `RETRY_WINDOW`. After that, calls fail fast until the window
/// elapses (the next attempt then resets the counter).
const MAX_RETRIES: u32 = 3;
const RETRY_WINDOW: Duration = Duration::from_secs(60);
const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

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

/// In-flight `tools/call` invocations indexed by their `progressToken` so
/// the global [`McpClientHandler`] handler can route incoming
/// `notifications/progress` events back to the right call's sink.
type ProgressRegistry = Arc<Mutex<HashMap<String, UnboundedSender<ProgressNotificationParam>>>>;

/// Server identifier captured at handshake-time so client-handler callbacks
/// (which only see the rmcp `Peer`) can attribute incoming server requests
/// back to the originating MCP server.
type ServerLabel = Arc<str>;

/// Custom rmcp client handler that owns a progress-token registry plus an
/// elicitation responder. The connection manager registers an
/// `mpsc::Sender` for each in-flight `tools/call`; this handler delivers
/// matching `notifications/progress` events to the call's sink without
/// coupling rmcp's transport layer to puffer's `ChunkSink` trait, and
/// fields server-initiated `elicitation/create` requests by delegating to
/// the configured [`ElicitationHandler`].
#[derive(Clone, Debug)]
struct McpClientHandler {
    /// MCP server label, set when the handler is built for a specific
    /// connection. Empty for the default constructor (used only when no
    /// server context is available — e.g. in unit tests).
    server: ServerLabel,
    progress: ProgressRegistry,
    elicitation: Arc<dyn ElicitationHandler>,
}

impl Default for McpClientHandler {
    fn default() -> Self {
        Self {
            server: Arc::from(""),
            progress: Arc::new(Mutex::new(HashMap::new())),
            elicitation: Arc::new(DeclineAllElicitations),
        }
    }
}

impl ClientHandler for McpClientHandler {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let key = progress_token_key(&params.progress_token);
        let sender = match self.progress.lock() {
            Ok(map) => map.get(&key).cloned(),
            Err(_) => None,
        };
        if let Some(sender) = sender {
            let _ = sender.send(params);
        }
    }

    async fn create_elicitation(
        &self,
        request: CreateElicitationRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateElicitationResult, McpError> {
        let dto = elicitation_request_to_dto(&self.server, request);
        let handler = Arc::clone(&self.elicitation);
        // The handler is sync and may block (UI prompt, channel wait, etc.)
        // — run it on the blocking pool so we don't stall the rmcp tokio
        // worker for the duration of the user's response.
        let response = tokio::task::spawn_blocking(move || handler.elicit(dto))
            .await
            .map_err(|e| {
                McpError::internal_error(format!("elicitation handler join: {e}"), None)
            })?;
        Ok(elicitation_response_to_rmcp(response))
    }
}

fn elicitation_request_to_dto(
    server: &str,
    params: CreateElicitationRequestParams,
) -> ElicitationRequest {
    match params {
        CreateElicitationRequestParams::FormElicitationParams {
            message,
            requested_schema,
            ..
        } => ElicitationRequest {
            server: server.to_string(),
            tool: String::new(),
            message,
            mode: ElicitationMode::Form,
            schema: serde_json::to_value(&requested_schema).unwrap_or(Value::Null),
            url: None,
            elicitation_id: None,
        },
        CreateElicitationRequestParams::UrlElicitationParams {
            message,
            url,
            elicitation_id,
            ..
        } => ElicitationRequest {
            server: server.to_string(),
            tool: String::new(),
            message,
            mode: ElicitationMode::Url,
            schema: Value::Null,
            url: Some(url),
            elicitation_id: Some(elicitation_id),
        },
    }
}

fn elicitation_response_to_rmcp(response: ElicitationResponse) -> CreateElicitationResult {
    match response {
        ElicitationResponse::Accept { content } => CreateElicitationResult {
            action: ElicitationAction::Accept,
            content: Some(content),
        },
        ElicitationResponse::Decline => CreateElicitationResult {
            action: ElicitationAction::Decline,
            content: None,
        },
        ElicitationResponse::Cancel => CreateElicitationResult {
            action: ElicitationAction::Cancel,
            content: None,
        },
    }
}

fn progress_token_key(token: &ProgressToken) -> String {
    match &token.0 {
        NumberOrString::String(s) => format!("s:{s}"),
        NumberOrString::Number(n) => format!("n:{n}"),
    }
}

/// Per-server connection state. Lives behind a `Mutex` inside the manager.
struct ServerSlot {
    recipe: TransportRecipe,
    /// Currently live rmcp client, if any. Stored as a fresh handle each
    /// time we (re)connect; `Drop` of the client triggers child shutdown.
    client: Option<Arc<RunningService<RoleClient, McpClientHandler>>>,
    /// Recent launch attempts, used for the bounded-retry budget.
    failure_history: Vec<Instant>,
    /// Shared progress registry, cloned into each [`McpClientHandler`] handler
    /// so the connection manager can route progress notifications back to
    /// in-flight `tools/call` invocations on this server.
    progress: ProgressRegistry,
    /// Per-server async mutex serializing OAuth refresh attempts so two
    /// concurrent calls observing an expired token don't both hit the
    /// refresh endpoint. Mirrors codex's per-server `OAuthPersistor` lock
    /// (see `references/codex/.../oauth.rs::refresh_if_needed`).
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
}

impl ServerSlot {
    fn new(recipe: TransportRecipe) -> Self {
        Self {
            recipe,
            client: None,
            failure_history: Vec::new(),
            progress: Arc::new(Mutex::new(HashMap::new())),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn record_failure(&mut self) {
        let now = Instant::now();
        self.failure_history
            .retain(|t| now.duration_since(*t) <= RETRY_WINDOW);
        self.failure_history.push(now);
    }

    fn retries_exhausted(&mut self) -> bool {
        let now = Instant::now();
        self.failure_history
            .retain(|t| now.duration_since(*t) <= RETRY_WINDOW);
        self.failure_history.len() >= MAX_RETRIES as usize
    }
}

/// Multiplexes MCP server connections behind a synchronous façade.
pub struct McpConnectionManager {
    /// Configured servers. Cloned once at construction; subsequent edits go
    /// through `with_servers` (used only by tests).
    servers: HashMap<String, Mutex<ServerSlot>>,
    /// Strategy for fielding server-initiated `elicitation/create` requests
    /// (shared across all configured MCP servers). Defaults to
    /// [`DeclineAllElicitations`]; the runner sets a real handler via
    /// [`McpConnectionManager::with_elicitation_handler`].
    elicitation: Arc<dyn ElicitationHandler>,
    /// Optional override for where OAuth token files live. When `None`
    /// the default ([`puffer_mcp_oauth::default_token_dir`]) is used —
    /// `<user-config>/puffer/mcp-tokens`. Tests pin this to a `TempDir`
    /// so per-test runs don't interfere.
    oauth_token_dir: Option<PathBuf>,
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
            .field("elicitation", &self.elicitation)
            .finish()
    }
}

impl Default for McpConnectionManager {
    fn default() -> Self {
        Self {
            servers: HashMap::new(),
            elicitation: Arc::new(DeclineAllElicitations),
            oauth_token_dir: None,
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
            elicitation: Arc::new(DeclineAllElicitations),
            oauth_token_dir: None,
            runtime: OnceLock::new(),
        }
    }

    /// Override the directory used for persisted OAuth tokens. Defaults
    /// to [`puffer_mcp_oauth::default_token_dir`]; tests use this hook
    /// to pin storage to a `TempDir`.
    pub fn with_oauth_token_dir(mut self, dir: PathBuf) -> Self {
        self.oauth_token_dir = Some(dir);
        self
    }

    /// Replaces the elicitation handler used for every subsequent rmcp
    /// connection. Existing live connections keep the handler that was
    /// active when they were spawned — callers that need to swap mid-flight
    /// should drop the manager and rebuild it.
    pub fn with_elicitation_handler(mut self, handler: Arc<dyn ElicitationHandler>) -> Self {
        self.elicitation = handler;
        self
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
        let mut attempt = 0;
        loop {
            let (client, _progress, timeout) = self.connect(server, &runtime)?;
            let result = runtime.block_on(async {
                let client = Arc::clone(&client);
                tokio::time::timeout(timeout, client.peer().list_all_tools()).await
            });
            match result {
                Ok(Ok(tools)) => return Ok(tools.into_iter().map(rmcp_tool_to_dto).collect()),
                Ok(Err(e)) if attempt == 0 && is_auth_required_service_error(&e) => {
                    self.drop_client_and_refresh_oauth(server, &runtime)?;
                    attempt += 1;
                    continue;
                }
                Ok(Err(e)) => {
                    return Err(RunnerError::Mcp(format!(
                        "tools/list on `{server}` failed: {e}"
                    )));
                }
                Err(_) => {
                    return Err(RunnerError::Mcp(format!(
                        "tools/list on `{server}` timed out after {}s",
                        timeout.as_secs()
                    )));
                }
            }
        }
    }

    /// Public, synchronous surface invoked by `McpHost::call_tool`.
    ///
    /// `sink` receives any `notifications/progress` events the server emits
    /// for this call as JSON via [`ChunkSink::event`]. rmcp itself mints a
    /// fresh `progressToken` for every cancellable request; the connection
    /// manager registers a matching sender on the per-server progress
    /// registry under that token, awaits the response, drains any pending
    /// notifications, and tears the registration down (success or failure).
    pub fn call_tool(
        &self,
        server: &str,
        tool: &str,
        args: Value,
        sink: &mut dyn ChunkSink,
    ) -> Result<McpResult, RunnerError> {
        let arguments = match args {
            Value::Null => None,
            Value::Object(map) => Some(map),
            other => {
                return Err(RunnerError::InvalidArgument(format!(
                    "MCP tool arguments must be a JSON object or null, got {other}",
                )));
            }
        };
        let runtime = self.runtime();
        let mut attempt = 0;
        loop {
            match self.call_tool_once(server, tool, arguments.clone(), sink, &runtime) {
                Ok(result) => return Ok(result),
                Err(RunnerError::Mcp(msg)) if attempt == 0 && msg_indicates_auth_required(&msg) => {
                    self.drop_client_and_refresh_oauth(server, &runtime)?;
                    attempt += 1;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// One attempt at `tools/call`. Caller wraps in a 1-shot retry loop
    /// that detects 401 / `AuthRequired` and triggers an OAuth refresh.
    fn call_tool_once(
        &self,
        server: &str,
        tool: &str,
        arguments: Option<serde_json::Map<String, Value>>,
        sink: &mut dyn ChunkSink,
        runtime: &Runtime,
    ) -> Result<McpResult, RunnerError> {
        let (client, progress_registry, timeout) = self.connect(server, runtime)?;
        let tool_name = tool.to_string();
        let server_label = server.to_string();

        // Run the call inside the manager's tokio runtime. Use rmcp's
        // `send_cancellable_request` so we can read the auto-generated
        // `progressToken` off the returned `RequestHandle` and wire a
        // matching subscriber into the per-server registry before awaiting
        // the response.
        let outcome_with_events = runtime.block_on(async move {
            let arguments: Option<JsonObject> =
                arguments.map(|m| m.into_iter().collect::<Map<_, _>>());
            let request = CallToolRequest {
                method: Default::default(),
                params: CallToolRequestParams {
                    name: tool_name.into(),
                    arguments,
                    meta: None,
                    task: None,
                },
                extensions: Default::default(),
            };
            let handle = match client
                .peer()
                .send_cancellable_request(
                    ClientRequest::CallToolRequest(request),
                    PeerRequestOptions {
                        timeout: Some(timeout),
                        meta: None,
                    },
                )
                .await
            {
                Ok(handle) => handle,
                Err(e) => return Err(e),
            };
            let registry_key = progress_token_key(&handle.progress_token);
            let (progress_tx, mut progress_rx) =
                tokio::sync::mpsc::unbounded_channel::<ProgressNotificationParam>();
            if let Ok(mut map) = progress_registry.lock() {
                map.insert(registry_key.clone(), progress_tx);
            }
            let response = handle.await_response().await;
            if let Ok(mut map) = progress_registry.lock() {
                map.remove(&registry_key);
            }
            let mut events: Vec<ProgressNotificationParam> = Vec::new();
            while let Ok(evt) = progress_rx.try_recv() {
                events.push(evt);
            }
            Ok((response, events))
        });

        let (response, events) =
            outcome_with_events.map_err(|e: rmcp::service::ServiceError| {
                RunnerError::Mcp(format!("tools/call `{tool}` on `{server}` failed: {e}"))
            })?;
        for evt in events {
            sink.event(progress_event_to_json(&evt));
        }
        let response = response.map_err(|e| {
            RunnerError::Mcp(format!("tools/call `{tool}` on `{server}` failed: {e}"))
        })?;
        let result = match response {
            ServerResult::CallToolResult(r) => r,
            other => {
                return Err(RunnerError::Mcp(format!(
                    "tools/call `{tool}` on `{server}` returned unexpected response: {other:?}"
                )));
            }
        };

        Ok(result_to_dto(server_label, tool, result))
    }

    /// Public, synchronous surface invoked by `McpHost::list_resources`.
    pub fn list_resources(&self, server: &str) -> Result<Vec<McpResourceRecord>, RunnerError> {
        let runtime = self.runtime();
        let mut attempt = 0;
        loop {
            let (client, _progress, _timeout) = self.connect(server, &runtime)?;
            let result = runtime.block_on(async move { client.peer().list_all_resources().await });
            match result {
                Ok(resources) => {
                    return Ok(resources
                        .into_iter()
                        .map(|r| rmcp_resource_to_dto(server, r))
                        .collect());
                }
                Err(e) if attempt == 0 && is_auth_required_service_error(&e) => {
                    self.drop_client_and_refresh_oauth(server, &runtime)?;
                    attempt += 1;
                    continue;
                }
                Err(e) => {
                    return Err(RunnerError::Mcp(format!(
                        "resources/list on `{server}` failed: {e}"
                    )));
                }
            }
        }
    }

    /// Public, synchronous surface invoked by `McpHost::read_resource`.
    pub fn read_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> Result<McpResourceContent, RunnerError> {
        let runtime = self.runtime();
        let mut attempt = 0;
        loop {
            let (client, _progress, _timeout) = self.connect(server, &runtime)?;
            let uri_owned = uri.to_string();
            let result = runtime.block_on(async move {
                client
                    .peer()
                    .read_resource(ReadResourceRequestParams {
                        uri: uri_owned,
                        meta: None,
                    })
                    .await
            });
            match result {
                Ok(r) => return Ok(read_resource_result_to_dto(server, uri, r)),
                Err(e) if attempt == 0 && is_auth_required_service_error(&e) => {
                    self.drop_client_and_refresh_oauth(server, &runtime)?;
                    attempt += 1;
                    continue;
                }
                Err(e) => {
                    return Err(RunnerError::Mcp(format!(
                        "resources/read `{uri}` on `{server}` failed: {e}"
                    )));
                }
            }
        }
    }

    /// Public, synchronous surface invoked by `McpHost::list_prompts`.
    pub fn list_prompts(&self, server: &str) -> Result<Vec<McpPrompt>, RunnerError> {
        let runtime = self.runtime();
        let mut attempt = 0;
        loop {
            let (client, _progress, _timeout) = self.connect(server, &runtime)?;
            let result = runtime.block_on(async move { client.peer().list_all_prompts().await });
            match result {
                Ok(prompts) => return Ok(prompts.into_iter().map(rmcp_prompt_to_dto).collect()),
                Err(e) if attempt == 0 && is_auth_required_service_error(&e) => {
                    self.drop_client_and_refresh_oauth(server, &runtime)?;
                    attempt += 1;
                    continue;
                }
                Err(e) => {
                    return Err(RunnerError::Mcp(format!(
                        "prompts/list on `{server}` failed: {e}"
                    )));
                }
            }
        }
    }

    /// Public, synchronous surface invoked by `McpHost::get_prompt`.
    pub fn get_prompt(
        &self,
        server: &str,
        name: &str,
        args: Value,
    ) -> Result<McpPromptContent, RunnerError> {
        let arguments = match args {
            Value::Null => None,
            Value::Object(map) => Some(map.into_iter().collect::<JsonObject>()),
            other => {
                return Err(RunnerError::InvalidArgument(format!(
                    "MCP prompt arguments must be a JSON object or null, got {other}",
                )));
            }
        };
        let runtime = self.runtime();
        let mut attempt = 0;
        loop {
            let (client, _progress, _timeout) = self.connect(server, &runtime)?;
            let name_owned = name.to_string();
            let arguments = arguments.clone();
            let result = runtime.block_on(async move {
                client
                    .peer()
                    .get_prompt(GetPromptRequestParams {
                        name: name_owned,
                        arguments,
                        meta: None,
                    })
                    .await
            });
            match result {
                Ok(r) => return Ok(get_prompt_result_to_dto(server, name, r)),
                Err(e) if attempt == 0 && is_auth_required_service_error(&e) => {
                    self.drop_client_and_refresh_oauth(server, &runtime)?;
                    attempt += 1;
                    continue;
                }
                Err(e) => {
                    return Err(RunnerError::Mcp(format!(
                        "prompts/get `{name}` on `{server}` failed: {e}"
                    )));
                }
            }
        }
    }

    /// Looks the server up, lazily (re)spawning the underlying child as
    /// needed. If the previous client dropped because the child exited, a
    /// fresh connection is attempted within the bounded-retry budget.
    /// Returns the live rmcp client plus the per-server progress registry.
    fn connect(
        &self,
        server: &str,
        runtime: &Runtime,
    ) -> Result<
        (
            Arc<RunningService<RoleClient, McpClientHandler>>,
            ProgressRegistry,
            Duration,
        ),
        RunnerError,
    > {
        let key = server.to_ascii_lowercase();
        let slot = self.servers.get(&key).ok_or_else(|| {
            RunnerError::NotFound(format!("MCP server `{server}` not registered"))
        })?;

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
                return Ok((
                    Arc::clone(client),
                    Arc::clone(&guard.progress),
                    guard.recipe.timeout(),
                ));
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
        let handler = McpClientHandler {
            server: Arc::from(server.to_string().as_str()),
            progress: Arc::clone(&guard.progress),
            elicitation: Arc::clone(&self.elicitation),
        };
        let token_dir = self
            .oauth_token_dir
            .clone()
            .unwrap_or_else(default_token_dir);
        match runtime.block_on(spawn_client(server, recipe, handler, token_dir)) {
            Ok(client) => {
                let arc = Arc::new(client);
                guard.client = Some(Arc::clone(&arc));
                Ok((arc, Arc::clone(&guard.progress), guard.recipe.timeout()))
            }
            Err(error) => {
                guard.record_failure();
                Err(error)
            }
        }
    }

    /// Drop the cached client for `server` (so the next `connect` re-builds
    /// the transport from scratch) and force the OAuth service to refresh
    /// tokens. Returns the per-server refresh lock so callers can serialize
    /// concurrent attempts.
    ///
    /// This is the puffer mirror of codex's `refresh_oauth_if_needed` —
    /// triggered reactively when an MCP request returns 401 (rmcp surfaces
    /// `StreamableHttpError::AuthRequired`) so the user doesn't have to
    /// re-run `puffer mcp login` for every transient token expiry.
    fn drop_client_and_refresh_oauth(
        &self,
        server: &str,
        runtime: &Runtime,
    ) -> Result<(), RunnerError> {
        let key = server.to_ascii_lowercase();
        let slot = self.servers.get(&key).ok_or_else(|| {
            RunnerError::NotFound(format!("MCP server `{server}` not registered"))
        })?;
        let (recipe, refresh_lock) = {
            let mut guard = slot.lock().map_err(|_| {
                RunnerError::Mcp(format!("MCP server `{server}` connection mutex poisoned"))
            })?;
            // Drop the live client so the next connect re-uses fresh tokens.
            guard.client = None;
            (guard.recipe.clone(), Arc::clone(&guard.refresh_lock))
        };
        // Only HTTP+OAuth recipes have anything to refresh.
        let TransportRecipe::Http(http) = recipe else {
            return Ok(());
        };
        let Some(oauth_spec) = http.oauth.clone() else {
            return Ok(());
        };
        let token_dir = self
            .oauth_token_dir
            .clone()
            .unwrap_or_else(default_token_dir);
        let service = build_oauth_service(server, &http.url, &oauth_spec, token_dir);
        let server_label = server.to_string();
        runtime.block_on(async move {
            // Serialize per-server so concurrent 401s from different
            // in-flight requests collapse to a single token refresh.
            let _g = refresh_lock.lock().await;
            // force_refresh ignores the local clock-based skew check —
            // we got here because the *server* rejected the token, so
            // even a not-yet-expired access token needs to be re-minted.
            // If the refresh attempt fails (or there are no stored
            // tokens / no refresh token), we propagate
            // RunnerError::OAuthRequired so the caller can prompt the
            // user to re-run `puffer mcp login`.
            match service.force_refresh().await {
                Ok(()) => Ok(()),
                Err(e) => Err(oauth_error_to_runner(&server_label, e)),
            }
        })
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
        let mut clients: Vec<Arc<RunningService<RoleClient, McpClientHandler>>> = Vec::new();
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
    handler: McpClientHandler,
    oauth_token_dir: PathBuf,
) -> Result<RunningService<RoleClient, McpClientHandler>, RunnerError> {
    match recipe {
        TransportRecipe::Stdio(spec) => spawn_stdio_client(server, spec, handler).await,
        TransportRecipe::Http(spec) => {
            spawn_http_client(server, spec, handler, oauth_token_dir).await
        }
    }
}

async fn spawn_stdio_client(
    server: &str,
    spec: StdioTransportSpec,
    handler: McpClientHandler,
) -> Result<RunningService<RoleClient, McpClientHandler>, RunnerError> {
    let connect_timeout = spec.connect_timeout;
    let transport: TokioChildProcess = spawn_stdio_child(server, &spec)
        .map_err(|e| RunnerError::Mcp(format!("spawn `{}`: {e}", spec.program)))?;
    match tokio::time::timeout(connect_timeout, handler.serve(transport)).await {
        Ok(result) => result
            .map_err(|e| RunnerError::Mcp(format!("MCP handshake with `{server}` failed: {e}"))),
        Err(_) => Err(RunnerError::Mcp(format!(
            "MCP handshake with `{server}` timed out after {}s",
            connect_timeout.as_secs()
        ))),
    }
}

/// Build the rmcp streamable-HTTP transport and run the initialize
/// handshake. The bounded-retry budget in [`ServerSlot`] applies just like
/// the stdio path: if the handshake fails we return an `Mcp` error and
/// the caller records a failure attempt. Mid-session reconnect is handled
/// inside rmcp's own SSE auto-reconnect loop until the transport is
/// declared closed by `peer().is_transport_closed()`, at which point the
/// connection manager's lazy-respawn kicks in for the *next* request.
async fn spawn_http_client(
    server: &str,
    spec: HttpTransportSpec,
    handler: McpClientHandler,
    oauth_token_dir: PathBuf,
) -> Result<RunningService<RoleClient, McpClientHandler>, RunnerError> {
    let connect_timeout = spec.connect_timeout;
    if let Some(oauth_spec) = spec.oauth.clone() {
        let oauth_service = build_oauth_service(server, &spec.url, &oauth_spec, oauth_token_dir);
        let resolved = match tokio::time::timeout(connect_timeout, oauth_service.resolve()).await {
            Ok(Ok(resolved)) => resolved,
            Ok(Err(error)) => return Err(oauth_error_to_runner(server, error)),
            Err(_) => {
                return Err(RunnerError::Mcp(format!(
                    "MCP OAuth setup for `{server}` timed out after {}s",
                    connect_timeout.as_secs()
                )));
            }
        };
        let transport = build_oauth_streamable_http_transport(&spec, resolved.client.clone());
        return match tokio::time::timeout(connect_timeout, handler.serve(transport)).await {
            Ok(result) => result.map_err(|e| {
                RunnerError::Mcp(format!("MCP handshake with `{server}` failed: {e}"))
            }),
            Err(_) => Err(RunnerError::Mcp(format!(
                "MCP handshake with `{server}` timed out after {}s",
                connect_timeout.as_secs()
            ))),
        };
    }
    let transport = build_streamable_http_transport(server, &spec)?;
    match tokio::time::timeout(connect_timeout, handler.serve(transport)).await {
        Ok(result) => result
            .map_err(|e| RunnerError::Mcp(format!("MCP handshake with `{server}` failed: {e}"))),
        Err(_) => Err(RunnerError::Mcp(format!(
            "MCP handshake with `{server}` timed out after {}s",
            connect_timeout.as_secs()
        ))),
    }
}

fn build_oauth_service(
    server: &str,
    url: &str,
    spec: &HttpOAuthSpec,
    token_dir: PathBuf,
) -> OAuthService {
    OAuthService::new(OAuthConfig {
        server_id: server.to_string(),
        server_url: url.to_string(),
        scopes: spec.scopes.clone(),
        client_name: if spec.client_name.is_empty() {
            "puffer".to_string()
        } else {
            spec.client_name.clone()
        },
        token_dir,
    })
}

/// True when an rmcp `ServiceError` was caused by a 401 / `WWW-Authenticate`
/// response from an HTTP MCP server.
///
/// rmcp's `StreamableHttpError::AuthRequired` variant displays as
/// "Auth required" via its `#[error]` derive. The error gets wrapped in
/// `DynamicTransportError` (which displays as
/// `Transport [name] error: <inner>`) and then in
/// `ServiceError::TransportSend` (which displays as
/// `Transport send error: <inner>`). We check the resulting display
/// chain for "Auth required" to decide whether to retry with a refreshed
/// token.
///
/// Codex does this with a typed `error.downcast_ref::<StreamableHttpError<...>>`
/// because they use a custom `StreamableHttpClientAdapter` whose `Error`
/// type they own. We use rmcp's stock reqwest transport whose
/// `<StreamableHttpClient>::Error = reqwest::Error`, making the downcast
/// dance more involved — string match keeps the change minimal and tracks
/// rmcp's `#[error("Auth required")]` literal.
fn is_auth_required_service_error(err: &rmcp::service::ServiceError) -> bool {
    msg_indicates_auth_required(&err.to_string())
}

fn msg_indicates_auth_required(msg: &str) -> bool {
    msg.contains("Auth required")
}

fn oauth_error_to_runner(server: &str, err: OAuthError) -> RunnerError {
    match err {
        OAuthError::OAuthRequired {
            server_id,
            authorization_url,
        } => RunnerError::OAuthRequired {
            server_id,
            authorization_url,
        },
        other => RunnerError::Mcp(format!(
            "OAuth setup for MCP server `{server}` failed: {other}"
        )),
    }
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

fn progress_event_to_json(params: &ProgressNotificationParam) -> Value {
    let token = match &params.progress_token.0 {
        NumberOrString::String(s) => Value::String(s.to_string()),
        NumberOrString::Number(n) => Value::Number((*n).into()),
    };
    let mut obj = Map::new();
    obj.insert("kind".into(), Value::String("mcp/progress".into()));
    obj.insert("progressToken".into(), token);
    obj.insert(
        "progress".into(),
        serde_json::Number::from_f64(params.progress)
            .map(Value::Number)
            .unwrap_or(Value::Null),
    );
    if let Some(total) = params.total {
        if let Some(n) = serde_json::Number::from_f64(total) {
            obj.insert("total".into(), Value::Number(n));
        }
    }
    if let Some(message) = params.message.as_ref() {
        obj.insert("message".into(), Value::String(message.clone()));
    }
    Value::Object(obj)
}

fn rmcp_resource_to_dto(server: &str, resource: rmcp::model::Resource) -> McpResourceRecord {
    let raw = resource.raw;
    McpResourceRecord {
        server: server.to_string(),
        uri: raw.uri,
        name: raw.name,
        mime_type: raw.mime_type,
        description: raw.description,
    }
}

fn read_resource_result_to_dto(
    server: &str,
    uri: &str,
    result: rmcp::model::ReadResourceResult,
) -> McpResourceContent {
    let parts = result
        .contents
        .into_iter()
        .map(|content| match content {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                ..
            } => McpResourceContentPart::Text {
                uri,
                mime_type,
                text,
            },
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => McpResourceContentPart::Blob {
                uri,
                mime_type,
                bytes: decode_blob_base64(&blob),
            },
        })
        .collect();
    McpResourceContent {
        server: server.to_string(),
        uri: uri.to_string(),
        parts,
    }
}

fn rmcp_prompt_to_dto(prompt: rmcp::model::Prompt) -> McpPrompt {
    McpPrompt {
        name: prompt.name,
        description: prompt.description,
        arguments: prompt
            .arguments
            .unwrap_or_default()
            .into_iter()
            .map(|arg| McpPromptArgument {
                name: arg.name,
                description: arg.description,
                required: arg.required.unwrap_or(false),
            })
            .collect(),
    }
}

fn get_prompt_result_to_dto(
    server: &str,
    name: &str,
    result: rmcp::model::GetPromptResult,
) -> McpPromptContent {
    let messages = result
        .messages
        .into_iter()
        .map(prompt_message_to_dto)
        .collect();
    McpPromptContent {
        server: server.to_string(),
        name: name.to_string(),
        messages,
    }
}

fn prompt_message_to_dto(message: PromptMessage) -> McpPromptMessage {
    let role = match message.role {
        PromptMessageRole::User => "user",
        PromptMessageRole::Assistant => "assistant",
    }
    .to_string();
    let text = match message.content {
        PromptMessageContent::Text { text } => text,
        PromptMessageContent::Image { .. } => "[image content]".to_string(),
        PromptMessageContent::Resource { resource } => match resource.raw.resource {
            ResourceContents::TextResourceContents { text, .. } => text,
            ResourceContents::BlobResourceContents { uri, .. } => {
                format!("[binary resource {uri}]")
            }
        },
        PromptMessageContent::ResourceLink { link } => {
            format!("[resource link {}]", link.raw.uri)
        }
    };
    McpPromptMessage { role, text }
}

/// Best-effort base64 decoder mirroring the inline encoder used by puffer's
/// `McpResourceContentPart::Blob` JSON serializer. rmcp returns blobs as the
/// raw base64 string the server sent, so we decode it back to bytes here.
fn decode_blob_base64(input: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let trimmed = input.trim().trim_end_matches('=');
    let mut out: Vec<u8> = Vec::with_capacity(trimmed.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits: u8 = 0;
    for ch in trimmed.bytes() {
        let Some(value) = ALPHABET.iter().position(|c| *c == ch) else {
            // Skip stray whitespace / newlines that some encoders emit.
            if ch.is_ascii_whitespace() {
                continue;
            }
            return Vec::new();
        };
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8 & 0xff);
        }
    }
    out
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
/// Transport dispatch:
///
/// * `transport: stdio` (default / blank) — `target` is shell-words split
///   into a binary + argv. Manifests that need richer argv handling can
///   pre-quote with `'...'` or `"..."` per shell-words rules.
/// * `transport: http` / `streamable-http` — the URL comes from `endpoint`
///   when set (legacy field used by `puffer mcp add` for SSE/HTTP) or
///   `target` otherwise. Header values are env-expanded so `${VAR}`
///   placeholders resolve at construction time.
pub fn entry_from_spec(spec: &puffer_resources::McpServerSpec) -> Option<ConnectionEntry> {
    if super::host::is_live_filesystem_server(&spec.id, &spec.target) {
        return None;
    }
    let transport = spec.transport.trim().to_ascii_lowercase();
    match transport.as_str() {
        "" | "stdio" => stdio_entry_from_spec(spec),
        "http" | "streamable-http" | "streamable_http" => http_entry_from_spec(spec),
        other => {
            tracing::warn!(
                target = "puffer::mcp",
                "MCP server `{id}`: unknown transport `{other}`; ignoring",
                id = spec.id
            );
            None
        }
    }
}

fn stdio_entry_from_spec(spec: &puffer_resources::McpServerSpec) -> Option<ConnectionEntry> {
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
        env: spec.env.clone(),
        inherit_env: spec.inherit_env,
        timeout: duration_from_seconds(spec.timeout, DEFAULT_TOOL_TIMEOUT),
        connect_timeout: duration_from_seconds(spec.connect_timeout, DEFAULT_CONNECT_TIMEOUT),
        cwd: None,
    });
    Some(ConnectionEntry::new(spec.id.clone(), recipe))
}

fn http_entry_from_spec(spec: &puffer_resources::McpServerSpec) -> Option<ConnectionEntry> {
    // `endpoint` is the historical field set by `puffer mcp add` for
    // HTTP/SSE entries; fall back to `target` so manifests that follow
    // the pass-1.5d convention (URL in `target`, `headers` map alongside)
    // also parse cleanly.
    let url_raw = if !spec.endpoint.trim().is_empty() {
        spec.endpoint.trim()
    } else {
        spec.target.trim()
    };
    if url_raw.is_empty() {
        return None;
    }
    let url = expand_env(url_raw);
    if url.is_empty() {
        return None;
    }
    let headers: Vec<(String, String)> = spec
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), expand_env(v)))
        .collect();
    let oauth = spec
        .oauth
        .as_ref()
        .filter(|o| o.enabled())
        .map(|o| HttpOAuthSpec {
            scopes: o.scopes(),
            client_name: o
                .client_name()
                .unwrap_or_else(|| format!("puffer-{}", spec.id)),
        });
    let recipe = TransportRecipe::Http(HttpTransportSpec {
        url,
        headers,
        oauth,
        timeout: duration_from_seconds(spec.timeout, DEFAULT_TOOL_TIMEOUT),
        connect_timeout: duration_from_seconds(spec.connect_timeout, DEFAULT_CONNECT_TIMEOUT),
    });
    Some(ConnectionEntry::new(spec.id.clone(), recipe))
}

fn duration_from_seconds(value: Option<u64>, default: Duration) -> Duration {
    match value {
        Some(0) | None => default,
        Some(seconds) => Duration::from_secs(seconds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_required_detection_matches_rmcp_display() {
        // rmcp's StreamableHttpError::AuthRequired displays as
        // "Auth required" via its `#[error]` derive. The full error
        // chain we observe in `ServiceError::TransportSend` looks like:
        //   Transport send error: Transport [streamable-http] error: Auth required
        assert!(msg_indicates_auth_required("Auth required"));
        assert!(msg_indicates_auth_required(
            "Transport send error: Transport [streamable-http] error: Auth required"
        ));
        assert!(!msg_indicates_auth_required("Some other error"));
    }
}
