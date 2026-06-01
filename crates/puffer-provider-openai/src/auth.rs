use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngCore;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::codex::{default_headers, default_originator, OPENAI_CODEX_COMPAT_VERSION};

/// The registered OpenAI OAuth client id used for Codex-style CLI flows.
pub const OPENAI_CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// The OpenAI OAuth authorization endpoint.
pub const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";

/// The OpenAI OAuth token endpoint.
pub const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

const OPENAI_REFRESH_TOKEN_URL_OVERRIDE_ENV: &str = "CODEX_REFRESH_TOKEN_URL_OVERRIDE";

/// The default localhost redirect URI for the OpenAI/Codex OAuth flow.
pub const OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";

/// The default OpenAI OAuth scope set used by the Codex-like flow.
pub const OPENAI_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";

/// Authentication modes supported by the OpenAI provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OpenAIAuth {
    None,
    ApiKey(String),
    OAuthBearer(String),
}

/// Persisted OAuth credentials for the OpenAI/Codex flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAIOAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: u64,
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

/// Stores generated PKCE values for the OpenAI OAuth flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAIPkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

/// Parameters required to build an OpenAI OAuth authorization URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAIOAuthConfig {
    pub state: String,
    pub code_challenge: String,
    pub redirect_uri: String,
    pub originator: String,
}

impl Default for OpenAIOAuthConfig {
    fn default() -> Self {
        Self {
            state: "puffer-state".to_string(),
            code_challenge: "puffer-code-challenge".to_string(),
            redirect_uri: OPENAI_REDIRECT_URI.to_string(),
            originator: default_originator(),
        }
    }
}

/// Generates a PKCE verifier/challenge pair and state for OpenAI OAuth.
pub(crate) fn generate_pkce() -> OpenAIPkce {
    let mut verifier_bytes = [0_u8; 64];
    rand::rng().fill_bytes(&mut verifier_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(digest);
    let mut state_bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut state_bytes);
    OpenAIPkce {
        verifier: verifier.clone(),
        challenge,
        state: URL_SAFE_NO_PAD.encode(state_bytes),
    }
}

/// Builds an OpenAI OAuth authorization URL from the provided flow parameters.
pub(crate) fn build_authorization_url(config: &OpenAIOAuthConfig) -> String {
    let mut url = url::Url::parse(OPENAI_AUTHORIZE_URL).expect("valid OpenAI authorize URL");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", OPENAI_CODEX_CLIENT_ID)
        .append_pair("redirect_uri", &config.redirect_uri)
        .append_pair("scope", OPENAI_SCOPE)
        .append_pair("code_challenge", &config.code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &config.state)
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", &config.originator);
    url.to_string()
}

/// Extracts an OAuth code and optional state from pasted manual input.
pub(crate) fn parse_authorization_input(input: &str) -> (Option<String>, Option<String>) {
    let value = input.trim();
    if value.is_empty() {
        return (None, None);
    }

    if let Ok(url) = url::Url::parse(value) {
        return (
            url.query_pairs()
                .find(|(key, _)| key == "code")
                .map(|(_, value)| value.into_owned()),
            url.query_pairs()
                .find(|(key, _)| key == "state")
                .map(|(_, value)| value.into_owned()),
        );
    }

    if let Some((code, state)) = value.split_once('#') {
        return (Some(code.to_string()), Some(state.to_string()));
    }

    if value.contains("code=") {
        let mut code = None;
        let mut state = None;
        for (key, value) in url::form_urlencoded::parse(value.as_bytes()) {
            if key == "code" {
                code = Some(value.into_owned());
            } else if key == "state" {
                state = Some(value.into_owned());
            }
        }
        return (code, state);
    }

    (Some(value.to_string()), None)
}

/// Exchanges an OpenAI OAuth authorization code for persisted OAuth credentials.
pub(crate) fn exchange_authorization_code(
    code: &str,
    verifier: &str,
    redirect_uri: Option<&str>,
) -> Result<OpenAIOAuthCredentials> {
    let client = codex_oauth_client()?;
    exchange_authorization_code_with_client(&client, code, verifier, redirect_uri)
}

pub(crate) fn exchange_authorization_code_with_client(
    client: &Client,
    code: &str,
    verifier: &str,
    redirect_uri: Option<&str>,
) -> Result<OpenAIOAuthCredentials> {
    let response = client
        .post(OPENAI_TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri.unwrap_or(OPENAI_REDIRECT_URI)),
            ("client_id", OPENAI_CODEX_CLIENT_ID),
        ])
        .send()
        .context("failed to exchange OpenAI authorization code")?;
    parse_token_response(response, "OpenAI token exchange")
}

/// Refreshes OpenAI OAuth credentials using a stored refresh token.
pub(crate) fn refresh_oauth_token(refresh_token: &str) -> Result<OpenAIOAuthCredentials> {
    let client = codex_oauth_client()?;
    refresh_oauth_token_with_client(&client, refresh_token)
}

pub(crate) fn refresh_oauth_token_with_client(
    client: &Client,
    refresh_token: &str,
) -> Result<OpenAIOAuthCredentials> {
    let response = client
        .post(openai_refresh_token_url())
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", OPENAI_CODEX_CLIENT_ID),
        ])
        .send()
        .context("failed to refresh OpenAI OAuth token")?;
    parse_token_response(response, "OpenAI token refresh")
}

fn parse_token_response(
    response: reqwest::blocking::Response,
    label: &str,
) -> Result<OpenAIOAuthCredentials> {
    let status = response.status();
    let payload: OpenAITokenResponse = response
        .json()
        .with_context(|| format!("failed to parse {label} response"))?;
    if !status.is_success() {
        return Err(anyhow!("{label} failed with status {status}"));
    }
    token_response_to_credentials(payload)
}

fn codex_oauth_client() -> Result<Client> {
    Client::builder()
        .default_headers(default_headers(
            OPENAI_CODEX_COMPAT_VERSION,
            &default_originator(),
        ))
        .build()
        .context("failed to build OpenAI OAuth client")
}

fn openai_refresh_token_url() -> String {
    std::env::var(OPENAI_REFRESH_TOKEN_URL_OVERRIDE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| OPENAI_TOKEN_URL.to_string())
}

fn token_response_to_credentials(payload: OpenAITokenResponse) -> Result<OpenAIOAuthCredentials> {
    let access_token = payload
        .access_token
        .ok_or_else(|| anyhow!("OpenAI token response did not include access_token"))?;
    let refresh_token = payload
        .refresh_token
        .ok_or_else(|| anyhow!("OpenAI token response did not include refresh_token"))?;
    let expires_in = payload
        .expires_in
        .ok_or_else(|| anyhow!("OpenAI token response did not include expires_in"))?;
    Ok(OpenAIOAuthCredentials {
        account_id: decode_account_id(&access_token),
        email: payload.id_token.as_deref().and_then(decode_email),
        plan_type: payload.id_token.as_deref().and_then(decode_plan_type),
        access_token,
        refresh_token,
        expires_at_ms: now_ms() + (expires_in as u64) * 1000,
    })
}

fn decode_account_id(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get("https://api.openai.com/auth")
        .and_then(|claim| claim.get("chatgpt_account_id"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Deserialize)]
struct OpenAITokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u32>,
    #[serde(default)]
    id_token: Option<String>,
}

fn decode_email(id_token: &str) -> Option<String> {
    let value = decode_jwt_payload(id_token)?;
    value
        .get("email")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            value
                .pointer("/https://api.openai.com/profile/email")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
}

fn decode_plan_type(id_token: &str) -> Option<String> {
    let value = decode_jwt_payload(id_token)?;
    value
        .get("https://api.openai.com/auth")
        .and_then(|claim| claim.get("chatgpt_plan_type"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    serde_json::from_slice(&decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_url_contains_expected_parameters() {
        let url = build_authorization_url(&OpenAIOAuthConfig {
            state: "state-1".to_string(),
            code_challenge: "challenge-1".to_string(),
            redirect_uri: OPENAI_REDIRECT_URI.to_string(),
            originator: "puffer".to_string(),
        });
        assert!(url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(url.contains("state=state-1"));
        assert!(url.contains("code_challenge=challenge-1"));
    }

    #[test]
    fn parse_authorization_input_handles_url() {
        let (code, state) =
            parse_authorization_input("http://localhost:1455/auth/callback?code=abc&state=xyz");
        assert_eq!(code.as_deref(), Some("abc"));
        assert_eq!(state.as_deref(), Some("xyz"));
    }

    #[test]
    fn generate_pkce_produces_non_empty_values() {
        let pkce = generate_pkce();
        assert!(!pkce.verifier.is_empty());
        assert!(!pkce.challenge.is_empty());
        assert!(!pkce.state.is_empty());
    }

    #[test]
    fn refresh_oauth_token_with_client_builds_request_path() {
        let client = codex_oauth_client().expect("client");
        let result = refresh_oauth_token_with_client(&client, "refresh-token");
        assert!(result.is_err());
    }

    #[test]
    fn decodes_account_id_from_jwt_payload() {
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct-123"
            }
        });
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        let token = format!("header.{payload}.sig");
        assert_eq!(decode_account_id(&token).as_deref(), Some("acct-123"));
    }

    #[test]
    fn decodes_email_and_plan_type_from_id_token() {
        let payload = serde_json::json!({
            "email": "dev@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "pro"
            }
        });
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        let token = format!("header.{payload}.sig");
        assert_eq!(decode_email(&token).as_deref(), Some("dev@example.com"));
        assert_eq!(decode_plan_type(&token).as_deref(), Some("pro"));
    }
}
