//! Browser extension launch settings derived from Puffer config.

use anyhow::Result;
use puffer_config::{builtin_captcha_solvers, ConfigPaths, PufferConfig};
use puffer_secrets::SecretVault;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Effective browser extension state used when starting a browser root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BrowserLaunchSettings {
    extension_dirs: Vec<PathBuf>,
    seeds: Vec<BrowserExtensionSeed>,
}

/// Local-storage values that should be injected into a bundled extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BrowserExtensionSeed {
    solver_id: String,
    api_key: String,
    base_url: String,
}

impl BrowserLaunchSettings {
    /// Builds launch settings from the currently loaded daemon config.
    pub(crate) fn from_config(paths: &ConfigPaths, config: &PufferConfig) -> Result<Self> {
        let browser = &config.browser;
        if !browser.extensions_enabled {
            return Ok(Self::default());
        }

        let mut extension_dirs = Vec::new();
        for extension in browser
            .extensions
            .iter()
            .filter(|extension| extension.enabled)
        {
            push_extension_dir(&mut extension_dirs, PathBuf::from(&extension.path));
        }

        let mut seeds = Vec::new();
        if browser.captcha.enabled {
            if let Some(solver) = builtin_captcha_solvers()
                .iter()
                .find(|solver| solver.id == browser.captcha.selected_solver)
            {
                let configured = browser.captcha.solvers.get(solver.id);
                push_extension_dir(
                    &mut extension_dirs,
                    paths.builtin_resources_dir.join(solver.extension_path),
                );
                if let Some(secret_id) = configured.and_then(|item| item.api_key_secret_id.as_ref())
                {
                    if let Some(api_key) = reveal_secret_value(paths, secret_id) {
                        let base_url = configured
                            .and_then(|item| item.base_url.clone())
                            .unwrap_or_else(|| solver.default_base_url.to_string());
                        seeds.push(BrowserExtensionSeed {
                            solver_id: solver.id.to_string(),
                            api_key,
                            base_url,
                        });
                    }
                }
            }
        }

        dedupe_extension_dirs(&mut extension_dirs);
        Ok(Self {
            extension_dirs,
            seeds,
        })
    }

    /// Returns unpacked extension directories that should be loaded by Chrome.
    pub(crate) fn extension_dirs(&self) -> &[PathBuf] {
        &self.extension_dirs
    }

    /// Returns extension local-storage seed values for bundled captcha solvers.
    pub(crate) fn seeds(&self) -> &[BrowserExtensionSeed] {
        &self.seeds
    }

    /// Creates launch settings with extension directories for command-line tests.
    #[cfg(test)]
    pub(super) fn with_extension_dirs(extension_dirs: Vec<PathBuf>) -> Self {
        Self {
            extension_dirs,
            seeds: Vec::new(),
        }
    }
}

impl BrowserExtensionSeed {
    /// Returns the built-in captcha solver id this seed targets.
    pub(crate) fn solver_id(&self) -> &str {
        &self.solver_id
    }

    /// Returns the decrypted API key for the extension.
    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Returns the base URL configured for the extension.
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }
}

fn push_extension_dir(extension_dirs: &mut Vec<PathBuf>, path: PathBuf) {
    if extension_manifest_present(&path) {
        extension_dirs.push(path);
    }
}

fn extension_manifest_present(path: &Path) -> bool {
    path.join("manifest.json").is_file()
}

fn reveal_secret_value(paths: &ConfigPaths, secret_id: &str) -> Option<String> {
    let store_path = SecretVault::default_path(&paths.user_config_dir);
    let vault = SecretVault::open(store_path).ok()?;
    match vault.reveal(secret_id) {
        Ok(secret) => Some(secret.value),
        Err(error) => {
            eprintln!(
                "puffer browser: captcha API key `{secret_id}` could not be revealed: {error}"
            );
            None
        }
    }
}

fn dedupe_extension_dirs(extension_dirs: &mut Vec<PathBuf>) {
    let mut seen = BTreeSet::new();
    extension_dirs.retain(|path| seen.insert(path.clone()));
}
