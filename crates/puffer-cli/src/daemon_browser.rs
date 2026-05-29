//! Chrome-backed browser sessions for the desktop Browser tab.
//!
//! This module owns a root Chrome process per browser session tree and a
//! page-level CDP worker per open tab. It streams `Page.screencastFrame`
//! images onto the daemon event bus and forwards the small navigation/input
//! surface the Svelte Browser pane needs for v1.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use url::Url;

use crate::daemon::ServerEnvelope;

mod agent;
mod cdp;
mod chrome;
mod client;
mod console;
mod cursor;
mod devtools;
mod input;
mod params;
mod recording;
mod ref_resolution;
mod rpc;
mod screenshot;
mod selection;
mod session;
mod tabs;
mod types;
mod upload;
mod worker;

pub(crate) use agent::handle_browser_agent;
pub(super) use cdp::parse_evaluation_response;
pub(super) use cdp::send_cdp;
pub(crate) use client::{default_cli_session_id, ensure_daemon, send_daemon_request};
use console::BrowserConsoleRegistry;
use recording::BrowserRecordingRegistry;
pub(crate) use rpc::*;
use screenshot::BrowserElementRef;
use session::{BrowserRootSession, BrowserSession};
use tabs::{backend_session_id, parse_backend_session_id, BrowserTabRegistry};
pub(crate) use tabs::{BrowserCurrentTabContext, BrowserTabInfo, BrowserTabsState};
pub(crate) use types::{
    BrowserCopySelection, BrowserCursor, BrowserEvaluation, BrowserHistoryDirection,
    BrowserInputEvent, BrowserState,
};

const DEFAULT_URL: &str = "about:blank";
const GLOBAL_BROWSER_ROOT_ID: &str = "puffer-global";
const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 720;
const CHROME_START_TIMEOUT: Duration = Duration::from_secs(12);
const CDP_READ_TIMEOUT: Duration = Duration::from_millis(50);
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const AGENT_RECORDING_WINDOW: Duration = Duration::from_secs(5);

/// Tracks live browser roots and page workers by session id.
pub(crate) struct BrowserRegistry {
    profile_root: PathBuf,
    roots: Arc<Mutex<HashMap<String, BrowserRootSession>>>,
    enabled: bool,
    sessions: Arc<Mutex<HashMap<String, BrowserSession>>>,
    tabs: Arc<Mutex<BrowserTabRegistry>>,
    agent_refs: Arc<Mutex<HashMap<String, Vec<BrowserElementRef>>>>,
    console_logs: Arc<Mutex<BrowserConsoleRegistry>>,
    recordings: Arc<Mutex<BrowserRecordingRegistry>>,
}

impl BrowserRegistry {
    /// Creates an empty browser session registry.
    pub(crate) fn new(profile_root: PathBuf, enabled: bool) -> Self {
        let roots = Arc::new(Mutex::new(HashMap::<String, BrowserRootSession>::new()));
        let sessions = Arc::new(Mutex::new(HashMap::<String, BrowserSession>::new()));
        let tabs = Arc::new(Mutex::new(BrowserTabRegistry::default()));
        let agent_refs = Arc::new(Mutex::new(HashMap::new()));
        let console_logs = Arc::new(Mutex::new(BrowserConsoleRegistry::default()));
        if enabled {
            spawn_idle_pruner(
                Arc::clone(&roots),
                Arc::clone(&sessions),
                Arc::clone(&tabs),
                Arc::clone(&agent_refs),
                Arc::clone(&console_logs),
            );
        }
        Self {
            profile_root,
            roots,
            enabled,
            sessions,
            tabs,
            agent_refs,
            console_logs,
            recordings: Arc::new(Mutex::new(BrowserRecordingRegistry::default())),
        }
    }

    /// Opens or reuses the browser page worker for `session_id`.
    pub(crate) fn open(
        &self,
        events: broadcast::Sender<ServerEnvelope>,
        session_id: String,
        url: Option<String>,
        width: u32,
        height: u32,
    ) -> Result<BrowserState> {
        if !self.enabled {
            bail!("Puffer browser is disabled for this runtime");
        }
        let width = width.max(1);
        let height = height.max(1);
        let normalized_url = url.as_deref().map(normalize_url).transpose()?;
        if let Some(session) = self.live_session(&session_id) {
            if session.resize(width, height).is_ok() {
                let browser_state = session.state();
                self.record_backend_state(&session_id, &browser_state);
                return Ok(browser_state);
            }
        }
        self.close_stale_page_session(&session_id);
        let root_session_id = session_root_id(&session_id).to_string();
        let root = self.ensure_root_session(&root_session_id, width, height)?;
        let session = BrowserSession::spawn(
            events,
            Arc::clone(&self.recordings),
            Arc::clone(&self.console_logs),
            session_id.clone(),
            root,
            width,
            height,
        )?;
        if let Some(url) = normalized_url.as_deref().filter(|url| *url != DEFAULT_URL) {
            session.navigate(url.to_string())?;
        }
        let browser_state = session.state();
        self.sessions
            .lock()
            .unwrap()
            .insert(session_id.clone(), session);
        self.record_backend_state(&session_id, &browser_state);
        Ok(browser_state)
    }

    /// Navigates a live page worker.
    pub(crate) fn navigate(&self, session_id: &str, url: String) -> Result<()> {
        self.get(session_id)?.navigate(normalize_url(&url)?)
    }

    /// Waits for a live page worker to report that navigation has completed.
    pub(crate) fn wait_for_load(&self, session_id: &str, timeout: Duration) -> Result<()> {
        self.get(session_id)?.wait_for_load(timeout)
    }

    /// Reloads a live page worker.
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

    /// Resizes a live browser page worker viewport.
    pub(crate) fn resize(&self, session_id: &str, width: u32, height: u32) -> Result<()> {
        self.get(session_id)?.resize(width.max(1), height.max(1))
    }

    /// Sends a user input event to a live page worker.
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

    /// Closes a page worker by backend id or an entire root browser by root id.
    pub(crate) fn close(&self, session_id: &str) -> Result<()> {
        if parse_backend_session_id(session_id).is_some() {
            self.close_page_session(session_id, true);
            return Ok(());
        }
        self.shutdown_root(session_id, false)
    }

    /// Lists daemon-owned browser tabs for an agent session.
    pub(crate) fn list_tabs(&self, root_session_id: &str) -> BrowserTabsState {
        let mut state = self.tabs.lock().unwrap().list(root_session_id);
        let sessions = self.sessions.lock().unwrap();
        for tab in &mut state.tabs {
            if let Some(session) = sessions.get(&tab.backend_session_id) {
                if session.is_alive() {
                    let browser_state = session.state();
                    tab.url = browser_state.url;
                    tab.title = browser_state.title;
                    tab.loading = browser_state.loading;
                    tab.connected = true;
                } else {
                    tab.connected = false;
                    tab.loading = false;
                }
            }
        }
        state
    }

    /// Returns the active-tab context snapshot for one browser root session.
    pub(crate) fn current_tab_context(&self, root_session_id: &str) -> BrowserCurrentTabContext {
        let Some(mut tab) = self.tabs.lock().unwrap().active_tab(root_session_id) else {
            return BrowserCurrentTabContext::no_active_tab();
        };
        if let Some(session) = self.live_session(&tab.backend_session_id) {
            let browser_state = session.state();
            tab.url = browser_state.url;
            tab.title = browser_state.title;
            tab.loading = browser_state.loading;
            tab.connected = true;
        } else {
            tab.connected = false;
            tab.loading = false;
        }
        BrowserCurrentTabContext::from_tab(&tab)
    }

    /// Opens or reuses a tab inside the agent session browser set.
    pub(crate) fn open_tab(
        &self,
        events: broadcast::Sender<ServerEnvelope>,
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
        let reused_live = self.live_session(&backend_id).is_some();
        let recovery_url = url.clone().or_else(|| {
            self.tabs
                .lock()
                .unwrap()
                .tab(&root_session_id, &tab_id)
                .map(|tab| tab.url)
        });
        let mut browser_state =
            self.open(events, backend_id.clone(), recovery_url, width, height)?;
        if reused_live {
            if let Some(url) = url {
                self.navigate(&backend_id, url)?;
                browser_state = self.get(&backend_id)?.state();
            }
        }
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
        self.close_page_session(&backend_id, true);
        Ok(self.list_tabs(root_session_id))
    }

    /// Reads buffered console logs for a live browser page worker.
    pub(crate) fn console_logs(&self, session_id: &str, clear: bool) -> Result<Value> {
        self.get(session_id)?;
        let logs = self.console_logs.lock().unwrap().read(session_id, clear);
        let count = logs.len();
        Ok(json!({
            "logs": logs,
            "count": count,
            "cleared": clear
        }))
    }

    /// Tears down the shared Chrome root and every page worker below it.
    pub(crate) fn close_root(&self, root_session_id: &str) -> Result<()> {
        self.shutdown_root(root_session_id, false)
    }

    fn get(&self, session_id: &str) -> Result<BrowserSession> {
        let session = self
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .with_context(|| format!("no browser session `{session_id}`"))?;
        if !session.is_alive() {
            bail!("browser session `{session_id}` is disconnected");
        }
        session.touch();
        Ok(session)
    }

    fn live_session(&self, session_id: &str) -> Option<BrowserSession> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .filter(|session| session.is_alive())
    }

    fn ensure_root_session(
        &self,
        _root_session_id: &str,
        width: u32,
        height: u32,
    ) -> Result<BrowserRootSession> {
        if let Some(root) = self
            .roots
            .lock()
            .unwrap()
            .get(GLOBAL_BROWSER_ROOT_ID)
            .cloned()
            .filter(|root| root.is_alive())
        {
            root.touch();
            return Ok(root);
        }
        let _ = self.shutdown_global_root();
        let root = BrowserRootSession::spawn(
            self.profile_root.join(GLOBAL_BROWSER_ROOT_ID),
            width,
            height,
        )?;
        self.roots
            .lock()
            .unwrap()
            .insert(GLOBAL_BROWSER_ROOT_ID.to_string(), root.clone());
        Ok(root)
    }

    fn close_page_session(&self, session_id: &str, remove_tab_entry: bool) {
        if let Some(session) = self.remove_page_session(session_id) {
            let _ = session.close();
        }
        if let Some((root_session_id, tab_id)) = parse_backend_session_id(session_id) {
            let mut tabs = self.tabs.lock().unwrap();
            if remove_tab_entry {
                tabs.close_tab(root_session_id, tab_id);
            } else {
                tabs.remove_backend(root_session_id, tab_id);
            }
        }
    }

    fn close_stale_page_session(&self, session_id: &str) {
        if let Some(stale) = self.remove_page_session(session_id) {
            let _ = stale.close();
        }
    }

    fn remove_page_session(&self, session_id: &str) -> Option<BrowserSession> {
        self.agent_refs.lock().unwrap().remove(session_id);
        self.console_logs.lock().unwrap().remove(session_id);
        self.sessions.lock().unwrap().remove(session_id)
    }

    fn shutdown_root(&self, root_session_id: &str, preserve_tabs: bool) -> Result<()> {
        let sessions = drain_root_sessions(&self.sessions, root_session_id);
        let backend_ids = sessions
            .iter()
            .map(|(session_id, _)| session_id.clone())
            .collect::<Vec<_>>();
        cleanup_root_metadata(
            &self.tabs,
            &self.agent_refs,
            &self.console_logs,
            root_session_id,
            &backend_ids,
            preserve_tabs,
        );
        let shutdown_acks = sessions
            .iter()
            .filter_map(|(_, session)| session.begin_close())
            .collect::<Vec<_>>();
        wait_for_shutdown_acks(shutdown_acks, Duration::from_secs(5));
        if self.sessions.lock().unwrap().is_empty() {
            self.shutdown_global_root()?;
        }
        Ok(())
    }

    fn shutdown_global_root(&self) -> Result<()> {
        let root = self.roots.lock().unwrap().remove(GLOBAL_BROWSER_ROOT_ID);
        if let Some(root) = root {
            root.shutdown()?;
        }
        Ok(())
    }

    fn record_backend_state(&self, session_id: &str, browser_state: &BrowserState) {
        if let Some((root_session_id, tab_id)) = parse_backend_session_id(session_id) {
            self.tabs.lock().unwrap().record_opened_backend(
                root_session_id,
                tab_id,
                session_id.to_string(),
                browser_state.clone(),
            );
        }
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
}

fn spawn_idle_pruner(
    roots: Arc<Mutex<HashMap<String, BrowserRootSession>>>,
    sessions: Arc<Mutex<HashMap<String, BrowserSession>>>,
    tabs: Arc<Mutex<BrowserTabRegistry>>,
    agent_refs: Arc<Mutex<HashMap<String, Vec<BrowserElementRef>>>>,
    console_logs: Arc<Mutex<BrowserConsoleRegistry>>,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(60));
        let stale_roots = {
            let mut roots = roots.lock().unwrap();
            let stale_ids = roots
                .iter()
                .filter_map(|(root_session_id, root)| {
                    (root.idle_for() >= SESSION_IDLE_TIMEOUT).then(|| root_session_id.clone())
                })
                .collect::<Vec<_>>();
            stale_ids
                .into_iter()
                .filter_map(|root_session_id| {
                    roots
                        .remove(&root_session_id)
                        .map(|root| (root_session_id, root))
                })
                .collect::<Vec<_>>()
        };
        for (root_session_id, root) in stale_roots {
            let root_sessions = if root_session_id == GLOBAL_BROWSER_ROOT_ID {
                drain_all_sessions(&sessions)
            } else {
                drain_root_sessions(&sessions, &root_session_id)
            };
            let backend_ids = root_sessions
                .iter()
                .map(|(session_id, _)| session_id.clone())
                .collect::<Vec<_>>();
            cleanup_sessions_metadata(&tabs, &agent_refs, &console_logs, &backend_ids, true);
            for (_, session) in root_sessions {
                let _ = session.close();
            }
            let _ = root.shutdown();
        }
    });
}

fn drain_root_sessions(
    sessions: &Arc<Mutex<HashMap<String, BrowserSession>>>,
    root_session_id: &str,
) -> Vec<(String, BrowserSession)> {
    let mut sessions = sessions.lock().unwrap();
    let stale_ids = sessions
        .keys()
        .filter(|session_id| session_root_id(session_id) == root_session_id)
        .cloned()
        .collect::<Vec<_>>();
    stale_ids
        .into_iter()
        .filter_map(|session_id| sessions.remove_entry(&session_id))
        .collect()
}

fn drain_all_sessions(
    sessions: &Arc<Mutex<HashMap<String, BrowserSession>>>,
) -> Vec<(String, BrowserSession)> {
    sessions.lock().unwrap().drain().collect()
}

fn cleanup_root_metadata(
    tabs: &Arc<Mutex<BrowserTabRegistry>>,
    agent_refs: &Arc<Mutex<HashMap<String, Vec<BrowserElementRef>>>>,
    console_logs: &Arc<Mutex<BrowserConsoleRegistry>>,
    root_session_id: &str,
    backend_ids: &[String],
    preserve_tabs: bool,
) {
    {
        let mut refs = agent_refs.lock().unwrap();
        refs.retain(|session_id, _| session_root_id(session_id) != root_session_id);
    }
    if preserve_tabs {
        let mut console_logs = console_logs.lock().unwrap();
        for backend_id in backend_ids {
            console_logs.remove(backend_id);
        }
    } else {
        console_logs.lock().unwrap().remove_root(root_session_id);
    }
    let mut tabs = tabs.lock().unwrap();
    if preserve_tabs {
        for backend_id in backend_ids {
            if let Some((root_session_id, tab_id)) = parse_backend_session_id(backend_id) {
                tabs.remove_backend(root_session_id, tab_id);
            }
        }
    } else {
        tabs.remove_root(root_session_id);
    }
}

fn cleanup_sessions_metadata(
    tabs: &Arc<Mutex<BrowserTabRegistry>>,
    agent_refs: &Arc<Mutex<HashMap<String, Vec<BrowserElementRef>>>>,
    console_logs: &Arc<Mutex<BrowserConsoleRegistry>>,
    backend_ids: &[String],
    preserve_tabs: bool,
) {
    let mut grouped = HashMap::<String, Vec<String>>::new();
    for backend_id in backend_ids {
        if let Some((root_session_id, _)) = parse_backend_session_id(backend_id) {
            grouped
                .entry(root_session_id.to_string())
                .or_default()
                .push(backend_id.clone());
        }
    }
    for (root_session_id, ids) in grouped {
        cleanup_root_metadata(
            tabs,
            agent_refs,
            console_logs,
            &root_session_id,
            &ids,
            preserve_tabs,
        );
    }
}

fn wait_for_shutdown_acks(receivers: Vec<Receiver<()>>, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    for receiver in receivers {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let _ = receiver.recv_timeout(remaining);
    }
}

fn session_root_id(session_id: &str) -> &str {
    parse_backend_session_id(session_id)
        .map(|(root_session_id, _)| root_session_id)
        .unwrap_or(session_id)
}

fn normalize_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(DEFAULT_URL.to_string());
    }
    if trimmed == DEFAULT_URL {
        return Ok(DEFAULT_URL.to_string());
    }
    if let Ok(parsed) = Url::parse(trimmed) {
        if matches!(parsed.scheme(), "http" | "https" | "file" | "data") {
            return Ok(trimmed.to_string());
        }
    }
    let with_scheme = if trimmed.starts_with("localhost")
        || trimmed.starts_with("127.")
        || trimmed.starts_with("[::1]")
    {
        format!("http://{trimmed}")
    } else {
        format!("https://{trimmed}")
    };
    Url::parse(&with_scheme).with_context(|| format!("invalid browser URL `{raw}`"))?;
    Ok(with_scheme)
}

#[cfg(test)]
mod tests;
