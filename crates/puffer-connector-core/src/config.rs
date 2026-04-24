use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Root configuration block read from `connectors` section of the Puffer
/// configuration file.
///
/// Each platform-specific crate is responsible for deserializing its own
/// entry out of [`ConnectorsConfig::platforms`] (keyed by platform id).
/// This keeps `puffer-connector-core` independent of any specific SDK.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectorsConfig {
    /// If `false`, the runtime ignores all configured connectors.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Per-platform raw config blobs.
    #[serde(default, flatten)]
    pub platforms: BTreeMap<String, ConnectorConfig>,
}

impl Default for ConnectorsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            platforms: BTreeMap::new(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

/// Raw, platform-specific config blob captured at the TOML/JSON level.
///
/// Each platform crate owns its own concrete schema and parses this via
/// `serde_json::from_value(config.raw.clone())` when it starts up.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectorConfig {
    /// If `false`, this specific connector is skipped on startup.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Optional human-facing display name; defaults to the platform id.
    #[serde(default)]
    pub display_name: Option<String>,

    /// Platform-specific configuration as raw JSON; deserialized by the
    /// platform crate into its own typed struct.
    #[serde(default, flatten)]
    pub raw: serde_json::Map<String, serde_json::Value>,
}

impl Default for ConnectorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            display_name: None,
            raw: serde_json::Map::new(),
        }
    }
}

impl ConnectorConfig {
    /// Parses the platform-specific `raw` blob into a typed struct.
    pub fn parse<T: serde::de::DeserializeOwned>(&self) -> anyhow::Result<T> {
        let value = serde_json::Value::Object(self.raw.clone());
        serde_json::from_value(value).map_err(|error| {
            anyhow::anyhow!("failed to parse connector config: {error}")
        })
    }
}

impl ConnectorsConfig {
    /// Loads a connector config file from `path`. Returns the default
    /// (enabled, empty) config when the file does not exist.
    ///
    /// The file is TOML; top-level keys that look like
    /// `[connectors.<platform>]` (or just `[<platform>]` at the root)
    /// become entries in [`ConnectorsConfig::platforms`].
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read connector config {}", path.display()))?;
        Self::from_toml(&raw).with_context(|| {
            format!("failed to parse connector config {}", path.display())
        })
    }

    /// Parses a connector config from a TOML string.
    ///
    /// Accepts two shapes for flexibility:
    /// 1. `[<platform>]` keys at the top level, alongside an optional
    ///    `enabled = false` toggle to disable all connectors.
    /// 2. A single `[connectors]` table containing the same.
    pub fn from_toml(raw: &str) -> Result<Self> {
        let value: toml::Value = toml::from_str(raw).context("invalid TOML")?;
        // Accept either shape:
        //   - a bare document (`[telegram]` at the top level, …)
        //   - a `[connectors]` wrapper
        let table = match value {
            toml::Value::Table(mut root) => match root.remove("connectors") {
                Some(toml::Value::Table(inner)) => inner,
                Some(other) => {
                    return Err(anyhow::anyhow!(
                        "`connectors` key must be a table, got {}",
                        other.type_str()
                    ));
                }
                None => root,
            },
            other => {
                return Err(anyhow::anyhow!(
                    "connector config root must be a table, got {}",
                    other.type_str()
                ));
            }
        };

        let mut enabled = true;
        let mut platforms: BTreeMap<String, ConnectorConfig> = BTreeMap::new();
        for (key, value) in table {
            if key == "enabled" {
                if let toml::Value::Boolean(flag) = value {
                    enabled = flag;
                }
                continue;
            }
            let json = toml_to_json(value);
            let config: ConnectorConfig = serde_json::from_value(json).with_context(|| {
                format!("invalid config for connector `{key}`")
            })?;
            platforms.insert(key, config);
        }
        Ok(Self { enabled, platforms })
    }
}

/// Converts a `toml::Value` into a `serde_json::Value` losslessly.
fn toml_to_json(value: toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(text) => serde_json::Value::String(text),
        toml::Value::Integer(n) => serde_json::Value::Number(n.into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        toml::Value::Boolean(b) => serde_json::Value::Bool(b),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(toml_to_json).collect())
        }
        toml::Value::Table(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                out.insert(key, toml_to_json(value));
            }
            serde_json::Value::Object(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_platform_specific_fields_via_serde() {
        let raw = serde_json::json!({
            "enabled": true,
            "display_name": "My Telegram",
            "token": "bot-token",
            "allowed_users": ["42", "7"]
        });
        let config: ConnectorConfig = serde_json::from_value(raw).unwrap();
        assert!(config.enabled);
        assert_eq!(config.display_name.as_deref(), Some("My Telegram"));

        #[derive(Debug, Deserialize)]
        struct TelegramShape {
            token: String,
            allowed_users: Vec<String>,
        }

        let parsed: TelegramShape = config.parse().unwrap();
        assert_eq!(parsed.token, "bot-token");
        assert_eq!(parsed.allowed_users, vec!["42", "7"]);
    }

    #[test]
    fn connectors_config_defaults_to_enabled_empty() {
        let cfg = ConnectorsConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.platforms.is_empty());
    }

    #[test]
    fn parse_surfaces_missing_required_fields() {
        let config = ConnectorConfig::default();
        #[derive(Debug, Deserialize)]
        struct Required {
            #[allow(dead_code)]
            token: String,
        }
        assert!(config.parse::<Required>().is_err());
    }

    #[test]
    fn load_returns_default_when_file_is_absent() {
        let cfg = ConnectorsConfig::load(std::path::Path::new("/does/not/exist/x.toml")).unwrap();
        assert!(cfg.enabled);
        assert!(cfg.platforms.is_empty());
    }

    #[test]
    fn from_toml_reads_bare_platform_tables() {
        let raw = r#"
[telegram]
token = "bot-token"
allowed_users = [42, 7]

[discord]
token = "disc-token"
"#;
        let cfg = ConnectorsConfig::from_toml(raw).unwrap();
        assert!(cfg.enabled);
        assert!(cfg.platforms.contains_key("telegram"));
        assert!(cfg.platforms.contains_key("discord"));
        let tg = cfg.platforms.get("telegram").unwrap();
        assert!(tg.enabled);
        assert_eq!(
            tg.raw.get("token").and_then(|v| v.as_str()),
            Some("bot-token")
        );
    }

    #[test]
    fn from_toml_reads_wrapped_connectors_table() {
        let raw = r#"
[connectors]
enabled = false

[connectors.telegram]
token = "bot-token"
"#;
        let cfg = ConnectorsConfig::from_toml(raw).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.platforms.len(), 1);
        assert!(cfg.platforms.contains_key("telegram"));
    }

    #[test]
    fn from_toml_honors_per_platform_enabled_flag() {
        let raw = r#"
[telegram]
enabled = false
token = "bot-token"
"#;
        let cfg = ConnectorsConfig::from_toml(raw).unwrap();
        let tg = cfg.platforms.get("telegram").unwrap();
        assert!(!tg.enabled);
    }

    #[test]
    fn from_toml_reports_invalid_platform_payload() {
        // `enabled` must be a boolean; surfaces via parse error.
        let raw = r#"
[telegram]
enabled = "yes"
token = "bot-token"
"#;
        let error = ConnectorsConfig::from_toml(raw).unwrap_err();
        let message = format!("{error:?}");
        assert!(message.contains("telegram"), "unexpected error: {message}");
    }
}
