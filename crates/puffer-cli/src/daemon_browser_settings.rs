use anyhow::{Context, Result};
use puffer_config::{
    is_builtin_solver, save_user_config, BrowserConfig, BrowserExtensionConfig, CaptchaConfig,
    CaptchaSolverConfig,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use url::Url;

use crate::daemon::DaemonState;
use crate::desktop_api_types::{
    SaveBrowserCaptchaSolverParams, SaveBrowserExtensionParams, SaveBrowserSettingsParams,
};

/// Persists browser extension and CAPTCHA solver settings from the desktop UI.
pub(crate) fn handle_save_browser_settings(state: &DaemonState, params: &Value) -> Result<Value> {
    let input: SaveBrowserSettingsParams =
        serde_json::from_value(params.clone()).context("invalid browser settings")?;
    let mut config = state.config_snapshot();
    config.browser = browser_config_from_input(input)?;
    let launch_settings =
        crate::daemon_browser::BrowserLaunchSettings::from_config(state.config_paths(), &config)?;
    save_user_config(state.config_paths(), &config).context("save user config")?;
    state.replace_config(config);
    state.browsers.update_launch_settings(launch_settings)?;
    state.settings_snapshot_value()
}

fn browser_config_from_input(input: SaveBrowserSettingsParams) -> Result<BrowserConfig> {
    if !is_builtin_solver(&input.captcha.selected_solver) {
        anyhow::bail!("unknown captcha solver `{}`", input.captcha.selected_solver);
    }
    let selected_solver = input.captcha.selected_solver;
    let mut browser = BrowserConfig {
        extensions_enabled: input.extensions_enabled,
        extensions: extensions_from_input(input.extensions)?,
        captcha: CaptchaConfig {
            enabled: input.captcha.enabled,
            selected_solver: selected_solver.clone(),
            solvers: solver_map_from_input(input.captcha.solvers, &selected_solver)?,
        },
    };
    browser.normalize();
    Ok(browser)
}

fn extensions_from_input(
    extensions: Vec<SaveBrowserExtensionParams>,
) -> Result<Vec<BrowserExtensionConfig>> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for extension in extensions {
        let id = required_trimmed("extension id", extension.id)?;
        if !seen.insert(id.clone()) {
            anyhow::bail!("duplicate browser extension id `{id}`");
        }
        let display_name = non_empty(Some(extension.display_name)).unwrap_or_else(|| id.clone());
        let path = normalize_extension_path(extension.path)?;
        out.push(BrowserExtensionConfig {
            id,
            display_name,
            path: path.display().to_string(),
            enabled: extension.enabled,
        });
    }
    Ok(out)
}

fn solver_map_from_input(
    solvers: Vec<SaveBrowserCaptchaSolverParams>,
    selected_solver: &str,
) -> Result<BTreeMap<String, CaptchaSolverConfig>> {
    let mut out = BTreeMap::new();
    for solver in solvers {
        if !is_builtin_solver(&solver.id) {
            anyhow::bail!("unknown captcha solver `{}`", solver.id);
        }
        let base_url = normalized_url(solver.base_url, &solver.id)?;
        let id = solver.id;
        let _client_enabled = solver.enabled;
        out.insert(
            id.clone(),
            CaptchaSolverConfig {
                enabled: id == selected_solver,
                base_url,
                api_key_secret_id: non_empty(solver.api_key_secret_id),
            },
        );
    }
    Ok(out)
}

fn normalize_extension_path(value: String) -> Result<PathBuf> {
    let value = required_trimmed("extension path", value)?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        anyhow::bail!("browser extension path must be absolute");
    }
    ensure_extension_manifest(&path)?;
    Ok(path)
}

fn ensure_extension_manifest(path: &Path) -> Result<()> {
    let manifest = path.join("manifest.json");
    if !manifest.is_file() {
        anyhow::bail!(
            "browser extension path `{}` is missing manifest.json",
            path.display()
        );
    }
    Ok(())
}

fn normalized_url(value: Option<String>, solver_id: &str) -> Result<Option<String>> {
    let Some(value) = non_empty(value) else {
        return Ok(None);
    };
    let parsed =
        Url::parse(&value).with_context(|| format!("invalid base URL for `{solver_id}`"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(Some(value)),
        other => anyhow::bail!("base URL for `{solver_id}` must use http or https, got `{other}`"),
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|item| {
        let trimmed = item.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn required_trimmed(label: &str, value: String) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{label} cannot be empty");
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::browser_config_from_input;
    use crate::desktop_api_types::{
        SaveBrowserCaptchaSettingsParams, SaveBrowserCaptchaSolverParams, SaveBrowserSettingsParams,
    };

    #[test]
    fn save_browser_settings_enables_only_selected_solver() {
        let config = browser_config_from_input(SaveBrowserSettingsParams {
            extensions_enabled: true,
            extensions: Vec::new(),
            captcha: SaveBrowserCaptchaSettingsParams {
                enabled: true,
                selected_solver: "2captcha".to_string(),
                solvers: vec![
                    SaveBrowserCaptchaSolverParams {
                        id: "nopecha".to_string(),
                        enabled: true,
                        base_url: Some("https://api.nopecha.com".to_string()),
                        api_key_secret_id: Some("secret-nopecha".to_string()),
                    },
                    SaveBrowserCaptchaSolverParams {
                        id: "2captcha".to_string(),
                        enabled: false,
                        base_url: Some("https://2captcha.com".to_string()),
                        api_key_secret_id: Some("secret-2captcha".to_string()),
                    },
                ],
            },
        })
        .expect("browser settings should parse");

        assert!(!config.captcha.solvers["nopecha"].enabled);
        assert!(config.captcha.solvers["2captcha"].enabled);
        assert_eq!(config.captcha.selected_solver, "2captcha");
    }
}
