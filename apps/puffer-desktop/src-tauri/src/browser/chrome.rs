use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use url::Url;

use super::CHROME_START_TIMEOUT;

/// Waits for Chrome to publish its browser-level DevTools WebSocket URL.
pub(super) fn read_devtools_ws_url(child: &mut Child, profile_dir: &Path) -> Result<String> {
    let stderr = child.stderr.take().context("Chrome stderr missing")?;
    spawn_stderr_drain(BufReader::new(stderr));
    let active_port_path = profile_dir.join("DevToolsActivePort");
    let start = Instant::now();
    while start.elapsed() < CHROME_START_TIMEOUT {
        if let Some(status) = child.try_wait().context("check Chrome launch status")? {
            bail!("Chrome exited before publishing a DevTools endpoint: {status}");
        }
        if let Ok(contents) = std::fs::read_to_string(&active_port_path) {
            if let Some(url) = devtools_url_from_active_port(&contents) {
                return Ok(url);
            }
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

/// Returns the HTTP origin for a browser-level DevTools WebSocket URL.
pub(super) fn cdp_http_endpoint(browser_ws: &str) -> Result<String> {
    let parsed = Url::parse(browser_ws).context("parse Chrome DevTools URL")?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("Chrome DevTools URL missing host"))?;
    let port = parsed
        .port()
        .ok_or_else(|| anyhow!("Chrome DevTools URL missing port"))?;
    Ok(format!("http://{host}:{port}"))
}

/// Returns the first page target Chrome opened at startup.
pub(super) fn first_page_target(browser_ws: &str) -> Result<String> {
    let endpoint = format!("{}/json/list", cdp_http_endpoint(browser_ws)?);
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build Chrome HTTP client")?;
    let values: Vec<Value> = client
        .get(endpoint)
        .send()
        .context("list Chrome targets")?
        .error_for_status()
        .context("Chrome target listing failed")?
        .json()
        .context("parse Chrome target list response")?;
    values
        .into_iter()
        .find(|value| value.get("type").and_then(Value::as_str) == Some("page"))
        .and_then(|value| {
            value
                .get("webSocketDebuggerUrl")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .ok_or_else(|| anyhow!("Chrome target list missing page webSocketDebuggerUrl"))
}

/// Creates a new Chrome page target and returns its DevTools WebSocket URL.
pub(super) fn create_page_target(browser_ws: &str, url: &str) -> Result<String> {
    let endpoint = format!(
        "{}/json/new?{}",
        cdp_http_endpoint(browser_ws)?,
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
    value
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("Chrome target response missing webSocketDebuggerUrl"))
}

/// Finds the Chrome or Chromium executable Puffer should manage.
pub(super) fn resolve_chrome_executable() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PUFFER_CHROME") {
        let path = PathBuf::from(path);
        if is_executable_candidate(&path) {
            return Some(path);
        }
    }
    for candidate in chrome_candidates() {
        if is_executable_candidate(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Terminates orphaned Chrome processes that still own `profile_dir`.
pub(super) fn terminate_profile_processes(profile_dir: &Path) {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    terminate_profile_processes_unix(profile_dir);

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = profile_dir;
}

fn chrome_candidates() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut candidates = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(
                PathBuf::from(home)
                    .join("chromium_tintin/src/out/Release/Chromium.app/Contents/MacOS/Chromium"),
            );
        }
        candidates.extend([
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            PathBuf::from(
                "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
            ),
            PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        ]);
        candidates
    }
    #[cfg(target_os = "windows")]
    {
        let mut candidates = Vec::new();
        for base in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(base) {
                candidates.push(PathBuf::from(&base).join("Google/Chrome/Application/chrome.exe"));
                candidates.push(PathBuf::from(&base).join("Chromium/Application/chrome.exe"));
            }
        }
        candidates
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
        ]
        .iter()
        .filter_map(which_on_path)
        .collect()
    }
}

fn is_executable_candidate(path: &Path) -> bool {
    path.is_file()
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

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn which_on_path(name: &&str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// Converts a session id into a filesystem-safe browser profile directory name.
pub(super) fn safe_profile_name(session_id: &str) -> String {
    session_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
