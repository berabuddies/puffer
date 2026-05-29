use anyhow::{anyhow, bail, Result};
use serde_json::{Number, Value};

const THEME_OPTIONS: &[&str] = &["puffer", "harbor", "sunrise"];
const EDITOR_MODE_OPTIONS: &[&str] = &["normal", "vim"];
const EFFORT_LEVEL_OPTIONS: &[&str] = &["auto", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Describes how one config setting expects its value to be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSettingValueKind {
    String,
    NullableString,
    Boolean,
    StringMap,
    NullableUnsignedInteger,
}

/// Describes whether one config setting persists to the workspace config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSettingScope {
    User,
    Workspace,
    Session,
}

/// Declares one supported `/config` and `Config` tool setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigSettingSpec {
    pub canonical_key: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub value_kind: ConfigSettingValueKind,
    pub scope: ConfigSettingScope,
    pub options: &'static [&'static str],
}

const SUPPORTED_CONFIG_SETTINGS: &[ConfigSettingSpec] = &[
    ConfigSettingSpec {
        canonical_key: "theme",
        aliases: &[],
        description: "Color theme for the UI.",
        value_kind: ConfigSettingValueKind::String,
        scope: ConfigSettingScope::User,
        options: THEME_OPTIONS,
    },
    ConfigSettingSpec {
        canonical_key: "model",
        aliases: &[],
        description: "Default provider/model selection for new turns.",
        value_kind: ConfigSettingValueKind::NullableString,
        scope: ConfigSettingScope::User,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "editorMode",
        aliases: &[],
        description: "Editor mode (`normal` or `vim`).",
        value_kind: ConfigSettingValueKind::String,
        scope: ConfigSettingScope::User,
        options: EDITOR_MODE_OPTIONS,
    },
    ConfigSettingSpec {
        canonical_key: "fastMode",
        aliases: &[],
        description: "Fast mode preference.",
        value_kind: ConfigSettingValueKind::Boolean,
        scope: ConfigSettingScope::User,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "copy_full_response",
        aliases: &["copyFullResponse"],
        description: "Copy the full assistant response when using /copy.",
        value_kind: ConfigSettingValueKind::Boolean,
        scope: ConfigSettingScope::User,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "autodreamEnabled",
        aliases: &["autodream_enabled"],
        description: "Enable automatic AutoDream background memory consolidation.",
        value_kind: ConfigSettingValueKind::Boolean,
        scope: ConfigSettingScope::User,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "effortLevel",
        aliases: &[],
        description: "Reasoning effort preference.",
        value_kind: ConfigSettingValueKind::String,
        scope: ConfigSettingScope::User,
        options: EFFORT_LEVEL_OPTIONS,
    },
    ConfigSettingSpec {
        canonical_key: "promptColor",
        aliases: &[],
        description: "Prompt bar color for the current session.",
        value_kind: ConfigSettingValueKind::String,
        scope: ConfigSettingScope::Session,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "statuslineEnabled",
        aliases: &[],
        description: "Status line visibility for the current session.",
        value_kind: ConfigSettingValueKind::Boolean,
        scope: ConfigSettingScope::Session,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "default_provider",
        aliases: &["defaultProvider"],
        description: "Default provider ID.",
        value_kind: ConfigSettingValueKind::NullableString,
        scope: ConfigSettingScope::User,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "default_model",
        aliases: &["defaultModel"],
        description: "Default model selector.",
        value_kind: ConfigSettingValueKind::NullableString,
        scope: ConfigSettingScope::User,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "openai_base_url",
        aliases: &["openaiBaseUrl"],
        description: "Workspace OpenAI-compatible base URL override.",
        value_kind: ConfigSettingValueKind::NullableString,
        scope: ConfigSettingScope::Workspace,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "openai_headers",
        aliases: &["openaiHeaders"],
        description: "Workspace OpenAI-compatible custom headers as a JSON object.",
        value_kind: ConfigSettingValueKind::StringMap,
        scope: ConfigSettingScope::Workspace,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "openai_query_params",
        aliases: &["openaiQueryParams"],
        description: "Workspace OpenAI-compatible query params as a JSON object.",
        value_kind: ConfigSettingValueKind::StringMap,
        scope: ConfigSettingScope::Workspace,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "no_alt_screen",
        aliases: &["noAltScreen"],
        description: "Disable alternate screen mode in this workspace.",
        value_kind: ConfigSettingValueKind::Boolean,
        scope: ConfigSettingScope::Workspace,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "tmux_golden_mode",
        aliases: &["tmuxGoldenMode"],
        description: "Enable tmux golden snapshot mode in this workspace.",
        value_kind: ConfigSettingValueKind::Boolean,
        scope: ConfigSettingScope::Workspace,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "status_line_command",
        aliases: &["statusLineCommand"],
        description: "Workspace status line command.",
        value_kind: ConfigSettingValueKind::NullableString,
        scope: ConfigSettingScope::Workspace,
        options: &[],
    },
    ConfigSettingSpec {
        canonical_key: "status_line_padding",
        aliases: &["statusLinePadding"],
        description: "Workspace status line padding.",
        value_kind: ConfigSettingValueKind::NullableUnsignedInteger,
        scope: ConfigSettingScope::Workspace,
        options: &[],
    },
];

/// Returns the full supported config-setting catalog.
pub fn supported_config_settings() -> &'static [ConfigSettingSpec] {
    SUPPORTED_CONFIG_SETTINGS
}

/// Looks up one config setting by canonical key or alias.
pub fn config_setting_spec(key: &str) -> Option<&'static ConfigSettingSpec> {
    let trimmed = key.trim();
    SUPPORTED_CONFIG_SETTINGS.iter().find(|spec| {
        spec.canonical_key == trimmed || spec.aliases.iter().any(|alias| *alias == trimmed)
    })
}

/// Normalizes one config-setting key to its canonical form.
pub fn normalize_config_setting_key(key: &str) -> Option<&'static str> {
    config_setting_spec(key).map(|spec| spec.canonical_key)
}

/// Returns whether one config setting persists to `.puffer/config.toml`.
pub fn config_setting_persists_to_workspace_file(key: &str) -> bool {
    config_setting_spec(key)
        .map(|spec| spec.scope == ConfigSettingScope::Workspace)
        .unwrap_or(false)
}

/// Returns the persistence scope for one config setting.
pub fn config_setting_scope(key: &str) -> Option<ConfigSettingScope> {
    config_setting_spec(key).map(|spec| spec.scope)
}

/// Parses one `/config set` value string into the typed JSON payload used by the Config tool.
pub fn parse_config_cli_value(key: &str, raw_value: &str) -> Result<Value> {
    let spec = config_setting_spec(key)
        .ok_or_else(|| anyhow!("Unsupported config key `{}`.", key.trim()))?;
    parse_value_for_spec(spec, raw_value.trim())
}

fn parse_value_for_spec(spec: &ConfigSettingSpec, raw_value: &str) -> Result<Value> {
    match spec.value_kind {
        ConfigSettingValueKind::String => Ok(Value::String(parse_string_value(raw_value))),
        ConfigSettingValueKind::NullableString => {
            if is_null_keyword(raw_value) {
                Ok(Value::Null)
            } else {
                Ok(Value::String(parse_string_value(raw_value)))
            }
        }
        ConfigSettingValueKind::Boolean => parse_bool_value(raw_value).map(Value::Bool),
        ConfigSettingValueKind::StringMap => parse_string_map_value(raw_value),
        ConfigSettingValueKind::NullableUnsignedInteger => {
            if is_null_keyword(raw_value) {
                Ok(Value::Null)
            } else {
                let value = raw_value.parse::<u64>().map_err(|_| {
                    anyhow!(
                        "expected an unsigned integer value for `{}`, got `{raw_value}`",
                        spec.canonical_key
                    )
                })?;
                Ok(Value::Number(Number::from(value)))
            }
        }
    }
}

fn parse_string_map_value(raw_value: &str) -> Result<Value> {
    if is_null_keyword(raw_value) {
        return Ok(Value::Null);
    }
    let parsed: Value = serde_json::from_str(raw_value).map_err(|error| {
        anyhow!("expected a JSON object with string values, got `{raw_value}`: {error}")
    })?;
    let Value::Object(entries) = parsed else {
        bail!("expected a JSON object with string values, got `{raw_value}`");
    };
    for (key, value) in &entries {
        if !value.is_string() {
            bail!("expected `{key}` to have a string value");
        }
    }
    Ok(Value::Object(entries))
}

fn parse_bool_value(raw_value: &str) -> Result<bool> {
    match raw_value.trim().to_ascii_lowercase().as_str() {
        "true" | "on" | "1" | "yes" => Ok(true),
        "false" | "off" | "0" | "no" => Ok(false),
        _ => bail!("expected a boolean value, got `{raw_value}`"),
    }
}

fn parse_string_value(raw_value: &str) -> String {
    let trimmed = raw_value.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        if let Ok(parsed) = serde_json::from_str::<String>(trimmed) {
            return parsed;
        }
    }
    trimmed.to_string()
}

fn is_null_keyword(raw_value: &str) -> bool {
    matches!(
        raw_value.trim().to_ascii_lowercase().as_str(),
        "null" | "none" | "default" | "unset" | "<unset>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn alias_lookup_returns_canonical_setting() {
        assert_eq!(
            normalize_config_setting_key("defaultProvider"),
            Some("default_provider")
        );
        assert_eq!(
            normalize_config_setting_key("statusLinePadding"),
            Some("status_line_padding")
        );
    }

    #[test]
    fn cli_value_parser_supports_boolean_null_and_object_values() {
        assert_eq!(
            parse_config_cli_value("fastMode", "on").unwrap(),
            json!(true)
        );
        assert_eq!(
            parse_config_cli_value("model", "default").unwrap(),
            Value::Null
        );
        assert_eq!(
            parse_config_cli_value("openai_headers", "{\"x-test\":\"one\"}").unwrap(),
            json!({"x-test": "one"})
        );
    }

    #[test]
    fn cli_value_parser_rejects_non_string_map_values() {
        let error = parse_config_cli_value("openai_headers", "{\"x-test\":1}")
            .unwrap_err()
            .to_string();
        assert!(error.contains("string value"));
    }
}
