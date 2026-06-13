//! Persisted per-instance configuration for a managed WeChat desktop.
//!
//! In the "puffer is the only thing on the box" topology there is no
//! WechatOnCloud panel, so puffer creates the container itself. Each instance
//! needs stable settings — image, the published localhost port, the randomly
//! generated KasmVNC credentials, and the data volume — so that a restart
//! re-attaches to the *same* logged-in session instead of forcing a new QR
//! scan. These live in `~/.puffer/wechat/<instance>.json` (override the dir
//! with `WECHAT_STATE_DIR`), generated on first bringup and reused thereafter.

use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::docker::DEFAULT_IMAGE;

/// Default localhost port the instance's KasmVNC web client is published on.
const DEFAULT_HOST_PORT: u16 = 37042;
/// Default KasmVNC username baked into the container (paired with a random pw).
const DEFAULT_KASM_USER: &str = "woc";

/// Stable settings for one WeChat instance, persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct InstanceConfig {
    /// Logical instance name (`default`); container/volume are `puffer-wechat-<name>`.
    pub(crate) instance: String,
    /// Container image (WechatOnCloud, multi-arch).
    pub(crate) image: String,
    /// Localhost TCP port the KasmVNC web client (container :3000) is published on.
    pub(crate) host_port: u16,
    /// KasmVNC basic-auth username injected as `CUSTOM_USER`.
    pub(crate) kasm_user: String,
    /// KasmVNC basic-auth password injected as `PASSWORD` (random per instance).
    pub(crate) kasm_password: String,
    /// In-container runtime user id (`PUID`).
    pub(crate) puid: String,
    /// In-container runtime group id (`PGID`).
    pub(crate) pgid: String,
    /// Container timezone (`TZ`).
    pub(crate) tz: String,
    /// Whether this instance runs the bare base image and has the accessibility
    /// stack layered on at RUNTIME (the Docker-free fallback used when the baked
    /// a11y image can't be built — e.g. Apple `container`'s build VM can't reach
    /// the apt mirrors). Set on the first bringup that falls back, and persisted
    /// so later starts (and the act/subscribe paths) re-apply a11y on the base
    /// image. `false` for the normal baked-image instances. Defaults to `false`
    /// for configs written before this field existed.
    #[serde(default)]
    pub(crate) runtime_a11y: bool,
}

impl InstanceConfig {
    /// Loads the instance config, creating and persisting a fresh one (with a
    /// random KasmVNC password) on first use. Environment variables override
    /// individual fields at creation time only: `WECHAT_IMAGE`, `WECHAT_PORT`,
    /// `WECHAT_KASM_USER`, `WECHAT_PUID`, `WECHAT_PGID`, `WECHAT_TZ`.
    pub(crate) fn load_or_create(instance: &str) -> Result<Self> {
        let path = state_path(instance)?;
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("read wechat config {}", path.display()))?;
            let config: Self = serde_json::from_str(&raw)
                .with_context(|| format!("parse wechat config {}", path.display()))?;
            return Ok(config);
        }
        let config = Self::generate(instance);
        config.save()?;
        Ok(config)
    }

    /// Builds a fresh config from defaults + environment overrides. The host
    /// port is `WECHAT_PORT` if set, else derived from the slug so distinct
    /// instances don't collide on one fixed port.
    fn generate(instance: &str) -> Self {
        let host_port = std::env::var("WECHAT_PORT")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| port_for_instance(instance));
        Self {
            instance: instance.to_string(),
            image: env_or("WECHAT_IMAGE", DEFAULT_IMAGE),
            host_port,
            kasm_user: env_or("WECHAT_KASM_USER", DEFAULT_KASM_USER),
            kasm_password: random_hex(24),
            puid: env_or("WECHAT_PUID", "1000"),
            pgid: env_or("WECHAT_PGID", "1000"),
            tz: env_or("WECHAT_TZ", "Asia/Shanghai"),
            // A fresh instance targets the baked a11y image; the runtime-a11y
            // fallback flips this on disk only if building/pulling that fails.
            runtime_a11y: false,
        }
    }

    /// Persists the config as pretty JSON, creating parent dirs as needed.
    pub(crate) fn save(&self) -> Result<()> {
        let path = state_path(&self.instance)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create wechat state dir {}", parent.display()))?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, raw).with_context(|| format!("write wechat config {}", path.display()))?;
        Ok(())
    }

    /// Container name (`puffer-wechat-<instance>`).
    pub(crate) fn container_name(&self) -> String {
        format!("puffer-wechat-{}", self.instance)
    }

    /// Data volume name (`puffer-wechat-<instance>`); holds login state + chat cache.
    pub(crate) fn volume_name(&self) -> String {
        format!("puffer-wechat-{}", self.instance)
    }

    /// Credential-embedded KasmVNC URL for a `host:port` authority, so the puffer
    /// browser pane loads it without a login prompt. The authority is loopback
    /// for Docker's published port and the container's vmnet IP for Apple
    /// `container` (which doesn't forward ports); see
    /// [`WechatInstance::desktop_authority`](super::docker::WechatInstance::desktop_authority).
    pub(crate) fn kasm_url_for_authority(&self, authority: &str) -> String {
        format!(
            "http://{}:{}@{}/",
            self.kasm_user, self.kasm_password, authority
        )
    }
}

/// Derives a stable, per-instance loopback port in [37042, 39042) from the
/// instance name, so two connections don't both try to bind one fixed port.
fn port_for_instance(instance: &str) -> u16 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    instance.hash(&mut hasher);
    DEFAULT_HOST_PORT + (hasher.finish() % 2000) as u16
}

/// Returns `~/.puffer/wechat/<instance>.json`, honoring `WECHAT_STATE_DIR` and
/// `PUFFER_HOME`/`HOME` for the base directory.
fn state_path(instance: &str) -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("WECHAT_STATE_DIR") {
        if !dir.trim().is_empty() {
            return Ok(PathBuf::from(dir).join(format!("{instance}.json")));
        }
    }
    let home = std::env::var("PUFFER_HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("HOME").ok())
        .context("neither PUFFER_HOME nor HOME is set; cannot locate wechat state dir")?;
    Ok(PathBuf::from(home)
        .join(".puffer")
        .join("wechat")
        .join(format!("{instance}.json")))
}

/// Returns `key`'s env value if set and non-empty, otherwise `default`.
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Returns `bytes` random bytes hex-encoded (so 24 → 48 hex chars).
fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    buf.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> InstanceConfig {
        InstanceConfig {
            instance: "default".to_string(),
            image: "img:latest".to_string(),
            host_port: 37042,
            kasm_user: "woc".to_string(),
            kasm_password: "secret".to_string(),
            puid: "1000".to_string(),
            pgid: "1000".to_string(),
            tz: "Asia/Shanghai".to_string(),
            runtime_a11y: false,
        }
    }

    #[test]
    fn names_are_prefixed() {
        let cfg = sample();
        assert_eq!(cfg.container_name(), "puffer-wechat-default");
        assert_eq!(cfg.volume_name(), "puffer-wechat-default");
    }

    #[test]
    fn kasm_url_embeds_credentials_for_any_authority() {
        // Docker (loopback) and Apple `container` (vmnet IP) authorities.
        assert_eq!(
            sample().kasm_url_for_authority("127.0.0.1:37042"),
            "http://woc:secret@127.0.0.1:37042/"
        );
        assert_eq!(
            sample().kasm_url_for_authority("192.168.64.6:3000"),
            "http://woc:secret@192.168.64.6:3000/"
        );
    }

    #[test]
    fn random_hex_has_expected_length_and_charset() {
        let hex = random_hex(24);
        assert_eq!(hex.len(), 48);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(random_hex(24), random_hex(24));
    }

    #[test]
    fn roundtrips_through_json() {
        let cfg = sample();
        let raw = serde_json::to_string(&cfg).unwrap();
        let back: InstanceConfig = serde_json::from_str(&raw).unwrap();
        assert_eq!(cfg, back);
    }
}
