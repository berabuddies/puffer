use anyhow::Context;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Describes the credential modes supported by a provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    ApiKey,
    #[serde(rename = "oauth")]
    OAuth,
    SessionIngress,
}

/// Stores OAuth credentials in a provider-agnostic format.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthCredential {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_at_ms: u64,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub organization_id: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub plan_type: Option<String>,
    #[serde(default)]
    pub rate_limit_tier: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Represents persisted credentials for one provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredCredential {
    ApiKey {
        key: String,
    },
    #[serde(rename = "oauth")]
    OAuth(OAuthCredential),
}

/// Stores all persisted provider credentials.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthStore {
    pub providers: BTreeMap<String, StoredCredential>,
}

impl AuthStore {
    /// Loads persisted credentials from disk, returning an empty store when the file is missing.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read auth store {}", path.display()))?;
        let store = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse auth store {}", path.display()))?;
        Ok(store)
    }

    /// Saves the current credential set to disk as pretty-printed JSON.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create auth store parent dir {}",
                    parent.display()
                )
            })?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        fs::write(path, raw)
            .with_context(|| format!("failed to write auth store {}", path.display()))?;
        Ok(())
    }

    /// Returns true when credentials for the given provider exist.
    pub fn has_auth(&self, provider_id: &str) -> bool {
        self.providers.contains_key(provider_id)
    }

    /// Returns the stored credential for a provider when present.
    pub fn get(&self, provider_id: &str) -> Option<&StoredCredential> {
        self.providers.get(provider_id)
    }

    /// Stores an API key for a provider.
    pub fn set_api_key(&mut self, provider_id: impl Into<String>, key: impl Into<String>) {
        self.providers.insert(
            provider_id.into(),
            StoredCredential::ApiKey { key: key.into() },
        );
    }

    /// Stores OAuth credentials for a provider.
    pub fn set_oauth(&mut self, provider_id: impl Into<String>, credential: OAuthCredential) {
        self.providers
            .insert(provider_id.into(), StoredCredential::OAuth(credential));
    }

    /// Removes any stored credentials for a provider.
    pub fn remove(&mut self, provider_id: &str) -> Option<StoredCredential> {
        self.providers.remove(provider_id)
    }

    /// Returns all provider ids that currently have stored credentials.
    pub fn provider_ids(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_store_supports_api_key_and_oauth_helpers() {
        let mut store = AuthStore::default();
        store.set_api_key("anthropic", "sk-ant");
        assert!(store.has_auth("anthropic"));
        assert!(matches!(
            store.get("anthropic"),
            Some(StoredCredential::ApiKey { key }) if key == "sk-ant"
        ));

        store.set_oauth(
            "openai",
            OAuthCredential {
                access_token: "acc".to_string(),
                refresh_token: "ref".to_string(),
                expires_at_ms: 42,
                account_id: Some("acct".to_string()),
                organization_id: Some("org".to_string()),
                email: None,
                plan_type: Some("pro".to_string()),
                rate_limit_tier: Some("tier-1".to_string()),
                scopes: vec!["openid".to_string()],
            },
        );
        assert!(matches!(
            store.get("openai"),
            Some(StoredCredential::OAuth(credential))
                if credential.account_id.as_deref() == Some("acct")
                    && credential.organization_id.as_deref() == Some("org")
                    && credential.plan_type.as_deref() == Some("pro")
        ));

        let removed = store.remove("anthropic");
        assert!(removed.is_some());
        assert!(!store.has_auth("anthropic"));
    }
}
