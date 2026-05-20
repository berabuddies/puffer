//! Auth Station login URL building, callback parsing, JWT decoding.

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngCore;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Default Auth Station base URL (Sandbox). Production is
/// `https://auth.worldrouter.ai`. The env var named by
/// [`WORLDAGENT_AUTH_URL_OVERRIDE_ENV`] overrides this at runtime.
pub const WORLDAGENT_AUTH_BASE_URL: &str = "https://auth-worldrouter.vercel.app";

/// Env var name that overrides the Auth Station base URL.
pub const WORLDAGENT_AUTH_URL_OVERRIDE_ENV: &str = "PUFFER_WORLDAGENT_AUTH_URL";

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
        expires_at_ms: now_ms() + 24 * 3600 * 1000,
        sub: profile.sub,
        email: profile.email,
        name: profile.name,
    })
}

/// Exchanges an Auth Station JWT for an inference API key.
///
/// **TODO (waiting on worldrouter backend):** the endpoint and
/// request shape are not yet finalized. Once defined, this function
/// will `POST <worldrouter>/api/v1/keys/exchange` (or whatever the
/// backend picks) with `Authorization: Bearer <access_token>` and
/// return the `api_key` string. The login handler will then upgrade
/// the stored credential to an `ApiKey { key }` variant.
pub fn exchange_jwt_for_api_key(_access_token: &str) -> Result<String> {
    Err(anyhow!(
        "worldagent JWT-to-api-key exchange is not yet implemented; \
         paste your WorldRouter API key for now"
    ))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
        });
        let encoded = URL_SAFE_NO_PAD.encode(payload.to_string());
        let token = format!("header.{encoded}.sig");
        let profile = decode_jwt_profile(&token);
        assert_eq!(profile.sub.as_deref(), Some("user_01ABC"));
        assert_eq!(profile.email.as_deref(), Some("dev@example.com"));
        assert_eq!(profile.name.as_deref(), Some("Dev User"));
    }

    #[test]
    fn decode_jwt_profile_handles_malformed_token() {
        let profile = decode_jwt_profile("not-a-jwt");
        assert!(profile.sub.is_none());
        assert!(profile.email.is_none());
        assert!(profile.name.is_none());
    }

    #[test]
    fn exchange_jwt_for_api_key_is_a_placeholder() {
        let result = exchange_jwt_for_api_key("any.access.token");
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("not yet implemented"));
    }
}
