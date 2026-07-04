//! GitHub Copilot runtime auth.
//!
//! Copilot's chat endpoint (`api.githubcopilot.com`) speaks OpenAI Chat
//! Completions, but the bearer it accepts is NOT the stored GitHub OAuth token.
//! The GitHub token (`gho_`/`ghu_`, obtained via the device-flow login) must be
//! exchanged for a short-lived Copilot token at `copilot_internal/v2/token`.
//! We cache the exchanged token per GitHub token until shortly before it
//! expires so we don't hit the exchange endpoint on every request.

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
/// Refresh this many seconds before the server-reported expiry.
const EXPIRY_SKEW_SECS: u64 = 120;
/// Fallback lifetime when the response omits `expires_at` (Copilot tokens are
/// typically ~30 min).
const FALLBACK_LIFETIME_SECS: u64 = 25 * 60;
/// How long to reuse a bearer whose auto-mode session mint failed before
/// re-exchanging. Bounds retries to ~this interval during a `/models/session`
/// outage instead of re-running the token exchange on every chat turn (which
/// risks a GitHub secondary-rate-limit lockout). Must exceed `EXPIRY_SKEW_SECS`
/// for the cache entry to be reused at all.
const SESSION_MINT_RETRY_SECS: u64 = EXPIRY_SKEW_SECS + 30;

/// Default Copilot API host when the token response omits `endpoints.api`.
const DEFAULT_COPILOT_API: &str = "https://api.githubcopilot.com";

/// A short-lived Copilot bearer plus the account-specific API base URL. The
/// endpoint varies by SKU (individual / business / enterprise), so we always
/// use the `endpoints.api` the exchange returns rather than a hardcoded host.
#[derive(Clone)]
pub(crate) struct CopilotAuth {
    pub token: String,
    pub api_url: String,
    /// Auto-mode session for plans without manual model selection (Free /
    /// Student since GitHub's 2026-06-24 policy change). Chat requests on
    /// those plans must carry `Copilot-Session-Token`; direct model selection
    /// is server-rejected with `model_not_supported`. `None` on paid plans
    /// (their models work directly) or when minting the session failed.
    pub session: Option<CopilotSession>,
}

/// A minted `/models/session` auto-mode session token that chat requests attach
/// as `Copilot-Session-Token`. Only the token is kept here — the plan's servable
/// `available_models` list is obtained by a SEPARATE, independent
/// `/models/session` mint in the discovery path
/// (`discovery.rs::copilot_session_available_models`); the two mints are not
/// shared.
#[derive(Clone)]
pub(crate) struct CopilotSession {
    pub token: String,
}

/// True for SKUs that only get auto model selection (no manual picker):
/// GitHub's free/student/no-auth tiers. Queried from the token envelope —
/// `sku` and `limited_user_quotas` — rather than hardcoding model lists, so
/// plan reshuffles surface as data changes, not code changes.
fn copilot_sku_is_auto_only(envelope: &serde_json::Value) -> bool {
    let sku = envelope.get("sku").and_then(|s| s.as_str()).unwrap_or("");
    sku.starts_with("free_")
        || sku == "no_auth_limited_copilot"
        || envelope
            .get("limited_user_quotas")
            .map(|q| !q.is_null())
            .unwrap_or(false)
}

struct CachedToken {
    auth: CopilotAuth,
    expires_at_secs: u64,
}

fn cache() -> &'static Mutex<HashMap<String, CachedToken>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedToken>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Drops the cached exchanged bearer for a GitHub token. Called when the chat
/// endpoint rejects the bearer with 401 (e.g. it was invalidated before its
/// cached expiry — revoked Copilot seat, server-side rotation), and on
/// logout/reconnect, so the dead/logged-out bearer isn't reused or left
/// resident.
///
/// Best-effort: a chat turn already past the cache read but not yet re-inserting
/// can re-populate this key just after eviction, so a logged-out token's bearer
/// may linger until its natural ~25min expiry. Bounding that fully would need a
/// revocation set with its own GC; not worth it for a short-lived bearer.
pub(crate) fn invalidate_bearer(github_token: &str) {
    cache().lock().unwrap().remove(github_token);
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Exchanges a stored GitHub token for a short-lived Copilot bearer token plus
/// the account-specific API endpoint, caching until shortly before it expires.
///
/// Not singleflight: concurrent callers on a cold/expired cache each run their
/// own exchange (+ session mint) before inserting. In practice turns within a
/// session are sequential and the cache holds for ~25min, so the stampede is a
/// small burst at first use / expiry; a shared in-flight map would add locking
/// complexity for little gain, so we accept it.
pub(crate) fn copilot_bearer_token(github_token: &str) -> Result<CopilotAuth> {
    let now = now_secs();
    if let Some(cached) = cache().lock().unwrap().get(github_token) {
        if cached.expires_at_secs > now + EXPIRY_SKEW_SECS {
            return Ok(cached.auth.clone());
        }
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building Copilot token-exchange client")?;
    let response = client
        .get(COPILOT_TOKEN_URL)
        .header("Authorization", format!("token {github_token}"))
        .header("Accept", "application/json")
        .header("User-Agent", "GitHubCopilotChat/0.31.0")
        .header("Editor-Version", "vscode/1.104.0")
        .header("Editor-Plugin-Version", "copilot-chat/0.31.0")
        .header("Copilot-Integration-Id", "vscode-chat")
        .header("X-GitHub-Api-Version", "2025-10-01")
        .send()
        .context("requesting Copilot token")?;

    let status = response.status();
    let body = response.text().unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub Copilot token exchange failed ({status}): {body}");
    }

    let value: serde_json::Value =
        serde_json::from_str(&body).context("parsing Copilot token response")?;
    let token = value
        .get("token")
        .and_then(|t| t.as_str())
        .context("Copilot token response missing `token`")?
        .to_string();
    let expires_at = value
        .get("expires_at")
        .and_then(|e| e.as_u64())
        .unwrap_or(now + FALLBACK_LIFETIME_SECS);
    // The account-specific API host (individual / business / enterprise). This
    // value comes from the (TLS, authenticated) token-exchange response; still,
    // only trust an https URL before we send the Copilot bearer + session token
    // to it — anything else falls back to the default host.
    let api_url = value
        .get("endpoints")
        .and_then(|endpoints| endpoints.get("api"))
        .and_then(|api| api.as_str())
        .filter(|api| api.starts_with("https://"))
        .unwrap_or(DEFAULT_COPILOT_API)
        .trim_end_matches('/')
        .to_string();

    // Auto-only plans need a `/models/session` auto-mode session for chat to
    // work at all. Minting is unbilled and the session outlives the bearer's
    // cache window (~1h vs ~25min), so mint alongside each exchange.
    let auto_only = copilot_sku_is_auto_only(&value);
    let session = if auto_only {
        mint_auto_session(&client, &api_url, &token)
    } else {
        None
    };

    let auth = CopilotAuth {
        token,
        api_url,
        session,
    };
    // Cache the exchange until shortly before the bearer expires. When an
    // auto-only plan's session mint failed transiently (flaky network /
    // `/models/session` outage), cache the session-less bearer only briefly
    // (SESSION_MINT_RETRY_SECS) instead of the full window: long enough to stop
    // re-running the token exchange on every chat turn (which risks a GitHub
    // secondary-rate-limit lockout), short enough to re-mint the session soon
    // after the outage clears. (Paid plans legitimately have no session — cache
    // those for the full window.)
    let cache_expires_at = if auto_only && auth.session.is_none() {
        (now + SESSION_MINT_RETRY_SECS).min(expires_at)
    } else {
        expires_at
    };
    cache().lock().unwrap().insert(
        github_token.to_string(),
        CachedToken {
            auth: auth.clone(),
            expires_at_secs: cache_expires_at,
        },
    );
    Ok(auth)
}

/// Mints an auto-mode session (`POST /models/session`) and returns its JWT.
/// Best-effort: any failure returns `None` and chat proceeds without it.
fn mint_auto_session(client: &Client, api_url: &str, bearer: &str) -> Option<CopilotSession> {
    let response = client
        .post(format!("{api_url}/models/session"))
        .header("Authorization", format!("Bearer {bearer}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", "GitHubCopilotChat/0.31.0")
        .header("Editor-Version", "vscode/1.104.0")
        .header("Editor-Plugin-Version", "copilot-chat/0.31.0")
        .header("Copilot-Integration-Id", "vscode-chat")
        .header("X-GitHub-Api-Version", "2025-10-01")
        .json(&serde_json::json!({ "auto_mode": { "model_hints": ["auto"] } }))
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: serde_json::Value = response.json().ok()?;
    let token = value.get("session_token")?.as_str()?.to_string();
    Some(CopilotSession { token })
}
