//! Kimi-CLI-compatible OAuth 2.0 device authorization flow (RFC 8628).
//!
//! Mirrors the behavior of Moonshot AI's official `kimi-cli` client so we can
//! authenticate puffer against `api.kimi.com/coding/v1` using a browser-based
//! login instead of a rate-limited `sk-kimi-*` API key. The flow:
//!
//! 1. POST `/api/oauth/device_authorization` — receive `device_code`,
//!    `user_code`, verification URL, and polling `interval`.
//! 2. Display `user_code` + URL to the user, open a browser.
//! 3. Poll POST `/api/oauth/token` every `interval` seconds until the server
//!    returns an access token (or the device code expires).
//! 4. Persist `{access_token, refresh_token, expires_at}` locally.
//!
//! Refresh and chat calls require the same set of `X-Msh-*` device headers
//! plus `User-Agent: KimiCLI/<version>`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

/// Public OAuth client id baked into kimi-cli. No client secret (device flow).
pub const KIMI_OAUTH_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";

/// Default auth host. Override via `KIMI_OAUTH_HOST` env var.
pub const KIMI_OAUTH_HOST: &str = "https://auth.kimi.com";

/// Default user-agent string. Kimi's whitelist requires `KimiCLI/*`.
/// Kept in sync with the upstream kimi-cli version we masquerade as.
pub const KIMI_CLI_VERSION: &str = "1.37.0";
pub const KIMI_USER_AGENT: &str = "KimiCLI/1.37.0";

/// Platform label used by kimi-cli on every auth + API call.
pub const KIMI_PLATFORM: &str = "kimi_cli";

/// Device authorization response from `/api/oauth/device_authorization`.
#[derive(Debug, Clone, Deserialize)]
pub struct KimiDeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    #[serde(default)]
    pub verification_uri: String,
    pub verification_uri_complete: String,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

fn default_interval() -> u64 {
    5
}

/// Token payload returned on successful polling / refresh.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KimiOAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix milliseconds when the access token expires.
    pub expires_at_ms: u64,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub token_type: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// Returns the resolved auth host, honoring `KIMI_OAUTH_HOST` override.
pub fn kimi_oauth_host() -> String {
    std::env::var("KIMI_OAUTH_HOST").unwrap_or_else(|_| KIMI_OAUTH_HOST.to_string())
}

/// Loads (or creates) the per-machine device id that kimi-cli sends as
/// `X-Msh-Device-Id`. Stored at `$PUFFER_HOME/kimi-device-id` (default
/// `~/.puffer/kimi-device-id`), chmod 0600.
pub fn kimi_device_id() -> Result<String> {
    kimi_device_id_in(&puffer_home_dir()?)
}

/// Testable variant: loads (or creates) the device id under a caller-supplied
/// directory so tests can avoid racing on `PUFFER_HOME`.
pub fn kimi_device_id_in(home: &std::path::Path) -> Result<String> {
    std::fs::create_dir_all(home)
        .with_context(|| format!("failed to create puffer home at {}", home.display()))?;
    let path = home.join("kimi-device-id");
    if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(&path, &id)
        .with_context(|| format!("failed to write {}", path.display()))?;
    apply_private_file_permissions(&path);
    Ok(id)
}

fn puffer_home_dir() -> Result<std::path::PathBuf> {
    if let Ok(explicit) = std::env::var("PUFFER_HOME") {
        return Ok(std::path::PathBuf::from(explicit));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| anyhow!("HOME is not set; cannot resolve puffer home directory"))?;
    Ok(std::path::PathBuf::from(home).join(".puffer"))
}

#[cfg(unix)]
fn apply_private_file_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn apply_private_file_permissions(_path: &std::path::Path) {}

/// The `X-Msh-*` + UA header set that kimi-cli attaches to auth AND API calls.
///
/// Puffer sends these on every request to `api.kimi.com/coding/v1` when the
/// provider is `kimi-for-coding`, so Kimi's backend can identify the CLI
/// session for subscription quota accounting.
pub fn kimi_common_headers() -> Result<BTreeMap<String, String>> {
    let mut headers = BTreeMap::new();
    headers.insert("User-Agent".to_string(), KIMI_USER_AGENT.to_string());
    headers.insert("X-Msh-Platform".to_string(), KIMI_PLATFORM.to_string());
    headers.insert("X-Msh-Version".to_string(), KIMI_CLI_VERSION.to_string());
    headers.insert("X-Msh-Device-Name".to_string(), ascii_header(device_name()));
    headers.insert("X-Msh-Device-Model".to_string(), ascii_header(device_model()));
    headers.insert("X-Msh-Os-Version".to_string(), ascii_header(os_version()));
    headers.insert("X-Msh-Device-Id".to_string(), kimi_device_id()?);
    Ok(headers)
}

fn device_name() -> String {
    // kimi-cli: platform.node() or socket.gethostname(), then ASCII-sanitized.
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| hostname::get().ok().and_then(|s| s.into_string().ok()))
        .unwrap_or_else(|| "puffer".to_string())
}

/// Builds `X-Msh-Device-Model` so it matches the exact string shape kimi-cli
/// emits: `macOS {sw_vers -productVersion} {arch}` on Darwin, `Linux
/// {uname -r} {arch}` on Linux, `{system} {release} {arch}` elsewhere.
/// Uses `uname -m` for arch to match Python's `platform.machine()` (which
/// returns `arm64` on Apple Silicon, not the LLVM triple `aarch64`).
fn device_model() -> String {
    let arch = uname_arch();
    #[cfg(target_os = "macos")]
    {
        if let Some(version) = shell_output(&["sw_vers", "-productVersion"]) {
            return format!("macOS {version} {arch}");
        }
        return format!("macOS {arch}");
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(release) = shell_output(&["uname", "-r"]) {
            return format!("Linux {release} {arch}");
        }
        return format!("Linux {arch}");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let info = os_info::get();
        let os = info.os_type();
        let version = info.version().to_string();
        if version.is_empty() || version == "Unknown" {
            format!("{os} {arch}")
        } else {
            format!("{os} {version} {arch}")
        }
    }
}

fn uname_arch() -> String {
    shell_output(&["uname", "-m"]).unwrap_or_else(|| std::env::consts::ARCH.to_string())
}

/// Returns `uname -v` output to mirror Python's `platform.version()`.
fn os_version() -> String {
    shell_output(&["uname", "-v"]).unwrap_or_else(|| os_info::get().version().to_string())
}

fn shell_output(argv: &[&str]) -> Option<String> {
    let (cmd, rest) = argv.split_first()?;
    let out = std::process::Command::new(cmd).args(rest).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Coerces a value to an ASCII-safe HTTP header string (kimi-cli uses
/// `_ascii_header_value` for the same reason: non-ASCII hostnames / OS
/// strings would otherwise crash aiohttp's header validator).
fn ascii_header(value: String) -> String {
    if value.is_ascii() {
        return value;
    }
    let cleaned: String = value.chars().filter(|c| c.is_ascii()).collect();
    if cleaned.trim().is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

fn build_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build blocking HTTP client")
}

/// POST /api/oauth/device_authorization.
pub fn request_device_authorization() -> Result<KimiDeviceAuthorization> {
    let client = build_client()?;
    let url = format!("{}/api/oauth/device_authorization", kimi_oauth_host());
    let headers = kimi_common_headers()?;
    let mut builder = client.post(&url);
    for (name, value) in &headers {
        builder = builder.header(name, value);
    }
    let response = builder
        .form(&[("client_id", KIMI_OAUTH_CLIENT_ID)])
        .send()
        .context("device_authorization POST failed")?;
    let status = response.status();
    let body = response
        .text()
        .context("failed to read device_authorization body")?;
    if !status.is_success() {
        return Err(anyhow!(
            "device_authorization returned HTTP {status}: {body}"
        ));
    }
    serde_json::from_str::<KimiDeviceAuthorization>(&body)
        .with_context(|| format!("failed to parse device_authorization response: {body}"))
}

/// One poll attempt against /api/oauth/token for the device code flow.
///
/// Returns `Ok(Some(creds))` when the user has authorized, `Ok(None)` when we
/// should keep polling (`authorization_pending` / `slow_down`), or `Err` on a
/// terminal error (`expired_token`, `access_denied`, transport failure).
pub fn poll_for_device_token_once(
    device_code: &str,
) -> Result<Option<KimiOAuthCredentials>> {
    let client = build_client()?;
    let url = format!("{}/api/oauth/token", kimi_oauth_host());
    let headers = kimi_common_headers()?;
    let mut builder = client.post(&url);
    for (name, value) in &headers {
        builder = builder.header(name, value);
    }
    let response = builder
        .form(&[
            ("client_id", KIMI_OAUTH_CLIENT_ID),
            ("device_code", device_code),
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:device_code",
            ),
        ])
        .send()
        .context("token polling POST failed")?;
    let status = response.status();
    let body = response.text().context("failed to read token response")?;
    let parsed: TokenResponse = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse token response: {body}"))?;

    if status.is_success() {
        let access_token = parsed
            .access_token
            .ok_or_else(|| anyhow!("token response missing access_token"))?;
        let refresh_token = parsed
            .refresh_token
            .ok_or_else(|| anyhow!("token response missing refresh_token"))?;
        let expires_in = parsed.expires_in.unwrap_or(3600);
        Ok(Some(KimiOAuthCredentials {
            access_token,
            refresh_token,
            expires_at_ms: now_ms() + expires_in.saturating_mul(1_000),
            scope: parsed.scope.unwrap_or_default(),
            token_type: parsed.token_type.unwrap_or_else(|| "Bearer".to_string()),
        }))
    } else {
        let error_code = parsed.error.unwrap_or_else(|| status.to_string());
        match error_code.as_str() {
            "authorization_pending" | "slow_down" => Ok(None),
            "expired_token" | "access_denied" => Err(anyhow!(
                "device code terminal error `{error_code}`: {}",
                parsed.error_description.unwrap_or_default()
            )),
            _ => Err(anyhow!(
                "token polling returned HTTP {status} error `{error_code}`: {}",
                parsed.error_description.unwrap_or(body)
            )),
        }
    }
}

/// Polls for a device token until the user completes authorization or the
/// overall deadline elapses. Honors the server-provided `interval` with an
/// `slow_down` bump.
pub fn poll_for_device_token(
    auth: &KimiDeviceAuthorization,
    max_wait_secs: u64,
    on_wait: impl Fn(&str),
) -> Result<KimiOAuthCredentials> {
    let deadline = Instant::now() + Duration::from_secs(max_wait_secs);
    let mut interval_secs = auth.interval.max(1);
    loop {
        match poll_for_device_token_once(&auth.device_code) {
            Ok(Some(creds)) => return Ok(creds),
            Ok(None) => {}
            Err(err) => {
                let msg = err.to_string();
                if msg.contains("slow_down") {
                    interval_secs = interval_secs.saturating_add(5);
                    on_wait(&format!("slow_down → raising interval to {interval_secs}s"));
                } else {
                    return Err(err);
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(anyhow!("timed out waiting for Kimi device authorization"));
        }
        on_wait("waiting for authorization in browser...");
        std::thread::sleep(Duration::from_secs(interval_secs));
    }
}

/// Cross-process-safe wrapper over `refresh_kimi_token`.
///
/// Kimi's OAuth server rotates the `refresh_token` on every refresh — the old
/// value is invalidated the moment a new one is issued. When 20 benchmark
/// workers share the same credential file and all hit 401 at the same time,
/// the first refresh wins and the rest get `invalid_grant`, killing their
/// tasks. `refresh_kimi_token_locked` serialises refresh across puffer
/// processes using an advisory file lock and re-reads the caller's view of
/// the stored token after acquiring it, so late arrivers pick up whatever a
/// peer just persisted instead of racing the server.
///
/// `on_reload(existing_refresh_token)` is invoked inside the lock **after**
/// acquiring it; if it returns `Some(creds)` whose expiry is still safely in
/// the future, we return those without calling the server. The caller is
/// expected to re-read the auth store from disk here.
pub fn refresh_kimi_token_locked<F>(
    lock_path: &std::path::Path,
    current_refresh_token: &str,
    on_reload: F,
) -> Result<KimiOAuthCredentials>
where
    F: FnOnce() -> Option<KimiOAuthCredentials>,
{
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create directory for kimi oauth lock {}", parent.display())
        })?;
    }
    let mut lock = fslock::LockFile::open(lock_path).with_context(|| {
        format!("failed to open kimi oauth lock {}", lock_path.display())
    })?;
    lock.lock()
        .with_context(|| format!("failed to acquire kimi oauth lock {}", lock_path.display()))?;
    // Re-check inside the lock: another peer may have refreshed while we
    // waited.  If the token on disk is no longer the one we started with,
    // adopt it instead of spending a refresh.
    if let Some(latest) = on_reload() {
        if latest.refresh_token != current_refresh_token
            && latest.expires_at_ms > now_ms() + 60_000
        {
            return Ok(latest);
        }
    }
    let refreshed = refresh_kimi_token(current_refresh_token)?;
    Ok(refreshed)
}

/// Default location for the cross-process lock file puffer uses while a
/// Kimi OAuth refresh is in flight.
pub fn default_kimi_lock_path() -> Result<std::path::PathBuf> {
    Ok(puffer_home_dir()?.join("kimi-oauth.lock"))
}

/// POST /api/oauth/token with grant_type=refresh_token.
pub fn refresh_kimi_token(refresh_token: &str) -> Result<KimiOAuthCredentials> {
    let client = build_client()?;
    let url = format!("{}/api/oauth/token", kimi_oauth_host());
    let headers = kimi_common_headers()?;
    let mut builder = client.post(&url);
    for (name, value) in &headers {
        builder = builder.header(name, value);
    }
    let response = builder
        .form(&[
            ("client_id", KIMI_OAUTH_CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .context("refresh token POST failed")?;
    let status = response.status();
    let body = response.text().context("failed to read refresh response")?;
    if !status.is_success() {
        return Err(anyhow!("refresh_token returned HTTP {status}: {body}"));
    }
    let parsed: TokenResponse = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse refresh response: {body}"))?;
    let access_token = parsed
        .access_token
        .ok_or_else(|| anyhow!("refresh response missing access_token"))?;
    let refresh_token = parsed
        .refresh_token
        .ok_or_else(|| anyhow!("refresh response missing refresh_token"))?;
    let expires_in = parsed.expires_in.unwrap_or(3600);
    Ok(KimiOAuthCredentials {
        access_token,
        refresh_token,
        expires_at_ms: now_ms() + expires_in.saturating_mul(1_000),
        scope: parsed.scope.unwrap_or_default(),
        token_type: parsed.token_type.unwrap_or_else(|| "Bearer".to_string()),
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_interval_is_5() {
        assert_eq!(default_interval(), 5);
    }

    #[test]
    fn ua_and_version_stay_aligned() {
        assert!(KIMI_USER_AGENT.ends_with(KIMI_CLI_VERSION));
        assert_eq!(KIMI_USER_AGENT, format!("KimiCLI/{KIMI_CLI_VERSION}"));
    }

    #[test]
    fn device_model_is_ascii() {
        let m = device_model();
        assert!(m.is_ascii(), "device_model must be ASCII, got {m:?}");
        assert!(!m.is_empty());
    }

    #[test]
    fn ascii_header_strips_nonascii() {
        assert_eq!(ascii_header("héllo".to_string()), "hllo");
        assert_eq!(ascii_header("".to_string()), "");
        assert_eq!(ascii_header("名字".to_string()), "unknown");
    }

    #[test]
    fn device_id_persists_across_calls() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let id1 = kimi_device_id_in(tmp.path()).expect("id");
        let id2 = kimi_device_id_in(tmp.path()).expect("id");
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 32);
    }

    #[test]
    fn device_id_is_fresh_per_directory() {
        let a = tempfile::tempdir().expect("tempdir");
        let b = tempfile::tempdir().expect("tempdir");
        let id_a = kimi_device_id_in(a.path()).expect("id");
        let id_b = kimi_device_id_in(b.path()).expect("id");
        assert_ne!(id_a, id_b);
    }
}
