use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};
use url::Url;

use super::ct_runtime;
use super::worker::set_read_timeout;
use super::{send_cdp, CDP_READ_TIMEOUT, CHROME_START_TIMEOUT, DEFAULT_URL};

const NATIVE_CEF_SLOT_FRAGMENT_PREFIX: &str = "puffer-cef-slot=";
const TARGET_RESET_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub(super) struct ChromePageTarget {
    pub(super) target_id: String,
    pub(super) page_ws: String,
    pub(super) close_on_release: bool,
    pub(super) native_cef_session_id: Option<String>,
    /// True for a target the daemon *adopted* (a tab the user opened directly in
    /// the native browser, issue #649) rather than allocated from the prewarm
    /// pool. Adopted targets are detach-only on release: the user owns the page,
    /// so the daemon must never reset, close, or return it to the pool.
    pub(super) adopted: bool,
}

/// A live page target discovered on the DevTools endpoint, carrying the live URL
/// and title so a newly adopted tab reflects what the user is actually viewing
/// without waiting for the page worker's first state evaluation.
#[derive(Clone, Debug)]
pub(super) struct DiscoveredTarget {
    pub(super) target_id: String,
    pub(super) page_ws: String,
    pub(super) url: String,
    pub(super) title: String,
    pub(super) native_cef_session_id: Option<String>,
}

impl ChromePageTarget {
    /// Builds a target wrapper for a user-opened page the daemon is adopting.
    pub(super) fn adopted(target_id: String, page_ws: String) -> Self {
        Self {
            target_id,
            page_ws,
            close_on_release: false,
            native_cef_session_id: None,
            adopted: true,
        }
    }
}

/// Waits for Chrome to publish its browser-level DevTools WebSocket URL.
pub(super) fn read_devtools_ws_url(child: &mut Child, profile_dir: &Path) -> Result<String> {
    let stderr = child.stderr.take().context("Chrome stderr missing")?;
    spawn_stderr_drain(BufReader::new(stderr));
    let active_port_path = profile_dir.join("DevToolsActivePort");
    let start = Instant::now();
    while start.elapsed() < CHROME_START_TIMEOUT {
        if let Ok(contents) = std::fs::read_to_string(&active_port_path) {
            if let Some(url) = devtools_url_from_active_port(&contents) {
                return Ok(url);
            }
        }
        if let Some(status) = child.try_wait().context("check Chrome launch status")? {
            bail!("Chrome exited before publishing a DevTools endpoint: {status}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("Chrome did not publish a DevTools endpoint");
}

fn devtools_url_from_active_port(contents: &str) -> Option<String> {
    let mut lines = contents.lines();
    let port = lines.next()?.trim();
    let path = lines.next()?.trim();
    if port.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("ws://127.0.0.1:{port}{path}"))
}

fn spawn_stderr_drain<R: std::io::Read + Send + 'static>(mut reader: BufReader<R>) {
    std::thread::spawn(move || {
        let mut sink = String::new();
        while reader.read_line(&mut sink).unwrap_or(0) > 0 {
            sink.clear();
        }
    });
}

/// Returns the initial Chrome page target created during browser launch, if present.
pub(super) fn initial_page_target(browser_ws: &str) -> Result<Option<ChromePageTarget>> {
    Ok(initial_page_targets(browser_ws)?.into_iter().next())
}

/// Returns all page targets currently published by the DevTools endpoint.
pub(super) fn initial_page_targets(browser_ws: &str) -> Result<Vec<ChromePageTarget>> {
    if let Ok(targets) = initial_page_targets_via_cdp(browser_ws) {
        return Ok(targets);
    }
    initial_page_targets_via_http(browser_ws)
}

fn initial_page_targets_via_http(browser_ws: &str) -> Result<Vec<ChromePageTarget>> {
    let endpoint = format!("{}/json/list", devtools_http_base(browser_ws)?);
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build Chrome HTTP client")?;
    let value: Value = client
        .get(endpoint)
        .send()
        .context("list Chrome targets")?
        .error_for_status()
        .context("Chrome target listing failed")?
        .json()
        .context("parse Chrome target listing")?;
    let Some(targets) = value.as_array() else {
        bail!("Chrome target listing response was not an array");
    };
    targets
        .iter()
        .filter(|target| is_reusable_page_target(target))
        .map(|target| parse_page_target(target, false))
        .collect()
}

fn initial_page_targets_via_cdp(browser_ws: &str) -> Result<Vec<ChromePageTarget>> {
    let (mut socket, _) = connect(browser_ws).context("connect to browser DevTools websocket")?;
    set_read_timeout(&socket, Some(CDP_READ_TIMEOUT));
    let mut next_id = 1u64;
    let id = send_cdp(
        &mut socket,
        &mut next_id,
        "Target.getTargets",
        serde_json::json!({}),
    );
    let response = wait_for_cdp_response(&mut socket, id, "list browser targets")?;
    let targets = response
        .get("result")
        .and_then(|result| result.get("targetInfos"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Target.getTargets response missing targetInfos"))?;
    targets
        .iter()
        .filter(|target| is_reusable_target_info(target))
        .map(|target| parse_target_info(browser_ws, target))
        .collect()
}

/// Waits for a page target to appear on an existing DevTools endpoint.
pub(super) fn wait_for_initial_page_target(
    browser_ws: &str,
    timeout: Duration,
) -> Result<Option<ChromePageTarget>> {
    Ok(wait_for_initial_page_targets(browser_ws, timeout)?
        .into_iter()
        .next())
}

/// Waits for at least one page target to appear on an existing DevTools endpoint.
pub(super) fn wait_for_initial_page_targets(
    browser_ws: &str,
    timeout: Duration,
) -> Result<Vec<ChromePageTarget>> {
    let start = Instant::now();
    let mut last_error = None;
    loop {
        match initial_page_targets(browser_ws) {
            Ok(targets) if !targets.is_empty() => return Ok(targets),
            Ok(_) => {}
            Err(error) => last_error = Some(error),
        }
        if start.elapsed() >= timeout {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Some(error) = last_error {
        return Err(error).context("wait for Chrome page target");
    }
    Ok(Vec::new())
}

/// Waits for an already-running DevTools HTTP endpoint to publish its browser WebSocket URL.
pub(super) fn read_remote_devtools_ws_url(port: u16, timeout: Duration) -> Result<String> {
    let endpoints = remote_devtools_version_endpoints(port);
    let client = Client::builder()
        .connect_timeout(Duration::from_millis(100))
        .timeout(Duration::from_millis(250))
        .build()
        .context("build DevTools HTTP client")?;
    let start = Instant::now();
    while start.elapsed() < timeout {
        for endpoint in &endpoints {
            if let Ok(response) = client.get(endpoint).send() {
                if let Ok(response) = response.error_for_status() {
                    if let Ok(value) = response.json::<Value>() {
                        if let Some(ws) = value.get("webSocketDebuggerUrl").and_then(Value::as_str)
                        {
                            return Ok(ws.to_string());
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("DevTools endpoint on loopback port {port} did not publish a browser WebSocket URL");
}

fn remote_devtools_version_endpoints(port: u16) -> Vec<String> {
    vec![
        format!("http://127.0.0.1:{port}/json/version"),
        format!("http://[::1]:{port}/json/version"),
    ]
}

/// Creates a new Chrome page target and returns its target id and DevTools WebSocket URL.
pub(super) fn create_page_target(browser_ws: &str, url: &str) -> Result<ChromePageTarget> {
    let endpoint = format!(
        "{}/json/new?{}",
        devtools_http_base(browser_ws)?,
        urlencoding(url)
    );
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build Chrome HTTP client")?;
    let value: Value = client
        .put(endpoint)
        .send()
        .context("create Chrome target")?
        .error_for_status()
        .context("Chrome target creation failed")?
        .json()
        .context("parse Chrome target response")?;
    parse_page_target(&value, true)
}

/// Closes one Chrome page target by target id.
pub(super) fn close_page_target(browser_ws: &str, target_id: &str) -> Result<()> {
    let endpoint = format!("{}/json/close/{target_id}", devtools_http_base(browser_ws)?);
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build Chrome HTTP client")?;
    client
        .get(endpoint)
        .send()
        .context("close Chrome target")?
        .error_for_status()
        .context("Chrome target close failed")?;
    Ok(())
}

/// Restores one reusable remote page target to its neutral blank slot state.
pub(super) fn reset_reusable_page_target(target: &ChromePageTarget) -> Result<()> {
    let reset_url = reusable_page_target_reset_url(target);
    let (mut socket, _) = connect(target.page_ws.as_str())
        .with_context(|| format!("connect to reusable page target {}", target.target_id))?;
    set_read_timeout(&socket, Some(CDP_READ_TIMEOUT));
    let mut next_id = 1u64;
    let id = send_cdp(
        &mut socket,
        &mut next_id,
        "Page.stopLoading",
        serde_json::json!({}),
    );
    wait_for_cdp_response(&mut socket, id, "stop reusable target loading")?;
    let id = send_cdp(
        &mut socket,
        &mut next_id,
        "Page.navigate",
        serde_json::json!({ "url": reset_url }),
    );
    wait_for_cdp_response(&mut socket, id, "navigate reusable target to neutral URL")?;
    let id = send_cdp(
        &mut socket,
        &mut next_id,
        "Page.resetNavigationHistory",
        serde_json::json!({}),
    );
    wait_for_cdp_response(&mut socket, id, "reset reusable target history")?;
    Ok(())
}

/// Finds the custom Chromium executable Puffer should manage.
pub(super) fn resolve_chrome_executable() -> Option<PathBuf> {
    ct_runtime::discover_chrome_executable()
}

/// Finds or downloads the custom Chromium executable Puffer should manage.
pub(super) fn ensure_chrome_executable() -> Result<PathBuf> {
    ct_runtime::ensure_chrome_executable()
}

/// Terminates orphaned Chrome processes that still own `profile_dir`.
pub(super) fn terminate_profile_processes(profile_dir: &Path) {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    terminate_profile_processes_unix(profile_dir);

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = profile_dir;
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn terminate_profile_processes_unix(profile_dir: &Path) {
    let profile = profile_dir.to_string_lossy();
    let Ok(output) = Command::new("ps").args(["-axo", "pid=,command="]).output() else {
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pids = stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, command) = line.split_once(char::is_whitespace)?;
            let is_chrome = command.contains("Chrome")
                || command.contains("chrome")
                || command.contains("Chromium")
                || command.contains("chromium");
            let owns_profile = is_chrome
                && command.contains("--user-data-dir=")
                && command.contains(profile.as_ref());
            owns_profile.then(|| pid.to_string())
        })
        .collect::<Vec<_>>();
    if pids.is_empty() {
        return;
    }
    for pid in &pids {
        let _ = Command::new("kill").arg("-TERM").arg(pid).status();
    }
    std::thread::sleep(Duration::from_millis(250));
    for pid in pids {
        if process_is_alive(&pid) {
            let _ = Command::new("kill").arg("-KILL").arg(pid).status();
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn process_is_alive(pid: &str) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn parse_page_target(value: &Value, close_on_release: bool) -> Result<ChromePageTarget> {
    let target_id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("Chrome target response missing id"))?;
    let page_ws = value
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("Chrome target response missing webSocketDebuggerUrl"))?;
    let native_cef_session_id = value
        .get("url")
        .and_then(Value::as_str)
        .and_then(native_cef_session_id_from_url);
    Ok(ChromePageTarget {
        target_id: target_id.to_string(),
        page_ws: page_ws.to_string(),
        close_on_release,
        native_cef_session_id,
        adopted: false,
    })
}

fn parse_target_info(browser_ws: &str, value: &Value) -> Result<ChromePageTarget> {
    let target_id = value
        .get("targetId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("Target info missing targetId"))?;
    let url = value.get("url").and_then(Value::as_str).unwrap_or_default();
    Ok(ChromePageTarget {
        target_id: target_id.to_string(),
        page_ws: page_ws_for_target(browser_ws, target_id)?,
        close_on_release: false,
        native_cef_session_id: native_cef_session_id_from_url(url),
        adopted: false,
    })
}

/// Lists ALL live page targets from the DevTools HTTP `/json/list` endpoint,
/// including pages the user opened directly in the native browser (which carry
/// real URLs and no prewarm-slot marker). Unlike [`initial_page_targets`] this
/// does NOT filter to reusable about:blank/slot targets — it is the discovery
/// side of issue #649, letting the daemon adopt user-opened tabs.
pub(super) fn discover_page_targets(browser_ws: &str) -> Result<Vec<DiscoveredTarget>> {
    let endpoint = format!("{}/json/list", devtools_http_base(browser_ws)?);
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build Chrome HTTP client")?;
    let value: Value = client
        .get(endpoint)
        .send()
        .context("list Chrome targets")?
        .error_for_status()
        .context("Chrome target listing failed")?
        .json()
        .context("parse Chrome target listing")?;
    let Some(targets) = value.as_array() else {
        bail!("Chrome target listing response was not an array");
    };
    let mut discovered = Vec::new();
    for target in targets {
        if target.get("type").and_then(Value::as_str) != Some("page") {
            continue;
        }
        let Some(target_id) = target
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(page_ws) = target
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let url = target
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let title = target
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let native_cef_session_id = native_cef_session_id_from_url(&url);
        discovered.push(DiscoveredTarget {
            target_id: target_id.to_string(),
            page_ws: page_ws.to_string(),
            url,
            title,
            native_cef_session_id,
        });
    }
    Ok(discovered)
}

fn is_reusable_page_target(value: &Value) -> bool {
    if value.get("type").and_then(Value::as_str) != Some("page") {
        return false;
    }
    let Some(url) = value.get("url").and_then(Value::as_str) else {
        return false;
    };
    url == "about:blank" || native_cef_session_id_from_url(url).is_some()
}

fn is_reusable_target_info(value: &Value) -> bool {
    if value.get("type").and_then(Value::as_str) != Some("page") {
        return false;
    }
    let Some(url) = value.get("url").and_then(Value::as_str) else {
        return false;
    };
    url == "about:blank" || native_cef_session_id_from_url(url).is_some()
}

fn native_cef_session_id_from_url(value: &str) -> Option<String> {
    let fragment = value.strip_prefix("about:blank#")?;
    let native_id = fragment.strip_prefix(NATIVE_CEF_SLOT_FRAGMENT_PREFIX)?;
    (!native_id.is_empty()).then(|| native_id.to_string())
}

fn page_ws_for_target(browser_ws: &str, target_id: &str) -> Result<String> {
    let mut parsed = Url::parse(browser_ws).context("parse Chrome DevTools URL")?;
    parsed.set_path(&format!("/devtools/page/{target_id}"));
    parsed.set_query(None);
    parsed.set_fragment(None);
    Ok(parsed.to_string())
}

fn reusable_page_target_reset_url(target: &ChromePageTarget) -> String {
    target
        .native_cef_session_id
        .as_deref()
        .map(|id| format!("{DEFAULT_URL}#{NATIVE_CEF_SLOT_FRAGMENT_PREFIX}{id}"))
        .unwrap_or_else(|| DEFAULT_URL.to_string())
}

fn wait_for_cdp_response(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    response_id: u64,
    action: &str,
) -> Result<Value> {
    let start = Instant::now();
    while start.elapsed() < TARGET_RESET_TIMEOUT {
        match socket.read() {
            Ok(Message::Text(text)) => {
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if value.get("id").and_then(Value::as_u64) != Some(response_id) {
                    continue;
                }
                if let Some(error) = value.get("error") {
                    bail!("{action} failed: {error}");
                }
                return Ok(value);
            }
            Ok(Message::Close(_)) => bail!("{action} failed: target socket closed"),
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).with_context(|| action.to_string()),
        }
    }
    bail!("timed out while {action}");
}

pub(super) fn devtools_http_base(browser_ws: &str) -> Result<String> {
    let mut parsed = Url::parse(browser_ws).context("parse Chrome DevTools URL")?;
    let scheme = match parsed.scheme() {
        "ws" => "http",
        "wss" => "https",
        scheme => bail!("unsupported Chrome DevTools URL scheme {scheme}"),
    };
    parsed
        .set_scheme(scheme)
        .map_err(|_| anyhow!("set DevTools HTTP URL scheme"))?;
    parsed.set_path("");
    parsed.set_query(None);
    parsed.set_fragment(None);
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[test]
    fn parse_reusable_page_target_keeps_remote_owner() {
        let target = parse_page_target(
            &json!({
                "id": "page-1",
                "webSocketDebuggerUrl": "ws://127.0.0.1:9333/devtools/page/page-1"
            }),
            false,
        )
        .unwrap();

        assert_eq!(target.target_id, "page-1");
        assert!(!target.close_on_release);
        assert!(target.native_cef_session_id.is_none());
    }

    #[test]
    fn parse_created_page_target_marks_close_on_release() {
        let target = parse_page_target(
            &json!({
                "id": "page-2",
                "webSocketDebuggerUrl": "ws://127.0.0.1:9333/devtools/page/page-2"
            }),
            true,
        )
        .unwrap();

        assert_eq!(target.target_id, "page-2");
        assert!(target.close_on_release);
        assert!(target.native_cef_session_id.is_none());
    }

    #[test]
    fn reusable_page_targets_ignore_nonblank_visible_pages() {
        assert!(is_reusable_page_target(&json!({
            "id": "page-1",
            "type": "page",
            "url": "about:blank",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9333/devtools/page/page-1"
        })));
        assert!(is_reusable_page_target(&json!({
            "id": "page-2",
            "type": "page",
            "url": "about:blank#puffer-cef-slot=__cef_prewarm_0__",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9333/devtools/page/page-2"
        })));
        assert!(!is_reusable_page_target(&json!({
            "id": "page-3",
            "type": "page",
            "url": "http://127.0.0.1:1420/native-start",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9333/devtools/page/page-3"
        })));
    }

    #[test]
    fn target_info_builds_page_websocket_and_native_slot() {
        let target = parse_target_info(
            "ws://127.0.0.1:9333/devtools/browser/browser-1",
            &json!({
                "targetId": "page-1",
                "type": "page",
                "url": "about:blank#puffer-cef-slot=__cef_prewarm_1__"
            }),
        )
        .unwrap();

        assert_eq!(target.target_id, "page-1");
        assert_eq!(target.page_ws, "ws://127.0.0.1:9333/devtools/page/page-1");
        assert!(!target.close_on_release);
        assert_eq!(
            target.native_cef_session_id.as_deref(),
            Some("__cef_prewarm_1__")
        );
    }

    #[test]
    fn target_info_preserves_ipv6_browser_websocket_origin() {
        let target = parse_target_info(
            "ws://[::1]:9333/devtools/browser/browser-1",
            &json!({
                "targetId": "page-1",
                "type": "page",
                "url": "about:blank#puffer-cef-slot=__cef_prewarm_1__"
            }),
        )
        .unwrap();

        assert_eq!(target.page_ws, "ws://[::1]:9333/devtools/page/page-1");
    }

    #[test]
    fn remote_devtools_probe_prefers_ipv4_loopback() {
        assert_eq!(
            remote_devtools_version_endpoints(9333),
            vec![
                "http://127.0.0.1:9333/json/version".to_string(),
                "http://[::1]:9333/json/version".to_string()
            ]
        );
    }

    #[test]
    fn devtools_http_base_preserves_ipv6_brackets() {
        assert_eq!(
            devtools_http_base("ws://[::1]:9333/devtools/browser/root").unwrap(),
            "http://[::1]:9333"
        );
    }

    #[test]
    fn reusable_target_info_ignores_visible_pages() {
        assert!(is_reusable_target_info(&json!({
            "targetId": "page-1",
            "type": "page",
            "url": "about:blank"
        })));
        assert!(is_reusable_target_info(&json!({
            "targetId": "page-2",
            "type": "page",
            "url": "about:blank#puffer-cef-slot=__cef_prewarm_2__"
        })));
        assert!(!is_reusable_target_info(&json!({
            "targetId": "page-3",
            "type": "page",
            "url": "https://example.com"
        })));
    }

    #[test]
    fn native_cef_session_id_is_read_from_prewarm_url() {
        let target = parse_page_target(
            &json!({
                "id": "page-1",
                "url": "about:blank#puffer-cef-slot=__cef_prewarm_2__",
                "webSocketDebuggerUrl": "ws://127.0.0.1:9333/devtools/page/page-1"
            }),
            false,
        )
        .unwrap();

        assert_eq!(
            target.native_cef_session_id.as_deref(),
            Some("__cef_prewarm_2__")
        );
    }

    #[test]
    fn reset_reusable_page_target_restores_native_slot_and_history() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let page_ws = format!(
            "ws://{}/devtools/page/target-1",
            listener.local_addr().unwrap()
        );
        let messages = Arc::new(Mutex::new(Vec::<Value>::new()));
        let server_messages = Arc::clone(&messages);
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            loop {
                let Message::Text(text) = socket.read().unwrap() else {
                    continue;
                };
                let value: Value = serde_json::from_str(&text).unwrap();
                let id = value.get("id").and_then(Value::as_u64).unwrap();
                let done = value.get("method").and_then(Value::as_str)
                    == Some("Page.resetNavigationHistory");
                server_messages.lock().unwrap().push(value);
                socket
                    .send(Message::Text(
                        json!({ "id": id, "result": {} }).to_string().into(),
                    ))
                    .unwrap();
                if done {
                    break;
                }
            }
        });
        let target = ChromePageTarget {
            target_id: "target-1".to_string(),
            page_ws,
            close_on_release: false,
            native_cef_session_id: Some("__cef_prewarm_2__".to_string()),
            adopted: false,
        };

        reset_reusable_page_target(&target).unwrap();
        server.join().unwrap();

        let messages = messages.lock().unwrap();
        let methods = messages
            .iter()
            .map(|message| message.get("method").and_then(Value::as_str).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            methods,
            vec![
                "Page.stopLoading",
                "Page.navigate",
                "Page.resetNavigationHistory"
            ]
        );
        assert_eq!(
            messages[1].pointer("/params/url").and_then(Value::as_str),
            Some("about:blank#puffer-cef-slot=__cef_prewarm_2__")
        );
    }
}
