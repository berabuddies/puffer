//! Persistent storage for [`SubscriptionSpec`]s.
//!
//! On-disk format is one JSON file (`subscriptions.json`) holding an
//! ordered array. Reads/writes are guarded by a mutex; writes go through a
//! tempfile + rename for atomicity.

use crate::spec::{validate_spec, SubscriptionSpec, SubscriptionStatus};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

/// Errors that can occur reading or writing subscription state.
#[derive(Debug, Error)]
pub enum SubscriptionStoreError {
    /// Wrapper around any I/O error encountered while reading/writing.
    #[error("subscription store io error: {0}")]
    Io(#[from] std::io::Error),
    /// Wrapper around JSON encode/decode failures.
    #[error("subscription store json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Validation failed for a spec being created or updated.
    #[error("invalid subscription spec: {0}")]
    Invalid(String),
    /// Lookup failed for the supplied id.
    #[error("subscription `{0}` not found")]
    NotFound(String),
    /// Attempted create with an id that already exists.
    #[error("subscription `{0}` already exists")]
    Conflict(String),
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct StoreFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    subscriptions: Vec<SubscriptionSpec>,
}

/// Persistent collection of subscription specs.
pub struct SubscriptionStore {
    path: PathBuf,
    inner: Mutex<StoreFile>,
}

impl SubscriptionStore {
    /// Loads the store at `path`. Missing files are treated as empty.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, SubscriptionStoreError> {
        let path = path.into();
        let inner = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            if raw.trim().is_empty() {
                StoreFile::default()
            } else {
                serde_json::from_str::<StoreFile>(&raw)?
            }
        } else {
            StoreFile::default()
        };
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    /// Returns a snapshot of every spec.
    pub fn list(&self) -> Vec<SubscriptionSpec> {
        self.inner.lock().unwrap().subscriptions.clone()
    }

    /// Returns the spec with the given id, if any.
    pub fn get(&self, id: &str) -> Option<SubscriptionSpec> {
        self.inner
            .lock()
            .unwrap()
            .subscriptions
            .iter()
            .find(|s| s.id == id)
            .cloned()
    }

    /// Inserts a fresh spec. Errors when the id collides or validation fails.
    pub fn create(&self, spec: SubscriptionSpec) -> Result<(), SubscriptionStoreError> {
        validate_spec(&spec).map_err(SubscriptionStoreError::Invalid)?;
        let mut guard = self.inner.lock().unwrap();
        if guard.subscriptions.iter().any(|s| s.id == spec.id) {
            return Err(SubscriptionStoreError::Conflict(spec.id));
        }
        guard.subscriptions.push(spec);
        write_atomic(&self.path, &*guard)
    }

    /// Updates a spec's status. Used by `SubscriptionPause`.
    pub fn set_status(
        &self,
        id: &str,
        status: SubscriptionStatus,
    ) -> Result<SubscriptionSpec, SubscriptionStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard
            .subscriptions
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| SubscriptionStoreError::NotFound(id.to_string()))?;
        entry.status = status;
        let updated = entry.clone();
        write_atomic(&self.path, &*guard)?;
        Ok(updated)
    }

    /// Removes the spec with `id`. Errors when not found.
    pub fn delete(&self, id: &str) -> Result<(), SubscriptionStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let before = guard.subscriptions.len();
        guard.subscriptions.retain(|s| s.id != id);
        if guard.subscriptions.len() == before {
            return Err(SubscriptionStoreError::NotFound(id.to_string()));
        }
        write_atomic(&self.path, &*guard)
    }
}

fn write_atomic(path: &Path, store: &StoreFile) -> Result<(), SubscriptionStoreError> {
    let tmp = path.with_extension("tmp");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_vec_pretty(store)?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ActionSpec, PrefilterSpec};
    use tempfile::tempdir;

    fn sample(id: &str) -> SubscriptionSpec {
        SubscriptionSpec {
            id: id.into(),
            description: "demo".into(),
            source_topic: "telegram-user".into(),
            status: SubscriptionStatus::Enabled,
            prefilter: Some(PrefilterSpec::Regex {
                pattern: r"hello".into(),
                case_insensitive: true,
            }),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::SqliteInsert {
                path: "x.db".into(),
                table: "msgs".into(),
            },
            created_at_ms: 0,
        }
    }

    #[test]
    fn roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("subs.json");
        let store = SubscriptionStore::load(&path).unwrap();
        store.create(sample("a")).unwrap();
        let reopened = SubscriptionStore::load(&path).unwrap();
        let list = reopened.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "a");
    }

    #[test]
    fn pause_persists() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("subs.json");
        let store = SubscriptionStore::load(&path).unwrap();
        store.create(sample("a")).unwrap();
        store.set_status("a", SubscriptionStatus::Paused).unwrap();
        let reopened = SubscriptionStore::load(&path).unwrap();
        assert_eq!(
            reopened.get("a").unwrap().status,
            SubscriptionStatus::Paused
        );
    }
}
