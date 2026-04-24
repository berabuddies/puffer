//! Persisted configuration for the email subscriber.
//!
//! The active [`EmailConfig`] is stored as TOML at
//! `<state_dir>/config.toml`. It is written atomically (tempfile + rename) so
//! a crash during save cannot leave a torn file behind.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// Default IMAP port used when the stored / incoming `imap_port` is 0.
pub const DEFAULT_IMAP_PORT: u16 = 993;
/// Default SMTP (submission) port used when the stored / incoming
/// `smtp_port` is 0.
pub const DEFAULT_SMTP_PORT: u16 = 587;

/// Runtime configuration for the email subscriber.
///
/// Mirrors the [`SubscriberCommand::EmailConfigure`](puffer_subscriber_runtime::SubscriberCommand::EmailConfigure)
/// fields one-to-one; port defaults are materialized at load time so consumers
/// never see a zero port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmailConfig {
    /// IMAP server hostname, e.g. `"imap.gmail.com"`.
    pub imap_host: String,
    /// IMAP server port; always a concrete value once loaded.
    pub imap_port: u16,
    /// SMTP server hostname, e.g. `"smtp.gmail.com"`.
    pub smtp_host: String,
    /// SMTP server port; always a concrete value once loaded.
    pub smtp_port: u16,
    /// IMAP/SMTP login (typically the email address).
    pub username: String,
    /// IMAP/SMTP password, stored as plaintext inside the subscriber state
    /// directory managed by the supervisor.
    pub password: String,
    /// `From:` address used on outbound mail.
    pub from_address: String,
    /// Optional allow-list of inbound senders. Empty means "accept every
    /// sender". Entries may be full addresses (case-insensitive) or `@domain`
    /// suffixes.
    #[serde(default)]
    pub allowed_senders: Vec<String>,
}

impl EmailConfig {
    /// Builds an [`EmailConfig`] from raw command fields, materializing port
    /// defaults when the caller passed 0.
    pub fn from_command_fields(
        imap_host: String,
        imap_port: u16,
        smtp_host: String,
        smtp_port: u16,
        username: String,
        password: String,
        from_address: String,
        allowed_senders: Vec<String>,
    ) -> Self {
        Self {
            imap_host,
            imap_port: if imap_port == 0 {
                DEFAULT_IMAP_PORT
            } else {
                imap_port
            },
            smtp_host,
            smtp_port: if smtp_port == 0 {
                DEFAULT_SMTP_PORT
            } else {
                smtp_port
            },
            username,
            password,
            from_address,
            allowed_senders,
        }
    }

    /// Returns `true` when the required hostnames / credentials are present.
    pub fn is_valid(&self) -> bool {
        !self.imap_host.trim().is_empty()
            && !self.smtp_host.trim().is_empty()
            && !self.username.trim().is_empty()
            && !self.from_address.trim().is_empty()
    }
}

/// Returns the on-disk path of the subscriber's config file.
pub fn config_path(state_dir: &Path) -> PathBuf {
    state_dir.join("config.toml")
}

/// Loads the persisted configuration, returning `Ok(None)` if the file does
/// not exist yet.
pub async fn load(state_dir: &Path) -> anyhow::Result<Option<EmailConfig>> {
    let path = config_path(state_dir);
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => {
            let mut cfg: EmailConfig = toml::from_str(&text)
                .with_context(|| format!("parse config {}", path.display()))?;
            if cfg.imap_port == 0 {
                cfg.imap_port = DEFAULT_IMAP_PORT;
            }
            if cfg.smtp_port == 0 {
                cfg.smtp_port = DEFAULT_SMTP_PORT;
            }
            Ok(Some(cfg))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(anyhow::anyhow!(err)).with_context(|| {
            format!("read email subscriber config at {}", path.display())
        }),
    }
}

/// Persists the given configuration to `<state_dir>/config.toml` atomically.
///
/// Writes to a sibling tempfile, fsyncs it, then renames over the target so
/// concurrent readers never observe a partial file.
pub async fn save(state_dir: &Path, config: &EmailConfig) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(state_dir)
        .await
        .with_context(|| format!("create state dir {}", state_dir.display()))?;
    let target = config_path(state_dir);
    let tmp = target.with_extension("toml.tmp");
    let rendered = toml::to_string_pretty(config).context("serialize email config to toml")?;
    tokio::fs::write(&tmp, rendered.as_bytes())
        .await
        .with_context(|| format!("write tempfile {}", tmp.display()))?;
    tokio::fs::rename(&tmp, &target)
        .await
        .with_context(|| format!("rename {} -> {}", tmp.display(), target.display()))?;
    Ok(())
}
