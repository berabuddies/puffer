pub mod env_vars;
mod settings_catalog;

use anyhow::Context;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
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
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub recap: RecapConfig,
    pub mascot: MascotConfig,
    pub ui: UiConfig,
    /// When set, the runtime constructs a remote `RemoteToolRunner` against
    /// this endpoint instead of using the in-process `LocalToolRunner`. The
    /// actual swap happens at `AppState` construction time in the binary;
    /// this struct is only the on-disk representation.
    #[serde(default)]
    pub remote_runner: Option<RemoteRunnerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteRunnerConfig {
    /// gRPC endpoint, e.g. `http://127.0.0.1:50051`.
    pub endpoint: String,
    /// Inline bearer token. Mutually exclusive with `auth_token_env`; if
    /// both are set, `auth_token` wins.
    #[serde(default)]
    pub auth_token: Option<String>,
    /// Environment variable to read the bearer token from. Lets the config
    /// file stay free of secrets.
    #[serde(default)]
    pub auth_token_env: Option<String>,
    /// Initial delay (ms) between startup `Ping` retries. Defaults to
    /// 1000 ms when unset.
    #[serde(default)]
    pub initial_backoff_ms: Option<u64>,
    /// Cap on the per-attempt backoff (ms). Defaults to 10_000 ms when
    /// unset.
    #[serde(default)]
    pub max_backoff_ms: Option<u64>,
    /// Whether runtime construction should block until the remote runner
    /// answers Ping. Defaults to true for interactive safety; managed
    /// daemon sessions can disable this so tool-runner startup does not
    /// block first-token latency when no tool is needed.
    #[serde(default = "default_remote_runner_wait_for_ready")]
    pub wait_for_ready: bool,
}

impl RemoteRunnerConfig {
    /// Resolves the effective bearer token by consulting the env var when
    /// `auth_token` is unset.
    pub fn resolve_auth_token(&self) -> Option<String> {
        if let Some(direct) = &self.auth_token {
            return Some(direct.clone());
        }
        self.auth_token_env
            .as_deref()
            .and_then(|name| std::env::var(name).ok())
    }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryConfig {
    #[serde(default = "default_memory_enabled")]
    pub enabled: bool,
    #[serde(default = "default_memory_char_limit")]
    pub char_limit: usize,
    #[serde(default = "default_review_nudge_interval")]
    pub review_nudge_interval: usize,
    #[serde(default = "default_flush_min_turns")]
    pub flush_min_turns: usize,
    #[serde(default = "default_background_review")]
    pub background_review: bool,
    #[serde(default = "default_flush_on_compact")]
    pub flush_on_compact: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: default_memory_enabled(),
            char_limit: default_memory_char_limit(),
            review_nudge_interval: default_review_nudge_interval(),
            flush_min_turns: default_flush_min_turns(),
            background_review: default_background_review(),
            flush_on_compact: default_flush_on_compact(),
        }
    }
}

/// Session recap (away-summary) configuration. Mirrors Anthropic's
/// claude-code `/recap` semantics: a short, model-generated one-liner that
/// surfaces in the UI when the user comes back after stepping away.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecapConfig {
    /// Master switch. When false, both `/recap` slash command and auto
    /// trigger are no-ops.
    #[serde(default = "default_recap_enabled")]
    pub enabled: bool,
    /// Whether auto-trigger (TUI idle timer / GUI window blur) fires
    /// recaps. The slash command is unaffected. Default true.
    #[serde(default = "default_recap_auto")]
    pub auto: bool,
    /// Minimum idle seconds before the auto-trigger fires. Matches
    /// claude-code's `on8 = 180_000` (3 min) default.
    #[serde(default = "default_recap_idle_secs")]
    pub idle_secs: u64,
    /// Minimum number of real user messages before any recap can fire.
    /// Matches claude-code's `Ls3 = 3`.
    #[serde(default = "default_recap_min_user_messages")]
    pub min_user_messages: usize,
    /// Minimum number of new user messages required between successive
    /// auto-recaps (cooldown). Matches claude-code's `ks3 = 2`.
    #[serde(default = "default_recap_cooldown_messages")]
    pub cooldown_messages: usize,
}

impl Default for RecapConfig {
    fn default() -> Self {
        Self {
            enabled: default_recap_enabled(),
            auto: default_recap_auto(),
            idle_secs: default_recap_idle_secs(),
            min_user_messages: default_recap_min_user_messages(),
            cooldown_messages: default_recap_cooldown_messages(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProjectRegistry {
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEntry {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProjectMemory {
    pub name: String,
    pub root: PathBuf,
    pub memory_file: PathBuf,
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
            memory: MemoryConfig::default(),
            recap: RecapConfig::default(),
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
            remote_runner: None,
        }
    }
}

fn default_editor_mode() -> String {
    "normal".to_string()
}

fn default_remote_runner_wait_for_ready() -> bool {
    true
}

fn default_memory_enabled() -> bool {
    true
}

fn default_memory_char_limit() -> usize {
    6_000
}

fn default_review_nudge_interval() -> usize {
    8
}

fn default_flush_min_turns() -> usize {
    6
}

fn default_background_review() -> bool {
    true
}

fn default_flush_on_compact() -> bool {
    true
}

fn default_recap_enabled() -> bool {
    true
}

fn default_recap_auto() -> bool {
    true
}

fn default_recap_idle_secs() -> u64 {
    180
}

fn default_recap_min_user_messages() -> usize {
    3
}

fn default_recap_cooldown_messages() -> usize {
    2
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

    /// Returns the user-level project registry file path.
    pub fn projects_file(&self) -> PathBuf {
        self.user_config_dir.join("projects.toml")
    }

    /// Returns the directory that stores per-project memory files.
    pub fn projects_memory_dir(&self) -> PathBuf {
        self.user_config_dir.join("projects")
    }
}

/// Loads the user-level project registry stored in `~/.puffer/projects.toml`.
pub fn load_project_registry(paths: &ConfigPaths) -> Result<ProjectRegistry> {
    let path = paths.projects_file();
    if !path.exists() {
        return Ok(ProjectRegistry::default());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read project registry {}", path.display()))?;
    toml::from_str(&raw)
        .with_context(|| format!("failed to parse project registry {}", path.display()))
}

/// Resolves the current working directory to a configured project memory file.
pub fn resolve_project_memory(
    paths: &ConfigPaths,
    cwd: &Path,
) -> Result<Option<ResolvedProjectMemory>> {
    let registry = load_project_registry(paths)?;
    let normalized_cwd = normalize_project_path(cwd);
    let mut best_match: Option<(usize, &ProjectEntry, PathBuf)> = None;

    for project in &registry.projects {
        let normalized_root = normalize_project_path(&project.path);
        if !path_matches_root(&normalized_cwd, &normalized_root) {
            continue;
        }
        let score = normalized_root.components().count();
        match &best_match {
            Some((best_score, _, _)) if *best_score >= score => continue,
            _ => best_match = Some((score, project, normalized_root)),
        }
    }

    Ok(
        best_match.map(|(_, project, normalized_root)| ResolvedProjectMemory {
            name: project.name.clone(),
            root: normalized_root.clone(),
            memory_file: paths
                .projects_memory_dir()
                .join(project_storage_slug(&project.name, &normalized_root))
                .join("MEMORY.md"),
        }),
    )
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

fn normalize_project_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn path_matches_root(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn project_storage_slug(name: &str, root: &Path) -> String {
    let mut slug = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    slug = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        slug = "project".to_string();
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    format!("{slug}-{:016x}", hasher.finish())
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

    #[test]
    fn remote_runner_wait_for_ready_defaults_to_true() {
        let mut config = PufferConfig::default();
        config.remote_runner = Some(RemoteRunnerConfig {
            endpoint: "http://127.0.0.1:50051".to_string(),
            auth_token: None,
            auth_token_env: None,
            initial_backoff_ms: None,
            max_backoff_ms: None,
            wait_for_ready: default_remote_runner_wait_for_ready(),
        });
        let raw = toml::to_string(&config).expect("serialize");
        let config: PufferConfig = toml::from_str(&raw).expect("config");

        assert!(config.remote_runner.expect("remote runner").wait_for_ready);
    }

    #[test]
    fn remote_runner_wait_for_ready_can_be_disabled() {
        let mut config = PufferConfig::default();
        config.remote_runner = Some(RemoteRunnerConfig {
            endpoint: "http://127.0.0.1:50051".to_string(),
            auth_token: None,
            auth_token_env: None,
            initial_backoff_ms: None,
            max_backoff_ms: None,
            wait_for_ready: false,
        });
        let raw = toml::to_string(&config).expect("serialize");
        let config: PufferConfig = toml::from_str(&raw).expect("config");

        assert!(!config.remote_runner.expect("remote runner").wait_for_ready);
    }

    #[test]
    fn resolve_project_memory_uses_registered_project_path() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let home = tempdir.path().join("home");
        let workspace = tempdir.path().join("workspace");
        let project_root = workspace.join("apps/api");
        let nested = project_root.join("src/features");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&nested).expect("nested");
        let _home = ScopedPufferHome::set(&home);

        let paths = ConfigPaths::discover(&workspace);
        ensure_workspace_dirs(&paths).expect("dirs");
        fs::write(
            paths.projects_file(),
            format!(
                "[[projects]]\nname = \"api\"\npath = \"{}\"\n",
                project_root.display()
            ),
        )
        .expect("projects file");

        let resolved = resolve_project_memory(&paths, &nested)
            .expect("resolve")
            .expect("project memory");
        assert_eq!(resolved.name, "api");
        assert_eq!(resolved.root, project_root);
        assert!(resolved.memory_file.ends_with("MEMORY.md"));
        assert!(resolved
            .memory_file
            .starts_with(paths.projects_memory_dir()));
    }

    #[test]
    fn resolve_project_memory_prefers_longest_matching_root() {
        let _guard = lock_puffer_home();
        let tempdir = tempdir().expect("tempdir");
        let home = tempdir.path().join("home");
        let workspace = tempdir.path().join("workspace");
        let mono_root = workspace.join("monorepo");
        let nested_root = mono_root.join("services/api");
        let cwd = nested_root.join("src");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&cwd).expect("cwd");
        let _home = ScopedPufferHome::set(&home);

        let paths = ConfigPaths::discover(&workspace);
        ensure_workspace_dirs(&paths).expect("dirs");
        fs::write(
            paths.projects_file(),
            format!(
                "[[projects]]\nname = \"mono\"\npath = \"{}\"\n\n[[projects]]\nname = \"api\"\npath = \"{}\"\n",
                mono_root.display(),
                nested_root.display()
            ),
        )
        .expect("projects file");

        let resolved = resolve_project_memory(&paths, &cwd)
            .expect("resolve")
            .expect("project memory");
        assert_eq!(resolved.name, "api");
        assert_eq!(resolved.root, nested_root);
    }
}
