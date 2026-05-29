//! Chrome-backed browser sessions for the desktop Browser tab.
//!
//! This module owns a narrow Chrome DevTools Protocol client. It launches a
//! managed Chrome/Chromium profile per Puffer session, streams
//! `Page.screencastFrame` images onto the daemon event bus, and forwards the
//! small navigation/input surface the Svelte Browser pane needs for v1.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket, connect};

use crate::events::EventEmitter;

mod agent;
mod cdp;
mod chrome;
mod cursor;
mod devtools;
mod input;
mod network_idle;
pub(crate) mod params;
mod recording;
mod selection;
mod tabs;
mod types;

use agent::BrowserElementRef;
pub(crate) use agent::handle_browser_agent;
use cdp::{apply_viewport, set_read_timeout, start_screencast};
pub(super) use cdp::{frame_session_id_string, normalize_url, send_cdp};
use chrome::{
    cdp_http_endpoint, create_page_target, first_page_target, read_devtools_ws_url,
    resolve_chrome_executable, safe_profile_name, terminate_profile_processes,
};
use cursor::{cursor_eval_expression, parse_cursor_response};
use devtools::emit_devtools_event;
use input::send_input;
use network_idle::{NetworkIdleTracker, NetworkIdleWaiter, drain_network_idle_waiters};
use recording::BrowserRecordingRegistry;
use selection::{parse_copy_selection_response, selection_eval_expression};
use tabs::{
    BrowserTabInfo, BrowserTabRegistry, BrowserTabsState, backend_session_id,
    parse_backend_session_id,
};
pub(crate) use types::{
    BrowserCopySelection, BrowserCursor, BrowserEvaluation, BrowserHistoryDirection,
    BrowserInputEvent, BrowserState,
};

const DEFAULT_URL: &str = "about:blank";
const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 720;
const CHROME_START_TIMEOUT: Duration = Duration::from_secs(12);
const CDP_READ_TIMEOUT: Duration = Duration::from_millis(50);
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const AGENT_RECORDING_WINDOW: Duration = Duration::from_secs(5);

/// Tracks live browser workers by Puffer session id.
#[derive(Clone)]
pub(crate) struct BrowserRegistry {
    profile_root: PathBuf,
    sessions: Arc<Mutex<HashMap<String, BrowserSession>>>,
    tabs: Arc<Mutex<BrowserTabRegistry>>,
    agent_refs: Arc<Mutex<HashMap<String, Vec<BrowserElementRef>>>>,
    recordings: Arc<Mutex<BrowserRecordingRegistry>>,
}

impl BrowserRegistry {
    /// Creates an empty browser session registry.
    pub(crate) fn new(profile_root: PathBuf) -> Self {
        let sessions = Arc::new(Mutex::new(HashMap::<String, BrowserSession>::new()));
        spawn_idle_pruner(Arc::clone(&sessions));
        Self {
            profile_root,
            sessions,
            tabs: Arc::new(Mutex::new(BrowserTabRegistry::default())),
            agent_refs: Arc::new(Mutex::new(HashMap::new())),
            recordings: Arc::new(Mutex::new(BrowserRecordingRegistry::default())),
        }
    }

    /// Opens or reuses the browser session for `session_id`.
    pub(crate) fn open(
        &self,
        events: EventEmitter,
        session_id: String,
        url: Option<String>,
        width: u32,
        height: u32,
    ) -> Result<BrowserState> {
        let mut sessions = self.sessions.lock().unwrap();
        if !sessions.contains_key(&session_id) {
            let session = BrowserSession::spawn(
                events,
                Arc::clone(&self.recordings),
                session_id.clone(),
                self.profile_root.join(safe_profile_name(&session_id)),
                width.max(1),
                height.max(1),
            )?;
            sessions.insert(session_id.clone(), session);
        }
        let session = sessions
            .get(&session_id)
            .with_context(|| format!("browser session `{session_id}` missing after spawn"))?;
        session.resize(width.max(1), height.max(1))?;
        if let Some(url) = url {
            session.navigate(normalize_url(&url)?)?;
        }
        let browser_state = session.state();
        if let Some((root_session_id, tab_id)) = parse_backend_session_id(&session_id) {
            self.tabs.lock().unwrap().record_opened_backend(
                root_session_id,
                tab_id,
                session_id.clone(),
                browser_state.clone(),
            );
        }
        Ok(browser_state)
    }

    /// Navigates a live session.
    pub(crate) fn navigate(&self, session_id: &str, url: String) -> Result<()> {
        self.get(session_id)?.navigate(normalize_url(&url)?)
    }

    /// Reloads a live session.
    pub(crate) fn reload(&self, session_id: &str) -> Result<()> {
        self.get(session_id)?.reload()
    }

    /// Moves through browser history.
    pub(crate) fn history(
        &self,
        session_id: &str,
        direction: BrowserHistoryDirection,
    ) -> Result<()> {
        self.get(session_id)?.history(direction)
    }

    /// Resizes a live browser session viewport.
    pub(crate) fn resize(&self, session_id: &str, width: u32, height: u32) -> Result<()> {
        self.get(session_id)?.resize(width.max(1), height.max(1))
    }

    /// Sends a user input event to a live session.
    pub(crate) fn input(&self, session_id: &str, event: BrowserInputEvent) -> Result<()> {
        self.get(session_id)?.input(event)
    }

    /// Copies the current Chrome-owned webpage selection.
    pub(crate) fn copy_selection(&self, session_id: &str) -> Result<BrowserCopySelection> {
        self.get(session_id)?.copy_selection()
    }

    /// Reads the browser cursor at the given viewport coordinate.
    pub(crate) fn cursor(&self, session_id: &str, x: f64, y: f64) -> Result<BrowserCursor> {
        self.get(session_id)?.cursor(x, y)
    }

    /// Closes a session. Missing ids are treated as already closed.
    pub(crate) fn close(&self, session_id: &str) -> Result<()> {
        let session = self.sessions.lock().unwrap().remove(session_id);
        if let Some(session) = session {
            session.close()?;
        }
        if let Some((root_session_id, tab_id)) = parse_backend_session_id(session_id) {
            self.tabs
                .lock()
                .unwrap()
                .remove_backend(root_session_id, tab_id);
        }
        Ok(())
    }

    /// Lists daemon-owned browser tabs for an agent session.
    pub(crate) fn list_tabs(&self, root_session_id: &str) -> BrowserTabsState {
        let mut state = self.tabs.lock().unwrap().list(root_session_id);
        let sessions = self.sessions.lock().unwrap();
        for tab in &mut state.tabs {
            if let Some(session) = sessions.get(&tab.backend_session_id) {
                let browser_state = session.state();
                tab.url = browser_state.url;
                tab.title = browser_state.title;
                tab.loading = browser_state.loading;
                tab.connected = true;
            }
        }
        state
    }

    /// Opens or reuses a tab inside the agent session browser set.
    pub(crate) fn open_tab(
        &self,
        events: EventEmitter,
        root_session_id: String,
        tab_id: Option<String>,
        label: Option<String>,
        url: Option<String>,
        width: u32,
        height: u32,
        activate: bool,
    ) -> Result<BrowserTabInfo> {
        let tab_id =
            tab_id.unwrap_or_else(|| self.tabs.lock().unwrap().next_tab_id(&root_session_id));
        let backend_id = backend_session_id(&root_session_id, &tab_id);
        let browser_state = self.open(events, backend_id.clone(), url, width, height)?;
        let tab = self.tabs.lock().unwrap().open_tab(
            &root_session_id,
            Some(tab_id),
            label,
            backend_id,
            browser_state,
            activate,
        );
        Ok(tab)
    }

    /// Focuses an existing browser tab for an agent session.
    pub(crate) fn focus_tab(&self, root_session_id: &str, tab_id: &str) -> Result<BrowserTabInfo> {
        let Some(tab) = self.tabs.lock().unwrap().focus_tab(root_session_id, tab_id) else {
            bail!("no browser tab `{tab_id}` for session `{root_session_id}`");
        };
        Ok(tab)
    }

    /// Closes an existing browser tab for an agent session.
    pub(crate) fn close_tab(
        &self,
        root_session_id: &str,
        tab_id: &str,
    ) -> Result<BrowserTabsState> {
        let backend_id = backend_session_id(root_session_id, tab_id);
        let _ = self.close(&backend_id);
        self.tabs.lock().unwrap().close_tab(root_session_id, tab_id);
        Ok(self.list_tabs(root_session_id))
    }

    fn get(&self, session_id: &str) -> Result<BrowserSession> {
        let session = self
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .with_context(|| format!("no browser session `{session_id}`"))?;
        session.touch();
        Ok(session)
    }

    /// Returns the deduplicated browser screen recording for an agent session.
    pub(crate) fn recording_frames(&self, root_session_id: &str) -> Value {
        json!({
            "frames": self.recordings.lock().unwrap().frames_for(root_session_id)
        })
    }

    /// Enables short-lived History recording for an agent-owned browser action.
    pub(crate) fn arm_agent_recording(&self, backend_session_id: &str) {
        self.recordings
            .lock()
            .unwrap()
            .arm_backend(backend_session_id, AGENT_RECORDING_WINDOW);
    }

    /// Ensures the active agent Browser tab exists and returns its browser-level CDP endpoint.
    pub(crate) fn cdp_endpoint_for_agent(
        &self,
        events: EventEmitter,
        root_session_id: &str,
    ) -> Result<String> {
        let active = active_or_first_tab(&self.tabs.lock().unwrap().list(root_session_id));
        let backend_id = if let Some(tab) = active {
            if self
                .resize(&tab.backend_session_id, INITIAL_WIDTH, INITIAL_HEIGHT)
                .is_err()
            {
                let state = self.open(
                    events.clone(),
                    tab.backend_session_id.clone(),
                    Some(DEFAULT_URL.to_string()),
                    INITIAL_WIDTH,
                    INITIAL_HEIGHT,
                )?;
                self.tabs.lock().unwrap().record_opened_backend(
                    root_session_id,
                    &tab.tab_id,
                    tab.backend_session_id.clone(),
                    state,
                );
            }
            tab.backend_session_id
        } else {
            self.open_tab(
                events,
                root_session_id.to_string(),
                None,
                None,
                Some(DEFAULT_URL.to_string()),
                INITIAL_WIDTH,
                INITIAL_HEIGHT,
                true,
            )?
            .backend_session_id
        };
        self.get(&backend_id)?.cdp_endpoint()
    }
}

fn active_or_first_tab(tabs: &BrowserTabsState) -> Option<BrowserTabInfo> {
    tabs.tabs
        .iter()
        .find(|tab| tab.active)
        .or_else(|| tabs.tabs.first())
        .cloned()
}

fn spawn_idle_pruner(sessions: Arc<Mutex<HashMap<String, BrowserSession>>>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(60));
            let stale = {
                let mut sessions = sessions.lock().unwrap();
                let stale = sessions
                    .iter()
                    .filter_map(|(id, session)| {
                        (session.idle_for() >= SESSION_IDLE_TIMEOUT).then(|| id.clone())
                    })
                    .collect::<Vec<_>>();
                stale
                    .iter()
                    .filter_map(|id| sessions.remove(id))
                    .collect::<Vec<_>>()
            };
            for session in stale {
                let _ = session.close();
            }
        }
    });
}

#[derive(Clone)]
struct BrowserSession {
    tx: Sender<BrowserCommand>,
    state: Arc<Mutex<BrowserState>>,
    last_active: Arc<Mutex<Instant>>,
    cdp_endpoint: String,
}

impl BrowserSession {
    fn spawn(
        events: EventEmitter,
        recordings: Arc<Mutex<BrowserRecordingRegistry>>,
        session_id: String,
        profile_dir: PathBuf,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let chrome = resolve_chrome_executable()
            .ok_or_else(|| anyhow!("Chrome or Chromium executable not found"))?;
        std::fs::create_dir_all(&profile_dir).context("create browser profile directory")?;
        terminate_profile_processes(&profile_dir);
        match std::fs::remove_file(profile_dir.join("DevToolsActivePort")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("remove stale Chrome DevToolsActivePort"),
        }

        let mut child = Command::new(&chrome)
            .arg("--headless=new")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile_dir.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-background-networking")
            .arg("--disable-features=Translate")
            .arg("--disable-gpu")
            .arg("--allow-file-access")
            .arg("--allow-file-access-from-files")
            .arg("--force-color-profile=srgb")
            .arg(format!("--window-size={width},{height}"))
            .arg(DEFAULT_URL)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("launch Chrome at {}", chrome.display()))?;

        let browser_ws = match read_devtools_ws_url(&mut child, &profile_dir) {
            Ok(url) => url,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };
        let cdp_endpoint = cdp_http_endpoint(&browser_ws)?;
        let page_ws = match first_page_target(&browser_ws)
            .or_else(|_| create_page_target(&browser_ws, DEFAULT_URL))
        {
            Ok(url) => url,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(BrowserState {
            url: DEFAULT_URL.to_string(),
            title: String::new(),
            loading: false,
            width,
            height,
        }));
        let last_active = Arc::new(Mutex::new(Instant::now()));
        let worker_state = Arc::clone(&state);
        std::thread::spawn(move || {
            run_cdp_worker(
                events,
                recordings,
                session_id,
                child,
                page_ws,
                rx,
                worker_state,
                width,
                height,
            );
        });
        Ok(Self {
            tx,
            state,
            last_active,
            cdp_endpoint,
        })
    }

    fn state(&self) -> BrowserState {
        self.touch();
        self.state.lock().unwrap().clone()
    }

    fn cdp_endpoint(&self) -> Result<String> {
        self.touch();
        Ok(self.cdp_endpoint.clone())
    }

    fn navigate(&self, url: String) -> Result<()> {
        self.send(BrowserCommand::Navigate(url.clone()))?;
        let mut state = self.state.lock().unwrap();
        state.url = url;
        state.loading = true;
        Ok(())
    }

    fn reload(&self) -> Result<()> {
        self.send(BrowserCommand::Reload)
    }

    fn history(&self, direction: BrowserHistoryDirection) -> Result<()> {
        self.send(BrowserCommand::History(direction))
    }

    fn resize(&self, width: u32, height: u32) -> Result<()> {
        self.send(BrowserCommand::Resize { width, height })
    }

    fn input(&self, event: BrowserInputEvent) -> Result<()> {
        self.send(BrowserCommand::Input(event))
    }

    fn copy_selection(&self) -> Result<BrowserCopySelection> {
        let (reply, rx) = mpsc::channel();
        self.send(BrowserCommand::CopySelection { reply })?;
        rx.recv_timeout(Duration::from_secs(5))
            .context("timed out waiting for browser selection")?
            .map_err(|message| anyhow!("{message}"))
    }

    fn cursor(&self, x: f64, y: f64) -> Result<BrowserCursor> {
        let (reply, rx) = mpsc::channel();
        self.send(BrowserCommand::Cursor { x, y, reply })?;
        rx.recv_timeout(Duration::from_secs(2))
            .context("timed out waiting for browser cursor")?
            .map_err(|message| anyhow!("{message}"))
    }

    fn evaluate(&self, expression: String) -> Result<BrowserEvaluation> {
        let (reply, rx) = mpsc::channel();
        self.send(BrowserCommand::Evaluate { expression, reply })?;
        rx.recv_timeout(Duration::from_secs(5))
            .context("timed out waiting for browser evaluation")?
            .map_err(|message| anyhow!("{message}"))
    }

    fn wait_network_idle(&self, idle_ms: u64, timeout_ms: u64) -> Result<()> {
        let (reply, rx) = mpsc::channel();
        self.send(BrowserCommand::WaitNetworkIdle {
            idle_ms,
            timeout_ms,
            reply,
        })?;
        rx.recv_timeout(Duration::from_millis(timeout_ms.saturating_add(1_000)))
            .context("timed out waiting for browser network-idle response")?
            .map_err(|message| anyhow!("{message}"))
    }

    fn close(&self) -> Result<()> {
        self.send(BrowserCommand::Close)
    }

    fn send(&self, command: BrowserCommand) -> Result<()> {
        self.touch();
        self.tx.send(command).context("browser worker stopped")
    }

    fn touch(&self) {
        *self.last_active.lock().unwrap() = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_active.lock().unwrap().elapsed()
    }
}

enum BrowserCommand {
    Navigate(String),
    Reload,
    History(BrowserHistoryDirection),
    Resize {
        width: u32,
        height: u32,
    },
    Input(BrowserInputEvent),
    CopySelection {
        reply: Sender<std::result::Result<BrowserCopySelection, String>>,
    },
    Cursor {
        x: f64,
        y: f64,
        reply: Sender<std::result::Result<BrowserCursor, String>>,
    },
    Evaluate {
        expression: String,
        reply: Sender<std::result::Result<BrowserEvaluation, String>>,
    },
    WaitNetworkIdle {
        idle_ms: u64,
        timeout_ms: u64,
        reply: Sender<std::result::Result<(), String>>,
    },
    Close,
}

enum PendingKind {
    StateEval,
    CopySelection {
        reply: Sender<std::result::Result<BrowserCopySelection, String>>,
    },
    Cursor {
        reply: Sender<std::result::Result<BrowserCursor, String>>,
    },
    Evaluate {
        reply: Sender<std::result::Result<BrowserEvaluation, String>>,
    },
}

fn run_cdp_worker(
    events: EventEmitter,
    recordings: Arc<Mutex<BrowserRecordingRegistry>>,
    session_id: String,
    mut child: Child,
    page_ws: String,
    rx: Receiver<BrowserCommand>,
    state: Arc<Mutex<BrowserState>>,
    width: u32,
    height: u32,
) {
    let channel_frame = format!("browser:{session_id}:frame");
    let channel_state = format!("browser:{session_id}:state");
    let channel_devtools = format!("browser:{session_id}:devtools");
    let mut socket = match connect(page_ws.as_str()) {
        Ok((socket, _)) => socket,
        Err(error) => {
            emit_state_error(&events, &channel_state, error);
            let _ = child.kill();
            return;
        }
    };
    set_read_timeout(&socket, Some(CDP_READ_TIMEOUT));

    let mut next_id = 1u64;
    let mut pending = HashMap::<u64, PendingKind>::new();
    let _ = send_cdp(&mut socket, &mut next_id, "Page.enable", json!({}));
    let _ = send_cdp(&mut socket, &mut next_id, "Runtime.enable", json!({}));
    let _ = send_cdp(&mut socket, &mut next_id, "Log.enable", json!({}));
    let _ = send_cdp(&mut socket, &mut next_id, "Network.enable", json!({}));
    let _ = send_cdp(&mut socket, &mut next_id, "Page.bringToFront", json!({}));
    let _ = apply_viewport(&mut socket, &mut next_id, width, height);
    let _ = start_screencast(&mut socket, &mut next_id, width, height);
    let id = send_state_eval(&mut socket, &mut next_id);
    pending.insert(id, PendingKind::StateEval);
    let mut network_idle = NetworkIdleTracker::default();
    let mut network_idle_waiters = Vec::<NetworkIdleWaiter>::new();

    let mut alive = true;
    while alive {
        loop {
            match rx.try_recv() {
                Ok(BrowserCommand::Close) | Err(TryRecvError::Disconnected) => {
                    alive = false;
                    break;
                }
                Ok(command) => handle_command(
                    command,
                    &mut socket,
                    &mut next_id,
                    &mut pending,
                    &state,
                    &channel_state,
                    &events,
                    &mut network_idle_waiters,
                ),
                Err(TryRecvError::Empty) => break,
            }
        }
        drain_network_idle_waiters(&mut network_idle_waiters, &network_idle);
        if !alive {
            break;
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    handle_cdp_message(
                        &events,
                        &channel_frame,
                        &channel_state,
                        &channel_devtools,
                        &session_id,
                        &recordings,
                        &mut socket,
                        &mut next_id,
                        &mut pending,
                        &state,
                        &mut network_idle,
                        value,
                    );
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => {
                emit_state_error(&events, &channel_state, error);
                break;
            }
        }
    }
    let _ = socket.close(None);
    let _ = child.kill();
}

fn handle_command(
    command: BrowserCommand,
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: &mut u64,
    pending: &mut HashMap<u64, PendingKind>,
    state: &Arc<Mutex<BrowserState>>,
    channel_state: &str,
    events: &EventEmitter,
    network_idle_waiters: &mut Vec<NetworkIdleWaiter>,
) {
    match command {
        BrowserCommand::Navigate(url) => {
            {
                let mut state = state.lock().unwrap();
                state.url = url.clone();
                state.loading = true;
                emit_state(events, channel_state, &state);
            }
            let _ = send_cdp(socket, next_id, "Page.bringToFront", json!({}));
            let _ = send_cdp(socket, next_id, "Page.navigate", json!({ "url": url }));
        }
        BrowserCommand::Reload => {
            let _ = send_cdp(
                socket,
                next_id,
                "Page.reload",
                json!({ "ignoreCache": false }),
            );
        }
        BrowserCommand::History(direction) => {
            let expression = match direction {
                BrowserHistoryDirection::Back => "history.back()",
                BrowserHistoryDirection::Forward => "history.forward()",
            };
            let _ = send_cdp(
                socket,
                next_id,
                "Runtime.evaluate",
                json!({ "expression": expression }),
            );
        }
        BrowserCommand::Resize { width, height } => {
            {
                let mut state = state.lock().unwrap();
                state.width = width;
                state.height = height;
                emit_state(events, channel_state, &state);
            }
            let _ = send_cdp(socket, next_id, "Page.bringToFront", json!({}));
            let _ = apply_viewport(socket, next_id, width, height);
            let _ = send_cdp(socket, next_id, "Page.stopScreencast", json!({}));
            let _ = start_screencast(socket, next_id, width, height);
        }
        BrowserCommand::Input(event) => {
            let _ = send_input(socket, next_id, event);
        }
        BrowserCommand::CopySelection { reply } => {
            let id = send_cdp(
                socket,
                next_id,
                "Runtime.evaluate",
                json!({
                    "expression": selection_eval_expression(),
                    "returnByValue": true
                }),
            );
            pending.insert(id, PendingKind::CopySelection { reply });
        }
        BrowserCommand::Cursor { x, y, reply } => {
            let id = send_cdp(
                socket,
                next_id,
                "Runtime.evaluate",
                json!({
                    "expression": cursor_eval_expression(x, y),
                    "returnByValue": true
                }),
            );
            pending.insert(id, PendingKind::Cursor { reply });
        }
        BrowserCommand::Evaluate { expression, reply } => {
            let id = send_cdp(
                socket,
                next_id,
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true
                }),
            );
            pending.insert(id, PendingKind::Evaluate { reply });
        }
        BrowserCommand::WaitNetworkIdle {
            idle_ms,
            timeout_ms,
            reply,
        } => network_idle_waiters.push(NetworkIdleWaiter::new(
            Duration::from_millis(idle_ms.max(1)),
            Duration::from_millis(timeout_ms.max(1)),
            reply,
        )),
        BrowserCommand::Close => {}
    }
}

fn handle_cdp_message(
    events: &EventEmitter,
    channel_frame: &str,
    channel_state: &str,
    channel_devtools: &str,
    session_id: &str,
    recordings: &Arc<Mutex<BrowserRecordingRegistry>>,
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: &mut u64,
    pending: &mut HashMap<u64, PendingKind>,
    state: &Arc<Mutex<BrowserState>>,
    network_idle: &mut NetworkIdleTracker,
    value: Value,
) {
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        match pending.remove(&id) {
            Some(PendingKind::StateEval) => {
                update_state_from_eval(events, channel_state, state, &value);
            }
            Some(PendingKind::CopySelection { reply }) => {
                let result =
                    parse_copy_selection_response(&value).map_err(|error| format!("{error:#}"));
                let _ = reply.send(result);
            }
            Some(PendingKind::Cursor { reply }) => {
                let result = parse_cursor_response(&value).map_err(|error| format!("{error:#}"));
                let _ = reply.send(result);
            }
            Some(PendingKind::Evaluate { reply }) => {
                let result =
                    parse_evaluation_response(&value).map_err(|error| format!("{error:#}"));
                let _ = reply.send(result);
            }
            None => {}
        }
        return;
    }
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return;
    };
    network_idle.record_event(method, &value);
    match method {
        "Page.screencastFrame" => {
            let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
            if let Some(frame_session_id) = params.get("sessionId").and_then(Value::as_i64) {
                let _ = send_cdp(
                    socket,
                    next_id,
                    "Page.screencastFrameAck",
                    json!({ "sessionId": frame_session_id }),
                );
            }
            let width = params
                .pointer("/metadata/deviceWidth")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| state.lock().unwrap().width as f64)
                as u32;
            let height = params
                .pointer("/metadata/deviceHeight")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| state.lock().unwrap().height as f64)
                as u32;
            let data = params.get("data").and_then(Value::as_str).unwrap_or("");
            events.emit(
                channel_frame.to_string(),
                json!({
                    "frameId": frame_session_id_string(session_id, params.get("sessionId")),
                    "mimeType": "image/jpeg",
                    "encoding": "base64",
                    "data": data,
                    "width": width,
                    "height": height
                }),
            );
            let browser_state = state.lock().unwrap().clone();
            let cdp_frame_id = params
                .get("sessionId")
                .map(Value::to_string)
                .unwrap_or_else(|| "frame".to_string());
            if let Some(frame) = recordings.lock().unwrap().record_frame(
                session_id,
                &cdp_frame_id,
                data,
                width,
                height,
                &browser_state,
            ) {
                events.emit(
                    format!("browser:{}:recording", frame.root_session_id),
                    serde_json::to_value(frame).unwrap_or_else(|_| json!({})),
                );
            }
        }
        "Page.loadEventFired" | "Page.frameStoppedLoading" => {
            let id = send_state_eval(socket, next_id);
            pending.insert(id, PendingKind::StateEval);
        }
        "Page.frameNavigated" => {
            if let Some(frame) = value.pointer("/params/frame") {
                if frame.get("parentId").is_none() {
                    let mut state = state.lock().unwrap();
                    if let Some(url) = frame.get("url").and_then(Value::as_str) {
                        state.url = url.to_string();
                    }
                    state.loading = true;
                    emit_state(events, channel_state, &state);
                }
            }
        }
        _ if emit_devtools_event(events, channel_devtools, method, &value) => {}
        _ => {}
    }
}

fn update_state_from_eval(
    events: &EventEmitter,
    channel_state: &str,
    state: &Arc<Mutex<BrowserState>>,
    value: &Value,
) {
    let Some(result) = value.pointer("/result/result/value") else {
        return;
    };
    let mut state = state.lock().unwrap();
    if let Some(url) = result.get("url").and_then(Value::as_str) {
        state.url = url.to_string();
    }
    if let Some(title) = result.get("title").and_then(Value::as_str) {
        state.title = title.to_string();
    }
    state.loading = false;
    emit_state(events, channel_state, &state);
}

fn emit_state(events: &EventEmitter, channel_state: &str, state: &BrowserState) {
    events.emit(
        channel_state.to_string(),
        json!({
            "url": state.url,
            "title": state.title,
            "loading": state.loading,
            "width": state.width,
            "height": state.height,
            "popOut": false
        }),
    );
}

fn emit_state_error<E: std::fmt::Display>(events: &EventEmitter, channel_state: &str, error: E) {
    events.emit(
        channel_state.to_string(),
        json!({
            "url": DEFAULT_URL,
            "title": "",
            "loading": false,
            "error": error.to_string(),
            "popOut": false
        }),
    );
}

fn send_state_eval(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>, next_id: &mut u64) -> u64 {
    send_cdp(
        socket,
        next_id,
        "Runtime.evaluate",
        json!({
            "expression": "({ url: location.href, title: document.title })",
            "returnByValue": true
        }),
    )
}

fn parse_evaluation_response(value: &Value) -> Result<BrowserEvaluation> {
    if let Some(exception) = value
        .pointer("/result/exceptionDetails/text")
        .and_then(Value::as_str)
    {
        bail!("browser evaluation failed: {exception}");
    }
    let Some(result) = value.pointer("/result/result") else {
        bail!("browser evaluation returned no result");
    };
    Ok(BrowserEvaluation {
        value: result.get("value").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests;
