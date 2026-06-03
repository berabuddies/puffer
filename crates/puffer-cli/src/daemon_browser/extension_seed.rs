//! CDP helpers for seeding bundled extension storage.

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

use super::chrome::devtools_http_base;
use super::launch_settings::BrowserExtensionSeed;

const EXTENSION_TARGET_WAIT: Duration = Duration::from_secs(2);
const EXTENSION_TARGET_POLL: Duration = Duration::from_millis(100);
const EXTENSION_SEED_TIMEOUT: Duration = Duration::from_secs(2);

struct ExtensionTarget {
    websocket_url: String,
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
            eprintln!(
                "puffer browser: loaded extension target for `{}` was not found",
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
