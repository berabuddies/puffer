mod settings_catalog;

use anyhow::Context;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const BUILTIN_RESOURCES_DIR_ENV: &str = "PUFFER_BUILTIN_RESOURCES_DIR";

pub use settings_catalog::{
    config_setting_persists_to_workspace_file, config_setting_scope, config_setting_spec,
    normalize_config_setting_key, parse_config_cli_value, supported_config_settings,
    ConfigSettingScope, ConfigSettingSpec, ConfigSettingValueKind,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PufferConfig {
    pub app_name: String,
    pub default_model: Option<String>,
    pub default_provider: Option<String>,
    pub openai_base_url: Option<String>,
    #[serde(default)]
    pub openai_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub openai_query_params: BTreeMap<String, String>,
    pub theme: String,
    #[serde(default = "default_editor_mode", alias = "editorMode")]
    pub editor_mode: String,
    #[serde(default, alias = "fastMode")]
    pub fast_mode: bool,
    #[serde(default, alias = "effortLevel")]
    pub effort_level: Option<String>,
    #[serde(default, alias = "copyFullResponse")]
    pub copy_full_response: bool,
    pub mascot: MascotConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MascotConfig {
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiConfig {
    pub no_alt_screen: bool,
    pub tmux_golden_mode: bool,
    #[serde(default)]
    pub status_line: Option<StatusLineConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusLineConfig {
    pub command: String,
    #[serde(default)]
    pub padding: u16,
}

impl Default for PufferConfig {
    fn default() -> Self {
        Self {
            app_name: "Puffer Code".to_string(),
            default_model: None,
            default_provider: Some("anthropic".to_string()),
            openai_base_url: None,
            openai_headers: BTreeMap::new(),
            openai_query_params: BTreeMap::new(),
            theme: "puffer".to_string(),
            editor_mode: default_editor_mode(),
            fast_mode: false,
            effort_level: None,
            copy_full_response: false,
            mascot: MascotConfig {
                id: "clawd".to_string(),
                display_name: "Clawd".to_string(),
                enabled: true,
            },
            ui: UiConfig {
                no_alt_screen: false,
                tmux_golden_mode: false,
                status_line: None,
            },
        }
    }
}

fn default_editor_mode() -> String {
    "normal".to_string()
}

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub workspace_root: PathBuf,
    pub workspace_config_dir: PathBuf,
    pub user_config_dir: PathBuf,
    pub builtin_resources_dir: PathBuf,
}

impl ConfigPaths {
    /// Discovers the standard Puffer config and resource paths from a workspace root.
    pub fn discover(workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        let workspace_config_dir = workspace_root.join(".puffer");
        let user_config_dir = std::env::var_os("PUFFER_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".puffer");
        let builtin_resources_dir = env::var_os(BUILTIN_RESOURCES_DIR_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root.join("resources"));
        Self {
            workspace_root,
            workspace_config_dir,
            user_config_dir,
            builtin_resources_dir,
        }
    }

    /// Returns the workspace-local configuration file path.
    pub fn workspace_config_file(&self) -> PathBuf {
        self.workspace_config_dir.join("config.toml")
    }

    /// Returns the user-level configuration file path.
    pub fn user_config_file(&self) -> PathBuf {
        self.user_config_dir.join("config.toml")
    }

    /// Returns true when the user-level config file already exists.
    pub fn has_user_config(&self) -> bool {
        self.user_config_file().exists()
    }

    /// Returns true when the workspace-level config file already exists.
    pub fn has_workspace_config(&self) -> bool {
        self.workspace_config_file().exists()
    }
}

/// Loads layered Puffer configuration from the user and workspace config files.
pub fn load_config(paths: &ConfigPaths) -> Result<PufferConfig> {
    let mut config = PufferConfig::default();
    let mut user_selection = None;
    if paths.user_config_file().exists() {
        merge_config_file(&mut config, &paths.user_config_file())?;
        user_selection = Some((
            config.default_provider.clone(),
            config.default_model.clone(),
            config.theme.clone(),
            config.editor_mode.clone(),
            config.fast_mode,
            config.effort_level.clone(),
            config.copy_full_response,
        ));
    }
    if paths.workspace_config_file().exists() {
        merge_config_file(&mut config, &paths.workspace_config_file())?;
    }
    apply_claude_status_line_fallback(&mut config, paths);
    if let Some((
        provider,
        model,
        theme,
        editor_mode,
        fast_mode,
        effort_level,
        copy_full_response,
    )) = user_selection
    {
        config.default_provider = provider;
        config.default_model = model;
        config.theme = theme;
        config.editor_mode = editor_mode;
        config.fast_mode = fast_mode;
        config.effort_level = effort_level;
        config.copy_full_response = copy_full_response;
    }
    Ok(config)
}

/// Saves the user-level Puffer configuration file.
pub fn save_user_config(paths: &ConfigPaths, config: &PufferConfig) -> Result<()> {
    ensure_workspace_dirs(paths)?;
    write_config_file(&paths.user_config_file(), config)
}

/// Saves the workspace-level Puffer configuration file.
pub fn save_workspace_config(paths: &ConfigPaths, config: &PufferConfig) -> Result<()> {
    ensure_workspace_dirs(paths)?;
    write_config_file(&paths.workspace_config_file(), config)
}

/// Ensures the standard user and workspace configuration directories exist.
pub fn ensure_workspace_dirs(paths: &ConfigPaths) -> Result<()> {
    fs::create_dir_all(&paths.workspace_config_dir).with_context(|| {
        format!(
            "failed to create workspace config dir {}",
            paths.workspace_config_dir.display()
        )
    })?;
    fs::create_dir_all(&paths.user_config_dir).with_context(|| {
        format!(
            "failed to create user config dir {}",
            paths.user_config_dir.display()
        )
    })?;
    Ok(())
}

fn merge_config_file(config: &mut PufferConfig, path: &Path) -> Result<()> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    let parsed: PufferConfig = toml::from_str(&raw)
        .with_context(|| format!("failed to parse config file {}", path.display()))?;
    *config = parsed;
    Ok(())
}

fn apply_claude_status_line_fallback(config: &mut PufferConfig, paths: &ConfigPaths) {
    if config.ui.status_line.is_some() {
        return;
    }
    if let Some(status_line) = load_claude_status_line(paths) {
        config.ui.status_line = Some(status_line);
    }
}

fn load_claude_status_line(paths: &ConfigPaths) -> Option<StatusLineConfig> {
    let raw = fs::read_to_string(claude_user_settings_file(paths)).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    let status_line = parsed.get("statusLine")?;
    if status_line
        .get("type")
        .and_then(Value::as_str)
        .filter(|kind| *kind == "command")
        .is_none()
    {
        return None;
    }
    let command = status_line.get("command").and_then(Value::as_str)?.trim();
    if command.is_empty() {
        return None;
    }
    Some(StatusLineConfig {
        command: command.to_string(),
        padding: 0,
    })
}

fn claude_user_settings_file(paths: &ConfigPaths) -> PathBuf {
    let home = paths
        .user_config_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| paths.user_config_dir.clone());
    home.join(".claude").join("settings.json")
}

fn write_config_file(path: &Path, config: &PufferConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config parent dir {}", parent.display()))?;
    }
    let raw = toml::to_string_pretty(config)
        .with_context(|| format!("failed to serialize config file {}", path.display()))?;
    fs::write(path, raw).with_context(|| format!("failed to write config file {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tempfile::tempdir;

    fn puffer_home_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn lock_puffer_home() -> MutexGuard<'static, ()> {
        puffer_home_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct ScopedPufferHome {
        old_home: Option<OsString>,
    }

    impl ScopedPufferHome {
        fn set(path: &Path) -> Self {
            let old_home = std::env::var_os("PUFFER_HOME");
            std::env::set_var("PUFFER_HOME", path);
            Self { old_home }
        }
    }

    impl Drop for ScopedPufferHome {
        fn drop(&mut self) {
            if let Some(value) = self.old_home.take() {
                std::env::set_var("PUFFER_HOME", value);
            } else {
                std::env::remove_var("PUFFER_HOME");
            }
        }
    }

    struct ScopedBuiltinResourcesDir {
        old_value: Option<OsString>,
    }

    impl ScopedBuiltinResourcesDir {
        fn set(path: &Path) -> Self {
            let old_value = std::env::var_os(BUILTIN_RESOURCES_DIR_ENV);
            std::env::set_var(BUILTIN_RESOURCES_DIR_ENV, path);
            Self { old_value }
        }
    }

    impl Drop for ScopedBuiltinResourcesDir {
        fn drop(&mut self) {
            if let Some(value) = self.old_value.take() {
                std::env::set_var(BUILTIN_RESOURCES_DIR_ENV, value);
            } else {
                std::env::remove_var(BUILTIN_RESOURCES_DIR_ENV);
            }
        }
    }

    #[test]
    fn load_config_preserves_user_provider_selection_over_workspace_defaults() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let home = tempdir.path().join("home");
        let workspace = tempdir.path().join("workspace");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&workspace).expect("workspace");
        let _home = ScopedPufferHome::set(&home);

        let paths = ConfigPaths::discover(&workspace);
        ensure_workspace_dirs(&paths).expect("dirs");

        let mut user = PufferConfig::default();
        user.default_provider = Some("openai".to_string());
        user.default_model = Some("openai/gpt-5".to_string());
        user.openai_base_url = Some("https://proxy.example/v1".to_string());
        user.openai_headers = BTreeMap::from([("x-openai-test".to_string(), "user".to_string())]);
        user.openai_query_params = BTreeMap::from([("user_param".to_string(), "1".to_string())]);
        user.theme = "sunrise".to_string();
        user.editor_mode = "vim".to_string();
        user.fast_mode = true;
        user.effort_level = Some("high".to_string());
        save_user_config(&paths, &user).expect("user config");

        let mut workspace = PufferConfig::default();
        workspace.default_provider = Some("anthropic".to_string());
        workspace.default_model = Some("anthropic/claude-sonnet-4-5".to_string());
        workspace.openai_headers =
            BTreeMap::from([("x-openai-test".to_string(), "workspace".to_string())]);
        workspace.openai_query_params =
            BTreeMap::from([("workspace_param".to_string(), "2".to_string())]);
        workspace.theme = "harbor".to_string();
        save_workspace_config(&paths, &workspace).expect("workspace config");

        let loaded = load_config(&paths).expect("load");
        assert_eq!(loaded.default_provider.as_deref(), Some("openai"));
        assert_eq!(loaded.default_model.as_deref(), Some("openai/gpt-5"));
        assert_eq!(loaded.openai_base_url, None);
        assert_eq!(
            loaded
                .openai_headers
                .get("x-openai-test")
                .map(String::as_str),
            Some("workspace")
        );
        assert_eq!(
            loaded
                .openai_query_params
                .get("workspace_param")
                .map(String::as_str),
            Some("2")
        );
        assert_eq!(loaded.theme, "sunrise");
        assert_eq!(loaded.editor_mode, "vim");
        assert!(loaded.fast_mode);
        assert_eq!(loaded.effort_level.as_deref(), Some("high"));
    }

    #[test]
    fn load_config_preserves_cleared_user_selection() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let home = tempdir.path().join("home");
        let workspace = tempdir.path().join("workspace");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&workspace).expect("workspace");
        let _home = ScopedPufferHome::set(&home);

        let paths = ConfigPaths::discover(&workspace);
        ensure_workspace_dirs(&paths).expect("dirs");

        let mut user = PufferConfig::default();
        user.default_provider = None;
        user.default_model = None;
        save_user_config(&paths, &user).expect("user config");

        let mut workspace = PufferConfig::default();
        workspace.default_provider = Some("anthropic".to_string());
        workspace.default_model = Some("anthropic/claude-sonnet-4-5".to_string());
        save_workspace_config(&paths, &workspace).expect("workspace config");

        let loaded = load_config(&paths).expect("load");
        assert_eq!(loaded.default_provider, None);
        assert_eq!(loaded.default_model, None);
    }

    #[test]
    fn load_config_allows_workspace_to_override_user_openai_base_url() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let home = tempdir.path().join("home");
        let workspace = tempdir.path().join("workspace");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&workspace).expect("workspace");
        let _home = ScopedPufferHome::set(&home);

        let paths = ConfigPaths::discover(&workspace);
        ensure_workspace_dirs(&paths).expect("dirs");

        let mut user = PufferConfig::default();
        user.openai_base_url = Some("https://user.example/v1".to_string());
        save_user_config(&paths, &user).expect("user config");

        let mut workspace = PufferConfig::default();
        workspace.openai_base_url = Some("https://workspace.example/v1".to_string());
        save_workspace_config(&paths, &workspace).expect("workspace config");

        let loaded = load_config(&paths).expect("load");
        assert_eq!(
            loaded.openai_base_url.as_deref(),
            Some("https://workspace.example/v1")
        );
    }

    #[test]
    fn load_config_reads_claude_statusline_when_puffer_config_is_unset() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let home = tempdir.path().join("home");
        let workspace = tempdir.path().join("workspace");
        fs::create_dir_all(home.join(".claude")).expect("claude dir");
        fs::create_dir_all(&workspace).expect("workspace");
        let _home = ScopedPufferHome::set(&home);

        fs::write(
            home.join(".claude/settings.json"),
            r#"{
  "statusLine": {
    "type": "command",
    "command": "printf claude-status"
  }
}"#,
        )
        .expect("claude settings");

        let paths = ConfigPaths::discover(&workspace);
        let loaded = load_config(&paths).expect("load");

        assert_eq!(
            loaded
                .ui
                .status_line
                .as_ref()
                .map(|status_line| status_line.command.as_str()),
            Some("printf claude-status")
        );
        assert_eq!(
            loaded
                .ui
                .status_line
                .as_ref()
                .map(|status_line| status_line.padding),
            Some(0)
        );
    }

    #[test]
    fn load_config_reads_fast_mode_and_effort_aliases() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let home = tempdir.path().join("home");
        let workspace = tempdir.path().join("workspace");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&workspace).expect("workspace");
        let _home = ScopedPufferHome::set(&home);

        let paths = ConfigPaths::discover(&workspace);
        ensure_workspace_dirs(&paths).expect("dirs");
        fs::write(
            paths.user_config_file(),
            r#"
app_name = "Puffer Code"
default_provider = "openai"
theme = "puffer"
fastMode = true
effortLevel = "xhigh"

[mascot]
id = "clawd"
display_name = "Clawd"
enabled = true

[ui]
no_alt_screen = false
tmux_golden_mode = false
"#,
        )
        .expect("user config");

        let loaded = load_config(&paths).expect("load");
        assert!(loaded.fast_mode);
        assert_eq!(loaded.effort_level.as_deref(), Some("xhigh"));
    }

    #[test]
    fn load_config_prefers_explicit_puffer_statusline_over_claude_settings() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let home = tempdir.path().join("home");
        let workspace = tempdir.path().join("workspace");
        fs::create_dir_all(home.join(".claude")).expect("claude dir");
        fs::create_dir_all(&workspace).expect("workspace");
        let _home = ScopedPufferHome::set(&home);

        fs::write(
            home.join(".claude/settings.json"),
            r#"{
  "statusLine": {
    "type": "command",
    "command": "printf claude-status"
  }
}"#,
        )
        .expect("claude settings");

        let paths = ConfigPaths::discover(&workspace);
        let mut workspace_config = PufferConfig::default();
        workspace_config.ui.status_line = Some(StatusLineConfig {
            command: "printf puffer-status".to_string(),
            padding: 2,
        });
        save_workspace_config(&paths, &workspace_config).expect("workspace config");

        let loaded = load_config(&paths).expect("load");
        assert_eq!(
            loaded
                .ui
                .status_line
                .as_ref()
                .map(|status_line| status_line.command.as_str()),
            Some("printf puffer-status")
        );
        assert_eq!(
            loaded
                .ui
                .status_line
                .as_ref()
                .map(|status_line| status_line.padding),
            Some(2)
        );
    }

    #[test]
    fn discover_honors_builtin_resources_override() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let workspace = tempdir.path().join("workspace");
        let override_dir = tempdir.path().join("bundled-resources");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&override_dir).expect("override");
        let _override = ScopedBuiltinResourcesDir::set(&override_dir);

        let paths = ConfigPaths::discover(&workspace);
        assert_eq!(paths.builtin_resources_dir, override_dir);
    }
}
