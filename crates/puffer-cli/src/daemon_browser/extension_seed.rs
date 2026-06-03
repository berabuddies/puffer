//! CDP helpers for seeding bundled extension storage.

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

use super::chrome::devtools_http_base;
use super::launch_settings::BrowserExtensionSeed;

const EXTENSION_TARGET_WAIT: Duration = Duration::from_secs(2);
const EXTENSION_TARGET_POLL: Duration = Duration::from_millis(100);
const EXTENSION_REGISTRATION_WAIT: Duration = Duration::from_secs(3);
const EXTENSION_SEED_TIMEOUT: Duration = Duration::from_secs(2);

struct ExtensionTarget {
    websocket_url: String,
}

/// Verifies requested unpacked extensions were registered by the browser runtime.
pub(super) fn ensure_extensions_registered(
    browser_ws: &str,
    profile_dir: Option<&Path>,
    extension_dirs: &[PathBuf],
) -> Result<()> {
    if extension_dirs.is_empty() {
        return Ok(());
    }
    let targets = wait_for_extension_targets(browser_ws)?;
    if !targets.is_empty() {
        return Ok(());
    }
    if let Some(profile_dir) = profile_dir {
        if wait_for_extension_preferences(profile_dir, extension_dirs)? {
            return Ok(());
        }
    }
    bail!(
        "browser extensions were requested but no extension runtime was registered; \
         use a Puffer CT Chromium/CEF build that honors unpacked extension loading"
    )
}

/// Seeds local storage for bundled CAPTCHA extensions that are loaded in Chrome.
pub(super) fn seed_extensions(browser_ws: &str, seeds: &[BrowserExtensionSeed]) -> Result<()> {
    if seeds.is_empty() {
        return Ok(());
    }
    let targets = wait_for_extension_targets(browser_ws)?;
    for seed in seeds {
        let mut matched = false;
        for target in &targets {
            match seed_extension_target(target, seed) {
                Ok(true) => {
                    matched = true;
                    break;
                }
                Ok(false) => {}
                Err(error) => {
                    eprintln!(
                        "puffer browser: extension seed `{}` failed on one target: {error}",
                        seed.solver_id()
                    );
                }
            }
        }
        if !matched {
            bail!(
                "loaded extension target for `{}` was not found, so its API key could not be configured",
                seed.solver_id()
            );
        }
    }
    Ok(())
}

fn wait_for_extension_targets(browser_ws: &str) -> Result<Vec<ExtensionTarget>> {
    let start = Instant::now();
    let mut targets = Vec::new();
    while start.elapsed() < EXTENSION_TARGET_WAIT {
        targets = list_extension_targets(browser_ws)?;
        if !targets.is_empty() {
            break;
        }
        thread::sleep(EXTENSION_TARGET_POLL);
    }
    Ok(targets)
}

fn list_extension_targets(browser_ws: &str) -> Result<Vec<ExtensionTarget>> {
    let endpoint = format!("{}/json/list", devtools_http_base(browser_ws)?);
    let client = Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .context("build Chrome HTTP client")?;
    let value: Value = client
        .get(endpoint)
        .send()
        .context("list Chrome extension targets")?
        .error_for_status()
        .context("Chrome extension target listing failed")?
        .json()
        .context("parse Chrome extension target listing")?;
    let Some(targets) = value.as_array() else {
        bail!("Chrome target listing response was not an array");
    };
    Ok(targets
        .iter()
        .filter(|target| is_extension_target(target))
        .filter_map(|target| {
            target
                .get("webSocketDebuggerUrl")
                .and_then(Value::as_str)
                .map(|websocket_url| ExtensionTarget {
                    websocket_url: websocket_url.to_string(),
                })
        })
        .collect())
}

fn wait_for_extension_preferences(profile_dir: &Path, extension_dirs: &[PathBuf]) -> Result<bool> {
    let start = Instant::now();
    while start.elapsed() < EXTENSION_REGISTRATION_WAIT {
        if extension_preferences_include(profile_dir, extension_dirs)? {
            return Ok(true);
        }
        thread::sleep(EXTENSION_TARGET_POLL);
    }
    Ok(false)
}

fn extension_preferences_include(profile_dir: &Path, extension_dirs: &[PathBuf]) -> Result<bool> {
    let path = profile_dir.join("Default").join("Preferences");
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let Ok(value) = serde_json::from_str::<Value>(&contents) else {
        return Ok(false);
    };
    let Some(settings) = value
        .get("extensions")
        .and_then(|extensions| extensions.get("settings"))
        .and_then(Value::as_object)
    else {
        return Ok(false);
    };
    let registered = settings
        .values()
        .filter_map(extension_setting_path)
        .map(normalize_path_string)
        .collect::<Vec<_>>();
    let requested = extension_dirs
        .iter()
        .map(|path| normalize_path_string(path.display().to_string()))
        .collect::<Vec<_>>();
    Ok(requested
        .iter()
        .all(|path| registered.iter().any(|candidate| candidate == path)))
}

fn extension_setting_path(value: &Value) -> Option<String> {
    value
        .get("path")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn normalize_path_string(value: String) -> String {
    std::fs::canonicalize(&value)
        .map(|path| path.display().to_string())
        .unwrap_or(value)
}

fn is_extension_target(target: &Value) -> bool {
    let target_type = target.get("type").and_then(Value::as_str);
    let url = target
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    matches!(
        target_type,
        Some("service_worker") | Some("background_page") | Some("worker")
    ) && url.starts_with("chrome-extension://")
}

fn seed_extension_target(target: &ExtensionTarget, seed: &BrowserExtensionSeed) -> Result<bool> {
    let (mut socket, _) =
        connect(&target.websocket_url).context("connect to Chrome extension target")?;
    set_socket_timeout(&mut socket);
    let expression = seed_expression(seed)?;
    socket
        .send(Message::Text(
            json!({
                "id": 1,
                "method": "Runtime.evaluate",
                "params": {
                    "expression": expression,
                    "awaitPromise": true,
                    "returnByValue": true
                }
            })
            .to_string(),
        ))
        .context("send extension storage seed")?;
    loop {
        let message = socket
            .read()
            .context("read extension storage seed response")?;
        let Message::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(&text).context("parse extension seed response")?;
        if value.get("id").and_then(Value::as_u64) != Some(1) {
            continue;
        }
        if let Some(error) = value.get("error") {
            bail!("extension seed evaluation failed: {error}");
        }
        return Ok(value
            .get("result")
            .and_then(|result| result.get("result"))
            .and_then(|result| result.get("value"))
            .and_then(|result| result.get("matched"))
            .and_then(Value::as_bool)
            .unwrap_or(false));
    }
}

fn seed_expression(seed: &BrowserExtensionSeed) -> Result<String> {
    let api_key = serde_json::to_string(seed.api_key())?;
    let base_url = serde_json::to_string(seed.base_url())?;
    match seed.solver_id() {
        "nopecha" => Ok(nopecha_seed_expression(&api_key, &base_url)),
        "2captcha" => Ok(two_captcha_seed_expression(&api_key, &base_url)),
        other => bail!("unsupported captcha extension seed `{other}`"),
    }
}

fn nopecha_seed_expression(api_key: &str, base_url: &str) -> String {
    format!(
        r#"(async () => {{
  const manifest = chrome?.runtime?.getManifest?.();
  if (!manifest || manifest.name !== "NopeCHA: CAPTCHA Solver") return {{ matched: false }};
  const current = await new Promise((resolve) => chrome.storage.local.get("nopecha", resolve));
  const defaults = manifest.nopecha || {{}};
  const existing = current.nopecha || {{}};
  const next = {{ ...defaults, ...existing, enabled: true, key: {api_key}, _base_api: {base_url} }};
  await new Promise((resolve) => chrome.storage.local.set({{ nopecha: next }}, resolve));
  return {{ matched: true }};
}})()"#
    )
}

fn two_captcha_seed_expression(api_key: &str, base_url: &str) -> String {
    format!(
        r#"(async () => {{
  const manifest = chrome?.runtime?.getManifest?.();
  const homepage = manifest?.homepage_url || "";
  const name = manifest?.name || "";
  if (!homepage.includes("2captcha.com") && !name.includes("2Captcha") && !name.includes("__MSG_extName__")) return {{ matched: false }};
  const current = await new Promise((resolve) => chrome.storage.local.get("config", resolve));
  const existing = current.config || {{}};
  const next = {{ ...existing, isPluginEnabled: true, apiKey: {api_key}, apiServer: {base_url}, baseUrl: {base_url} }};
  await new Promise((resolve) => chrome.storage.local.set({{ config: next }}, resolve));
  return {{ matched: true }};
}})()"#
    )
}

fn set_socket_timeout(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) {
    if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
        let _ = stream.set_read_timeout(Some(EXTENSION_SEED_TIMEOUT));
        let _ = stream.set_write_timeout(Some(EXTENSION_SEED_TIMEOUT));
    }
}
