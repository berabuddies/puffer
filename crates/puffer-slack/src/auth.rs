//! Slack credential persistence.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Serialized authentication material for one Slack connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SlackAuthKind {
    /// Slack app tokens: bot token for Web API and app token for Socket Mode.
    App {
        /// Bot User OAuth token, usually `xoxb-...`.
        bot_token: String,
        /// App-level token, usually `xapp-...`.
        app_token: String,
    },
    /// Direct Slack OAuth token.
    Standard {
        /// OAuth token, usually `xoxb-...` or `xoxp-...`.
        token: String,
        /// Human-readable token kind for diagnostics.
        token_type: String,
    },
    /// Browser session tokens imported from a local Slack desktop profile.
    Browser {
        /// Workspace URL, for example `https://example.slack.com`.
        workspace_url: String,
        /// Browser `d` cookie value, usually `xoxd-...`.
        xoxd_token: String,
        /// Browser API token, usually `xoxc-...`.
        xoxc_token: String,
    },
}

/// Credential file for one authorized Slack account or app connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlackCredential {
    /// Stable Puffer connection slug.
    pub connection_slug: String,
    /// Connector template slug, for example `slack-login` or `slack-app`.
    pub connector_slug: String,
    /// Slack workspace id returned by `auth.test` when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Slack workspace name returned by `auth.test` when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    /// Slack user id returned by `auth.test` when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Slack user name returned by `auth.test` when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    /// Authentication material.
    pub auth: SlackAuthKind,
}

/// Returns the credential directory for one Slack connection.
pub fn credential_dir(user_config_dir: &Path, connection_slug: &str) -> PathBuf {
    user_config_dir.join("slack-accounts").join(connection_slug)
}

/// Returns the credential file path for one Slack connection.
pub fn credential_path(user_config_dir: &Path, connection_slug: &str) -> PathBuf {
    credential_dir(user_config_dir, connection_slug).join("credentials.json")
}

/// Saves Slack credentials with user-only permissions where the platform supports it.
pub fn save_credential(path: &Path, credential: &SlackCredential) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create Slack credential dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let encoded = serde_json::to_vec_pretty(credential).context("encode Slack credentials")?;
    write_secret_file(&tmp, &encoded)
        .with_context(|| format!("write Slack credential file {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("install Slack credential file {}", path.display()))?;
    Ok(())
}

/// Loads Slack credentials for one connection.
pub fn load_credential(path: &Path) -> Result<SlackCredential> {
    let raw =
        fs::read(path).with_context(|| format!("read Slack credential file {}", path.display()))?;
    serde_json::from_slice(&raw).context("parse Slack credentials")
}

/// Returns the connector template slug that matches a Slack auth kind.
pub fn connector_slug_for_auth(auth: &SlackAuthKind) -> &'static str {
    match auth {
        SlackAuthKind::App { .. } => "slack-app",
        SlackAuthKind::Standard { .. } | SlackAuthKind::Browser { .. } => "slack-login",
    }
}

/// Builds a user-facing connection description without exposing tokens.
pub fn connection_description(credential: &SlackCredential) -> String {
    let workspace = credential
        .workspace_name
        .as_deref()
        .or(credential.workspace_id.as_deref())
        .unwrap_or("workspace");
    let user = credential
        .user_name
        .as_deref()
        .or(credential.user_id.as_deref());
    match (&credential.auth, user) {
        (SlackAuthKind::App { .. }, Some(user)) => format!("Slack app for {workspace} ({user})"),
        (SlackAuthKind::App { .. }, None) => format!("Slack app for {workspace}"),
        (_, Some(user)) => format!("Slack login for {workspace} ({user})"),
        (_, None) => format!("Slack login for {workspace}"),
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
    fn credential_path_is_account_scoped() {
        let root = PathBuf::from("/tmp/puffer");

        assert_eq!(
            credential_path(&root, "work"),
            PathBuf::from("/tmp/puffer/slack-accounts/work/credentials.json")
        );
    }

    #[test]
    fn credential_round_trips_without_losing_kind() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("credentials.json");
        let credential = SlackCredential {
            connection_slug: "work".into(),
            connector_slug: "slack-login".into(),
            workspace_id: Some("T1".into()),
            workspace_name: Some("Acme".into()),
            user_id: Some("U1".into()),
            user_name: Some("shou".into()),
            auth: SlackAuthKind::Browser {
                workspace_url: "https://acme.slack.com".into(),
                xoxd_token: "xoxd-token".into(),
                xoxc_token: "xoxc-token".into(),
            },
        };

        save_credential(&path, &credential).unwrap();
        let loaded = load_credential(&path).unwrap();

        assert_eq!(loaded, credential);
        assert_eq!(
            connection_description(&loaded),
            "Slack login for Acme (shou)"
        );
    }
}
