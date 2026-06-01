use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Network-level settings for traffic initiated by Puffer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NetworkConfig {
    #[serde(default)]
    pub proxy: ProxyConfig,
}

/// Provider proxy settings persisted in user configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProxyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub selected: Option<String>,
    #[serde(default = "default_proxy_bypass")]
    pub bypass: Vec<String>,
    #[serde(default)]
    pub proxies: Vec<ProxyEndpoint>,
}

/// One configured proxy endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProxyEndpoint {
    pub id: String,
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

/// Supported proxy URI schemes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProxyScheme {
    Http,
    Https,
    Socks5,
    Socks5h,
}

/// Credential-free proxy endpoint snapshot suitable for UI payloads.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SanitizedProxyEndpoint {
    pub id: String,
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub has_password: bool,
    pub uri: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            selected: None,
            bypass: default_proxy_bypass(),
            proxies: Vec::new(),
        }
    }
}

impl ProxyScheme {
    /// Returns the URI scheme string accepted by reqwest proxy configuration.
    pub fn as_uri_scheme(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Socks5 => "socks5",
            Self::Socks5h => "socks5h",
        }
    }
}

impl ProxyEndpoint {
    /// Returns a credential-free endpoint snapshot suitable for UI payloads.
    pub fn sanitized(&self) -> SanitizedProxyEndpoint {
        SanitizedProxyEndpoint {
            id: self.id.clone(),
            scheme: self.scheme,
            host: self.host.clone(),
            port: self.port,
            username: self
                .username
                .clone()
                .filter(|value| !value.trim().is_empty()),
            has_password: self
                .password
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            uri: format!(
                "{}://{}:{}",
                self.scheme.as_uri_scheme(),
                self.host,
                self.port
            ),
        }
    }
}

impl ProxyConfig {
    /// Normalizes enabled and selected state against the configured proxy list.
    pub fn normalize_selection(&mut self) {
        self.selected = self.selected.as_ref().and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });

        if self.proxies.is_empty() {
            self.enabled = false;
            self.selected = None;
            return;
        }

        let selected_exists = self
            .selected
            .as_deref()
            .is_some_and(|selected| self.proxies.iter().any(|endpoint| endpoint.id == selected));
        if !selected_exists {
            self.selected = self.enabled.then(|| self.proxies[0].id.clone());
        }
    }

    /// Validates selected proxy, endpoint identity, endpoint address, and bypass entries.
    pub fn validate(&self) -> Result<()> {
        let mut ids = std::collections::BTreeSet::new();
        for endpoint in &self.proxies {
            if endpoint.id.trim().is_empty() {
                anyhow::bail!("proxy id must not be empty");
            }
            if !ids.insert(endpoint.id.as_str()) {
                anyhow::bail!("duplicate proxy id `{}`", endpoint.id);
            }
            if endpoint.host.trim().is_empty() {
                anyhow::bail!("proxy `{}` host must not be empty", endpoint.id);
            }
            if endpoint.port == 0 {
                anyhow::bail!("proxy `{}` port must be between 1 and 65535", endpoint.id);
            }
        }
        if let Some(selected) = self
            .selected
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            if !ids.contains(selected) {
                anyhow::bail!("selected proxy `{selected}` does not exist");
            }
        }
        for entry in &self.bypass {
            validate_bypass_entry(entry)?;
        }
        Ok(())
    }
}

fn default_proxy_bypass() -> Vec<String> {
    vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
        "10.0.0.0/8".to_string(),
        "172.16.0.0/12".to_string(),
        "192.168.0.0/16".to_string(),
    ]
}

fn validate_bypass_entry(entry: &str) -> Result<()> {
    let trimmed = entry.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if trimmed.parse::<std::net::IpAddr>().is_ok() {
        return Ok(());
    }
    if trimmed.parse::<ipnet::IpNet>().is_ok() {
        return Ok(());
    }
    if trimmed.contains('*') || trimmed.contains('/') || trimmed.contains('\\') {
        anyhow::bail!("invalid proxy bypass entry `{trimmed}`");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{ProxyEndpoint, ProxyScheme, PufferConfig};

    #[test]
    fn proxy_config_defaults_to_disabled_with_bypass_entries() {
        let config = PufferConfig::default();
        assert!(!config.network.proxy.enabled);
        assert_eq!(config.network.proxy.selected, None);
        assert!(config
            .network
            .proxy
            .bypass
            .iter()
            .any(|entry| entry == "127.0.0.1"));
        assert!(config.network.proxy.proxies.is_empty());
    }

    #[test]
    fn proxy_config_round_trips_through_toml() {
        let raw = r#"
app_name = "Puffer Code"
default_provider = "anthropic"
theme = "puffer"

[memory]
enabled = true
char_limit = 6000
review_nudge_interval = 8
flush_min_turns = 6
background_review = true
flush_on_compact = true

[recap]
enabled = true
auto = true
idle_secs = 180
min_user_messages = 3
cooldown_messages = 2

[browser]

[mascot]
id = "clawd"
display_name = "Clawd"
enabled = true

[ui]
no_alt_screen = false
tmux_golden_mode = false

[network.proxy]
enabled = true
selected = "local-socks"
bypass = ["localhost", "127.0.0.1", "::1", "10.0.0.0/8"]

[[network.proxy.proxies]]
id = "local-socks"
scheme = "socks5h"
host = "127.0.0.1"
port = 7890
username = "alice"
password = "secret"
"#;
        let config: PufferConfig = toml::from_str(raw).expect("config");
        assert!(config.network.proxy.enabled);
        assert_eq!(
            config.network.proxy.selected.as_deref(),
            Some("local-socks")
        );
        assert_eq!(config.network.proxy.proxies[0].scheme, ProxyScheme::Socks5h);
        assert_eq!(config.network.proxy.proxies[0].host, "127.0.0.1");
        assert_eq!(config.network.proxy.proxies[0].port, 7890);
        assert_eq!(
            config.network.proxy.proxies[0].username.as_deref(),
            Some("alice")
        );
        assert_eq!(
            config.network.proxy.proxies[0].password.as_deref(),
            Some("secret")
        );

        let serialized = toml::to_string(&config).expect("serialize");
        assert!(serialized.contains("[network.proxy]"));
        assert!(serialized.contains("[[network.proxy.proxies]]"));
    }

    #[test]
    fn proxy_config_validation_rejects_bad_values() {
        let mut config = PufferConfig::default();
        config.network.proxy.enabled = true;
        assert!(config.network.proxy.validate().is_ok());

        config.network.proxy.selected = Some("missing".to_string());
        config.network.proxy.proxies.push(ProxyEndpoint {
            id: "local-socks".to_string(),
            scheme: ProxyScheme::Socks5h,
            host: "127.0.0.1".to_string(),
            port: 7890,
            username: None,
            password: None,
        });
        assert!(config.network.proxy.validate().is_err());

        config.network.proxy.selected = Some("local-socks".to_string());
        config.network.proxy.proxies.push(ProxyEndpoint {
            id: "local-socks".to_string(),
            scheme: ProxyScheme::Http,
            host: "example.com".to_string(),
            port: 8080,
            username: None,
            password: None,
        });
        assert!(config.network.proxy.validate().is_err());
    }

    #[test]
    fn proxy_config_normalizes_enabled_empty_list_to_disabled() {
        let mut proxy = PufferConfig::default().network.proxy;
        proxy.enabled = true;
        proxy.selected = Some("missing".to_string());

        proxy.normalize_selection();

        assert!(!proxy.enabled);
        assert_eq!(proxy.selected, None);
        assert!(proxy.validate().is_ok());
    }

    #[test]
    fn sanitized_proxy_snapshot_redacts_credentials() {
        let endpoint = ProxyEndpoint {
            id: "office".to_string(),
            scheme: ProxyScheme::Http,
            host: "proxy.example".to_string(),
            port: 8080,
            username: Some("alice".to_string()),
            password: Some("secret".to_string()),
        };
        let sanitized = endpoint.sanitized();
        assert_eq!(sanitized.username.as_deref(), Some("alice"));
        assert!(sanitized.has_password);
        assert_eq!(sanitized.uri, "http://proxy.example:8080");
    }
}
