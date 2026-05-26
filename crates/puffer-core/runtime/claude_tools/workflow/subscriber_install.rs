//! `SubscriberInstall` workflow tool — loads a manifest from disk and
//! starts the subscriber via the running [`SubscriptionManager`].
//!
//! Lookup order: workspace `.puffer/subscribers/<id>/`, user
//! `~/.puffer/subscribers/<id>/`, then the bundled
//! `resources/subscribers/<id>/`. The first directory containing a
//! `manifest.toml` wins.

use crate::AppState;
use anyhow::{anyhow, Context, Result};
use puffer_subscriber_runtime::Manifest;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::subscription_globals;

#[derive(Debug, Deserialize)]
struct InstallInput {
    /// Subscriber id (matches the manifest `id` and the directory name).
    id: String,
}

/// Executes `SubscriberInstall`. Returns the directory used and the
/// manifest's effective topic.
pub fn execute_subscriber_install(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: InstallInput =
        serde_json::from_value(input).context("invalid SubscriberInstall input")?;
    let manager = subscription_globals::manager()?;
    let dir = locate_manifest_dir(&parsed.id).ok_or_else(|| {
        anyhow!(
            "no manifest.toml found for `{}` in workspace, user (~/.puffer/subscribers/{}), or bundled subscribers",
            parsed.id,
            parsed.id
        )
    })?;
    let manifest =
        Manifest::load(&dir).with_context(|| format!("load manifest at {}", dir.display()))?;
    let topic = manifest.topic().to_string();
    let id = manager.start_subscriber(manifest)?;
    Ok(json!({
        "id": id,
        "topic": topic,
        "dir": dir.display().to_string(),
        "next": "Use ConnectionCreate and WorkflowCreate for new connector workflows; SubscriptionCreate remains compatibility-only.",
    })
    .to_string())
}

fn locate_manifest_dir(id: &str) -> Option<PathBuf> {
    let workspace = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join(".puffer").join("subscribers").join(id));
    if let Some(path) = workspace {
        if path.join("manifest.toml").exists() {
            return Some(path);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let user = PathBuf::from(home)
            .join(".puffer")
            .join("subscribers")
            .join(id);
        if user.join("manifest.toml").exists() {
            return Some(user);
        }
    }
    let bundled = PathBuf::from("resources/subscribers").join(id);
    if bundled.join("manifest.toml").exists() {
        return Some(bundled);
    }
    None
}
