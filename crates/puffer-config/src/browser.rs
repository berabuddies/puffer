use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Built-in CAPTCHA solver extension metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinCaptchaSolver {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub default_base_url: &'static str,
    pub version: &'static str,
    pub extension_path: &'static str,
    pub release_url: &'static str,
    pub download_url: &'static str,
    pub sha256: &'static str,
    pub license: &'static str,
}

/// Browser launch preferences persisted in the user config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserConfig {
    #[serde(default = "default_extensions_enabled")]
    pub extensions_enabled: bool,
    #[serde(default)]
    pub extensions: Vec<BrowserExtensionConfig>,
    #[serde(default)]
    pub captcha: CaptchaConfig,
}

/// User-added browser extension path and enablement settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserExtensionConfig {
    pub id: String,
    pub display_name: String,
    pub path: String,
    #[serde(default = "default_extension_enabled")]
    pub enabled: bool,
}

/// CAPTCHA solver settings persisted under the browser config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptchaConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_captcha_solver")]
    pub selected_solver: String,
    #[serde(default)]
    pub solvers: BTreeMap<String, CaptchaSolverConfig>,
}

/// Per-solver user configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CaptchaSolverConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_secret_id: Option<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            extensions_enabled: default_extensions_enabled(),
            extensions: Vec::new(),
            captcha: CaptchaConfig::default(),
        }
    }
}

impl Default for CaptchaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            selected_solver: default_captcha_solver(),
            solvers: BTreeMap::new(),
        }
    }
}

impl BrowserConfig {
    /// Normalizes browser extension settings against the built-in solver catalog.
    pub fn normalize(&mut self) {
        self.extensions
            .retain_mut(|extension| extension.normalize());
        self.captcha.normalize();
    }
}

impl BrowserExtensionConfig {
    /// Normalizes extension fields and reports whether the entry remains valid.
    pub fn normalize(&mut self) -> bool {
        self.id = normalized_string(&self.id);
        self.display_name = normalized_string(&self.display_name);
        self.path = normalized_string(&self.path);
        if self.display_name.is_empty() {
            self.display_name = self.id.clone();
        }
        !self.id.is_empty() && !self.path.is_empty()
    }
}

impl CaptchaConfig {
    /// Normalizes selected and per-solver settings against the built-in catalog.
    pub fn normalize(&mut self) {
        if !is_builtin_solver(&self.selected_solver) {
            self.selected_solver = default_captcha_solver();
        }
        self.solvers.retain(|id, _| is_builtin_solver(id));
        for (id, solver) in self.solvers.iter_mut() {
            solver.enabled = id == &self.selected_solver;
            solver.normalize();
        }
    }
}

impl CaptchaSolverConfig {
    /// Normalizes optional string fields by treating blanks as unset.
    pub fn normalize(&mut self) {
        self.base_url = normalized_optional(self.base_url.take());
        self.api_key_secret_id = normalized_optional(self.api_key_secret_id.take());
    }
}

/// Returns the built-in CAPTCHA solver extension catalog.
pub fn builtin_captcha_solvers() -> &'static [BuiltinCaptchaSolver] {
    &BUILTIN_CAPTCHA_SOLVERS
}

/// Returns true when `id` names a built-in CAPTCHA solver.
pub fn is_builtin_solver(id: &str) -> bool {
    BUILTIN_CAPTCHA_SOLVERS.iter().any(|solver| solver.id == id)
}

fn default_extensions_enabled() -> bool {
    true
}

fn default_extension_enabled() -> bool {
    true
}

fn default_captcha_solver() -> String {
    "nopecha".to_string()
}

fn normalized_string(value: &str) -> String {
    value.trim().to_string()
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value.and_then(|item| {
        let trimmed = item.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

const BUILTIN_CAPTCHA_SOLVERS: [BuiltinCaptchaSolver; 2] = [
    BuiltinCaptchaSolver {
        id: "nopecha",
        display_name: "NopeCHA",
        description: "General CAPTCHA solver for reCAPTCHA, hCaptcha, Turnstile, FunCAPTCHA, and related challenges.",
        default_base_url: "https://api.nopecha.com",
        version: "0.6.0",
        extension_path: "browser_extensions/nopecha/chromium_automation",
        release_url: "https://github.com/NopeCHALLC/nopecha-extension/releases/tag/0.6.0",
        download_url: "https://github.com/NopeCHALLC/nopecha-extension/releases/download/0.6.0/chromium_automation.zip",
        sha256: "4871e1c6ed200dde8e5e790c23458415cb3213312701d3ff757c8ee115b79c3b",
        license: "MIT",
    },
    BuiltinCaptchaSolver {
        id: "2captcha",
        display_name: "2Captcha",
        description: "2Captcha browser extension for common token, image, grid, and interactive CAPTCHA tasks.",
        default_base_url: "https://2captcha.com",
        version: "3.7.2",
        extension_path: "browser_extensions/2captcha/chromium",
        release_url: "https://github.com/rucaptcha/2captcha-solver/releases/tag/v3.7.2",
        download_url: "https://github.com/rucaptcha/2captcha-solver/releases/download/v3.7.2/2captcha-solver-chrome-3.7.2.zip",
        sha256: "",
        license: "MIT",
    },
];

#[cfg(test)]
mod tests {
    use super::{builtin_captcha_solvers, BrowserConfig};

    #[test]
    fn browser_config_defaults_to_nopecha_catalog_selection() {
        let config = BrowserConfig::default();
        assert!(config.extensions_enabled);
        assert!(!config.captcha.enabled);
        assert_eq!(config.captcha.selected_solver, "nopecha");
        assert_eq!(builtin_captcha_solvers().len(), 2);
    }

    #[test]
    fn browser_config_drops_unknown_solver_settings() {
        let mut config = BrowserConfig::default();
        config.captcha.selected_solver = "unknown".to_string();
        config
            .captcha
            .solvers
            .insert("unknown".to_string(), Default::default());
        config.normalize();

        assert_eq!(config.captcha.selected_solver, "nopecha");
        assert!(config.captcha.solvers.is_empty());
    }

    #[test]
    fn browser_config_enables_only_selected_solver() {
        let mut config = BrowserConfig::default();
        config.captcha.selected_solver = "2captcha".to_string();
        config.captcha.solvers.insert(
            "nopecha".to_string(),
            super::CaptchaSolverConfig {
                enabled: true,
                ..Default::default()
            },
        );
        config.captcha.solvers.insert(
            "2captcha".to_string(),
            super::CaptchaSolverConfig {
                enabled: false,
                ..Default::default()
            },
        );

        config.normalize();

        assert!(!config.captcha.solvers["nopecha"].enabled);
        assert!(config.captcha.solvers["2captcha"].enabled);
    }

    #[test]
    fn browser_config_normalizes_custom_extensions() {
        let mut config = BrowserConfig::default();
        config.extensions.push(super::BrowserExtensionConfig {
            id: " custom ".to_string(),
            display_name: " ".to_string(),
            path: " /tmp/ext ".to_string(),
            enabled: true,
        });
        config.extensions.push(super::BrowserExtensionConfig {
            id: " ".to_string(),
            display_name: "Blank".to_string(),
            path: "/tmp/ext".to_string(),
            enabled: true,
        });

        config.normalize();

        assert_eq!(config.extensions.len(), 1);
        assert_eq!(config.extensions[0].id, "custom");
        assert_eq!(config.extensions[0].display_name, "custom");
        assert_eq!(config.extensions[0].path, "/tmp/ext");
    }
}
