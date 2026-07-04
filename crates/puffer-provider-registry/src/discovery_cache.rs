use crate::model::ModelDescriptor;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Cache TTL: 1 hour.
pub(crate) const CACHE_TTL_MS: u64 = 3_600_000;

const CAPABILITY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DiscoveryCache {
    pub(crate) entries: HashMap<String, DiscoveryCacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DiscoveryCacheEntry {
    pub(crate) models: Vec<ModelDescriptor>,
    pub(crate) cached_at_ms: u64,
    #[serde(default)]
    pub(crate) capability_schema_version: u32,
}

impl DiscoveryCacheEntry {
    /// Creates a cache entry using the current capability schema.
    pub(crate) fn fresh(models: Vec<ModelDescriptor>, cached_at_ms: u64) -> Self {
        Self {
            models,
            cached_at_ms,
            capability_schema_version: CAPABILITY_SCHEMA_VERSION,
        }
    }

    /// Returns whether this entry can be applied at the given timestamp.
    pub(crate) fn is_fresh_at(&self, now_ms: u64) -> bool {
        self.capability_schema_version == CAPABILITY_SCHEMA_VERSION
            && now_ms.saturating_sub(self.cached_at_ms) < CACHE_TTL_MS
    }
}

/// Loads the on-disk cache, applies provider updates, and saves the result.
pub(crate) fn persist_cache_updates(updates: &HashMap<String, DiscoveryCacheEntry>) {
    let path = discovery_cache_path();
    let mut cache = load_discovery_cache(&path).unwrap_or(DiscoveryCache {
        entries: HashMap::new(),
    });
    for (id, entry) in updates {
        cache.entries.insert(id.clone(), entry.clone());
    }
    let _ = save_discovery_cache(&path, &cache);
}

/// Removes a provider's cached discovery entry so its models are re-discovered
/// on the next run. Used when a provider's account/credential changes (e.g.
/// switching GitHub Copilot accounts): the cache is keyed by provider id only,
/// so without this the previous account's entitlement-filtered model list would
/// keep being served until the TTL expires — and a lower-tier account would be
/// offered models it can't use (model_not_supported at chat time).
///
/// NOTE: like `persist_cache_updates`, this is a non-atomic load/modify/save on
/// the shared cache file (no lock), so a concurrent discovery write could
/// briefly resurrect the removed entry. Both callers immediately follow with a
/// re-discovery that re-narrows it, and any stale entry self-corrects at the
/// next discovery or the TTL, so we accept this rather than add file locking to
/// the whole cache path.
pub(crate) fn invalidate_cached_provider(provider_id: &str) {
    let path = discovery_cache_path();
    let Some(mut cache) = load_discovery_cache(&path) else {
        return;
    };
    if cache.entries.remove(provider_id).is_some() {
        let _ = save_discovery_cache(&path, &cache);
    }
}

/// Returns the configured discovery-cache path.
pub(crate) fn discovery_cache_path() -> PathBuf {
    if let Ok(path) = std::env::var("PUFFER_DISCOVERY_CACHE_PATH") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".puffer").join("model_discovery_cache.json")
}

/// Returns the current Unix time in milliseconds.
pub(crate) fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Reads a discovery cache from disk.
pub(crate) fn load_discovery_cache(path: &Path) -> Option<DiscoveryCache> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Writes a discovery cache to disk.
pub(crate) fn save_discovery_cache(path: &Path, cache: &DiscoveryCache) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string(cache)?;
    std::fs::write(path, data)?;
    Ok(())
}
