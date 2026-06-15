//! Root Chrome ownership and per-page CDP worker sessions.

mod pending;

use super::chrome::{
    close_page_target, create_page_target, ensure_chrome_executable, initial_page_target,
    read_devtools_ws_url, read_remote_devtools_ws_url, wait_for_initial_page_targets,
    ChromePageTarget,
};
use super::command::BrowserCommand;
use super::console::BrowserConsoleRegistry;
use super::cursor::{cursor_eval_expression, parse_cursor_response};
use super::devtools::{devtools_event_payload, emit_devtools_payload};
use super::extension_seed::{ensure_extensions_registered, seed_extensions};
use super::input::send_input;
use super::launch_settings::BrowserLaunchSettings;
use super::network_idle::BrowserNetworkState;
use super::recording::BrowserRecordingRegistry;
use super::screenshot::{
    capture_screenshot_command_params, parse_capture_screenshot_response,
    BrowserCaptureScreenshotOptions, BrowserCapturedScreenshot,
};
use super::selection::{parse_copy_selection_response, selection_eval_expression};
use super::session_launch::{
    cef_profile_dir, cef_remote_debugging_port, configure_chrome_command, log_browser_backend,
    remove_stale_devtools_port,
};
use super::state_events::{emit_state, emit_state_error, update_state_from_eval};
use super::upload::{
    parse_upload_handle_response, parse_upload_runtime_response, parse_upload_set_files_response,
    upload_finalize_function, upload_prepare_function,
};
use super::worker::{
    apply_viewport, frame_session_id_string, release_remote_object, send_state_eval,
    set_read_timeout, start_screencast,
};
use super::{
    parse_evaluation_response, send_cdp, BrowserCopySelection, BrowserCursor, BrowserEvaluation,
    BrowserHistoryDirection, BrowserInputEvent, BrowserState, CDP_READ_TIMEOUT, DEFAULT_URL,
};
use crate::browser_profiles::prepare_managed_profile;
use crate::daemon::ServerEnvelope;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

use pending::PendingKind;

const CEF_REMOTE_START_TIMEOUT: Duration = Duration::from_secs(30);
const CEF_TARGET_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// The native-CEF remote debugging port, if the desktop configured one. Used to
/// decide whether tab discovery may lazily attach to an existing CEF (#649)
/// without risking a managed-Chrome launch.
pub(super) fn native_cef_remote_port() -> Option<u16> {
    cef_remote_debugging_port()
}

/// Whether something is accepting TCP connections on the loopback CEF DevTools
/// port. A short connect timeout so a missing CEF runtime (e.g. `tauri dev`, which
/// doesn't stage CEF) falls back to the Chrome screencast backend fast instead of
/// blocking the open for the full CEF startup timeout.
fn cef_remote_port_listening(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(700)).is_ok()
}

#[derive(Clone)]
pub(super) struct BrowserRootSession {
    inner: Arc<Mutex<BrowserRootState>>,
}

struct BrowserRootState {
    profile_dir: PathBuf,
    browser_ws: String,
    child: Option<Child>,
    owns_targets: bool,
    reusable_targets: Vec<ChromePageTarget>,
    last_active: Instant,
}
#[derive(Clone)]
pub(super) struct BrowserSession {
    pub(super) tx: Sender<BrowserCommand>,
    pub(super) state: Arc<Mutex<BrowserState>>,
    pub(super) network: Arc<Mutex<BrowserNetworkState>>,
    pub(super) last_active: Arc<Mutex<Instant>>,
    pub(super) alive: Arc<AtomicBool>,
    pub(super) root: Option<BrowserRootSession>,
    pub(super) native_cef_session_id: Option<String>,
    /// The CDP target this worker drives. Used to reconcile the registry against
    /// the live DevTools target list so a target isn't adopted twice (#649).
    pub(super) target_id: Option<String>,
}
impl BrowserRootSession {
    /// Spawns one shared Chrome root owner for a browser session tree.
    pub(super) fn spawn(
        profile_dir: PathBuf,
        width: u32,
        height: u32,
        launch_settings: BrowserLaunchSettings,
    ) -> Result<Self> {
        if let Some(root) = Self::spawn_cef_remote_root(&profile_dir, &launch_settings)? {
            return Ok(root);
        }
        let chrome = ensure_chrome_executable()?;
        log_browser_backend(format!(
            "launching Chrome screencast fallback at {}",
            chrome.display()
        ));
        let launch = prepare_managed_profile(&profile_dir)?;
        if launch.owns_user_data_dir {
            super::chrome::terminate_profile_processes(&launch.user_data_dir);
            remove_stale_devtools_port(&launch.user_data_dir)?;
        }
        let mut command = Command::new(&chrome);
        configure_chrome_command(&mut command, &launch, width, height, &launch_settings);
        let mut child = command
            .spawn()
            .with_context(|| format!("launch Chrome at {}", chrome.display()))?;
        let browser_ws = match read_devtools_ws_url(&mut child, &launch.user_data_dir) {
            Ok(url) => url,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };
        if let Err(error) = ensure_extensions_registered(
            &browser_ws,
            Some(&launch.user_data_dir),
            launch_settings.extension_dirs(),
        ) {
            let _ = child.kill();
            return Err(error);
        }
        seed_extensions(&browser_ws, launch_settings.seeds())?;
        let reusable_target = match initial_page_target(&browser_ws) {
            Ok(target) => target,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };
        Ok(Self {
            inner: Arc::new(Mutex::new(BrowserRootState {
                profile_dir: launch.user_data_dir,
                browser_ws,
                child: Some(child),
                owns_targets: true,
                reusable_targets: reusable_target.into_iter().collect(),
                last_active: Instant::now(),
            })),
        })
    }
    /// Establishes a native-CEF remote root **without** the managed-Chrome
    /// fallback. Returns `Ok(None)` when no CEF endpoint is configured. Used to
    /// lazily attach to the desktop's CEF for tab discovery (#649) even when the
    /// agent never opened a browser itself — so a plain `list` can never end up
    /// launching a headless Chrome.
    pub(super) fn try_spawn_cef_remote(
        profile_dir: &PathBuf,
        launch_settings: &BrowserLaunchSettings,
    ) -> Result<Option<Self>> {
        Self::spawn_cef_remote_root(profile_dir, launch_settings)
    }

    fn spawn_cef_remote_root(
        profile_dir: &PathBuf,
        launch_settings: &BrowserLaunchSettings,
    ) -> Result<Option<Self>> {
        let Some(port) = cef_remote_debugging_port() else {
            log_browser_backend(
                "CEF remote debugging port is not configured; using Chrome fallback",
            );
            return Ok(None);
        };
        // Fast probe first: if nothing is listening on the CEF DevTools port —
        // e.g. the app was launched via `tauri dev`, where the embedded CEF
        // runtime is not staged (only release bundles stage it) — fall back to the
        // Chrome screencast backend IMMEDIATELY instead of polling the full
        // CEF_REMOTE_START_TIMEOUT (30s) and then erroring. That 30s error is what
        // the desktop surfaces as "Puffer daemon request timed out: browser_open".
        if !cef_remote_port_listening(port) {
            log_browser_backend(format!(
                "CEF DevTools port {port} is not listening; using Chrome screencast fallback"
            ));
            return Ok(None);
        }
        log_browser_backend(format!(
            "trying native CEF DevTools on loopback port {port}"
        ));
        let browser_ws = match read_remote_devtools_ws_url(port, CEF_REMOTE_START_TIMEOUT) {
            Ok(browser_ws) => browser_ws,
            // The port was reachable but the DevTools handshake never completed —
            // fall back to Chrome rather than failing the whole open.
            Err(error) => {
                log_browser_backend(format!(
                    "CEF DevTools on port {port} did not complete its handshake ({error:#}); \
                     using Chrome screencast fallback"
                ));
                return Ok(None);
            }
        };
        let reusable_targets =
            wait_for_initial_page_targets(&browser_ws, CEF_TARGET_DISCOVERY_TIMEOUT)
                .context("read CEF DevTools page targets")?;
        if reusable_targets.is_empty() {
            bail!("native CEF DevTools did not provide prewarmed page targets");
        }
        let cef_profile_dir = cef_profile_dir().unwrap_or_else(|| profile_dir.clone());
        ensure_extensions_registered(
            &browser_ws,
            Some(&cef_profile_dir),
            launch_settings.extension_dirs(),
        )?;
        seed_extensions(&browser_ws, launch_settings.seeds())?;
        log_browser_backend(format!("using native CEF DevTools at {browser_ws}"));
        Ok(Some(Self {
            inner: Arc::new(Mutex::new(BrowserRootState {
                profile_dir: cef_profile_dir,
                browser_ws,
                child: None,
                owns_targets: false,
                reusable_targets,
                last_active: Instant::now(),
            })),
        }))
    }

    pub(super) fn allocate_target(&self) -> Result<ChromePageTarget> {
        let mut inner = self.inner.lock().unwrap();
        inner.last_active = Instant::now();
        ensure_root_alive(&mut inner)?;
        if let Some(target) = inner.reusable_targets.pop() {
            return Ok(target);
        }
        if inner.owns_targets {
            create_page_target(&inner.browser_ws, DEFAULT_URL)
        } else {
            bail!(
                "native CEF DevTools has no available prewarmed page targets; restart Puffer with a larger PUFFER_CEF_PREWARM_TARGETS value"
            )
        }
    }

    pub(super) fn close_target(&self, target: &ChromePageTarget) -> Result<()> {
        // Adopted targets (#649) belong to the user, not the prewarm pool. On
        // release we simply detach the CDP worker: never reset the page, close
        // the target, or return it to the pool.
        if target.adopted {
            return Ok(());
        }
        let (browser_ws, owns_targets) = {
            let mut inner = self.inner.lock().unwrap();
            inner.last_active = Instant::now();
            ensure_root_alive(&mut inner)?;
            (inner.browser_ws.clone(), inner.owns_targets)
        };
        if !owns_targets && !target.close_on_release {
            // Reset is best-effort: a wedged/unresponsive page (issue #585) may
            // never answer the CDP reset, but we MUST still return the slot to the
            // pool. Dropping it here would permanently shrink the fixed native-CEF
            // prewarm pool, so one hung page could poison the whole browser tree.
            if let Err(error) = super::chrome::reset_reusable_page_target(target) {
                log_browser_backend(format!(
                    "reusable native CEF slot {} reset failed ({error}); returning it to the pool anyway",
                    target.target_id
                ));
            }
            let mut inner = self.inner.lock().unwrap();
            inner.last_active = Instant::now();
            inner.reusable_targets.push(target.clone());
            return Ok(());
        }
        close_page_target(&browser_ws, &target.target_id)
    }

    /// Terminates the shared Chrome process for this root owner.
    pub(super) fn shutdown(&self) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        while let Some(target) = inner.reusable_targets.pop() {
            if inner.owns_targets || target.close_on_release {
                let _ = close_page_target(&inner.browser_ws, &target.target_id);
            }
        }
        let Some(child) = inner.child.as_mut() else {
            return Ok(());
        };
        match child.try_wait().context("check Chrome shutdown status")? {
            Some(_) => Ok(()),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                Ok(())
            }
        }
    }

    pub(super) fn touch(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.last_active = Instant::now();
    }

    pub(super) fn idle_for(&self) -> Duration {
        self.inner.lock().unwrap().last_active.elapsed()
    }

    /// The shared browser-level DevTools WebSocket URL, used to enumerate live
    /// targets when reconciling user-opened tabs into the registry (#649).
    pub(super) fn browser_ws(&self) -> String {
        self.inner.lock().unwrap().browser_ws.clone()
    }

    /// Reports whether this root hands out a FIXED pool of prewarmed page targets
    /// (native CEF, `owns_targets == false`) that cannot be grown on demand. When
    /// true, an exhausted pool must be replenished by reclaiming an existing page
    /// worker's slot rather than creating a new target.
    pub(super) fn reuses_fixed_target_pool(&self) -> bool {
        !self.inner.lock().unwrap().owns_targets
    }

    pub(super) fn is_alive(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        match inner.child.as_mut() {
            Some(child) => child
                .try_wait()
                .map(|status| status.is_none())
                .unwrap_or(false),
            None => true,
        }
    }
}

impl BrowserSession {
    /// Spawns one page worker by allocating a fresh target from the root's pool.
    pub(super) fn spawn(
        events: broadcast::Sender<ServerEnvelope>,
        recordings: Arc<Mutex<BrowserRecordingRegistry>>,
        console_logs: Arc<Mutex<BrowserConsoleRegistry>>,
        session_id: String,
        root: BrowserRootSession,
        width: u32,
        height: u32,
        foreground: bool,
    ) -> Result<Self> {
        let target = root.allocate_target()?;
        Ok(Self::from_target(
            events,
            recordings,
            console_logs,
            session_id,
            root,
            target,
            width,
            height,
            foreground,
            BrowserState {
                url: DEFAULT_URL.to_string(),
                title: String::new(),
                loading: false,
                width,
                height,
            },
        ))
    }

    /// Spawns a page worker that *adopts* an existing CDP target the user opened
    /// directly in the native browser (issue #649). The target is not drawn from
    /// the prewarm pool, so the worker must never reset or pool it on release.
    /// The initial state is seeded from the live URL/title so `list`/`snapshot`
    /// reflect what the user is viewing immediately.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn adopt(
        events: broadcast::Sender<ServerEnvelope>,
        recordings: Arc<Mutex<BrowserRecordingRegistry>>,
        console_logs: Arc<Mutex<BrowserConsoleRegistry>>,
        session_id: String,
        root: BrowserRootSession,
        target: ChromePageTarget,
        width: u32,
        height: u32,
        foreground: bool,
        url: String,
        title: String,
    ) -> Self {
        Self::from_target(
            events,
            recordings,
            console_logs,
            session_id,
            root,
            target,
            width,
            height,
            foreground,
            BrowserState {
                url,
                title,
                loading: false,
                width,
                height,
            },
        )
    }

    /// Spawns the CDP worker thread bound to a resolved page target.
    #[allow(clippy::too_many_arguments)]
    fn from_target(
        events: broadcast::Sender<ServerEnvelope>,
        recordings: Arc<Mutex<BrowserRecordingRegistry>>,
        console_logs: Arc<Mutex<BrowserConsoleRegistry>>,
        session_id: String,
        root: BrowserRootSession,
        target: ChromePageTarget,
        width: u32,
        height: u32,
        foreground: bool,
        initial: BrowserState,
    ) -> Self {
        let native_cef_session_id = target.native_cef_session_id.clone();
        let target_id = Some(target.target_id.clone());
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(initial));
        let network = Arc::new(Mutex::new(BrowserNetworkState::default()));
        let last_active = Arc::new(Mutex::new(Instant::now()));
        let alive = Arc::new(AtomicBool::new(true));
        let worker_state = Arc::clone(&state);
        let worker_network = Arc::clone(&network);
        let worker_alive = Arc::clone(&alive);
        let worker_root = root.clone();
        std::thread::spawn(move || {
            run_cdp_worker(
                events,
                recordings,
                console_logs,
                session_id,
                worker_root,
                target,
                rx,
                worker_state,
                worker_network,
                worker_alive,
                width,
                height,
                foreground,
            );
        });
        Self {
            tx,
            state,
            network,
            last_active,
            alive,
            root: Some(root),
            native_cef_session_id,
            target_id,
        }
    }

    /// Returns the native desktop CEF slot id that owns this CDP target, when known.
    pub(super) fn native_cef_session_id(&self) -> Option<String> {
        self.native_cef_session_id.clone()
    }

    /// Returns the CDP target id this worker drives, when known.
    pub(super) fn target_id(&self) -> Option<String> {
        self.target_id.clone()
    }
    pub(super) fn state(&self) -> BrowserState {
        self.touch();
        self.state.lock().unwrap().clone()
    }
    pub(super) fn navigate(&self, url: String) -> Result<()> {
        self.send(BrowserCommand::Navigate(url.clone()))?;
        let mut state = self.state.lock().unwrap();
        state.url = url;
        state.loading = true;
        Ok(())
    }

    /// Waits until the page worker reports that the current document reached DOM ready.
    pub(super) fn wait_for_load(&self, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        loop {
            if !self.state.lock().unwrap().loading {
                return Ok(());
            }
            if start.elapsed() >= timeout {
                bail!("timed out waiting for browser page load");
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Waits until the page worker reports no active network requests for `idle`.
    pub(super) fn wait_for_network_idle(&self, idle: Duration, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        loop {
            let active_count = {
                let network = self.network.lock().unwrap();
                if network.is_idle_for(idle) {
                    return Ok(());
                }
                network.active_count()
            };
            if start.elapsed() >= timeout {
                bail!(
                    "timed out waiting for browser network idle after {}ms with {active_count} active request(s)",
                    timeout.as_millis()
                );
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    pub(super) fn reload(&self) -> Result<()> {
        self.send(BrowserCommand::Reload)
    }
    pub(super) fn history(&self, direction: BrowserHistoryDirection) -> Result<()> {
        self.send(BrowserCommand::History(direction))
    }
    pub(super) fn resize(&self, width: u32, height: u32) -> Result<()> {
        self.send(BrowserCommand::Resize { width, height })
    }
    pub(super) fn input(&self, event: BrowserInputEvent) -> Result<()> {
        self.send(BrowserCommand::Input(event))
    }

    pub(super) fn copy_selection(&self) -> Result<BrowserCopySelection> {
        let (reply, rx) = mpsc::channel();
        self.send(BrowserCommand::CopySelection { reply })?;
        rx.recv_timeout(Duration::from_secs(5))
            .context("timed out waiting for browser selection")?
            .map_err(|message| anyhow!("{message}"))
    }

    pub(super) fn cursor(&self, x: f64, y: f64) -> Result<BrowserCursor> {
        let (reply, rx) = mpsc::channel();
        self.send(BrowserCommand::Cursor { x, y, reply })?;
        rx.recv_timeout(Duration::from_secs(2))
            .context("timed out waiting for browser cursor")?
            .map_err(|message| anyhow!("{message}"))
    }

    pub(super) fn evaluate(&self, expression: String) -> Result<BrowserEvaluation> {
        let (reply, rx) = mpsc::channel();
        self.send(BrowserCommand::Evaluate { expression, reply })?;
        rx.recv_timeout(Duration::from_secs(5))
            .context("timed out waiting for browser evaluation")?
            .map_err(|message| anyhow!("{message}"))
    }

    /// Captures one still screenshot of the current browser viewport.
    pub(super) fn capture_screenshot(
        &self,
        options: BrowserCaptureScreenshotOptions,
    ) -> Result<BrowserCapturedScreenshot> {
        let (reply, rx) = mpsc::channel();
        self.send(BrowserCommand::CaptureScreenshot { options, reply })?;
        rx.recv_timeout(Duration::from_secs(5))
            .context("timed out waiting for browser screenshot")?
            .map_err(|message| anyhow!("{message}"))
    }

    /// Uploads one or more files into a native file input selected by JavaScript.
    pub(super) fn upload(&self, expression: String, files: Vec<String>) -> Result<()> {
        let (reply, rx) = mpsc::channel();
        self.send(BrowserCommand::Upload {
            expression,
            files,
            reply,
        })?;
        rx.recv_timeout(Duration::from_secs(10))
            .context("timed out waiting for browser upload")?
            .map_err(|message| anyhow!("{message}"))
    }

    /// Starts shutting down the page worker and returns the shutdown ack receiver.
    pub(super) fn begin_close(&self) -> Option<Receiver<()>> {
        if !self.is_alive() {
            return None;
        }
        let (reply, rx) = mpsc::channel();
        match self.tx.send(BrowserCommand::Close { reply }) {
            Ok(()) => {
                self.touch();
                Some(rx)
            }
            Err(_) => {
                self.alive.store(false, Ordering::SeqCst);
                None
            }
        }
    }

    pub(super) fn close(&self) -> Result<()> {
        if let Some(rx) = self.begin_close() {
            rx.recv_timeout(Duration::from_secs(5))
                .context("timed out waiting for browser worker shutdown")?;
        }
        Ok(())
    }

    pub(super) fn touch(&self) {
        *self.last_active.lock().unwrap() = Instant::now();
        if let Some(root) = &self.root {
            root.touch();
        }
    }

    pub(super) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Time since this page worker last serviced an action. Used to pick the
    /// least-recently-active worker when reclaiming a native-CEF prewarm slot.
    pub(super) fn idle_for(&self) -> Duration {
        self.last_active.lock().unwrap().elapsed()
    }

    fn send(&self, command: BrowserCommand) -> Result<()> {
        self.touch();
        self.tx
            .send(command)
            .context("browser worker stopped")
            .map_err(|error| {
                self.alive.store(false, Ordering::SeqCst);
                error
            })
    }
}

fn run_cdp_worker(
    events: broadcast::Sender<ServerEnvelope>,
    recordings: Arc<Mutex<BrowserRecordingRegistry>>,
    console_logs: Arc<Mutex<BrowserConsoleRegistry>>,
    session_id: String,
    root: BrowserRootSession,
    target: ChromePageTarget,
    rx: Receiver<BrowserCommand>,
    state: Arc<Mutex<BrowserState>>,
    network: Arc<Mutex<BrowserNetworkState>>,
    alive: Arc<AtomicBool>,
    width: u32,
    height: u32,
    foreground: bool,
) {
    let channel_frame = format!("browser:{session_id}:frame");
    let channel_state = format!("browser:{session_id}:state");
    let channel_devtools = format!("browser:{session_id}:devtools");
    let mut shutdown_reply = None;
    let mut socket = match connect(target.page_ws.as_str()) {
        Ok((socket, _)) => socket,
        Err(error) => {
            emit_state_error(&events, &channel_state, error);
            let _ = root.close_target(&target);
            alive.store(false, Ordering::SeqCst);
            return;
        }
    };
    set_read_timeout(&socket, Some(CDP_READ_TIMEOUT));

    // Adopted user tabs (#649) render through the desktop's own native NSView, so
    // the daemon must NOT screencast them or override their viewport — that would
    // disrupt the page the user is looking at. Only managed headless Chrome
    // targets (no native slot, not adopted) need screencast to be visible.
    let use_screencast = target.native_cef_session_id.is_none() && !target.adopted;
    let mut next_id = 1u64;
    let mut pending = HashMap::<u64, PendingKind>::new();
    let _ = send_cdp(&mut socket, &mut next_id, "Page.enable", json!({}));
    let _ = send_cdp(&mut socket, &mut next_id, "Runtime.enable", json!({}));
    let _ = send_cdp(&mut socket, &mut next_id, "DOM.enable", json!({}));
    let _ = send_cdp(&mut socket, &mut next_id, "Log.enable", json!({}));
    let _ = send_cdp(&mut socket, &mut next_id, "Network.enable", json!({}));
    if foreground {
        let _ = send_cdp(&mut socket, &mut next_id, "Page.bringToFront", json!({}));
    }
    if use_screencast {
        let _ = apply_viewport(&mut socket, &mut next_id, width, height);
        let _ = start_screencast(&mut socket, &mut next_id, width, height);
    }
    let id = send_state_eval(&mut socket, &mut next_id);
    pending.insert(id, PendingKind::StateEval);

    let mut running = true;
    while running {
        loop {
            match rx.try_recv() {
                Ok(BrowserCommand::Close { reply }) => {
                    shutdown_reply = Some(reply);
                    running = false;
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    running = false;
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
                    foreground,
                    use_screencast,
                ),
                Err(TryRecvError::Empty) => break,
            }
        }
        if !running {
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
                        &console_logs,
                        &mut socket,
                        &mut next_id,
                        &mut pending,
                        &state,
                        &network,
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
    let _ = root.close_target(&target);
    alive.store(false, Ordering::SeqCst);
    if let Some(reply) = shutdown_reply {
        let _ = reply.send(());
    }
}

fn handle_command(
    command: BrowserCommand,
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: &mut u64,
    pending: &mut HashMap<u64, PendingKind>,
    state: &Arc<Mutex<BrowserState>>,
    channel_state: &str,
    events: &broadcast::Sender<ServerEnvelope>,
    foreground: bool,
    use_screencast: bool,
) {
    // Browser action log: every executed command lands in the durable process
    // log (~/.puffer/logs/puffer.log) with bounded, secret-free detail.
    // Input/Cursor are demoted to debug: unthrottled pointermove streams from
    // the screencast pane would otherwise write 100+ info lines/sec.
    {
        let (action, detail) = command.log_summary();
        match command {
            BrowserCommand::Input(_) | BrowserCommand::Cursor { .. } => tracing::debug!(
                target: "puffer::browser",
                channel = %channel_state,
                action,
                detail = %detail,
                "browser command"
            ),
            _ => tracing::info!(
                target: "puffer::browser",
                channel = %channel_state,
                action,
                detail = %detail,
                "browser command"
            ),
        }
    }
    match command {
        BrowserCommand::Navigate(url) => {
            {
                let mut state = state.lock().unwrap();
                state.url = url.clone();
                state.loading = true;
                emit_state(events, channel_state, &state);
            }
            if foreground {
                let _ = send_cdp(socket, next_id, "Page.bringToFront", json!({}));
            }
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
            if foreground {
                let _ = send_cdp(socket, next_id, "Page.bringToFront", json!({}));
            }
            if use_screencast {
                let _ = apply_viewport(socket, next_id, width, height);
                let _ = send_cdp(socket, next_id, "Page.stopScreencast", json!({}));
                let _ = start_screencast(socket, next_id, width, height);
            }
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
        BrowserCommand::CaptureScreenshot { options, reply } => {
            let id = send_cdp(
                socket,
                next_id,
                "Page.captureScreenshot",
                capture_screenshot_command_params(options),
            );
            pending.insert(
                id,
                PendingKind::CaptureScreenshot {
                    format: options.format,
                    reply,
                },
            );
        }
        BrowserCommand::Upload {
            expression,
            files,
            reply,
        } => {
            if foreground {
                let _ = send_cdp(socket, next_id, "Page.bringToFront", json!({}));
            }
            let id = send_cdp(
                socket,
                next_id,
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": false,
                    "awaitPromise": true
                }),
            );
            pending.insert(id, PendingKind::UploadResolve { files, reply });
        }
        BrowserCommand::Close { .. } => {}
    }
}

fn handle_cdp_message(
    events: &broadcast::Sender<ServerEnvelope>,
    channel_frame: &str,
    channel_state: &str,
    channel_devtools: &str,
    session_id: &str,
    recordings: &Arc<Mutex<BrowserRecordingRegistry>>,
    console_logs: &Arc<Mutex<BrowserConsoleRegistry>>,
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: &mut u64,
    pending: &mut HashMap<u64, PendingKind>,
    state: &Arc<Mutex<BrowserState>>,
    network: &Arc<Mutex<BrowserNetworkState>>,
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
            Some(PendingKind::CaptureScreenshot { format, reply }) => {
                let result = parse_capture_screenshot_response(&value, format)
                    .map_err(|error| format!("{error:#}"));
                let _ = reply.send(result);
            }
            Some(PendingKind::UploadResolve { files, reply }) => {
                match parse_upload_handle_response(&value) {
                    Ok(object_id) => {
                        let id = send_cdp(
                            socket,
                            next_id,
                            "Runtime.callFunctionOn",
                            json!({
                                "objectId": &object_id,
                                "functionDeclaration": upload_prepare_function(),
                                "arguments": [{ "value": files.len() }],
                                "returnByValue": true,
                                "awaitPromise": true
                            }),
                        );
                        pending.insert(
                            id,
                            PendingKind::UploadPrepare {
                                object_id,
                                files,
                                reply,
                            },
                        );
                    }
                    Err(error) => {
                        let _ = reply.send(Err(format!("{error:#}")));
                    }
                }
            }
            Some(PendingKind::UploadPrepare {
                object_id,
                files,
                reply,
            }) => match parse_upload_runtime_response(&value, "browser upload target validation") {
                Ok(()) => {
                    let id = send_cdp(
                        socket,
                        next_id,
                        "DOM.setFileInputFiles",
                        json!({
                            "objectId": &object_id,
                            "files": files
                        }),
                    );
                    pending.insert(id, PendingKind::UploadSetFiles { object_id, reply });
                }
                Err(error) => {
                    release_remote_object(socket, next_id, &object_id);
                    let _ = reply.send(Err(format!("{error:#}")));
                }
            },
            Some(PendingKind::UploadSetFiles { object_id, reply }) => {
                match parse_upload_set_files_response(&value) {
                    Ok(()) => {
                        let id = send_cdp(
                            socket,
                            next_id,
                            "Runtime.callFunctionOn",
                            json!({
                                "objectId": &object_id,
                                "functionDeclaration": upload_finalize_function(),
                                "returnByValue": true,
                                "awaitPromise": true
                            }),
                        );
                        pending.insert(id, PendingKind::UploadFinalize { object_id, reply });
                    }
                    Err(error) => {
                        release_remote_object(socket, next_id, &object_id);
                        let _ = reply.send(Err(format!("{error:#}")));
                    }
                }
            }
            Some(PendingKind::UploadFinalize { object_id, reply }) => {
                let result = parse_upload_runtime_response(&value, "browser upload event dispatch");
                release_remote_object(socket, next_id, &object_id);
                let _ = reply.send(result.map_err(|error| format!("{error:#}")));
            }
            None => {}
        }
        return;
    }
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return;
    };
    network.lock().unwrap().update_from_cdp(method, &value);
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
            let _ = events.send(ServerEnvelope::Event {
                event: channel_frame.to_string(),
                payload: json!({
                    "frameId": frame_session_id_string(session_id, params.get("sessionId")),
                    "mimeType": "image/jpeg",
                    "encoding": "base64",
                    "data": data,
                    "width": width,
                    "height": height
                }),
            });
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
                let _ = events.send(ServerEnvelope::Event {
                    event: format!("browser:{}:recording", frame.root_session_id),
                    payload: serde_json::to_value(frame).unwrap_or_else(|_| json!({})),
                });
            }
        }
        "Page.domContentEventFired" | "Page.loadEventFired" | "Page.frameStoppedLoading" => {
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
        _ => {
            if let Some(payload) = devtools_event_payload(method, &value) {
                console_logs.lock().unwrap().record(session_id, &payload);
                emit_devtools_payload(events, channel_devtools, payload);
            }
        }
    }
}

fn ensure_root_alive(inner: &mut BrowserRootState) -> Result<()> {
    let Some(child) = inner.child.as_mut() else {
        return Ok(());
    };
    if let Some(status) = child.try_wait().context("check Chrome root status")? {
        bail!(
            "Chrome root {} exited: {status}",
            inner.profile_dir.display()
        );
    }
    Ok(())
}
