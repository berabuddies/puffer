use serde::{Deserialize, Serialize};

/// Stores the Browser-specific policy section in `permissions.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct BrowserPolicySettings {
    #[serde(default)]
    pub(crate) deny_target_classes: Vec<String>,
    #[serde(default)]
    pub(crate) deny_origins: Vec<String>,
    #[serde(default)]
    pub(crate) deny_domains: Vec<String>,
    #[serde(default)]
    pub(crate) deny_evaluate_target_classes: Vec<String>,
    #[serde(default)]
    pub(crate) allow_target_classes: Vec<String>,
    #[serde(default)]
    pub(crate) allow_origins: Vec<String>,
    #[serde(default)]
    pub(crate) allow_domains: Vec<String>,
}

impl BrowserPolicySettings {
    pub(crate) fn normalized(self) -> Self {
        Self {
            deny_target_classes: normalize_values(self.deny_target_classes),
            deny_origins: normalize_values(self.deny_origins),
            deny_domains: normalize_values(self.deny_domains),
            deny_evaluate_target_classes: normalize_values(self.deny_evaluate_target_classes),
            allow_target_classes: normalize_values(self.allow_target_classes),
            allow_origins: normalize_values(self.allow_origins),
            allow_domains: normalize_values(self.allow_domains),
        }
    }
}

fn normalize_values(values: Vec<String>) -> Vec<String> {
    let mut normalized = values
        .into_iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}
