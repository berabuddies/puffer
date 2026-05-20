//! Auth Station login URL building, callback parsing, JWT decoding.

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngCore;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Default Auth Station base URL (Production). Sandbox is
/// `https://auth-worldrouter.vercel.app`. The env var named by
/// [`WORLDAGENT_AUTH_URL_OVERRIDE_ENV`] overrides this at runtime.
/// `http://127.0.0.1:1456` must be in the target environment's
/// `ALLOWED_REDIRECT_ORIGINS` allow-list (confirmed for Production
/// 2026-05-20).
pub const WORLDAGENT_AUTH_BASE_URL: &str = "https://auth.worldrouter.ai";

/// Env var name that overrides the Auth Station base URL.
pub const WORLDAGENT_AUTH_URL_OVERRIDE_ENV: &str = "PUFFER_WORLDAGENT_AUTH_URL";

/// Default WR control-api base URL (preview deployment). Backend
/// will move this to a stable URL — keep the env override available
/// for swapping without a rebuild.
pub const WORLDAGENT_CONTROL_BASE_URL: &str = "https://control-api-pre-7f819c.worldrouter.ai";

/// Env var name that overrides the control-api base URL.
pub const WORLDAGENT_CONTROL_URL_OVERRIDE_ENV: &str = "PUFFER_WORLDAGENT_CONTROL_URL";

/// Prefix applied to every generated `key_alias` so backend can
/// distinguish keys minted for the puffer desktop client.
pub const WORLDAGENT_KEY_ALIAS_PREFIX: &str = "puffer-";

/// Fixed loopback callback path used by Puffer desktop. The auth
/// team must allow-list the full URI on both Sandbox and Production.
pub const WORLDAGENT_CALLBACK_PATH: &str = "/callback";

/// Fixed loopback callback port used by Puffer desktop. See
/// [`WORLDAGENT_CALLBACK_PATH`] for the path component.
pub const WORLDAGENT_CALLBACK_PORT: u16 = 1456;

/// Concatenated fixed loopback redirect URI.
pub const WORLDAGENT_DEFAULT_REDIRECT_URI: &str = "http://127.0.0.1:1456/callback";

/// Parameters needed to build an Auth Station login URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldAgentLoginConfig {
    /// Base URL of Auth Station, no trailing slash.
    pub auth_base_url: String,
    /// Full redirect URI for the desktop callback listener.
    pub redirect_uri: String,
    /// Opaque random value used as the CSRF guard.
    pub client_state: String,
}

impl Default for WorldAgentLoginConfig {
    fn default() -> Self {
        let auth_base_url = std::env::var(WORLDAGENT_AUTH_URL_OVERRIDE_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| WORLDAGENT_AUTH_BASE_URL.to_string());
        Self {
            auth_base_url,
            redirect_uri: WORLDAGENT_DEFAULT_REDIRECT_URI.to_string(),
            client_state: generate_client_state(),
        }
    }
}

/// Generates an opaque url-safe random `client_state`.
pub fn generate_client_state() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Builds the Auth Station `/login` URL for the given config.
pub fn build_login_url(config: &WorldAgentLoginConfig) -> String {
    let trimmed = config.auth_base_url.trim_end_matches('/');
    let mut url = url::Url::parse(&format!("{trimmed}/login"))
        .expect("auth_base_url must be a valid URL");
    url.query_pairs_mut()
        .append_pair("redirect_uri", &config.redirect_uri)
        .append_pair("client_state", &config.client_state);
    url.to_string()
}

/// Parsed callback fields. Each field is `None` when its parameter
/// was absent from the callback URL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorldAgentCallback {
    /// `token` query parameter — the access token JWT.
    pub token: Option<String>,
    /// `refresh_token` query parameter — the refresh token JWT.
    pub refresh_token: Option<String>,
    /// `state` query parameter — original `client_state` echoed back.
    pub state: Option<String>,
    /// `error` query parameter — populated when login failed.
    pub error: Option<String>,
    /// `error_description` query parameter — populated on failure.
    pub error_description: Option<String>,
}

/// Extracts `token`, `refresh_token`, `state`, `error`, and
/// `error_description` from a callback URL or raw query string.
pub fn parse_callback_input(input: &str) -> WorldAgentCallback {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return WorldAgentCallback::default();
    }
    let mut callback = WorldAgentCallback::default();
    let pairs: Box<dyn Iterator<Item = (String, String)>> =
        if let Ok(url) = url::Url::parse(trimmed) {
            Box::new(
                url.query_pairs()
                    .into_owned()
                    .collect::<Vec<_>>()
                    .into_iter(),
            )
        } else {
            Box::new(
                url::form_urlencoded::parse(trimmed.as_bytes())
                    .into_owned()
                    .collect::<Vec<_>>()
                    .into_iter(),
            )
        };
    for (key, value) in pairs {
        match key.as_str() {
            "token" => callback.token = Some(value),
            "refresh_token" => callback.refresh_token = Some(value),
            "state" => callback.state = Some(value),
            "error" => callback.error = Some(value),
            "error_description" => callback.error_description = Some(value),
            _ => {}
        }
    }
    callback
}

/// Decoded JWT profile fields, best-effort.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorldAgentJwtProfile {
    /// JWT `sub` claim — Auth Station user id (WorkOS user id).
    pub sub: Option<String>,
    /// JWT `email` claim.
    pub email: Option<String>,
    /// JWT `name` claim — may be an empty string upstream.
    pub name: Option<String>,
    /// `default_team_id` claim — used as the path segment for the
    /// control-api key creation endpoint.
    pub default_team_id: Option<String>,
}

/// Decodes `sub` / `email` / `name` from the access token JWT
/// payload. Any decode/parse failure yields an empty profile.
pub fn decode_jwt_profile(access_token: &str) -> WorldAgentJwtProfile {
    let Some(payload_b64) = access_token.split('.').nth(1) else {
        return WorldAgentJwtProfile::default();
    };
    let Ok(payload_bytes) = URL_SAFE_NO_PAD.decode(payload_b64.as_bytes()) else {
        return WorldAgentJwtProfile::default();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload_bytes) else {
        return WorldAgentJwtProfile::default();
    };
    WorldAgentJwtProfile {
        sub: value
            .get("sub")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        email: value
            .get("email")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        default_team_id: value
            .get("default_team_id")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
    }
}

/// Persisted Auth Station credentials for the worldagent provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldAgentOAuthCredentials {
    /// Auth Station access token (24h validity per current docs).
    pub access_token: String,
    /// Auth Station refresh token (7d validity).
    pub refresh_token: String,
    /// Unix epoch milliseconds when the access token expires.
    pub expires_at_ms: u64,
    /// `sub` claim from the access token JWT.
    pub sub: Option<String>,
    /// `email` claim from the access token JWT.
    pub email: Option<String>,
    /// `name` claim from the access token JWT.
    pub name: Option<String>,
}

/// Exchanges a stored refresh token for a new access token via
/// `POST <auth>/token/refresh`. Preserves the existing
/// `refresh_token` (Auth Station does not rotate refresh tokens, and
/// `/token/refresh` returns only `{ "token": ... }`).
pub fn refresh_oauth_token(
    refresh_token: &str,
    auth_base_url: Option<&str>,
) -> Result<WorldAgentOAuthCredentials> {
    let base = auth_base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            std::env::var(WORLDAGENT_AUTH_URL_OVERRIDE_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| WORLDAGENT_AUTH_BASE_URL.to_string())
        });
    let url = format!("{}/token/refresh", base.trim_end_matches('/'));
    let response = Client::new()
        .post(&url)
        .json(&serde_json::json!({ "refresh_token": refresh_token }))
        .send()
        .context("failed to send worldagent refresh request")?;
    let status = response.status();
    let payload: RefreshResponse = response
        .json()
        .context("failed to parse worldagent refresh response")?;
    if !status.is_success() {
        return Err(anyhow!(
            "worldagent token refresh failed with status {status}: {}",
            payload.error.unwrap_or_default()
        ));
    }
    let access_token = payload
        .token
        .ok_or_else(|| anyhow!("worldagent refresh response missing token"))?;
    let profile = decode_jwt_profile(&access_token);
    Ok(WorldAgentOAuthCredentials {
        access_token,
        refresh_token: refresh_token.to_string(),
        expires_at_ms: worldagent_access_token_expires_at_ms(),
        sub: profile.sub,
        email: profile.email,
        name: profile.name,
    })
}

/// Output of a successful JWT→api_key exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangedApiKey {
    /// The WR api_key to send as `Authorization: Bearer <key>` against
    /// the inference endpoint.
    pub api_key: String,
    /// Server-assigned identifier for the key. Useful for future
    /// revoke calls.
    pub token_id: String,
    /// Client-chosen alias used to create the key.
    pub key_alias: String,
}

/// Exchanges an Auth Station JWT for a WorldRouter inference api_key.
///
/// Two-hop flow:
/// 1. `POST {control_base}/auth/exchange` body `{ "access_token": <jwt> }`
///    → infer-monorepo recognises the Auth Station issuer, verifies
///    the RS256 signature against the Auth Station JWKS, and returns
///    `{ session_token, default_team_id, ... }` where `session_token`
///    is an HS256 Infer Session JWT that the litellm gateway in front
///    of control-api accepts.
/// 2. `POST {control_base}/platform/v1/teams/{default_team_id}/keys`
///    with `Authorization: Bearer <session_token>` and a fresh
///    `key_alias = "puffer-<uuid>"` body → returns `{ key: "sk-worldrouter-..." }`.
///
/// TODO(worldagent): preview-stage shape. Open questions:
/// - idempotent get-or-create per (user, device) instead of always
///   minting a new key (avoid leaking dead keys when user re-logins
///   on the same machine)
/// - DELETE/revoke endpoint to call on logout
/// - production base URL (currently `control-api-pre-7f819c…`)
/// Re-evaluate when backend lands the stable shape.
pub fn exchange_jwt_for_api_key(access_token: &str) -> Result<ExchangedApiKey> {
    let base = std::env::var(WORLDAGENT_CONTROL_URL_OVERRIDE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| WORLDAGENT_CONTROL_BASE_URL.to_string());
    let base = base.trim_end_matches('/').to_string();
    let client = Client::new();

    // Hop 1: Auth Station JWT → Infer Session JWT
    let exchange_url = format!("{base}/auth/exchange");
    let exchange_response = client
        .post(&exchange_url)
        .json(&serde_json::json!({ "access_token": access_token }))
        .send()
        .context("failed to send worldagent /auth/exchange request")?;
    let exchange_status = exchange_response.status();
    let exchange_body = exchange_response
        .text()
        .context("failed to read worldagent /auth/exchange response body")?;
    if !exchange_status.is_success() {
        return Err(anyhow!(
            "worldagent /auth/exchange failed: POST {exchange_url} -> {exchange_status}\nresponse body: {exchange_body}"
        ));
    }
    let envelope: SessionEnvelopeResponse = serde_json::from_str(&exchange_body).with_context(
        || format!("failed to parse worldagent /auth/exchange response body: {exchange_body}"),
    )?;

    // Hop 2: Infer Session JWT → WR inference api_key
    let key_url = format!(
        "{base}/platform/v1/teams/{team_id}/keys",
        team_id = envelope.default_team_id
    );
    let key_alias = format!(
        "{}{}",
        WORLDAGENT_KEY_ALIAS_PREFIX,
        uuid::Uuid::new_v4()
            .as_hyphenated()
            .to_string()
            .to_uppercase()
    );
    let key_response = client
        .post(&key_url)
        .bearer_auth(&envelope.session_token)
        .json(&serde_json::json!({ "key_alias": key_alias }))
        .send()
        .context("failed to send worldagent key creation request")?;
    let key_status = key_response.status();
    let key_body = key_response
        .text()
        .context("failed to read worldagent key creation response body")?;
    if !key_status.is_success() {
        return Err(anyhow!(
            "worldagent key creation failed: POST {key_url} -> {key_status}\nresponse body: {key_body}"
        ));
    }
    let payload: KeyCreationResponse = serde_json::from_str(&key_body).with_context(|| {
        format!("failed to parse worldagent key creation response body: {key_body}")
    })?;
    let api_key = payload
        .key
        .ok_or_else(|| anyhow!("worldagent key creation response missing `key`"))?;
    let token_id = payload
        .token_id
        .ok_or_else(|| anyhow!("worldagent key creation response missing `token_id`"))?;
    Ok(ExchangedApiKey {
        api_key,
        token_id,
        key_alias,
    })
}

#[derive(Debug, Deserialize)]
struct SessionEnvelopeResponse {
    session_token: String,
    default_team_id: String,
}

#[derive(Debug, Deserialize)]
struct KeyCreationResponse {
    #[serde(default)]
    token_id: Option<String>,
    #[serde(default)]
    key: Option<String>,
}

/// Returns the Unix epoch milliseconds at which an Auth Station
/// access token issued **now** expires. Auth Station access tokens
/// have a 24-hour lifetime per the public API doc; this helper
/// centralizes that constant.
pub fn worldagent_access_token_expires_at_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64 + 24 * 3600 * 1000)
        .unwrap_or(24 * 3600 * 1000)
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_login_url_contains_redirect_uri_and_client_state() {
        let config = WorldAgentLoginConfig {
            auth_base_url: "https://auth-worldrouter.vercel.app".to_string(),
            redirect_uri: "http://127.0.0.1:1456/callback".to_string(),
            client_state: "state-xyz".to_string(),
        };
        let url = build_login_url(&config);
        assert!(url.starts_with("https://auth-worldrouter.vercel.app/login?"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1456%2Fcallback"));
        assert!(url.contains("client_state=state-xyz"));
    }

    #[test]
    fn parse_callback_input_extracts_token_refresh_state() {
        let parsed = parse_callback_input(
            "http://127.0.0.1:1456/callback?token=acc&refresh_token=ref&state=xyz",
        );
        assert_eq!(parsed.token.as_deref(), Some("acc"));
        assert_eq!(parsed.refresh_token.as_deref(), Some("ref"));
        assert_eq!(parsed.state.as_deref(), Some("xyz"));
        assert!(parsed.error.is_none());
    }

    #[test]
    fn parse_callback_input_extracts_error() {
        let parsed = parse_callback_input(
            "http://127.0.0.1:1456/callback?error=invalid_state&error_description=bad+state&state=xyz",
        );
        assert_eq!(parsed.error.as_deref(), Some("invalid_state"));
        assert_eq!(parsed.error_description.as_deref(), Some("bad state"));
        assert!(parsed.token.is_none());
    }

    #[test]
    fn parse_callback_input_handles_raw_query_string() {
        let parsed = parse_callback_input("token=acc&state=xyz");
        assert_eq!(parsed.token.as_deref(), Some("acc"));
        assert_eq!(parsed.state.as_deref(), Some("xyz"));
    }

    #[test]
    fn decode_jwt_profile_reads_sub_email_name() {
        let payload = serde_json::json!({
            "sub": "user_01ABC",
            "email": "dev@example.com",
            "name": "Dev User",
            "default_team_id": "team-abc-123",
        });
        let encoded = URL_SAFE_NO_PAD.encode(payload.to_string());
        let token = format!("header.{encoded}.sig");
        let profile = decode_jwt_profile(&token);
        assert_eq!(profile.sub.as_deref(), Some("user_01ABC"));
        assert_eq!(profile.email.as_deref(), Some("dev@example.com"));
        assert_eq!(profile.name.as_deref(), Some("Dev User"));
        assert_eq!(profile.default_team_id.as_deref(), Some("team-abc-123"));
    }

    #[test]
    fn decode_jwt_profile_handles_malformed_token() {
        let profile = decode_jwt_profile("not-a-jwt");
        assert!(profile.sub.is_none());
        assert!(profile.email.is_none());
        assert!(profile.name.is_none());
    }

    // Note: `exchange_jwt_for_api_key` now does two real HTTP hops
    // (`/auth/exchange` then `/platform/v1/teams/{team_id}/keys`).
    // Unit tests would need an HTTP mock server; we exercise the
    // function through the manual e2e probe in `puffer auth login
    // worldagent` instead.
}
