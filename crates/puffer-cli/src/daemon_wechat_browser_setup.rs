//! Daemon-owned setup flow for the WeChat connector (GUI, from zero).
//!
//! Mirrors the gmail-browser setup but the "browser" being shown is the
//! instance's own KasmVNC desktop (native WeChat running in a container). The
//! flow, all driven from the desktop app with no terminal:
//!
//! 1. start the container runtime if needed (the Docker engine, or Apple
//!    `container` on macOS 26+), create the instance container, and install the
//!    native WeChat client into it,
//! 2. open the KasmVNC desktop in a puffer browser pane — WeChat shows its
//!    login QR there,
//! 3. ask the user to scan the QR with their phone and click Continue,
//! 4. poll the client until a logged-in (wide) WeChat window appears,
//! 5. register the connection.
//!
//! The data volume persists the login across restarts, so the scan is one-time.
//! Deleting the connection only STOPS the container (its volume, image, and
//! per-instance state are kept for a fast re-connect; see
//! `connection_delete::cleanup_wechat_container`). A later re-connect still needs
//! a fresh scan because the restart re-binds the WeChat device, signing the
//! previous session out.

use crate::daemon::{DaemonState, ServerEnvelope};
use crate::wechat_connector::{InstanceConfig, WechatInstance};
use anyhow::{bail, Context, Result};
use puffer_core::{CancelToken, UserQuestionPromptResponse};
use puffer_subscriptions::{ConnectionRecord, ConnectionState};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// Connector slug this setup handles (matches the catalog template).
pub(crate) const CONNECTOR_SLUG: &str = "wechat-login";
/// Default connection slug when the user does not name one.
const DEFAULT_CONNECTION: &str = "wechat-user";

const SETUP_TAB_ID: &str = "wechat-qr";
const BROWSER_WIDTH: u32 = 1100;
const BROWSER_HEIGHT: u32 = 820;

const SCAN_QUESTION: &str =
    "Scan the WeChat QR code shown on the left with your phone (WeChat ▸ + ▸ Scan).";
/// Interval between login-marker polls.
const LOGIN_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// How long to wait for the WeChat window to appear after install.
const WINDOW_WAIT_TIMEOUT: Duration = Duration::from_secs(90);
/// Total time to wait for the user to scan + log in before giving up.
const LOGIN_WAIT_TOTAL: Duration = Duration::from_secs(300);

type PendingQuestions = Arc<Mutex<HashMap<String, mpsc::Sender<UserQuestionPromptResponse>>>>;

/// Returns true when connector setup args target the WeChat connector.
pub(crate) fn connect_args_are_wechat(connect_args: &str) -> bool {
    connect_args
        .split_whitespace()
        .next()
        .is_some_and(|connector| connector == CONNECTOR_SLUG)
}

/// Executes daemon-native WeChat connector setup, returning a summary message.
pub(crate) fn execute_wechat_setup(
    state: Arc<DaemonState>,
    channel: String,
    turn_id: String,
    connect_args: String,
    next_request_id: Arc<AtomicU64>,
    pending_questions: PendingQuestions,
    cancel: CancelToken,
) -> Result<String> {
    let connection_slug = parse_connection_slug(&connect_args)?;
    let session_id = format!("wechat-browser-setup-{}", safe_session_part(&turn_id));
    // The setup runs on a blocking thread, so we own a small runtime to drive
    // the async docker helpers.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build wechat setup runtime")?;
    let flow = SetupFlow {
        state,
        channel,
        turn_id,
        next_request_id,
        pending_questions,
        cancel,
        session_id,
        connection_slug,
        rt,
    };
    flow.run()
}

struct SetupFlow {
    state: Arc<DaemonState>,
    channel: String,
    turn_id: String,
    next_request_id: Arc<AtomicU64>,
    pending_questions: PendingQuestions,
    cancel: CancelToken,
    session_id: String,
    connection_slug: String,
    rt: tokio::runtime::Runtime,
}

impl SetupFlow {
    fn run(&self) -> Result<String> {
        self.cancel.check()?;
        let cfg = InstanceConfig::load_or_create(&self.connection_slug)?;
        let instance = WechatInstance::for_connection(&self.connection_slug);

        // 1. Engine + container + client. These can take a while on a cold box;
        //    surface progress as health-style status events.
        self.status(
            "Preparing the WeChat container (first run builds the image and can take \
             several minutes)…",
        );
        self.rt
            .block_on(instance.ensure_container(&cfg))
            .context("bring up the WeChat container")?;

        // The desktop's reachable host:port depends on the runtime: Docker
        // publishes the web port to loopback, while Apple `container` exposes it
        // only at the container's vmnet IP. Resolve it once now that the
        // container exists, and use it for every URL shown to the user.
        let authority = self.rt.block_on(instance.desktop_authority(&cfg));

        // 2. Tell the user where the desktop lives (and, in test mode, the
        //    credentials) so they can open it manually if the embedded pane is
        //    unavailable.
        self.announce_desktop(&cfg, &authority);

        // 3. Make sure WeChat is installed (large download on first run).
        if !self.rt.block_on(instance.wechat_installed()).unwrap_or(false) {
            self.status("Downloading and installing WeChat into the container (large download, please wait)…");
            self.rt
                .block_on(instance.install_wechat())
                .context("install WeChat in the container")?;
        }

        // 4. Wait for the WeChat window (login QR) to appear.
        self.status("Waiting for WeChat to start…");
        self.wait_for_window(&instance)?;

        // 5. Show the QR in puffer's in-app browser pane (the KasmVNC desktop) —
        //    ON by default. Set WECHAT_EMBED_PANE=0 to instead show the URL for an
        //    external browser. Login detection works regardless of how it's shown.
        if embed_pane_enabled() {
            self.status("Opening the WeChat screen…");
            if let Err(error) = self.open_desktop(&cfg.kasm_url_for_authority(&authority)) {
                self.status(&format!(
                    "Could not embed the WeChat screen in-app ({error:#}). Open this URL in any browser to scan the QR: {}",
                    self.display_url(&cfg, &authority)
                ));
            }
        } else {
            self.status(&format!(
                "Open this URL in a browser to see the WeChat QR, then scan it with your phone: {}",
                self.display_url(&cfg, &authority)
            ));
        }

        // 6. Show the QR prompt and AUTO-ADVANCE the moment login is detected —
        //    the user does not need to click Continue.
        self.wait_for_login(&instance, &cfg, &authority)?;

        // 7. Register the connection.
        let registered = self.register()?;
        let action = if registered { "created" } else { "updated" };
        Ok(format!(
            "Connected WeChat as `{}` ({action}). To start receiving messages, click \
             \"Start monitoring\" on this connection (that's what activates the monitor); \
             sending is available immediately.",
            self.connection_slug
        ))
    }

    /// Opens the instance's KasmVNC desktop in a puffer browser pane.
    fn open_desktop(&self, url: &str) -> Result<()> {
        crate::daemon_browser::handle_browser_agent(
            &self.state,
            &json!({
                "action": "open",
                "sessionId": &self.session_id,
                "tabId": SETUP_TAB_ID,
                "label": "WeChat",
                "url": url,
                "width": BROWSER_WIDTH,
                "height": BROWSER_HEIGHT,
                "activate": true,
            }),
        )
        .with_context(|| format!("open WeChat desktop pane at {url}"))?;
        Ok(())
    }

    /// Waits until a WeChat window is visible (or times out, which is non-fatal
    /// — the user may still see the QR shortly after).
    fn wait_for_window(&self, instance: &WechatInstance) -> Result<()> {
        let deadline = Instant::now() + WINDOW_WAIT_TIMEOUT;
        loop {
            self.cancel.check()?;
            let width = self
                .rt
                .block_on(instance.max_wechat_window_width())
                .unwrap_or(0);
            if width > 0 || Instant::now() >= deadline {
                return Ok(());
            }
            std::thread::sleep(LOGIN_POLL_INTERVAL);
        }
    }

    /// Shows the QR prompt and waits for login, advancing AUTOMATICALLY the
    /// instant the account marker appears — the user never has to click Continue
    /// (though a manual Continue still works). Re-shows the prompt if the user
    /// confirms before logging in. Times out after [`LOGIN_WAIT_TOTAL`].
    fn wait_for_login(
        &self,
        instance: &WechatInstance,
        cfg: &InstanceConfig,
        authority: &str,
    ) -> Result<()> {
        if self.rt.block_on(instance.is_logged_in()).unwrap_or(false) {
            return Ok(());
        }
        let deadline = Instant::now() + LOGIN_WAIT_TOTAL;
        let (mut request_id, mut rx) = self.publish_scan_question(cfg, authority)?;
        loop {
            self.cancel.check()?;
            if self.rt.block_on(instance.is_logged_in()).unwrap_or(false) {
                self.pending_questions.lock().unwrap().remove(&request_id);
                self.status("WeChat login detected — finishing up…");
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.pending_questions.lock().unwrap().remove(&request_id);
                bail!("WeChat login was not detected in time; click WeChat ▸ Start to retry.");
            }
            // Block up to one poll interval for a manual Continue; on timeout we
            // loop and re-check the login marker.
            match rx.recv_timeout(LOGIN_POLL_INTERVAL) {
                Ok(_) => {
                    // Manual Continue but not logged in yet — re-show the prompt.
                    self.pending_questions.lock().unwrap().remove(&request_id);
                    let (rid, new_rx) = self.publish_scan_question(cfg, authority)?;
                    request_id = rid;
                    rx = new_rx;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.pending_questions.lock().unwrap().remove(&request_id);
                    bail!("connector setup question channel closed");
                }
            }
        }
    }

    /// Publishes a non-blocking status line to the setup event channel.
    fn status(&self, message: &str) {
        let mut payload = Map::new();
        payload.insert("type".to_string(), json!("connector-setup-status"));
        payload.insert("turnId".to_string(), json!(self.turn_id));
        payload.insert("status".to_string(), json!(message));
        self.state.publish_event(ServerEnvelope::Event {
            event: self.channel.clone(),
            payload: Value::Object(payload),
        });
    }

    /// Announces the desktop URL (and, in test mode, the credentials) so the
    /// user can reach the WeChat screen even if the embedded pane is unavailable.
    fn announce_desktop(&self, cfg: &InstanceConfig, authority: &str) {
        if show_creds() {
            self.status(&format!(
                "WeChat screen ready at http://{authority} — login `{}` / password `{}` \
                 (TEST credentials, shown only in test mode; production uses a random password \
                 that is never displayed).",
                cfg.kasm_user, cfg.kasm_password
            ));
        } else {
            self.status(&format!("WeChat screen ready at http://{authority}."));
        }
    }

    /// URL to show the user for manual access: credential-embedded in test mode,
    /// bare host:port in production.
    fn display_url(&self, cfg: &InstanceConfig, authority: &str) -> String {
        if show_creds() {
            cfg.kasm_url_for_authority(authority)
        } else {
            format!("http://{authority}/")
        }
    }

    /// Publishes the "scan the QR" prompt (which renders the desktop pane in the
    /// modal via `browserSessionId`) and returns the request id plus a receiver
    /// that fires if the user clicks Continue. The caller polls login alongside
    /// it and auto-advances, so Continue is optional.
    fn publish_scan_question(
        &self,
        cfg: &InstanceConfig,
        authority: &str,
    ) -> Result<(String, mpsc::Receiver<UserQuestionPromptResponse>)> {
        let display = self.display_url(cfg, authority);
        let request_id = self
            .next_request_id
            .fetch_add(1, Ordering::SeqCst)
            .to_string();
        let (tx, rx) = mpsc::channel();
        self.pending_questions
            .lock()
            .unwrap()
            .insert(request_id.clone(), tx);

        let mut payload = Map::new();
        payload.insert("type".to_string(), json!("user-question-request"));
        payload.insert("turnId".to_string(), json!(self.turn_id));
        payload.insert("requestId".to_string(), json!(request_id));
        payload.insert(
            "questions".to_string(),
            json!([{
                "type": "choice",
                "header": "Log in to WeChat",
                "question": format!("{SCAN_QUESTION} (It continues automatically once you log in. If the screen is blank, open {display} in a browser.)"),
                "multiSelect": false,
                "options": []
            }]),
        );
        payload.insert("browserSessionId".to_string(), json!(self.session_id));
        payload.insert("browserTabId".to_string(), json!(SETUP_TAB_ID));
        // The pane renders via `browserSessionId` (the daemon session that
        // open_desktop already authenticated with the credentialed URL), so the
        // user-facing `browserUrl` must be credential-free — never leak the KasmVNC
        // user/password in the URL sent to the client.
        payload.insert("browserUrl".to_string(), json!(display));
        self.state.publish_event(ServerEnvelope::Event {
            event: self.channel.clone(),
            payload: Value::Object(payload),
        });
        Ok((request_id, rx))
    }

    /// Registers (or refreshes) the WeChat connection as authenticated.
    fn register(&self) -> Result<bool> {
        let manager = puffer_core::subscription_manager()?;
        let description = format!("WeChat ({})", self.connection_slug);
        let registered = if let Some(existing) = manager.connection_store().get(&self.connection_slug)
        {
            if existing.connector_slug != CONNECTOR_SLUG {
                bail!(
                    "connection `{}` already exists for connector `{}`",
                    self.connection_slug,
                    existing.connector_slug
                );
            }
            manager
                .connection_store()
                .update(&self.connection_slug, |record| {
                    record.description = description.clone();
                    record.state = ConnectionState::Authenticated;
                    record.auth_failure_notified = false;
                })?;
            false
        } else {
            manager
                .connection_store()
                .create(ConnectionRecord::authenticated(
                    &self.connection_slug,
                    CONNECTOR_SLUG,
                    description,
                ))?;
            true
        };
        manager.refresh_connection_consumers()?;
        manager.refresh_connection_auth()?;
        Ok(registered)
    }
}

/// Whether to embed the KasmVNC desktop in an in-app browser pane. Default ON;
/// set `WECHAT_EMBED_PANE=0` to skip it and show the URL for an external browser.
fn embed_pane_enabled() -> bool {
    std::env::var("WECHAT_EMBED_PANE")
        .map(|value| value.trim() != "0")
        .unwrap_or(true)
}

/// Whether to reveal the KasmVNC credentials in user-facing status. Default OFF
/// (the random password is never shown); opt in with `WECHAT_SHOW_CREDS=1`. The
/// in-app pane authenticates via the URL regardless, so this only gates display.
fn show_creds() -> bool {
    std::env::var("WECHAT_SHOW_CREDS")
        .map(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Parses `wechat-login [<connection-name>]` into the connection slug.
fn parse_connection_slug(connect_args: &str) -> Result<String> {
    let mut parts = connect_args.split_whitespace();
    let connector = parts.next().unwrap_or_default();
    if connector != CONNECTOR_SLUG {
        bail!("expected wechat connector setup, got `{connector}`");
    }
    let slug = parts
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_CONNECTION);
    if parts.next().is_some() {
        bail!("Usage: /connect wechat-login <connection-name>");
    }
    Ok(slug.to_string())
}

/// Sanitizes a turn id into a browser-session-safe token.
fn safe_session_part(value: &str) -> String {
    let mut output = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    while output.contains("--") {
        output = output.replace("--", "-");
    }
    let trimmed = output.trim_matches('-');
    if trimmed.is_empty() {
        "wechat".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wechat_connect_args() {
        assert!(connect_args_are_wechat("wechat-login"));
        assert!(connect_args_are_wechat("wechat-login work"));
        assert!(!connect_args_are_wechat("gmail-browser"));
    }

    #[test]
    fn parses_connection_slug_with_default() {
        assert_eq!(parse_connection_slug("wechat-login").unwrap(), "wechat-user");
        assert_eq!(parse_connection_slug("wechat-login work").unwrap(), "work");
        assert!(parse_connection_slug("wechat-login a b").is_err());
        assert!(parse_connection_slug("gmail-browser").is_err());
    }

    #[test]
    fn safe_session_part_sanitizes() {
        assert_eq!(safe_session_part("abc/123"), "abc-123");
        assert_eq!(safe_session_part("--"), "wechat");
        assert_eq!(safe_session_part("a::b"), "a-b");
    }
}
