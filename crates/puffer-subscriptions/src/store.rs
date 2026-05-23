//! Persistent storage for [`WorkflowBindingSpec`]s.
//!
//! On-disk format is one JSON file (`workflow_bindings.json`) holding an
//! ordered array. Reads/writes are guarded by a mutex; writes go through a
//! tempfile + rename for atomicity.

use crate::spec::{validate_workflow_binding, WorkflowBindingSpec, WorkflowBindingStatus};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

/// Backwards-compatible alias for older callers.
pub type SubscriptionStore = WorkflowBindingStore;

/// Backwards-compatible alias for older callers.
pub type SubscriptionStoreError = WorkflowBindingStoreError;

/// Errors that can occur reading or writing workflow binding state.
#[derive(Debug, Error)]
pub enum WorkflowBindingStoreError {
    /// Wrapper around any I/O error encountered while reading/writing.
    #[error("workflow binding store io error: {0}")]
    Io(#[from] std::io::Error),
    /// Wrapper around JSON encode/decode failures.
    #[error("workflow binding store json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Validation failed for a spec being created or updated.
    #[error("invalid workflow binding spec: {0}")]
    Invalid(String),
    /// Lookup failed for the supplied id.
    #[error("workflow binding `{0}` not found")]
    NotFound(String),
    /// Attempted create with an id that already exists.
    #[error("workflow binding `{0}` already exists")]
    Conflict(String),
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct StoreFile {
    #[serde(default)]
    version: u32,
    #[serde(default, alias = "subscriptions")]
    bindings: Vec<WorkflowBindingSpec>,
}

/// Persistent collection of workflow trigger bindings.
pub struct WorkflowBindingStore {
    path: PathBuf,
    inner: Mutex<StoreFile>,
}

impl WorkflowBindingStore {
    /// Loads the store at `path`. Missing files are treated as empty.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, WorkflowBindingStoreError> {
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
    pub fn list(&self) -> Vec<WorkflowBindingSpec> {
        self.inner.lock().unwrap().bindings.clone()
    }

    /// Returns the spec with the given id, if any.
    pub fn get(&self, id: &str) -> Option<WorkflowBindingSpec> {
        self.inner
            .lock()
            .unwrap()
            .bindings
            .iter()
            .find(|s| s.slug == id)
            .cloned()
    }

    /// Inserts a fresh spec. Errors when the id collides or validation fails.
    pub fn create(&self, spec: WorkflowBindingSpec) -> Result<(), WorkflowBindingStoreError> {
        validate_workflow_binding(&spec).map_err(WorkflowBindingStoreError::Invalid)?;
        let mut guard = self.inner.lock().unwrap();
        if guard.bindings.iter().any(|s| s.slug == spec.slug) {
            return Err(WorkflowBindingStoreError::Conflict(spec.slug));
        }
        guard.bindings.push(spec);
        write_atomic(&self.path, &*guard)
    }

    /// Inserts or replaces a workflow binding with the same slug.
    pub fn upsert(
        &self,
        spec: WorkflowBindingSpec,
    ) -> Result<WorkflowBindingSpec, WorkflowBindingStoreError> {
        validate_workflow_binding(&spec).map_err(WorkflowBindingStoreError::Invalid)?;
        let mut guard = self.inner.lock().unwrap();
        guard.bindings.retain(|existing| existing.slug != spec.slug);
        guard.bindings.push(spec.clone());
        guard.bindings.sort_by(|a, b| a.slug.cmp(&b.slug));
        write_atomic(&self.path, &*guard)?;
        Ok(spec)
    }

    /// Updates a spec's status. Used by workflow toggles and legacy pause.
    pub fn set_status(
        &self,
        id: &str,
        status: WorkflowBindingStatus,
    ) -> Result<WorkflowBindingSpec, WorkflowBindingStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard
            .bindings
            .iter_mut()
            .find(|s| s.slug == id)
            .ok_or_else(|| WorkflowBindingStoreError::NotFound(id.to_string()))?;
        entry.status = status;
        let updated = entry.clone();
        write_atomic(&self.path, &*guard)?;
        Ok(updated)
    }

    /// Removes the spec with `id`. Errors when not found.
    pub fn delete(&self, id: &str) -> Result<(), WorkflowBindingStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let before = guard.bindings.len();
        guard.bindings.retain(|s| s.slug != id);
        if guard.bindings.len() == before {
            return Err(WorkflowBindingStoreError::NotFound(id.to_string()));
        }
        write_atomic(&self.path, &*guard)
    }
}

fn write_atomic(path: &Path, store: &StoreFile) -> Result<(), WorkflowBindingStoreError> {
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
    use crate::spec::{ActionSpec, FilterSpec, TaggedFilterSpec};
    use tempfile::tempdir;

    fn sample(id: &str) -> WorkflowBindingSpec {
        WorkflowBindingSpec {
            slug: id.into(),
            description: "demo".into(),
            connection_slug: "telegram-user".into(),
            connector_slug: Some("telegram-login".into()),
            status: WorkflowBindingStatus::Enabled,
            filter: Some(FilterSpec::Tagged(TaggedFilterSpec::Regex {
                pattern: r"hello".into(),
                case_insensitive: true,
            })),
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
        assert_eq!(list[0].slug, "a");
    }

    #[test]
    fn pause_persists() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("subs.json");
        let store = SubscriptionStore::load(&path).unwrap();
        store.create(sample("a")).unwrap();
        store
            .set_status("a", WorkflowBindingStatus::Paused)
            .unwrap();
        let reopened = SubscriptionStore::load(&path).unwrap();
        assert_eq!(
            reopened.get("a").unwrap().status,
            WorkflowBindingStatus::Paused
        );
    }

    #[test]
    fn legacy_subscriptions_key_loads_as_bindings() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("subs.json");
        std::fs::write(
            &path,
            r#"{"subscriptions":[{"id":"a","source_topic":"telegram-user","action":{"type":"run_workflow","slug":"demo"}}]}"#,
        )
        .unwrap();

        let reopened = SubscriptionStore::load(&path).unwrap();
        let list = reopened.list();
        assert_eq!(list[0].slug, "a");
        assert_eq!(list[0].connection_slug, "telegram-user");
    }
}
