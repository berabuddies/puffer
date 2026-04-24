//! Wiring that launches platform-specific connectors (Telegram, Slack,
//! …) alongside a running Puffer process. See
//! [`puffer-connector-core`] for the underlying trait and bridge.
//!
//! Each platform connector is compiled in behind its own feature flag
//! (`connector-<name>`). The `serve` subcommand iterates the parsed
//! `connectors.toml` and starts one handle per enabled, compiled-in
//! platform.

use anyhow::{anyhow, Context, Result};
use puffer_config::{ConfigPaths, PufferConfig};
use puffer_connector_core::{
    Connector, ConnectorHandle, ConnectorRuntime, ConnectorRuntimeConfig, ConnectorStartError,
    ConnectorsConfig, ConversationSessionMap,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Loads the connector config file using the standard Puffer search
/// order: workspace override (`.puffer/connectors.toml`), then
/// user-level (`~/.puffer/connectors.toml`). Returns the default
/// (enabled, empty) config when no file is found.
pub(crate) fn load_connectors_config(paths: &ConfigPaths) -> Result<ConnectorsConfig> {
    let workspace = paths.workspace_config_dir.join("connectors.toml");
    let user = paths.user_config_dir.join("connectors.toml");

    if workspace.exists() {
        return ConnectorsConfig::load(&workspace);
    }
    if user.exists() {
        return ConnectorsConfig::load(&user);
    }
    Ok(ConnectorsConfig::default())
}

/// Returns the on-disk path used for the persistent conversation →
/// session map.
pub(crate) fn session_map_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("connector_sessions.json")
}

/// Owns the handles for every running connector. Dropping this (or
/// calling [`RunningConnectors::shutdown`]) fires the graceful shutdown
/// signal for each and joins the background threads.
pub(crate) struct RunningConnectors {
    handles: Vec<ConnectorHandle>,
}

impl RunningConnectors {
    pub(crate) fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    pub(crate) fn ids(&self) -> Vec<String> {
        self.handles.iter().map(|h| h.id.clone()).collect()
    }

    /// Signals each connector to shut down, then joins every background
    /// thread. Collected errors are returned; the shutdown is
    /// best-effort (we always attempt every handle even if one fails).
    pub(crate) fn shutdown(self) -> Vec<(String, anyhow::Error)> {
        let mut errors = Vec::new();
        for handle in self.handles {
            let ConnectorHandle {
                id,
                shutdown,
                join,
            } = handle;
            (shutdown)();
            match join.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => errors.push((id, error)),
                Err(_) => errors.push((id.clone(), anyhow!("connector `{id}` thread panicked"))),
            }
        }
        errors
    }
}

/// Builds a [`ConnectorRuntime`] with a snapshot of the current Puffer
/// state (config / resources / providers / auth / session store).
///
/// The runtime owns **clones** of everything it needs. Connectors are
/// self-contained once started; mutations inside the runtime (new
/// sessions, refreshed auth) write through to disk but do not propagate
/// back to any concurrently running TUI.
pub(crate) fn build_runtime(
    config: PufferConfig,
    resources: LoadedResources,
    providers: ProviderRegistry,
    auth_store: AuthStore,
    auth_path: PathBuf,
    session_store: SessionStore,
    default_cwd: PathBuf,
    map_path: &Path,
) -> Result<Arc<ConnectorRuntime>> {
    let session_map = ConversationSessionMap::load(map_path)
        .with_context(|| format!("failed to load connector session map {}", map_path.display()))?;
    Ok(Arc::new(ConnectorRuntime::new(ConnectorRuntimeConfig {
        config,
        resources,
        providers,
        auth_store,
        auth_path,
        session_store,
        session_map,
        default_cwd,
    })))
}

/// Starts every connector enabled in `config`. Unknown or disabled
/// connectors are skipped (with a diagnostic on stderr). Platform
/// crates whose feature is not compiled in are also skipped; we never
/// fail the whole startup because of a single missing feature.
pub(crate) fn start_configured_connectors(
    runtime: Arc<ConnectorRuntime>,
    config: &ConnectorsConfig,
) -> Result<RunningConnectors> {
    if !config.enabled {
        return Ok(RunningConnectors { handles: vec![] });
    }
    let mut handles = Vec::new();
    for (platform, entry) in &config.platforms {
        if !entry.enabled {
            continue;
        }
        match build_connector(platform, entry) {
            BuildOutcome::Built(connector) => {
                match connector.start(runtime.clone()) {
                    Ok(handle) => handles.push(handle),
                    Err(error) => eprintln!(
                        "connector `{platform}` failed to start: {}",
                        describe_start_error(&error)
                    ),
                }
            }
            BuildOutcome::ConfigError(error) => {
                eprintln!("connector `{platform}` config invalid: {error:#}");
            }
            BuildOutcome::NotCompiledIn => {
                eprintln!(
                    "connector `{platform}` is configured but not compiled in; \
                     enable the `connector-{platform}` feature on puffer-cli"
                );
            }
            BuildOutcome::Unknown => {
                eprintln!(
                    "connector `{platform}` is not recognized; \
                     known platforms: telegram, discord, slack, matrix, email, webhook"
                );
            }
        }
    }
    Ok(RunningConnectors { handles })
}

fn describe_start_error(error: &ConnectorStartError) -> String {
    match error {
        ConnectorStartError::Disabled { id } => format!("disabled (`{id}`)"),
        ConnectorStartError::MissingConfig { id: _, detail } => format!("missing config: {detail}"),
        ConnectorStartError::Other { source, .. } => format!("{source:#}"),
    }
}

#[allow(dead_code)]
enum BuildOutcome {
    // These two arms are only constructed when at least one
    // `connector-*` feature is enabled; silence the warning when
    // `puffer-cli` is built with no connector features.
    Built(Box<dyn Connector>),
    ConfigError(anyhow::Error),
    NotCompiledIn,
    Unknown,
}

fn build_connector(
    platform: &str,
    entry: &puffer_connector_core::ConnectorConfig,
) -> BuildOutcome {
    match platform {
        "telegram" => {
            #[cfg(feature = "connector-telegram")]
            {
                match entry.parse::<puffer_connector_telegram::TelegramConfig>() {
                    Ok(cfg) => BuildOutcome::Built(Box::new(
                        puffer_connector_telegram::TelegramConnector::new(cfg),
                    )),
                    Err(error) => BuildOutcome::ConfigError(error),
                }
            }
            #[cfg(not(feature = "connector-telegram"))]
            {
                let _ = entry;
                BuildOutcome::NotCompiledIn
            }
        }
        "discord" => {
            #[cfg(feature = "connector-discord")]
            {
                match entry.parse::<puffer_connector_discord::DiscordConfig>() {
                    Ok(cfg) => BuildOutcome::Built(Box::new(
                        puffer_connector_discord::DiscordConnector::new(cfg),
                    )),
                    Err(error) => BuildOutcome::ConfigError(error),
                }
            }
            #[cfg(not(feature = "connector-discord"))]
            {
                let _ = entry;
                BuildOutcome::NotCompiledIn
            }
        }
        "slack" => {
            #[cfg(feature = "connector-slack")]
            {
                match entry.parse::<puffer_connector_slack::SlackConfig>() {
                    Ok(cfg) => BuildOutcome::Built(Box::new(
                        puffer_connector_slack::SlackConnector::new(cfg),
                    )),
                    Err(error) => BuildOutcome::ConfigError(error),
                }
            }
            #[cfg(not(feature = "connector-slack"))]
            {
                let _ = entry;
                BuildOutcome::NotCompiledIn
            }
        }
        "matrix" => {
            #[cfg(feature = "connector-matrix")]
            {
                match entry.parse::<puffer_connector_matrix::MatrixConfig>() {
                    Ok(cfg) => BuildOutcome::Built(Box::new(
                        puffer_connector_matrix::MatrixConnector::new(cfg),
                    )),
                    Err(error) => BuildOutcome::ConfigError(error),
                }
            }
            #[cfg(not(feature = "connector-matrix"))]
            {
                let _ = entry;
                BuildOutcome::NotCompiledIn
            }
        }
        "email" => {
            #[cfg(feature = "connector-email")]
            {
                match entry.parse::<puffer_connector_email::EmailConfig>() {
                    Ok(cfg) => BuildOutcome::Built(Box::new(
                        puffer_connector_email::EmailConnector::new(cfg),
                    )),
                    Err(error) => BuildOutcome::ConfigError(error),
                }
            }
            #[cfg(not(feature = "connector-email"))]
            {
                let _ = entry;
                BuildOutcome::NotCompiledIn
            }
        }
        "webhook" => {
            #[cfg(feature = "connector-webhook")]
            {
                match entry.parse::<puffer_connector_webhook::WebhookConfig>() {
                    Ok(cfg) => BuildOutcome::Built(Box::new(
                        puffer_connector_webhook::WebhookConnector::new(cfg),
                    )),
                    Err(error) => BuildOutcome::ConfigError(error),
                }
            }
            #[cfg(not(feature = "connector-webhook"))]
            {
                let _ = entry;
                BuildOutcome::NotCompiledIn
            }
        }
        _ => BuildOutcome::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_connector_core::ConnectorConfig;

    #[test]
    fn unknown_platform_reports_unknown() {
        let entry = ConnectorConfig::default();
        let outcome = build_connector("nonsense", &entry);
        assert!(matches!(outcome, BuildOutcome::Unknown));
    }

    #[test]
    fn start_returns_empty_when_config_is_disabled() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let session_store_root = ConfigPaths {
            workspace_root: dir.path().to_path_buf(),
            workspace_config_dir: dir.path().join(".puffer"),
            user_config_dir: dir.path().join(".puffer-user"),
            builtin_resources_dir: dir.path().join("resources"),
        };
        std::fs::create_dir_all(&session_store_root.user_config_dir).unwrap();
        let runtime = build_runtime(
            PufferConfig::default(),
            LoadedResources::default(),
            ProviderRegistry::default(),
            AuthStore::default(),
            dir.path().join("auth.json"),
            SessionStore::from_paths(&session_store_root).unwrap(),
            dir.path().to_path_buf(),
            &session_map_path(&session_store_root),
        )
        .unwrap();

        let mut config = ConnectorsConfig::default();
        config.enabled = false;
        let running = start_configured_connectors(runtime, &config).unwrap();
        assert!(running.is_empty());
    }

    #[test]
    fn load_config_returns_default_when_files_are_absent() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let paths = ConfigPaths {
            workspace_root: dir.path().to_path_buf(),
            workspace_config_dir: dir.path().join(".puffer"),
            user_config_dir: dir.path().join(".puffer-user"),
            builtin_resources_dir: dir.path().join("resources"),
        };
        let config = load_connectors_config(&paths).unwrap();
        assert!(config.enabled);
        assert!(config.platforms.is_empty());
    }

    #[test]
    fn load_config_prefers_workspace_over_user() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let paths = ConfigPaths {
            workspace_root: dir.path().to_path_buf(),
            workspace_config_dir: dir.path().join(".puffer"),
            user_config_dir: dir.path().join(".puffer-user"),
            builtin_resources_dir: dir.path().join("resources"),
        };
        std::fs::create_dir_all(&paths.workspace_config_dir).unwrap();
        std::fs::create_dir_all(&paths.user_config_dir).unwrap();
        std::fs::write(
            paths.workspace_config_dir.join("connectors.toml"),
            "[telegram]\ntoken = \"workspace\"\n",
        )
        .unwrap();
        std::fs::write(
            paths.user_config_dir.join("connectors.toml"),
            "[discord]\ntoken = \"user\"\n",
        )
        .unwrap();

        let config = load_connectors_config(&paths).unwrap();
        assert!(config.platforms.contains_key("telegram"));
        assert!(!config.platforms.contains_key("discord"));
    }
}
