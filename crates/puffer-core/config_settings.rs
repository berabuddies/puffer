use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_config::{
    config_setting_scope as shared_config_setting_scope, normalize_config_setting_key,
    parse_config_cli_value as shared_parse_config_cli_value, save_user_config,
    save_workspace_config, supported_config_settings as shared_supported_config_settings,
    ConfigPaths, ConfigSettingScope, ConfigSettingSpec, PufferConfig,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Returns the shared metadata for supported `/config` and `Config` tool settings.
pub(crate) fn supported_config_settings() -> &'static [ConfigSettingSpec] {
    shared_supported_config_settings()
}

/// Normalizes a user-supplied config key to its canonical form.
pub(crate) fn normalize_config_key(setting: &str) -> Option<&'static str> {
    normalize_config_setting_key(setting)
}

/// Returns the persistence scope for one supported config setting.
pub(crate) fn config_setting_scope(setting: &str) -> Result<ConfigSettingScope> {
    shared_config_setting_scope(setting)
        .ok_or_else(|| anyhow!("Unsupported config setting `{}`", setting.trim()))
}

/// Returns the config file path that backs one setting, if any.
pub(crate) fn config_setting_path(paths: &ConfigPaths, setting: &str) -> Result<Option<PathBuf>> {
    Ok(match config_setting_scope(setting)? {
        ConfigSettingScope::User => Some(paths.user_config_file()),
        ConfigSettingScope::Workspace => Some(paths.workspace_config_file()),
        ConfigSettingScope::Session => None,
    })
}

/// Renders the shared help text for `/config`.
pub(crate) fn render_config_help() -> String {
    let mut text = String::from("Supported config keys:\n");
    for scope in [
        ConfigSettingScope::User,
        ConfigSettingScope::Workspace,
        ConfigSettingScope::Session,
    ] {
        let label = scope_label(scope);
        text.push_str(&format!("\n[{label}]\n"));
        for descriptor in shared_supported_config_settings()
            .iter()
            .filter(|entry| entry.scope == scope)
        {
            let aliases = if descriptor.aliases.is_empty() {
                String::new()
            } else {
                format!(" aliases={}", descriptor.aliases.join(", "))
            };
            text.push_str(&format!(
                "- {}{}: {}\n",
                descriptor.canonical_key, aliases, descriptor.description
            ));
        }
    }
    text.trim_end().to_string()
}

/// Returns the supported config key list used by `/config list`.
pub(crate) fn render_supported_config_key_list() -> String {
    render_config_help()
}

/// Parses a raw `/config set` value according to the setting's expected type.
pub(crate) fn parse_config_cli_value(setting: &str, raw: &str) -> Result<Value> {
    shared_parse_config_cli_value(setting, raw)
}

/// Reads one shared config setting from the active application state.
pub(crate) fn get_config_value(state: &AppState, setting: &str) -> Result<Value> {
    match config_setting_descriptor(setting)?.canonical_key {
        "theme" => Ok(json!(state.config.theme)),
        "model" => Ok(json!(state.current_model)),
        "editorMode" => Ok(json!(state.config.editor_mode)),
        "default_provider" => Ok(json!(state.config.default_provider)),
        "default_model" => Ok(json!(state.config.default_model)),
        "openai_base_url" => Ok(json!(state.config.openai_base_url)),
        "openai_headers" => Ok(json!(state.config.openai_headers)),
        "openai_query_params" => Ok(json!(state.config.openai_query_params)),
        "no_alt_screen" => Ok(json!(state.config.ui.no_alt_screen)),
        "tmux_golden_mode" => Ok(json!(state.config.ui.tmux_golden_mode)),
        "status_line_command" => Ok(json!(state
            .config
            .ui
            .status_line
            .as_ref()
            .map(|status_line| status_line.command.as_str()))),
        "status_line_padding" => Ok(json!(state
            .config
            .ui
            .status_line
            .as_ref()
            .map(|status_line| status_line.padding))),
        "fastMode" => Ok(json!(state.fast_mode)),
        "copy_full_response" => Ok(json!(state.config.copy_full_response)),
        "autodreamEnabled" => Ok(json!(state.config.memory.autodream_enabled)),
        "effortLevel" => Ok(json!(state
            .config
            .effort_level
            .as_deref()
            .unwrap_or("auto"))),
        "promptColor" => Ok(json!(state.prompt_color)),
        "statuslineEnabled" => Ok(json!(state.statusline_enabled)),
        other => bail!("Unsupported config setting `{other}`"),
    }
}

/// Applies one shared config setting to the active application state.
pub(crate) fn set_config_value(state: &mut AppState, setting: &str, value: Value) -> Result<()> {
    match config_setting_descriptor(setting)?.canonical_key {
        "theme" => {
            state.config.theme = value
                .as_str()
                .ok_or_else(|| anyhow!("theme must be a string"))?
                .to_string();
        }
        "model" => match value {
            Value::Null => {
                state.current_model = None;
                state.config.default_model = None;
                state.current_provider = state.config.default_provider.clone();
            }
            Value::String(model) => {
                state.current_model = Some(model.clone());
                state.config.default_model = Some(model.clone());
                state.current_provider = model
                    .split_once('/')
                    .map(|(provider, _)| provider.to_string())
                    .or_else(|| state.current_provider.clone());
                if let Some(provider) = state.current_provider.clone() {
                    state.config.default_provider = Some(provider);
                }
            }
            _ => bail!("model must be a string or null"),
        },
        "editorMode" => {
            let mode = value
                .as_str()
                .ok_or_else(|| anyhow!("editorMode must be a string"))?;
            match mode {
                "vim" => {
                    state.vim_mode = true;
                    state.config.editor_mode = "vim".to_string();
                }
                "default" | "normal" => {
                    state.vim_mode = false;
                    state.config.editor_mode = "normal".to_string();
                }
                other => bail!("unsupported editorMode `{other}`"),
            }
        }
        "default_provider" => {
            state.config.default_provider = match value {
                Value::Null => None,
                Value::String(text) => Some(text),
                _ => bail!("default_provider must be a string or null"),
            };
            state.current_provider = state.config.default_provider.clone();
        }
        "default_model" => {
            state.config.default_model = match value {
                Value::Null => None,
                Value::String(text) => Some(text),
                _ => bail!("default_model must be a string or null"),
            };
            state.current_model = state.config.default_model.clone();
            if let Some(provider) = state
                .config
                .default_model
                .as_deref()
                .and_then(|model| model.split_once('/').map(|(provider, _)| provider))
            {
                state.current_provider = Some(provider.to_string());
                state.config.default_provider = Some(provider.to_string());
            }
        }
        "openai_base_url" => {
            state.config.openai_base_url = match value {
                Value::Null => None,
                Value::String(text) => Some(text),
                _ => bail!("openai_base_url must be a string or null"),
            };
        }
        "openai_headers" => {
            state.config.openai_headers = value_to_string_map(value, "openai_headers")?;
        }
        "openai_query_params" => {
            state.config.openai_query_params = value_to_string_map(value, "openai_query_params")?;
        }
        "no_alt_screen" => {
            state.config.ui.no_alt_screen = value
                .as_bool()
                .ok_or_else(|| anyhow!("no_alt_screen must be a boolean"))?;
        }
        "tmux_golden_mode" => {
            state.config.ui.tmux_golden_mode = value
                .as_bool()
                .ok_or_else(|| anyhow!("tmux_golden_mode must be a boolean"))?;
        }
        "status_line_command" => {
            state.config.ui.status_line = match value {
                Value::Null => None,
                Value::String(command) => Some(puffer_config::StatusLineConfig {
                    command,
                    padding: state
                        .config
                        .ui
                        .status_line
                        .as_ref()
                        .map(|status_line| status_line.padding)
                        .unwrap_or(0),
                }),
                _ => bail!("status_line_command must be a string or null"),
            };
        }
        "status_line_padding" => {
            let padding = match value {
                Value::Null => 0,
                Value::Number(number) => number
                    .as_u64()
                    .ok_or_else(|| anyhow!("status_line_padding must be an unsigned integer"))?
                    as u16,
                _ => bail!("status_line_padding must be an unsigned integer or null"),
            };
            let status_line =
                state
                    .config
                    .ui
                    .status_line
                    .get_or_insert(puffer_config::StatusLineConfig {
                        command: String::new(),
                        padding: 0,
                    });
            status_line.padding = padding;
        }
        "fastMode" => {
            let parsed = value
                .as_bool()
                .ok_or_else(|| anyhow!("fastMode must be a boolean"))?;
            state.fast_mode = parsed;
            state.config.fast_mode = parsed;
        }
        "copy_full_response" => {
            state.config.copy_full_response = value
                .as_bool()
                .ok_or_else(|| anyhow!("copy_full_response must be a boolean"))?;
        }
        "autodreamEnabled" => {
            state.config.memory.autodream_enabled = value
                .as_bool()
                .ok_or_else(|| anyhow!("autodreamEnabled must be a boolean"))?;
        }
        "effortLevel" => {
            let parsed = value
                .as_str()
                .ok_or_else(|| anyhow!("effortLevel must be a string"))?;
            match parsed {
                "auto" | "unset" | "default" => {
                    state.effort_level = "auto".to_string();
                    state.config.effort_level = None;
                }
                other => {
                    state.effort_level = other.to_string();
                    state.config.effort_level = Some(other.to_string());
                }
            }
        }
        "promptColor" => {
            state.prompt_color = value
                .as_str()
                .ok_or_else(|| anyhow!("promptColor must be a string"))?
                .to_string();
        }
        "statuslineEnabled" => {
            state.statusline_enabled = value
                .as_bool()
                .ok_or_else(|| anyhow!("statuslineEnabled must be a boolean"))?;
        }
        other => bail!("Unsupported config setting `{other}`"),
    }
    Ok(())
}

/// Persists one shared config setting to the correct user or workspace config file.
pub(crate) fn persist_config_setting(
    paths: &ConfigPaths,
    state: &AppState,
    setting: &str,
) -> Result<Option<PathBuf>> {
    match config_setting_scope(setting)? {
        ConfigSettingScope::Session => Ok(None),
        ConfigSettingScope::User => {
            let mut config = load_config_file_or_default(&paths.user_config_file())?;
            copy_setting_into_config(&mut config, state, setting)?;
            save_user_config(paths, &config)?;
            Ok(Some(paths.user_config_file()))
        }
        ConfigSettingScope::Workspace => {
            let mut config = load_config_file_or_default(&paths.workspace_config_file())?;
            copy_setting_into_config(&mut config, state, setting)?;
            save_workspace_config(paths, &config)?;
            Ok(Some(paths.workspace_config_file()))
        }
    }
}

/// Persists the full set of user-scoped config settings to the user config file.
pub(crate) fn persist_user_settings(paths: &ConfigPaths, state: &AppState) -> Result<()> {
    let mut config = load_config_file_or_default(&paths.user_config_file())?;
    for descriptor in shared_supported_config_settings()
        .iter()
        .filter(|descriptor| descriptor.scope == ConfigSettingScope::User)
    {
        copy_setting_into_config(&mut config, state, descriptor.canonical_key)?;
    }
    save_user_config(paths, &config)
}

/// Returns the stable label used in user-facing summaries for one setting scope.
pub(crate) fn scope_label(scope: ConfigSettingScope) -> &'static str {
    match scope {
        ConfigSettingScope::User => "user",
        ConfigSettingScope::Workspace => "workspace",
        ConfigSettingScope::Session => "session",
    }
}

fn config_setting_descriptor(setting: &str) -> Result<&'static ConfigSettingSpec> {
    let canonical = normalize_config_key(setting)
        .ok_or_else(|| anyhow!("Unsupported config setting `{}`", setting.trim()))?;
    supported_config_settings()
        .iter()
        .find(|descriptor| descriptor.canonical_key == canonical)
        .ok_or_else(|| anyhow!("Unsupported config setting `{}`", setting.trim()))
}

fn value_to_string_map(value: Value, setting: &str) -> Result<BTreeMap<String, String>> {
    match value {
        Value::Null => Ok(BTreeMap::new()),
        Value::Object(entries) => entries
            .into_iter()
            .map(|(key, value)| match value {
                Value::String(text) => Ok((key, text)),
                _ => bail!("{setting} must be an object with string values"),
            })
            .collect(),
        _ => bail!("{setting} must be an object with string values"),
    }
}

fn load_config_file_or_default(path: &Path) -> Result<PufferConfig> {
    if !path.exists() {
        return Ok(PufferConfig::default());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse config file {}", path.display()))
}

fn copy_setting_into_config(
    target: &mut PufferConfig,
    state: &AppState,
    setting: &str,
) -> Result<()> {
    match config_setting_descriptor(setting)?.canonical_key {
        "theme" => target.theme = state.config.theme.clone(),
        "model" => {
            target.default_provider = state.config.default_provider.clone();
            target.default_model = state.config.default_model.clone();
        }
        "editorMode" => target.editor_mode = state.config.editor_mode.clone(),
        "fastMode" => target.fast_mode = state.config.fast_mode,
        "copy_full_response" => target.copy_full_response = state.config.copy_full_response,
        "autodreamEnabled" => {
            target.memory.autodream_enabled = state.config.memory.autodream_enabled;
        }
        "effortLevel" => target.effort_level = state.config.effort_level.clone(),
        "default_provider" => target.default_provider = state.config.default_provider.clone(),
        "default_model" => target.default_model = state.config.default_model.clone(),
        "openai_base_url" => target.openai_base_url = state.config.openai_base_url.clone(),
        "openai_headers" => target.openai_headers = state.config.openai_headers.clone(),
        "openai_query_params" => {
            target.openai_query_params = state.config.openai_query_params.clone();
        }
        "no_alt_screen" => target.ui.no_alt_screen = state.config.ui.no_alt_screen,
        "tmux_golden_mode" => target.ui.tmux_golden_mode = state.config.ui.tmux_golden_mode,
        "status_line_command" | "status_line_padding" => {
            target.ui.status_line = state.config.ui.status_line.clone();
        }
        "promptColor" | "statuslineEnabled" => {}
        other => bail!("Unsupported config setting `{other}`"),
    }
    Ok(())
}
