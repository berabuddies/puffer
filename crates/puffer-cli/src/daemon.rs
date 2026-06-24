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
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path as AxumPath, Query, State,
    },
    http::{header, HeaderMap, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use indexmap::IndexMap;
use puffer_config::{
    ensure_workspace_dirs, load_config, save_user_config, ConfigPaths, MediaConfig,
    MediaGenerationConfig, ProxyConfig, ProxyEndpoint, ProxyScheme, PufferConfig,
};
use puffer_core::{
    command_surface, default_effort_level, dispatch_command, enter_plan_mode, execute_connect_flow,
    execute_user_turn_streaming_with_prompt_tools_and_cancel, provider_preference_family,
    supported_effort_levels, with_user_question_prompt_handler, AppState,
    BrowserPermissionPromptActionSet, BrowserPermissionPromptSource,
    BrowserPermissionPromptTargetClass, CancelToken, MessageRole, ModelPreferenceFamily,
    PermissionPromptAction, PermissionPromptRequest, ToolCallRequest, ToolInvocation,
    TurnStreamEvent, UserQuestionPromptRequest, UserQuestionPromptResponse,
};
use puffer_media::{
    discover_exact_media_capabilities, generate_exact_media_with_cache,
    generated_video_access_metadata_by_artifact, list_exact_media_capabilities_with_cache,
    read_generated_media_preview_by_artifact, ExactMediaDiscoveryCache,
    ExactMediaGenerationRequest, GeneratedVideoAccessMetadata, GeneratedVideoAccessMetadataResult,
    MediaCapabilityView,
};
use puffer_provider_openai::{
    build_realtime_client_secret_request,
    parse_authorization_input as parse_openai_authorization_input, BuiltOpenAIRequest, OpenAIAuth,
    OpenAIRealtimeClientSecretRequest, OpenAIRequestConfig,
};
use puffer_provider_registry::{
    AuthStore, ModelDescriptor, ProviderDescriptor, ProviderRegistry, StoredCredential,
};
use puffer_resources::{load_resources, LoadedResources, McpServerSpec};
use puffer_session_store::{
    MessageActor, SessionMetadata, SessionStore, StoredAttachment, StoredAttachmentKind,
    TranscriptEvent, TurnBoundaryState,
};
use puffer_transport_anthropic::{
    parse_authorization_input as parse_anthropic_authorization_input, ANTHROPIC_API_BASE_URL,
    ANTHROPIC_MANUAL_REDIRECT_URL,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{BufReader, Read};
use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::{broadcast, Mutex as AsyncMutex};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::auth_credentials::{
    inferred_anthropic_redirect_uri, set_stored_credential, store_anthropic_credential,
    to_registry_oauth_credential_openai,
};
use crate::auth_provider::{
    oauth_family_for_provider, oauth_login_bundle_for_provider, OauthFamily,
};
use crate::daemon_browser::{BrowserLaunchSettings, BrowserRegistry};
use crate::daemon_fs_watch::FsWatchRegistry;
use crate::daemon_lambda_skills::{
    handle_list_lambda_skill_libraries, handle_remove_lambda_skill_library,
    handle_save_lambda_skill_library, handle_set_lambda_skill_approval,
    handle_set_lambda_skill_enabled,
};
use crate::daemon_pty::PtyRegistry;
use crate::daemon_turn_recovery::{
    recovery_actor, stale_turn_recovery_decision, StaleTurnRecoveryDecision,
    DEFAULT_STALE_TURN_RETRY_AFTER_MS,
};
use crate::daemon_turn_routing::persist_explicit_turn_routing;
use crate::daemon_ui_state::{
    load_file_tabs_state, load_pin_state, load_session_routing_state, set_file_tabs_state,
    set_pin_state, set_session_routing_state, DesktopFileTab, DesktopFileTabsState,
    DesktopPinState, DesktopSessionRouting,
};
use crate::desktop_api;
use crate::desktop_api_types::{
    ExternalCredentialDto, FolderGroupDto, McpServerDto, MediaCapabilityAxisDto,
    MediaCapabilityInfoDto, ModelDescriptorDto, ProxyEndpointInputDto, ProxyTestResultDto,
    RepoActionResultDto, RepoStatusDto, SaveProxySettingsParams, SessionDetailDto,
    SettingsSnapshotDto, ThinkingOptionDto,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateMediaParams {
    kind: String,
    prompt: String,
    #[serde(default)]
    count: Option<u8>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateMediaArtifactResult {
    artifact_id: String,
    index: usize,
    path: String,
    mime_type: String,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_source_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateMediaResult {
    job_id: String,
    requested_count: u8,
    artifacts: Vec<GenerateMediaArtifactResult>,
    kind: String,
    provider_id: String,
    model_id: String,
    status: String,
    prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedMediaPreviewParams {
    session_id: String,
    artifact_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedVideoAccessParams {
    session_id: String,
    artifact_id: String,
}

#[derive(Debug, Clone)]
struct GeneratedVideoTicket {
    path: std::path::PathBuf,
    mime_type: String,
    size: u64,
    expires_at_ms: u64,
}

pub(crate) struct DaemonOptions {
    pub host_guard: crate::daemon_singleton::DaemonHostGuard,
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
    let DaemonOptions {
        host_guard,
        bind,
        handshake_file,
        token,
        print_handshake,
        no_browser,
        system_prompt_1,
        disable_auto_title,
        yolo,
    } = options;
    let daemon_host_guard = host_guard;
    let cwd = std::env::current_dir()?;
    let paths = ConfigPaths::discover(&cwd);
    ensure_workspace_dirs(&paths)?;

    let token = token.unwrap_or_else(|| load_or_generate_token(&paths));

    let state = DaemonState::load(
        cwd.clone(),
        paths.clone(),
        token.clone(),
        no_browser,
        disable_auto_title,
        yolo,
    )?;
    if let Some(prompt) = system_prompt_1
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        std::env::set_var("PUFFER_SYSTEM_PROMPT_1", prompt);
    }
    if no_browser {
        std::env::set_var("PUFFER_NO_BROWSER", "1");
    } else {
        std::env::remove_var("PUFFER_NO_BROWSER");
    }
    let state = Arc::new(state);

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route(
            "/media/generated-video/:ticket",
            get(generated_video_handler),
        )
        .with_state(state.clone());

    let addr: SocketAddr = bind.parse().context("bind address")?;
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
    let handshake_path = handshake_file
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| paths.user_config_dir.join("daemon.handshake"));
    if let Some(parent) = handshake_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let handshake_json = serde_json::to_string(&handshake)?;
    std::fs::write(&handshake_path, &handshake_json)?;

    if print_handshake {
        // One-line NDJSON so Tauri (or shell scripts) can `read` a line,
        // parse, and act. We continue to serve after printing.
        println!("{handshake_json}");
    }

    eprintln!(
        "puffer daemon listening on {url} (workspace: {})",
        paths.workspace_root.display()
    );

    let serve_result = axum::serve(listener, app).await;
    drop(daemon_host_guard);
    serve_result?;
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

fn daemon_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

const GENERATED_VIDEO_TICKET_TTL_MS: u64 = 10 * 60 * 1000;

fn generated_video_ticket_path(ticket: &str) -> String {
    format!("/media/generated-video/{ticket}")
}

/// Map a workspace file path to a `<video>`-playable MIME type by extension,
/// or `None` for anything we don't preview. `.ogg` is excluded on purpose: it
/// is commonly an audio container and audio is out of scope.
fn file_media_mime_type(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mov" => "video/quicktime",
        _ => return None,
    })
}

fn random_ticket() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    hex(&buf)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeneratedVideoRangeError {
    Invalid,
    Unsatisfiable,
}

fn parse_single_byte_range(
    header: &str,
    size: u64,
) -> std::result::Result<Option<(u64, u64)>, GeneratedVideoRangeError> {
    let header = header.trim();
    if header.is_empty() {
        return Ok(None);
    }
    let Some(range) = header.strip_prefix("bytes=") else {
        return Err(GeneratedVideoRangeError::Invalid);
    };
    if range.contains(',') || size == 0 {
        return Err(GeneratedVideoRangeError::Unsatisfiable);
    }
    let Some((start, end)) = range.split_once('-') else {
        return Err(GeneratedVideoRangeError::Invalid);
    };
    if start.is_empty() {
        let suffix_len = end
            .parse::<u64>()
            .map_err(|_| GeneratedVideoRangeError::Invalid)?;
        if suffix_len == 0 {
            return Err(GeneratedVideoRangeError::Unsatisfiable);
        }
        let start = size.saturating_sub(suffix_len);
        return Ok(Some((start, size - 1)));
    }
    let start = start
        .parse::<u64>()
        .map_err(|_| GeneratedVideoRangeError::Invalid)?;
    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>()
            .map_err(|_| GeneratedVideoRangeError::Invalid)?
    };
    if start >= size || end < start {
        return Err(GeneratedVideoRangeError::Unsatisfiable);
    }
    Ok(Some((start, end.min(size - 1))))
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
    /// Local model setup/status jobs used by the desktop MiniCPM card.
    pub(crate) local_models: crate::daemon_local_model::LocalModelInstaller,
    disable_auto_title: bool,
    yolo: bool,
    media_discovery_cache: Arc<Mutex<Option<ExactMediaDiscoveryCache>>>,
    generated_video_tickets: Arc<Mutex<HashMap<String, GeneratedVideoTicket>>>,
    /// Transcript replay buffer — a bounded ring of recent session / clone /
    /// workspace events. On a fresh WebSocket connection we replay these
    /// so UIs that disconnected mid-turn don't miss deltas. Size capped at
    /// `RECENT_EVENT_CAPACITY` (oldest evicted) to keep memory bounded.
    recent_events: Arc<Mutex<VecDeque<ServerEnvelope>>>,
    /// Number of live WebSocket clients. When this drops to zero the
    /// disconnect watchdog cancels orphaned interactive turns after a grace
    /// window so a runaway browser checkout can't keep running once the user's
    /// app has gone away (issue #600).
    live_connections: Arc<AtomicUsize>,
    /// Monitor reply action sessions stay source-bound across follow-up turns.
    monitor_reply_sessions: Arc<Mutex<HashMap<String, String>>>,
}

const RECENT_EVENT_CAPACITY: usize = 500;

/// Grace window after the last WebSocket client disconnects before orphaned
/// interactive turns are cancelled. Long enough to ride out an app reload /
/// transient network blip (the UI reconnects and replays the ring buffer),
/// short enough to stop a runaway checkout promptly (issue #600).
const DISCONNECT_CANCEL_GRACE: Duration = Duration::from_secs(15);

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

    /// Returns a clone of the currently loaded daemon config.
    pub(crate) fn config_snapshot(&self) -> PufferConfig {
        self.config.lock().unwrap().clone()
    }

    /// Replaces the currently loaded daemon config.
    pub(crate) fn replace_config(&self, config: PufferConfig) {
        *self.config.lock().unwrap() = config;
    }

    /// Builds the current desktop settings snapshot as a JSON value.
    pub(crate) fn settings_snapshot_value(&self) -> Result<Value> {
        let inputs = self.build_runtime_inputs()?;
        let config = self.config_snapshot();
        let snapshot: SettingsSnapshotDto = desktop_api::load_settings_snapshot(
            &self.paths,
            &config,
            &inputs.resources,
            &inputs.providers,
            &inputs.auth_store,
            &inputs.session_store,
        )?;
        Ok(serde_json::to_value(snapshot)?)
    }

    fn prune_expired_generated_video_tickets(&self, now_ms: u64) {
        self.generated_video_tickets
            .lock()
            .unwrap()
            .retain(|_, ticket| ticket.expires_at_ms > now_ms);
    }

    fn insert_generated_video_ticket(
        &self,
        metadata: GeneratedVideoAccessMetadata,
    ) -> (String, GeneratedVideoTicket) {
        let now_ms = daemon_now_ms();
        self.prune_expired_generated_video_tickets(now_ms);
        let token = random_ticket();
        let ticket = GeneratedVideoTicket {
            path: metadata.path,
            mime_type: metadata.mime_type,
            size: metadata.byte_count,
            expires_at_ms: now_ms + GENERATED_VIDEO_TICKET_TTL_MS,
        };
        self.generated_video_tickets
            .lock()
            .unwrap()
            .insert(token.clone(), ticket.clone());
        (token, ticket)
    }

    fn generated_video_ticket(&self, token: &str) -> Option<GeneratedVideoTicket> {
        let now_ms = daemon_now_ms();
        self.prune_expired_generated_video_tickets(now_ms);
        self.generated_video_tickets
            .lock()
            .unwrap()
            .get(token)
            .filter(|ticket| ticket.expires_at_ms > now_ms)
            .cloned()
    }
}

#[derive(Clone)]
struct TurnHandle {
    session_id: Option<String>,
    session_uuid: Option<Uuid>,
    channel: String,
    message: String,
    attachments: Vec<StoredAttachment>,
    cancel: CancelToken,
    cancel_reported: Arc<AtomicBool>,
    user_prompt_persisted: Arc<AtomicBool>,
    pending: Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<PermissionPromptAction>>>>,
    pending_questions:
        Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<UserQuestionPromptResponse>>>>,
    progress: Arc<Mutex<TurnProgress>>,
}

#[derive(Default)]
struct TurnProgress {
    assistant_text: String,
    assistant_text_retry_checkpoint: usize,
    tool_invocations: Vec<ToolInvocation>,
    tool_invocations_retry_checkpoint: usize,
    ordered_items: Vec<TurnProgressItem>,
    ordered_items_retry_checkpoint: usize,
    pending_tool_calls: Vec<ToolCallRequest>,
    persisted_on_cancel: bool,
}

enum TurnProgressItem {
    AssistantText(String),
    PendingToolCall(ToolCallRequest),
    ToolInvocation(ToolInvocation),
}

impl TurnProgress {
    fn record_text_delta(&mut self, delta: &str) {
        self.assistant_text.push_str(delta);
        match self.ordered_items.last_mut() {
            Some(TurnProgressItem::AssistantText(text)) => text.push_str(delta),
            _ => self
                .ordered_items
                .push(TurnProgressItem::AssistantText(delta.to_string())),
        }
    }

    fn record_tool_invocations(&mut self, invocations: Vec<ToolInvocation>) {
        for invocation in invocations {
            self.tool_invocations.push(invocation.clone());
            let pending_index = self
                .pending_tool_calls
                .iter()
                .position(|request| request.call_id == invocation.call_id);
            if let Some(index) = pending_index {
                self.pending_tool_calls.remove(index);
            }
            if let Some(text) = send_user_message_text(&invocation.tool_id, &invocation.input) {
                if let Some(index) = self.pending_tool_item_index(&invocation.call_id) {
                    self.ordered_items[index] = TurnProgressItem::AssistantText(text);
                } else {
                    self.push_ordered_assistant_text(text);
                }
                continue;
            }
            let ordered_index = self.ordered_items.iter().position(|item| {
                matches!(
                    item,
                    TurnProgressItem::PendingToolCall(request)
                        if request.call_id == invocation.call_id
                )
            });
            if let Some(index) = ordered_index {
                self.ordered_items[index] = TurnProgressItem::ToolInvocation(invocation);
            } else {
                self.ordered_items
                    .push(TurnProgressItem::ToolInvocation(invocation));
            }
        }
    }

    fn record_tool_call_requests(&mut self, requests: Vec<ToolCallRequest>) {
        for request in requests {
            self.pending_tool_calls.push(request.clone());
            self.ordered_items
                .push(TurnProgressItem::PendingToolCall(request));
        }
    }

    fn pending_tool_item_index(&self, call_id: &str) -> Option<usize> {
        self.ordered_items.iter().position(|item| {
            matches!(
                item,
                TurnProgressItem::PendingToolCall(request) if request.call_id == call_id
            )
        })
    }

    fn push_ordered_assistant_text(&mut self, text: String) {
        match self.ordered_items.last_mut() {
            Some(TurnProgressItem::AssistantText(existing)) => existing.push_str(&text),
            _ => self
                .ordered_items
                .push(TurnProgressItem::AssistantText(text)),
        }
    }

    fn checkpoint_stream_attempt(&mut self) {
        self.assistant_text_retry_checkpoint = self.assistant_text.len();
        self.tool_invocations_retry_checkpoint = self.tool_invocations.len();
        self.ordered_items_retry_checkpoint = self.ordered_items.len();
    }

    fn reset_to_retry_checkpoint(&mut self) {
        let checkpoint = self
            .assistant_text_retry_checkpoint
            .min(self.assistant_text.len());
        self.assistant_text.truncate(checkpoint);
        let tool_checkpoint = self
            .tool_invocations_retry_checkpoint
            .min(self.tool_invocations.len());
        self.tool_invocations.truncate(tool_checkpoint);
        let ordered_checkpoint = self
            .ordered_items_retry_checkpoint
            .min(self.ordered_items.len());
        self.ordered_items.truncate(ordered_checkpoint);
        self.pending_tool_calls.clear();
    }
}

fn send_user_message_text(tool_id: &str, input: &str) -> Option<String> {
    if !matches!(tool_id, "SendUserMessage" | "Brief") {
        return None;
    }
    let parsed: Value = serde_json::from_str(input).ok()?;
    let message = parsed.get("message")?.as_str()?.to_string();
    if message.trim().is_empty() {
        return None;
    }
    Some(message)
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
        let browser_launch_settings = BrowserLaunchSettings::from_config(&paths, &config)?;
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
            browsers: Arc::new(BrowserRegistry::new(
                browser_profile_root,
                !no_browser,
                browser_launch_settings,
            )),
            local_models: crate::daemon_local_model::LocalModelInstaller::new(),
            disable_auto_title,
            yolo,
            media_discovery_cache: Arc::new(Mutex::new(None)),
            generated_video_tickets: Arc::new(Mutex::new(HashMap::new())),
            recent_events: Arc::new(Mutex::new(VecDeque::with_capacity(RECENT_EVENT_CAPACITY))),
            live_connections: Arc::new(AtomicUsize::new(0)),
            monitor_reply_sessions: Arc::new(Mutex::new(HashMap::new())),
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
        || event.starts_with("contacts:infer:")
        || event.starts_with("connector-setup:")
}

impl DaemonState {
    fn build_runtime_inputs(&self) -> Result<RuntimeInputs> {
        self.build_runtime_inputs_with_discovery(true)
    }

    /// Builds runtime inputs from local cache without refreshing model discovery.
    pub(crate) fn build_runtime_inputs_without_discovery(&self) -> Result<RuntimeInputs> {
        self.build_runtime_inputs_with_discovery(false)
    }

    fn build_runtime_inputs_with_discovery(&self, discover_all: bool) -> Result<RuntimeInputs> {
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
        if let Some(display_name) = config.openai_display_name.as_deref() {
            providers.set_openai_display_name(display_name);
        }
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
        let stale_discovery_provider_ids = if discover_all {
            if let Ok(client) = proxy_discovery_client(&config) {
                let _ =
                    providers.discover_and_merge_all_with_discovery_client(&auth_store, &client);
            } else {
                let _ = providers.discover_and_merge_all(&auth_store);
            }
            Vec::new()
        } else {
            // Even on the fast path we apply the on-disk discovery cache
            // — that's a synchronous file read, no network — so callers
            // like `create_session` can resolve previously-discovered
            // model names without paying for a fresh network round-trip.
            providers.apply_discovery_cache()
        };
        let session_store = SessionStore::from_paths(&self.paths)?;
        Ok(RuntimeInputs {
            resources,
            providers,
            auth_store,
            session_store,
            stale_discovery_provider_ids,
        })
    }

    fn exact_media_discovery_cache(&self, inputs: &RuntimeInputs) -> ExactMediaDiscoveryCache {
        let now_ms = daemon_now_ms();
        if let Some(cache) = self
            .media_discovery_cache
            .lock()
            .unwrap()
            .as_ref()
            .filter(|cache| cache.is_fresh_at(now_ms))
            .cloned()
        {
            return cache;
        }
        let cache = discover_exact_media_capabilities(&inputs.providers, &inputs.auth_store);
        *self.media_discovery_cache.lock().unwrap() = Some(cache.clone());
        cache
    }
}

/// Runtime dependencies loaded from config, resources, auth, and session stores.
pub(crate) struct RuntimeInputs {
    pub(crate) resources: LoadedResources,
    pub(crate) providers: ProviderRegistry,
    pub(crate) auth_store: AuthStore,
    pub(crate) session_store: SessionStore,
    pub(crate) stale_discovery_provider_ids: Vec<String>,
}

fn proxy_discovery_client(
    config: &PufferConfig,
) -> Result<puffer_provider_registry::ModelDiscoveryClient> {
    let client = puffer_core::blocking_client_for_url(
        &config.network.proxy,
        puffer_core::HttpPurpose::Discovery,
        "https://api.openai.com/v1/models",
        Duration::from_secs(8),
    )?;
    Ok(puffer_provider_registry::ModelDiscoveryClient::with_client(
        client,
    ))
}

fn proxy_connectivity_test_urls() -> &'static [&'static str] {
    &[
        "https://www.gstatic.com/generate_204",
        "https://cp.cloudflare.com/generate_204",
        "https://www.cloudflare.com/cdn-cgi/trace",
    ]
}

fn test_proxy_connectivity(
    endpoint: &ProxyEndpoint,
    timeout: Duration,
) -> Result<(&'static str, puffer_core::ProxyTestOutcome)> {
    let mut last_error = None;
    for target_url in proxy_connectivity_test_urls() {
        match puffer_core::test_proxy_endpoint(endpoint, target_url, timeout) {
            Ok(outcome) => return Ok((target_url, outcome)),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no proxy connectivity test URLs configured")))
}

fn desktop_latency_ms(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn parse_proxy_scheme(value: &str) -> Result<ProxyScheme> {
    match value.trim().to_ascii_lowercase().as_str() {
        "http" => Ok(ProxyScheme::Http),
        "https" => Ok(ProxyScheme::Https),
        "socks5" => Ok(ProxyScheme::Socks5),
        "socks5h" => Ok(ProxyScheme::Socks5h),
        other => anyhow::bail!("unknown proxy scheme `{other}`"),
    }
}

fn proxy_endpoint_from_input(
    input: ProxyEndpointInputDto,
    current_config: &PufferConfig,
) -> Result<ProxyEndpoint> {
    let existing_password = current_config
        .network
        .proxy
        .proxies
        .iter()
        .find(|endpoint| endpoint.id == input.id)
        .and_then(|endpoint| endpoint.password.clone());
    let password = input
        .password
        .or_else(|| input.keep_password.then_some(existing_password).flatten());
    Ok(ProxyEndpoint {
        id: input.id,
        scheme: parse_proxy_scheme(&input.scheme)?,
        host: input.host,
        port: input.port,
        username: input.username,
        password,
    })
}

fn proxy_test_error_message(endpoint: &ProxyEndpoint, error: &anyhow::Error) -> String {
    let mut message = error.to_string();
    if let Some(password) = endpoint
        .password
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        message = message.replace(password, "******");
    }
    let mut chars = message.chars();
    let truncated = chars.by_ref().take(240).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestProxyParams {
    proxy_id: Option<String>,
    endpoint: Option<ProxyEndpointInputDto>,
}

fn proxy_oauth_client(config: &PufferConfig, url: &str) -> Result<reqwest::blocking::Client> {
    puffer_core::blocking_client_for_url(
        &config.network.proxy,
        puffer_core::HttpPurpose::OAuth,
        url,
        Duration::from_secs(60),
    )
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

// Streams a ticketed media file with HTTP Range support. Tickets are minted by
// `create_generated_video_access` (chat-generated videos) AND
// `create_file_media_access` (arbitrary in-workspace video files for the Files
// pane); both validate the path before inserting, so a ticket never escapes the
// workspace. The route name is historical — kept to avoid churning a tested path.
async fn generated_video_handler(
    State(state): State<Arc<DaemonState>>,
    AxumPath(ticket): AxumPath<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(ticket) = state.generated_video_ticket(&ticket) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let file = match tokio::fs::File::open(&ticket.path).await {
        Ok(file) => file,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let size = match file.metadata().await {
        Ok(metadata) => metadata.len(),
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let range_header = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let range = match parse_single_byte_range(range_header, size) {
        Ok(range) => range,
        Err(GeneratedVideoRangeError::Unsatisfiable) => {
            return StatusCode::RANGE_NOT_SATISFIABLE.into_response();
        }
        Err(GeneratedVideoRangeError::Invalid) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, ticket.mime_type)
        .header(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    let (body, content_length) = if let Some((start, end)) = range {
        builder = builder
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
        let mut file = file;
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return StatusCode::NOT_FOUND.into_response();
        }
        let length = end - start + 1;
        (
            Body::from_stream(ReaderStream::new(file.take(length))),
            length,
        )
    } else {
        builder = builder.status(StatusCode::OK);
        (Body::from_stream(ReaderStream::new(file)), size)
    };
    builder
        .header(header::CONTENT_LENGTH, content_length.to_string())
        .body(body)
        .unwrap()
        .into_response()
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

fn event_payload_with_actor(mut payload: Value, actor: &MessageActor) -> Value {
    if let Some(map) = payload.as_object_mut() {
        map.insert("actor".to_string(), json!(actor));
    }
    payload
}

fn lambda_gate_stream_payload(turn_id: &str, invocation: &ToolInvocation) -> Option<Value> {
    let lambda = invocation.metadata.get("lambda_skill")?;
    let event = lambda.get("event").and_then(Value::as_str)?;
    Some(json!({
        "type": "lambda-gate",
        "turnId": turn_id,
        "callId": &invocation.call_id,
        "toolId": &invocation.tool_id,
        "gateEvent": event,
        "hostTool": lambda.get("host_tool").and_then(Value::as_str),
        "hostArgs": lambda.get("host_args").cloned(),
        "concreteTool": lambda.get("concrete_tool").and_then(Value::as_str),
        "concreteInput": lambda.get("concrete_input").cloned(),
        "reason": lambda.get("reason").and_then(Value::as_str),
        "retryTool": lambda.get("retry_tool").and_then(Value::as_str),
        "recoverable": lambda.get("recoverable").and_then(Value::as_bool),
        "registeredFacts": lambda.get("registered_facts").cloned(),
    }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LambdaGateStreamPhase {
    BeforeToolInvocations,
    AfterToolInvocations,
}

fn lambda_gate_stream_phase(payload: &Value) -> LambdaGateStreamPhase {
    match payload.get("gateEvent").and_then(Value::as_str) {
        Some("host_call_admitted") => LambdaGateStreamPhase::BeforeToolInvocations,
        _ => LambdaGateStreamPhase::AfterToolInvocations,
    }
}

async fn handle_socket(socket: WebSocket, state: Arc<DaemonState>) {
    // Count this client for the lifetime of the connection; on drop (any exit
    // path) the watchdog is armed if it was the last one (issue #600).
    let _connection = ConnectionGuard::new(state.clone());
    let (ws_tx, mut ws_rx) = socket.split();
    let tx = Arc::new(AsyncMutex::new(ws_tx));
    let subscriptions = Arc::new(AsyncMutex::new(HashSet::<String>::new()));

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
    let event_subscriptions = subscriptions.clone();
    let event_forwarder = tokio::spawn(async move {
        while let Ok(env) = events_rx.recv().await {
            if !should_forward_live_event(&env, &event_subscriptions).await {
                continue;
            }
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
        dispatch_request(request, state.clone(), tx.clone(), subscriptions.clone()).await;
    }

    event_forwarder.abort();
}

async fn should_forward_live_event(
    env: &ServerEnvelope,
    subscriptions: &Arc<AsyncMutex<HashSet<String>>>,
) -> bool {
    let ServerEnvelope::Event { event, .. } = env else {
        return true;
    };
    if !requires_explicit_subscription(event) {
        return true;
    }
    subscriptions.lock().await.contains(event)
}

fn requires_explicit_subscription(event: &str) -> bool {
    event.starts_with("browser:") && (event.ends_with(":frame") || event.ends_with(":recording"))
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
    subscriptions: Arc<AsyncMutex<HashSet<String>>>,
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
        "subscribe_event" => {
            if let Some(event) = params.get("event").and_then(Value::as_str) {
                subscriptions.lock().await.insert(event.to_string());
                let _ = send_envelope(
                    &tx,
                    &ServerEnvelope::Response {
                        id,
                        result: Some(json!({"ok": true})),
                        error: None,
                    },
                )
                .await;
            } else {
                let _ = send_envelope(
                    &tx,
                    &ServerEnvelope::Response {
                        id,
                        result: None,
                        error: Some(RpcError {
                            code: "invalid-params".to_string(),
                            message: "subscribe_event requires an event string".to_string(),
                        }),
                    },
                )
                .await;
            }
        }
        "unsubscribe_event" => {
            if let Some(event) = params.get("event").and_then(Value::as_str) {
                subscriptions.lock().await.remove(event);
            }
            let _ = send_envelope(
                &tx,
                &ServerEnvelope::Response {
                    id,
                    result: Some(json!({"ok": true})),
                    error: None,
                },
            )
            .await;
        }
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
        "list_grouped_sessions_page" => {
            respond!(detached!(|s, p| handle_list_grouped_sessions_page(&s, &p)))
        }
        "load_desktop_pins" => respond!(handle_load_desktop_pins(&state)),
        "set_desktop_pin" => respond!(handle_set_desktop_pin(&state, &params)),
        "load_file_tabs" => respond!(handle_load_file_tabs(&state, &params)),
        "save_file_tabs" => respond!(handle_save_file_tabs(&state, &params)),
        "load_session_detail" => respond!(handle_load_session_detail(&state, &params)),
        "rename_session" => respond!(handle_rename_session(&state, &params)),
        "delete_session" => respond!(handle_delete_session(&state, &params)),
        "set_session_tags" => respond!(handle_set_session_tags(&state, &params)),
        "delete_project" => respond!(handle_delete_project(&state, &params)),
        "set_project_tags" => respond!(handle_set_project_tags(&state, &params)),
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
        "save_browser_settings" => {
            respond!(detached!(|s, p| {
                crate::daemon_browser_settings::handle_save_browser_settings(&s, &p)
            }))
        }
        "list_mcp_servers" => respond!(detached!(|s| handle_list_mcp_servers(&s))),
        "add_mcp_server" => respond!(detached!(|s, p| handle_add_mcp_server(&s, &p))),
        "list_lambda_skill_libraries" => {
            respond!(detached!(|s| handle_list_lambda_skill_libraries(&s)))
        }
        "save_lambda_skill_library" => {
            respond!(detached!(|s, p| handle_save_lambda_skill_library(&s, &p)))
        }
        "remove_lambda_skill_library" => {
            respond!(detached!(|s, p| handle_remove_lambda_skill_library(&s, &p)))
        }
        "set_lambda_skill_enabled" => {
            respond!(detached!(|s, p| handle_set_lambda_skill_enabled(&s, &p)))
        }
        "set_lambda_skill_approval" => {
            respond!(detached!(|s, p| handle_set_lambda_skill_approval(&s, &p)))
        }
        "list_provider_models" => {
            respond!(detached!(|s, p| handle_list_provider_models(&s, &p)))
        }
        "list_media_capabilities" => {
            respond!(detached!(|s, p| handle_list_media_capabilities(&s, &p)))
        }
        "generate_media" => respond!(detached!(|s, p| handle_generate_media(&s, &p))),
        "read_generated_media_preview" => {
            respond!(detached!(|s, p| handle_read_generated_media_preview(
                &s, &p
            )))
        }
        "create_generated_video_access" => {
            respond!(detached!(|s, p| handle_create_generated_video_access(
                &s, &p
            )))
        }
        "create_file_media_access" => {
            respond!(detached!(|s, p| handle_create_file_media_access(&s, &p)))
        }
        "save_proxy_settings" => respond!(detached!(|s, p| handle_save_proxy_settings(&s, &p))),
        "save_secret" => respond!(detached!(|s, p| handle_save_secret(&s, &p))),
        "delete_secret" => respond!(detached!(|s, p| handle_delete_secret(&s, &p))),
        "contacts_list" => {
            respond!(detached!(|s, p| {
                crate::daemon_contacts::handle_contacts_list(s.config_paths(), &p)
            }))
        }
        "contacts_search" => {
            respond!(detached!(|s, p| {
                crate::daemon_contacts::handle_contacts_search(s.config_paths(), &p)
            }))
        }
        "contacts_refresh" => {
            respond!(detached!(|s, p| {
                crate::daemon_contacts::handle_contacts_refresh(s.config_paths(), &p)
            }))
        }
        "contacts_context" => {
            respond!(detached!(|s, p| {
                crate::daemon_contacts::handle_contacts_context(s.config_paths(), &p)
            }))
        }
        "contacts_save" => {
            respond!(detached!(|s, p| {
                crate::daemon_contacts::handle_contacts_save(s.config_paths(), &p)
            }))
        }
        "contacts_delete" => {
            respond!(detached!(|s, p| {
                crate::daemon_contacts::handle_contacts_delete(s.config_paths(), &p)
            }))
        }
        "contacts_infer" => {
            respond!(detached!(|s, p| {
                crate::daemon_contacts::handle_contacts_infer(&s, &p)
            }))
        }
        "telegram_rank_relationships" => {
            respond!(detached!(|s, p| handle_telegram_rank_relationships(&s, &p)))
        }
        "import_chrome_secrets" => {
            respond!(detached!(|s| handle_import_chrome_secrets(&s)))
        }
        "import_browser_secrets" => {
            respond!(detached!(|s, p| handle_import_browser_secrets(&s, &p)))
        }
        "list_secret_sources" => respond!(handle_list_secret_sources()),
        "import_onepassword_export" => {
            respond!(detached!(|s, p| handle_import_onepassword_export(&s, &p)))
        }
        "test_proxy" => respond!(detached!(|s, p| handle_test_proxy(&s, &p))),
        "create_openai_realtime_client_secret" => {
            respond!(detached!(|s, p| {
                handle_create_openai_realtime_client_secret(&s, &p)
            }))
        }
        "list_permissions" => respond!(handle_list_permissions(&state)),
        "save_permissions" => respond!(handle_save_permissions(&state, &params)),
        "update_config" => respond!(detached!(|s, p| handle_update_config(&s, &p))),
        "user_memory_list" => respond!(handle_user_memory_list()),
        "user_memory_get" => respond!(handle_user_memory_get(&params)),
        "user_memory_set" => respond!(handle_user_memory_set(&params)),
        "user_memory_delete" => respond!(handle_user_memory_delete(&params)),
        "create_pull_request" => respond!(handle_create_pull_request(&state, &params)),
        "merge_pull_request" => respond!(handle_merge_pull_request(&state, &params)),
        "create_session" => {
            // Provider-specific sessions resolve runtime inputs, which may run
            // model discovery through reqwest::blocking; keep that off tokio.
            respond!(detached!(|s, p| handle_create_session(&s, &p)))
        }
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
        "browser_backend_status" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_backend_status(&s, &p)
        })),
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
        "browser_current_tab" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_current_tab(&s, &p)
        })),
        "browser_agent" => respond!(detached!(|s, p| {
            crate::daemon_browser::handle_browser_agent(&s, &p)
        })),
        "canvas_state_update" => respond!(detached!(|s, p| handle_canvas_state_update(&s, &p))),
        "local_model_status" => {
            respond!(detached!(|s, p| handle_local_model_status(&s, &p)))
        }
        "install_local_model" => {
            respond!(detached!(|s, p| handle_install_local_model(&s, &p)))
        }
        "workflow_list" => respond!(crate::daemon_workflows::handle_workflow_list(&state.paths)),
        "workflow_save" => respond!(crate::daemon_workflows::handle_workflow_save(
            &state.paths,
            &params
        )),
        "workflow_binding_create" => respond!(
            crate::daemon_workflows::handle_workflow_binding_create(&state.paths, &params)
        ),
        "monitor_create" | "task_monitor_create" => respond!(
            crate::daemon_workflows::handle_monitor_create(&state.paths, &params)
        ),
        "monitor_task_ignore" | "task_monitor_ignore" => respond!(
            crate::daemon_workflows::handle_monitor_task_ignore(&state.paths, &params)
        ),
        "monitor_rule_add" | "task_monitor_rule_add" => respond!(
            crate::daemon_workflows::handle_monitor_rule_add(&state.paths, &params)
        ),
        "monitor_rule_delete" | "task_monitor_rule_delete" => respond!(
            crate::daemon_workflows::handle_monitor_rule_delete(&state.paths, &params)
        ),
        "monitor_task_complete" | "task_monitor_complete" => respond!(
            crate::daemon_workflows::handle_monitor_task_complete(&state.paths, &params)
        ),
        "monitor_reply_send" | "task_monitor_reply_send" => respond!(
            crate::daemon_workflows::handle_monitor_reply_send(&state.paths, &params)
        ),
        "monitor_action_execute" | "task_monitor_action_execute" => respond!(
            crate::daemon_workflows::handle_monitor_action_execute(&state.paths, &params)
        ),
        "monitor_memory_save" | "task_monitor_memory_save" => respond!(
            crate::daemon_workflows::handle_monitor_memory_save(&state.paths, &params)
        ),
        "monitor_history_list" | "task_monitor_history_list" => respond!(
            crate::daemon_workflows::handle_monitor_history_list(&state.paths, &params)
        ),
        "monitor_trace_list" | "task_monitor_trace_list" => respond!(
            crate::daemon_workflows::handle_monitor_trace_list(&state.paths, &params)
        ),
        "telegram_diagnostics_export" | "task_telegram_diagnostics_export" => respond!(
            crate::daemon_workflows::handle_telegram_diagnostics_export(&state.paths, &params)
        ),
        "workflow_binding_delete" => respond!(
            crate::daemon_workflows::handle_workflow_binding_delete(&state.paths, &params)
        ),
        "workflow_connection_delete" => respond!(
            crate::daemon_workflows::handle_workflow_connection_delete(&state.paths, &params)
        ),
        "workflow_toggle" => respond!(crate::daemon_workflows::handle_workflow_toggle(
            &state.paths,
            &params
        )),
        "workflow_runs_list" => respond!(crate::daemon_workflows::handle_workflow_runs_list(
            &state.paths,
            &params
        )),
        "workflow_run_show" => respond!(crate::daemon_workflows::handle_workflow_run_show(
            &state.paths,
            &params
        )),

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

        "recover_stale_turn" => {
            let tx_clone = tx.clone();
            let state_clone = state.clone();
            let id_clone = id.clone();
            tokio::spawn(async move {
                let result = recover_stale_turn(state_clone, params).await;
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
                            code: "stale-turn-recovery-error".to_string(),
                            message: format!("{e:#}"),
                        }),
                    },
                };
                let _ = send_envelope(&tx_clone, &env).await;
            });
        }

        "dispatch_slash_command" => {
            let tx_clone = tx.clone();
            let state_clone = state.clone();
            let id_clone = id.clone();
            tokio::spawn(async move {
                let result = start_slash_command_turn(state_clone, params).await;
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
                            code: "slash-dispatch-error".to_string(),
                            message: format!("{e:#}"),
                        }),
                    },
                };
                let _ = send_envelope(&tx_clone, &env).await;
            });
        }

        "start_connector_setup" => {
            let tx_clone = tx.clone();
            let state_clone = state.clone();
            let id_clone = id.clone();
            tokio::spawn(async move {
                let result = start_connector_setup_turn(state_clone, params).await;
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
                            code: "connector-setup-error".to_string(),
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

fn handle_canvas_state_update(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("canvas_state_update requires sessionId"))?;
    let canvas_id = params
        .get("canvasId")
        .or_else(|| params.get("canvas_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("canvas_state_update requires canvasId"))?;
    let patch = params
        .get("patch")
        .ok_or_else(|| anyhow::anyhow!("canvas_state_update requires patch"))?;
    let session_store = SessionStore::from_paths(&state.paths)?;
    let cwd = desktop_api::load_session_cwd(&session_store, session_id)?;
    let updated = puffer_core::apply_canvas_state_patch(&cwd, session_id, canvas_id, patch)?;
    Ok(json!({
        "ok": true,
        "canvasId": canvas_id,
        "updatedAtMs": updated.get("updatedAtMs").cloned().unwrap_or(Value::Null)
    }))
}

fn handle_list_grouped_sessions(state: &DaemonState) -> Result<Value> {
    let session_store = SessionStore::from_paths(&state.paths)?;
    let project_tags = project_tag_map(&state.paths);
    let mut groups: Vec<FolderGroupDto> =
        desktop_api::list_grouped_sessions(&session_store, &project_tags)?;
    apply_session_routing_to_groups(state, &mut groups)?;
    Ok(serde_json::to_value(groups)?)
}

const DEFAULT_SESSION_LIST_PAGE_SIZE: usize = 30;
const MAX_SESSION_LIST_PAGE_SIZE: usize = 200;

fn handle_list_grouped_sessions_page(state: &DaemonState, params: &Value) -> Result<Value> {
    let offset = params
        .get("offset")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_SESSION_LIST_PAGE_SIZE)
        .clamp(1, MAX_SESSION_LIST_PAGE_SIZE);
    let session_store = SessionStore::from_paths(&state.paths)?;
    let project_tags = project_tag_map(&state.paths);
    let mut page =
        desktop_api::list_grouped_sessions_page(&session_store, &project_tags, offset, limit)?;
    apply_session_routing_to_groups(state, &mut page.groups)?;
    Ok(serde_json::to_value(page)?)
}

fn project_tag_map(paths: &ConfigPaths) -> BTreeMap<String, Vec<String>> {
    crate::project_metadata::ProjectMetadataStore::from_paths(paths)
        .all()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, meta)| (key, meta.tags))
        .collect()
}

fn apply_session_routing_to_groups(
    state: &DaemonState,
    groups: &mut [FolderGroupDto],
) -> Result<()> {
    for group in groups {
        for session in &mut group.sessions {
            match load_session_routing_state(&state.paths.user_config_dir, &session.session_id) {
                Ok(routing) => {
                    session.provider_id = routing.provider_id;
                    session.model_id = routing.model_id;
                }
                Err(_) => {
                    session.provider_id = None;
                    session.model_id = None;
                }
            }
        }
    }
    Ok(())
}

fn apply_session_routing_to_detail(
    state: &DaemonState,
    detail: &mut SessionDetailDto,
) -> Result<()> {
    let routing = load_session_routing_state(&state.paths.user_config_dir, &detail.session_id)?;
    detail.provider_id = routing.provider_id;
    detail.model_id = routing.model_id;
    Ok(())
}

fn apply_session_runtime_turn_state(
    state: &DaemonState,
    session_store: &SessionStore,
    detail: &mut SessionDetailDto,
) -> Result<()> {
    let session_uuid = Uuid::parse_str(&detail.session_id).context("invalid session id")?;
    let record = session_store.load_session(session_uuid)?;
    detail.active_turn_id = active_turn_id_for_session(state, session_uuid, &record.events);
    Ok(())
}

fn active_turn_id_for_session(
    state: &DaemonState,
    session_uuid: Uuid,
    events: &[TranscriptEvent],
) -> Option<Option<String>> {
    if let Some(turn_id) = state
        .turns
        .lock()
        .unwrap()
        .iter()
        .find_map(|(turn_id, handle)| {
            (handle.session_uuid == Some(session_uuid)).then(|| turn_id.clone())
        })
    {
        return Some(Some(turn_id));
    }

    if events.is_empty()
        || events
            .iter()
            .any(|event| matches!(event, TranscriptEvent::TurnBoundary { .. }))
    {
        return Some(None);
    }

    None
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
    let mut detail: SessionDetailDto =
        desktop_api::load_session_detail(&session_store, session_id)?;
    apply_session_runtime_turn_state(state, &session_store, &mut detail)?;
    apply_session_routing_to_detail(state, &mut detail)?;
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
    let mut detail: SessionDetailDto =
        desktop_api::load_session_detail(&session_store, session_id)?;
    apply_session_runtime_turn_state(state, &session_store, &mut detail)?;
    apply_session_routing_to_detail(state, &mut detail)?;
    Ok(serde_json::to_value(detail)?)
}

fn handle_delete_session(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(|v| v.as_str())
        .context("missing sessionId")?;
    let session_uuid = Uuid::parse_str(session_id).context("invalid sessionId")?;
    let session_store = SessionStore::from_paths(&state.paths)?;
    session_store.delete_session(session_uuid)?;
    crate::attachment_bridge::cleanup_session_attachments(session_uuid);
    state.publish_event(ServerEnvelope::Event {
        event: "workspace:sessions:changed".to_string(),
        payload: json!({
            "reason": "delete_session",
            "sessionId": session_id,
        }),
    });
    Ok(json!({ "ok": true, "sessionId": session_id }))
}

fn handle_set_session_tags(state: &DaemonState, params: &Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(|v| v.as_str())
        .context("missing sessionId")?;
    let tags = params
        .get("tags")
        .and_then(|v| v.as_array())
        .context("missing tags")?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let session_uuid = Uuid::parse_str(session_id).context("invalid sessionId")?;
    let session_store = SessionStore::from_paths(&state.paths)?;
    session_store.set_tags(session_uuid, tags)?;
    state.publish_event(ServerEnvelope::Event {
        event: "workspace:sessions:changed".to_string(),
        payload: json!({
            "reason": "set_session_tags",
            "sessionId": session_id,
        }),
    });
    let mut detail: SessionDetailDto =
        desktop_api::load_session_detail(&session_store, session_id)?;
    apply_session_runtime_turn_state(state, &session_store, &mut detail)?;
    apply_session_routing_to_detail(state, &mut detail)?;
    Ok(serde_json::to_value(detail)?)
}

fn handle_delete_project(state: &DaemonState, params: &Value) -> Result<Value> {
    let folder_path = params
        .get("folderPath")
        .or_else(|| params.get("folder_path"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("missing folderPath")?
        .to_string();
    let session_store = SessionStore::from_paths(&state.paths)?;
    let sessions = session_store.list_sessions()?;
    let target = std::path::Path::new(&folder_path);
    let mut removed = 0usize;
    for session in sessions {
        if desktop_api::session_group_root(&session.cwd) == target {
            session_store.delete_session(session.id)?;
            crate::attachment_bridge::cleanup_session_attachments(session.id);
            removed += 1;
        }
    }
    let _ = crate::project_metadata::ProjectMetadataStore::from_paths(&state.paths).delete(target);
    state.publish_event(ServerEnvelope::Event {
        event: "workspace:sessions:changed".to_string(),
        payload: json!({
            "reason": "delete_project",
            "folderPath": folder_path,
            "removedSessions": removed,
        }),
    });
    Ok(json!({ "ok": true, "folderPath": folder_path, "removedSessions": removed }))
}

fn handle_set_project_tags(state: &DaemonState, params: &Value) -> Result<Value> {
    let folder_path = params
        .get("folderPath")
        .or_else(|| params.get("folder_path"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("missing folderPath")?
        .to_string();
    let tags = params
        .get("tags")
        .and_then(|v| v.as_array())
        .context("missing tags")?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let store = crate::project_metadata::ProjectMetadataStore::from_paths(&state.paths);
    let meta = store.set_tags(std::path::Path::new(&folder_path), tags)?;
    state.publish_event(ServerEnvelope::Event {
        event: "workspace:sessions:changed".to_string(),
        payload: json!({
            "reason": "set_project_tags",
            "folderPath": folder_path,
        }),
    });
    Ok(json!({ "ok": true, "folderPath": folder_path, "tags": meta.tags }))
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
    state.settings_snapshot_value()
}

fn handle_save_secret(state: &DaemonState, params: &Value) -> Result<Value> {
    crate::daemon_secrets::save_secret(state.config_paths(), params)?;
    handle_load_settings_snapshot(state)
}

fn handle_delete_secret(state: &DaemonState, params: &Value) -> Result<Value> {
    let _ = crate::daemon_secrets::delete_secret(state.config_paths(), params)?;
    handle_load_settings_snapshot(state)
}

/// Ranks the top-5 Telegram contacts by recent chat frequency and runs a local
/// qwen35 relationship analysis, pushing progress + result over the event bus.
/// `params.connectionSlug` is optional — the first connected account is used
/// when omitted.
fn handle_telegram_rank_relationships(state: &DaemonState, params: &Value) -> Result<Value> {
    let tg_root = state
        .config_paths()
        .user_config_dir
        .join("telegram-accounts");
    let slug = match params
        .get("connectionSlug")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(s) => s.to_string(),
        None => first_telegram_account_slug(&tg_root).context(
            "no connected Telegram account with message diagnostics found under telegram-accounts/",
        )?,
    };
    let diagnostics = tg_root.join(&slug).join("message-diagnostics.ndjson");
    if !diagnostics.exists() {
        anyhow::bail!(
            "no message diagnostics for telegram account `{slug}` at {}",
            diagnostics.display()
        );
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    // Model backend: cloud (default — ~$0.002/run, better quality) or local
    // qwen35 (privacy). Cloud reads the stored OpenAI key.
    let use_local = params
        .get("useLocal")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let backend = if use_local {
        crate::daemon_telegram_ranking::ModelBackend::local()
    } else {
        let auth_path = state.config_paths().user_config_dir.join("auth.json");
        let auth_store = AuthStore::load(&auth_path).context("load auth store for cloud model")?;
        let key = match auth_store.providers.get("openai") {
            Some(StoredCredential::ApiKey { key }) => key.clone(),
            _ => anyhow::bail!(
                "cloud analysis needs an OpenAI API key — connect OpenAI, or set useLocal: true to use the local qwen35 model"
            ),
        };
        crate::daemon_telegram_ranking::ModelBackend::cloud(key)
    };

    crate::daemon_telegram_ranking::run(
        &diagnostics,
        &state.event_sender(),
        &slug,
        &backend,
        now_ms,
    )
}

/// Picks a default Telegram account: the first sub-directory of
/// `telegram-accounts/` that has a `message-diagnostics.ndjson`.
fn first_telegram_account_slug(root: &std::path::Path) -> Option<String> {
    let mut slugs: Vec<String> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().join("message-diagnostics.ndjson").exists())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    slugs.sort();
    slugs.into_iter().next()
}

fn handle_import_chrome_secrets(state: &DaemonState) -> Result<Value> {
    let report = crate::daemon_secrets::import_chrome_secrets(state.config_paths())?;
    let settings = handle_load_settings_snapshot(state)?;
    Ok(json!({
        "settings": settings,
        "report": report,
    }))
}

fn handle_import_browser_secrets(state: &DaemonState, params: &Value) -> Result<Value> {
    let source = params
        .get("source")
        .and_then(|value| value.as_str())
        .context("import_browser_secrets requires a `source`")?;
    let report = crate::daemon_secrets::import_browser_secrets(state.config_paths(), source)?;
    let settings = handle_load_settings_snapshot(state)?;
    Ok(json!({
        "settings": settings,
        "report": report,
    }))
}

fn handle_list_secret_sources() -> Result<Value> {
    Ok(json!({ "sources": crate::daemon_secrets::list_secret_sources() }))
}

fn handle_import_onepassword_export(state: &DaemonState, params: &Value) -> Result<Value> {
    let path = params
        .get("path")
        .and_then(|value| value.as_str())
        .context("import_onepassword_export requires a `path`")?;
    let report = crate::daemon_secrets::import_onepassword_export(state.config_paths(), path)?;
    let settings = handle_load_settings_snapshot(state)?;
    Ok(json!({
        "settings": settings,
        "report": report,
    }))
}

fn handle_save_proxy_settings(state: &DaemonState, params: &Value) -> Result<Value> {
    let input: SaveProxySettingsParams =
        serde_json::from_value(params.clone()).context("invalid proxy settings")?;
    let current_config = state.config.lock().unwrap().clone();
    let proxies = input
        .proxies
        .into_iter()
        .map(|endpoint| proxy_endpoint_from_input(endpoint, &current_config))
        .collect::<Result<Vec<_>>>()?;
    let mut config = current_config;
    config.network.proxy = ProxyConfig {
        enabled: input.enabled,
        selected: input.selected.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }),
        bypass: input.bypass,
        proxies,
    };
    config.network.proxy.normalize_selection();
    config.network.proxy.validate()?;
    save_user_config(&state.paths, &config).context("save user config")?;
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

fn handle_test_proxy(state: &DaemonState, params: &Value) -> Result<Value> {
    let input: TestProxyParams =
        serde_json::from_value(params.clone()).context("invalid proxy test params")?;
    let current_config = state.config.lock().unwrap().clone();
    let endpoint = if let Some(endpoint) = input.endpoint {
        proxy_endpoint_from_input(endpoint, &current_config)?
    } else {
        let proxy_id = input
            .proxy_id
            .or_else(|| current_config.network.proxy.selected.clone())
            .context("missing proxyId or endpoint")?;
        current_config
            .network
            .proxy
            .proxies
            .iter()
            .find(|endpoint| endpoint.id == proxy_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown proxy `{proxy_id}`"))?
    };
    let result = match test_proxy_connectivity(&endpoint, Duration::from_secs(8)) {
        Ok((target_url, outcome)) => ProxyTestResultDto {
            proxy_id: Some(endpoint.id.clone()),
            ok: true,
            message: format!(
                "Connected to {target_url} with HTTP {}",
                outcome.status_code
            ),
            latency_ms: Some(desktop_latency_ms(outcome.latency_ms)),
            status_code: Some(outcome.status_code),
        },
        Err(error) => ProxyTestResultDto {
            proxy_id: Some(endpoint.id.clone()),
            ok: false,
            message: proxy_test_error_message(&endpoint, &error),
            latency_ms: None,
            status_code: None,
        },
    };
    Ok(serde_json::to_value(result)?)
}

/// Stores an API key credential in the workspace auth store and returns
/// the refreshed settings snapshot so the UI can re-render without a
/// second round-trip.
fn handle_login_with_api_key(state: &DaemonState, params: &Value) -> Result<Value> {
    let requested_provider_id = params
        .get("providerId")
        .or_else(|| params.get("provider_id"))
        .and_then(|v| v.as_str())
        .context("missing providerId")?;
    let provider_id = canonical_desktop_provider_id(requested_provider_id);
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
        &provider_id,
        api_key,
    )?;
    let _ = desktop_api::ensure_default_routing(
        &state.paths,
        &inputs.providers,
        &inputs.auth_store,
        &provider_id,
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
    let requested_provider_id = params
        .get("providerId")
        .or_else(|| params.get("provider_id"))
        .and_then(|v| v.as_str())
        .context("missing providerId")?;
    let provider_id = canonical_desktop_provider_id(requested_provider_id);

    let mut inputs = state.build_runtime_inputs()?;
    let auth_path = state.paths.user_config_dir.join("auth.json");
    let listener = crate::authflow::CallbackListener::bind_localhost("/callback")?;
    let bundle =
        oauth_login_bundle_for_provider(&inputs.providers, &provider_id, listener.redirect_uri())?;
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

    match oauth_family_for_provider(&inputs.providers, &provider_id) {
        Some(OauthFamily::OpenAi) => {
            let (code, parsed_state) = parse_openai_authorization_input(&callback);
            let code = code
                .ok_or_else(|| anyhow::anyhow!("could not extract an OpenAI authorization code"))?;
            if let Some(parsed_state) = parsed_state {
                if parsed_state != bundle.state {
                    anyhow::bail!("oauth state mismatch for openai");
                }
            }
            let oauth_client = proxy_oauth_client(
                &state.config.lock().unwrap().clone(),
                puffer_provider_openai::OPENAI_TOKEN_URL,
            )?;
            let credential = puffer_provider_openai::exchange_authorization_code_with_client(
                &oauth_client,
                &code,
                &bundle.verifier,
                None,
            )?;
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
            let oauth_client = proxy_oauth_client(
                &state.config.lock().unwrap().clone(),
                ANTHROPIC_API_BASE_URL,
            )?;
            let credential = puffer_transport_anthropic::exchange_authorization_code_with_client(
                &oauth_client,
                &code,
                &bundle.verifier,
                &bundle.state,
                Some(&redirect_uri),
                Some(ANTHROPIC_API_BASE_URL),
            )?;
            store_anthropic_credential(&mut inputs.auth_store, &provider_id, credential)?;
        }
        None => anyhow::bail!("oauth login is not implemented for {provider_id}"),
    }

    inputs.auth_store.save(&auth_path)?;
    let _ = desktop_api::ensure_default_routing(
        &state.paths,
        &inputs.providers,
        &inputs.auth_store,
        &provider_id,
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
    let requested_provider_id = params
        .get("providerId")
        .or_else(|| params.get("provider_id"))
        .and_then(|v| v.as_str())
        .context("missing providerId")?;
    let provider_id = canonical_desktop_provider_id(requested_provider_id);
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
        &provider_id,
        source,
    )?;
    let _ = desktop_api::ensure_default_routing(
        &state.paths,
        &inputs.providers,
        &inputs.auth_store,
        &provider_id,
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
    let requested_provider_id = params
        .get("providerId")
        .or_else(|| params.get("provider_id"))
        .and_then(|v| v.as_str())
        .context("missing providerId")?;
    let provider_id = canonical_desktop_provider_id(requested_provider_id);
    let mut inputs = state.build_runtime_inputs()?;
    let auth_path = state.paths.user_config_dir.join("auth.json");
    desktop_api::logout_provider(&mut inputs.auth_store, &auth_path, &provider_id)?;
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
        env: Default::default(),
        inherit_env: true,
        timeout: None,
        connect_timeout: None,
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
    let requested_provider_id = params
        .get("providerId")
        .or_else(|| params.get("provider_id"))
        .and_then(|v| v.as_str())
        .context("missing providerId")?;
    let provider_id = canonical_desktop_provider_id(requested_provider_id);
    let mut inputs = state.build_runtime_inputs_without_discovery()?;
    let config = state.config.lock().unwrap().clone();
    let needs_fresh_discovery = inputs
        .stale_discovery_provider_ids
        .iter()
        .any(|id| id == &provider_id);
    if needs_fresh_discovery {
        let discovery_result = if let Ok(client) = proxy_discovery_client(&config) {
            inputs
                .providers
                .discover_and_merge_provider_with_discovery_client(
                    &provider_id,
                    &inputs.auth_store,
                    &client,
                )
        } else {
            inputs
                .providers
                .discover_and_merge_provider(&provider_id, &inputs.auth_store)
        };
        match discovery_result {
            Ok(()) => {}
            Err(_error) => {
                // The model picker must stay usable while a provider's live
                // discovery endpoint is slow or unavailable. Cached/static
                // models were already applied by
                // `build_runtime_inputs_without_discovery`.
            }
        }
    }
    let entry = inputs
        .providers
        .provider_entries()
        .find(|p| p.descriptor.id == provider_id)
        .with_context(|| format!("unknown provider `{provider_id}`"))?;
    let family = provider_preference_family(&inputs.providers, &provider_id);
    let models: Vec<ModelDescriptorDto> = entry
        .descriptor
        .models
        .iter()
        .map(|model| model_descriptor_dto(family, model))
        .collect();
    Ok(json!({ "providerId": provider_id, "models": models }))
}

fn handle_list_media_capabilities(state: &DaemonState, params: &Value) -> Result<Value> {
    let kind = optional_trimmed_value(params, &["kind"]);
    let inputs = state.build_runtime_inputs_without_discovery()?;
    let discovery_cache = state.exact_media_discovery_cache(&inputs);
    Ok(json!({
        "capabilities": list_media_capabilities(
            &inputs.providers,
            &inputs.auth_store,
            kind.as_deref(),
            &discovery_cache
        )
    }))
}

fn list_media_capabilities(
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    kind: Option<&str>,
    discovery_cache: &ExactMediaDiscoveryCache,
) -> Vec<MediaCapabilityInfoDto> {
    list_exact_media_capabilities_with_cache(providers, auth_store, kind, discovery_cache)
        .into_iter()
        .map(media_capability_info_dto)
        .collect()
}

fn media_capability_info_dto(capability: MediaCapabilityView) -> MediaCapabilityInfoDto {
    MediaCapabilityInfoDto {
        provider_id: capability.provider_id,
        provider_display_name: capability.provider_display_name,
        model_id: capability.model_id,
        model_display_name: capability.model_display_name,
        kind: capability.kind,
        operation: capability.operation,
        adapter: capability.adapter,
        axes: capability
            .axes
            .into_iter()
            .map(|axis| MediaCapabilityAxisDto {
                id: axis.id,
                label: axis.label,
                role: axis.role.as_str().to_string(),
                control: serde_json::to_value(axis.control).unwrap_or(Value::Null),
            })
            .collect(),
        status: capability.status,
        source: capability.source,
        reason: capability.reason,
        checked_at_ms: capability.checked_at_ms,
    }
}

fn handle_generate_media(state: &DaemonState, params: &Value) -> Result<Value> {
    let input: GenerateMediaParams =
        serde_json::from_value(params.clone()).context("invalid media generation params")?;
    let kind = input.kind.trim().to_lowercase();
    if kind != "image" && kind != "video" {
        anyhow::bail!("unsupported media kind `{kind}`");
    }
    let prompt = input.prompt.trim().to_string();
    if prompt.is_empty() {
        anyhow::bail!("/{kind} requires a prompt");
    }
    let count = input.count;
    generate_media_job(state, &kind, prompt, count)
}

fn generate_media_job(
    state: &DaemonState,
    kind: &str,
    prompt: String,
    count: Option<u8>,
) -> Result<Value> {
    let config = state.config_snapshot();
    let selection = media_selection_for_kind(config.media, kind)?;
    let inputs = state.build_runtime_inputs_without_discovery()?;
    let discovery_cache = state.exact_media_discovery_cache(&inputs);
    let generation = generate_exact_media_with_cache(
        &inputs.providers,
        &inputs.auth_store,
        &state.paths.workspace_root,
        exact_media_generation_request(kind, prompt.clone(), count, selection)?,
        &discovery_cache,
    )?;
    let artifacts = generation
        .artifacts
        .into_iter()
        .map(|artifact| GenerateMediaArtifactResult {
            artifact_id: artifact.artifact_id,
            index: artifact.index,
            path: artifact.path.display().to_string(),
            mime_type: artifact.mime_type,
            size: artifact.byte_count,
            remote_source_url: artifact.remote_source_url,
        })
        .collect();
    let result = GenerateMediaResult {
        job_id: generation.job_id,
        requested_count: generation.requested_count,
        artifacts,
        kind: generation.kind,
        provider_id: generation.provider_id,
        model_id: generation.model_id,
        status: generation.status,
        prompt,
    };
    Ok(serde_json::to_value(result)?)
}

fn media_selection_for_kind(media: MediaConfig, kind: &str) -> Result<MediaGenerationConfig> {
    match kind {
        "image" => media.image,
        "video" => media.video,
        _ => anyhow::bail!("unsupported media kind `{kind}`"),
    }
    .ok_or_else(|| anyhow::anyhow!("{kind} media provider/model is not configured"))
}

fn exact_media_generation_request(
    kind: &str,
    prompt: String,
    count: Option<u8>,
    selection: MediaGenerationConfig,
) -> Result<ExactMediaGenerationRequest> {
    let missing_context = format!("{kind} media provider/model is not configured");
    // `model_id` carries the logical model; `parameters` carries the axis
    // selections. The runtime resolves these into a concrete upstream model id,
    // adapter, and request parameters.
    Ok(ExactMediaGenerationRequest {
        kind: kind.to_string(),
        provider_id: non_empty_media_field(&selection.provider_id, "provider_id")
            .context(missing_context.clone())?,
        model_id: non_empty_media_field(&selection.logical_model_id, "logical_model_id")
            .context(missing_context)?,
        operation: "generate".to_string(),
        prompt,
        image_references: Vec::new(),
        parameters: selection.selections,
        count,
    })
}

fn non_empty_media_field(value: &str, field: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("media selection `{field}` is empty");
    }
    Ok(trimmed.to_string())
}

fn handle_read_generated_media_preview(state: &DaemonState, params: &Value) -> Result<Value> {
    let input: GeneratedMediaPreviewParams =
        serde_json::from_value(params.clone()).context("invalid generated media preview params")?;
    let session_store = SessionStore::from_paths(&state.paths)?;
    let cwd = desktop_api::load_session_cwd(&session_store, &input.session_id)?;
    let result = read_generated_media_preview_by_artifact(&cwd, &input.artifact_id);
    Ok(serde_json::to_value(result)?)
}

/// Insert a streaming ticket for `metadata` and render the `available` JSON
/// response shared by `create_generated_video_access` and
/// `create_file_media_access` (both mint tickets for the same range handler).
fn generated_video_ticket_response(
    state: &DaemonState,
    metadata: GeneratedVideoAccessMetadata,
) -> Value {
    let (token, ticket) = state.insert_generated_video_ticket(metadata);
    json!({
        "state": "available",
        "path": generated_video_ticket_path(&token),
        "mimeType": ticket.mime_type,
        "size": ticket.size,
        "expiresAtMs": ticket.expires_at_ms
    })
}

fn handle_create_generated_video_access(state: &DaemonState, params: &Value) -> Result<Value> {
    let input: GeneratedVideoAccessParams =
        serde_json::from_value(params.clone()).context("invalid generated video access params")?;
    let session_store = SessionStore::from_paths(&state.paths)?;
    let cwd = desktop_api::load_session_cwd(&session_store, &input.session_id)?;
    let result = generated_video_access_metadata_by_artifact(&cwd, &input.artifact_id);
    let GeneratedVideoAccessMetadataResult::Available(metadata) = result else {
        return Ok(match result {
            GeneratedVideoAccessMetadataResult::Missing => json!({ "state": "missing" }),
            GeneratedVideoAccessMetadataResult::Unsupported => json!({ "state": "unsupported" }),
            GeneratedVideoAccessMetadataResult::Available(_) => unreachable!(),
        });
    };
    Ok(generated_video_ticket_response(state, metadata))
}

fn handle_create_file_media_access(state: &DaemonState, params: &Value) -> Result<Value> {
    let raw_path = params
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing path"))?;
    // Reuse the same allowed-roots + canonicalization check read_file uses, so
    // a ticket can never point outside the workspace. Any failure (escape,
    // nonexistent, not absolute) collapses to "missing".
    let canonical = match crate::daemon_files::validate_path(state, raw_path) {
        Ok(path) => path,
        Err(_) => return Ok(json!({ "state": "missing" })),
    };
    let metadata = match std::fs::metadata(&canonical) {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return Ok(json!({ "state": "missing" })),
    };
    let Some(mime_type) = file_media_mime_type(&canonical) else {
        return Ok(json!({ "state": "unsupported" }));
    };
    Ok(generated_video_ticket_response(
        state,
        GeneratedVideoAccessMetadata {
            path: canonical,
            mime_type: mime_type.to_string(),
            byte_count: metadata.len(),
            remote_source_url: None,
        },
    ))
}

const DEFAULT_REALTIME_MODEL: &str = "gpt-realtime-2";
const DEFAULT_REALTIME_VOICE: &str = "marin";

fn handle_create_openai_realtime_client_secret(
    state: &DaemonState,
    params: &Value,
) -> Result<Value> {
    let requested_provider_id = optional_trimmed_value(params, &["providerId", "provider_id"])
        .unwrap_or_else(|| "openai".to_string());
    let provider_id = canonical_desktop_provider_id(&requested_provider_id);
    let inputs = state.build_runtime_inputs_without_discovery()?;
    let provider = inputs
        .providers
        .provider(&provider_id)
        .with_context(|| format!("unknown provider `{provider_id}`"))?;
    let request_config = openai_realtime_request_config(provider, &inputs.auth_store)?;
    let (session, model, voice) = realtime_session_config_from_params(params)?;
    let request = build_realtime_client_secret_request(
        &request_config,
        &OpenAIRealtimeClientSecretRequest { session },
    )?;
    let response = send_openai_realtime_request(&request)?;
    let client_secret = response
        .get("value")
        .and_then(Value::as_str)
        .context("malformed Realtime client-secret response: missing value")?;
    let expires_at = response.get("expires_at").and_then(Value::as_i64);

    Ok(json!({
        "providerId": provider_id,
        "model": model,
        "voice": voice,
        "clientSecret": client_secret,
        "expiresAt": expires_at,
    }))
}

fn openai_realtime_request_config(
    provider: &ProviderDescriptor,
    auth_store: &AuthStore,
) -> Result<OpenAIRequestConfig> {
    let key = match auth_store.get(provider.id.as_str()) {
        Some(StoredCredential::ApiKey { key }) => key.clone(),
        Some(StoredCredential::OAuth(_)) => {
            anyhow::bail!("OpenAI Realtime client-secret minting requires an API key credential")
        }
        None => anyhow::bail!("missing OpenAI API key credential for `{}`", provider.id),
    };

    Ok(OpenAIRequestConfig {
        base_url: provider.base_url.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        auth: OpenAIAuth::ApiKey(key),
        originator: "puffer_desktop".to_string(),
        session_id: None,
        account_id: None,
        custom_headers: provider
            .headers
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        query_params: provider
            .query_params
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        chat_completions_path: None,
        responses_path: None,
    })
}

fn realtime_session_config_from_params(params: &Value) -> Result<(Value, String, String)> {
    let mut session = params.get("session").cloned().unwrap_or_else(|| json!({}));
    let model = optional_trimmed_value(params, &["model", "modelId", "model_id"])
        .or_else(|| nested_trimmed_string(&session, &["model"]))
        .unwrap_or_else(|| DEFAULT_REALTIME_MODEL.to_string());
    let explicit_voice = optional_trimmed_value(params, &["voice"]);
    let voice = explicit_voice
        .clone()
        .or_else(|| nested_trimmed_string(&session, &["audio", "output", "voice"]))
        .unwrap_or_else(|| DEFAULT_REALTIME_VOICE.to_string());
    let reasoning_effort = optional_trimmed_value(params, &["reasoningEffort", "reasoning_effort"]);

    let session_object = session
        .as_object_mut()
        .context("Realtime session config must be a JSON object")?;
    session_object
        .entry("type".to_string())
        .or_insert_with(|| json!("realtime"));
    if session_object.get("type").and_then(Value::as_str) != Some("realtime") {
        anyhow::bail!("Realtime session type must be `realtime`");
    }
    session_object.insert("model".to_string(), json!(model));
    if let Some(reasoning_effort) = reasoning_effort {
        session_object.insert(
            "reasoning".to_string(),
            json!({ "effort": reasoning_effort }),
        );
    }
    ensure_realtime_audio_defaults(session_object, &voice, explicit_voice.is_some());

    Ok((session, model, voice))
}

fn nested_trimmed_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn ensure_realtime_audio_defaults(
    session: &mut serde_json::Map<String, Value>,
    voice: &str,
    override_voice: bool,
) {
    let audio = session
        .entry("audio".to_string())
        .or_insert_with(|| json!({}));
    if !audio.is_object() {
        *audio = json!({});
    }
    let Some(audio) = audio.as_object_mut() else {
        return;
    };

    let input = audio
        .entry("input".to_string())
        .or_insert_with(|| json!({}));
    if !input.is_object() {
        *input = json!({});
    }
    if let Some(input) = input.as_object_mut() {
        input
            .entry("transcription".to_string())
            .or_insert(Value::Null);
    }

    let output = audio
        .entry("output".to_string())
        .or_insert_with(|| json!({}));
    if !output.is_object() {
        *output = json!({});
    }
    if let Some(output) = output.as_object_mut() {
        if override_voice {
            output.insert("voice".to_string(), json!(voice));
        } else {
            output
                .entry("voice".to_string())
                .or_insert_with(|| json!(voice));
        }
    }
}

fn send_openai_realtime_request(request: &BuiltOpenAIRequest) -> Result<Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut builder = client.post(&request.url);
    for (key, value) in &request.headers {
        builder = builder.header(key, value);
    }
    let response = builder
        .body(request.body.clone())
        .send()
        .with_context(|| format!("request to {} failed", request.url))?;
    let status = response.status();
    let body = response.text().context("read Realtime response body")?;
    let parsed = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({ "text": body }));
    if !status.is_success() {
        let message = parsed
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("OpenAI Realtime request failed");
        anyhow::bail!("OpenAI Realtime request failed with status {status}: {message}");
    }
    Ok(parsed)
}

fn model_descriptor_dto(
    family: ModelPreferenceFamily,
    model: &ModelDescriptor,
) -> ModelDescriptorDto {
    let thinking_options = thinking_options_for_model(family, model);
    let default_thinking_option_id = thinking_options
        .iter()
        .find(|option| option.is_default)
        .map(|option| option.id.clone());
    ModelDescriptorDto {
        id: model.id.clone(),
        display_name: model.display_name.clone(),
        provider: model.provider.clone(),
        api: model.api.clone(),
        context_window: model.context_window,
        max_output_tokens: model.max_output_tokens,
        supports_reasoning: model.supports_reasoning,
        thinking_options,
        default_thinking_option_id,
    }
}

fn thinking_options_for_model(
    family: ModelPreferenceFamily,
    model: &ModelDescriptor,
) -> Vec<ThinkingOptionDto> {
    if !model.supports_reasoning {
        return Vec::new();
    }
    let default_effort = default_effort_level(family);
    supported_effort_levels(family)
        .iter()
        .map(|effort| ThinkingOptionDto {
            id: (*effort).to_string(),
            label: effort_label(effort).to_string(),
            description: format!("Use {effort} reasoning effort for this turn."),
            is_default: *effort == default_effort,
        })
        .collect()
}

fn effort_label(effort: &str) -> &'static str {
    match effort {
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "X-high",
        "max" => "Max",
        _ => "Custom",
    }
}

/// Workspace permissions are stored as a TOML map of `tool_id → policy`
/// (e.g. `bash = "ask"`) plus a `[browser]` policy section. We read the
/// file directly so we don't have to plumb the `pub(crate)` type through
/// puffer-core, while preserving browser policy fields on round-trip.
#[derive(serde::Deserialize, serde::Serialize, Default)]
struct BrowserPolicyFileDto {
    #[serde(default)]
    deny_target_classes: Vec<String>,
    #[serde(default)]
    deny_origins: Vec<String>,
    #[serde(default)]
    deny_domains: Vec<String>,
    #[serde(default)]
    deny_evaluate_target_classes: Vec<String>,
    #[serde(default)]
    allow_target_classes: Vec<String>,
    #[serde(default)]
    allow_origins: Vec<String>,
    #[serde(default)]
    allow_domains: Vec<String>,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
struct PermissionsFileDto {
    #[serde(default)]
    tools: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    browser: BrowserPolicyFileDto,
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
    let tools = loaded
        .tools
        .into_iter()
        .filter(|(tool, _)| !puffer_core::is_browser_tool_selector(tool))
        .collect::<std::collections::BTreeMap<_, _>>();
    Ok(json!({
        "path": path.display().to_string(),
        "tools": tools,
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
        if puffer_core::is_browser_tool_selector(&t) {
            continue;
        }
        let p = policy.trim().to_ascii_lowercase();
        if !matches!(p.as_str(), "allow" | "ask" | "deny" | "disabled") {
            anyhow::bail!("invalid policy `{policy}` for `{t}` — expected allow|ask|deny|disabled");
        }
        normalized.insert(t, p);
    }
    let path = permissions_file_path(state);
    let browser = if path.exists() {
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        toml::from_str::<PermissionsFileDto>(&text)
            .with_context(|| format!("parse {}", path.display()))?
            .browser
    } else {
        BrowserPolicyFileDto::default()
    };
    let dto = PermissionsFileDto {
        tools: normalized,
        browser,
    };
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
            "openaiDisplayName" | "openai_display_name" => {
                guard.openai_display_name = match value {
                    Value::Null => None,
                    Value::String(s) if s.trim().is_empty() => None,
                    Value::String(s) => Some(s.trim().to_string()),
                    _ => anyhow::bail!("openaiDisplayName must be string or null"),
                };
            }
            "media" => {
                guard.media = if value.is_null() {
                    MediaConfig::default()
                } else {
                    serde_json::from_value(value.clone())
                        .context("media must be an object with image and video settings")?
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
    let routing = resolve_create_session_routing(
        state,
        optional_trimmed_value(params, &["providerId", "provider_id"]),
        optional_trimmed_value(params, &["modelId", "model_id"]),
    )?;
    ensure_session_cwd(&cwd)?;
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
    if routing.provider_id.is_some() || routing.model_id.is_some() {
        set_session_routing_state(
            &state.paths.user_config_dir,
            &session.id.to_string(),
            routing.clone(),
        )?;
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
        "providerId": routing.provider_id,
        "modelId": routing.model_id,
    }))
}

fn ensure_session_cwd(cwd: &Path) -> Result<()> {
    if cwd.exists() {
        if cwd.is_dir() {
            return Ok(());
        }
        anyhow::bail!(
            "session cwd exists but is not a directory: {}",
            cwd.display()
        );
    }
    std::fs::create_dir_all(cwd)
        .with_context(|| format!("failed to create session cwd {}", cwd.display()))
}

fn resolve_create_session_routing(
    state: &DaemonState,
    provider_id: Option<String>,
    model_id: Option<String>,
) -> Result<DesktopSessionRouting> {
    if provider_id.is_none() && model_id.is_none() {
        return Ok(DesktopSessionRouting::default());
    }

    // Creating a session only needs to verify the provider/model exists in
    // the locally-registered set — we don't need a refreshed model catalog
    // here. Discovery makes a synchronous network round-trip to every
    // eligible provider; doing it on every "Start agent" click made the
    // first click after a daemon restart stall for several seconds.
    let inputs = state.build_runtime_inputs_without_discovery()?;
    if let Some(provider_id) = provider_id {
        let provider_id = canonical_desktop_provider_id(&provider_id);
        let provider = inputs
            .providers
            .provider(&provider_id)
            .with_context(|| format!("unknown provider `{provider_id}`"))?;
        let model_id = resolve_create_session_model_id(
            &state.config.lock().unwrap(),
            provider,
            model_id.as_deref(),
        )?;
        return Ok(DesktopSessionRouting {
            provider_id: Some(provider_id),
            model_id,
        });
    }

    let Some(requested_model) = model_id else {
        return Ok(DesktopSessionRouting::default());
    };
    let (provider_id, model_id) = resolve_turn_model(&inputs.providers, &requested_model)?;
    Ok(DesktopSessionRouting {
        provider_id: Some(provider_id),
        model_id: Some(model_id),
    })
}

fn resolve_create_session_model_id(
    config: &PufferConfig,
    provider: &ProviderDescriptor,
    requested_model: Option<&str>,
) -> Result<Option<String>> {
    let selected = requested_model
        .and_then(|model| normalize_provider_model_id(provider, model))
        .or_else(|| {
            config
                .default_model
                .as_deref()
                .filter(|_| {
                    config
                        .default_provider
                        .as_deref()
                        .is_some_and(|default| desktop_provider_ids_match(default, &provider.id))
                })
                .and_then(|model| normalize_provider_model_id(provider, model))
        })
        .or_else(|| provider.models.first().map(|model| model.id.clone()));

    let Some(model_id) = selected else {
        return Ok(None);
    };
    if provider.models.iter().any(|model| model.id == model_id) {
        return Ok(Some(model_id));
    }
    anyhow::bail!("unknown model `{model_id}` for provider `{}`", provider.id)
}

fn normalize_provider_model_id(provider: &ProviderDescriptor, model_id: &str) -> Option<String> {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return None;
    }
    if provider.models.iter().any(|model| model.id == trimmed) {
        return Some(trimmed.to_string());
    }
    if let Some((prefix, model)) = trimmed.split_once('/') {
        let model = model.trim();
        if model.is_empty() {
            return None;
        }
        let prefix = canonical_desktop_provider_id(prefix);
        let provider_id = canonical_desktop_provider_id(&provider.id);
        if prefix != provider_id {
            return None;
        }
        if provider
            .models
            .iter()
            .any(|descriptor| descriptor.id == model)
        {
            return Some(model.to_string());
        }
        if desktop_provider_model_prefix_is_alias(&provider_id) {
            return Some(model.to_string());
        }
        return Some(trimmed.to_string());
    }
    Some(trimmed.to_string())
}

fn desktop_provider_model_prefix_is_alias(provider_id: &str) -> bool {
    matches!(
        canonical_desktop_provider_id(provider_id).as_str(),
        "anthropic" | "openai"
    )
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

// ---- global user memory (~/.puffer/user.md) CRUD over the daemon RPC ----
// Mirrors the agent-side `Remember` tool; lets the desktop UI list/get/set/
// delete the same keyed user facts the agent reads from context.
fn user_memory_required_key(params: &Value) -> Result<String> {
    params
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| !key.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("key is required"))
}

fn handle_user_memory_list() -> Result<Value> {
    let memory = puffer_core::user_memory::UserMemory::global()?;
    Ok(json!({
        "facts": memory.list()?,
        "path": memory.path().display().to_string(),
    }))
}

fn handle_user_memory_get(params: &Value) -> Result<Value> {
    let key = user_memory_required_key(params)?;
    let memory = puffer_core::user_memory::UserMemory::global()?;
    Ok(json!({
        "key": puffer_core::user_memory::normalize_key(&key),
        "value": memory.get(&key)?,
    }))
}

fn handle_user_memory_set(params: &Value) -> Result<Value> {
    let key = user_memory_required_key(params)?;
    let value = params
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("value is required"))?;
    let memory = puffer_core::user_memory::UserMemory::global()?;
    let block = memory.set(&key, value)?;
    Ok(json!({ "success": true, "key": block.key }))
}

fn handle_user_memory_delete(params: &Value) -> Result<Value> {
    let key = user_memory_required_key(params)?;
    let memory = puffer_core::user_memory::UserMemory::global()?;
    Ok(json!({ "success": true, "removed": memory.delete(&key)? }))
}

fn local_model_id_param(params: &Value) -> &str {
    params
        .get("modelId")
        .or_else(|| params.get("model_id"))
        .and_then(Value::as_str)
        .unwrap_or("qwen35")
}

fn handle_local_model_status(state: &DaemonState, params: &Value) -> Result<Value> {
    let status = state.local_models.status(local_model_id_param(params))?;
    serde_json::to_value(status).context("serialize local model status")
}

fn handle_install_local_model(state: &DaemonState, params: &Value) -> Result<Value> {
    let job = state
        .local_models
        .install_or_start(state.event_sender(), local_model_id_param(params))?;
    serde_json::to_value(job).context("serialize local model install job")
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
    let action = parse_permission_action(params)?;
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

fn parse_permission_action(params: &Value) -> Result<PermissionPromptAction> {
    let action_str = params
        .get("action")
        .and_then(|v| v.as_str())
        .context("missing action")?;
    Ok(match action_str {
        "allow_once" => PermissionPromptAction::AllowOnce,
        "allow_session" => PermissionPromptAction::AllowSession,
        "allow_all_session" => PermissionPromptAction::AllowAllSession,
        "deny" => PermissionPromptAction::Deny,
        other => anyhow::bail!("unknown action `{other}`"),
    })
}

fn browser_permission_payload_json(payload: &puffer_core::BrowserPermissionPromptPayload) -> Value {
    json!({
        "source": match payload.source {
            BrowserPermissionPromptSource::BrowserTool => "browser_tool",
            BrowserPermissionPromptSource::BrowserInternalTool => "browser_internal_tool",
        },
        "actionSet": match payload.action_set {
            BrowserPermissionPromptActionSet::Inspect => "inspect",
            BrowserPermissionPromptActionSet::Navigate => "navigate",
            BrowserPermissionPromptActionSet::Interact => "interact",
            BrowserPermissionPromptActionSet::Evaluate => "evaluate",
        },
        "url": payload.url,
        "origin": payload.origin,
        "host": payload.host,
        "targetClass": match payload.target_class {
            BrowserPermissionPromptTargetClass::LocalDev => "local_dev",
            BrowserPermissionPromptTargetClass::WorkspaceFile => "workspace_file",
            BrowserPermissionPromptTargetClass::NonWorkspaceFile => "non_workspace_file",
            BrowserPermissionPromptTargetClass::DataUrl => "data_url",
            BrowserPermissionPromptTargetClass::OpenWeb => "open_web",
            BrowserPermissionPromptTargetClass::Unknown => "unknown",
        },
        "tabId": payload.tab_id,
        "isCrossSession": payload.is_cross_session,
    })
}

fn permission_review_payload_json(payload: &puffer_core::PermissionPromptReviewPayload) -> Value {
    json!({
        "decision": match payload.decision {
            puffer_core::BrowserAutoReviewRuntimeResult::AllowOnce => "allow_once",
            puffer_core::BrowserAutoReviewRuntimeResult::AllowSession => "allow_session",
            puffer_core::BrowserAutoReviewRuntimeResult::Deny => "deny",
            puffer_core::BrowserAutoReviewRuntimeResult::NeedsUser => "needs_user",
            puffer_core::BrowserAutoReviewRuntimeResult::Unavailable => "unavailable",
        },
        "risk": payload.risk,
        "rationale": payload.rationale,
        "resolvedRootSessionId": payload.resolved_root_session_id,
        "sessionTargeting": match payload.session_targeting {
            puffer_core::BrowserAutoReviewSessionTargeting::CurrentSession => "current_session",
            puffer_core::BrowserAutoReviewSessionTargeting::ExplicitSession => "explicit_session",
        },
    })
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
    if cancel_turn_by_id(state, turn_id) {
        Ok(json!({"ok": true}))
    } else {
        Ok(json!({"ok": false, "error": "turn not found"}))
    }
}

/// Cancels one running turn by id: flips its cancel token, denies any pending
/// permission/question prompts, reports the cancellation to listeners, and
/// removes it from the registry. Returns whether a turn with this id existed.
/// Shared by the `cancel_turn` RPC and the client-disconnect watchdog (#600).
fn cancel_turn_by_id(state: &DaemonState, turn_id: &str) -> bool {
    let handle = {
        let turns = state.turns.lock().unwrap();
        turns.get(turn_id).cloned()
    };
    let Some(handle) = handle else {
        return false;
    };
    handle.cancel.cancel();
    {
        let mut pending = handle.pending.lock().unwrap();
        for (_, tx) in pending.drain() {
            let _ = tx.send(PermissionPromptAction::Deny);
        }
    }
    {
        let mut pending_questions = handle.pending_questions.lock().unwrap();
        for (_, tx) in pending_questions.drain() {
            let _ = tx.send(UserQuestionPromptResponse {
                answers: serde_json::Map::new(),
                annotations: serde_json::Map::new(),
            });
        }
    }
    // Cancellation cleanup is best-effort: never let a failed report block the
    // cancel (especially on the disconnect path, where no client is waiting).
    if let (Some(session_uuid), Some(session_id)) =
        (handle.session_uuid, handle.session_id.as_deref())
    {
        let _ = report_cancelled_turn(
            state,
            session_uuid,
            session_id,
            &handle.channel,
            turn_id,
            &handle.message,
            &handle.attachments,
            &handle.cancel_reported,
            &handle.user_prompt_persisted,
            &handle.progress,
        );
    } else {
        report_cancelled_sessionless_turn(state, &handle.channel, turn_id, &handle.cancel_reported);
    }
    state.turns.lock().unwrap().remove(turn_id);
    true
}

/// Cancels every running turn in the registry. Only WS-initiated interactive
/// turns live here (`start_turn` / slash-command / connector-setup) — Telegram
/// monitor / connector-message turns run outside this registry — so this is
/// safe to call when the last UI client disconnects without affecting
/// background work (issue #600).
fn cancel_all_active_turns(state: &DaemonState) -> usize {
    let turn_ids: Vec<String> = state.turns.lock().unwrap().keys().cloned().collect();
    turn_ids
        .iter()
        .filter(|turn_id| cancel_turn_by_id(state, turn_id))
        .count()
}

/// Tracks a live WebSocket client. On drop (client disconnect) it decrements
/// the live-connection count and, when the last client goes away, arms the
/// disconnect watchdog so orphaned interactive turns get cancelled after a
/// grace window (issue #600).
struct ConnectionGuard {
    state: Arc<DaemonState>,
}

impl ConnectionGuard {
    fn new(state: Arc<DaemonState>) -> Self {
        state.live_connections.fetch_add(1, Ordering::SeqCst);
        Self { state }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let remaining = self
            .state
            .live_connections
            .fetch_sub(1, Ordering::SeqCst)
            .saturating_sub(1);
        if remaining == 0 {
            arm_disconnect_cancel_watchdog(self.state.clone());
        }
    }
}

/// After the last client disconnects, wait out the grace window; if no client
/// has reconnected, cancel orphaned interactive turns (issue #600).
fn arm_disconnect_cancel_watchdog(state: Arc<DaemonState>) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(DISCONNECT_CANCEL_GRACE).await;
        if state.live_connections.load(Ordering::SeqCst) != 0 {
            return; // a client reconnected during the grace window
        }
        let cancelled = cancel_all_active_turns(&state);
        if cancelled > 0 {
            eprintln!(
                "cancelled {cancelled} orphaned interactive turn(s) after the last UI client disconnected"
            );
        }
    });
}

const CANCELLED_TURN_MESSAGE: &str = "Interrupted by user.";

fn append_turn_boundary(
    session_store: &SessionStore,
    session_uuid: Uuid,
    turn_id: &str,
    state: TurnBoundaryState,
) -> Result<()> {
    session_store.append_event(
        session_uuid,
        TranscriptEvent::TurnBoundary {
            turn_id: turn_id.to_string(),
            state,
        },
    )
}

fn append_turn_started(
    session_store: &SessionStore,
    session_uuid: Uuid,
    turn_id: &str,
) -> Result<()> {
    append_turn_boundary(
        session_store,
        session_uuid,
        turn_id,
        TurnBoundaryState::Started,
    )
}

fn append_turn_finished(
    session_store: &SessionStore,
    session_uuid: Uuid,
    turn_id: &str,
) -> Result<()> {
    append_turn_boundary(
        session_store,
        session_uuid,
        turn_id,
        TurnBoundaryState::Finished,
    )
}

fn report_cancelled_turn(
    state: &DaemonState,
    session_uuid: Uuid,
    session_id: &str,
    channel: &str,
    turn_id: &str,
    message: &str,
    attachments: &[StoredAttachment],
    cancel_reported: &AtomicBool,
    user_prompt_persisted: &AtomicBool,
    progress: &Arc<Mutex<TurnProgress>>,
) -> Result<bool> {
    if cancel_reported.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }
    let session_store = SessionStore::from_paths(&state.paths)?;
    if !user_prompt_persisted.swap(true, Ordering::SeqCst) {
        append_turn_started(&session_store, session_uuid, turn_id)?;
        session_store.append_event(
            session_uuid,
            TranscriptEvent::UserMessage {
                text: message.to_string(),
                attachments: attachments.to_vec(),
                actor: None,
            },
        )?;
    }
    persist_cancelled_turn_progress(&session_store, session_uuid, progress)?;
    session_store.append_event(
        session_uuid,
        TranscriptEvent::SystemMessage {
            text: CANCELLED_TURN_MESSAGE.to_string(),
            actor: None,
        },
    )?;
    append_turn_finished(&session_store, session_uuid, turn_id)?;
    publish_turn_error_event(
        state,
        channel,
        session_id,
        turn_id,
        CANCELLED_TURN_MESSAGE.to_string(),
        None,
        Some("cancelled"),
    );
    Ok(true)
}

fn report_cancelled_sessionless_turn(
    state: &DaemonState,
    channel: &str,
    turn_id: &str,
    cancel_reported: &AtomicBool,
) {
    if cancel_reported.swap(true, Ordering::SeqCst) {
        return;
    }
    publish_sessionless_turn_error_event(
        state,
        channel,
        turn_id,
        CANCELLED_TURN_MESSAGE.to_string(),
        Some("cancelled"),
    );
}

fn persist_cancelled_turn_progress(
    session_store: &SessionStore,
    session_uuid: Uuid,
    progress: &Arc<Mutex<TurnProgress>>,
) -> Result<()> {
    let ordered_items = {
        let mut progress = progress.lock().unwrap();
        if progress.persisted_on_cancel {
            return Ok(());
        }
        progress.persisted_on_cancel = true;
        std::mem::take(&mut progress.ordered_items)
    };

    append_ordered_turn_progress(
        session_store,
        session_uuid,
        ordered_items,
        None,
        |_| None,
        true,
    )?;
    Ok(())
}

fn append_ordered_turn_progress(
    session_store: &SessionStore,
    session_uuid: Uuid,
    ordered_items: Vec<TurnProgressItem>,
    actor: Option<MessageActor>,
    subject_for_tool: impl Fn(&ToolInvocation) -> Option<MessageActor>,
    fail_pending_tools: bool,
) -> Result<String> {
    let mut assistant_text = String::new();
    for item in ordered_items {
        match item {
            TurnProgressItem::AssistantText(text) => {
                assistant_text.push_str(&text);
                if !text.is_empty() {
                    session_store.append_event(
                        session_uuid,
                        TranscriptEvent::AssistantMessage {
                            text,
                            actor: actor.clone(),
                        },
                    )?;
                }
            }
            TurnProgressItem::ToolInvocation(invocation) => {
                let subject = subject_for_tool(&invocation);
                session_store.append_event(
                    session_uuid,
                    TranscriptEvent::ToolInvocation {
                        call_id: invocation.call_id,
                        tool_id: invocation.tool_id,
                        input: invocation.input,
                        output: invocation.output,
                        success: invocation.success,
                        metadata: (!invocation.metadata.is_null()).then_some(invocation.metadata),
                        actor: actor.clone(),
                        subject,
                    },
                )?;
            }
            TurnProgressItem::PendingToolCall(request) => {
                if !fail_pending_tools {
                    continue;
                }
                session_store.append_event(
                    session_uuid,
                    TranscriptEvent::ToolInvocation {
                        call_id: request.call_id,
                        tool_id: request.tool_id,
                        input: request.input,
                        output: CANCELLED_TURN_MESSAGE.to_string(),
                        success: false,
                        metadata: Some(json!({
                            "cancelled": true,
                            "reason": "interrupted_by_user"
                        })),
                        actor: actor.clone(),
                        subject: None,
                    },
                )?;
            }
        }
    }
    Ok(assistant_text)
}

// ---------------------------------------------------------------------------
// Turn driver — spawns a std::thread to run the (synchronous) agent loop
// and relays events onto the broadcast bus.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentTurnMode {
    Default,
    Plan,
}

fn parse_agent_turn_mode(params: &Value) -> Result<AgentTurnMode> {
    let Some(raw_mode) = params
        .get("mode")
        .or_else(|| params.get("turnMode"))
        .or_else(|| params.get("turn_mode"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
    else {
        return Ok(AgentTurnMode::Default);
    };

    let raw_mode = raw_mode.to_ascii_lowercase();
    match raw_mode.as_str() {
        "default" | "normal" => Ok(AgentTurnMode::Default),
        "plan" | "plan-mode" | "plan_mode" => Ok(AgentTurnMode::Plan),
        other => anyhow::bail!("unsupported turn mode `{other}`; expected default or plan"),
    }
}

fn parse_attachment_ids(params: &Value) -> Result<Vec<String>> {
    let Some(value) = params
        .get("attachmentIds")
        .or_else(|| params.get("attachment_ids"))
    else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .context("attachmentIds must be an array")?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .context("attachmentIds must contain strings")
        })
        .collect()
}

fn unix_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn recover_stale_turn(state: Arc<DaemonState>, params: Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("session_id"))
        .and_then(|v| v.as_str())
        .context("missing sessionId")?
        .to_string();
    let session_uuid = Uuid::parse_str(&session_id).context("invalid sessionId")?;
    let retry_after_ms = params
        .get("retryAfterMs")
        .or_else(|| params.get("retry_after_ms"))
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_STALE_TURN_RETRY_AFTER_MS);

    {
        let turns = state.turns.lock().unwrap();
        if let Some((existing_turn_id, _)) = turns
            .iter()
            .find(|(_, handle)| handle.session_uuid == Some(session_uuid))
        {
            return Ok(json!({
                "recovery": "not_recoverable",
                "reason": "turn_in_flight",
                "turnId": existing_turn_id,
            }));
        }
    }

    let session_store = SessionStore::from_paths(&state.paths)?;
    let record = session_store.load_session(session_uuid)?;
    match stale_turn_recovery_decision(&record, unix_timestamp_ms(), retry_after_ms) {
        StaleTurnRecoveryDecision::Retry {
            message,
            attachment_ids,
            marker,
        } => {
            session_store.append_event(
                session_uuid,
                TranscriptEvent::SystemMessage {
                    text: marker,
                    actor: Some(recovery_actor()),
                },
            )?;
            state.publish_event(ServerEnvelope::Event {
                event: "workspace:sessions:changed".to_string(),
                payload: json!({
                    "reason": "stale_turn_retry_marker",
                    "sessionId": session_id.clone(),
                }),
            });

            let mut retry_params = params.clone();
            if let Some(object) = retry_params.as_object_mut() {
                object.insert("sessionId".to_string(), json!(session_id.clone()));
                object.insert("message".to_string(), json!(message));
                if !attachment_ids.is_empty() {
                    object.insert("attachmentIds".to_string(), json!(attachment_ids));
                }
            } else {
                anyhow::bail!("recover_stale_turn params must be an object");
            }
            let result = start_turn(state, retry_params).await?;
            let turn_id = result
                .get("turnId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(json!({
                "recovery": "retry_started",
                "turnId": turn_id,
            }))
        }
        StaleTurnRecoveryDecision::AlreadyRetried { marker } => {
            session_store.append_event(
                session_uuid,
                TranscriptEvent::SystemMessage {
                    text: marker,
                    actor: Some(recovery_actor()),
                },
            )?;
            state.publish_event(ServerEnvelope::Event {
                event: "workspace:sessions:changed".to_string(),
                payload: json!({
                    "reason": "stale_turn_retry_exhausted",
                    "sessionId": session_id.clone(),
                }),
            });
            Ok(json!({
                "recovery": "already_retried",
            }))
        }
        StaleTurnRecoveryDecision::NotRecoverable => Ok(json!({
            "recovery": "not_recoverable",
            "reason": "session_not_stale",
        })),
    }
}

/// Returns true when a replayed transcript shows this session already used
/// the browser — either through the Browser/BrowserAction tools or through
/// Bash running the `browser ...` CLI (the browser skill's documented path).
fn session_used_browser_tool(events: &[TranscriptEvent]) -> bool {
    events.iter().any(|event| {
        let TranscriptEvent::ToolInvocation { tool_id, input, .. } = event else {
            return false;
        };
        if tool_id.eq_ignore_ascii_case("browser") || tool_id.eq_ignore_ascii_case("browseraction")
        {
            return true;
        }
        if !tool_id.eq_ignore_ascii_case("bash") {
            return false;
        }
        let Ok(parsed) = serde_json::from_str::<Value>(input) else {
            return false;
        };
        parsed
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(bash_command_invokes_browser_cli)
    })
}

/// True when one Bash command line actually invokes the browser CLI. Matched
/// per shell segment on the leading token, so compound or env-prefixed
/// invocations (`cd x && browser open ...`, `FOO=1 browser snapshot`) count
/// while commands that merely mention the strings (grep/echo in coding
/// sessions) do not.
fn bash_command_invokes_browser_cli(command: &str) -> bool {
    command.split(['\n', ';', '|', '&']).any(|segment| {
        let mut tokens = segment
            .split_whitespace()
            .skip_while(|token| token.contains('=') && !token.starts_with('-'));
        let Some(first) = tokens.next() else {
            return false;
        };
        if first == "browser" {
            return true;
        }
        (first == "puffer" || first.ends_with("/puffer"))
            && tokens.next() == Some("internal-tool")
            && tokens.next() == Some("browser")
    })
}

/// Computes the live browser status line for the per-turn system reminder
/// (issue #560). Injected only when the transcript already used the browser
/// or a live tab exists, so pure coding sessions stay free of browser noise.
/// The no-active-tab line is the core fix: after a daemon restart the tab
/// registry is empty while the replayed transcript still claims
/// `connected:true`, and without this the model keeps trusting it.
fn browser_status_for_turn(
    context: &crate::daemon_browser::BrowserCurrentTabContext,
    session_used_browser: bool,
) -> Option<String> {
    (session_used_browser || context.has_active_tab()).then(|| context.agent_status_line())
}

/// Resolves the live tab context for a turn. Tabs live under two registry
/// keyspaces: the chat root session UUID (typed Browser/BrowserAction tools)
/// and the workspace-stable cli-browser id (Bash `browser ...` commands, the
/// browser skill's documented path). Checking only the UUID would report live
/// CLI tabs as gone — the inverse of the #560 bug.
fn turn_browser_tab_context(
    state: &DaemonState,
    browser_root_session_id: &str,
) -> crate::daemon_browser::BrowserCurrentTabContext {
    // Reconcile user-opened native tabs into the registry first, so the per-turn
    // browser status reflects a page the user opened without the agent (#649).
    state
        .browsers
        .sync_native_tabs(&state.event_sender(), browser_root_session_id, 960, 720);
    let primary = state.browsers.current_tab_context(browser_root_session_id);
    if primary.has_active_tab() {
        return primary;
    }
    let Ok(cli_session_id) = crate::daemon_browser::default_cli_session_id(&state.paths) else {
        return primary;
    };
    let cli = state.browsers.current_tab_context(&cli_session_id);
    if cli.has_active_tab() {
        cli
    } else {
        primary
    }
}

const MONITOR_REPLY_ACTION_PROMPT_SCOPE: &str = "monitor-reply-action";
const MONITOR_GMAIL_ACTION_PROMPT_SCOPE: &str = "monitor-gmail-action";
const MONITOR_TELEGRAM_ACTION_PROMPT_SCOPE: &str = "monitor-telegram-action";
const MONITOR_ACTION_PROMPT_PREFIX: &str = "Act on monitored task ";

#[derive(Clone, Debug)]
struct MonitorReplyTurnScope {
    task_id: String,
    session_id: String,
    turn_id: String,
    prompt_tool_scope: &'static str,
}

fn resolve_monitor_reply_turn_scope(
    state: &DaemonState,
    params: &Value,
    message: &str,
    session_id: &str,
    turn_id: &str,
) -> Result<Option<MonitorReplyTurnScope>> {
    let Some(prompt_task_id) = parse_monitor_action_prompt_task_id(message) else {
        if monitor_action_task_id_param(params).is_some() {
            anyhow::bail!("monitorActionTaskId requires a monitor action prompt");
        }
        if monitor_action_kind_param(params).is_some() {
            anyhow::bail!("monitorActionKind requires a monitor action prompt");
        }
        return resolve_existing_monitor_reply_session_scope(state, session_id, turn_id);
    };
    let Some(explicit_task_id) = monitor_action_task_id_param(params) else {
        anyhow::bail!("human-gated monitor action prompt requires matching monitorActionTaskId");
    };
    if explicit_task_id != prompt_task_id {
        anyhow::bail!(
            "monitorActionTaskId `{explicit_task_id}` does not match prompt task `{prompt_task_id}`"
        );
    }

    let task = load_monitor_task_for_scope(&state.paths, &prompt_task_id)?;
    let stored_kind = monitor_task_kind(&task);
    if stored_kind.is_some() && monitor_action_kind_param(params).is_none() {
        anyhow::bail!("typed monitor action prompt requires matching monitorActionKind");
    }
    if let (Some(explicit_kind), Some(stored_kind)) =
        (monitor_action_kind_param(params), stored_kind.as_deref())
    {
        if explicit_kind != stored_kind {
            anyhow::bail!(
                "monitorActionKind `{explicit_kind}` does not match monitor task kind `{stored_kind}`"
            );
        }
    }
    let prompt_tool_scope = match stored_kind.as_deref() {
        Some("gmail.reply") => {
            if monitor_task_is_terminal(&task) {
                return Ok(None);
            }
            if !monitor_task_has_gmail_thread_target(&task) {
                anyhow::bail!("monitor task `{prompt_task_id}` is missing a Gmail thread target");
            }
            MONITOR_GMAIL_ACTION_PROMPT_SCOPE
        }
        Some("telegram.reply") => {
            if monitor_task_is_terminal(&task) {
                return Ok(None);
            }
            if !monitor_task_has_delivery_target(&task) {
                anyhow::bail!(
                    "monitor task `{prompt_task_id}` is missing a source delivery target"
                );
            }
            MONITOR_TELEGRAM_ACTION_PROMPT_SCOPE
        }
        Some(kind) if kind != "telegram.reply" => return Ok(None),
        _ => {
            if !monitor_task_is_human_gated(&task) {
                return Ok(None);
            }
            if !monitor_task_has_delivery_target(&task) {
                anyhow::bail!(
                    "monitor task `{prompt_task_id}` is missing a source delivery target"
                );
            }
            MONITOR_REPLY_ACTION_PROMPT_SCOPE
        }
    };
    state
        .monitor_reply_sessions
        .lock()
        .unwrap()
        .insert(session_id.to_string(), prompt_task_id.clone());

    Ok(Some(MonitorReplyTurnScope {
        task_id: prompt_task_id,
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        prompt_tool_scope,
    }))
}

fn resolve_existing_monitor_reply_session_scope(
    state: &DaemonState,
    session_id: &str,
    turn_id: &str,
) -> Result<Option<MonitorReplyTurnScope>> {
    let mut scoped_task_id = state
        .monitor_reply_sessions
        .lock()
        .unwrap()
        .get(session_id)
        .cloned();
    if scoped_task_id.is_none() {
        scoped_task_id = recover_monitor_reply_task_id_from_transcript(state, session_id)?;
        if let Some(task_id) = &scoped_task_id {
            state
                .monitor_reply_sessions
                .lock()
                .unwrap()
                .insert(session_id.to_string(), task_id.clone());
        }
    }
    let Some(task_id) = scoped_task_id else {
        return Ok(None);
    };

    let task = match load_monitor_task_for_scope(&state.paths, &task_id) {
        Ok(task) => task,
        Err(_) => {
            state
                .monitor_reply_sessions
                .lock()
                .unwrap()
                .remove(session_id);
            return Ok(None);
        }
    };
    if monitor_task_is_terminal(&task) {
        state
            .monitor_reply_sessions
            .lock()
            .unwrap()
            .remove(session_id);
        return Ok(None);
    }
    let prompt_tool_scope = match monitor_task_kind(&task).as_deref() {
        Some("gmail.reply") => {
            if !monitor_task_has_gmail_thread_target(&task) {
                anyhow::bail!("monitor task `{task_id}` is missing a Gmail thread target");
            }
            MONITOR_GMAIL_ACTION_PROMPT_SCOPE
        }
        Some("telegram.reply") => {
            if !monitor_task_has_delivery_target(&task) {
                anyhow::bail!("monitor task `{task_id}` is missing a source delivery target");
            }
            MONITOR_TELEGRAM_ACTION_PROMPT_SCOPE
        }
        Some(kind) if kind != "telegram.reply" => {
            state
                .monitor_reply_sessions
                .lock()
                .unwrap()
                .remove(session_id);
            return Ok(None);
        }
        _ => {
            if !monitor_task_is_human_gated(&task) {
                state
                    .monitor_reply_sessions
                    .lock()
                    .unwrap()
                    .remove(session_id);
                return Ok(None);
            }
            if !monitor_task_has_delivery_target(&task) {
                anyhow::bail!("monitor task `{task_id}` is missing a source delivery target");
            }
            MONITOR_REPLY_ACTION_PROMPT_SCOPE
        }
    };

    Ok(Some(MonitorReplyTurnScope {
        task_id,
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        prompt_tool_scope,
    }))
}

fn recover_monitor_reply_task_id_from_transcript(
    state: &DaemonState,
    session_id: &str,
) -> Result<Option<String>> {
    let Ok(session_uuid) = Uuid::parse_str(session_id) else {
        return Ok(None);
    };
    let Ok(session_store) = SessionStore::from_paths(&state.paths) else {
        return Ok(None);
    };
    let Ok(record) = session_store.load_session(session_uuid) else {
        return Ok(None);
    };

    Ok(record.events.iter().rev().find_map(|event| {
        let TranscriptEvent::UserMessage { text, .. } = event else {
            return None;
        };
        parse_monitor_action_prompt_task_id(text)
    }))
}

fn parse_monitor_action_prompt_task_id(message: &str) -> Option<String> {
    let first_line = message.lines().next()?.trim();
    let rest = first_line.strip_prefix(MONITOR_ACTION_PROMPT_PREFIX)?;
    let task_id = rest.split_once(':').map(|(id, _)| id).unwrap_or(rest);
    non_empty_str(task_id).map(ToString::to_string)
}

fn monitor_action_task_id_param(params: &Value) -> Option<String> {
    params
        .get("monitorActionTaskId")
        .or_else(|| params.get("monitor_action_task_id"))
        .and_then(Value::as_str)
        .and_then(non_empty_str)
        .map(ToString::to_string)
}

fn monitor_action_kind_param(params: &Value) -> Option<String> {
    params
        .get("monitorActionKind")
        .or_else(|| params.get("monitor_action_kind"))
        .and_then(Value::as_str)
        .and_then(non_empty_str)
        .map(ToString::to_string)
}

fn monitor_task_kind(task: &Value) -> Option<String> {
    task.get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("monitor"))
        .and_then(Value::as_object)
        .and_then(|monitor| {
            let schema_version = monitor
                .get("schema_version")
                .or_else(|| monitor.get("schemaVersion"))
                .and_then(Value::as_u64)?;
            (schema_version == 2).then(|| {
                monitor
                    .get("kind")
                    .and_then(Value::as_str)
                    .and_then(non_empty_str)
                    .map(ToString::to_string)
            })?
        })
}

fn load_monitor_task_for_scope(paths: &ConfigPaths, task_id: &str) -> Result<Value> {
    let path = monitor_tasks_path_for_scope(paths);
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let store: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid monitor task store {}", path.display()))?;
    store
        .get("tasks")
        .and_then(Value::as_array)
        .and_then(|tasks| {
            tasks.iter().find(|task| {
                task_string(task, &["task_id", "taskId", "id"]).as_deref() == Some(task_id)
            })
        })
        .cloned()
        .with_context(|| format!("monitor task `{task_id}` not found"))
}

fn monitor_tasks_path_for_scope(paths: &ConfigPaths) -> std::path::PathBuf {
    paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
        .join("monitor_tasks.json")
}

fn monitor_task_is_human_gated(task: &Value) -> bool {
    let Some(metadata) = task.get("metadata").and_then(Value::as_object) else {
        return false;
    };
    let policy = metadata
        .get("completion_policy")
        .or_else(|| metadata.get("completionPolicy"));
    let mode = policy.and_then(|policy| {
        policy
            .get("mode")
            .and_then(Value::as_str)
            .or_else(|| policy.as_str())
    });
    matches!(mode, Some("draft_then_approve" | "send_to_source"))
        || policy
            .and_then(|policy| {
                policy
                    .get("requires_human_approval")
                    .or_else(|| policy.get("requiresHumanApproval"))
                    .and_then(Value::as_bool)
            })
            .unwrap_or(false)
        || monitor_task_has_delivery_target(task)
}

fn monitor_task_has_delivery_target(task: &Value) -> bool {
    task.pointer("/metadata/source_context/delivery_target/chat_id")
        .or_else(|| task.pointer("/metadata/sourceContext/deliveryTarget/chatId"))
        .or_else(|| task.pointer("/metadata/sourceContext/deliveryTarget/chat_id"))
        .or_else(|| task.pointer("/metadata/source_context/deliveryTarget/chatId"))
        .or_else(|| task.pointer("/metadata/monitor/source/chat_id"))
        .or_else(|| task.pointer("/metadata/monitor/source/chatId"))
        .and_then(value_to_non_empty_string)
        .is_some()
        || task
            .pointer("/metadata/monitor_connector")
            .or_else(|| task.pointer("/metadata/monitorConnector"))
            .and_then(Value::as_str)
            .is_some_and(|connector| connector.contains("telegram"))
            && task
                .pointer("/metadata/chat_id")
                .or_else(|| task.pointer("/metadata/chatId"))
                .and_then(value_to_non_empty_string)
                .is_some()
}

fn monitor_task_has_gmail_thread_target(task: &Value) -> bool {
    monitor_task_kind(task).as_deref() == Some("gmail.reply")
        && task
            .pointer("/metadata/monitor/source/thread_id")
            .or_else(|| task.pointer("/metadata/monitor/source/threadId"))
            .and_then(value_to_non_empty_string)
            .is_some()
}

fn monitor_task_is_terminal(task: &Value) -> bool {
    matches!(
        task.get("status").and_then(Value::as_str),
        Some("completed") | Some("cancelled")
    )
}

fn task_string(task: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| task.get(*key).and_then(Value::as_str))
        .and_then(non_empty_str)
        .map(ToString::to_string)
}

fn non_empty_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn value_to_non_empty_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => non_empty_str(value).map(ToString::to_string),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

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
    let turn_options = TurnRequestOptions::from_params(&params);
    let turn_mode = parse_agent_turn_mode(&params)?;
    let attachment_ids = parse_attachment_ids(&params)?;
    let turn_id = Uuid::new_v4().to_string();
    let channel = format!("session:{session_id}:event");

    // Parse cheap, non-tokio-touching things synchronously so we can fail
    // fast with a clean error. Anything that builds a runtime (i.e. the
    // provider registry's reqwest::blocking::Client) must NOT run here:
    // we're inside an async task, and dropping that inner runtime from an
    // async context panics. All such setup moved into the worker thread.
    let session_uuid = Uuid::parse_str(&session_id).context("invalid sessionId")?;
    let staged_attachments = if attachment_ids.is_empty() {
        Vec::new()
    } else {
        match SessionStore::from_paths(&state.paths)?
            .load_staged_attachments(session_uuid, &attachment_ids)
            .context("attachment staging")
        {
            Ok(attachments) => attachments,
            Err(error) => {
                publish_turn_error_event(
                    &state,
                    &channel,
                    &session_id,
                    &turn_id,
                    format!("{error:#}"),
                    None,
                    Some("attachment-staging"),
                );
                state.turns.lock().unwrap().remove(&turn_id);
                return Ok(json!({ "turnId": turn_id }));
            }
        }
    };
    let monitor_reply_scope =
        resolve_monitor_reply_turn_scope(&state, &params, &message, &session_id, &turn_id)?;

    let pending: Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<PermissionPromptAction>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_questions: Arc<
        Mutex<HashMap<String, std::sync::mpsc::Sender<UserQuestionPromptResponse>>>,
    > = Arc::new(Mutex::new(HashMap::new()));
    let cancel = CancelToken::new();
    let cancel_reported = Arc::new(AtomicBool::new(false));
    let user_prompt_persisted = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(TurnProgress::default()));

    {
        let mut turns = state.turns.lock().unwrap();
        if let Some((existing_turn_id, _)) = turns
            .iter()
            .find(|(_, handle)| handle.session_uuid == Some(session_uuid))
        {
            anyhow::bail!("session {session_id} already has an in-flight turn {existing_turn_id}");
        }
        turns.insert(
            turn_id.clone(),
            TurnHandle {
                session_id: Some(session_id.clone()),
                session_uuid: Some(session_uuid),
                channel: channel.clone(),
                message: message.clone(),
                attachments: staged_attachments.clone(),
                cancel: cancel.clone(),
                cancel_reported: cancel_reported.clone(),
                user_prompt_persisted: user_prompt_persisted.clone(),
                pending: pending.clone(),
                pending_questions: pending_questions.clone(),
                progress: progress.clone(),
            },
        );
    }

    let state_for_thread = state.clone();
    let turn_id_thread = turn_id.clone();
    let turn_id_resp = turn_id.clone();
    let channel_thread = channel.clone();
    let next_req_id = state.next_request_id.clone();
    let cancel_reported_thread = cancel_reported.clone();
    let user_prompt_persisted_thread = user_prompt_persisted.clone();
    let progress_thread = progress.clone();

    // Run the synchronous agent loop on a fresh OS thread, *completely
    // detached* from tokio. `ProviderRegistry::discover_and_merge_all` +
    // `execute_user_turn_streaming_with_permissions` both build tokio
    // runtimes via reqwest internally, and dropping those runtimes while
    // any tokio handle is installed on the current thread panics. A plain
    // `std::thread::spawn` gives us a thread with no ambient tokio state.
    let setup_state = state.clone();
    let message_for_thread = message.clone();
    let session_id_for_thread = session_id.clone();
    let turn_options_for_thread = turn_options.clone();
    let turn_mode_for_thread = turn_mode;
    let staged_attachments_for_thread = staged_attachments.clone();
    let monitor_reply_scope_for_thread = monitor_reply_scope.clone();
    std::thread::spawn(move || {
        setup_state.publish_event(ServerEnvelope::Event {
            event: channel_thread.clone(),
            payload: json!({"type": "turn-start", "turnId": turn_id_thread.clone()}),
        });

        // Load provider registry + auth + resources + session record on
        // this thread so the inner reqwest runtime is built + dropped in
        // the same clean (non-tokio) context.
        let mut inputs = match setup_state.build_runtime_inputs() {
            Ok(v) => v,
            Err(err) => {
                publish_turn_error_event(
                    &setup_state,
                    &channel_thread,
                    &session_id_for_thread,
                    &turn_id_thread,
                    format!("build_runtime_inputs: {err:#}"),
                    None,
                    None,
                );
                setup_state.turns.lock().unwrap().remove(&turn_id_thread);
                return;
            }
        };
        let record = match inputs.session_store.load_session(session_uuid) {
            Ok(v) => v,
            Err(err) => {
                publish_turn_error_event(
                    &setup_state,
                    &channel_thread,
                    &session_id_for_thread,
                    &turn_id_thread,
                    format!("load_session: {err:#}"),
                    None,
                    None,
                );
                setup_state.turns.lock().unwrap().remove(&turn_id_thread);
                return;
            }
        };
        let has_user_message = record
            .events
            .iter()
            .any(|event| matches!(event, TranscriptEvent::UserMessage { .. }));
        let session_used_browser = session_used_browser_tool(&record.events);
        // Browser panes key off the root session (mirrors
        // AppState::browser_root_session_id, which is core-private).
        let browser_root_session_id = record
            .metadata
            .parent_session_id
            .unwrap_or(session_uuid)
            .to_string();
        let auto_title = if crate::daemon_title::should_generate_title_for_turn(
            &inputs.resources,
            record.metadata.display_name.as_deref(),
            record.metadata.generated_title.as_deref(),
            has_user_message,
            setup_state.disable_auto_title,
            &message_for_thread,
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
        let mut effective_turn_options = turn_options_for_thread.clone();
        let explicit_turn_routing = effective_turn_options.has_explicit_routing();
        if effective_turn_options.model_override.is_none()
            && effective_turn_options.provider_id.is_none()
            && effective_turn_options.model_id.is_none()
        {
            match load_session_routing_state(
                &setup_state.paths.user_config_dir,
                &session_id_for_thread,
            ) {
                Ok(routing) => effective_turn_options.apply_session_routing(routing),
                Err(err) => {
                    publish_turn_error_event(
                        &setup_state,
                        &channel_thread,
                        &session_id_for_thread,
                        &turn_id_thread,
                        format!("load session routing: {err:#}"),
                        None,
                        None,
                    );
                    setup_state.turns.lock().unwrap().remove(&turn_id_thread);
                    return;
                }
            }
        }
        let cfg_for_turn = setup_state.config.lock().unwrap().clone();
        let mut app_state = AppState::from_session_record(cfg_for_turn.clone(), record);
        if let Some(scope) = &monitor_reply_scope_for_thread {
            app_state.set_monitor_reply_scope_for_turn(
                scope.task_id.clone(),
                scope.session_id.clone(),
                scope.turn_id.clone(),
            );
        }
        // Issue #560: reconcile the model's view of the browser with the real
        // tab registry every turn, instead of letting it trust stale
        // `connected:true` tool output replayed from the transcript.
        app_state.browser_status = browser_status_for_turn(
            &turn_browser_tab_context(&setup_state, &browser_root_session_id),
            session_used_browser,
        );
        if let Err(err) =
            apply_turn_request_options(&mut app_state, &inputs.providers, &effective_turn_options)
        {
            publish_turn_error_event(
                &setup_state,
                &channel_thread,
                &session_id_for_thread,
                &turn_id_thread,
                format!("turn options: {err:#}"),
                None,
                None,
            );
            setup_state.turns.lock().unwrap().remove(&turn_id_thread);
            return;
        }
        match persist_explicit_turn_routing(
            &setup_state.paths.user_config_dir,
            &session_id_for_thread,
            explicit_turn_routing,
            app_state.current_provider.as_deref(),
            app_state.current_model.as_deref(),
        ) {
            Ok(true) => setup_state.publish_event(ServerEnvelope::Event {
                event: "workspace:sessions:changed".to_string(),
                payload: json!({
                    "reason": "session_routing",
                    "sessionId": session_id_for_thread.clone(),
                }),
            }),
            Ok(false) => {}
            Err(err) => {
                publish_turn_error_event(
                    &setup_state,
                    &channel_thread,
                    &session_id_for_thread,
                    &turn_id_thread,
                    format!("persist session routing: {err:#}"),
                    None,
                    None,
                );
                setup_state.turns.lock().unwrap().remove(&turn_id_thread);
                return;
            }
        }
        if setup_state.yolo {
            apply_daemon_yolo_mode(&mut app_state);
        }
        if cancel.is_cancelled() {
            let _ = report_cancelled_turn(
                &setup_state,
                session_uuid,
                &session_id_for_thread,
                &channel_thread,
                &turn_id_thread,
                &message_for_thread,
                &staged_attachments_for_thread,
                &cancel_reported_thread,
                &user_prompt_persisted_thread,
                &progress_thread,
            );
            setup_state.turns.lock().unwrap().remove(&turn_id_thread);
            return;
        }
        app_state.set_exact_media_discovery_cache(setup_state.exact_media_discovery_cache(&inputs));
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
        if turn_mode_for_thread == AgentTurnMode::Plan {
            if let Err(err) = enter_plan_mode(&mut app_state) {
                publish_turn_error_event(
                    &setup_state,
                    &channel_thread,
                    &session_id_for_thread,
                    &turn_id_thread,
                    format!("plan mode: {err:#}"),
                    None,
                    None,
                );
                setup_state.turns.lock().unwrap().remove(&turn_id_thread);
                return;
            }
            let _ = inputs
                .session_store
                .append_event(session_uuid, app_state.snapshot_event());
        }

        // File attachments still receive temp path hints for Read. Image
        // attachments stay as metadata here and hydrate to transient data URLs
        // on AppState below.
        let file_attachments = staged_attachments_for_thread
            .iter()
            .filter(|attachment| attachment.kind == StoredAttachmentKind::File)
            .cloned()
            .collect::<Vec<_>>();
        let materialized_files = crate::attachment_bridge::materialize_attachments(
            &inputs.session_store,
            session_uuid,
            &file_attachments,
        );
        let model_input =
            crate::attachment_bridge::build_model_input(&message_for_thread, &materialized_files);

        app_state.push_user_message_with_attachments(
            model_input.clone(),
            staged_attachments_for_thread
                .iter()
                .cloned()
                .map(puffer_core::RenderedAttachment::from_stored)
                .collect(),
        );
        if let Err(err) = crate::attachment_bridge::hydrate_model_image_urls(
            &mut app_state,
            &inputs.session_store,
            session_uuid,
        ) {
            publish_turn_error_event(
                &setup_state,
                &channel_thread,
                &session_id_for_thread,
                &turn_id_thread,
                format!("{err:#}"),
                None,
                Some("attachment-hydration"),
            );
            setup_state.turns.lock().unwrap().remove(&turn_id_thread);
            return;
        }

        // Persist the user message before the turn starts so a crash
        // doesn't silently drop it.
        if !user_prompt_persisted_thread.swap(true, Ordering::SeqCst) {
            let _ = inputs.session_store.append_event(
                session_uuid,
                TranscriptEvent::TurnBoundary {
                    turn_id: turn_id_thread.clone(),
                    state: TurnBoundaryState::Started,
                },
            );
            let _ = inputs.session_store.append_event(
                session_uuid,
                TranscriptEvent::UserMessage {
                    text: message_for_thread.clone(),
                    attachments: staged_attachments_for_thread.clone(),
                    actor: Some(app_state.user_actor()),
                },
            );
        }
        let _ = &session_id_for_thread; // keep the String alive for logging

        let stream_actor = app_state.assistant_actor();
        let ev_state = setup_state.clone();
        let ev_channel = channel_thread.clone();
        let ev_turn = turn_id_thread.clone();
        let ev_actor = stream_actor.clone();
        let ev_progress = progress_thread.clone();
        let ev_cancel_reported = cancel_reported_thread.clone();
        let on_event = move |event: TurnStreamEvent| {
            if ev_cancel_reported.load(Ordering::SeqCst) {
                return;
            }
            let payload = match event {
                TurnStreamEvent::TextDelta(delta) => {
                    ev_progress.lock().unwrap().record_text_delta(&delta);
                    json!({"type": "text-delta", "turnId": ev_turn, "delta": delta})
                }
                TurnStreamEvent::ThinkingDelta(delta) => {
                    json!({"type": "thinking-delta", "turnId": ev_turn, "delta": delta})
                }
                TurnStreamEvent::ToolCallsRequested(reqs) => {
                    ev_progress
                        .lock()
                        .unwrap()
                        .record_tool_call_requests(reqs.clone());
                    json!({
                        "type": "tool-calls-requested",
                        "turnId": ev_turn,
                        "requests": reqs.iter().map(|r| json!({
                            "callId": r.call_id,
                            "toolId": r.tool_id,
                            "input": r.input,
                        })).collect::<Vec<_>>(),
                    })
                }
                TurnStreamEvent::ToolInvocations(invs) => {
                    {
                        let mut progress = ev_progress.lock().unwrap();
                        let advances_stream_attempt = invs
                            .iter()
                            .any(|invocation| !invocation.is_provider_stream_invocation());
                        progress.record_tool_invocations(invs.clone());
                        if advances_stream_attempt {
                            progress.checkpoint_stream_attempt();
                        }
                    }
                    let mut before_gates = Vec::new();
                    let mut after_gates = Vec::new();
                    for gate_payload in invs
                        .iter()
                        .filter_map(|inv| lambda_gate_stream_payload(&ev_turn, inv))
                    {
                        match lambda_gate_stream_phase(&gate_payload) {
                            LambdaGateStreamPhase::BeforeToolInvocations => {
                                before_gates.push(gate_payload);
                            }
                            LambdaGateStreamPhase::AfterToolInvocations => {
                                after_gates.push(gate_payload);
                            }
                        }
                    }
                    for gate_payload in before_gates {
                        ev_state.publish_event(ServerEnvelope::Event {
                            event: ev_channel.clone(),
                            payload: event_payload_with_actor(gate_payload, &ev_actor),
                        });
                    }
                    let tool_payload = json!({
                        "type": "tool-invocations",
                        "turnId": ev_turn.clone(),
                        "invocations": invs.iter().map(|i| json!({
                            "callId": i.call_id,
                            "toolId": i.tool_id,
                            "input": i.input,
                            "output": i.output,
                            "success": i.success,
                            "metadata": if i.metadata.is_null() { Value::Null } else { i.metadata.clone() },
                        })).collect::<Vec<_>>(),
                    });
                    ev_state.publish_event(ServerEnvelope::Event {
                        event: ev_channel.clone(),
                        payload: event_payload_with_actor(tool_payload, &ev_actor),
                    });
                    for gate_payload in after_gates {
                        ev_state.publish_event(ServerEnvelope::Event {
                            event: ev_channel.clone(),
                            payload: event_payload_with_actor(gate_payload, &ev_actor),
                        });
                    }
                    return;
                }
                TurnStreamEvent::PlanUpdated { file_path, content } => json!({
                    "type": "plan-updated",
                    "turnId": ev_turn,
                    "filePath": file_path,
                    "content": content,
                }),
                TurnStreamEvent::PlanCompleted { file_path, content } => json!({
                    "type": "plan-completed",
                    "turnId": ev_turn,
                    "filePath": file_path,
                    "content": content,
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
                    kind,
                } => {
                    let mut progress = ev_progress.lock().unwrap();
                    progress.reset_to_retry_checkpoint();
                    json!({
                        "type": "retry-attempt",
                        "turnId": ev_turn,
                        "attempt": attempt,
                        "maxAttempts": max_attempts,
                        "error": error,
                        "kind": kind.as_str(),
                    })
                }
            };
            let payload = event_payload_with_actor(payload, &ev_actor);
            ev_state.publish_event(ServerEnvelope::Event {
                event: ev_channel.clone(),
                payload,
            });
        };

        let perm_state = setup_state.clone();
        let perm_channel = channel_thread.clone();
        let perm_turn = turn_id_thread.clone();
        let perm_pending = pending.clone();
        let perm_actor = stream_actor.clone();
        let on_permission = move |req: PermissionPromptRequest| -> PermissionPromptAction {
            let request_id = next_req_id.fetch_add(1, Ordering::SeqCst).to_string();
            let (tx, rx) = std::sync::mpsc::channel();
            perm_pending.lock().unwrap().insert(request_id.clone(), tx);

            perm_state.publish_event(ServerEnvelope::Event {
                event: perm_channel.clone(),
                payload: event_payload_with_actor(
                    json!({
                        "type": "permission-request",
                        "turnId": perm_turn,
                        "requestId": request_id,
                        "toolId": req.tool_id,
                        "summary": req.summary,
                        "reason": req.reason,
                        "browser": req.browser.as_ref().map(browser_permission_payload_json),
                        "review": req.review.as_ref().map(permission_review_payload_json),
                    }),
                    &perm_actor,
                ),
            });

            rx.recv().unwrap_or(PermissionPromptAction::Deny)
        };

        let question_state = setup_state.clone();
        let question_channel = channel_thread.clone();
        let question_turn = turn_id_thread.clone();
        let question_pending = pending_questions.clone();
        let question_next_id = setup_state.next_request_id.clone();
        let question_actor = stream_actor.clone();
        let on_user_question = move |req: UserQuestionPromptRequest| -> UserQuestionPromptResponse {
            let request_id = question_next_id.fetch_add(1, Ordering::SeqCst).to_string();
            let (tx, rx) = std::sync::mpsc::channel();
            question_pending
                .lock()
                .unwrap()
                .insert(request_id.clone(), tx);

            question_state.publish_event(ServerEnvelope::Event {
                event: question_channel.clone(),
                payload: event_payload_with_actor(
                    json!({
                        "type": "user-question-request",
                        "turnId": question_turn.clone(),
                        "requestId": request_id,
                        "questions": req.questions,
                        "metadata": req.metadata,
                    }),
                    &question_actor,
                ),
            });

            rx.recv().unwrap_or(UserQuestionPromptResponse {
                answers: serde_json::Map::new(),
                annotations: serde_json::Map::new(),
            })
        };

        let mut auth_store = inputs.auth_store.clone();
        let outcome = with_user_question_prompt_handler(on_user_question, || {
            execute_user_turn_streaming_with_prompt_tools_and_cancel(
                &mut app_state,
                &inputs.resources,
                &inputs.providers,
                &mut auth_store,
                &model_input,
                monitor_reply_scope_for_thread
                    .as_ref()
                    .map(|scope| scope.prompt_tool_scope),
                None,
                &cancel,
                on_event,
                on_permission,
            )
        });

        match outcome {
            Ok(turn) => {
                if cancel_reported_thread.load(Ordering::SeqCst) {
                    state_for_thread
                        .turns
                        .lock()
                        .unwrap()
                        .remove(&turn_id_thread);
                    return;
                }
                if !turn.assistant_text.is_empty() {
                    app_state.push_message(MessageRole::Assistant, turn.assistant_text.clone());
                }
                let ordered_items = {
                    let mut progress = progress_thread.lock().unwrap();
                    progress.persisted_on_cancel = true;
                    std::mem::take(&mut progress.ordered_items)
                };
                let persisted_text = append_ordered_turn_progress(
                    &inputs.session_store,
                    session_uuid,
                    ordered_items,
                    Some(stream_actor.clone()),
                    |inv| app_state.tool_subject_actor(&inv.tool_id, &inv.output),
                    false,
                )
                .unwrap_or_default();
                if let Some(missing_text) = turn.assistant_text.strip_prefix(&persisted_text) {
                    if !missing_text.is_empty() {
                        let _ = inputs.session_store.append_event(
                            session_uuid,
                            TranscriptEvent::AssistantMessage {
                                text: missing_text.to_string(),
                                actor: Some(stream_actor.clone()),
                            },
                        );
                    }
                }
                for trace in &turn.reflection_traces {
                    let _ = inputs.session_store.append_trace_event(
                        session_uuid,
                        puffer_session_store::TRACE_RUNTIME,
                        trace,
                    );
                }
                let _ = append_turn_finished(&inputs.session_store, session_uuid, &turn_id_thread);
                let _ = inputs
                    .session_store
                    .append_event(session_uuid, app_state.snapshot_event());
                setup_state.publish_event(ServerEnvelope::Event {
                    event: channel_thread.clone(),
                    payload: event_payload_with_actor(
                        json!({
                            "type": "turn-complete",
                            "turnId": turn_id_thread.clone(),
                            "assistantText": turn.assistant_text,
                        }),
                        &stream_actor,
                    ),
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
                if cancel_reported_thread.load(Ordering::SeqCst) {
                    state_for_thread
                        .turns
                        .lock()
                        .unwrap()
                        .remove(&turn_id_thread);
                    return;
                }
                eprintln!("turn {turn_id_thread} failed: {err:#}");
                let _ = inputs
                    .session_store
                    .append_event(session_uuid, app_state.snapshot_event());
                let (friendly, category) = classify_turn_error(&err);
                let raw = format!("{err:#}");
                let _ = inputs.session_store.append_event(
                    session_uuid,
                    TranscriptEvent::SystemMessage {
                        text: friendly.clone(),
                        actor: Some(stream_actor.clone()),
                    },
                );
                let _ = append_turn_finished(&inputs.session_store, session_uuid, &turn_id_thread);
                publish_turn_error_event(
                    &setup_state,
                    &channel_thread,
                    &session_id_for_thread,
                    &turn_id_thread,
                    friendly,
                    Some(raw),
                    Some(category),
                );
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

/// Runs a slash command (`/connect ...`, `/workflows ...`, etc.) through
/// `puffer_core::dispatch_command` without contacting any provider. AskUser-
/// Question prompts emitted by deterministic flows like `/connect` reach the
/// UI via the same `session:{id}:event` channel as a regular turn.
async fn start_slash_command_turn(state: Arc<DaemonState>, params: Value) -> Result<Value> {
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
    if !message.trim_start().starts_with('/') {
        anyhow::bail!("dispatch_slash_command expects a leading '/' command");
    }

    let session_uuid = Uuid::parse_str(&session_id).context("invalid sessionId")?;
    let turn_id = Uuid::new_v4().to_string();
    let channel = format!("session:{session_id}:event");
    let pending: Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<PermissionPromptAction>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_questions: Arc<
        Mutex<HashMap<String, std::sync::mpsc::Sender<UserQuestionPromptResponse>>>,
    > = Arc::new(Mutex::new(HashMap::new()));
    let cancel = CancelToken::new();
    let cancel_reported = Arc::new(AtomicBool::new(false));
    let user_prompt_persisted = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(TurnProgress::default()));

    {
        let mut turns = state.turns.lock().unwrap();
        if let Some((existing_turn_id, _)) = turns
            .iter()
            .find(|(_, handle)| handle.session_uuid == Some(session_uuid))
        {
            anyhow::bail!("session {session_id} already has an in-flight turn {existing_turn_id}");
        }
        turns.insert(
            turn_id.clone(),
            TurnHandle {
                session_id: Some(session_id.clone()),
                session_uuid: Some(session_uuid),
                channel: channel.clone(),
                message: message.clone(),
                attachments: Vec::new(),
                cancel: cancel.clone(),
                cancel_reported: cancel_reported.clone(),
                user_prompt_persisted: user_prompt_persisted.clone(),
                pending: pending.clone(),
                pending_questions: pending_questions.clone(),
                progress: progress.clone(),
            },
        );
    }

    let setup_state = state.clone();
    let turn_id_thread = turn_id.clone();
    let turn_id_resp = turn_id.clone();
    let channel_thread = channel.clone();
    let next_req_id = state.next_request_id.clone();
    let session_id_for_thread = session_id.clone();
    let message_for_thread = message.clone();

    std::thread::spawn(move || {
        setup_state.publish_event(ServerEnvelope::Event {
            event: channel_thread.clone(),
            payload: json!({"type": "turn-start", "turnId": turn_id_thread.clone()}),
        });

        let mut inputs = match setup_state.build_runtime_inputs_without_discovery() {
            Ok(v) => v,
            Err(err) => {
                publish_turn_error_event(
                    &setup_state,
                    &channel_thread,
                    &session_id_for_thread,
                    &turn_id_thread,
                    format!("build_runtime_inputs: {err:#}"),
                    None,
                    None,
                );
                setup_state.turns.lock().unwrap().remove(&turn_id_thread);
                return;
            }
        };
        let record = match inputs.session_store.load_session(session_uuid) {
            Ok(v) => v,
            Err(err) => {
                publish_turn_error_event(
                    &setup_state,
                    &channel_thread,
                    &session_id_for_thread,
                    &turn_id_thread,
                    format!("load_session: {err:#}"),
                    None,
                    None,
                );
                setup_state.turns.lock().unwrap().remove(&turn_id_thread);
                return;
            }
        };
        let cfg_for_turn = setup_state.config.lock().unwrap().clone();
        // Issue #560: slash-command turns run the same agent loop, so they
        // need the same browser-state reconciliation as start_turn.
        let session_used_browser = session_used_browser_tool(&record.events);
        let browser_root_session_id = record
            .metadata
            .parent_session_id
            .unwrap_or(session_uuid)
            .to_string();
        let mut app_state = AppState::from_session_record(cfg_for_turn, record);
        app_state.browser_status = browser_status_for_turn(
            &turn_browser_tab_context(&setup_state, &browser_root_session_id),
            session_used_browser,
        );
        let stream_actor = app_state.assistant_actor();

        let question_state = setup_state.clone();
        let question_channel = channel_thread.clone();
        let question_turn = turn_id_thread.clone();
        let question_pending = pending_questions.clone();
        let question_next_id = next_req_id.clone();
        let question_actor = stream_actor.clone();
        let on_user_question = move |req: UserQuestionPromptRequest| -> UserQuestionPromptResponse {
            let request_id = question_next_id.fetch_add(1, Ordering::SeqCst).to_string();
            let (tx, rx) = std::sync::mpsc::channel();
            question_pending
                .lock()
                .unwrap()
                .insert(request_id.clone(), tx);

            question_state.publish_event(ServerEnvelope::Event {
                event: question_channel.clone(),
                payload: event_payload_with_actor(
                    json!({
                        "type": "user-question-request",
                        "turnId": question_turn.clone(),
                        "requestId": request_id,
                        "questions": req.questions,
                        "metadata": req.metadata,
                    }),
                    &question_actor,
                ),
            });

            rx.recv().unwrap_or(UserQuestionPromptResponse {
                answers: serde_json::Map::new(),
                annotations: serde_json::Map::new(),
            })
        };

        let mut auth_store = inputs.auth_store.clone();
        let commands = command_surface(&inputs.resources);
        let outcome = with_user_question_prompt_handler(on_user_question, || {
            dispatch_command(
                &mut app_state,
                &commands,
                &inputs.resources,
                &mut inputs.providers,
                &mut auth_store,
                &inputs.session_store,
                &message_for_thread,
            )
        });

        match outcome {
            Ok(()) => {
                let assistant_text = app_state
                    .transcript
                    .iter()
                    .rev()
                    .find(|m| matches!(m.role, MessageRole::System | MessageRole::Assistant))
                    .map(|m| m.text.clone())
                    .unwrap_or_default();
                let _ = inputs
                    .session_store
                    .append_event(session_uuid, app_state.snapshot_event());
                setup_state.publish_event(ServerEnvelope::Event {
                    event: channel_thread.clone(),
                    payload: event_payload_with_actor(
                        json!({
                            "type": "turn-complete",
                            "turnId": turn_id_thread,
                            "assistantText": assistant_text,
                        }),
                        &stream_actor,
                    ),
                });
                setup_state.publish_event(ServerEnvelope::Event {
                    event: "workspace:sessions:changed".to_string(),
                    payload: json!({
                        "reason": "turn_complete",
                        "sessionId": session_id_for_thread.clone(),
                    }),
                });
            }
            Err(err) => {
                eprintln!("slash command {turn_id_thread} failed: {err:#}");
                let _ = inputs
                    .session_store
                    .append_event(session_uuid, app_state.snapshot_event());
                publish_turn_error_event(
                    &setup_state,
                    &channel_thread,
                    &session_id_for_thread,
                    &turn_id_thread,
                    format!("{err:#}"),
                    None,
                    None,
                );
            }
        }

        let _ = (
            cancel.clone(),
            cancel_reported.clone(),
            user_prompt_persisted.clone(),
            progress.clone(),
            pending.clone(),
        );
        setup_state.turns.lock().unwrap().remove(&turn_id_thread);
    });

    Ok(json!({"turnId": turn_id_resp}))
}

async fn start_connector_setup_turn(state: Arc<DaemonState>, params: Value) -> Result<Value> {
    let turn_id = connector_setup_id(&params)?;
    let message = params
        .get("message")
        .and_then(|v| v.as_str())
        .context("missing message")?
        .to_string();
    let connect_args = connector_setup_connect_args(&message)?;
    let channel = format!("connector-setup:{turn_id}:event");
    let pending: Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<PermissionPromptAction>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_questions: Arc<
        Mutex<HashMap<String, std::sync::mpsc::Sender<UserQuestionPromptResponse>>>,
    > = Arc::new(Mutex::new(HashMap::new()));
    let cancel = CancelToken::new();
    let cancel_reported = Arc::new(AtomicBool::new(false));
    let user_prompt_persisted = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(TurnProgress::default()));

    {
        let mut turns = state.turns.lock().unwrap();
        if turns.contains_key(&turn_id) {
            anyhow::bail!("connector setup `{turn_id}` is already in flight");
        }
        turns.insert(
            turn_id.clone(),
            TurnHandle {
                session_id: None,
                session_uuid: None,
                channel: channel.clone(),
                message: message.clone(),
                attachments: Vec::new(),
                cancel: cancel.clone(),
                cancel_reported: cancel_reported.clone(),
                user_prompt_persisted,
                pending,
                pending_questions: pending_questions.clone(),
                progress,
            },
        );
    }

    let setup_state = state.clone();
    let turn_id_thread = turn_id.clone();
    let turn_id_resp = turn_id.clone();
    let channel_thread = channel.clone();
    let next_req_id = state.next_request_id.clone();
    let cancel_thread = cancel.clone();
    let cancel_reported_thread = cancel_reported.clone();
    std::thread::spawn(move || {
        setup_state.publish_event(ServerEnvelope::Event {
            event: channel_thread.clone(),
            payload: json!({"type": "turn-start", "turnId": turn_id_thread.clone()}),
        });

        #[cfg(unix)]
        if crate::daemon_wechat_browser_setup::connect_args_are_wechat(&connect_args) {
            let outcome = crate::daemon_wechat_browser_setup::execute_wechat_setup(
                setup_state.clone(),
                channel_thread.clone(),
                turn_id_thread.clone(),
                connect_args.clone(),
                next_req_id.clone(),
                pending_questions.clone(),
                cancel_thread.clone(),
            );
            match outcome {
                Ok(assistant_text) => {
                    setup_state.publish_event(ServerEnvelope::Event {
                        event: channel_thread.clone(),
                        payload: json!({
                            "type": "turn-complete",
                            "turnId": turn_id_thread.clone(),
                            "assistantText": assistant_text,
                        }),
                    });
                }
                Err(error) => {
                    if !(cancel_thread.is_cancelled()
                        && cancel_reported_thread.load(Ordering::SeqCst))
                    {
                        publish_sessionless_turn_error_event(
                            &setup_state,
                            &channel_thread,
                            &turn_id_thread,
                            format!("{error:#}"),
                            None,
                        );
                    }
                }
            }
            setup_state.turns.lock().unwrap().remove(&turn_id_thread);
            return;
        }

        if crate::daemon_gcal_browser_setup::connect_args_are_gcal_browser(&connect_args) {
            let outcome = crate::daemon_gcal_browser_setup::execute_gcal_browser_setup(
                setup_state.clone(),
                channel_thread.clone(),
                turn_id_thread.clone(),
                connect_args.clone(),
                next_req_id.clone(),
                pending_questions.clone(),
                cancel_thread.clone(),
            );
            match outcome {
                Ok(assistant_text) => {
                    setup_state.publish_event(ServerEnvelope::Event {
                        event: channel_thread.clone(),
                        payload: json!({
                            "type": "turn-complete",
                            "turnId": turn_id_thread.clone(),
                            "assistantText": assistant_text,
                        }),
                    });
                }
                Err(error) => {
                    if !(cancel_thread.is_cancelled()
                        && cancel_reported_thread.load(Ordering::SeqCst))
                    {
                        publish_sessionless_turn_error_event(
                            &setup_state,
                            &channel_thread,
                            &turn_id_thread,
                            format!("{error:#}"),
                            None,
                        );
                    }
                }
            }
            setup_state.turns.lock().unwrap().remove(&turn_id_thread);
            return;
        }

        if crate::daemon_gmail_browser_setup::connect_args_are_gmail_browser(&connect_args) {
            let outcome = crate::daemon_gmail_browser_setup::execute_gmail_browser_setup(
                setup_state.clone(),
                channel_thread.clone(),
                turn_id_thread.clone(),
                connect_args.clone(),
                next_req_id.clone(),
                pending_questions.clone(),
                cancel_thread.clone(),
            );
            match outcome {
                Ok(assistant_text) => {
                    setup_state.publish_event(ServerEnvelope::Event {
                        event: channel_thread.clone(),
                        payload: json!({
                            "type": "turn-complete",
                            "turnId": turn_id_thread.clone(),
                            "assistantText": assistant_text,
                        }),
                    });
                }
                Err(error) => {
                    if !(cancel_thread.is_cancelled()
                        && cancel_reported_thread.load(Ordering::SeqCst))
                    {
                        publish_sessionless_turn_error_event(
                            &setup_state,
                            &channel_thread,
                            &turn_id_thread,
                            format!("{error:#}"),
                            None,
                        );
                    }
                }
            }
            setup_state.turns.lock().unwrap().remove(&turn_id_thread);
            return;
        }

        if crate::daemon_lark_browser_setup::connect_args_are_lark_browser(&connect_args) {
            let outcome = crate::daemon_lark_browser_setup::execute_lark_browser_setup(
                setup_state.clone(),
                channel_thread.clone(),
                turn_id_thread.clone(),
                connect_args.clone(),
                next_req_id.clone(),
                pending_questions.clone(),
                cancel_thread.clone(),
            );
            match outcome {
                Ok(assistant_text) => {
                    setup_state.publish_event(ServerEnvelope::Event {
                        event: channel_thread.clone(),
                        payload: json!({
                            "type": "turn-complete",
                            "turnId": turn_id_thread.clone(),
                            "assistantText": assistant_text,
                        }),
                    });
                }
                Err(error) => {
                    if !(cancel_thread.is_cancelled()
                        && cancel_reported_thread.load(Ordering::SeqCst))
                    {
                        publish_sessionless_turn_error_event(
                            &setup_state,
                            &channel_thread,
                            &turn_id_thread,
                            format!("{error:#}"),
                            None,
                        );
                    }
                }
            }
            setup_state.turns.lock().unwrap().remove(&turn_id_thread);
            return;
        }

        let inputs = match setup_state.build_runtime_inputs_without_discovery() {
            Ok(v) => v,
            Err(err) => {
                publish_sessionless_turn_error_event(
                    &setup_state,
                    &channel_thread,
                    &turn_id_thread,
                    format!("build_runtime_inputs: {err:#}"),
                    None,
                );
                setup_state.turns.lock().unwrap().remove(&turn_id_thread);
                return;
            }
        };
        let cfg_for_turn = setup_state.config.lock().unwrap().clone();
        let metadata = connector_setup_session_metadata(setup_state.cwd.clone(), Uuid::new_v4());
        let mut app_state = AppState::new(cfg_for_turn, setup_state.cwd.clone(), metadata);
        let stream_actor = app_state.system_actor();

        let question_state = setup_state.clone();
        let question_channel = channel_thread.clone();
        let question_turn = turn_id_thread.clone();
        let question_pending = pending_questions.clone();
        let question_next_id = next_req_id.clone();
        let question_actor = stream_actor.clone();
        let on_user_question = move |req: UserQuestionPromptRequest| -> UserQuestionPromptResponse {
            let request_id = question_next_id.fetch_add(1, Ordering::SeqCst).to_string();
            let (tx, rx) = std::sync::mpsc::channel();
            question_pending
                .lock()
                .unwrap()
                .insert(request_id.clone(), tx);

            question_state.publish_event(ServerEnvelope::Event {
                event: question_channel.clone(),
                payload: event_payload_with_actor(
                    json!({
                        "type": "user-question-request",
                        "turnId": question_turn.clone(),
                        "requestId": request_id,
                        "questions": req.questions,
                        "metadata": req.metadata,
                    }),
                    &question_actor,
                ),
            });

            rx.recv().unwrap_or(UserQuestionPromptResponse {
                answers: serde_json::Map::new(),
                annotations: serde_json::Map::new(),
            })
        };

        let outcome = with_user_question_prompt_handler(on_user_question, || {
            cancel_thread.check()?;
            let turn = execute_connect_flow(&mut app_state, &inputs.resources, &connect_args)?;
            cancel_thread.check()?;
            Ok::<_, anyhow::Error>(turn)
        });

        match outcome {
            Ok(turn) => {
                setup_state.publish_event(ServerEnvelope::Event {
                    event: channel_thread.clone(),
                    payload: event_payload_with_actor(
                        json!({
                            "type": "turn-complete",
                            "turnId": turn_id_thread.clone(),
                            "assistantText": turn.assistant_text,
                        }),
                        &stream_actor,
                    ),
                });
            }
            Err(error) => {
                if !(cancel_thread.is_cancelled() && cancel_reported_thread.load(Ordering::SeqCst))
                {
                    publish_sessionless_turn_error_event(
                        &setup_state,
                        &channel_thread,
                        &turn_id_thread,
                        format!("{error:#}"),
                        None,
                    );
                }
            }
        }
        setup_state.turns.lock().unwrap().remove(&turn_id_thread);
    });

    Ok(json!({"turnId": turn_id_resp, "setupId": turn_id_resp}))
}

fn connector_setup_id(params: &Value) -> Result<String> {
    let setup_id = params
        .get("setupId")
        .or_else(|| params.get("setup_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    if !setup_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        anyhow::bail!("connector setup id must be ASCII alphanumeric, '-' or '_'");
    }
    Ok(setup_id)
}

fn connector_setup_connect_args(message: &str) -> Result<String> {
    let trimmed = message.trim();
    let without_slash = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let (name, args) = without_slash
        .split_once(' ')
        .map(|(name, args)| (name, args.trim()))
        .unwrap_or((without_slash, ""));
    if name != "connect" {
        anyhow::bail!("start_connector_setup expects a /connect command");
    }
    Ok(args.to_string())
}

fn connector_setup_session_metadata(cwd: std::path::PathBuf, id: Uuid) -> SessionMetadata {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    SessionMetadata {
        id,
        display_name: Some("Connector Setup".to_string()),
        generated_title: None,
        cwd,
        created_at_ms: now,
        updated_at_ms: now,
        parent_session_id: None,
        slug: Some(format!("connector-setup-{}", id.simple())),
        tags: Vec::new(),
        note: None,
    }
}

fn publish_turn_error_event(
    state: &DaemonState,
    channel: &str,
    session_id: &str,
    turn_id: &str,
    error: String,
    error_raw: Option<String>,
    category: Option<&str>,
) {
    let mut payload = json!({
        "type": "turn-error",
        "turnId": turn_id,
        "error": error,
    });
    if let Some(raw) = error_raw {
        payload["errorRaw"] = json!(raw);
    }
    if let Some(category) = category {
        payload["category"] = json!(category);
    }
    state.publish_event(ServerEnvelope::Event {
        event: channel.to_string(),
        payload,
    });
    state.publish_event(ServerEnvelope::Event {
        event: "workspace:sessions:changed".to_string(),
        payload: json!({
            "reason": "turn_error",
            "sessionId": session_id,
        }),
    });
}

fn publish_sessionless_turn_error_event(
    state: &DaemonState,
    channel: &str,
    turn_id: &str,
    error: String,
    category: Option<&str>,
) {
    let mut payload = json!({
        "type": "turn-error",
        "turnId": turn_id,
        "error": error,
    });
    if let Some(category) = category {
        payload["category"] = json!(category);
    }
    state.publish_event(ServerEnvelope::Event {
        event: channel.to_string(),
        payload,
    });
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
///   * `model_gateway_unavailable` — the OpenAI-compatible model endpoint
///     was unreachable or reset the TCP connection before returning a
///     response. The raw transport chain remains in `errorRaw`.
///   * `provider_stream_closed` — provider SSE closed before the OpenAI
///     Responses stream emitted `response.completed`.
///   * `other` — anything we haven't classified.
fn classify_turn_error(err: &anyhow::Error) -> (String, &'static str) {
    // anyhow walks the chain via Display when you `format!("{:#}", err)`
    let chain = format!("{err:#}");
    let lower = chain.to_ascii_lowercase();
    if lower.contains("failed to parse sse response")
        || lower.contains("stream closed before response.completed")
        || lower.contains("idle timeout waiting for sse")
    {
        return (chain, "provider_stream_closed");
    }
    if is_model_gateway_transport_error(&lower) {
        return (
            "the model gateway is temporarily unavailable or reset the connection before \
             responding. retry in a few seconds; if it keeps failing, check the model gateway \
             status or switch models."
                .to_string(),
            "model_gateway_unavailable",
        );
    }
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

fn is_model_gateway_transport_error(lower: &str) -> bool {
    let is_model_endpoint = lower.contains("/v1/responses") || lower.contains("/chat/completions");
    if !is_model_endpoint {
        return false;
    }
    lower.contains("error sending request")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("connect")
        || lower.contains("dns")
        || lower.contains("tls")
        || lower.contains("timeout")
        || lower.contains("timed out")
}

#[derive(Debug, Clone, Default)]
struct TurnRequestOptions {
    model_override: Option<String>,
    provider_id: Option<String>,
    model_id: Option<String>,
    thinking_option_id: Option<String>,
    fast_mode: Option<bool>,
    permission_mode: Option<String>,
}

impl TurnRequestOptions {
    fn from_params(params: &Value) -> Self {
        Self {
            model_override: optional_trimmed_value(params, &["modelOverride", "model_override"]),
            provider_id: optional_trimmed_value(params, &["providerId", "provider_id"])
                .map(|provider_id| canonical_desktop_provider_id(&provider_id)),
            model_id: optional_trimmed_value(params, &["modelId", "model_id"]),
            thinking_option_id: optional_trimmed_value(
                params,
                &["thinkingOptionId", "thinking_option_id", "effort"],
            )
            .filter(|value| value != "default"),
            fast_mode: params
                .get("fastMode")
                .or_else(|| params.get("fast_mode"))
                .and_then(Value::as_bool),
            permission_mode: optional_trimmed_value(params, &["permissionMode", "permission_mode"])
                .and_then(|mode| match mode.as_str() {
                    "read-only" | "workspace-write" | "full-access" => Some(mode),
                    _ => None,
                }),
        }
    }

    fn apply_session_routing(&mut self, routing: DesktopSessionRouting) {
        if self.provider_id.is_none() {
            self.provider_id = routing
                .provider_id
                .map(|provider_id| canonical_desktop_provider_id(&provider_id));
        }
        if self.model_id.is_none() {
            self.model_id = routing.model_id;
        }
    }

    fn has_explicit_routing(&self) -> bool {
        self.model_override.is_some() || self.provider_id.is_some() || self.model_id.is_some()
    }
}

fn optional_trimmed_value(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| params.get(*key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn apply_turn_request_options(
    app_state: &mut AppState,
    providers: &ProviderRegistry,
    options: &TurnRequestOptions,
) -> Result<()> {
    let effort = options
        .thinking_option_id
        .as_deref()
        .unwrap_or(app_state.effort_level.as_str())
        .to_string();
    let fast_mode = options.fast_mode.unwrap_or(app_state.fast_mode);

    if let Some(requested) = options.model_override.as_deref() {
        apply_turn_model_override_with_preferences(
            app_state, providers, requested, &effort, fast_mode,
        )?;
    } else if let Some(model_id) = options.model_id.as_deref() {
        apply_turn_model_selection(
            app_state,
            providers,
            options.provider_id.as_deref(),
            model_id,
            &effort,
            fast_mode,
        )?;
    } else if let Some(provider_id) = options.provider_id.as_deref() {
        apply_turn_provider_selection(app_state, providers, provider_id, &effort, fast_mode)?;
    } else {
        if let Some(effort) = options.thinking_option_id.as_deref() {
            app_state.effort_level = effort.to_string();
            app_state.config.effort_level = Some(effort.to_string());
        }
        if let Some(fast_mode) = options.fast_mode {
            app_state.fast_mode = fast_mode;
            app_state.config.fast_mode = fast_mode;
        }
    }

    if let Some(mode) = options.permission_mode.as_deref() {
        apply_turn_permission_mode(app_state, mode);
    }
    Ok(())
}

fn apply_turn_provider_selection(
    app_state: &mut AppState,
    providers: &ProviderRegistry,
    provider_id: &str,
    effort: &str,
    fast_mode: bool,
) -> Result<()> {
    let provider_id = canonical_desktop_provider_id(provider_id);
    let provider = providers
        .provider(&provider_id)
        .with_context(|| format!("provider {provider_id} not found"))?;
    let model_id = resolve_create_session_model_id(&app_state.config, provider, None)?
        .with_context(|| format!("provider `{provider_id}` has no models"))?;
    apply_turn_model_preferences(app_state, &provider_id, &model_id, effort, fast_mode);
    Ok(())
}

fn apply_turn_model_selection(
    app_state: &mut AppState,
    providers: &ProviderRegistry,
    provider_id: Option<&str>,
    model_id: &str,
    effort: &str,
    fast_mode: bool,
) -> Result<()> {
    let requested = if let Some(provider_id) = provider_id {
        let provider_id = canonical_desktop_provider_id(provider_id);
        let provider = providers
            .provider(&provider_id)
            .with_context(|| format!("provider {provider_id} not found"))?;
        let model_id =
            normalize_provider_model_id(provider, model_id).unwrap_or_else(|| model_id.to_string());
        format!("{provider_id}/{model_id}")
    } else {
        model_id.to_string()
    };
    apply_turn_model_override_with_preferences(app_state, providers, &requested, effort, fast_mode)
}

fn apply_turn_model_override_with_preferences(
    app_state: &mut AppState,
    providers: &ProviderRegistry,
    requested: &str,
    effort: &str,
    fast_mode: bool,
) -> Result<()> {
    let (provider_id, model_id) = resolve_turn_model(providers, requested)?;
    apply_turn_model_preferences(app_state, &provider_id, &model_id, effort, fast_mode);
    Ok(())
}

fn apply_turn_model_preferences(
    app_state: &mut AppState,
    provider_id: &str,
    model_id: &str,
    effort: &str,
    fast_mode: bool,
) {
    app_state.current_provider = Some(provider_id.to_string());
    app_state.current_model = Some(format!("{provider_id}/{model_id}"));
    app_state.config.default_provider = Some(provider_id.to_string());
    app_state.config.default_model = Some(format!("{provider_id}/{model_id}"));
    app_state.effort_level = effort.to_string();
    app_state.config.effort_level = if effort == "auto" {
        None
    } else {
        Some(effort.to_string())
    };
    app_state.fast_mode = fast_mode;
    app_state.config.fast_mode = fast_mode;
}

#[cfg(test)]
fn apply_turn_model_override(
    app_state: &mut AppState,
    providers: &ProviderRegistry,
    requested: &str,
) -> Result<()> {
    let effort = app_state.effort_level.clone();
    let fast_mode = app_state.fast_mode;
    apply_turn_model_override_with_preferences(app_state, providers, requested, &effort, fast_mode)
}

fn resolve_turn_model(providers: &ProviderRegistry, requested: &str) -> Result<(String, String)> {
    let (provider_id, model_id) = if let Some((provider_id, model_id)) = requested.split_once('/') {
        let provider_id = canonical_desktop_provider_id(provider_id);
        let model_id = model_id.trim();
        let provider = providers
            .provider(&provider_id)
            .with_context(|| format!("provider {provider_id} not found"))?;
        let model = provider
            .models
            .iter()
            .find(|model| model.id == model_id)
            .or_else(|| provider.models.iter().find(|model| model.id == requested))
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

    Ok((provider_id, model_id))
}

fn canonical_desktop_provider_id(provider_id: &str) -> String {
    let trimmed = provider_id.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "anthropic" => "anthropic".to_string(),
        "claude" => "anthropic".to_string(),
        "codex" => "openai".to_string(),
        "openai" => "openai".to_string(),
        _ => trimmed.to_string(),
    }
}

fn desktop_provider_ids_match(left: &str, right: &str) -> bool {
    canonical_desktop_provider_id(left) == canonical_desktop_provider_id(right)
}

fn apply_turn_permission_mode(app_state: &mut AppState, permission_mode: &str) {
    match permission_mode {
        "read-only" => {
            app_state.sandbox_mode = "read-only".to_string();
        }
        "full-access" => apply_daemon_yolo_mode(app_state),
        _ => {
            app_state.sandbox_mode = "workspace-write".to_string();
        }
    }
}

fn apply_daemon_yolo_mode(app_state: &mut AppState) {
    app_state.sandbox_mode = "danger-full-access".to_string();
    app_state.grant_all_tools_for_session();
}

#[cfg(test)]
mod tests {
    use super::{
        append_ordered_turn_progress, apply_daemon_yolo_mode, apply_turn_model_override,
        apply_turn_request_options, browser_permission_payload_json, browser_status_for_turn,
        cancel_all_active_turns, connector_setup_connect_args, connector_setup_id, daemon_now_ms,
        desktop_latency_ms, file_media_mime_type, generated_video_handler,
        handle_create_file_media_access, handle_create_generated_video_access,
        handle_create_openai_realtime_client_secret, handle_create_session, handle_generate_media,
        handle_import_external_credential, handle_list_lambda_skill_libraries,
        handle_list_media_capabilities, handle_list_permissions, handle_list_provider_models,
        handle_load_session_detail, handle_local_model_status, handle_login_with_api_key,
        handle_logout_provider, handle_read_generated_media_preview,
        handle_remove_lambda_skill_library, handle_save_lambda_skill_library,
        handle_save_permissions, handle_save_proxy_settings, handle_set_lambda_skill_approval,
        handle_set_lambda_skill_enabled, handle_update_config, model_descriptor_dto,
        parse_single_byte_range, permission_review_payload_json,
        realtime_session_config_from_params, report_cancelled_turn, requires_explicit_subscription,
        resolve_create_session_model_id, run_off_runtime, session_used_browser_tool,
        start_connector_setup_turn, turn_browser_tab_context, CancelToken, ConnectionGuard,
        DaemonState, GenerateMediaArtifactResult, GenerateMediaResult, GeneratedVideoRangeError,
        GeneratedVideoTicket, ServerEnvelope, TurnHandle, TurnProgress, TurnProgressItem,
        TurnRequestOptions,
    };
    use axum::{
        extract::{Path as AxumPath, State},
        http::{header, HeaderMap, HeaderValue, StatusCode},
        response::IntoResponse,
    };
    use indexmap::IndexMap;
    use puffer_config::{
        ensure_workspace_dirs, load_config, ConfigPaths, MediaGenerationConfig, ProxyConfig,
        ProxyEndpoint, ProxyScheme, PufferConfig,
    };
    use puffer_core::{AppState, ModelPreferenceFamily, ToolCallRequest, ToolInvocation};
    use puffer_provider_registry::{
        AuthStore, Modality, ModelDescriptor, ProviderDescriptor, ProviderRegistry,
    };
    use puffer_session_store::{SessionMetadata, SessionStore, TranscriptEvent, TurnBoundaryState};
    use serde_json::{json, Value};
    use std::collections::{BTreeMap, HashMap};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::thread;
    use uuid::Uuid;

    fn discovery_cache_env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn generate_media_result_serializes_artifacts_array() {
        let result = GenerateMediaResult {
            job_id: "job-1".to_string(),
            requested_count: 2,
            artifacts: vec![
                GenerateMediaArtifactResult {
                    artifact_id: "artifact-1".to_string(),
                    index: 0,
                    path: "/tmp/image-1.png".to_string(),
                    mime_type: "image/png".to_string(),
                    size: 10,
                    remote_source_url: None,
                },
                GenerateMediaArtifactResult {
                    artifact_id: "artifact-2".to_string(),
                    index: 1,
                    path: "/tmp/image-2.png".to_string(),
                    mime_type: "image/png".to_string(),
                    size: 11,
                    remote_source_url: None,
                },
            ],
            kind: "image".to_string(),
            provider_id: "openai".to_string(),
            model_id: "gpt-image-1".to_string(),
            status: "succeeded".to_string(),
            prompt: "draw".to_string(),
        };
        let value = serde_json::to_value(result).unwrap();

        assert!(value.get("artifactId").is_none());
        assert!(value.get("path").is_none());
        assert_eq!(value["requestedCount"], 2);
        assert_eq!(value["artifacts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn proxy_test_uses_public_connectivity_endpoint() {
        assert_eq!(
            super::proxy_connectivity_test_urls(),
            &[
                "https://www.gstatic.com/generate_204",
                "https://cp.cloudflare.com/generate_204",
                "https://www.cloudflare.com/cdn-cgi/trace",
            ]
        );
    }

    struct DiscoveryCacheEnvGuard {
        cache_path: std::path::PathBuf,
        previous: Option<std::ffi::OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl DiscoveryCacheEnvGuard {
        fn set() -> Self {
            let lock = discovery_cache_env_lock();
            let previous = std::env::var_os("PUFFER_DISCOVERY_CACHE_PATH");
            let temp = tempfile::NamedTempFile::new().expect("cache file");
            let cache_path = temp.path().to_path_buf();
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_millis() as u64;
            let cache = json!({
                "entries": {
                    "llama-cpp": {"models": [], "cached_at_ms": now_ms},
                    "lmstudio": {"models": [], "cached_at_ms": now_ms},
                    "ollama": {"models": [], "cached_at_ms": now_ms},
                    "vllm": {"models": [], "cached_at_ms": now_ms}
                }
            });
            std::fs::write(&cache_path, cache.to_string()).expect("seed discovery cache");
            let (_, path) = temp.keep().expect("persist cache file");
            std::env::set_var("PUFFER_DISCOVERY_CACHE_PATH", path);
            Self {
                cache_path,
                previous,
                _lock: lock,
            }
        }
    }

    #[test]
    fn lambda_gate_stream_payload_maps_tool_metadata() {
        let invocation = ToolInvocation {
            call_id: "call-1".to_string(),
            tool_id: "LambdaHostCall".to_string(),
            input: "{}".to_string(),
            output: "admitted".to_string(),
            success: true,
            metadata: json!({
                "lambda_skill": {
                    "event": "host_call_admitted",
                    "host_tool": "gh_pr_view",
                    "host_args": {"owner": "acme"},
                    "concrete_tool": "Bash",
                    "concrete_input": {"command": "gh pr view"}
                }
            }),
            terminate: false,
        };

        let payload =
            super::lambda_gate_stream_payload("turn-1", &invocation).expect("lambda gate event");

        assert_eq!(payload["type"], "lambda-gate");
        assert_eq!(payload["turnId"], "turn-1");
        assert_eq!(payload["callId"], "call-1");
        assert_eq!(payload["gateEvent"], "host_call_admitted");
        assert_eq!(payload["hostTool"], "gh_pr_view");
        assert_eq!(payload["concreteTool"], "Bash");
    }

    #[test]
    fn lambda_gate_stream_phase_places_admit_before_commit_after_batch() {
        let admitted = json!({
            "type": "lambda-gate",
            "gateEvent": "host_call_admitted"
        });
        let committed = json!({
            "type": "lambda-gate",
            "gateEvent": "host_call_committed"
        });
        let rejected = json!({
            "type": "lambda-gate",
            "gateEvent": "gate_rejected"
        });

        assert_eq!(
            super::lambda_gate_stream_phase(&admitted),
            super::LambdaGateStreamPhase::BeforeToolInvocations
        );
        assert_eq!(
            super::lambda_gate_stream_phase(&committed),
            super::LambdaGateStreamPhase::AfterToolInvocations
        );
        assert_eq!(
            super::lambda_gate_stream_phase(&rejected),
            super::LambdaGateStreamPhase::AfterToolInvocations
        );
    }

    impl Drop for DiscoveryCacheEnvGuard {
        fn drop(&mut self) {
            match self.previous.as_ref() {
                Some(value) => std::env::set_var("PUFFER_DISCOVERY_CACHE_PATH", value),
                None => std::env::remove_var("PUFFER_DISCOVERY_CACHE_PATH"),
            }
            let _ = std::fs::remove_file(&self.cache_path);
        }
    }

    struct PufferHomeEnvGuard {
        previous_home: Option<std::ffi::OsString>,
        previous_puffer_home: Option<std::ffi::OsString>,
        _temp: tempfile::TempDir,
        _lock: MutexGuard<'static, ()>,
    }

    impl PufferHomeEnvGuard {
        fn set() -> Self {
            let lock = discovery_cache_env_lock();
            let previous_home = std::env::var_os("HOME");
            let previous_puffer_home = std::env::var_os("PUFFER_HOME");
            let temp = tempfile::tempdir().expect("temp puffer home");
            std::env::set_var("HOME", temp.path());
            std::env::set_var("PUFFER_HOME", temp.path());
            Self {
                previous_home,
                previous_puffer_home,
                _temp: temp,
                _lock: lock,
            }
        }
    }

    impl Drop for PufferHomeEnvGuard {
        fn drop(&mut self) {
            match self.previous_home.as_ref() {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match self.previous_puffer_home.as_ref() {
                Some(value) => std::env::set_var("PUFFER_HOME", value),
                None => std::env::remove_var("PUFFER_HOME"),
            }
        }
    }

    struct OpenAiEnvGuard {
        openai_api_key: Option<std::ffi::OsString>,
        puffer_openai_api_key: Option<std::ffi::OsString>,
    }

    impl OpenAiEnvGuard {
        fn set_puffer_key(value: &str) -> Self {
            let guard = Self {
                openai_api_key: std::env::var_os("OPENAI_API_KEY"),
                puffer_openai_api_key: std::env::var_os("PUFFER_OPENAI_API_KEY"),
            };
            std::env::remove_var("OPENAI_API_KEY");
            std::env::set_var("PUFFER_OPENAI_API_KEY", value);
            guard
        }
    }

    impl Drop for OpenAiEnvGuard {
        fn drop(&mut self) {
            match self.openai_api_key.as_ref() {
                Some(value) => std::env::set_var("OPENAI_API_KEY", value),
                None => std::env::remove_var("OPENAI_API_KEY"),
            }
            match self.puffer_openai_api_key.as_ref() {
                Some(value) => std::env::set_var("PUFFER_OPENAI_API_KEY", value),
                None => std::env::remove_var("PUFFER_OPENAI_API_KEY"),
            }
        }
    }

    fn spawn_image_generation_server() -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let address = listener.local_addr().expect("address");
        let handle = thread::spawn(move || {
            let started = std::time::Instant::now();
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(accepted) => break accepted,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if started.elapsed() > std::time::Duration::from_secs(2) {
                            return String::new();
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("generation request: {error}"),
                }
            };
            let mut buffer = [0_u8; 8192];
            let size = stream.read(&mut buffer).expect("read request");
            let request_text = String::from_utf8_lossy(&buffer[..size]).to_string();
            let body = json!({
                "data": [{"b64_json": "aW1hZ2UtYnl0ZXM="}]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("response");
            request_text
        });
        (format!("http://{address}"), handle)
    }

    fn spawn_discovered_chat_image_generation_server() -> (String, thread::JoinHandle<Vec<String>>)
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let address = listener.local_addr().expect("address");
        let handle = thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut requests = Vec::new();
            while requests.len() < 2 {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buffer = [0_u8; 8192];
                        let size = stream.read(&mut buffer).expect("read request");
                        let request_text = String::from_utf8_lossy(&buffer[..size]).to_string();
                        let body = if request_text.starts_with("GET /models") {
                            json!({
                                "data": [{
                                    "id": "openrouter/image-chat",
                                    "name": "Image Chat",
                                    "architecture": {"output_modalities": ["text", "image"]}
                                }]
                            })
                            .to_string()
                        } else {
                            json!({
                                "choices": [{
                                    "message": {
                                        "images": [{"b64_json": "aW1hZ2UtYnl0ZXM="}]
                                    }
                                }]
                            })
                            .to_string()
                        };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        stream.write_all(response.as_bytes()).expect("response");
                        requests.push(request_text);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if started.elapsed() > std::time::Duration::from_secs(2) {
                            return requests;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("media request: {error}"),
                }
            }
            requests
        });
        (format!("http://{address}"), handle)
    }

    fn write_openrouter_resource_override(paths: &ConfigPaths, base_url: &str) {
        let providers_dir = paths.workspace_config_dir.join("resources/providers");
        std::fs::create_dir_all(&providers_dir).expect("provider resources dir");
        std::fs::write(
            providers_dir.join("openrouter.yaml"),
            format!(
                r#"id: openrouter
display_name: OpenRouter
base_url: {base_url}
default_api: openai-completions
auth_modes:
  - api_key
media:
  image:
    discovery:
      adapter: trusted_image_output
      path: /models
      query:
        output_modalities: image
    execution:
      adapter: chat_image_output
      path: /chat/completions
models:
  - id: auto
    display_name: Auto
    provider: openrouter
    api: openai-completions
    context_window: 200000
    max_output_tokens: 16384
"#
            ),
        )
        .expect("write openrouter override");
    }

    fn write_replicate_video_resource_override(paths: &ConfigPaths) {
        let providers_dir = paths.workspace_config_dir.join("resources/providers");
        std::fs::create_dir_all(&providers_dir).expect("provider resources dir");
        std::fs::write(
            providers_dir.join("replicate.yaml"),
            r#"id: replicate
display_name: Replicate
base_url: https://api.replicate.com
default_api: openai-completions
auth_modes:
  - api_key
media:
  video:
    discovery:
      adapter: static
    execution:
      adapter: replicate_video
      path: /v1/predictions
    models:
      - id: owner/model-version
        display_name: Video Model
        operations:
          - generate
        axes:
          - { id: aspect_ratio, label: Aspect ratio, role: param, control: !enum { values: ["16:9", "9:16"], default: "16:9" }, request_field: aspect_ratio }
          - { id: duration_seconds, label: Duration, role: param, control: !enum { values: ["5", "8"], default: "5" }, request_field: duration }
        variants: { model_id: owner/model-version }
models: []
"#,
        )
        .expect("write replicate override");
    }

    fn write_relaydance_video_resource_override(paths: &ConfigPaths) {
        let providers_dir = paths.workspace_config_dir.join("resources/providers");
        std::fs::create_dir_all(&providers_dir).expect("provider resources dir");
        std::fs::write(
            providers_dir.join("relaydance.yaml"),
            r#"id: relaydance
display_name: Relaydance
base_url: https://relaydance.com
default_api: openai-completions
auth_modes:
  - api_key
media:
  video:
    discovery:
      adapter: static
    execution:
      adapter: relaydance_video
      path: /v1/video/generations
    models:
      - id: doubao-seedance-2-0-720p
        display_name: Seedance 2.0
        operations:
          - generate
        axes:
          - { id: duration_seconds, label: Duration, role: param, control: !enum { values: ["4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15"], default: "5" }, request_field: seconds }
          - { id: resolution, label: Resolution, role: param, control: !enum { values: ["480p", "720p", "1080p"], default: "720p" }, request_field: metadata.resolution }
          - { id: aspect_ratio, label: Aspect ratio, role: param, control: !enum { values: ["16:9", "4:3", "1:1", "3:4", "9:16", "21:9", "adaptive"], default: "16:9" }, request_field: metadata.ratio }
        variants: { model_id: doubao-seedance-2-0-720p }
models: []
"#,
        )
        .expect("write relaydance override");
    }

    fn daemon_state_with_relaydance_video_capability(
    ) -> (PufferHomeEnvGuard, tempfile::TempDir, DaemonState) {
        let home_guard = PufferHomeEnvGuard::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        write_relaydance_video_resource_override(&paths);
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("relaydance", "sk-test");
        auth_store
            .save(&paths.user_config_dir.join("auth.json"))
            .expect("save auth");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");
        (home_guard, temp, state)
    }

    fn daemon_state_with_replicate_video_capability(
    ) -> (PufferHomeEnvGuard, tempfile::TempDir, DaemonState) {
        let home_guard = PufferHomeEnvGuard::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        write_replicate_video_resource_override(&paths);
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("replicate", "sk-test");
        auth_store
            .save(&paths.user_config_dir.join("auth.json"))
            .expect("save auth");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");
        (home_guard, temp, state)
    }

    /// End-to-end through the real `browser_open` RPC handler + a real
    /// `DaemonState`: native-CEF pool exhaustion (issue #603) must self-heal by
    /// reclaiming a slot, not hard-fail. Backs the daemon-RPC verification.
    #[test]
    fn browser_open_rpc_self_heals_native_cef_pool_exhaustion() {
        use crate::daemon_browser::test_support::{cef_env_lock, FakeCefDevtools};

        let _cef_guard = cef_env_lock().lock().unwrap();
        let _home_guard = PufferHomeEnvGuard::set();
        let previous_port = std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT");
        let previous_profile = std::env::var_os("PUFFER_CEF_PROFILE_DIR");

        // Fake native CEF with exactly two prewarmed page slots.
        let cef = FakeCefDevtools::spawn(2);
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", cef.port.to_string());
        std::env::set_var(
            "PUFFER_CEF_PROFILE_DIR",
            paths.user_config_dir.join("cef-profile"),
        );

        let state = Arc::new(
            DaemonState::load(workspace_root, paths, "token".into(), false, false, false)
                .expect("daemon state"),
        );
        let open = |session_id: &str| {
            crate::daemon_browser::handle_browser_open(&state, &json!({ "sessionId": session_id }))
        };

        open("sess-a:browser:t1").expect("browser_open A allocates the first prewarm slot");
        std::thread::sleep(std::time::Duration::from_millis(40));
        open("sess-b:browser:t1").expect("browser_open B allocates the second prewarm slot");
        std::thread::sleep(std::time::Duration::from_millis(40));

        // Pool exhausted: the third browser_open RPC must self-heal by reclaiming
        // the least-recently-active slot instead of returning "no available
        // prewarmed page targets".
        let third = open("sess-c:browser:t1");

        match previous_port {
            Some(value) => std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", value),
            None => std::env::remove_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT"),
        }
        match previous_profile {
            Some(value) => std::env::set_var("PUFFER_CEF_PROFILE_DIR", value),
            None => std::env::remove_var("PUFFER_CEF_PROFILE_DIR"),
        }

        assert!(
            third.is_ok(),
            "third browser_open RPC should self-heal native-CEF pool exhaustion, got {:?}",
            third.err()
        );
    }

    fn empty_turn_handle(cancel: CancelToken) -> TurnHandle {
        TurnHandle {
            session_id: None,
            session_uuid: None,
            channel: "agent".to_string(),
            message: String::new(),
            attachments: Vec::new(),
            cancel,
            cancel_reported: Arc::new(AtomicBool::new(false)),
            user_prompt_persisted: Arc::new(AtomicBool::new(false)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            pending_questions: Arc::new(Mutex::new(HashMap::new())),
            progress: Arc::new(Mutex::new(TurnProgress::default())),
        }
    }

    fn test_daemon_state() -> DaemonState {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state")
    }

    fn insert_session_turn(state: &DaemonState, session_id: Uuid, turn_id: &str) {
        let cancel = CancelToken::new();
        let mut handle = empty_turn_handle(cancel);
        handle.session_id = Some(session_id.to_string());
        handle.session_uuid = Some(session_id);
        handle.channel = format!("session:{session_id}:event");
        state
            .turns
            .lock()
            .unwrap()
            .insert(turn_id.to_string(), handle);
    }

    #[test]
    fn load_session_detail_reports_active_turn_from_registry() {
        let state = test_daemon_state();
        let session_store = SessionStore::from_paths(&state.paths).unwrap();
        let session = session_store
            .create_session(state.cwd_path().to_path_buf())
            .unwrap();
        insert_session_turn(&state, session.id, "t-live");

        let detail = handle_load_session_detail(
            &state,
            &json!({
                "sessionId": session.id.to_string(),
            }),
        )
        .unwrap();

        assert_eq!(detail.get("activeTurnId"), Some(&json!("t-live")));
    }

    #[test]
    fn load_session_detail_does_not_infer_active_turn_from_started_boundary() {
        let state = test_daemon_state();
        let session_store = SessionStore::from_paths(&state.paths).unwrap();
        let session = session_store
            .create_session(state.cwd_path().to_path_buf())
            .unwrap();
        session_store
            .append_event(
                session.id,
                TranscriptEvent::TurnBoundary {
                    turn_id: "persisted-start-only".to_string(),
                    state: TurnBoundaryState::Started,
                },
            )
            .unwrap();
        session_store
            .append_event(
                session.id,
                TranscriptEvent::UserMessage {
                    text: "hello".to_string(),
                    attachments: Vec::new(),
                    actor: None,
                },
            )
            .unwrap();

        let detail = handle_load_session_detail(
            &state,
            &json!({
                "sessionId": session.id.to_string(),
            }),
        )
        .unwrap();

        assert_eq!(detail.get("activeTurnId"), Some(&Value::Null));
    }

    #[test]
    fn load_session_detail_omits_active_turn_for_legacy_transcript_without_boundaries() {
        let state = test_daemon_state();
        let session_store = SessionStore::from_paths(&state.paths).unwrap();
        let session = session_store
            .create_session(state.cwd_path().to_path_buf())
            .unwrap();
        session_store
            .append_event(
                session.id,
                TranscriptEvent::UserMessage {
                    text: "legacy prompt".to_string(),
                    attachments: Vec::new(),
                    actor: None,
                },
            )
            .unwrap();
        session_store
            .append_event(
                session.id,
                TranscriptEvent::AssistantMessage {
                    text: "legacy answer".to_string(),
                    actor: None,
                },
            )
            .unwrap();

        let detail = handle_load_session_detail(
            &state,
            &json!({
                "sessionId": session.id.to_string(),
            }),
        )
        .unwrap();

        assert!(detail.get("activeTurnId").is_none());
    }

    fn bash_invocation(command: &str) -> TranscriptEvent {
        TranscriptEvent::ToolInvocation {
            call_id: "c1".to_string(),
            tool_id: "Bash".to_string(),
            input: json!({ "command": command }).to_string(),
            output: "ok".to_string(),
            success: true,
            metadata: None,
            actor: None,
            subject: None,
        }
    }

    /// Gate signal for issue #560: replayed transcripts reveal browser usage
    /// either through the Browser/BrowserAction tools or through Bash running
    /// the `browser ...` CLI (the browser skill's documented path).
    #[test]
    fn session_used_browser_tool_detects_browser_usage_in_transcript() {
        assert!(session_used_browser_tool(&[bash_invocation(
            "browser open https://example.com --label docs"
        )]));
        assert!(session_used_browser_tool(&[bash_invocation(
            "browser --json snapshot --tab-id t1"
        )]));
        assert!(session_used_browser_tool(&[bash_invocation(
            "puffer internal-tool browser list"
        )]));
        let typed_tool = TranscriptEvent::ToolInvocation {
            call_id: "c2".to_string(),
            tool_id: "BrowserAction".to_string(),
            input: "{}".to_string(),
            output: "ok".to_string(),
            success: true,
            metadata: None,
            actor: None,
            subject: None,
        };
        assert!(session_used_browser_tool(&[typed_tool]));

        assert!(!session_used_browser_tool(&[bash_invocation("ls -la")]));
        assert!(!session_used_browser_tool(&[bash_invocation(
            "git branch --list"
        )]));
        assert!(!session_used_browser_tool(&[
            TranscriptEvent::UserMessage {
                text: "please open the browser".to_string(),
                attachments: Vec::new(),
                actor: None,
            }
        ]));
    }

    /// The gate must parse per shell segment: compound/env-prefixed commands
    /// that really invoke the browser CLI count, while commands that merely
    /// mention the strings (grep/echo in coding sessions) must not.
    #[test]
    fn session_used_browser_tool_parses_bash_per_segment() {
        assert!(session_used_browser_tool(&[bash_invocation(
            "cd /tmp && browser open https://example.com"
        )]));
        assert!(session_used_browser_tool(&[bash_invocation(
            "FOO=1 browser snapshot --tab-id t1"
        )]));
        assert!(session_used_browser_tool(&[bash_invocation(
            "/usr/local/bin/puffer internal-tool browser list"
        )]));
        assert!(session_used_browser_tool(&[bash_invocation(
            "browser list; git status"
        )]));

        assert!(!session_used_browser_tool(&[bash_invocation(
            "grep -rn 'internal-tool browser' crates/"
        )]));
        assert!(!session_used_browser_tool(&[bash_invocation(
            "echo \"use internal-tool browser for tabs\""
        )]));
        assert!(!session_used_browser_tool(&[bash_invocation(
            "cat docs/browser.md"
        )]));
    }

    /// Bash `browser ...` CLI tabs register under the workspace-stable
    /// cli-browser id, not the chat session UUID. The turn lookup must check
    /// both keyspaces or live CLI tabs get reported as "browser is gone".
    #[test]
    fn turn_browser_tab_context_falls_back_to_workspace_cli_keyspace() {
        let _home_guard = PufferHomeEnvGuard::set();
        let state = test_daemon_state();
        let chat_root = Uuid::new_v4().to_string();

        let empty = turn_browser_tab_context(&state, &chat_root);
        assert!(!empty.has_active_tab(), "fresh registry has no tab");

        let cli_id =
            crate::daemon_browser::default_cli_session_id(&state.paths).expect("cli session id");
        state
            .browsers
            .test_record_tab(&cli_id, "t1", "https://example.com/cart", "Cart");

        let context = turn_browser_tab_context(&state, &chat_root);
        assert!(
            context.has_active_tab(),
            "live CLI tab must be visible through the chat-session lookup"
        );
        assert_eq!(context.url.as_deref(), Some("https://example.com/cart"));
    }

    /// Issue #560: a restarted daemon has an empty tab registry, so a resumed
    /// session whose transcript used the browser must get a stale-state
    /// reminder; sessions that never touched the browser stay clean.
    #[test]
    fn browser_status_for_turn_gates_on_history_and_live_tab() {
        use crate::daemon_browser::{BrowserCurrentTabContext, BrowserTabInfo};

        let no_tab = BrowserCurrentTabContext::no_active_tab();
        assert_eq!(browser_status_for_turn(&no_tab, false), None);

        let stale = browser_status_for_turn(&no_tab, true).expect("inject for browser session");
        assert!(stale.contains("No browser tab"), "status: {stale}");

        let live_tab = BrowserTabInfo {
            tab_id: "t1".to_string(),
            label: "Tab 1".to_string(),
            url: "https://example.com/cart".to_string(),
            title: "Cart".to_string(),
            loading: false,
            connected: true,
            active: true,
            backend_session_id: "sess:browser:t1".to_string(),
            native_cef_session_id: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let live = BrowserCurrentTabContext::from_tab(&live_tab);
        let status =
            browser_status_for_turn(&live, false).expect("live tab injects even without history");
        assert!(
            status.contains("https://example.com/cart"),
            "status: {status}"
        );
    }

    /// Core of the client-disconnect watchdog (issue #600): orphaned interactive
    /// turns in the registry get their cancel token flipped and are removed.
    #[test]
    fn cancel_all_active_turns_cancels_orphaned_interactive_turns() {
        let _home_guard = PufferHomeEnvGuard::set();
        let state = test_daemon_state();

        let cancel = CancelToken::new();
        let observer = cancel.clone();
        state
            .turns
            .lock()
            .unwrap()
            .insert("turn-1".to_string(), empty_turn_handle(cancel));

        assert!(!observer.is_cancelled());
        let cancelled = cancel_all_active_turns(&state);

        assert_eq!(cancelled, 1, "the orphaned interactive turn is cancelled");
        assert!(observer.is_cancelled(), "its cancel token must be flipped");
        assert!(
            state.turns.lock().unwrap().is_empty(),
            "cancelled turns are removed from the registry"
        );
    }

    /// The live-client counter that arms the watchdog (issue #600): it tracks
    /// open connections and dropping the last guard returns cleanly even without
    /// a Tokio runtime (the watchdog is simply not armed).
    #[test]
    fn connection_guard_tracks_live_client_count() {
        let _home_guard = PufferHomeEnvGuard::set();
        let state = Arc::new(test_daemon_state());
        assert_eq!(state.live_connections.load(Ordering::SeqCst), 0);

        let first = ConnectionGuard::new(state.clone());
        let second = ConnectionGuard::new(state.clone());
        assert_eq!(state.live_connections.load(Ordering::SeqCst), 2);

        drop(second);
        assert_eq!(state.live_connections.load(Ordering::SeqCst), 1);
        drop(first); // last client gone — decrements to 0 without panicking
        assert_eq!(state.live_connections.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn high_volume_browser_events_require_explicit_subscription() {
        assert!(requires_explicit_subscription(
            "browser:session:browser:tab-1:frame"
        ));
        assert!(requires_explicit_subscription("browser:session:recording"));
        assert!(!requires_explicit_subscription(
            "browser:session:browser:tab-1:state"
        ));
        assert!(!requires_explicit_subscription("browser:session:tabs"));
        assert!(!requires_explicit_subscription("session:abc:events"));
        assert!(!requires_explicit_subscription(
            "workspace:sessions:changed"
        ));
    }

    #[test]
    fn local_model_status_handler_returns_qwen35_contract() {
        let _home_guard = PufferHomeEnvGuard::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let status =
            handle_local_model_status(&state, &json!({"modelId": "qwen35"})).expect("status");

        assert_eq!(status["id"], "qwen35");
        assert_eq!(status["modelId"], "qwen3.5-0.8b");
        assert_eq!(status["displayName"], "Qwen3.5-0.8B (local)");
        assert!(status["checks"]
            .as_array()
            .is_some_and(|checks| !checks.is_empty()));
    }

    #[test]
    fn media_capabilities_handler_returns_openai_image_capability() {
        let _home_guard = PufferHomeEnvGuard::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("codex", "sk-test");
        auth_store
            .save(&paths.user_config_dir.join("auth.json"))
            .expect("save auth");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let response =
            handle_list_media_capabilities(&state, &json!({"kind": "image"})).expect("response");
        let capabilities = response["capabilities"].as_array().expect("capabilities");
        let ids = capabilities
            .iter()
            .map(|capability| capability["modelId"].as_str().expect("model id"))
            .collect::<Vec<_>>();

        assert!(ids.contains(&"gpt-image-1"));
        assert!(!ids.contains(&"auto"));
        assert!(capabilities.iter().all(|capability| {
            capability["providerId"] == "openai"
                && capability["kind"] == "image"
                && capability["operation"] == "generate"
                && capability["adapter"] == "images_json"
                && capability["status"] == "available"
        }));
        let gpt_image = capabilities
            .iter()
            .find(|capability| capability["modelId"] == "gpt-image-1")
            .expect("gpt-image-1 capability");
        let axes = gpt_image["axes"].as_array().expect("axes");
        assert!(axes
            .iter()
            .all(|axis| axis.get("requestField").is_none() && axis.get("wireType").is_none()));
        assert!(axes.iter().any(|axis| {
            axis["id"] == "mode"
                && axis["label"] == "Mode"
                && axis["role"] == "param"
                && axis["control"]["enum"]["default"] == "1K SD"
        }));
        assert!(axes.iter().any(|axis| {
            axis["id"] == "ratio"
                && axis["label"] == "Ratio"
                && axis["control"]["enum"]["default"] == "Auto"
        }));
        assert!(axes.iter().any(|axis| {
            axis["id"] == "output"
                && axis["label"] == "Output"
                && axis["control"]["range"]["min"] == 1.0
                && axis["control"]["range"]["max"] == 9.0
        }));
    }

    #[test]
    fn media_capabilities_handler_hides_openai_without_auth_store_even_with_env_key() {
        let _home_guard = PufferHomeEnvGuard::set();
        let _openai_env_guard = OpenAiEnvGuard::set_puffer_key("sk-test");
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let response =
            handle_list_media_capabilities(&state, &json!({"kind": "image"})).expect("response");
        let capabilities = response["capabilities"].as_array().expect("capabilities");

        assert!(capabilities.is_empty());
    }

    #[test]
    fn media_capabilities_handler_hides_openai_without_auth_store_even_with_codex_file() {
        let _home_guard = PufferHomeEnvGuard::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let codex_auth_path = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .expect("home")
            .join(".codex")
            .join("auth.json");
        std::fs::create_dir_all(codex_auth_path.parent().expect("codex auth parent"))
            .expect("codex auth dir");
        std::fs::write(&codex_auth_path, "{}").expect("codex auth file");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let response =
            handle_list_media_capabilities(&state, &json!({"kind": "image"})).expect("response");
        let capabilities = response["capabilities"].as_array().expect("capabilities");

        assert!(capabilities.is_empty());
    }

    #[test]
    fn media_capabilities_handler_exposes_worldrouter_image_models() {
        let _home_guard = PufferHomeEnvGuard::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("worldrouter", "sk-test");
        auth_store
            .save(&paths.user_config_dir.join("auth.json"))
            .expect("save auth");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let response =
            handle_list_media_capabilities(&state, &json!({"kind": "image"})).expect("response");
        let capabilities = response["capabilities"].as_array().expect("capabilities");
        let models_by_id = capabilities
            .iter()
            .map(|capability| {
                (
                    capability["modelId"].as_str().expect("model id"),
                    capability["adapter"].as_str().expect("adapter"),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(
            models_by_id,
            std::collections::BTreeMap::from([
                ("gemini-2.5-flash-image", "gemini_generate_content"),
                ("gemini-3-pro-image-preview", "gemini_generate_content"),
                ("gemini-3.1-flash-image-preview", "gemini_generate_content"),
                ("gpt-image-2", "images_json"),
            ])
        );
        assert!(capabilities
            .iter()
            .all(|capability| capability["status"] == "available"));
    }

    #[test]
    fn media_capabilities_handler_returns_connected_static_cloud_image_providers() {
        let _home_guard = PufferHomeEnvGuard::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let mut auth_store = AuthStore::default();
        for provider in [
            "byteplus",
            "zhipu",
            "xai",
            "vercel-ai-gateway",
            "worldrouter",
        ] {
            auth_store.set_api_key(provider, "sk-test");
        }
        auth_store
            .save(&paths.user_config_dir.join("auth.json"))
            .expect("save auth");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let response =
            handle_list_media_capabilities(&state, &json!({"kind": "image"})).expect("response");
        let capabilities = response["capabilities"].as_array().expect("capabilities");
        let provider_ids = capabilities
            .iter()
            .map(|capability| capability["providerId"].as_str().expect("provider id"))
            .collect::<std::collections::BTreeSet<_>>();

        assert!(provider_ids.contains("byteplus"));
        assert!(provider_ids.contains("zhipu"));
        assert!(provider_ids.contains("xai"));
        assert!(provider_ids.contains("vercel-ai-gateway"));
        assert!(provider_ids.contains("worldrouter"));
        assert!(capabilities
            .iter()
            .all(|capability| capability["status"] == "available"));
    }

    #[test]
    fn daemon_list_media_capabilities_returns_video_capability() {
        let (_home_guard, _temp, state) = daemon_state_with_replicate_video_capability();

        let response =
            handle_list_media_capabilities(&state, &json!({"kind": "video"})).expect("response");
        let capabilities = response["capabilities"].as_array().expect("capabilities");

        let capability = capabilities
            .iter()
            .find(|capability| capability["providerId"] == "replicate")
            .expect("replicate video capability");

        assert_eq!(capability["adapter"], "replicate_video");
        assert_eq!(capability["status"], "available");
        assert!(capability.get("parameters").is_none());
        assert!(capability.get("defaults").is_none());

        let axes = capability["axes"].as_array().expect("axes");
        assert!(axes
            .iter()
            .all(|axis| axis.get("requestField").is_none() && axis.get("wireType").is_none()));
        assert!(axes.iter().any(|axis| {
            axis["id"] == "aspect_ratio"
                && axis["label"] == "Ratio"
                && axis["role"] == "param"
                && axis["control"]["enum"]["default"] == "16:9"
        }));
        assert!(axes.iter().any(|axis| {
            axis["id"] == "duration_seconds"
                && axis["label"] == "Duration"
                && axis["control"]["enum"]["default"] == "5"
        }));
    }

    #[test]
    fn daemon_list_media_capabilities_returns_relaydance_video_capability() {
        let (_home_guard, _temp, state) = daemon_state_with_relaydance_video_capability();

        let response =
            handle_list_media_capabilities(&state, &json!({"kind": "video"})).expect("response");
        let capabilities = response["capabilities"].as_array().expect("capabilities");

        let capability = capabilities
            .iter()
            .find(|capability| capability["providerId"] == "relaydance")
            .expect("relaydance video capability");
        assert!(capability.get("parameters").is_none());
        assert!(capability.get("defaults").is_none());
        let axes = capability["axes"].as_array().expect("axes");
        assert!(axes
            .iter()
            .all(|axis| axis.get("requestField").is_none() && axis.get("wireType").is_none()));
        assert!(axes.iter().any(|axis| {
            axis["id"] == "resolution"
                && axis["label"] == "Mode"
                && axis["control"]["enum"]["default"] == "720p"
        }));
        assert!(axes.iter().any(|axis| {
            axis["id"] == "aspect_ratio"
                && axis["label"] == "Ratio"
                && axis["control"]["enum"]["default"] == "16:9"
        }));
    }

    #[test]
    fn daemon_generate_media_requires_available_video_model_setting() {
        let (_home_guard, _temp, state) = daemon_state_with_replicate_video_capability();
        {
            let mut config = state.config.lock().unwrap();
            config.media.video = Some(MediaGenerationConfig {
                provider_id: "replicate".to_string(),
                logical_model_id: "owner/unknown-model".to_string(),
                selections: BTreeMap::from([
                    ("aspect_ratio".to_string(), "16:9".to_string()),
                    ("duration_seconds".to_string(), "5".to_string()),
                ]),
            });
        }

        let error = handle_generate_media(&state, &json!({"kind": "video", "prompt": "animate"}))
            .unwrap_err()
            .to_string();

        assert!(error.contains("unknown media model"), "{error}");
    }

    #[test]
    fn generate_media_rejects_stale_image_config_before_http_request() {
        let _home_guard = PufferHomeEnvGuard::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("openai", "sk-test");
        auth_store
            .save(&paths.user_config_dir.join("auth.json"))
            .expect("save auth");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");
        {
            let mut config = state.config.lock().unwrap();
            config.openai_base_url = Some("http://127.0.0.1:9".to_string());
            config.media.image = Some(MediaGenerationConfig {
                provider_id: "openai".to_string(),
                logical_model_id: "stale-image".to_string(),
                selections: BTreeMap::new(),
            });
        }

        let error =
            handle_generate_media(&state, &json!({"kind": "image", "prompt": "draw an icon"}))
                .expect_err("stale config should fail");

        assert!(error.to_string().contains("stale-image"), "{error}");
    }

    #[test]
    fn generate_media_returns_succeeded_image_artifact_metadata() {
        let _home_guard = PufferHomeEnvGuard::set();
        let (base_url, server) = spawn_image_generation_server();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("openai", "sk-test");
        auth_store
            .save(&paths.user_config_dir.join("auth.json"))
            .expect("save auth");
        let state = DaemonState::load(
            workspace_root.clone(),
            paths,
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");
        {
            let mut config = state.config.lock().unwrap();
            config.openai_base_url = Some(base_url);
            config.media.image = Some(MediaGenerationConfig {
                provider_id: "openai".to_string(),
                logical_model_id: "gpt-image-1".to_string(),
                selections: BTreeMap::from([
                    ("mode".to_string(), "1K SD".to_string()),
                    ("ratio".to_string(), "1:1".to_string()),
                    ("output".to_string(), "1".to_string()),
                ]),
            });
        }

        let response =
            handle_generate_media(&state, &json!({"kind": "image", "prompt": "draw an icon"}))
                .expect("generation response");

        let request_text = server.join().expect("server");
        assert!(request_text.contains("\"model\":\"gpt-image-1\""));
        assert!(request_text.contains("\"size\":\"1024x1024\""));
        assert_eq!(response["status"], "succeeded");
        assert_eq!(response["providerId"], "openai");
        assert_eq!(response["modelId"], "gpt-image-1");
        assert!(response.get("artifactId").is_none());
        assert!(response.get("path").is_none());
        assert_eq!(response["requestedCount"], 1);
        let artifact = &response["artifacts"][0];
        assert!(artifact["artifactId"].as_str().is_some());
        let path = artifact["path"].as_str().expect("artifact path");
        assert!(path.contains(".puffer/media/images"));
        assert_eq!(std::fs::read(path).expect("artifact bytes"), b"image-bytes");
        assert!(workspace_root.join(".puffer/media/jobs").is_dir());
        assert!(workspace_root
            .join(".puffer/media/artifact-sidecars")
            .is_dir());
    }

    fn test_state_with_paths(paths: ConfigPaths) -> DaemonState {
        DaemonState::load(
            paths.workspace_root.clone(),
            paths,
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state")
    }

    fn write_generated_image_artifact(
        workspace: &std::path::Path,
        artifact_id: &str,
        filename: &str,
        bytes: &[u8],
    ) {
        let image_dir = workspace.join(".puffer/media/images").join(artifact_id);
        std::fs::create_dir_all(&image_dir).unwrap();
        let image_path = image_dir.join(filename);
        std::fs::write(&image_path, bytes).unwrap();
        let sidecar_dir = workspace.join(".puffer/media/artifact-sidecars");
        std::fs::create_dir_all(&sidecar_dir).unwrap();
        std::fs::write(
            sidecar_dir.join(format!("{artifact_id}.json")),
            serde_json::to_string_pretty(&serde_json::json!({
                "id": artifact_id,
                "jobId": "job-1",
                "kind": "image",
                "path": image_path,
                "mimeType": "image/jpeg",
                "byteCount": bytes.len(),
                "metadata": {},
                "createdAtMs": 1
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_generated_video_artifact(
        workspace: &std::path::Path,
        artifact_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> std::path::PathBuf {
        let video_dir = workspace.join(".puffer/media/videos").join(artifact_id);
        std::fs::create_dir_all(&video_dir).unwrap();
        let video_path = video_dir.join(filename);
        std::fs::write(&video_path, bytes).unwrap();
        let sidecar_dir = workspace.join(".puffer/media/artifact-sidecars");
        std::fs::create_dir_all(&sidecar_dir).unwrap();
        std::fs::write(
            sidecar_dir.join(format!("{artifact_id}.json")),
            serde_json::to_string_pretty(&serde_json::json!({
                "id": artifact_id,
                "jobId": "job-video-1",
                "kind": "video",
                "path": video_path,
                "mimeType": "video/mp4",
                "byteCount": bytes.len(),
                "metadata": {},
                "createdAtMs": 1
            }))
            .unwrap(),
        )
        .unwrap();
        video_path
    }

    fn write_generated_video_artifact_with_poster(
        workspace: &std::path::Path,
        artifact_id: &str,
        filename: &str,
        bytes: &[u8],
        poster_bytes: &[u8],
    ) {
        let video_dir = workspace.join(".puffer/media/videos").join(artifact_id);
        std::fs::create_dir_all(&video_dir).unwrap();
        let video_path = video_dir.join(filename);
        std::fs::write(&video_path, bytes).unwrap();
        let poster_dir = workspace.join(".puffer/media/artifacts").join(artifact_id);
        std::fs::create_dir_all(&poster_dir).unwrap();
        let poster_path = poster_dir.join("poster.jpg");
        std::fs::write(&poster_path, poster_bytes).unwrap();
        let sidecar_dir = workspace.join(".puffer/media/artifact-sidecars");
        std::fs::create_dir_all(&sidecar_dir).unwrap();
        std::fs::write(
            sidecar_dir.join(format!("{artifact_id}.json")),
            serde_json::to_string_pretty(&serde_json::json!({
                "id": artifact_id,
                "jobId": "job-video-1",
                "kind": "video",
                "path": video_path,
                "mimeType": "video/mp4",
                "byteCount": bytes.len(),
                "metadata": {},
                "preview": {
                    "kind": "poster",
                    "state": "available",
                    "path": poster_path,
                    "mimeType": "image/jpeg",
                    "byteCount": poster_bytes.len()
                },
                "createdAtMs": 1
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn read_generated_media_preview_resolves_session_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let workspace = temp.path().join("other-workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let session = session_store.create_session(workspace.clone()).unwrap();
        write_generated_image_artifact(&workspace, "artifact-1", "image.jpeg", b"\xff\xd8\xff\xd9");
        let state = test_state_with_paths(paths);

        let response = handle_read_generated_media_preview(
            &state,
            &serde_json::json!({
                "sessionId": session.id.to_string(),
                "artifactId": "artifact-1"
            }),
        )
        .unwrap();

        assert_eq!(response["state"], "available");
        assert_eq!(response["mimeType"], "image/jpeg");
    }

    #[test]
    fn read_generated_media_preview_returns_video_poster_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let workspace = temp.path().join("other-workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let session = session_store.create_session(workspace.clone()).unwrap();
        write_generated_video_artifact_with_poster(
            &workspace,
            "artifact-video-1",
            "generated.mp4",
            b"mp4-bytes",
            b"\xff\xd8\xff\xd9",
        );
        let state = test_state_with_paths(paths);

        let response = handle_read_generated_media_preview(
            &state,
            &serde_json::json!({
                "sessionId": session.id.to_string(),
                "artifactId": "artifact-video-1"
            }),
        )
        .unwrap();
        let bytes: Vec<u8> = serde_json::from_value(response["bytes"].clone()).unwrap();

        assert_eq!(response["state"], "available");
        assert_eq!(response["mimeType"], "image/jpeg");
        assert_eq!(bytes, b"\xff\xd8\xff\xd9");
    }

    #[test]
    fn file_media_mime_type_maps_video_extensions() {
        use std::path::Path;
        assert_eq!(
            file_media_mime_type(Path::new("/a/clip.mp4")),
            Some("video/mp4")
        );
        assert_eq!(
            file_media_mime_type(Path::new("/a/clip.m4v")),
            Some("video/mp4")
        );
        assert_eq!(
            file_media_mime_type(Path::new("/a/clip.webm")),
            Some("video/webm")
        );
        assert_eq!(
            file_media_mime_type(Path::new("/a/clip.ogv")),
            Some("video/ogg")
        );
        assert_eq!(
            file_media_mime_type(Path::new("/a/clip.MOV")),
            Some("video/quicktime")
        );
        assert_eq!(file_media_mime_type(Path::new("/a/song.ogg")), None);
        assert_eq!(file_media_mime_type(Path::new("/a/notes.txt")), None);
        assert_eq!(file_media_mime_type(Path::new("/a/noext")), None);
    }

    #[test]
    fn create_file_media_access_returns_ticket_for_video() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let workspace_root = paths.workspace_root.clone();
        std::fs::create_dir_all(&workspace_root).unwrap();
        let clip = workspace_root.join("clip.mp4");
        std::fs::write(&clip, b"mp4-bytes").unwrap();
        let state = test_state_with_paths(paths);

        let response = handle_create_file_media_access(
            &state,
            &serde_json::json!({ "path": clip.display().to_string() }),
        )
        .unwrap();

        assert_eq!(response["state"], "available");
        assert_eq!(response["mimeType"], "video/mp4");
        assert_eq!(response["size"], 9);
        assert!(response["expiresAtMs"].as_u64().unwrap() > daemon_now_ms());
        let path = response["path"].as_str().expect("ticket path");
        assert!(path.starts_with("/media/generated-video/"));
    }

    #[test]
    fn create_file_media_access_rejects_directory() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let dir = paths.workspace_root.join("clip.mp4");
        std::fs::create_dir_all(&dir).unwrap();
        let state = test_state_with_paths(paths);

        let response = handle_create_file_media_access(
            &state,
            &serde_json::json!({ "path": dir.display().to_string() }),
        )
        .unwrap();
        assert_eq!(response["state"], "missing");
    }

    #[test]
    fn create_file_media_access_rejects_non_video() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let workspace_root = paths.workspace_root.clone();
        std::fs::create_dir_all(&workspace_root).unwrap();
        let doc = workspace_root.join("notes.txt");
        std::fs::write(&doc, b"hello").unwrap();
        let state = test_state_with_paths(paths);

        let response = handle_create_file_media_access(
            &state,
            &serde_json::json!({ "path": doc.display().to_string() }),
        )
        .unwrap();
        assert_eq!(response["state"], "unsupported");
    }

    #[test]
    fn create_file_media_access_rejects_missing_path() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let missing = paths.workspace_root.join("nope.mp4");
        let state = test_state_with_paths(paths);

        let response = handle_create_file_media_access(
            &state,
            &serde_json::json!({ "path": missing.display().to_string() }),
        )
        .unwrap();
        assert_eq!(response["state"], "missing");
    }

    #[test]
    fn create_generated_video_access_returns_ticket_path() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(temp.path());
        let session_store = SessionStore::from_paths(&paths).unwrap();
        let workspace = temp.path().join("other-workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let session = session_store.create_session(workspace.clone()).unwrap();
        write_generated_video_artifact(
            &workspace,
            "artifact-video-1",
            "generated.mp4",
            b"mp4-bytes",
        );
        let state = test_state_with_paths(paths);

        let response = handle_create_generated_video_access(
            &state,
            &serde_json::json!({
                "sessionId": session.id.to_string(),
                "artifactId": "artifact-video-1"
            }),
        )
        .unwrap();

        assert_eq!(response["state"], "available");
        assert_eq!(response["mimeType"], "video/mp4");
        assert_eq!(response["size"], 9);
        assert!(response["expiresAtMs"].as_u64().unwrap() > daemon_now_ms());
        let path = response["path"].as_str().expect("ticket path");
        assert!(path.starts_with("/media/generated-video/"));
        assert!(response.get("url").is_none());
        assert!(!path.contains("token"));
    }

    #[test]
    fn generated_video_range_parser_supports_closed_open_and_suffix_ranges() {
        assert_eq!(
            parse_single_byte_range("bytes=2-5", 10).unwrap(),
            Some((2, 5))
        );
        assert_eq!(
            parse_single_byte_range("bytes=4-", 10).unwrap(),
            Some((4, 9))
        );
        assert_eq!(
            parse_single_byte_range("bytes=-3", 10).unwrap(),
            Some((7, 9))
        );
        assert_eq!(parse_single_byte_range("", 10).unwrap(), None);
    }

    #[test]
    fn generated_video_range_parser_rejects_unsatisfiable_ranges() {
        assert!(matches!(
            parse_single_byte_range("bytes=20-30", 10),
            Err(GeneratedVideoRangeError::Unsatisfiable)
        ));
        assert!(matches!(
            parse_single_byte_range("bytes=8-2", 10),
            Err(GeneratedVideoRangeError::Unsatisfiable)
        ));
    }

    #[test]
    fn generated_video_ticket_lookup_prunes_expired_entries() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state_with_paths(ConfigPaths::discover(temp.path()));
        state.generated_video_tickets.lock().unwrap().insert(
            "expired".to_string(),
            GeneratedVideoTicket {
                path: temp.path().join("missing.mp4"),
                mime_type: "video/mp4".to_string(),
                size: 0,
                expires_at_ms: daemon_now_ms().saturating_sub(1),
            },
        );

        assert!(state.generated_video_ticket("expired").is_none());
        assert!(state.generated_video_tickets.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn generated_video_handler_serves_full_body() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state_with_paths(ConfigPaths::discover(temp.path())));
        let video_path = temp.path().join("generated.mp4");
        std::fs::write(&video_path, b"0123456789").unwrap();
        state.generated_video_tickets.lock().unwrap().insert(
            "ticket-full".to_string(),
            GeneratedVideoTicket {
                path: video_path,
                mime_type: "video/mp4".to_string(),
                size: 10,
                expires_at_ms: daemon_now_ms() + 60_000,
            },
        );

        let response = generated_video_handler(
            State(state),
            AxumPath("ticket-full".to_string()),
            HeaderMap::new(),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "video/mp4"
        );
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"0123456789");
    }

    #[tokio::test]
    async fn generated_video_handler_serves_single_range() {
        let temp = tempfile::tempdir().unwrap();
        let state = Arc::new(test_state_with_paths(ConfigPaths::discover(temp.path())));
        let video_path = temp.path().join("generated.mp4");
        std::fs::write(&video_path, b"0123456789").unwrap();
        state.generated_video_tickets.lock().unwrap().insert(
            "ticket-1".to_string(),
            GeneratedVideoTicket {
                path: video_path,
                mime_type: "video/mp4".to_string(),
                size: 10,
                expires_at_ms: daemon_now_ms() + 60_000,
            },
        );
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=2-5"));

        let response =
            generated_video_handler(State(state), AxumPath("ticket-1".to_string()), headers)
                .await
                .into_response();

        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 2-5/10"
        );
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"2345");
    }

    #[test]
    fn read_generated_media_preview_returns_png_bytes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let session_store = SessionStore::from_paths(&paths).expect("session store");
        let session = session_store
            .create_session(workspace_root.clone())
            .expect("create session");
        write_generated_image_artifact(
            &workspace_root,
            "artifact-1",
            "generated.png",
            b"\x89PNG\r\n\x1a\nimage-bytes",
        );
        let state = test_state_with_paths(paths);

        let response = handle_read_generated_media_preview(
            &state,
            &json!({
                "sessionId": session.id.to_string(),
                "artifactId": "artifact-1"
            }),
        )
        .expect("preview response");
        let bytes: Vec<u8> = serde_json::from_value(response["bytes"].clone()).expect("bytes");

        assert_eq!(response["state"], "available");
        assert_eq!(response["mimeType"], "image/png");
        assert_eq!(bytes, b"\x89PNG\r\n\x1a\nimage-bytes");
        assert!(response.get("path").is_none());
    }

    #[test]
    fn generate_media_uses_discovery_cache_for_chat_image_output_models() {
        let _home_guard = PufferHomeEnvGuard::set();
        let (base_url, server) = spawn_discovered_chat_image_generation_server();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        write_openrouter_resource_override(&paths, &base_url);
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("openrouter", "sk-test");
        auth_store
            .save(&paths.user_config_dir.join("auth.json"))
            .expect("save auth");
        let state = DaemonState::load(
            workspace_root.clone(),
            paths,
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");
        {
            let mut config = state.config.lock().unwrap();
            config.media.image = Some(MediaGenerationConfig {
                provider_id: "openrouter".to_string(),
                logical_model_id: "openrouter/image-chat".to_string(),
                selections: BTreeMap::new(),
            });
        }
        let listed =
            handle_list_media_capabilities(&state, &json!({"kind": "image"})).expect("list");
        assert!(listed["capabilities"]
            .as_array()
            .is_some_and(|capabilities| {
                capabilities.iter().any(|capability| {
                    capability["providerId"] == "openrouter"
                        && capability["modelId"] == "openrouter/image-chat"
                        && capability["adapter"] == "chat_image_output"
                        && capability["source"] == "provider_discovery"
                })
            }));

        let response =
            handle_generate_media(&state, &json!({"kind": "image", "prompt": "draw an icon"}))
                .expect("generation response");

        let requests = server.join().expect("server");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /models?output_modalities=image HTTP/1.1"));
        assert!(requests[1].starts_with("POST /chat/completions HTTP/1.1"));
        assert!(requests[1].contains("\"model\":\"openrouter/image-chat\""));
        assert_eq!(response["status"], "succeeded");
        assert_eq!(response["providerId"], "openrouter");
        assert_eq!(response["modelId"], "openrouter/image-chat");
        assert_eq!(response["requestedCount"], 1);
        let path = response["artifacts"][0]["path"]
            .as_str()
            .expect("artifact path");
        assert!(path.contains(".puffer/media/images"));
        assert_eq!(std::fs::read(path).expect("artifact bytes"), b"image-bytes");
    }

    #[test]
    fn update_config_accepts_media_defaults() {
        let _home_guard = PufferHomeEnvGuard::set();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let response = handle_update_config(
            &state,
            &json!({
                "media": {
                    "image": {
                        "providerId": "openai",
                        "logicalModelId": "gpt-image-1",
                        "selections": {
                            "mode": "1K SD",
                            "ratio": "3:2",
                            "output": "2"
                        }
                    },
                    "video": null
                }
            }),
        )
        .expect("update config");

        assert_eq!(
            response["config"]["media"],
            json!({
                "image": {
                    "providerId": "openai",
                    "logicalModelId": "gpt-image-1",
                    "selections": {
                        "mode": "1K SD",
                        "ratio": "3:2",
                        "output": "2"
                    }
                },
                "video": null
            })
        );

        let response =
            handle_update_config(&state, &json!({"media": null})).expect("reset media config");

        assert_eq!(
            response["config"]["media"],
            json!({
                "image": null,
                "video": null
            })
        );
    }

    #[test]
    fn connector_setup_connect_args_accepts_connect_slash_command() {
        assert_eq!(
            connector_setup_connect_args("  /connect gmail-browser gmail-browser  ")
                .expect("connect args"),
            "gmail-browser gmail-browser"
        );
    }

    #[test]
    fn connector_setup_connect_args_rejects_other_commands() {
        let err = connector_setup_connect_args("/session list").expect_err("reject command");
        assert!(format!("{err:#}").contains("expects a /connect command"));
    }

    #[test]
    fn connector_setup_id_rejects_path_like_values() {
        let err =
            connector_setup_id(&json!({"setupId": "../sessions"})).expect_err("reject setup id");
        assert!(format!("{err:#}").contains("ASCII alphanumeric"));
    }

    #[test]
    fn connector_setup_turn_does_not_create_visible_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = Arc::new(
            DaemonState::load(
                workspace_root.clone(),
                paths.clone(),
                "token".into(),
                true,
                false,
                false,
            )
            .expect("daemon state"),
        );
        let mut events = state.event_sender().subscribe();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let response = runtime
            .block_on(start_connector_setup_turn(
                state.clone(),
                json!({
                    "setupId": "setup-test",
                    "message": "/connect not-a-real-connector demo"
                }),
            ))
            .expect("start connector setup");

        assert_eq!(response["turnId"], "setup-test");

        let payload = runtime
            .block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        let env = events.recv().await.expect("connector setup event");
                        let ServerEnvelope::Event { event, payload } = env else {
                            continue;
                        };
                        if event == "connector-setup:setup-test:event"
                            && payload["type"] == "turn-error"
                        {
                            break payload;
                        }
                    }
                })
                .await
            })
            .expect("connector setup error event");
        assert_eq!(payload["turnId"], "setup-test");

        let store = SessionStore::from_paths(&paths).expect("session store");
        let page = store.list_sessions_page(0, 10).expect("sessions page");
        assert_eq!(page.total_sessions, 0);
    }

    fn spawn_openai_discovery_server() -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind discovery server");
        listener
            .set_nonblocking(true)
            .expect("nonblocking discovery server");
        let address = listener.local_addr().expect("discovery server address");
        let handle = std::thread::spawn(move || {
            for _ in 0..100 {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        read_discovery_http_request(&mut stream).expect("read discovery request");
                        let body = r#"{"data":[{"id":"gpt-local-discovered","name":"GPT Local"}]}"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        std::io::Write::write_all(&mut stream, response.as_bytes())
                            .expect("write discovery response");
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept discovery request: {error}"),
                }
            }
            panic!("discovery server was not contacted");
        });
        (format!("http://{address}"), handle)
    }

    fn spawn_failing_discovery_server() -> (String, std::thread::JoinHandle<()>) {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind failing discovery server");
        listener
            .set_nonblocking(true)
            .expect("nonblocking failing discovery server");
        let address = listener
            .local_addr()
            .expect("failing discovery server address");
        let handle = std::thread::spawn(move || {
            for _ in 0..100 {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        read_discovery_http_request(&mut stream)
                            .expect("read failing discovery request");
                        let body = r#"{"error":"temporarily unavailable"}"#;
                        let response = format!(
                            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        std::io::Write::write_all(&mut stream, response.as_bytes())
                            .expect("write failing discovery response");
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept failing discovery request: {error}"),
                }
            }
            panic!("failing discovery server was not contacted");
        });
        (format!("http://{address}"), handle)
    }

    fn spawn_realtime_client_secret_server(
        captured: Arc<Mutex<String>>,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind realtime server");
        listener
            .set_nonblocking(true)
            .expect("nonblocking realtime server");
        let address = listener.local_addr().expect("realtime server address");
        let handle = std::thread::spawn(move || {
            for _ in 0..100 {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_discovery_http_request(&mut stream)
                            .expect("read realtime request");
                        *captured.lock().expect("captured request") =
                            String::from_utf8_lossy(&request).to_string();
                        let body =
                            r#"{"value":"ephemeral-test-client-secret","expires_at":1234567890}"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        std::io::Write::write_all(&mut stream, response.as_bytes())
                            .expect("write realtime response");
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept realtime request: {error}"),
                }
            }
            panic!("realtime server was not contacted");
        });
        (format!("http://{address}"), handle)
    }

    fn read_discovery_http_request(stream: &mut std::net::TcpStream) -> std::io::Result<Vec<u8>> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 8192];
        let mut body_offset = None;
        let mut expected_len = None;
        loop {
            let bytes = match std::io::Read::read(stream, &mut chunk) {
                Ok(bytes) => bytes,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) && !buffer.is_empty() =>
                {
                    break;
                }
                Err(error) => return Err(error),
            };
            if bytes == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..bytes]);
            if body_offset.is_none() {
                if let Some(offset) = test_http_body_offset(&buffer) {
                    body_offset = Some(offset);
                    expected_len = test_content_length(&buffer[..offset]);
                    if expected_len.is_none() {
                        break;
                    }
                }
            }
            if let (Some(offset), Some(length)) = (body_offset, expected_len) {
                if buffer.len() >= offset + length {
                    break;
                }
            }
        }
        Ok(buffer)
    }

    fn test_http_body_offset(buffer: &[u8]) -> Option<usize> {
        buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
    }

    fn test_content_length(headers: &[u8]) -> Option<usize> {
        let text = String::from_utf8_lossy(headers);
        text.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
                .flatten()
        })
    }

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
    fn create_session_creates_missing_cwd() {
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
        let missing = workspace_root.join("new-project").join("nested");

        let response = handle_create_session(
            &state,
            &json!({
                "cwd": missing.display().to_string(),
            }),
        )
        .expect("create session");

        assert!(missing.is_dir());
        let session_id = response["sessionId"].as_str().expect("sessionId");
        let store = SessionStore::from_paths(&paths).expect("session store");
        let session_id = uuid::Uuid::parse_str(session_id).expect("valid session id");
        let session = store.load_session(session_id).expect("stored session");
        assert_eq!(session.metadata.cwd, missing);
    }

    #[test]
    fn report_cancelled_turn_persists_streamed_progress_chronologically() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let store = SessionStore::from_paths(&paths).expect("session store");
        let session = store
            .create_session(workspace_root.clone())
            .expect("create session");
        let state = DaemonState::load(
            workspace_root,
            paths.clone(),
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");
        let mut progress_state = TurnProgress::default();
        progress_state.record_text_delta("partial answer");
        progress_state.record_tool_invocations(vec![ToolInvocation {
            call_id: "call-done".to_string(),
            tool_id: "Bash".to_string(),
            input: "{\"command\":\"pwd\"}".to_string(),
            output: "/tmp\n".to_string(),
            success: true,
            metadata: serde_json::Value::Null,
            terminate: false,
        }]);
        progress_state.record_text_delta(" after tool");
        progress_state.record_tool_call_requests(vec![ToolCallRequest {
            call_id: "call-pending".to_string(),
            tool_id: "Bash".to_string(),
            input: "{\"command\":\"sleep 10\"}".to_string(),
        }]);
        let progress = Arc::new(Mutex::new(progress_state));
        let cancel_reported = AtomicBool::new(false);
        let user_prompt_persisted = AtomicBool::new(false);

        assert!(report_cancelled_turn(
            &state,
            session.id,
            &session.id.to_string(),
            "session:test:event",
            "turn-1",
            "question",
            &[],
            &cancel_reported,
            &user_prompt_persisted,
            &progress,
        )
        .expect("report cancelled"));

        let record = store.load_session(session.id).expect("stored session");
        let user_index = record
            .events
            .iter()
            .position(|event| {
                matches!(event, TranscriptEvent::UserMessage { text, .. } if text == "question")
            })
            .expect("user message");
        let assistant_index = record
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    TranscriptEvent::AssistantMessage { text, .. } if text == "partial answer"
                )
            })
            .expect("assistant progress");
        let done_tool_index = record
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    TranscriptEvent::ToolInvocation { call_id, success, .. }
                        if call_id == "call-done" && *success
                )
            })
            .expect("completed tool");
        let pending_tool_index = record
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    TranscriptEvent::ToolInvocation {
                        call_id,
                        success,
                        output,
                        ..
                    } if call_id == "call-pending"
                        && !*success
                        && output == "Interrupted by user."
                )
            })
            .expect("cancelled pending tool");
        let post_tool_assistant_index = record
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    TranscriptEvent::AssistantMessage { text, .. } if text == " after tool"
                )
            })
            .expect("post-tool assistant progress");
        let interrupt_index = record
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    TranscriptEvent::SystemMessage { text, .. } if text == "Interrupted by user."
                )
            })
            .expect("interrupt");
        assert!(user_index < assistant_index);
        assert!(assistant_index < done_tool_index);
        assert!(done_tool_index < post_tool_assistant_index);
        assert!(post_tool_assistant_index < pending_tool_index);
        assert!(pending_tool_index < interrupt_index);
    }

    #[test]
    fn ordered_turn_progress_persists_text_around_tools() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let store = SessionStore::from_paths(&paths).expect("store");
        let session_id = store
            .create_session(workspace_root)
            .expect("create session")
            .id;
        let items = vec![
            TurnProgressItem::AssistantText("before tool".to_string()),
            TurnProgressItem::ToolInvocation(ToolInvocation {
                call_id: "call-1".to_string(),
                tool_id: "Sleep".to_string(),
                input: "{\"duration_ms\":20000}".to_string(),
                output: "done".to_string(),
                success: true,
                metadata: serde_json::Value::Null,
                terminate: false,
            }),
            TurnProgressItem::AssistantText("after tool".to_string()),
        ];

        let persisted_text =
            append_ordered_turn_progress(&store, session_id, items, None, |_| None, true)
                .expect("append ordered progress");
        let record = store.load_session(session_id).expect("load session");

        assert_eq!(persisted_text, "before toolafter tool");
        assert!(matches!(
            &record.events[0],
            TranscriptEvent::AssistantMessage { text, .. } if text == "before tool"
        ));
        assert!(matches!(
            &record.events[1],
            TranscriptEvent::ToolInvocation { tool_id, .. } if tool_id == "Sleep"
        ));
        assert!(matches!(
            &record.events[2],
            TranscriptEvent::AssistantMessage { text, .. } if text == "after tool"
        ));
    }

    #[test]
    fn ordered_turn_progress_uses_tool_request_position() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let store = SessionStore::from_paths(&paths).expect("store");
        let session_id = store
            .create_session(workspace_root)
            .expect("create session")
            .id;
        let mut progress = TurnProgress::default();
        progress.record_text_delta("before sleep");
        progress.record_tool_call_requests(vec![ToolCallRequest {
            call_id: "call-1".to_string(),
            tool_id: "Sleep".to_string(),
            input: "{\"duration_ms\":20000}".to_string(),
        }]);
        progress.record_text_delta(" after request");
        progress.record_tool_invocations(vec![ToolInvocation {
            call_id: "call-1".to_string(),
            tool_id: "Sleep".to_string(),
            input: "{\"duration_ms\":20000}".to_string(),
            output: "done".to_string(),
            success: true,
            metadata: serde_json::Value::Null,
            terminate: false,
        }]);

        append_ordered_turn_progress(
            &store,
            session_id,
            progress.ordered_items,
            None,
            |_| None,
            true,
        )
        .expect("append ordered progress");
        let record = store.load_session(session_id).expect("load session");

        assert!(matches!(
            &record.events[0],
            TranscriptEvent::AssistantMessage { text, .. } if text == "before sleep"
        ));
        assert!(matches!(
            &record.events[1],
            TranscriptEvent::ToolInvocation { tool_id, .. } if tool_id == "Sleep"
        ));
        assert!(matches!(
            &record.events[2],
            TranscriptEvent::AssistantMessage { text, .. } if text == " after request"
        ));
    }

    #[test]
    fn ordered_turn_progress_renders_send_user_message_as_text() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let store = SessionStore::from_paths(&paths).expect("store");
        let session_id = store
            .create_session(workspace_root)
            .expect("create session")
            .id;
        let mut progress = TurnProgress::default();
        progress.record_tool_call_requests(vec![
            ToolCallRequest {
                call_id: "call-message".to_string(),
                tool_id: "SendUserMessage".to_string(),
                input: r#"{"message":"Phase 1 starting."}"#.to_string(),
            },
            ToolCallRequest {
                call_id: "call-sleep".to_string(),
                tool_id: "Sleep".to_string(),
                input: r#"{"duration_ms":20000}"#.to_string(),
            },
        ]);
        progress.record_tool_invocations(vec![ToolInvocation {
            call_id: "call-message".to_string(),
            tool_id: "SendUserMessage".to_string(),
            input: r#"{"message":"Phase 1 starting."}"#.to_string(),
            output: "{}".to_string(),
            success: true,
            metadata: serde_json::Value::Null,
            terminate: false,
        }]);
        progress.record_tool_invocations(vec![ToolInvocation {
            call_id: "call-sleep".to_string(),
            tool_id: "Sleep".to_string(),
            input: r#"{"duration_ms":20000}"#.to_string(),
            output: "done".to_string(),
            success: true,
            metadata: serde_json::Value::Null,
            terminate: false,
        }]);

        append_ordered_turn_progress(
            &store,
            session_id,
            progress.ordered_items,
            None,
            |_| None,
            true,
        )
        .expect("append ordered progress");
        let record = store.load_session(session_id).expect("load session");

        assert_eq!(record.events.len(), 2);
        assert!(matches!(
            &record.events[0],
            TranscriptEvent::AssistantMessage { text, .. } if text == "Phase 1 starting."
        ));
        assert!(matches!(
            &record.events[1],
            TranscriptEvent::ToolInvocation { tool_id, .. } if tool_id == "Sleep"
        ));
    }

    /// Writes a single discovered model into the discovery cache file
    /// pointed at by `PUFFER_DISCOVERY_CACHE_PATH`. `create_session`
    /// reads this cache synchronously instead of firing a network
    /// discovery request, which is what these tests exercise.
    fn prewarm_discovery_cache(provider_id: &str, model_id: &str) {
        let path = std::env::var("PUFFER_DISCOVERY_CACHE_PATH")
            .expect("PUFFER_DISCOVERY_CACHE_PATH must be set via DiscoveryCacheEnvGuard");
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let cache = json!({
            "entries": {
                provider_id: {
                    "models": [
                        {
                            "id": model_id,
                            "display_name": model_id,
                            "provider": provider_id,
                            "api": "openai",
                            "context_window": 8192,
                            "max_output_tokens": 4096,
                        }
                    ],
                    "cached_at_ms": now_ms,
                }
            }
        });
        std::fs::write(&path, serde_json::to_string(&cache).unwrap())
            .expect("write discovery cache");
    }

    #[test]
    fn create_session_provider_routing_runs_off_tokio_runtime() {
        let _cache_guard = DiscoveryCacheEnvGuard::set();
        prewarm_discovery_cache("openai", "gpt-local-discovered");
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
            paths,
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let response = runtime
            .block_on(async move {
                let params = json!({
                    "cwd": workspace_root.display().to_string(),
                    "providerId": "openai",
                });
                run_off_runtime(move || handle_create_session(&state, &params)).await
            })
            .expect("create session");

        assert_eq!(response["providerId"], "openai");
        assert_eq!(response["modelId"], "gpt-local-discovered");
    }

    #[test]
    fn create_session_accepts_desktop_provider_aliases_off_tokio_runtime() {
        let _cache_guard = DiscoveryCacheEnvGuard::set();
        prewarm_discovery_cache("openai", "gpt-local-discovered");
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
            paths,
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let response = runtime
            .block_on(async move {
                let params = json!({
                    "cwd": workspace_root.display().to_string(),
                    "providerId": "codex",
                    "modelId": "codex/gpt-local-discovered",
                });
                run_off_runtime(move || handle_create_session(&state, &params)).await
            })
            .expect("create session");

        assert_eq!(response["providerId"], "openai");
        assert_eq!(response["modelId"], "gpt-local-discovered");
    }

    #[test]
    fn create_session_accepts_display_case_provider_names_off_tokio_runtime() {
        let _cache_guard = DiscoveryCacheEnvGuard::set();
        prewarm_discovery_cache("openai", "gpt-local-discovered");
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
            paths,
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let response = runtime
            .block_on(async move {
                let params = json!({
                    "cwd": workspace_root.display().to_string(),
                    "providerId": "OpenAI",
                    "modelId": "OpenAI/gpt-local-discovered",
                });
                run_off_runtime(move || handle_create_session(&state, &params)).await
            })
            .expect("create session");

        assert_eq!(response["providerId"], "openai");
        assert_eq!(response["modelId"], "gpt-local-discovered");
    }

    /// Binds a local TCP listener whose only job is to verify that nobody
    /// connects to it. If `create_session` ever fires a network discovery
    /// call, this listener accepts the connection and the test thread
    /// panics. Used by `create_session_does_not_fire_network_discovery`.
    fn spawn_unexpected_discovery_server() -> (String, std::thread::JoinHandle<()>) {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind unexpected discovery server");
        listener
            .set_nonblocking(true)
            .expect("nonblocking unexpected discovery server");
        let address = listener
            .local_addr()
            .expect("unexpected discovery server address");
        let handle = std::thread::spawn(move || {
            // Sample for ~500ms — long enough for any network discovery to
            // land, short enough to keep the test fast.
            for _ in 0..50 {
                match listener.accept() {
                    Ok(_) => {
                        panic!("create_session should not contact the provider discovery URL")
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("unexpected listener error: {error}"),
                }
            }
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn create_session_does_not_fire_network_discovery() {
        let _cache_guard = DiscoveryCacheEnvGuard::set();
        // Pre-populate the cache so the discovered model is resolvable
        // synchronously — exactly the path real `create_session` calls take
        // after the daemon has run any earlier model-list refresh.
        prewarm_discovery_cache("openai", "gpt-local-discovered");
        let (unreachable_base_url, server) = spawn_unexpected_discovery_server();
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
            paths,
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");
        state.config.lock().unwrap().openai_base_url = Some(unreachable_base_url);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let start = std::time::Instant::now();
        let response = runtime
            .block_on(async move {
                let params = json!({
                    "cwd": workspace_root.display().to_string(),
                    "providerId": "openai",
                    "modelId": "gpt-local-discovered",
                });
                run_off_runtime(move || handle_create_session(&state, &params)).await
            })
            .expect("create session");
        let elapsed = start.elapsed();

        // The "unexpected" server must finish its sampling window without
        // ever accepting a connection — that's the regression assertion.
        server.join().expect("unexpected discovery server panicked");

        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "create_session took {elapsed:?} — should resolve from cache without network"
        );
        assert_eq!(response["providerId"], "openai");
        assert_eq!(response["modelId"], "gpt-local-discovered");
    }

    #[test]
    fn list_provider_models_accepts_desktop_aliases() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let response = handle_list_provider_models(&state, &json!({ "providerId": "claude" }))
            .expect("list provider models");

        assert_eq!(response["providerId"], "anthropic");
        assert!(
            response["models"]
                .as_array()
                .is_some_and(|models| !models.is_empty()),
            "{response}"
        );
    }

    #[test]
    fn list_provider_models_uses_fresh_discovery_over_static_models() {
        let _cache_guard = DiscoveryCacheEnvGuard::set();
        let (openai_base_url, server) = spawn_openai_discovery_server();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");
        state.config.lock().unwrap().openai_base_url = Some(openai_base_url);

        let response = handle_list_provider_models(&state, &json!({ "providerId": "codex" }))
            .expect("list provider models");

        server.join().expect("discovery server");
        let model_ids = response["models"]
            .as_array()
            .expect("models array")
            .iter()
            .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(response["providerId"], "openai");
        assert!(model_ids.contains(&"gpt-local-discovered"), "{response}");
        assert!(
            !model_ids.contains(&"gpt-5"),
            "fresh discovery should remove stale static models: {response}"
        );
    }

    #[test]
    fn list_provider_models_keeps_static_models_when_fresh_discovery_fails() {
        let _cache_guard = DiscoveryCacheEnvGuard::set();
        let (openai_base_url, server) = spawn_failing_discovery_server();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");
        state.config.lock().unwrap().openai_base_url = Some(openai_base_url);

        let response = handle_list_provider_models(&state, &json!({ "providerId": "codex" }))
            .expect("list provider models");

        server.join().expect("failing discovery server");
        let model_ids = response["models"]
            .as_array()
            .expect("models array")
            .iter()
            .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(response["providerId"], "openai");
        assert!(
            model_ids.contains(&"gpt-5"),
            "static models should remain available after fresh discovery failure: {response}"
        );
    }

    #[test]
    fn list_provider_models_does_not_fire_network_discovery_when_cache_is_fresh() {
        let _cache_guard = DiscoveryCacheEnvGuard::set();
        prewarm_discovery_cache("openai", "gpt-cached-discovered");
        let (openai_base_url, server) = spawn_unexpected_discovery_server();
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");
        state.config.lock().unwrap().openai_base_url = Some(openai_base_url);

        let start = std::time::Instant::now();
        let response = handle_list_provider_models(&state, &json!({ "providerId": "codex" }))
            .expect("list provider models");
        let elapsed = start.elapsed();

        server.join().expect("unexpected discovery server panicked");
        let model_ids = response["models"]
            .as_array()
            .expect("models array")
            .iter()
            .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "list_provider_models took {elapsed:?} despite a fresh discovery cache"
        );
        assert_eq!(response["providerId"], "openai");
        assert!(
            model_ids.contains(&"gpt-cached-discovered"),
            "cached discovery should be returned synchronously: {response}"
        );
    }

    #[test]
    fn daemon_proxy_clients_accept_configured_proxy() {
        let config = PufferConfig {
            network: puffer_config::NetworkConfig {
                proxy: puffer_config::ProxyConfig {
                    enabled: false,
                    selected: None,
                    bypass: vec![],
                    proxies: vec![],
                },
            },
            ..PufferConfig::default()
        };

        let discovery = super::proxy_discovery_client(&config).expect("discovery client");
        let oauth = super::proxy_oauth_client(&config, puffer_provider_openai::OPENAI_TOKEN_URL)
            .expect("oauth client");
        let _ = (discovery, oauth);
    }

    #[test]
    fn realtime_session_config_defaults_to_only_realtime() {
        let (session, model, voice) =
            realtime_session_config_from_params(&json!({})).expect("session config");

        assert_eq!(model, "gpt-realtime-2");
        assert_eq!(voice, "marin");
        assert_eq!(session["type"], "realtime");
        assert_eq!(session["model"], "gpt-realtime-2");
        assert!(session["audio"]["input"]["transcription"].is_null());
        assert_eq!(session["audio"]["output"]["voice"], "marin");
    }

    #[test]
    fn realtime_client_secret_rpc_mints_without_returning_api_key() {
        let captured = Arc::new(Mutex::new(String::new()));
        let (openai_base_url, server) = spawn_realtime_client_secret_server(captured.clone());
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let auth_path = paths.user_config_dir.join("auth.json");
        let mut auth = AuthStore::default();
        auth.set_api_key("openai", "test-api-key");
        auth.save(&auth_path).expect("save auth");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");
        state.config.lock().unwrap().openai_base_url = Some(openai_base_url);

        let response = handle_create_openai_realtime_client_secret(
            &state,
            &json!({
                "model": "gpt-realtime-2",
                "voice": "marin",
                "reasoningEffort": "low"
            }),
        )
        .expect("create realtime client secret");

        server.join().expect("realtime server");
        let request = captured.lock().expect("captured request").clone();
        assert!(request.starts_with("POST /v1/realtime/client_secrets "));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-api-key"));
        assert!(request.contains("\"transcription\":null"));
        assert_eq!(response["providerId"], "openai");
        assert_eq!(response["model"], "gpt-realtime-2");
        assert_eq!(response["voice"], "marin");
        assert_eq!(response["clientSecret"], "ephemeral-test-client-secret");
        assert_eq!(response["expiresAt"], 1234567890);
        assert!(!response.to_string().contains("test-api-key"));
    }

    #[test]
    fn save_proxy_settings_preserves_password_and_redacts_snapshot() {
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
            workspace_root,
            paths.clone(),
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");
        state.config.lock().unwrap().network.proxy = ProxyConfig {
            enabled: true,
            selected: Some("corp".to_string()),
            bypass: vec!["localhost".to_string()],
            proxies: vec![ProxyEndpoint {
                id: "corp".to_string(),
                scheme: ProxyScheme::Http,
                host: "old-proxy.example.com".to_string(),
                port: 8080,
                username: Some("alice".to_string()),
                password: Some("old-secret".to_string()),
            }],
        };

        let response = handle_save_proxy_settings(
            &state,
            &json!({
                "enabled": true,
                "selected": "corp",
                "bypass": ["localhost"],
                "proxies": [{
                    "id": "corp",
                    "scheme": "https",
                    "host": "proxy.example.com",
                    "port": 8443,
                    "username": "alice",
                    "password": null,
                    "keepPassword": true
                }]
            }),
        )
        .expect("save proxy settings");

        let snapshot = &response["networkProxy"];
        assert_eq!(snapshot["enabled"], true);
        assert_eq!(snapshot["selected"], "corp");
        assert_eq!(snapshot["proxies"][0]["hasPassword"], true);
        assert_eq!(
            snapshot["proxies"][0]["uri"],
            "https://proxy.example.com:8443"
        );
        assert!(snapshot["proxies"][0].get("password").is_none());
        let saved = load_config(&paths).expect("saved config");
        let endpoint = saved
            .network
            .proxy
            .proxies
            .iter()
            .find(|endpoint| endpoint.id == "corp")
            .expect("saved proxy");
        assert_eq!(endpoint.scheme, ProxyScheme::Https);
        assert_eq!(endpoint.password.as_deref(), Some("old-secret"));
    }

    #[test]
    fn save_proxy_settings_disables_empty_enabled_proxy() {
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
            workspace_root,
            paths.clone(),
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");

        let response = handle_save_proxy_settings(
            &state,
            &json!({
                "enabled": true,
                "selected": null,
                "bypass": ["localhost"],
                "proxies": []
            }),
        )
        .expect("save proxy settings");

        let snapshot = &response["networkProxy"];
        assert_eq!(snapshot["enabled"], false);
        assert!(snapshot["selected"].is_null());
        assert_eq!(snapshot["proxies"].as_array().expect("proxies").len(), 0);
        let saved = load_config(&paths).expect("saved config");
        assert!(!saved.network.proxy.enabled);
        assert_eq!(saved.network.proxy.selected, None);
    }

    #[test]
    fn save_proxy_settings_selects_first_proxy_when_enabling_without_selection() {
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
            workspace_root,
            paths.clone(),
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");

        let response = handle_save_proxy_settings(
            &state,
            &json!({
                "enabled": true,
                "selected": null,
                "bypass": ["localhost"],
                "proxies": [{
                    "id": "local",
                    "scheme": "socks5",
                    "host": "127.0.0.1",
                    "port": 7890,
                    "username": null,
                    "password": null,
                    "keepPassword": false
                }]
            }),
        )
        .expect("save proxy settings");

        let snapshot = &response["networkProxy"];
        assert_eq!(snapshot["enabled"], true);
        assert_eq!(snapshot["selected"], "local");
        let saved = load_config(&paths).expect("saved config");
        assert!(saved.network.proxy.enabled);
        assert_eq!(saved.network.proxy.selected.as_deref(), Some("local"));
    }

    #[test]
    fn login_with_api_key_accepts_desktop_provider_aliases() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let auth_path = paths.user_config_dir.join("auth.json");
        let state = DaemonState::load(
            workspace_root,
            paths.clone(),
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");

        let response = handle_login_with_api_key(
            &state,
            &json!({ "providerId": "codex", "apiKey": "sk-test" }),
        )
        .expect("login with api key");
        let auth = AuthStore::load(&auth_path).expect("auth store");

        assert!(auth.get("openai").is_some());
        assert!(auth.get("codex").is_none());
        assert!(response["auth"]
            .as_array()
            .is_some_and(|auth| auth.iter().any(|item| item["providerId"] == "openai")));
    }

    #[test]
    fn import_external_credential_accepts_desktop_provider_aliases() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let error = handle_import_external_credential(
            &state,
            &json!({ "providerId": "codex", "source": "not-real" }),
        )
        .expect_err("invalid source should fail after provider alias lookup");

        assert!(error
            .to_string()
            .contains("unknown import source `not-real`"));
    }

    #[test]
    fn logout_provider_accepts_desktop_provider_aliases() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let auth_path = paths.user_config_dir.join("auth.json");
        let mut auth = AuthStore::default();
        auth.set_api_key("openai", "sk-test");
        auth.save(&auth_path).expect("seed auth store");
        let state = DaemonState::load(
            workspace_root,
            paths.clone(),
            "token".into(),
            true,
            false,
            false,
        )
        .expect("daemon state");

        let response = handle_logout_provider(&state, &json!({ "providerId": "codex" }))
            .expect("logout provider");
        let auth = AuthStore::load(&auth_path).expect("auth store");

        assert!(auth.get("openai").is_none());
        assert!(response["auth"]
            .as_array()
            .is_some_and(|auth| auth.iter().all(|item| item["providerId"] != "openai")));
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
    fn turn_request_options_parse_desktop_fields() {
        let options = TurnRequestOptions::from_params(&json!({
            "providerId": "anthropic",
            "modelId": "claude-sonnet-4-5",
            "thinkingOptionId": "high",
            "fastMode": true,
            "permissionMode": "read-only",
        }));

        assert_eq!(options.provider_id.as_deref(), Some("anthropic"));
        assert_eq!(options.model_id.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(options.thinking_option_id.as_deref(), Some("high"));
        assert_eq!(options.fast_mode, Some(true));
        assert_eq!(options.permission_mode.as_deref(), Some("read-only"));
    }

    #[test]
    fn turn_request_options_apply_desktop_model_effort_and_permissions() {
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
        state.effort_level = "low".to_string();
        state.fast_mode = true;

        let mut providers = ProviderRegistry::new();
        providers.register(provider("openai", &["gpt-5.4"]));
        providers.register(provider("anthropic", &["claude-sonnet-4-5"]));
        let options = TurnRequestOptions::from_params(&json!({
            "providerId": "anthropic",
            "modelId": "claude-sonnet-4-5",
            "thinkingOptionId": "high",
            "fastMode": false,
            "permissionMode": "read-only",
        }));

        apply_turn_request_options(&mut state, &providers, &options).expect("turn options");

        assert_eq!(state.current_provider.as_deref(), Some("anthropic"));
        assert_eq!(
            state.current_model.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
        assert_eq!(state.effort_level, "high");
        assert!(!state.fast_mode);
        assert_eq!(state.sandbox_mode, "read-only");
    }

    #[test]
    fn turn_request_options_provider_without_model_selects_provider_default() {
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

        let mut providers = ProviderRegistry::new();
        providers.register(provider("openai", &["gpt-5"]));
        providers.register(provider("anthropic", &["claude-sonnet-4-5"]));
        let options = TurnRequestOptions::from_params(&json!({
            "providerId": "anthropic",
        }));

        apply_turn_request_options(&mut state, &providers, &options).expect("turn options");

        assert_eq!(state.current_provider.as_deref(), Some("anthropic"));
        assert_eq!(
            state.current_model.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
    }

    #[test]
    fn turn_request_options_accept_desktop_provider_aliases() {
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
        state.effort_level = "low".to_string();

        let mut providers = ProviderRegistry::new();
        providers.register(provider("openai", &["gpt-5.4"]));
        providers.register(provider("anthropic", &["claude-sonnet-4-5"]));
        let options = TurnRequestOptions::from_params(&json!({
            "providerId": "claude",
            "modelId": "claude/claude-sonnet-4-5",
            "thinkingOptionId": "high",
        }));

        apply_turn_request_options(&mut state, &providers, &options).expect("turn options");

        assert_eq!(state.current_provider.as_deref(), Some("anthropic"));
        assert_eq!(
            state.current_model.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
        assert_eq!(state.effort_level, "high");

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
        let options = TurnRequestOptions::from_params(&json!({
            "modelOverride": "codex/gpt-5.4",
        }));

        apply_turn_request_options(&mut state, &providers, &options).expect("turn options");

        assert_eq!(state.current_provider.as_deref(), Some("openai"));
        assert_eq!(state.current_model.as_deref(), Some("openai/gpt-5.4"));

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
        let options = TurnRequestOptions::from_params(&json!({
            "providerId": "OpenAI",
            "modelId": "OpenAI/gpt-5.4",
        }));

        apply_turn_request_options(&mut state, &providers, &options).expect("turn options");

        assert_eq!(state.current_provider.as_deref(), Some("openai"));
        assert_eq!(state.current_model.as_deref(), Some("openai/gpt-5.4"));

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
        let options = TurnRequestOptions::from_params(&json!({
            "providerId": "Anthropic",
            "modelId": "Anthropic/claude-sonnet-4-5",
        }));

        apply_turn_request_options(&mut state, &providers, &options).expect("turn options");

        assert_eq!(state.current_provider.as_deref(), Some("anthropic"));
        assert_eq!(
            state.current_model.as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
    }

    #[test]
    fn turn_request_options_preserve_custom_provider_slash_model_ids() {
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

        let mut providers = ProviderRegistry::new();
        providers.register(provider("openrouter", &["openrouter/owl-alpha"]));
        let options = TurnRequestOptions::from_params(&json!({
            "providerId": "openrouter",
            "modelId": "openrouter/owl-alpha",
        }));

        apply_turn_request_options(&mut state, &providers, &options).expect("turn options");

        assert_eq!(state.current_provider.as_deref(), Some("openrouter"));
        assert_eq!(
            state.current_model.as_deref(),
            Some("openrouter/openrouter/owl-alpha")
        );
        let model_id = resolve_create_session_model_id(
            &state.config,
            providers.provider("openrouter").expect("provider"),
            None,
        )
        .expect("saved default model");
        assert_eq!(model_id.as_deref(), Some("openrouter/owl-alpha"));
    }

    #[test]
    fn model_descriptor_dto_exposes_family_thinking_options() {
        let provider = provider("anthropic", &["claude-sonnet-4-5"]);
        let dto = model_descriptor_dto(ModelPreferenceFamily::Anthropic, &provider.models[0]);

        let option_ids: Vec<&str> = dto
            .thinking_options
            .iter()
            .map(|option| option.id.as_str())
            .collect();
        assert_eq!(option_ids, vec!["low", "medium", "high", "max"]);
        assert_eq!(dto.default_thinking_option_id.as_deref(), Some("high"));
        assert!(dto
            .thinking_options
            .iter()
            .any(|option| option.id == "high" && option.is_default));

        let mut no_reasoning = provider.models[0].clone();
        no_reasoning.supports_reasoning = false;
        let dto = model_descriptor_dto(ModelPreferenceFamily::Anthropic, &no_reasoning);
        assert!(dto.thinking_options.is_empty());
        assert_eq!(dto.default_thinking_option_id, None);
    }

    #[test]
    fn create_session_model_routing_resolves_provider_default() {
        let mut config = PufferConfig::default();
        config.default_provider = Some("anthropic".to_string());
        config.default_model = Some("anthropic/claude-sonnet-4-5".to_string());
        let provider = provider("anthropic", &["claude-sonnet-4-5", "claude-haiku"]);

        let model_id =
            resolve_create_session_model_id(&config, &provider, None).expect("default model");
        assert_eq!(model_id.as_deref(), Some("claude-sonnet-4-5"));

        let model_id = resolve_create_session_model_id(&config, &provider, Some("claude-haiku"))
            .expect("requested model");
        assert_eq!(model_id.as_deref(), Some("claude-haiku"));
    }

    #[test]
    fn create_session_model_routing_uses_alias_provider_default() {
        let mut config = PufferConfig::default();
        config.default_provider = Some("codex".to_string());
        config.default_model = Some("codex/gpt-5.4".to_string());
        let openai_provider = provider("openai", &["gpt-5", "gpt-5.4"]);

        let model_id = resolve_create_session_model_id(&config, &openai_provider, None)
            .expect("default model");
        assert_eq!(model_id.as_deref(), Some("gpt-5.4"));

        config.default_provider = Some("claude".to_string());
        config.default_model = Some("claude/claude-opus-4-6".to_string());
        let anthropic_provider = provider("anthropic", &["claude-sonnet-4-5", "claude-opus-4-6"]);

        let model_id = resolve_create_session_model_id(&config, &anthropic_provider, None)
            .expect("default model");
        assert_eq!(model_id.as_deref(), Some("claude-opus-4-6"));
    }

    #[test]
    fn create_session_model_routing_preserves_custom_provider_slash_model_ids() {
        let mut config = PufferConfig::default();
        config.default_provider = Some("openrouter".to_string());
        config.default_model = Some("openrouter/owl-alpha".to_string());
        let openrouter_provider = provider("openrouter", &["openrouter/owl-alpha"]);

        let model_id = resolve_create_session_model_id(&config, &openrouter_provider, None)
            .expect("default model");
        assert_eq!(model_id.as_deref(), Some("openrouter/owl-alpha"));

        let model_id = resolve_create_session_model_id(
            &config,
            &openrouter_provider,
            Some("openrouter/owl-alpha"),
        )
        .expect("requested model");
        assert_eq!(model_id.as_deref(), Some("openrouter/owl-alpha"));

        config.default_model = Some("openrouter/openrouter/owl-alpha".to_string());
        let model_id = resolve_create_session_model_id(&config, &openrouter_provider, None)
            .expect("wrapped default model");
        assert_eq!(model_id.as_deref(), Some("openrouter/owl-alpha"));
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

        let err = anyhow::anyhow!("stream closed before response.completed").context(
            "failed to parse SSE response from https://inference-api.worldrouter.ai/v1/responses",
        );
        let (msg, cat) = classify_turn_error(&err);
        assert_eq!(cat, "provider_stream_closed");
        assert!(msg.contains("stream closed before response.completed"));

        // Anything else falls through with the raw chain preserved.
        let err = anyhow::anyhow!("HTTP 503 from upstream");
        let (msg, cat) = classify_turn_error(&err);
        assert_eq!(cat, "other");
        assert!(msg.contains("HTTP 503"));
    }

    #[test]
    fn classify_turn_error_distinguishes_model_gateway_transport_reset() {
        use super::classify_turn_error;

        let err = anyhow::anyhow!(
            "error sending request for url \
             (https://infer-api-test-46cc90.worldrouter.ai/v1/responses): \
             client error (Connect): Connection reset by peer (os error 54)"
        )
        .context("request to https://infer-api-test-46cc90.worldrouter.ai/v1/responses failed");

        let (msg, cat) = classify_turn_error(&err);

        assert_eq!(cat, "model_gateway_unavailable", "{msg}");
        assert!(msg.contains("model gateway"), "{msg}");
        assert!(msg.contains("retry"), "{msg}");
        assert!(!msg.contains("infer-api-test-46cc90"), "{msg}");
        assert!(!msg.contains("os error 54"), "{msg}");

        let err = anyhow::anyhow!("tcp connect error: Connection refused")
            .context("request to https://infer-api-test-46cc90.worldrouter.ai/v1/responses failed");
        let (msg, cat) = classify_turn_error(&err);
        assert_eq!(cat, "model_gateway_unavailable", "{msg}");
    }

    #[test]
    fn classify_turn_error_keeps_sse_idle_timeout_as_stream_closed() {
        use super::classify_turn_error;

        let err = anyhow::anyhow!("idle timeout waiting for sse")
            .context("failed to parse SSE response from https://example.test/v1/responses");

        let (msg, cat) = classify_turn_error(&err);

        assert_eq!(cat, "provider_stream_closed", "{msg}");
        assert!(msg.contains("idle timeout waiting for sse"), "{msg}");
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
        assert!(state.session_permission_state().allow_all_tools());
    }

    #[test]
    fn desktop_generic_permissions_preserve_browser_section() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        std::fs::write(
            paths.workspace_config_dir.join("permissions.toml"),
            "[tools]\nbrowser = \"allow\"\nbash = \"ask\"\n\n[browser]\ndeny_domains = [\"example.com\"]\n",
        )
        .expect("write permissions");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let listed = handle_list_permissions(&state).expect("list permissions");
        assert_eq!(listed["tools"]["bash"], "ask");
        assert!(listed["tools"].get("browser").is_none());

        let saved = handle_save_permissions(
            &state,
            &json!({
                "tools": {
                    "browser": "deny",
                    "bash": "allow"
                }
            }),
        )
        .expect("save permissions");
        assert_eq!(saved["tools"]["bash"], "allow");
        assert!(saved["tools"].get("browser").is_none());
        let stored: toml::Value = toml::from_str(
            &std::fs::read_to_string(state.paths.workspace_config_dir.join("permissions.toml"))
                .expect("read saved permissions"),
        )
        .expect("parse saved permissions");
        assert_eq!(
            stored["browser"]["deny_domains"][0].as_str(),
            Some("example.com")
        );
    }

    #[test]
    fn desktop_lambda_skill_library_save_writes_manifest_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let skill_dir = lambda_root.join("ci/gh-fix-ci");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {}\nskill gh_fix_ci {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: gh-fix-ci\ndescription: Verified CI repair\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["net_r"],
              "domains": [],
              "tools": [
                {"name": "gh_pr_view", "params": [], "result": "unit", "effects": ["net_r"], "concreteTools": ["Bash", "WebSearch"], "concreteInputContracts": {"Bash": {"command": "gh pr view"}, "WebSearch": {"query": "gh pr view"}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let saved = handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "verified",
                "root": lambda_root.display().to_string(),
                "hostCatalogueSubpath": "out/host.json",
                "allowedTools": [" Bash ", "Read"],
                "hostToolBindings": {
                    " formal_search ": [" Bash "]
                },
                "skillHostToolBindings": {
                    "gh-fix-ci": {
                        " gh_pr_view ": [" Bash "]
                    }
                }
            }),
        )
        .expect("save lambda skill library");

        assert_eq!(saved["libraries"][0]["id"], "verified");
        assert_eq!(saved["libraries"][0]["allowedTools"][0], "Bash");
        assert!(saved["libraries"][0]["hostToolBindings"]
            .as_object()
            .unwrap()
            .is_empty());
        let manifest_path = state
            .paths
            .workspace_config_dir
            .join("resources/lambda_skill_libraries/verified.yaml");
        let manifest = std::fs::read_to_string(&manifest_path).expect("read manifest");
        assert!(manifest.contains("host_catalogue_subpath: out/host.json"));
        assert!(manifest.contains("host_tool_bindings:"));
        assert!(manifest.contains("skill_host_tool_bindings:"));

        let listed = handle_list_lambda_skill_libraries(&state).expect("list libraries");
        assert_eq!(listed["libraries"][0]["sourceKind"], "workspace");
        assert!(listed["doctor"]
            .as_str()
            .unwrap()
            .contains("lambda_skills=1"));
    }

    #[test]
    fn desktop_lambda_skill_library_save_infers_folder_defaults() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let skill_dir = lambda_root.join("ci/gh-fix-ci");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {}\nskill gh_fix_ci {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: gh-fix-ci\ndescription: Verified CI repair\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["net_r"],
              "domains": [],
              "tools": [
                {"name": "gh_pr_view", "params": [], "result": "unit", "effects": ["net_r"], "concreteTools": ["Bash", "WebSearch"], "concreteInputContracts": {"Bash": {"command": "gh pr view"}, "WebSearch": {"query": "gh pr view"}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let saved = handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "verified",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect("save lambda skill library");

        assert_eq!(
            saved["libraries"][0]["hostCatalogueSubpath"],
            "out/host.json"
        );
        assert_eq!(saved["libraries"][0]["allowedTools"][0], "Bash");
        assert_eq!(saved["libraries"][0]["allowedTools"][1], "WebSearch");
        assert!(saved["warnings"].as_array().unwrap().is_empty());
        assert_eq!(saved["skills"][0]["name"], "gh-fix-ci");
        assert_eq!(saved["skills"][0]["enabled"], true);
        assert_eq!(saved["skills"][0]["modelInvocable"], true);
        assert_eq!(saved["skills"][0]["libraryId"], "verified");
        assert!(saved["doctor"]
            .as_str()
            .unwrap()
            .contains("lambda_skills=1 model_invocable=1 missing_gate_config=0"));

        let manifest_path = state
            .paths
            .workspace_config_dir
            .join("resources/lambda_skill_libraries/verified.yaml");
        let manifest = std::fs::read_to_string(&manifest_path).expect("read manifest");
        assert!(manifest.contains("host_catalogue_subpath: out/host.json"));
        assert!(manifest.contains("- Bash"));
        assert!(manifest.contains("- WebSearch"));
    }

    #[test]
    fn desktop_lambda_skill_library_snapshot_infers_legacy_manifest_defaults() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let skill_dir = lambda_root.join("ci/gh-fix-ci");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {}\nskill gh_fix_ci {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: gh-fix-ci\ndescription: Verified CI repair\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["net_r"],
              "domains": [],
              "tools": [
                {"name": "gh_pr_view", "params": [], "result": "unit", "effects": ["net_r"], "concreteTools": ["Bash"], "concreteInputContracts": {"Bash": {"command": "gh pr view"}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let manifest_dir = paths
            .workspace_config_dir
            .join("resources/lambda_skill_libraries");
        std::fs::create_dir_all(&manifest_dir).expect("manifest dir");
        std::fs::write(
            manifest_dir.join("legacy.yaml"),
            format!(
                "id: legacy\nroot: {}\ncompiler_path: /tmp/lskillc\nhost_tool_bindings:\n  gh_pr_view:\n  - Bash\n",
                lambda_root.display()
            ),
        )
        .expect("legacy manifest");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let listed = handle_list_lambda_skill_libraries(&state).expect("list libraries");

        assert_eq!(
            listed["libraries"][0]["hostCatalogueSubpath"],
            "out/host.json"
        );
        assert_eq!(listed["libraries"][0]["allowedTools"][0], "Bash");
        assert!(listed["libraries"][0]["hostToolBindings"]
            .as_object()
            .unwrap()
            .is_empty());
        assert_eq!(listed["skills"][0]["modelInvocable"], true);
    }

    #[test]
    fn desktop_lambda_skill_library_snapshot_deduplicates_same_config_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let config_dir = temp.path().join("shared").join(".puffer");
        let lambda_root = temp.path().join("lambda-skills");
        let skill_dir = lambda_root.join("ci/gh-fix-ci");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {}\nskill gh_fix_ci {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: gh-fix-ci\ndescription: Verified CI repair\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["net_r"],
              "domains": [],
              "tools": [
                {"name": "gh_pr_view", "params": [], "result": "unit", "effects": ["net_r"], "concreteTools": ["Bash"], "concreteInputContracts": {"Bash": {"command": "gh pr view"}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: config_dir.clone(),
            user_config_dir: config_dir,
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let saved = handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "verified",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect("save lambda skill library");

        assert_eq!(saved["libraries"].as_array().unwrap().len(), 1);
        assert_eq!(saved["skills"].as_array().unwrap().len(), 1);
        assert_eq!(saved["skills"][0]["sourceKind"], "workspace");
    }

    #[test]
    fn desktop_lambda_skill_library_save_prunes_nested_folder_configs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let nested_root = lambda_root.join("vendor");
        let skill_dir = nested_root.join("gh-fix-ci");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {}\nskill gh_fix_ci {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: gh-fix-ci\ndescription: Verified CI repair\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["net_r"],
              "domains": [],
              "tools": [
                {"name": "gh_pr_view", "params": [], "result": "unit", "effects": ["net_r"], "concreteTools": ["Bash"], "concreteInputContracts": {"Bash": {"command": "gh pr view"}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "nested",
                "root": nested_root.display().to_string()
            }),
        )
        .expect("save nested lambda skill library");
        let saved = handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "parent",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect("save parent lambda skill library");

        assert_eq!(saved["libraries"].as_array().unwrap().len(), 1);
        assert_eq!(saved["libraries"][0]["id"], "parent");
        assert_eq!(saved["skills"].as_array().unwrap().len(), 1);
        assert!(!state
            .paths
            .workspace_config_dir
            .join("resources/lambda_skill_libraries/nested.yaml")
            .exists());
    }

    #[test]
    fn desktop_lambda_skill_library_save_ignores_folder_covered_by_parent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let nested_root = lambda_root.join("vendor");
        let skill_dir = nested_root.join("gh-fix-ci");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {}\nskill gh_fix_ci {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: gh-fix-ci\ndescription: Verified CI repair\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["net_r"],
              "domains": [],
              "tools": [
                {"name": "gh_pr_view", "params": [], "result": "unit", "effects": ["net_r"], "concreteTools": ["Bash"], "concreteInputContracts": {"Bash": {"command": "gh pr view"}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "parent",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect("save parent lambda skill library");
        let saved = handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "nested",
                "root": nested_root.display().to_string()
            }),
        )
        .expect("covered nested save should return snapshot");

        assert_eq!(saved["libraries"].as_array().unwrap().len(), 1);
        assert_eq!(saved["libraries"][0]["id"], "parent");
        assert!(!state
            .paths
            .workspace_config_dir
            .join("resources/lambda_skill_libraries/nested.yaml")
            .exists());
    }

    #[test]
    fn desktop_lambda_skill_library_remove_deletes_manifest_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let skill_dir = lambda_root.join("ci/gh-fix-ci");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {}\nskill gh_fix_ci {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: gh-fix-ci\ndescription: Verified CI repair\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["net_r"],
              "domains": [],
              "tools": [
                {"name": "gh_pr_view", "params": [], "result": "unit", "effects": ["net_r"], "concreteTools": ["Bash"], "concreteInputContracts": {"Bash": {"command": "gh pr view"}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "verified",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect("save lambda skill library");
        let removed = handle_remove_lambda_skill_library(
            &state,
            &json!({
                "libraryId": "verified",
                "sourceKind": "workspace"
            }),
        )
        .expect("remove lambda skill library");

        assert!(removed["libraries"].as_array().unwrap().is_empty());
        assert!(removed["skills"].as_array().unwrap().is_empty());
        assert!(!state
            .paths
            .workspace_config_dir
            .join("resources/lambda_skill_libraries/verified.yaml")
            .exists());
    }

    #[test]
    fn desktop_lambda_skill_enabled_toggle_updates_manifest_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let skill_dir = lambda_root.join("ci/gh-fix-ci");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {}\nskill gh_fix_ci {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: gh-fix-ci\ndescription: Verified CI repair\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["net_r"],
              "domains": [],
              "tools": [
                {"name": "gh_pr_view", "params": [], "result": "unit", "effects": ["net_r"], "concreteTools": ["Bash"], "concreteInputContracts": {"Bash": {"command": "gh pr view"}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "verified",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect("save lambda skill library");

        let approval_required = handle_set_lambda_skill_approval(
            &state,
            &json!({
                "libraryId": "verified",
                "sourceKind": "workspace",
                "requireApproval": true
            }),
        )
        .expect("enable verified approval prompt");

        assert_eq!(approval_required["libraries"][0]["requireApproval"], true);
        assert_eq!(approval_required["skills"][0]["requireApproval"], true);
        let manifest_path = state
            .paths
            .workspace_config_dir
            .join("resources/lambda_skill_libraries/verified.yaml");
        let manifest = std::fs::read_to_string(&manifest_path).expect("read manifest");
        assert!(manifest.contains("require_approval: true"));

        let disabled = handle_set_lambda_skill_enabled(
            &state,
            &json!({
                "libraryId": "verified",
                "sourceKind": "workspace",
                "skillName": "gh-fix-ci",
                "enabled": false
            }),
        )
        .expect("disable lambda skill");

        assert_eq!(disabled["libraries"][0]["disabledSkills"][0], "gh-fix-ci");
        assert_eq!(disabled["skills"][0]["enabled"], false);
        assert_eq!(disabled["skills"][0]["modelInvocable"], false);
        let manifest = std::fs::read_to_string(&manifest_path).expect("read manifest");
        assert!(manifest.contains("disabled_skills:"));

        let enabled = handle_set_lambda_skill_enabled(
            &state,
            &json!({
                "libraryId": "verified",
                "sourceKind": "workspace",
                "skillName": "gh-fix-ci",
                "enabled": true
            }),
        )
        .expect("enable lambda skill");

        assert!(enabled["libraries"][0]["disabledSkills"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(enabled["skills"][0]["enabled"], true);
        assert_eq!(enabled["skills"][0]["modelInvocable"], true);
    }

    #[test]
    fn desktop_lambda_skill_library_save_rejects_missing_host_catalogue() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let external_project = temp.path().join("external-project");
        let lambda_root = external_project.join("skills");
        let skill_dir = lambda_root.join("vendor/web-check");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {\n  tool fetch_page(url: Str) -> Str {\n    effects: [net_r]\n  }\n  tool ask_for_approval() -> Unit {\n    effects: [user_in]\n  }\n}\nskill web_check {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: web-check\ndescription: Verified web check\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let error = handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "verified",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect_err("missing host catalogue should reject import");

        assert!(error
            .to_string()
            .contains("missing precompiled host catalogues"));
        assert!(!state
            .paths
            .workspace_config_dir
            .join("resources/lambda_skill_libraries/verified.yaml")
            .exists());
    }

    #[test]
    fn desktop_lambda_skill_library_save_rejects_host_without_concrete_tools() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let skill_dir = lambda_root.join("vendor/web-check");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {\n  tool fetch_page(url: Str) -> Str {\n    effects: [net_r]\n  }\n}\nskill web_check {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: web-check\ndescription: Verified web check\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["net_r"],
              "domains": [],
              "tools": [
                {"name": "fetch_page", "params": [{"name": "url", "ty": "str"}], "result": "str", "effects": ["net_r"], "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("raw host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let error = handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "verified",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect_err("raw host catalogue should reject import");

        assert!(error.to_string().contains("lacks concreteTools bindings"));
    }

    #[test]
    fn desktop_lambda_skill_library_save_rejects_unsupported_concrete_tool() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let skill_dir = lambda_root.join("vendor/touchdesigner");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {\n  tool run_td(code: Str) -> Str {\n    effects: [net_w]\n  }\n}\nskill touchdesigner {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: touchdesigner\ndescription: TD\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["net_w"],
              "domains": [],
              "tools": [
                {"name": "run_td", "params": [{"name": "code", "ty": "str"}], "result": "str", "effects": ["net_w"], "concreteTools": ["td_execute_python"], "concreteInputContracts": {"td_execute_python": {"script": {"$arg": "code"}}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let error = handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "verified",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect_err("unsupported concrete tool should reject import");

        assert!(error
            .to_string()
            .contains("binds unsupported concrete tool td_execute_python"));
    }

    #[test]
    fn desktop_lambda_skill_library_save_uses_tool_registry_for_concrete_tool_support() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let skill_dir = lambda_root.join("vendor/custom-tool");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {\n  tool wait() -> Unit {\n    effects: []\n  }\n}\nskill custom_tool {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: custom-tool\ndescription: Custom concrete tool\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": [],
              "domains": [],
              "tools": [
                {"name": "wait", "params": [], "result": "unit", "effects": [], "concreteTools": ["VerifiedSleep"], "concreteInputContracts": {"VerifiedSleep": {"duration_ms": 1}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let tool_dir = paths.workspace_config_dir.join("resources/tools");
        std::fs::create_dir_all(&tool_dir).expect("tool dir");
        std::fs::write(
            tool_dir.join("verified_sleep.yaml"),
            r#"id: VerifiedSleep
name: VerifiedSleep
description: Workspace registered sleep tool.
handler: runtime:sleep
approval_policy: never
sandbox_policy: read-only
input_schema:
  type: object
  properties:
    duration_ms:
      type: integer
  required:
    - duration_ms
  additionalProperties: false
"#,
        )
        .expect("workspace tool");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let saved = handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "verified",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect("workspace registry tool should support verified import");

        assert_eq!(saved["skills"][0]["ready"], true);
        assert_eq!(saved["libraries"][0]["allowedTools"][0], "VerifiedSleep");
    }

    #[test]
    fn desktop_lambda_skill_library_save_accepts_verified_bridge_tools() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let skill_dir = lambda_root.join("vendor/bridge-tools");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {\n  tool bridge(value: Str, port: Int) -> Str {\n    effects: [proc]\n  }\n}\nskill bridge_tools {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: bridge-tools\ndescription: Bridge tools\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["proc"],
              "domains": [],
              "tools": [
                {"name": "comfy_cloud", "params": [{"name": "choice", "ty": "str"}, {"name": "api_key", "ty": "str"}], "result": "str", "effects": ["proc"], "concreteTools": ["ComfyUiAction"], "concreteInputContracts": {"ComfyUiAction": {"action": "configureCloud", "choice": {"$arg": "choice"}, "apiKey": {"$arg": "api_key"}}}, "registers": [], "contextReq": null},
                {"name": "debugpy_attach", "params": [{"name": "port", "ty": "int"}], "result": "unit", "effects": ["proc"], "concreteTools": ["DebugpyAction"], "concreteInputContracts": {"DebugpyAction": {"action": "attach", "port": {"$int_arg": "port"}}}, "registers": [], "contextReq": null},
                {"name": "mcp_check", "params": [{"name": "value", "ty": "str"}], "result": "str", "effects": ["proc"], "concreteTools": ["McpStatus"], "concreteInputContracts": {"McpStatus": {"server": {"$arg": "value"}}}, "registers": [], "contextReq": null},
                {"name": "modal_run", "params": [{"name": "value", "ty": "str"}], "result": "str", "effects": ["proc"], "concreteTools": ["ModalAction"], "concreteInputContracts": {"ModalAction": {"action": "createSecret", "value": {"$arg": "value"}}}, "registers": [], "contextReq": null},
                {"name": "native_mcp_discover", "params": [], "result": "str", "effects": ["proc"], "concreteTools": ["NativeMcpAction"], "concreteInputContracts": {"NativeMcpAction": {"action": "discoverTools"}}, "registers": [], "contextReq": null},
                {"name": "secret_prepare", "params": [{"name": "value", "ty": "str"}], "result": "str", "effects": ["proc"], "concreteTools": ["SecretValue"], "concreteInputContracts": {"SecretValue": {"action": "prepare", "value": {"$arg": "value"}}}, "registers": [], "contextReq": null},
                {"name": "shopify_fulfill", "params": [{"name": "value", "ty": "str"}], "result": "str", "effects": ["proc"], "concreteTools": ["ShopifyAction"], "concreteInputContracts": {"ShopifyAction": {"action": "fulfillmentCreate", "orderId": {"$arg": "value"}, "inputJson": "{}"}}, "registers": [], "contextReq": null},
                {"name": "td_validate", "params": [{"name": "value", "ty": "str"}], "result": "str", "effects": ["proc"], "concreteTools": ["TouchDesignerAction"], "concreteInputContracts": {"TouchDesignerAction": {"action": "validateScript", "code": {"$arg": "value"}}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let saved = handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "verified",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect("verified bridge tools should import");

        assert_eq!(saved["skills"][0]["ready"], true);
        assert_eq!(saved["skills"][0]["modelInvocable"], true);
        assert_eq!(
            saved["libraries"][0]["allowedTools"],
            json!([
                "ComfyUiAction",
                "DebugpyAction",
                "McpStatus",
                "ModalAction",
                "NativeMcpAction",
                "SecretValue",
                "ShopifyAction",
                "TouchDesignerAction"
            ])
        );
    }

    #[test]
    fn desktop_lambda_skill_library_save_accepts_not_ready_runtime_contracts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        let lambda_root = temp.path().join("lambda-skills");
        let skill_dir = lambda_root.join("vendor/web-check");
        std::fs::create_dir_all(skill_dir.join("out")).expect("skill dir");
        std::fs::write(
            skill_dir.join("skill.lskill"),
            "host {\n  tool fetch_page(url: Str) -> Str {\n    effects: [net_r]\n  }\n}\nskill web_check {}\n",
        )
        .expect("skill source");
        std::fs::write(
            skill_dir.join("out/GENERATED.SKILL.md"),
            "---\nname: web-check\ndescription: Verified web check\n---\nUse generated prompt.\n",
        )
        .expect("generated skill");
        std::fs::write(
            skill_dir.join("out/host.json"),
            r#"{
              "effects": ["net_r"],
              "domains": [],
              "tools": [
                {"name": "fetch_page", "params": [{"name": "url", "ty": "str"}], "result": "str", "effects": ["net_r"], "concreteTools": ["ToolSearch"], "registers": [], "contextReq": null},
                {"name": "send_embed", "params": [{"name": "to", "ty": "str"}, {"name": "body", "ty": "str"}, {"name": "embeds", "ty": "str"}], "result": "str", "effects": ["net_w"], "concreteTools": ["DiscordAction"], "concreteInputContracts": {"DiscordAction": {"action": "sendEmbeds", "service": "discord", "channelId": {"$arg": "to"}, "body": {"$arg": "body"}, "embeds": {"$arg": "embeds"}}}, "registers": [], "contextReq": null}
              ]
            }"#,
        )
        .expect("host catalogue");
        let paths = ConfigPaths {
            workspace_root: workspace_root.clone(),
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: workspace_root.join("resources"),
        };
        ensure_workspace_dirs(&paths).expect("workspace dirs");
        let state = DaemonState::load(workspace_root, paths, "token".into(), true, false, false)
            .expect("daemon state");

        let saved = handle_save_lambda_skill_library(
            &state,
            &json!({
                "id": "verified",
                "root": lambda_root.display().to_string()
            }),
        )
        .expect("runtime-not-ready host should still import");

        assert_eq!(saved["skills"][0]["ready"], false);
        assert_eq!(saved["skills"][0]["modelInvocable"], false);
        assert_eq!(saved["libraries"][0]["allowedTools"][0], "DiscordAction");
        assert!(saved["skills"][0]["failureReason"]
            .as_str()
            .unwrap()
            .contains("lacks a concrete input contract"));
        assert!(state
            .paths
            .workspace_config_dir
            .join("resources/lambda_skill_libraries/verified.yaml")
            .exists());
    }

    #[test]
    fn browser_permission_payload_json_exposes_context_only() {
        let payload =
            browser_permission_payload_json(&puffer_core::BrowserPermissionPromptPayload {
                source: puffer_core::BrowserPermissionPromptSource::BrowserTool,
                action_set: puffer_core::BrowserPermissionPromptActionSet::Navigate,
                url: Some("https://docs.example.com/a".to_string()),
                origin: Some("https://docs.example.com".to_string()),
                host: Some("docs.example.com".to_string()),
                target_class: puffer_core::BrowserPermissionPromptTargetClass::OpenWeb,
                tab_id: Some("tab-1".to_string()),
                is_cross_session: false,
            });

        assert_eq!(payload["source"], "browser_tool");
        assert_eq!(payload["actionSet"], "navigate");
        assert_eq!(payload["host"], "docs.example.com");
        assert_eq!(payload["isCrossSession"], false);
        assert!(payload.get("availableScopes").is_none());
        assert!(payload.get("suggestedScope").is_none());
    }

    #[test]
    fn permission_review_payload_json_exposes_reviewer_conclusion() {
        let payload = permission_review_payload_json(&puffer_core::PermissionPromptReviewPayload {
            decision: puffer_core::BrowserAutoReviewRuntimeResult::NeedsUser,
            risk: "medium".to_string(),
            rationale: "Session targeting is explicit but the destination is ambiguous."
                .to_string(),
            resolved_root_session_id: "root-1".to_string(),
            session_targeting: puffer_core::BrowserAutoReviewSessionTargeting::ExplicitSession,
        });

        assert_eq!(payload["decision"], "needs_user");
        assert_eq!(payload["risk"], "medium");
        assert_eq!(
            payload["rationale"],
            "Session targeting is explicit but the destination is ambiguous."
        );
        assert_eq!(payload["resolvedRootSessionId"], "root-1");
        assert_eq!(payload["sessionTargeting"], "explicit_session");
    }

    #[test]
    fn desktop_latency_ms_saturates_to_frontend_safe_integer() {
        assert_eq!(desktop_latency_ms(42), 42);
        assert_eq!(desktop_latency_ms(u128::from(u64::MAX)), u64::MAX);
        assert_eq!(desktop_latency_ms(u128::from(u64::MAX) + 1), u64::MAX);
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
            media: None,
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
