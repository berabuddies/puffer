//! Puffer daemon: WebSocket/NDJSON server exposing the runtime to
//! desktop / web / remote clients.
//!
//! Wire protocol (one JSON object per WebSocket text frame):
//!
//!   server → client on connect:
//!     { "event": "hello", "payload": { "protocolVersion": "1", ... } }
//!
//!   client → server:
//!     { "id": "<uuid>", "method": "<name>", "params": { ... } }
//!
//!   server → client, correlated by id:
//!     { "id": "<uuid>", "result": <json> }
//!     { "id": "<uuid>", "error": { "code": "...", "message": "..." } }
//!
//!   server → client, fire-and-forget (streaming):
//!     { "event": "<channel>", "payload": <json> }
//!
//! Auth: `?token=<secret>` on the upgrade request. Token lives at
//! `<user-config>/daemon.token` (0600) — generated on first run.

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use indexmap::IndexMap;
use puffer_config::{
    ensure_workspace_dirs, load_config, save_user_config, ConfigPaths, PufferConfig,
};
use puffer_core::{
    apply_model_preferences, execute_user_turn_streaming_with_permissions_and_cancel,
    with_user_question_prompt_handler, AppState, CancelToken, MessageRole, PermissionPromptAction,
    PermissionPromptRequest, TurnStreamEvent, UserQuestionPromptRequest,
    UserQuestionPromptResponse,
};
use puffer_provider_openai::{
    exchange_authorization_code as exchange_openai_authorization_code,
    parse_authorization_input as parse_openai_authorization_input,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry, StoredCredential};
use puffer_resources::{load_resources, LoadedResources, McpServerSpec};
use puffer_session_store::{SessionStore, TranscriptEvent};
use puffer_transport_anthropic::{
    exchange_authorization_code as exchange_anthropic_authorization_code,
    parse_authorization_input as parse_anthropic_authorization_input, ANTHROPIC_API_BASE_URL,
    ANTHROPIC_MANUAL_REDIRECT_URL,
};
use puffer_workflow::WorkflowStore;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufReader, Read};
use std::net::SocketAddr;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, Mutex as AsyncMutex};
use uuid::Uuid;

use crate::auth_credentials::{
    inferred_anthropic_redirect_uri, set_stored_credential, store_anthropic_credential,
    to_registry_oauth_credential_openai,
};
use crate::auth_provider::{
    oauth_family_for_provider, oauth_login_bundle_for_provider, OauthFamily,
};
use crate::daemon_browser::BrowserRegistry;
use crate::daemon_fs_watch::FsWatchRegistry;
use crate::daemon_pty::PtyRegistry;
use crate::daemon_ui_state::{
    load_file_tabs_state, load_pin_state, set_file_tabs_state, set_pin_state, DesktopFileTab,
    DesktopFileTabsState, DesktopPinState,
};
use crate::desktop_api;
use crate::desktop_api_types::{
    ExternalCredentialDto, FolderGroupDto, McpServerDto, ModelDescriptorDto, RepoActionResultDto,
    RepoStatusDto, SessionDetailDto, SettingsSnapshotDto,
};

const PROTOCOL_VERSION: &str = "1";

/// Handshake record written to the handshake file + stdout so parent
/// processes (Tauri) can discover the URL and token.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Handshake {
    pub(crate) url: String,
    pub(crate) token: String,
    pub(crate) protocol_version: String,
    pub(crate) workspace_root: String,
}

pub(crate) struct DaemonOptions {
    pub bind: String,
    pub handshake_file: Option<String>,
    pub token: Option<String>,
    pub print_handshake: bool,
    pub no_browser: bool,
    pub system_prompt_1: Option<String>,
    pub disable_auto_title: bool,
    pub yolo: bool,
}

pub(crate) fn run(options: DaemonOptions) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    rt.block_on(async move { run_async(options).await })
}

async fn run_async(options: DaemonOptions) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let paths = ConfigPaths::discover(&cwd);
    ensure_workspace_dirs(&paths)?;

    let token = options
        .token
        .unwrap_or_else(|| load_or_generate_token(&paths));

    let state = DaemonState::load(
        cwd.clone(),
        paths.clone(),
        token.clone(),
        options.no_browser,
        options.disable_auto_title,
        options.yolo,
    )?;
    if let Some(prompt) = options
        .system_prompt_1
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        std::env::set_var("PUFFER_SYSTEM_PROMPT_1", prompt);
    }
    if options.no_browser {
        std::env::set_var("PUFFER_NO_BROWSER", "1");
    } else {
        std::env::remove_var("PUFFER_NO_BROWSER");
    }
    let state = Arc::new(state);

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state.clone());

    let addr: SocketAddr = options.bind.parse().context("bind address")?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let url = format!("ws://{bound}/ws");

    let handshake = Handshake {
        url: url.clone(),
        token: token.clone(),
        protocol_version: PROTOCOL_VERSION.to_string(),
        workspace_root: paths.workspace_root.display().to_string(),
    };

    // Handshake file (machine-readable + surviving parent-process restart).
    let handshake_path = options
        .handshake_file
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| paths.user_config_dir.join("daemon.handshake"));
    if let Some(parent) = handshake_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let handshake_json = serde_json::to_string(&handshake)?;
    std::fs::write(&handshake_path, &handshake_json)?;

    if options.print_handshake {
        // One-line NDJSON so Tauri (or shell scripts) can `read` a line,
        // parse, and act. We continue to serve after printing.
        println!("{handshake_json}");
    }

    eprintln!(
        "puffer daemon listening on {url} (workspace: {})",
        paths.workspace_root.display()
    );

    axum::serve(listener, app).await?;
    Ok(())
}

fn load_or_generate_token(paths: &ConfigPaths) -> String {
    let token_path = paths.user_config_dir.join("daemon.token");
    if let Ok(existing) = std::fs::read_to_string(&token_path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    let token = hex(&buf);
    if let Some(parent) = token_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&token_path, &token);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600));
    }
    token
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

// ---------------------------------------------------------------------------
// DaemonState — the shared runtime inputs + in-flight turn registry.
// ---------------------------------------------------------------------------

pub(crate) struct DaemonState {
    cwd: std::path::PathBuf,
    paths: ConfigPaths,
    token: String,
    /// Live workspace + user config merged at daemon start. Wrapped in a
    /// mutex so `update_config` can edit it without tearing down the
    /// daemon — readers clone-under-lock to keep the critical section
    /// short and avoid holding the mutex across I/O.
    config: Mutex<PufferConfig>,
    // Event bus — every client gets a broadcast receiver.
    events: broadcast::Sender<ServerEnvelope>,
    // Turn registry — permission responders + cancel flags keyed by turn_id.
    turns: Arc<Mutex<HashMap<String, TurnHandle>>>,
    next_request_id: Arc<AtomicU64>,
    /// User-facing PTYs (one per Terminal tab). Shared via Arc so the
    /// reader threads can drop their own entries on child exit without
    /// going through the RPC layer.
    pub(crate) ptys: Arc<PtyRegistry>,
    /// Filesystem watches (one per FilesPane subscription). Shared so the
    /// debounce threads and the RPC handlers hit the same registry and
    /// dropping the watch id actually tears down the kernel subscription.
    pub(crate) fs_watches: Arc<FsWatchRegistry>,
    /// Chrome-backed browser sessions used by the desktop Browser tab.
    pub(crate) browsers: Arc<BrowserRegistry>,
    disable_auto_title: bool,
    yolo: bool,
    /// Transcript replay buffer — a bounded ring of recent session / clone /
    /// workspace events. On a fresh WebSocket connection we replay these
    /// so UIs that disconnected mid-turn don't miss deltas. Size capped at
    /// `RECENT_EVENT_CAPACITY` (oldest evicted) to keep memory bounded.
    recent_events: Arc<Mutex<VecDeque<ServerEnvelope>>>,
}

const RECENT_EVENT_CAPACITY: usize = 500;

impl DaemonState {
    /// The daemon's working directory — used as one of the allowed roots
    /// for filesystem RPCs (list_dir / read_file). Also the default cwd
    /// for newly-created sessions.
    pub(crate) fn cwd_path(&self) -> &std::path::Path {
        &self.cwd
    }

    /// The Puffer workspace root (config + sessions live underneath).
    /// One of the allowed roots for filesystem RPCs.
    pub(crate) fn workspace_root(&self) -> std::path::PathBuf {
        self.paths.workspace_root.clone()
    }

    pub(crate) fn config_paths(&self) -> &ConfigPaths {
        &self.paths
    }
}

struct TurnHandle {
    cancel: CancelToken,
    pending: Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<PermissionPromptAction>>>>,
    pending_questions:
        Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<UserQuestionPromptResponse>>>>,
}

impl DaemonState {
    fn load(
        cwd: std::path::PathBuf,
        paths: ConfigPaths,
        token: String,
        no_browser: bool,
        disable_auto_title: bool,
        yolo: bool,
    ) -> Result<Self> {
        let config = load_config(&paths)?;
        let (events, _rx) = broadcast::channel::<ServerEnvelope>(256);
        let browser_profile_root = paths.user_config_dir.join("browser-profiles");
        let ptys = Arc::new(PtyRegistry::new());
        ptys.spawn_idle_pruner();
        Ok(Self {
            cwd,
            paths,
            token,
            config: Mutex::new(config),
            events,
            turns: Arc::new(Mutex::new(HashMap::new())),
            next_request_id: Arc::new(AtomicU64::new(0)),
            ptys,
            fs_watches: Arc::new(FsWatchRegistry::new()),
            browsers: Arc::new(BrowserRegistry::new(browser_profile_root, !no_browser)),
            disable_auto_title,
            yolo,
            recent_events: Arc::new(Mutex::new(VecDeque::with_capacity(RECENT_EVENT_CAPACITY))),
        })
    }

    /// Returns a clone of the daemon event bus sender for background
    /// subsystems that need to stream UI-visible events.
    pub(crate) fn event_sender(&self) -> broadcast::Sender<ServerEnvelope> {
        self.events.clone()
    }

    /// Publishes `env` to the broadcast bus and — if it's a replay-worthy
    /// event — tees a copy into the ring buffer. UIs that reconnect after a
    /// drop get the last N events replayed so they don't miss text deltas /
    /// tool updates.
    pub(crate) fn publish_event(&self, env: ServerEnvelope) {
        if let ServerEnvelope::Event { event, .. } = &env {
            if is_replay_channel(event) {
                let mut buf = self.recent_events.lock().unwrap();
                if buf.len() >= RECENT_EVENT_CAPACITY {
                    buf.pop_front();
                }
                buf.push_back(env.clone());
            }
        }
        let _ = self.events.send(env);
    }

    /// Snapshot of the current replay buffer — used on WebSocket connect
    /// to catch new clients up on in-flight work.
    fn replay_snapshot(&self) -> Vec<ServerEnvelope> {
        self.recent_events.lock().unwrap().iter().cloned().collect()
    }
}

/// Whether an event channel should be teed into the replay buffer. We only
/// persist events that carry transcript- or progress-level state; pure
/// fire-and-forget UI pings (like `hello`) aren't useful to replay.
fn is_replay_channel(event: &str) -> bool {
    event.starts_with("session:")
        || event.starts_with("workspace:")
        || event.starts_with("clone:")
        || event.starts_with("workflow:")
}

impl DaemonState {
    fn build_runtime_inputs(&self) -> Result<RuntimeInputs> {
        let auth_path = self.paths.user_config_dir.join("auth.json");
        let auth_store = AuthStore::load(&auth_path)?;
        let resources = load_resources(&self.paths, &puffer_runner_local::LocalToolRunner::new())?;
        let mut providers = ProviderRegistry::new();
        for provider in &resources.providers {
            providers.register_with_source(
                provider.value.clone().into_descriptor(),
                provider.source_info.as_provider_source(),
            );
        }
        let config = self.config.lock().unwrap().clone();
        providers.apply_openai_base_url_override(config.openai_base_url.as_deref());
        if !config.openai_headers.is_empty() {
            providers.set_openai_headers(
                config
                    .openai_headers
                    .clone()
                    .into_iter()
                    .collect::<IndexMap<_, _>>(),
            );
        }
        if !config.openai_query_params.is_empty() {
            providers.set_openai_query_params(
                config
                    .openai_query_params
                    .clone()
                    .into_iter()
                    .collect::<IndexMap<_, _>>(),
            );
        }
        let _ = providers.discover_and_merge_all(&auth_store);
        let session_store = SessionStore::from_paths(&self.paths)?;
        Ok(RuntimeInputs {
            resources,
            providers,
            auth_store,
            session_store,
        })
    }
}

struct RuntimeInputs {
    resources: LoadedResources,
    providers: ProviderRegistry,
    auth_store: AuthStore,
    session_store: SessionStore,
}

// ---------------------------------------------------------------------------
// WebSocket handler + RPC dispatcher
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct WsQuery {
    token: String,
}

#[derive(Deserialize, Debug)]
struct ClientRequest {
    id: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum ServerEnvelope {
    Response {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<RpcError>,
    },
    Event {
        event: String,
        payload: Value,
    },
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RpcError {
    pub(crate) code: String,
    pub(crate) message: String,
}

async fn ws_handler(
    State(state): State<Arc<DaemonState>>,
    Query(q): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if !constant_time_eq(q.token.as_bytes(), state.token.as_bytes()) {
        return (axum::http::StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut x = 0u8;
    for (ai, bi) in a.iter().zip(b.iter()) {
        x |= ai ^ bi;
    }
    x == 0
}

async fn handle_socket(socket: WebSocket, state: Arc<DaemonState>) {
    let (ws_tx, mut ws_rx) = socket.split();
    let tx = Arc::new(AsyncMutex::new(ws_tx));

    // Say hello.
    let _ = send_envelope(
        &tx,
        &ServerEnvelope::Event {
            event: "hello".to_string(),
            payload: json!({
                "protocolVersion": PROTOCOL_VERSION,
                "workspaceRoot": state.paths.workspace_root.display().to_string(),
            }),
        },
    )
    .await;

    // Replay the ring buffer so UIs that disconnected mid-turn catch up on
    // events they missed. Each replayed envelope gets `"replay": true`
    // merged into its payload so the UI can distinguish re-delivery from
    // fresh events (App.svelte already dedupes tool cards by callId, but
    // this flag lets other handlers suppress double-counting / toasts).
    let snapshot = state.replay_snapshot();
    for mut env in snapshot {
        if let ServerEnvelope::Event { payload, .. } = &mut env {
            if let Some(map) = payload.as_object_mut() {
                map.insert("replay".to_string(), Value::Bool(true));
            }
        }
        if send_envelope(&tx, &env).await.is_err() {
            return;
        }
    }

    // Fan out global events to this client.
    let mut events_rx = state.events.subscribe();
    let tx_events = tx.clone();
    let event_forwarder = tokio::spawn(async move {
        while let Ok(env) = events_rx.recv().await {
            if send_envelope(&tx_events, &env).await.is_err() {
                break;
            }
        }
    });

    // Handle incoming requests.
    while let Some(Ok(msg)) = ws_rx.next().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let request: ClientRequest = match serde_json::from_str(&text) {
            Ok(r) => r,
            Err(e) => {
                let _ = send_envelope(
                    &tx,
                    &ServerEnvelope::Response {
                        id: "?".to_string(),
                        result: None,
                        error: Some(RpcError {
                            code: "parse-error".to_string(),
                            message: e.to_string(),
                        }),
                    },
                )
                .await;
                continue;
            }
        };
        dispatch_request(request, state.clone(), tx.clone()).await;
    }

    event_forwarder.abort();
}

async fn send_envelope(
    tx: &Arc<AsyncMutex<futures::stream::SplitSink<WebSocket, Message>>>,
    env: &ServerEnvelope,
) -> Result<(), axum::Error> {
    let text = serde_json::to_string(env).unwrap();
    let mut guard = tx.lock().await;
    guard.send(Message::Text(text)).await
}

/// Runs a sync handler on a fresh OS thread (no tokio context) and awaits
/// its result. Required for any handler that transitively builds + drops
/// a `reqwest::blocking::Client` — the inner runtime panics on drop when
/// dropped from a tokio worker. See the agent-turn loop above for the
/// same pattern. Results route back through a oneshot so the caller stays
/// async-friendly.
async fn run_off_runtime<F>(f: F) -> Result<Value>
where
    F: FnOnce() -> Result<Value> + Send + 'static,
{
    let (tx_done, rx_done) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx_done.send(f());
    });
    rx_done
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("handler thread panicked or vanished")))
}

async fn dispatch_request(
    request: ClientRequest,
    state: Arc<DaemonState>,
    tx: Arc<AsyncMutex<futures::stream::SplitSink<WebSocket, Message>>>,
) {
    let id = request.id.clone().unwrap_or_default();
    let params = request.params;

    macro_rules! respond {
        ($result:expr) => {{
            let env = match $result {
                Ok(v) => ServerEnvelope::Response {
                    id: id.clone(),
                    result: Some(v),
                    error: None,
                },
                Err(e) => ServerEnvelope::Response {
                    id: id.clone(),
                    result: None,
                    error: Some(RpcError {
                        code: "method-error".to_string(),
                        message: format!("{e:#}"),
                    }),
                },
            };
            let _ = send_envelope(&tx, &env).await;
        }};
    }

    /// Bind state/params clones into a `move ||` closure so a sync handler
    /// can run on a non-tokio thread. Two arms — handlers that take just
    /// the state and handlers that take state + params.
    macro_rules! detached {
        (|$s:ident| $body:expr) => {{
            let $s = state.clone();
            run_off_runtime(move || $body).await
        }};
        (|$s:ident, $p:ident| $body:expr) => {{
            let $s = state.clone();
            let $p = params.clone();
            run_off_runtime(move || $body).await
        }};
    }

    match request.method.as_str() {
        "ping" => {
            let _ = send_envelope(
                &tx,
                &ServerEnvelope::Response {
                    id,
                    result: Some(json!({"ok": true, "protocolVersion": PROTOCOL_VERSION})),
                    error: None,
                },
            )
            .await;
        }

        "list_grouped_sessions" => respond!(handle_list_grouped_sessions(&state)),
        "load_desktop_pins" => respond!(handle_load_desktop_pins(&state)),
        "set_desktop_pin" => respond!(handle_set_desktop_pin(&state, &params)),
        "load_file_tabs" => respond!(handle_load_file_tabs(&state, &params)),
        "save_file_tabs" => respond!(handle_save_file_tabs(&state, &params)),
        "load_session_detail" => respond!(handle_load_session_detail(&state, &params)),
        "rename_session" => respond!(handle_rename_session(&state, &params)),
        "refresh_repo_status" => respond!(handle_refresh_repo_status(&state, &params)),
        "load_settings_snapshot" => respond!(detached!(|s| handle_load_settings_snapshot(&s))),
        "login_with_api_key" => {
            respond!(detached!(|s, p| handle_login_with_api_key(&s, &p)))
        }
        "login_with_oauth" => {
            respond!(detached!(|s, p| handle_login_with_oauth(&s, &p)))
        }
        "list_external_credentials" => {
            respond!(detached!(|s| handle_list_external_credentials(&s)))
        }
        "import_external_credential" => {
            respond!(detached!(|s, p| handle_import_external_credential(&s, &p)))
        }
        "logout_provider" => respond!(detached!(|s, p| handle_logout_provider(&s, &p))),
        "list_mcp_servers" => respond!(detached!(|s| handle_list_mcp_servers(&s))),
        "add_mcp_server" => respond!(detached!(|s, p| handle_add_mcp_server(&s, &p))),
        "list_provider_models" => {
            respond!(detached!(|s, p| handle_list_provider_models(&s, &p)))
        }
        "list_permissions" => respond!(handle_list_permissions(&state)),
        "save_permissions" => respond!(handle_save_permissions(&state, &params)),
        "update_config" => respond!(detached!(|s, p| handle_update_config(&s, &p))),
        "create_pull_request" => respond!(handle_create_pull_request(&state, &params)),
        "merge_pull_request" => respond!(handle_merge_pull_request(&state, &params)),
        "create_session" => respond!(handle_create_session(&state, &params)),
        "default_workspace" => respond!(handle_default_workspace(&state)),
        "git_clone" => respond!(handle_git_clone(&state, &params)),
        "pty_list" => respond!(handle_pty_list(&state, &params)),
        "pty_open" => respond!(handle_pty_open(&state, &params)),
        "pty_focus" => respond!(handle_pty_focus(&state, &params)),
        "pty_replay" => respond!(handle_pty_replay(&state, &params)),
        "pty_rename" => respond!(handle_pty_rename(&state, &params)),
        "pty_write" => respond!(handle_pty_write(&state, &params)),
        "pty_resize" => respond!(handle_pty_resize(&state, &params)),
        "pty_close" => respond!(handle_pty_close(&state, &params)),
        "list_dir" => respond!(crate::daemon_files::handle_list_dir(&state, &params)),
        "read_file" => respond!(crate::daemon_files::handle_read_file(&state, &params)),
        "write_file" => respond!(crate::daemon_files::handle_write_file(&state, &params)),
        "lsp_inspect" => respond!(detached!(|s, p| crate::daemon_lsp::handle_lsp_inspect(
            &s, &p
        ))),
        "fs_watch" => respond!(crate::daemon_fs_watch::handle_fs_watch(&state, &params)),
        "fs_unwatch" => respond!(crate::daemon_fs_watch::handle_fs_unwatch(&state, &params)),
        "browser_open" => respond!(detached!(
            |s, p| crate::daemon_browser::handle_browser_open(&s, &p)
        )),
        "browser_navigate" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_navigate(&s, &p)
        })),
        "browser_reload" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_reload(&s, &p)
        })),
        "browser_history" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_history(&s, &p)
        })),
        "browser_resize" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_resize(&s, &p)
        })),
        "browser_input" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_input(&s, &p)
        })),
        "browser_copy_selection" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_copy_selection(&s, &p)
        })),
        "browser_cursor" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_cursor(&s, &p)
        })),
        "browser_close" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_close(&s, &p)
        })),
        "browser_recording" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_recording(&s, &p)
        })),
        "browser_agent" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_agent(&s, &p)
        })),
        "workflow_list" => respond!(handle_workflow_list(&state)),
        "workflow_runs_list" => respond!(handle_workflow_runs_list(&state, &params)),
        "workflow_run_show" => respond!(handle_workflow_run_show(&state, &params)),

        "run_agent_turn" => {
            let tx_clone = tx.clone();
            let state_clone = state.clone();
            let id_clone = id.clone();
            tokio::spawn(async move {
                let result = start_turn(state_clone, params).await;
                let env = match result {
                    Ok(v) => ServerEnvelope::Response {
                        id: id_clone,
                        result: Some(v),
                        error: None,
                    },
                    Err(e) => ServerEnvelope::Response {
                        id: id_clone,
                        result: None,
                        error: Some(RpcError {
                            code: "turn-start-error".to_string(),
                            message: format!("{e:#}"),
                        }),
                    },
                };
                let _ = send_envelope(&tx_clone, &env).await;
            });
        }

        "resolve_permission" => respond!(handle_resolve_permission(&state, &params)),
        "resolve_user_question" => respond!(handle_resolve_user_question(&state, &params)),
        "cancel_turn" => respond!(handle_cancel_turn(&state, &params)),

        other => {
            let _ = send_envelope(
                &tx,
                &ServerEnvelope::Response {
                    id,
                    result: None,
                    error: Some(RpcError {
                        code: "unknown-method".to_string(),
                        message: format!("unknown method `{other}`"),
                    }),
                },
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// RPC handlers — blocking work runs in a tokio::task::spawn_blocking closure.
// ---------------------------------------------------------------------------

fn handle_list_grouped_sessions(state: &DaemonState) -> Result<Value> {
    let session_store = SessionStore::from_paths(&state.paths)?;
    let groups: Vec<FolderGroupDto> = desktop_api::list_grouped_sessions(&session_store)?;
    Ok(serde_json::to_value(groups)?)
}

fn handle_load_desktop_pins(state: &DaemonState) -> Result<Value> {
    let pins = load_pin_state(&state.paths.user_config_dir)?;
    Ok(serde_json::to_value(pins)?)
}

fn handle_set_desktop_pin(state: &DaemonState, params: &Value) -> Result<Value> {
    let kind = params
        .get("kind")
        .and_then(Value::as_str)
        .context("missing kind")?;
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .context("missing id")?;
    let pinned = params
        .get("pinned")
        .and_then(Value::as_bool)
        .context("missing pinned")?;
    let pins: DesktopPinState = set_pin_state(&state.paths.user_config_dir, kind, id, pinned)?;
    state.publish_event(ServerEnvelope::Event {
        event: "desktop:pins:changed".to_string(),
        payload: serde_json::to_value(&pins)?,
    });
    Ok(serde_json::to_value(pins)?)
}

fn handle_load_file_tabs(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(Value::as_str)
        .context("missing sessionId")?;
    let stored = load_file_tabs_state(&state.paths.user_config_dir, session_id)?;
    Ok(serde_json::to_value(validate_file_tabs_state(
        state, stored,
    ))?)
}

fn handle_save_file_tabs(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(Value::as_str)
        .context("missing sessionId")?;
    let requested: DesktopFileTabsState =
        serde_json::from_value(params.clone()).context("invalid file tabs payload")?;
    let validated = validate_file_tabs_state(state, requested);
    let saved = set_file_tabs_state(&state.paths.user_config_dir, session_id, validated)?;
    Ok(serde_json::to_value(saved)?)
}

fn validate_file_tabs_state(
    state: &DaemonState,
    requested: DesktopFileTabsState,
) -> DesktopFileTabsState {
    let mut tabs = Vec::new();
    for tab in requested.tabs.into_iter().take(64) {
        let Ok(path) = crate::daemon_files::validate_path(state, &tab.path) else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            continue;
        }
        tabs.push(DesktopFileTab {
            path: path.display().to_string(),
            pinned: tab.pinned,
        });
    }

    let active_path = requested.active_path.and_then(|active| {
        tabs.iter()
            .find(|tab| tab.path == active)
            .map(|tab| tab.path.clone())
            .or_else(|| {
                crate::daemon_files::validate_path(state, &active)
                    .ok()
                    .and_then(|path| {
                        let canonical = path.display().to_string();
                        tabs.iter()
                            .any(|tab| tab.path == canonical)
                            .then_some(canonical)
                    })
            })
    });

    DesktopFileTabsState {
        active_path: active_path.or_else(|| tabs.first().map(|tab| tab.path.clone())),
        tabs,
    }
}

fn handle_load_session_detail(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(|v| v.as_str())
        .context("missing sessionId")?;
    let session_store = SessionStore::from_paths(&state.paths)?;
    let detail: SessionDetailDto = desktop_api::load_session_detail(&session_store, session_id)?;
    Ok(serde_json::to_value(detail)?)
}

fn handle_rename_session(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(|v| v.as_str())
        .context("missing sessionId")?;
    let title = params
        .get("title")
        .and_then(|v| v.as_str())
        .context("missing title")?
        .trim();
    let session_uuid = Uuid::parse_str(session_id).context("invalid sessionId")?;
    let session_store = SessionStore::from_paths(&state.paths)?;
    if title.is_empty() {
        session_store.set_display_name(session_uuid, None)?;
    } else {
        session_store.rename_session(session_uuid, title.to_string())?;
    }
    state.publish_event(ServerEnvelope::Event {
        event: "workspace:sessions:changed".to_string(),
        payload: json!({
            "reason": "rename_session",
            "sessionId": session_id,
        }),
    });
    let detail: SessionDetailDto = desktop_api::load_session_detail(&session_store, session_id)?;
    Ok(serde_json::to_value(detail)?)
}

fn handle_refresh_repo_status(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(|v| v.as_str())
        .context("missing sessionId")?;
    let session_store = SessionStore::from_paths(&state.paths)?;
    let cwd = desktop_api::load_session_cwd(&session_store, session_id)?;
    let status: RepoStatusDto = desktop_api::repo_status(session_id, &cwd);
    Ok(serde_json::to_value(status)?)
}

fn handle_load_settings_snapshot(state: &DaemonState) -> Result<Value> {
    let inputs = state.build_runtime_inputs()?;
    let config = state.config.lock().unwrap().clone();
    let snapshot: SettingsSnapshotDto = desktop_api::load_settings_snapshot(
        &state.paths,
        &config,
        &inputs.resources,
        &inputs.providers,
        &inputs.auth_store,
        &inputs.session_store,
    )?;
    Ok(serde_json::to_value(snapshot)?)
}

fn handle_workflow_list(state: &DaemonState) -> Result<Value> {
    let store = WorkflowStore::new(&state.paths.workspace_config_dir);
    Ok(serde_json::to_value(store.snapshot()?)?)
}

fn handle_workflow_runs_list(state: &DaemonState, params: &Value) -> Result<Value> {
    let slug = params
        .get("workflowSlug")
        .or_else(|| params.get("workflow_slug"))
        .and_then(Value::as_str)
        .context("missing workflowSlug")?;
    let store = WorkflowStore::new(&state.paths.workspace_config_dir);
    Ok(serde_json::to_value(store.list_runs_for(slug)?)?)
}

fn handle_workflow_run_show(state: &DaemonState, params: &Value) -> Result<Value> {
    let idx = params
        .get("idx")
        .and_then(Value::as_u64)
        .context("missing idx")?;
    let store = WorkflowStore::new(&state.paths.workspace_config_dir);
    Ok(serde_json::to_value(store.get_run(idx)?)?)
}

/// Stores an API key credential in the workspace auth store and returns
/// the refreshed settings snapshot so the UI can re-render without a
/// second round-trip.
fn handle_login_with_api_key(state: &DaemonState, params: &Value) -> Result<Value> {
    let provider_id = params
        .get("providerId")
        .or_else(|| params.get("provider_id"))
        .and_then(|v| v.as_str())
        .context("missing providerId")?;
    let api_key = params
        .get("apiKey")
        .or_else(|| params.get("api_key"))
        .and_then(|v| v.as_str())
        .context("missing apiKey")?;

    let mut inputs = state.build_runtime_inputs()?;
    let auth_path = state.paths.user_config_dir.join("auth.json");
    desktop_api::store_api_key(
        &mut inputs.auth_store,
        &inputs.providers,
        &auth_path,
        provider_id,
        api_key,
    )?;
    let _ = desktop_api::ensure_default_routing(
        &state.paths,
        &inputs.providers,
        &inputs.auth_store,
        provider_id,
    );
    reload_daemon_config(state)?;
    // Rebuild so provider discovery can pick up the newly-stored key.
    let fresh = state.build_runtime_inputs()?;
    let config = state.config.lock().unwrap().clone();
    let snapshot: SettingsSnapshotDto = desktop_api::load_settings_snapshot(
        &state.paths,
        &config,
        &fresh.resources,
        &fresh.providers,
        &fresh.auth_store,
        &fresh.session_store,
    )?;
    Ok(serde_json::to_value(snapshot)?)
}

/// Runs the provider OAuth flow from the daemon host and returns a fresh snapshot.
fn handle_login_with_oauth(state: &DaemonState, params: &Value) -> Result<Value> {
    let provider_id = params
        .get("providerId")
        .or_else(|| params.get("provider_id"))
        .and_then(|v| v.as_str())
        .context("missing providerId")?;

    let mut inputs = state.build_runtime_inputs()?;
    let auth_path = state.paths.user_config_dir.join("auth.json");
    let listener = crate::authflow::CallbackListener::bind_localhost("/callback")?;
    let bundle =
        oauth_login_bundle_for_provider(&inputs.providers, provider_id, listener.redirect_uri())?;
    let launch_url = bundle
        .automatic_authorization_url
        .as_deref()
        .unwrap_or(bundle.authorization_url.as_str());
    if !crate::authflow::open_browser(launch_url) {
        anyhow::bail!("could not open the system browser for `{provider_id}` login");
    }
    let callback = listener
        .wait_for_callback_url(Duration::from_secs(180))?
        .ok_or_else(|| anyhow::anyhow!("timed out waiting for OAuth callback"))?;

    match oauth_family_for_provider(&inputs.providers, provider_id) {
        Some(OauthFamily::OpenAi) => {
            let (code, parsed_state) = parse_openai_authorization_input(&callback);
            let code = code
                .ok_or_else(|| anyhow::anyhow!("could not extract an OpenAI authorization code"))?;
            if let Some(parsed_state) = parsed_state {
                if parsed_state != bundle.state {
                    anyhow::bail!("oauth state mismatch for openai");
                }
            }
            let credential = exchange_openai_authorization_code(&code, &bundle.verifier, None)?;
            set_stored_credential(
                &mut inputs.auth_store,
                provider_id.to_string(),
                StoredCredential::OAuth(to_registry_oauth_credential_openai(credential)),
            );
        }
        Some(OauthFamily::Anthropic) => {
            let (code, parsed_state) = parse_anthropic_authorization_input(&callback);
            let code = code.ok_or_else(|| {
                anyhow::anyhow!("could not extract an Anthropic authorization code")
            })?;
            let parsed_state = parsed_state.unwrap_or_else(|| bundle.state.clone());
            if parsed_state != bundle.state {
                anyhow::bail!("oauth state mismatch for anthropic");
            }
            let redirect_uri = inferred_anthropic_redirect_uri(&callback)
                .or_else(|| bundle.manual_redirect_uri.clone())
                .unwrap_or_else(|| ANTHROPIC_MANUAL_REDIRECT_URL.to_string());
            let credential = exchange_anthropic_authorization_code(
                &code,
                &bundle.verifier,
                &bundle.state,
                Some(&redirect_uri),
                Some(ANTHROPIC_API_BASE_URL),
            )?;
            store_anthropic_credential(&mut inputs.auth_store, provider_id, credential)?;
        }
        None => anyhow::bail!("oauth login is not implemented for {provider_id}"),
    }

    inputs.auth_store.save(&auth_path)?;
    let _ = desktop_api::ensure_default_routing(
        &state.paths,
        &inputs.providers,
        &inputs.auth_store,
        provider_id,
    );
    reload_daemon_config(state)?;
    let fresh = state.build_runtime_inputs()?;
    let config = state.config.lock().unwrap().clone();
    let snapshot: SettingsSnapshotDto = desktop_api::load_settings_snapshot(
        &state.paths,
        &config,
        &fresh.resources,
        &fresh.providers,
        &fresh.auth_store,
        &fresh.session_store,
    )?;
    Ok(serde_json::to_value(snapshot)?)
}

/// Lists importable credentials discovered under external tool config roots.
fn handle_list_external_credentials(state: &DaemonState) -> Result<Value> {
    let inputs = state.build_runtime_inputs()?;
    let credentials: Vec<ExternalCredentialDto> =
        desktop_api::list_external_credentials(&inputs.providers)?;
    Ok(serde_json::to_value(credentials)?)
}

/// Imports a discovered external credential and returns a fresh settings snapshot.
fn handle_import_external_credential(state: &DaemonState, params: &Value) -> Result<Value> {
    let provider_id = params
        .get("providerId")
        .or_else(|| params.get("provider_id"))
        .and_then(|v| v.as_str())
        .context("missing providerId")?;
    let source = params
        .get("source")
        .and_then(|v| v.as_str())
        .context("missing source")?;

    let mut inputs = state.build_runtime_inputs()?;
    let auth_path = state.paths.user_config_dir.join("auth.json");
    desktop_api::import_external_credential(
        &state.paths,
        &mut inputs.auth_store,
        &inputs.providers,
        &auth_path,
        provider_id,
        source,
    )?;
    let _ = desktop_api::ensure_default_routing(
        &state.paths,
        &inputs.providers,
        &inputs.auth_store,
        provider_id,
    );
    reload_daemon_config(state)?;
    let fresh = state.build_runtime_inputs()?;
    let config = state.config.lock().unwrap().clone();
    let snapshot: SettingsSnapshotDto = desktop_api::load_settings_snapshot(
        &state.paths,
        &config,
        &fresh.resources,
        &fresh.providers,
        &fresh.auth_store,
        &fresh.session_store,
    )?;
    Ok(serde_json::to_value(snapshot)?)
}

fn reload_daemon_config(state: &DaemonState) -> Result<()> {
    let config = load_config(&state.paths)?;
    *state.config.lock().unwrap() = config;
    Ok(())
}

/// Removes stored credentials for a provider and returns the refreshed
/// settings snapshot.
fn handle_logout_provider(state: &DaemonState, params: &Value) -> Result<Value> {
    let provider_id = params
        .get("providerId")
        .or_else(|| params.get("provider_id"))
        .and_then(|v| v.as_str())
        .context("missing providerId")?;
    let mut inputs = state.build_runtime_inputs()?;
    let auth_path = state.paths.user_config_dir.join("auth.json");
    desktop_api::logout_provider(&mut inputs.auth_store, &auth_path, provider_id)?;
    let fresh = state.build_runtime_inputs()?;
    let config = state.config.lock().unwrap().clone();
    let snapshot: SettingsSnapshotDto = desktop_api::load_settings_snapshot(
        &state.paths,
        &config,
        &fresh.resources,
        &fresh.providers,
        &fresh.auth_store,
        &fresh.session_store,
    )?;
    Ok(serde_json::to_value(snapshot)?)
}

/// Lists all MCP servers discovered across workspace + user + builtin
/// resource roots.
fn handle_list_mcp_servers(state: &DaemonState) -> Result<Value> {
    let inputs = state.build_runtime_inputs()?;
    let servers = mcp_server_dtos(&inputs.resources);
    Ok(json!({ "servers": servers }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMcpServerParams {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    transport: String,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

fn handle_add_mcp_server(state: &DaemonState, params: &Value) -> Result<Value> {
    let params: AddMcpServerParams = serde_json::from_value(params.clone())?;
    let id = params.id.trim();
    validate_mcp_id(id)?;
    let transport = params.transport.trim();
    if !matches!(transport, "stdio" | "sse" | "http") {
        anyhow::bail!("unsupported MCP transport `{transport}`");
    }

    let endpoint = params.endpoint.unwrap_or_default().trim().to_string();
    let target = params.target.unwrap_or_default().trim().to_string();
    if transport == "stdio" && target.is_empty() {
        anyhow::bail!("stdio MCP servers require a command");
    }
    if transport != "stdio" && endpoint.is_empty() {
        anyhow::bail!("{transport} MCP servers require a URL");
    }

    let spec = McpServerSpec {
        id: id.to_string(),
        display_name: params
            .display_name
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| id.to_string()),
        transport: transport.to_string(),
        endpoint,
        target,
        description: params
            .description
            .map(|value| value.trim().to_string())
            .unwrap_or_default(),
        headers: Default::default(),
        oauth: None,
    };

    let dir = match params.scope.as_deref().unwrap_or("local") {
        "user" => state.paths.user_config_dir.join("resources/mcp_servers"),
        "local" | "project" | "workspace" => state
            .paths
            .workspace_config_dir
            .join("resources/mcp_servers"),
        other => anyhow::bail!("unsupported MCP scope `{other}`"),
    };
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{id}.yaml"));
    std::fs::write(&path, serde_yaml::to_string(&spec)?)?;

    let inputs = state.build_runtime_inputs()?;
    Ok(json!({ "servers": mcp_server_dtos(&inputs.resources) }))
}

fn validate_mcp_id(id: &str) -> Result<()> {
    if id.is_empty() {
        anyhow::bail!("MCP server id is required");
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        anyhow::bail!(
            "MCP server id may only contain letters, numbers, dots, dashes, and underscores"
        );
    }
    Ok(())
}

fn mcp_server_dtos(resources: &LoadedResources) -> Vec<McpServerDto> {
    resources
        .mcp_servers
        .iter()
        .map(|item| McpServerDto {
            id: item.value.id.clone(),
            display_name: if item.value.display_name.is_empty() {
                item.value.id.clone()
            } else {
                item.value.display_name.clone()
            },
            description: item.value.description.clone(),
            transport: item.value.transport.clone(),
            endpoint: item.value.endpoint.clone(),
            target: item.value.target.clone(),
            source_kind: format!("{:?}", item.source_info.kind).to_lowercase(),
            source_path: Some(item.source_info.path.display().to_string()),
        })
        .collect()
}

/// Returns the full model list for one provider. The snapshot only carries
/// a count; the Settings → Models pane calls this to populate the picker.
fn handle_list_provider_models(state: &DaemonState, params: &Value) -> Result<Value> {
    let provider_id = params
        .get("providerId")
        .or_else(|| params.get("provider_id"))
        .and_then(|v| v.as_str())
        .context("missing providerId")?;
    let inputs = state.build_runtime_inputs()?;
    let entry = inputs
        .providers
        .provider_entries()
        .find(|p| p.descriptor.id == provider_id)
        .with_context(|| format!("unknown provider `{provider_id}`"))?;
    let models: Vec<ModelDescriptorDto> = entry
        .descriptor
        .models
        .iter()
        .map(|m| ModelDescriptorDto {
            id: m.id.clone(),
            display_name: m.display_name.clone(),
            provider: m.provider.clone(),
            api: m.api.clone(),
            context_window: m.context_window,
            max_output_tokens: m.max_output_tokens,
            supports_reasoning: m.supports_reasoning,
        })
        .collect();
    Ok(json!({ "providerId": provider_id, "models": models }))
}

/// Workspace permissions are stored as a TOML map of `tool_id → policy`
/// (e.g. `bash = "ask"`). We read the file directly so we don't have to
/// plumb the `pub(crate)` type through puffer-core.
#[derive(serde::Deserialize, serde::Serialize, Default)]
struct PermissionsFileDto {
    #[serde(default)]
    tools: std::collections::BTreeMap<String, String>,
}

fn permissions_file_path(state: &DaemonState) -> std::path::PathBuf {
    state.paths.workspace_config_dir.join("permissions.toml")
}

fn handle_list_permissions(state: &DaemonState) -> Result<Value> {
    let path = permissions_file_path(state);
    let loaded: PermissionsFileDto = if path.exists() {
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?
    } else {
        PermissionsFileDto::default()
    };
    Ok(json!({
        "path": path.display().to_string(),
        "tools": loaded.tools,
    }))
}

fn handle_save_permissions(state: &DaemonState, params: &Value) -> Result<Value> {
    let tools_val = params.get("tools").context("missing tools map")?;
    let tools: std::collections::BTreeMap<String, String> =
        serde_json::from_value(tools_val.clone()).context("tools must be object<string,string>")?;
    // Normalize: lowercase policy, trim tool ids. Reject unknown policies
    // early so the UI shows a clear error instead of a silent no-op.
    let mut normalized = std::collections::BTreeMap::new();
    for (tool, policy) in tools {
        let t = tool.trim().to_string();
        if t.is_empty() {
            continue;
        }
        let p = policy.trim().to_ascii_lowercase();
        if !matches!(p.as_str(), "allow" | "ask" | "deny" | "disabled") {
            anyhow::bail!("invalid policy `{policy}` for `{t}` — expected allow|ask|deny|disabled");
        }
        normalized.insert(t, p);
    }
    let dto = PermissionsFileDto { tools: normalized };
    let path = permissions_file_path(state);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let serialized = toml::to_string_pretty(&dto).context("serialize permissions.toml")?;
    std::fs::write(&path, serialized).with_context(|| format!("write {}", path.display()))?;
    Ok(json!({
        "path": path.display().to_string(),
        "tools": dto.tools,
    }))
}

/// Applies partial updates to `PufferConfig` and persists to the user
/// config file. Only allowlisted fields are accepted — unknown keys are
/// rejected so typos surface immediately.
fn handle_update_config(state: &DaemonState, params: &Value) -> Result<Value> {
    let patch = params
        .as_object()
        .context("update_config expects an object")?;
    let mut guard = state.config.lock().unwrap();
    for (key, value) in patch {
        match key.as_str() {
            "defaultProvider" | "default_provider" => {
                guard.default_provider = match value {
                    Value::Null => None,
                    Value::String(s) if s.trim().is_empty() => None,
                    Value::String(s) => Some(s.clone()),
                    _ => anyhow::bail!("defaultProvider must be string or null"),
                };
            }
            "defaultModel" | "default_model" => {
                guard.default_model = match value {
                    Value::Null => None,
                    Value::String(s) if s.trim().is_empty() => None,
                    Value::String(s) => Some(s.clone()),
                    _ => anyhow::bail!("defaultModel must be string or null"),
                };
            }
            "theme" => {
                guard.theme = value
                    .as_str()
                    .context("theme must be a string")?
                    .to_string();
            }
            "openaiBaseUrl" | "openai_base_url" => {
                guard.openai_base_url = match value {
                    Value::Null => None,
                    Value::String(s) if s.trim().is_empty() => None,
                    Value::String(s) => Some(s.clone()),
                    _ => anyhow::bail!("openaiBaseUrl must be string or null"),
                };
            }
            other => anyhow::bail!("update_config: unknown key `{other}`"),
        }
    }
    let snapshot_cfg = guard.clone();
    drop(guard);
    save_user_config(&state.paths, &snapshot_cfg).context("save user config")?;
    // Return the refreshed settings snapshot so the UI re-renders without
    // a second round-trip.
    let inputs = state.build_runtime_inputs()?;
    let snapshot: SettingsSnapshotDto = desktop_api::load_settings_snapshot(
        &state.paths,
        &snapshot_cfg,
        &inputs.resources,
        &inputs.providers,
        &inputs.auth_store,
        &inputs.session_store,
    )?;
    Ok(serde_json::to_value(snapshot)?)
}

fn handle_create_pull_request(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(|v| v.as_str())
        .context("missing sessionId")?;
    let title = params
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let body = params
        .get("body")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let session_store = SessionStore::from_paths(&state.paths)?;
    let cwd = desktop_api::load_session_cwd(&session_store, session_id)?;
    let result: RepoActionResultDto =
        desktop_api::create_pull_request(session_id, &cwd, title, body);
    Ok(serde_json::to_value(result)?)
}

fn handle_merge_pull_request(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(|v| v.as_str())
        .context("missing sessionId")?;
    let pr_number = params
        .get("pullRequestNumber")
        .or_else(|| params.get("pull_request_number"))
        .and_then(|v| v.as_u64());
    let method = params
        .get("mergeMethod")
        .or_else(|| params.get("merge_method"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let session_store = SessionStore::from_paths(&state.paths)?;
    let cwd = desktop_api::load_session_cwd(&session_store, session_id)?;
    let result: RepoActionResultDto =
        desktop_api::merge_pull_request(session_id, &cwd, pr_number, method);
    Ok(serde_json::to_value(result)?)
}

/// Creates a new blank session. If `cwd` is omitted the daemon's boot
/// workspace root is used. Returns a minimal DTO the UI can open
/// immediately — the session's `session_id` is the correlation key for
/// subsequent `run_agent_turn` / `load_session_detail` calls.
fn handle_create_session(state: &DaemonState, params: &Value) -> Result<Value> {
    let cwd = params
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| state.cwd.clone());
    let display_name = params
        .get("displayName")
        .or_else(|| params.get("display_name"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let session_store = SessionStore::from_paths(&state.paths)?;
    let session = session_store.create_session(cwd)?;
    if let Some(display_name) = display_name {
        session_store.set_display_name(session.id, Some(display_name))?;
    }
    let session = session_store.load_session(session.id)?.metadata;
    // Broadcast so connected UIs can refresh their workspace board without
    // polling. Ignored silently if no one's listening.
    state.publish_event(ServerEnvelope::Event {
        event: "workspace:sessions:changed".to_string(),
        payload: json!({
            "reason": "create_session",
            "sessionId": session.id.to_string(),
        }),
    });
    Ok(json!({
        "sessionId": session.id.to_string(),
        "displayName": session.display_name,
        "generatedTitle": session.generated_title,
        "cwd": session.cwd.display().to_string(),
        "createdAtMs": session.created_at_ms,
        "updatedAtMs": session.updated_at_ms,
        "slug": session.slug,
    }))
}

/// Reports the daemon's default workspace — the cwd it booted in, which is
/// also what `create_session` uses when no explicit cwd is provided. The
/// frontend can show this in the onboarding / new-agent picker so the user
/// knows where their session is going.
fn handle_default_workspace(state: &DaemonState) -> Result<Value> {
    Ok(json!({
        "cwd": state.cwd.display().to_string(),
        "workspaceRoot": state.paths.workspace_root.display().to_string(),
    }))
}

/// Clones a git repository to a destination directory. Runs in the daemon's
/// filesystem — callers route "remote clone" by connecting to a remote
/// daemon first (see the SSH launcher).
///
/// Returns IMMEDIATELY with `{cloneId, dest}` — the clone runs in a
/// background std::thread that streams progress over two event channels:
///
///   `clone:<cloneId>:progress` — `{cloneId, line}` for each stderr line
///   `clone:<cloneId>:done`     — `{cloneId, ok, exitCode, dest, stdout, stderr}`
///
/// Callers wait on `clone:<id>:done` to know when the tree is usable.
/// `dest` is interpreted relative to the daemon's cwd when not absolute.
/// If `dest` already exists and is non-empty, the RPC errors out so users
/// don't accidentally trash existing checkouts.
fn handle_git_clone(state: &Arc<DaemonState>, params: &Value) -> Result<Value> {
    let url = params
        .get("url")
        .and_then(|v| v.as_str())
        .context("missing url")?
        .to_string();
    let dest_raw = params
        .get("dest")
        .and_then(|v| v.as_str())
        .context("missing dest")?;
    let depth = params.get("depth").and_then(|v| v.as_u64());

    let dest = {
        let p = std::path::PathBuf::from(dest_raw);
        if p.is_absolute() {
            p
        } else {
            state.cwd.join(p)
        }
    };

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent directory {}", parent.display()))?;
    }
    if dest.exists() {
        let non_empty = std::fs::read_dir(&dest)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);
        if non_empty {
            anyhow::bail!(
                "destination {} already exists and is not empty",
                dest.display()
            );
        }
    }

    let clone_id = Uuid::new_v4().to_string();
    let progress_channel = format!("clone:{clone_id}:progress");
    let done_channel = format!("clone:{clone_id}:done");

    // `--progress` makes git emit progress lines even when stderr is piped
    // (normally it only emits when stderr is a tty). Each carriage-returned
    // "Receiving objects: 17%" line arrives as its own LF-terminated chunk
    // with `--progress` + piped stderr.
    let mut cmd = std::process::Command::new("git");
    cmd.arg("clone").arg("--progress");
    if let Some(d) = depth {
        cmd.arg("--depth").arg(d.to_string());
    }
    cmd.arg("--").arg(&url).arg(&dest);
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let mut child = cmd
        .spawn()
        .context("failed to spawn `git clone` — is git installed?")?;
    let stderr = child.stderr.take().context("git clone stderr missing")?;
    let stdout_pipe = child.stdout.take();

    let clone_id_thread = clone_id.clone();
    let dest_thread = dest.clone();
    let state_thread = state.clone();
    std::thread::spawn(move || {
        // Git uses '\r' to repaint progress (e.g. "Receiving objects: 42%").
        // Splitting on *either* \r or \n gives us one event per repaint.
        let reader = BufReader::new(stderr);
        let mut captured = String::new();
        let mut buf = Vec::<u8>::with_capacity(512);
        let mut bytes = reader.bytes();
        while let Some(Ok(b)) = bytes.next() {
            if b == b'\r' || b == b'\n' {
                if !buf.is_empty() {
                    let line = String::from_utf8_lossy(&buf).into_owned();
                    captured.push_str(&line);
                    captured.push('\n');
                    state_thread.publish_event(ServerEnvelope::Event {
                        event: progress_channel.clone(),
                        payload: json!({
                            "cloneId": clone_id_thread,
                            "line": line,
                        }),
                    });
                    buf.clear();
                }
            } else {
                buf.push(b);
            }
        }
        if !buf.is_empty() {
            let line = String::from_utf8_lossy(&buf).into_owned();
            captured.push_str(&line);
            captured.push('\n');
            state_thread.publish_event(ServerEnvelope::Event {
                event: progress_channel.clone(),
                payload: json!({
                    "cloneId": clone_id_thread,
                    "line": line,
                }),
            });
        }

        // Drain stdout (usually empty, but don't leak the pipe).
        let stdout_captured = if let Some(out) = stdout_pipe {
            let mut s = String::new();
            let _ = BufReader::new(out).read_to_string(&mut s);
            s
        } else {
            String::new()
        };

        let status = match child.wait() {
            Ok(s) => s,
            Err(err) => {
                state_thread.publish_event(ServerEnvelope::Event {
                    event: done_channel.clone(),
                    payload: json!({
                        "cloneId": clone_id_thread,
                        "ok": false,
                        "exitCode": serde_json::Value::Null,
                        "dest": dest_thread.display().to_string(),
                        "stdout": stdout_captured,
                        "stderr": format!("wait() failed: {err}"),
                    }),
                });
                return;
            }
        };

        state_thread.publish_event(ServerEnvelope::Event {
            event: done_channel,
            payload: json!({
                "cloneId": clone_id_thread,
                "ok": status.success(),
                "exitCode": status.code(),
                "dest": dest_thread.display().to_string(),
                "stdout": stdout_captured,
                "stderr": captured,
            }),
        });
    });

    Ok(json!({
        "cloneId": clone_id,
        "dest": dest.display().to_string(),
    }))
}

// ---------------------------------------------------------------------------
// PTY — user-facing Terminal tab. Spawns a shell per RPC call; data flows
// out via `pty:<id>:data` events and exit status via `pty:<id>:exit`.
// ---------------------------------------------------------------------------

fn handle_pty_list(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(Value::as_str)
        .context("missing sessionId")?;
    Ok(serde_json::to_value(state.ptys.list(session_id))?)
}

fn handle_pty_open(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(Value::as_str)
        .unwrap_or("default")
        .to_string();
    let cwd = params
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| state.cwd.clone());
    let cols = params
        .get("cols")
        .and_then(|v| v.as_u64())
        .map(|n| n as u16)
        .unwrap_or(80);
    let rows = params
        .get("rows")
        .and_then(|v| v.as_u64())
        .map(|n| n as u16)
        .unwrap_or(24);
    let title = params
        .get("title")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let pty_id = state
        .ptys
        .open(state.events.clone(), session_id, cwd, cols, rows, title)?;
    Ok(json!({ "ptyId": pty_id }))
}

fn handle_pty_focus(state: &DaemonState, params: &Value) -> Result<Value> {
    let pty_id = params
        .get("ptyId")
        .or_else(|| params.get("pty_id"))
        .and_then(Value::as_str)
        .context("missing ptyId")?;
    state.ptys.focus(pty_id)?;
    Ok(json!({ "ok": true }))
}

fn handle_pty_replay(state: &DaemonState, params: &Value) -> Result<Value> {
    let pty_id = params
        .get("ptyId")
        .or_else(|| params.get("pty_id"))
        .and_then(Value::as_str)
        .context("missing ptyId")?;
    Ok(json!({ "chunks": state.ptys.replay(pty_id)? }))
}

fn handle_pty_rename(state: &DaemonState, params: &Value) -> Result<Value> {
    let pty_id = params
        .get("ptyId")
        .or_else(|| params.get("pty_id"))
        .and_then(Value::as_str)
        .context("missing ptyId")?;
    let title = params
        .get("title")
        .and_then(Value::as_str)
        .context("missing title")?;
    Ok(serde_json::to_value(
        state.ptys.rename(pty_id, title.to_string())?,
    )?)
}

fn handle_pty_write(state: &DaemonState, params: &Value) -> Result<Value> {
    let pty_id = params
        .get("ptyId")
        .or_else(|| params.get("pty_id"))
        .and_then(|v| v.as_str())
        .context("missing ptyId")?;
    let data = params
        .get("data")
        .and_then(|v| v.as_str())
        .context("missing data")?;
    state.ptys.write(pty_id, data)?;
    Ok(json!({ "ok": true }))
}

fn handle_pty_resize(state: &DaemonState, params: &Value) -> Result<Value> {
    let pty_id = params
        .get("ptyId")
        .or_else(|| params.get("pty_id"))
        .and_then(|v| v.as_str())
        .context("missing ptyId")?;
    let cols = params
        .get("cols")
        .and_then(|v| v.as_u64())
        .map(|n| n as u16)
        .context("missing cols")?;
    let rows = params
        .get("rows")
        .and_then(|v| v.as_u64())
        .map(|n| n as u16)
        .context("missing rows")?;
    state.ptys.resize(pty_id, cols, rows)?;
    Ok(json!({ "ok": true }))
}

fn handle_pty_close(state: &DaemonState, params: &Value) -> Result<Value> {
    let pty_id = params
        .get("ptyId")
        .or_else(|| params.get("pty_id"))
        .and_then(|v| v.as_str())
        .context("missing ptyId")?;
    state.ptys.close(pty_id)?;
    Ok(json!({ "ok": true }))
}

fn handle_resolve_permission(state: &DaemonState, params: &Value) -> Result<Value> {
    let turn_id = params
        .get("turnId")
        .or_else(|| params.get("turn_id"))
        .and_then(|v| v.as_str())
        .context("missing turnId")?;
    let request_id = params
        .get("requestId")
        .or_else(|| params.get("request_id"))
        .and_then(|v| v.as_str())
        .context("missing requestId")?;
    let action_str = params
        .get("action")
        .and_then(|v| v.as_str())
        .context("missing action")?;
    let action = match action_str {
        "allow_once" => PermissionPromptAction::AllowOnce,
        "allow_session" => PermissionPromptAction::AllowSession,
        "allow_all_session" => PermissionPromptAction::AllowAllSession,
        "deny" => PermissionPromptAction::Deny,
        other => anyhow::bail!("unknown action `{other}`"),
    };
    let responder = {
        let mut turns = state.turns.lock().unwrap();
        turns
            .get_mut(turn_id)
            .and_then(|h| h.pending.lock().unwrap().remove(request_id))
    };
    let responder = responder
        .ok_or_else(|| anyhow::anyhow!("no pending request `{request_id}` on turn `{turn_id}`"))?;
    responder
        .send(action)
        .map_err(|_| anyhow::anyhow!("worker already released the permission channel"))?;
    Ok(json!({"ok": true}))
}

fn handle_resolve_user_question(state: &DaemonState, params: &Value) -> Result<Value> {
    let turn_id = params
        .get("turnId")
        .or_else(|| params.get("turn_id"))
        .and_then(|v| v.as_str())
        .context("missing turnId")?;
    let request_id = params
        .get("requestId")
        .or_else(|| params.get("request_id"))
        .and_then(|v| v.as_str())
        .context("missing requestId")?;
    let answers = params
        .get("answers")
        .and_then(|v| v.as_object())
        .cloned()
        .context("missing answers")?;
    let annotations = params
        .get("annotations")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let responder = {
        let mut turns = state.turns.lock().unwrap();
        turns
            .get_mut(turn_id)
            .and_then(|h| h.pending_questions.lock().unwrap().remove(request_id))
    };
    let responder = responder.ok_or_else(|| {
        anyhow::anyhow!("no pending user question request `{request_id}` on turn `{turn_id}`")
    })?;
    responder
        .send(UserQuestionPromptResponse {
            answers,
            annotations,
        })
        .map_err(|_| anyhow::anyhow!("worker already released the user question channel"))?;
    Ok(json!({"ok": true}))
}

fn handle_cancel_turn(state: &DaemonState, params: &Value) -> Result<Value> {
    let turn_id = params
        .get("turnId")
        .or_else(|| params.get("turn_id"))
        .and_then(|v| v.as_str())
        .context("missing turnId")?;
    let mut turns = state.turns.lock().unwrap();
    if let Some(handle) = turns.get_mut(turn_id) {
        handle.cancel.cancel();
        let mut pending = handle.pending.lock().unwrap();
        for (_, tx) in pending.drain() {
            let _ = tx.send(PermissionPromptAction::Deny);
        }
        let mut pending_questions = handle.pending_questions.lock().unwrap();
        for (_, tx) in pending_questions.drain() {
            let _ = tx.send(UserQuestionPromptResponse {
                answers: serde_json::Map::new(),
                annotations: serde_json::Map::new(),
            });
        }
    }
    Ok(json!({"ok": true}))
}

// ---------------------------------------------------------------------------
// Turn driver — spawns a std::thread to run the (synchronous) agent loop
// and relays events onto the broadcast bus.
// ---------------------------------------------------------------------------

async fn start_turn(state: Arc<DaemonState>, params: Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(|v| v.as_str())
        .context("missing sessionId")?
        .to_string();
    let message = params
        .get("message")
        .and_then(|v| v.as_str())
        .context("missing message")?
        .to_string();
    let model_override = params
        .get("modelOverride")
        .or_else(|| params.get("model_override"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    // Parse cheap, non-tokio-touching things synchronously so we can fail
    // fast with a clean error. Anything that builds a runtime (i.e. the
    // provider registry's reqwest::blocking::Client) must NOT run here:
    // we're inside an async task, and dropping that inner runtime from an
    // async context panics. All such setup moved into the worker thread.
    let session_uuid = Uuid::parse_str(&session_id).context("invalid sessionId")?;

    let turn_id = Uuid::new_v4().to_string();
    let channel = format!("session:{session_id}:event");
    let pending: Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<PermissionPromptAction>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_questions: Arc<
        Mutex<HashMap<String, std::sync::mpsc::Sender<UserQuestionPromptResponse>>>,
    > = Arc::new(Mutex::new(HashMap::new()));
    let cancel = CancelToken::new();

    state.turns.lock().unwrap().insert(
        turn_id.clone(),
        TurnHandle {
            cancel: cancel.clone(),
            pending: pending.clone(),
            pending_questions: pending_questions.clone(),
        },
    );

    let state_for_thread = state.clone();
    let turn_id_thread = turn_id.clone();
    let turn_id_resp = turn_id.clone();
    let channel_thread = channel.clone();
    let next_req_id = state.next_request_id.clone();

    // Run the synchronous agent loop on a fresh OS thread, *completely
    // detached* from tokio. `ProviderRegistry::discover_and_merge_all` +
    // `execute_user_turn_streaming_with_permissions` both build tokio
    // runtimes via reqwest internally, and dropping those runtimes while
    // any tokio handle is installed on the current thread panics. A plain
    // `std::thread::spawn` gives us a thread with no ambient tokio state.
    let setup_state = state.clone();
    let message_for_thread = message.clone();
    let session_id_for_thread = session_id.clone();
    let model_override_for_thread = model_override.clone();
    std::thread::spawn(move || {
        setup_state.publish_event(ServerEnvelope::Event {
            event: channel_thread.clone(),
            payload: json!({"type": "turn-start", "turnId": turn_id_thread}),
        });

        // Load provider registry + auth + resources + session record on
        // this thread so the inner reqwest runtime is built + dropped in
        // the same clean (non-tokio) context.
        let mut inputs = match setup_state.build_runtime_inputs() {
            Ok(v) => v,
            Err(err) => {
                setup_state.publish_event(ServerEnvelope::Event {
                    event: channel_thread.clone(),
                    payload: json!({
                        "type": "turn-error",
                        "turnId": turn_id_thread,
                        "error": format!("build_runtime_inputs: {err:#}"),
                    }),
                });
                setup_state.turns.lock().unwrap().remove(&turn_id_thread);
                return;
            }
        };
        let record = match inputs.session_store.load_session(session_uuid) {
            Ok(v) => v,
            Err(err) => {
                setup_state.publish_event(ServerEnvelope::Event {
                    event: channel_thread.clone(),
                    payload: json!({
                        "type": "turn-error",
                        "turnId": turn_id_thread,
                        "error": format!("load_session: {err:#}"),
                    }),
                });
                setup_state.turns.lock().unwrap().remove(&turn_id_thread);
                return;
            }
        };
        let has_user_message = record
            .events
            .iter()
            .any(|event| matches!(event, TranscriptEvent::UserMessage { .. }));
        let auto_title = if !setup_state.disable_auto_title
            && crate::daemon_title::should_auto_title(
                record.metadata.display_name.as_deref(),
                record.metadata.generated_title.as_deref(),
                has_user_message,
            ) {
            match crate::daemon_title::generate_title_with_model(
                &AppState::from_session_record(
                    setup_state.config.lock().unwrap().clone(),
                    record.clone(),
                ),
                &inputs.resources,
                &inputs.providers,
                &mut inputs.auth_store,
                &message_for_thread,
            ) {
                Ok(Some(title)) => Some(title),
                Ok(None) => crate::daemon_title::title_from_first_message(&message_for_thread),
                Err(error) => {
                    eprintln!("session title generation failed; falling back: {error:#}");
                    crate::daemon_title::title_from_first_message(&message_for_thread)
                }
            }
        } else {
            None
        };
        let cfg_for_turn = setup_state.config.lock().unwrap().clone();
        let mut app_state = AppState::from_session_record(cfg_for_turn.clone(), record);
        if setup_state.yolo {
            apply_daemon_yolo_mode(&mut app_state);
        }
        if let Some(model_override) = model_override_for_thread.as_deref() {
            if let Err(err) =
                apply_turn_model_override(&mut app_state, &inputs.providers, model_override)
            {
                setup_state.publish_event(ServerEnvelope::Event {
                    event: channel_thread.clone(),
                    payload: json!({
                        "type": "turn-error",
                        "turnId": turn_id_thread,
                        "error": format!("modelOverride: {err:#}"),
                    }),
                });
                setup_state.turns.lock().unwrap().remove(&turn_id_thread);
                return;
            }
        }
        let runner = crate::runner_selection::select_tool_runner(
            &cfg_for_turn,
            &inputs.resources,
            app_state.cwd.clone(),
        );
        app_state = app_state.with_tool_runner(runner);
        if let Some(title) = auto_title {
            if inputs
                .session_store
                .set_generated_title(session_uuid, Some(title.clone()))
                .is_ok()
            {
                app_state.session.generated_title = Some(title);
                setup_state.publish_event(ServerEnvelope::Event {
                    event: "workspace:sessions:changed".to_string(),
                    payload: json!({
                        "reason": "generated_title",
                        "sessionId": session_id_for_thread.clone(),
                    }),
                });
            }
        }

        // Persist the user message before the turn starts so a crash
        // doesn't silently drop it.
        app_state.push_message(MessageRole::User, message_for_thread.clone());
        let _ = inputs.session_store.append_event(
            session_uuid,
            TranscriptEvent::UserMessage {
                text: message_for_thread.clone(),
            },
        );
        let _ = &session_id_for_thread; // keep the String alive for logging

        let ev_state = setup_state.clone();
        let ev_channel = channel_thread.clone();
        let ev_turn = turn_id_thread.clone();
        let on_event = move |event: TurnStreamEvent| {
            let payload = match event {
                TurnStreamEvent::TextDelta(delta) => {
                    json!({"type": "text-delta", "turnId": ev_turn, "delta": delta})
                }
                TurnStreamEvent::ThinkingDelta(delta) => {
                    json!({"type": "thinking-delta", "turnId": ev_turn, "delta": delta})
                }
                TurnStreamEvent::ToolCallsRequested(reqs) => json!({
                    "type": "tool-calls-requested",
                    "turnId": ev_turn,
                    "requests": reqs.iter().map(|r| json!({
                        "callId": r.call_id,
                        "toolId": r.tool_id,
                        "input": r.input,
                    })).collect::<Vec<_>>(),
                }),
                TurnStreamEvent::ToolInvocations(invs) => json!({
                    "type": "tool-invocations",
                    "turnId": ev_turn,
                    "invocations": invs.iter().map(|i| json!({
                        "callId": i.call_id,
                        "toolId": i.tool_id,
                        "input": i.input,
                        "output": i.output,
                        "success": i.success,
                    })).collect::<Vec<_>>(),
                }),
                TurnStreamEvent::Usage(r) => json!({
                    "type": "usage",
                    "turnId": ev_turn,
                    "report": {
                        "inputTokens": r.input_tokens,
                        "outputTokens": r.output_tokens,
                        "cacheReadTokens": r.cache_read_tokens,
                        "cacheCreationTokens": r.cache_creation_tokens,
                    }
                }),
                TurnStreamEvent::ReflectionCheckpoint(s) => json!({
                    "type": "reflection-checkpoint",
                    "turnId": ev_turn,
                    "summary": s,
                }),
                TurnStreamEvent::ReflectionTrace(_) => return,
                TurnStreamEvent::RetryAttempt {
                    attempt,
                    max_attempts,
                    error,
                } => json!({
                    "type": "retry-attempt",
                    "turnId": ev_turn,
                    "attempt": attempt,
                    "maxAttempts": max_attempts,
                    "error": error,
                }),
            };
            ev_state.publish_event(ServerEnvelope::Event {
                event: ev_channel.clone(),
                payload,
            });
        };

        let perm_state = setup_state.clone();
        let perm_channel = channel_thread.clone();
        let perm_turn = turn_id_thread.clone();
        let perm_pending = pending.clone();
        let on_permission = move |req: PermissionPromptRequest| -> PermissionPromptAction {
            let request_id = next_req_id.fetch_add(1, Ordering::SeqCst).to_string();
            let (tx, rx) = std::sync::mpsc::channel();
            perm_pending.lock().unwrap().insert(request_id.clone(), tx);

            perm_state.publish_event(ServerEnvelope::Event {
                event: perm_channel.clone(),
                payload: json!({
                    "type": "permission-request",
                    "turnId": perm_turn,
                    "requestId": request_id,
                    "toolId": req.tool_id,
                    "summary": req.summary,
                    "reason": req.reason,
                }),
            });

            rx.recv().unwrap_or(PermissionPromptAction::Deny)
        };

        let question_state = setup_state.clone();
        let question_channel = channel_thread.clone();
        let question_turn = turn_id_thread.clone();
        let question_pending = pending_questions.clone();
        let question_next_id = setup_state.next_request_id.clone();
        let on_user_question = move |req: UserQuestionPromptRequest| -> UserQuestionPromptResponse {
            let request_id = question_next_id.fetch_add(1, Ordering::SeqCst).to_string();
            let (tx, rx) = std::sync::mpsc::channel();
            question_pending
                .lock()
                .unwrap()
                .insert(request_id.clone(), tx);

            question_state.publish_event(ServerEnvelope::Event {
                event: question_channel.clone(),
                payload: json!({
                    "type": "user-question-request",
                    "turnId": question_turn.clone(),
                    "requestId": request_id,
                    "questions": req.questions,
                }),
            });

            rx.recv().unwrap_or(UserQuestionPromptResponse {
                answers: serde_json::Map::new(),
                annotations: serde_json::Map::new(),
            })
        };

        let mut auth_store = inputs.auth_store.clone();
        let outcome = with_user_question_prompt_handler(on_user_question, || {
            execute_user_turn_streaming_with_permissions_and_cancel(
                &mut app_state,
                &inputs.resources,
                &inputs.providers,
                &mut auth_store,
                &message_for_thread,
                None,
                &cancel,
                on_event,
                on_permission,
            )
        });

        match outcome {
            Ok(turn) => {
                for inv in &turn.tool_invocations {
                    let _ = inputs.session_store.append_event(
                        session_uuid,
                        TranscriptEvent::ToolInvocation {
                            call_id: inv.call_id.clone(),
                            tool_id: inv.tool_id.clone(),
                            input: inv.input.clone(),
                            output: inv.output.clone(),
                            success: inv.success,
                        },
                    );
                }
                if !turn.assistant_text.is_empty() {
                    app_state.push_message(MessageRole::Assistant, turn.assistant_text.clone());
                    let _ = inputs.session_store.append_event(
                        session_uuid,
                        TranscriptEvent::AssistantMessage {
                            text: turn.assistant_text.clone(),
                        },
                    );
                }
                for trace in &turn.reflection_traces {
                    let _ = inputs.session_store.append_trace_event(
                        session_uuid,
                        puffer_session_store::TRACE_RUNTIME,
                        trace,
                    );
                }
                setup_state.publish_event(ServerEnvelope::Event {
                    event: channel_thread.clone(),
                    payload: json!({
                        "type": "turn-complete",
                        "turnId": turn_id_thread,
                        "assistantText": turn.assistant_text,
                    }),
                });
                // Transcript mutated — let boards / sidebars re-render.
                setup_state.publish_event(ServerEnvelope::Event {
                    event: "workspace:sessions:changed".to_string(),
                    payload: json!({
                        "reason": "turn_complete",
                        "sessionId": session_id_for_thread.clone(),
                    }),
                });
            }
            Err(err) => {
                eprintln!("turn {turn_id_thread} failed: {err:#}");
                let (friendly, category) = classify_turn_error(&err);
                setup_state.publish_event(ServerEnvelope::Event {
                    event: channel_thread.clone(),
                    payload: json!({
                        "type": "turn-error",
                        "turnId": turn_id_thread,
                        "error": friendly,
                        "errorRaw": format!("{err:#}"),
                        "category": category,
                    }),
                });
            }
        }

        state_for_thread
            .turns
            .lock()
            .unwrap()
            .remove(&turn_id_thread);
    });

    Ok(json!({"turnId": turn_id_resp}))
}

/// Translates a raw `anyhow::Error` from `execute_user_turn_streaming_*`
/// into a `(friendly-message, category)` pair for the SSE `turn-error`
/// event. Frontends switch on `category` to surface UX-friendly affordances
/// (e.g. an automatic "retry" button for transient runner-unreachable
/// failures), and the raw error stays available as `errorRaw` for debug.
///
/// Categories we currently distinguish:
///   * `runner_unreachable` — gRPC channel to `remote_runner` returned
///     `tcp connect error` / `Connection refused`. On agentenv this is the
///     wake-after-sleep stale-`host_port` bug (issue agentenv/monorepo#401):
///     `config.toml`'s `remote_runner.endpoint` carries the host_port from
///     a prior session, but the hypervisor allocates a fresh port on
///     restore — the daemon then dials the old port and gets refused. The
///     bug is fixed in api-server (refresh sandbox metadata before passing
///     `runnerEndpoint` to `wakePuffer`); this categorisation is the
///     puffer-side belt-and-suspenders so the user-visible failure mode is
///     "the sandbox is still warming up, retry in a moment" instead of a
///     cryptic transport-level message.
///   * `cancelled` — agent loop bailed because `CancelToken::cancel`
///     fired. Mirrors the bail string `"cancelled"`.
///   * `other` — anything we haven't classified.
fn classify_turn_error(err: &anyhow::Error) -> (String, &'static str) {
    // anyhow walks the chain via Display when you `format!("{:#}", err)`
    let chain = format!("{err:#}");
    let lower = chain.to_ascii_lowercase();
    if lower.contains("tcp connect error")
        || lower.contains("connection refused")
        || lower.contains("connect: connection refused")
    {
        return (
            "the tool runner is not reachable yet — this can happen when the sandbox \
             just resumed from sleep and its runtime port has not been re-mapped. \
             retry in a few seconds."
                .to_string(),
            "runner_unreachable",
        );
    }
    if lower == "cancelled" || lower.contains(": cancelled") {
        return (chain, "cancelled");
    }
    (chain, "other")
}

fn apply_turn_model_override(
    app_state: &mut AppState,
    providers: &ProviderRegistry,
    requested: &str,
) -> Result<()> {
    let (provider_id, model_id) = if let Some((provider_id, model_id)) = requested.split_once('/') {
        let provider = providers
            .provider(provider_id)
            .with_context(|| format!("provider {provider_id} not found"))?;
        let model = provider
            .models
            .iter()
            .find(|model| model.id == model_id)
            .with_context(|| format!("model {requested} not found"))?;
        (provider.id.clone(), model.id.clone())
    } else {
        let (provider, model) = providers
            .providers()
            .find_map(|provider| {
                provider
                    .models
                    .iter()
                    .find(|model| model.id == requested)
                    .map(|model| (provider, model))
            })
            .with_context(|| format!("model {requested} not found"))?;
        (provider.id.clone(), model.id.clone())
    };

    let effort = app_state.effort_level.clone();
    let fast_mode = app_state.fast_mode;
    apply_model_preferences(app_state, &provider_id, &model_id, &effort, fast_mode)
}

fn apply_daemon_yolo_mode(app_state: &mut AppState) {
    app_state.sandbox_mode = "danger-full-access".to_string();
    app_state.set_session_allow_all();
}

#[cfg(test)]
mod tests {
    use super::{
        apply_daemon_yolo_mode, apply_turn_model_override, handle_create_session, DaemonState,
    };
    use indexmap::IndexMap;
    use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
    use puffer_core::AppState;
    use puffer_provider_registry::{
        Modality, ModelDescriptor, ProviderDescriptor, ProviderRegistry,
    };
    use puffer_session_store::{SessionMetadata, SessionStore};
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn create_session_accepts_display_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(
            workspace_root.clone(),
            paths.clone(),
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");

        let response = handle_create_session(
            &state,
            &json!({
                "cwd": workspace_root.display().to_string(),
                "displayName": "  Managed Agent  ",
            }),
        )
        .expect("create session");

        let session_id = response["sessionId"].as_str().expect("sessionId");
        assert_eq!(response["displayName"], "Managed Agent");
        assert_eq!(response["generatedTitle"], serde_json::Value::Null);

        let store = SessionStore::from_paths(&paths).expect("session store");
        let session_id = uuid::Uuid::parse_str(session_id).expect("valid session id");
        let session = store.load_session(session_id).expect("stored session");
        assert_eq!(
            session.metadata.display_name.as_deref(),
            Some("Managed Agent")
        );
        assert_eq!(session.metadata.generated_title, None);
    }

    #[test]
    fn turn_model_override_selects_matching_provider_model() {
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata = SessionMetadata {
            id: Uuid::new_v4(),
            display_name: None,
            generated_title: None,
            cwd: temp.path().to_path_buf(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        let mut state = AppState::new(PufferConfig::default(), temp.path().to_path_buf(), metadata);
        state.effort_level = "off".to_string();
        state.fast_mode = true;

        let mut providers = ProviderRegistry::new();
        providers.register(provider("openai", &["gpt-5.4"]));
        providers.register(provider("anthropic", &["claude-sonnet-4-5"]));

        apply_turn_model_override(&mut state, &providers, "anthropic/claude-sonnet-4-5")
            .expect("override");

        assert_eq!(state.current_provider.as_deref(), Some("anthropic"));
        assert_eq!(
            state.current_model.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
        assert_eq!(
            state.config.default_model.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
        assert_eq!(state.effort_level, "off");
        assert!(state.fast_mode);
    }

    #[test]
    fn classify_turn_error_distinguishes_runner_unreachable() {
        use super::classify_turn_error;

        // Real shape from a sleeping puffer agent's first tool call after
        // wake (production trace, agentenv issue #401): the
        // RemoteToolRunner returns `tonic::Code::Unavailable` because the
        // host_port `config.toml` carried is stale, surfacing as the wrapped
        // error string `transport error: tcp connect error`.
        let err = anyhow::anyhow!("transport error: tcp connect error: Connection refused");
        let (msg, cat) = classify_turn_error(&err);
        assert_eq!(cat, "runner_unreachable", "{msg}");
        assert!(msg.contains("retry"), "{msg}");

        // Cancel path keeps the lowercase canonical bail string so PR #109
        // and downstream consumers that key on `cancelled` still work.
        let err = anyhow::anyhow!("cancelled");
        let (msg, cat) = classify_turn_error(&err);
        assert_eq!(cat, "cancelled");
        assert_eq!(msg, "cancelled");

        // Anything else falls through with the raw chain preserved.
        let err = anyhow::anyhow!("HTTP 503 from upstream");
        let (msg, cat) = classify_turn_error(&err);
        assert_eq!(cat, "other");
        assert!(msg.contains("HTTP 503"));
    }

    #[test]
    fn daemon_yolo_mode_sets_allow_all_and_danger_full_access() {
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata = SessionMetadata {
            id: Uuid::new_v4(),
            display_name: None,
            generated_title: None,
            cwd: temp.path().to_path_buf(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        let mut state = AppState::new(PufferConfig::default(), temp.path().to_path_buf(), metadata);

        apply_daemon_yolo_mode(&mut state);

        assert_eq!(state.sandbox_mode, "danger-full-access");
        assert!(state.session_allow_all);
    }

    fn provider(id: &str, models: &[&str]) -> ProviderDescriptor {
        ProviderDescriptor {
            id: id.to_string(),
            display_name: id.to_string(),
            base_url: "https://example.invalid".to_string(),
            chat_completions_path: None,
            default_api: "openai-responses".to_string(),
            auth_modes: Vec::new(),
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            discovery: None,
            models: models
                .iter()
                .map(|model| ModelDescriptor {
                    id: (*model).to_string(),
                    display_name: (*model).to_string(),
                    provider: id.to_string(),
                    api: "openai-responses".to_string(),
                    context_window: 128_000,
                    max_output_tokens: 16_384,
                    supports_reasoning: true,
                    input: vec![Modality::Text],
                    cost: None,
                    compat: None,
                })
                .collect(),
        }
    }
}
