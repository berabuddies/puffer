use serde::{Deserialize, Serialize};

/// Platform-specific configuration for the webhook connector, parsed
/// from `[connectors.webhook]` in the Puffer config file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebhookConfig {
    /// Socket address the axum server binds to, e.g. `"127.0.0.1:8080"`.
    pub bind_address: String,

    /// HTTP path the inbound `POST` is served on. Defaults to `"/puffer"`.
    #[serde(default)]
    pub path: Option<String>,

    /// Optional shared secret expected in the `Authorization: Bearer ...`
    /// header. When absent, the endpoint is publicly accessible.
    #[serde(default)]
    pub auth_token: Option<String>,

    /// Browser origins allowed to talk to the endpoint. An empty list
    /// (the default) means any origin is acceptable — non-browser
    /// callers never send an `Origin` header anyway.
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// Optional greeting returned from `/start`. A default is used when
    /// absent.
    #[serde(default)]
    pub welcome_message: Option<String>,

    /// Whether the router should split long assistant replies into
    /// multiple chunks when returning them. Webhook responses are a
    /// single HTTP body so this is informational today — it only
    /// controls whether the connector logs a warning for over-sized
    /// bodies. Defaults to `true`.
    #[serde(default = "default_split_long_responses")]
    pub split_long_responses: bool,
}

fn default_split_long_responses() -> bool {
    true
}

impl WebhookConfig {
    /// HTTP path the connector serves, with the `/puffer` default
    /// applied. Always starts with `'/'`.
    pub fn resolved_path(&self) -> String {
        let raw = self
            .path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("/puffer");
        if raw.starts_with('/') {
            raw.to_string()
        } else {
            format!("/{raw}")
        }
    }

    /// Returns `true` when `token` matches the configured bearer
    /// secret. When no secret is configured every caller is allowed.
    pub fn is_token_allowed(&self, token: Option<&str>) -> bool {
        match self.auth_token.as_deref() {
            None => true,
            Some(expected) => token == Some(expected),
        }
    }

    /// Returns `true` when `origin` is permitted to call the endpoint.
    /// Callers that don't send an `Origin` header (cURL, backend
    /// services, …) are always allowed; browsers are checked against
    /// `allowed_origins`.
    pub fn is_origin_allowed(&self, origin: Option<&str>) -> bool {
        if self.allowed_origins.is_empty() {
            return true;
        }
        match origin {
            None => true,
            Some(value) => self.allowed_origins.iter().any(|o| o == value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parses_from_raw_json() {
        let raw = serde_json::json!({
            "bind_address": "127.0.0.1:8080",
            "path": "/hook",
            "auth_token": "sekret",
            "allowed_origins": ["https://app.example.com"],
            "welcome_message": "hi"
        });
        let config: WebhookConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(config.bind_address, "127.0.0.1:8080");
        assert_eq!(config.resolved_path(), "/hook");
        assert_eq!(config.auth_token.as_deref(), Some("sekret"));
        assert_eq!(config.allowed_origins, vec!["https://app.example.com"]);
        assert_eq!(config.welcome_message.as_deref(), Some("hi"));
    }

    #[test]
    fn resolved_path_defaults_to_slash_puffer() {
        let config: WebhookConfig = serde_json::from_value(serde_json::json!({
            "bind_address": "127.0.0.1:9001"
        }))
        .unwrap();
        assert_eq!(config.resolved_path(), "/puffer");
        // Leading slash is ensured even if the caller forgets it.
        let tweaked = WebhookConfig {
            bind_address: "127.0.0.1:9001".to_string(),
            path: Some("hook".to_string()),
            auth_token: None,
            allowed_origins: Vec::new(),
            welcome_message: None,
            split_long_responses: true,
        };
        assert_eq!(tweaked.resolved_path(), "/hook");
    }

    #[test]
    fn missing_auth_token_means_public_endpoint() {
        let config: WebhookConfig = serde_json::from_value(serde_json::json!({
            "bind_address": "127.0.0.1:9001"
        }))
        .unwrap();
        assert!(config.auth_token.is_none());
        assert!(config.is_token_allowed(None));
        assert!(config.is_token_allowed(Some("anything")));
    }

    #[test]
    fn split_long_responses_defaults_to_true_when_missing() {
        let config: WebhookConfig = serde_json::from_value(serde_json::json!({
            "bind_address": "127.0.0.1:9001"
        }))
        .unwrap();
        assert!(config.split_long_responses);
    }

    #[test]
    fn split_long_responses_respects_explicit_false() {
        let config: WebhookConfig = serde_json::from_value(serde_json::json!({
            "bind_address": "127.0.0.1:9001",
            "split_long_responses": false
        }))
        .unwrap();
        assert!(!config.split_long_responses);
    }

    #[test]
    fn allowed_origins_defaults_to_empty_and_accepts_all() {
        let config: WebhookConfig = serde_json::from_value(serde_json::json!({
            "bind_address": "127.0.0.1:9001"
        }))
        .unwrap();
        assert!(config.allowed_origins.is_empty());
        assert!(config.is_origin_allowed(None));
        assert!(config.is_origin_allowed(Some("https://any.example")));
    }
}
