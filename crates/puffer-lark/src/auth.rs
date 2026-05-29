//! Lark credential persistence.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Lark Open Platform brand and endpoint family.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LarkBrand {
    /// International Lark endpoint family.
    Lark,
    /// Mainland Feishu endpoint family.
    Feishu,
}

impl LarkBrand {
    /// Parses a brand string accepted by the Lark CLI reference.
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_lowercase().as_str() {
            "lark" | "larksuite" => Ok(Self::Lark),
            "feishu" => Ok(Self::Feishu),
            other => bail!("unsupported Lark brand `{other}`; expected `lark` or `feishu`"),
        }
    }

    /// Returns the brand id used in serialized credentials.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lark => "lark",
            Self::Feishu => "feishu",
        }
    }

    /// Returns the OpenAPI base URL for this brand.
    pub fn open_base(self) -> &'static str {
        match self {
            Self::Lark => "https://open.larksuite.com",
            Self::Feishu => "https://open.feishu.cn",
        }
    }

    /// Returns the OAuth accounts base URL for this brand.
    pub fn accounts_base(self) -> &'static str {
        match self {
            Self::Lark => "https://accounts.larksuite.com",
            Self::Feishu => "https://accounts.feishu.cn",
        }
    }
}

impl Default for LarkBrand {
    fn default() -> Self {
        Self::Lark
    }
}

/// Serialized authentication material for one Lark connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LarkAuthKind {
    /// Custom app credentials used to mint tenant access tokens.
    App {
        /// Lark app id, usually `cli_...`.
        app_id: String,
        /// Lark app secret.
        app_secret: String,
    },
    /// User access token credential.
    UserToken {
        /// Optional app id associated with the user token.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        /// User access token.
        user_access_token: String,
    },
}

impl LarkAuthKind {
    /// Returns the app id when the credential carries one.
    pub fn app_id(&self) -> Option<&str> {
        match self {
            Self::App { app_id, .. } => Some(app_id.as_str()),
            Self::UserToken { app_id, .. } => app_id.as_deref(),
        }
    }
}

/// Credential file for one authorized Lark account or app connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LarkCredential {
    /// Stable Puffer connection slug.
    pub connection_slug: String,
    /// Connector template slug, for example `lark-login` or `lark-app`.
    pub connector_slug: String,
    /// Lark endpoint brand.
    #[serde(default)]
    pub brand: LarkBrand,
    /// Tenant key returned by auth checks when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_key: Option<String>,
    /// User open id returned by user token auth checks when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_open_id: Option<String>,
    /// User display name returned by user token auth checks when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    /// Authentication material.
    pub auth: LarkAuthKind,
}

/// Returns the credential directory for one Lark connection.
pub fn credential_dir(user_config_dir: &Path, connection_slug: &str) -> PathBuf {
    user_config_dir.join("lark-accounts").join(connection_slug)
}

/// Returns the credential file path for one Lark connection.
pub fn credential_path(user_config_dir: &Path, connection_slug: &str) -> PathBuf {
    credential_dir(user_config_dir, connection_slug).join("credentials.json")
}

/// Saves Lark credentials with user-only permissions where the platform supports it.
pub fn save_credential(path: &Path, credential: &LarkCredential) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create Lark credential dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let encoded = serde_json::to_vec_pretty(credential).context("encode Lark credentials")?;
    write_secret_file(&tmp, &encoded)
        .with_context(|| format!("write Lark credential file {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("install Lark credential file {}", path.display()))?;
    Ok(())
}

/// Loads Lark credentials for one connection.
pub fn load_credential(path: &Path) -> Result<LarkCredential> {
    let raw =
        fs::read(path).with_context(|| format!("read Lark credential file {}", path.display()))?;
    serde_json::from_slice(&raw).context("parse Lark credentials")
}

/// Returns the connector template slug that matches a Lark auth kind.
pub fn connector_slug_for_auth(auth: &LarkAuthKind) -> &'static str {
    match auth {
        LarkAuthKind::App { .. } => "lark-app",
        LarkAuthKind::UserToken { .. } => "lark-login",
    }
}

/// Builds a user-facing connection description without exposing secrets.
pub fn connection_description(credential: &LarkCredential) -> String {
    let brand = credential.brand.as_str();
    let app = credential.auth.app_id().unwrap_or("app");
    let user = credential
        .user_name
        .as_deref()
        .or(credential.user_open_id.as_deref());
    match (&credential.auth, user) {
        (LarkAuthKind::App { .. }, Some(user)) => {
            format!("Lark app for {brand}:{app} ({user})")
        }
        (LarkAuthKind::App { .. }, None) => format!("Lark app for {brand}:{app}"),
        (_, Some(user)) => format!("Lark login for {brand} ({user})"),
        (_, None) => format!("Lark login for {brand}"),
    }
}

#[cfg(unix)]
fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_parses_endpoint_families() {
        assert_eq!(LarkBrand::parse("lark").unwrap(), LarkBrand::Lark);
        assert_eq!(LarkBrand::parse("feishu").unwrap(), LarkBrand::Feishu);
        assert_eq!(LarkBrand::Lark.open_base(), "https://open.larksuite.com");
    }

    #[test]
    fn credential_round_trips_without_losing_kind() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("credentials.json");
        let credential = LarkCredential {
            connection_slug: "work".into(),
            connector_slug: "lark-login".into(),
            brand: LarkBrand::Lark,
            tenant_key: None,
            user_open_id: Some("ou_1".into()),
            user_name: Some("shou".into()),
            auth: LarkAuthKind::UserToken {
                app_id: Some("cli_1".into()),
                user_access_token: "u-token".into(),
            },
        };

        save_credential(&path, &credential).unwrap();
        let loaded = load_credential(&path).unwrap();

        assert_eq!(loaded, credential);
        assert_eq!(connector_slug_for_auth(&loaded.auth), "lark-login");
        assert_eq!(
            connection_description(&loaded),
            "Lark login for lark (shou)"
        );
    }
}
