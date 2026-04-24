//! `SubscriberList` workflow tool — enumerates subscriber manifests
//! discoverable by Puffer (workspace, user, bundled) and the subset
//! currently running in the process.

use crate::AppState;
use anyhow::Result;
use puffer_subscriber_runtime::Manifest;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::subscription_globals;

#[derive(Debug, Serialize)]
struct DiscoveredSubscriber {
    id: String,
    topic: String,
    display_name: Option<String>,
    source: &'static str,
    dir: String,
    running: bool,
}

/// Executes `SubscriberList`. Returns a JSON array of discovered
/// subscribers with their on-disk source and live status.
pub fn execute_subscriber_list(
    _state: &mut AppState,
    _cwd: &Path,
    _input: Value,
) -> Result<String> {
    let running = match subscription_globals::manager() {
        Ok(m) => m.subscriber_ids(),
        Err(_) => Vec::new(),
    };
    let mut by_id: BTreeMap<String, DiscoveredSubscriber> = BTreeMap::new();
    // Order matters: bundled first, then user, then workspace, so later
    // entries overwrite earlier ones (workspace has the highest priority,
    // matching the broader Puffer resource layering rules).
    for (root, source) in roots() {
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let dir = entry.path();
                if !dir.is_dir() || !dir.join("manifest.toml").exists() {
                    continue;
                }
                if let Ok(manifest) = Manifest::load(&dir) {
                    let id = manifest.spec.id.clone();
                    by_id.insert(
                        id.clone(),
                        DiscoveredSubscriber {
                            running: running.iter().any(|x| x == &id),
                            id,
                            topic: manifest.topic().to_string(),
                            display_name: manifest.spec.display_name.clone(),
                            source,
                            dir: dir.display().to_string(),
                        },
                    );
                }
            }
        }
    }
    let result: Vec<DiscoveredSubscriber> = by_id.into_values().collect();
    Ok(serde_json::to_string_pretty(&result)?)
}

fn roots() -> Vec<(PathBuf, &'static str)> {
    let mut out: Vec<(PathBuf, &'static str)> = Vec::new();
    out.push((PathBuf::from("resources/subscribers"), "bundled"));
    if let Some(home) = std::env::var_os("HOME") {
        out.push((
            PathBuf::from(home).join(".puffer").join("subscribers"),
            "user",
        ));
    }
    if let Ok(cwd) = std::env::current_dir() {
        out.push((cwd.join(".puffer").join("subscribers"), "workspace"));
    }
    out
}
