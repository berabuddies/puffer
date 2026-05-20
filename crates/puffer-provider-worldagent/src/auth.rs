//! Auth Station login URL building, callback parsing, JWT decoding.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngCore;

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
}
