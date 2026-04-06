use anyhow::Context;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::secure_oauth::{
    delete_oauth_secret, load_oauth_secret, store_oauth_secret, OAuthSecret,
};

/// Describes the credential modes supported by a provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    ApiKey,
    #[serde(rename = "oauth", alias = "o_auth")]
    OAuth,
    SessionIngress,
}

/// Stores OAuth credentials in a provider-agnostic format.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthCredential {
    pub access_token: String,
    pub refresh_token: String,
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
    #[serde(default)]
    pub organization_name: Option<String>,
    #[serde(default)]
    pub organization_role: Option<String>,
    #[serde(default)]
    pub workspace_role: Option<String>,
}

/// Represents persisted credentials for one provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredCredential {
    ApiKey {
        key: String,
    },
    #[serde(rename = "oauth", alias = "o_auth")]
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
        if let Ok(persisted) = serde_json::from_str::<PersistedAuthStore>(&raw) {
            return Self::from_persisted(path, persisted);
        }

        let legacy = serde_json::from_str::<LegacyAuthStore>(&raw)
            .with_context(|| format!("failed to parse auth store {}", path.display()))?;
        let store = Self::from_legacy(path, legacy)?;
        store.save(path)?;
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
        let existing = read_persisted_provider_kinds(path)?;
        let mut persisted = PersistedAuthStore {
            format_version: 1,
            providers: BTreeMap::new(),
        };

        for (provider_id, credential) in &self.providers {
            match credential {
                StoredCredential::ApiKey { key } => {
                    persisted.providers.insert(
                        provider_id.clone(),
                        PersistedStoredCredential::ApiKey { key: key.clone() },
                    );
                }
                StoredCredential::OAuth(credential) => {
                    store_oauth_secret(
                        path,
                        provider_id,
                        &OAuthSecret {
                            access_token: credential.access_token.clone(),
                            refresh_token: credential.refresh_token.clone(),
                        },
                    )?;
                    persisted.providers.insert(
                        provider_id.clone(),
                        PersistedStoredCredential::OAuth(PersistedOAuthCredential {
                            expires_at_ms: credential.expires_at_ms,
                            account_id: credential.account_id.clone(),
                            organization_id: credential.organization_id.clone(),
                            email: credential.email.clone(),
                            plan_type: credential.plan_type.clone(),
                            rate_limit_tier: credential.rate_limit_tier.clone(),
                            scopes: credential.scopes.clone(),
                            organization_name: credential.organization_name.clone(),
                            organization_role: credential.organization_role.clone(),
                            workspace_role: credential.workspace_role.clone(),
                        }),
                    );
                }
            }
        }

        for (provider_id, kind) in existing {
            if kind == PersistedCredentialKind::OAuth
                && !matches!(
                    self.providers.get(&provider_id),
                    Some(StoredCredential::OAuth(_))
                )
            {
                delete_oauth_secret(path, &provider_id)?;
            }
        }

        let raw = serde_json::to_string_pretty(&persisted)?;
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

    fn from_persisted(path: &Path, persisted: PersistedAuthStore) -> Result<Self> {
        let mut providers = BTreeMap::new();
        for (provider_id, credential) in persisted.providers {
            let hydrated = match credential {
                PersistedStoredCredential::ApiKey { key } => StoredCredential::ApiKey { key },
                PersistedStoredCredential::OAuth(metadata) => {
                    let Some(secret) = load_oauth_secret(path, &provider_id)? else {
                        continue;
                    };
                    StoredCredential::OAuth(OAuthCredential {
                        access_token: secret.access_token,
                        refresh_token: secret.refresh_token,
                        expires_at_ms: metadata.expires_at_ms,
                        account_id: metadata.account_id,
                        organization_id: metadata.organization_id,
                        email: metadata.email,
                        plan_type: metadata.plan_type,
                        rate_limit_tier: metadata.rate_limit_tier,
                        scopes: metadata.scopes,
                        organization_name: metadata.organization_name,
                        organization_role: metadata.organization_role,
                        workspace_role: metadata.workspace_role,
                    })
                }
            };
            providers.insert(provider_id, hydrated);
        }
        Ok(Self { providers })
    }

    fn from_legacy(path: &Path, legacy: LegacyAuthStore) -> Result<Self> {
        let store = Self {
            providers: legacy.providers,
        };
        for (provider_id, credential) in &store.providers {
            if let StoredCredential::OAuth(credential) = credential {
                store_oauth_secret(
                    path,
                    provider_id,
                    &OAuthSecret {
                        access_token: credential.access_token.clone(),
                        refresh_token: credential.refresh_token.clone(),
                    },
                )?;
            }
        }
        Ok(store)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedAuthStore {
    format_version: u32,
    providers: BTreeMap<String, PersistedStoredCredential>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PersistedStoredCredential {
    ApiKey {
        key: String,
    },
    #[serde(rename = "oauth", alias = "o_auth")]
    OAuth(PersistedOAuthCredential),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PersistedOAuthCredential {
    expires_at_ms: u64,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    organization_id: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit_tier: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    organization_name: Option<String>,
    #[serde(default)]
    organization_role: Option<String>,
    #[serde(default)]
    workspace_role: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct LegacyAuthStore {
    providers: BTreeMap<String, StoredCredential>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersistedCredentialKind {
    ApiKey,
    OAuth,
}

fn read_persisted_provider_kinds(path: &Path) -> Result<BTreeMap<String, PersistedCredentialKind>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read auth store {}", path.display()))?;
    if let Ok(persisted) = serde_json::from_str::<PersistedAuthStore>(&raw) {
        return Ok(persisted
            .providers
            .into_iter()
            .map(|(provider_id, credential)| {
                let kind = match credential {
                    PersistedStoredCredential::ApiKey { .. } => PersistedCredentialKind::ApiKey,
                    PersistedStoredCredential::OAuth(_) => PersistedCredentialKind::OAuth,
                };
                (provider_id, kind)
            })
            .collect());
    }
    let legacy = serde_json::from_str::<LegacyAuthStore>(&raw)
        .with_context(|| format!("failed to parse auth store {}", path.display()))?;
    Ok(legacy
        .providers
        .into_iter()
        .map(|(provider_id, credential)| {
            let kind = match credential {
                StoredCredential::ApiKey { .. } => PersistedCredentialKind::ApiKey,
                StoredCredential::OAuth(_) => PersistedCredentialKind::OAuth,
            };
            (provider_id, kind)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;
    use tempfile::tempdir;

    static TEST_SECRET_BACKEND: Once = Once::new();

    fn use_plaintext_secret_backend() {
        TEST_SECRET_BACKEND.call_once(|| unsafe {
            std::env::set_var("PUFFER_TEST_PLAINTEXT_OAUTH_STORAGE", "1");
        });
    }

    #[test]
    fn auth_store_supports_api_key_and_oauth_helpers() {
        use_plaintext_secret_backend();
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
                organization_name: None,
                organization_role: None,
                workspace_role: None,
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

    #[test]
    fn auth_store_saves_oauth_metadata_to_disk_and_tokens_to_secure_store() {
        use_plaintext_secret_backend();
        let tempdir = tempdir().unwrap();
        let path = tempdir.path().join("auth.json");
        let mut store = AuthStore::default();
        store.set_oauth(
            "anthropic",
            OAuthCredential {
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                expires_at_ms: 42,
                account_id: Some("acct-1".to_string()),
                organization_id: Some("org-1".to_string()),
                email: Some("dev@example.com".to_string()),
                plan_type: Some("max".to_string()),
                rate_limit_tier: Some("team_tier".to_string()),
                scopes: vec!["user:profile".to_string(), "user:inference".to_string()],
                organization_name: Some("Example Org".to_string()),
                organization_role: Some("owner".to_string()),
                workspace_role: Some("developer".to_string()),
            },
        );

        store.save(&path).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"format_version\": 1"));
        assert!(!raw.contains("access-token"));
        assert!(!raw.contains("refresh-token"));

        let loaded = AuthStore::load(&path).unwrap();
        assert_eq!(loaded, store);
    }

    #[test]
    fn auth_store_load_migrates_legacy_oauth_files() {
        use_plaintext_secret_backend();
        let tempdir = tempdir().unwrap();
        let path = tempdir.path().join("auth.json");
        fs::write(
            &path,
            serde_json::json!({
                "providers": {
                    "anthropic": {
                        "kind": "oauth",
                        "access_token": "legacy-access",
                        "refresh_token": "legacy-refresh",
                        "expires_at_ms": 99,
                        "account_id": "acct-legacy",
                        "organization_id": "org-legacy",
                        "email": "legacy@example.com",
                        "plan_type": "pro",
                        "rate_limit_tier": "default",
                        "scopes": ["user:profile", "user:inference"]
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let loaded = AuthStore::load(&path).unwrap();
        let Some(StoredCredential::OAuth(credential)) = loaded.get("anthropic") else {
            panic!("expected migrated OAuth credential");
        };
        assert_eq!(credential.access_token, "legacy-access");
        assert_eq!(credential.refresh_token, "legacy-refresh");

        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"format_version\": 1"));
        assert!(!raw.contains("legacy-access"));
    }

    #[test]
    fn auth_store_load_skips_oauth_metadata_when_secret_store_is_missing() {
        use_plaintext_secret_backend();
        let tempdir = tempdir().unwrap();
        let path = tempdir.path().join("auth.json");
        fs::write(
            &path,
            serde_json::json!({
                "format_version": 1,
                "providers": {
                    "anthropic": {
                        "kind": "oauth",
                        "expires_at_ms": 42,
                        "scopes": ["user:profile"]
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let loaded = AuthStore::load(&path).unwrap();
        assert!(loaded.get("anthropic").is_none());
    }
}
