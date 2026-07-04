//! GitHub Copilot device-flow login.
//!
//! Copilot does not use the PKCE redirect flow that openai/anthropic use.
//! Instead we run GitHub's OAuth *device flow*: request a device+user code,
//! the user enters the user code at github.com/login/device, and we poll until
//! GitHub returns a `gho_`/`ghu_` access token. That token is stored as the
//! provider credential; the runtime later exchanges it for a short-lived
//! Copilot bearer (see `puffer-core/runtime/copilot.rs`).

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::time::Duration;

/// Public client id of the VS Code Copilot extension. Copilot gates model
/// access on the client id, so we present the same identity the editors use.
pub(crate) const COPILOT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building GitHub device-flow client")
}

/// Result of starting the device flow — shown to the user so they can authorize.
#[derive(Debug, Clone)]
pub(crate) struct DeviceFlowStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

/// Requests a device + user code from GitHub.
pub(crate) fn start_device_flow() -> Result<DeviceFlowStart> {
    #[derive(Deserialize)]
    struct Resp {
        device_code: String,
        user_code: String,
        verification_uri: String,
        #[serde(default)]
        interval: u64,
        #[serde(default)]
        expires_in: u64,
    }
    let client = http_client()?;
    let response = client
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "GitHubCopilotChat/0.23.0")
        .form(&[("client_id", COPILOT_CLIENT_ID), ("scope", "read:user")])
        .send()
        .context("requesting GitHub device code")?;
    let status = response.status();
    let body = response.text().unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub device-code request failed ({status}): {body}");
    }
    let parsed: Resp =
        serde_json::from_str(&body).context("parsing GitHub device-code response")?;
    Ok(DeviceFlowStart {
        device_code: parsed.device_code,
        user_code: parsed.user_code,
        verification_uri: parsed.verification_uri,
        interval: if parsed.interval == 0 { 5 } else { parsed.interval },
        expires_in: parsed.expires_in,
    })
}

/// Outcome of a single poll of the device-flow token endpoint.
pub(crate) enum DeviceFlowPoll {
    /// User has not authorized yet — keep polling.
    Pending,
    /// GitHub asked us to slow down — increase the interval, keep polling.
    SlowDown,
    /// Authorized — the GitHub access token.
    Done(String),
    /// Terminal failure (expired / denied / etc.).
    Failed(String),
}

/// Polls the token endpoint once with the device code.
pub(crate) fn poll_device_flow(device_code: &str) -> Result<DeviceFlowPoll> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        access_token: Option<String>,
        #[serde(default)]
        error: Option<String>,
    }
    let client = http_client()?;
    // A single poll must not abort the whole login on a transient blip. Network
    // errors and non-JSON/unknown bodies (e.g. a 5xx HTML error page from an
    // infra hiccup) are treated as Pending so the caller keeps polling until the
    // device code genuinely expires; only GitHub's documented terminal device-
    // flow errors end the flow.
    let response = match client
        .post(ACCESS_TOKEN_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "GitHubCopilotChat/0.23.0")
        .form(&[
            ("client_id", COPILOT_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
    {
        Ok(response) => response,
        Err(_) => return Ok(DeviceFlowPoll::Pending),
    };
    let body = response.text().unwrap_or_default();
    let Ok(parsed) = serde_json::from_str::<Resp>(&body) else {
        return Ok(DeviceFlowPoll::Pending);
    };
    if let Some(token) = parsed.access_token.filter(|t| !t.is_empty()) {
        return Ok(DeviceFlowPoll::Done(token));
    }
    match parsed.error.as_deref() {
        // Keep polling.
        Some("authorization_pending") | None => Ok(DeviceFlowPoll::Pending),
        Some("slow_down") => Ok(DeviceFlowPoll::SlowDown),
        // GitHub's documented terminal device-flow errors.
        Some(
            err @ ("access_denied" | "expired_token" | "unsupported_grant_type"
            | "incorrect_client_credentials" | "incorrect_device_code"
            | "device_flow_disabled"),
        ) => Ok(DeviceFlowPoll::Failed(err.to_string())),
        // Unknown error code — treat as transient rather than aborting.
        Some(_) => Ok(DeviceFlowPoll::Pending),
    }
}
