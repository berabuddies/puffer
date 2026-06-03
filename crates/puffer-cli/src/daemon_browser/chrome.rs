use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use url::Url;

use super::ct_runtime;
use super::CHROME_START_TIMEOUT;

#[derive(Clone, Debug)]
pub(super) struct ChromePageTarget {
    pub(super) target_id: String,
    pub(super) page_ws: String,
    pub(super) close_on_release: bool,
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
    Ok(targets
        .iter()
        .find(|target| target.get("type").and_then(Value::as_str) == Some("page"))
        .map(|target| parse_page_target(target, false))
        .transpose()?)
}

/// Waits for a page target to appear on an existing DevTools endpoint.
pub(super) fn wait_for_initial_page_target(
    browser_ws: &str,
    timeout: Duration,
) -> Result<Option<ChromePageTarget>> {
    let start = Instant::now();
    let mut last_error = None;
    loop {
        match initial_page_target(browser_ws) {
            Ok(Some(target)) => return Ok(Some(target)),
            Ok(None) => {}
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
    Ok(None)
}

/// Waits for an already-running DevTools HTTP endpoint to publish its browser WebSocket URL.
pub(super) fn read_remote_devtools_ws_url(port: u16, timeout: Duration) -> Result<String> {
    let endpoint = format!("http://127.0.0.1:{port}/json/version");
    let client = Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .context("build DevTools HTTP client")?;
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(response) = client.get(&endpoint).send() {
            if let Ok(response) = response.error_for_status() {
                if let Ok(value) = response.json::<Value>() {
                    if let Some(ws) = value.get("webSocketDebuggerUrl").and_then(Value::as_str) {
                        return Ok(ws.to_string());
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("DevTools endpoint at {endpoint} did not publish a browser WebSocket URL");
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
    Ok(ChromePageTarget {
        target_id: target_id.to_string(),
        page_ws: page_ws.to_string(),
        close_on_release,
    })
}

pub(super) fn devtools_http_base(browser_ws: &str) -> Result<String> {
    let parsed = Url::parse(browser_ws).context("parse Chrome DevTools URL")?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("Chrome DevTools URL missing host"))?;
    let port = parsed
        .port()
        .ok_or_else(|| anyhow!("Chrome DevTools URL missing port"))?;
    Ok(format!("http://{host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    }
}
