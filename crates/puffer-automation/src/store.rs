//! Persistent storage for Automation records.
//!
//! The store owns record metadata and runtime initialization. Product callers
//! provide user semantics; compiler/runner callers provide runtime artifacts.

use crate::{
    automation_spec_hash, validate_automation_record, AutomationRecord, AutomationRuntimeState,
    AutomationRuntimeStatus, AutomationSpec, AutomationStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Errors that can occur reading or writing Automation records.
#[derive(Debug, Error)]
pub enum AutomationStoreError {
    /// Wrapper around any I/O error encountered while reading/writing.
    #[error("automation store io error: {0}")]
    Io(#[from] std::io::Error),
    /// Wrapper around JSON encode/decode failures.
    #[error("automation store json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Validation failed for a record being created or updated.
    #[error("invalid automation record: {0}")]
    Invalid(String),
    /// Lookup failed for the supplied id.
    #[error("automation `{0}` not found")]
    NotFound(String),
    /// Attempted create with an id that already exists.
    #[error("automation `{0}` already exists")]
    Conflict(String),
    /// Attempted update from a stale caller revision.
    #[error("automation `{id}` revision conflict: expected {expected}, found {found}")]
    RevisionConflict {
        /// Automation id.
        id: String,
        /// Caller-supplied revision.
        expected: u64,
        /// Current persisted revision.
        found: u64,
    },
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct StoreFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    records: Vec<AutomationRecord>,
}

/// Persistent collection of user-facing Automations.
pub struct AutomationStore {
    path: PathBuf,
    inner: Mutex<StoreFile>,
}

impl AutomationStore {
    /// Loads the store at `path`. Missing files are treated as empty.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, AutomationStoreError> {
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
        let mut ids = BTreeSet::new();
        for record in &inner.records {
            validate_automation_record(record).map_err(AutomationStoreError::Invalid)?;
            if !ids.insert(record.id.as_str()) {
                return Err(AutomationStoreError::Conflict(record.id.clone()));
            }
        }
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    /// Returns a snapshot of every Automation record.
    pub fn list(&self) -> Vec<AutomationRecord> {
        self.inner.lock().unwrap().records.clone()
    }

    /// Returns one Automation record.
    pub fn get(&self, id: &str) -> Result<AutomationRecord, AutomationStoreError> {
        self.inner
            .lock()
            .unwrap()
            .records
            .iter()
            .find(|record| record.id == id)
            .cloned()
            .ok_or_else(|| AutomationStoreError::NotFound(id.to_string()))
    }

    /// Creates a new Automation record from user-authored semantics.
    pub fn create(
        &self,
        id: impl Into<String>,
        spec: AutomationSpec,
        status: AutomationStatus,
    ) -> Result<AutomationRecord, AutomationStoreError> {
        let id = id.into();
        let now = now_ms();
        let record = AutomationRecord {
            id,
            status,
            revision: 1,
            spec,
            runtime: AutomationRuntimeState::default(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        validate_automation_record(&record).map_err(AutomationStoreError::Invalid)?;

        let mut guard = self.inner.lock().unwrap();
        if guard
            .records
            .iter()
            .any(|existing| existing.id == record.id)
        {
            return Err(AutomationStoreError::Conflict(record.id));
        }
        guard.records.push(record.clone());
        guard.records.sort_by(|a, b| a.id.cmp(&b.id));
        write_atomic(&self.path, &*guard)?;
        Ok(record)
    }

    /// Saves a user-authored spec after optimistic revision checking.
    pub fn save_spec(
        &self,
        id: &str,
        expected_revision: u64,
        spec: AutomationSpec,
    ) -> Result<AutomationRecord, AutomationStoreError> {
        validate_automation_record(&AutomationRecord {
            id: id.to_string(),
            status: AutomationStatus::Paused,
            revision: expected_revision.max(1),
            spec: spec.clone(),
            runtime: AutomationRuntimeState::default(),
            created_at_ms: 0,
            updated_at_ms: 0,
        })
        .map_err(AutomationStoreError::Invalid)?;

        let mut guard = self.inner.lock().unwrap();
        let entry = find_record_mut(&mut guard, id)?;
        ensure_revision(id, expected_revision, entry.revision)?;

        let existing_hash =
            automation_spec_hash(&entry.spec).map_err(AutomationStoreError::Invalid)?;
        let new_hash = automation_spec_hash(&spec).map_err(AutomationStoreError::Invalid)?;

        if existing_hash != new_hash {
            entry.revision += 1;
            entry.spec = spec;
            mark_runtime_stale_for_spec_change(&mut entry.runtime);
        }
        entry.updated_at_ms = now_ms();

        validate_automation_record(entry).map_err(AutomationStoreError::Invalid)?;
        let updated = entry.clone();
        write_atomic(&self.path, &*guard)?;
        Ok(updated)
    }

    /// Updates user-visible status without touching the runtime artifacts.
    pub fn set_status(
        &self,
        id: &str,
        status: AutomationStatus,
    ) -> Result<AutomationRecord, AutomationStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let entry = find_record_mut(&mut guard, id)?;
        entry.status = status;
        entry.updated_at_ms = now_ms();

        validate_automation_record(entry).map_err(AutomationStoreError::Invalid)?;
        let updated = entry.clone();
        write_atomic(&self.path, &*guard)?;
        Ok(updated)
    }

    /// Archives the Automation.
    pub fn archive(&self, id: &str) -> Result<AutomationRecord, AutomationStoreError> {
        self.set_status(id, AutomationStatus::Archived)
    }

    /// Removes the Automation. Errors when not found.
    pub fn delete(&self, id: &str) -> Result<(), AutomationStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let before = guard.records.len();
        guard.records.retain(|record| record.id != id);
        if guard.records.len() == before {
            return Err(AutomationStoreError::NotFound(id.to_string()));
        }
        write_atomic(&self.path, &*guard)
    }

    /// Replaces internal runtime artifacts after a successful compile/deploy.
    pub fn replace_runtime(
        &self,
        id: &str,
        expected_revision: u64,
        runtime: AutomationRuntimeState,
    ) -> Result<AutomationRecord, AutomationStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let entry = find_record_mut(&mut guard, id)?;
        ensure_revision(id, expected_revision, entry.revision)?;

        let expected_hash =
            automation_spec_hash(&entry.spec).map_err(AutomationStoreError::Invalid)?;
        if runtime.compiled_revision != Some(entry.revision) {
            return Err(AutomationStoreError::Invalid(format!(
                "automation runtime compiled_revision must be Some({})",
                entry.revision
            )));
        }
        if runtime.spec_hash.as_deref() != Some(expected_hash.as_str()) {
            return Err(AutomationStoreError::Invalid(format!(
                "automation runtime spec_hash must be `{expected_hash}`"
            )));
        }

        entry.runtime = runtime;
        entry.updated_at_ms = now_ms();

        validate_automation_record(entry).map_err(AutomationStoreError::Invalid)?;
        let updated = entry.clone();
        write_atomic(&self.path, &*guard)?;
        Ok(updated)
    }

    /// Records a compile/deploy failure without marking the runtime as synced.
    pub fn replace_runtime_error(
        &self,
        id: &str,
        expected_revision: u64,
        last_error: impl Into<String>,
    ) -> Result<AutomationRecord, AutomationStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let entry = find_record_mut(&mut guard, id)?;
        ensure_revision(id, expected_revision, entry.revision)?;

        entry.runtime = AutomationRuntimeState {
            status: AutomationRuntimeStatus::Error,
            last_error: Some(last_error.into()),
            ..AutomationRuntimeState::default()
        };
        entry.updated_at_ms = now_ms();

        validate_automation_record(entry).map_err(AutomationStoreError::Invalid)?;
        let updated = entry.clone();
        write_atomic(&self.path, &*guard)?;
        Ok(updated)
    }
}

fn find_record_mut<'a>(
    store: &'a mut StoreFile,
    id: &str,
) -> Result<&'a mut AutomationRecord, AutomationStoreError> {
    store
        .records
        .iter_mut()
        .find(|record| record.id == id)
        .ok_or_else(|| AutomationStoreError::NotFound(id.to_string()))
}

fn ensure_revision(id: &str, expected: u64, found: u64) -> Result<(), AutomationStoreError> {
    if expected != found {
        return Err(AutomationStoreError::RevisionConflict {
            id: id.to_string(),
            expected,
            found,
        });
    }
    Ok(())
}

fn mark_runtime_stale_for_spec_change(runtime: &mut AutomationRuntimeState) {
    let has_artifacts =
        !runtime.agentenv_workflows.is_empty() || !runtime.puffer_bindings.is_empty();
    runtime.status = if has_artifacts {
        AutomationRuntimeStatus::Stale
    } else {
        AutomationRuntimeStatus::NotCompiled
    };
    runtime.agentenv_workflows.clear();
    runtime.puffer_bindings.clear();
    runtime.last_error = None;
}

fn write_atomic(path: &Path, store: &StoreFile) -> Result<(), AutomationStoreError> {
    let tmp = path.with_extension("tmp");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_vec_pretty(store)?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn now_ms() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i128)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentEnvNodeRef, AutomationFlowSpec, AutomationReviewSpec, AutomationRuntimeStatus,
        AutomationSource, AutomationStepSpec, AutomationTriggerSpec, CompiledAgentEnvWorkflow,
        CompiledPufferBinding, CompiledWorkflowRole, AUTOMATION_SPEC_VERSION,
    };
    use serde_json::Value;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn agentenv_node(node_type: &str) -> AgentEnvNodeRef {
        AgentEnvNodeRef {
            node_type: node_type.into(),
            name: Some(node_type.into()),
            trusted: Some(true),
            config: BTreeMap::new(),
        }
    }

    fn sample_spec() -> AutomationSpec {
        AutomationSpec {
            spec_version: AUTOMATION_SPEC_VERSION,
            name: "Reply helper".into(),
            description: None,
            source: AutomationSource::Blank,
            instructions: "Draft a reply for review.".into(),
            run_location: Default::default(),
            triggers: vec![AutomationTriggerSpec::PufferConnection {
                id: "incoming".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: Some("telegram-login".into()),
                filter: None,
                ignore_filters: Vec::new(),
                contact_ids: Vec::new(),
                summary: None,
            }],
            flow: AutomationFlowSpec {
                steps: vec![AutomationStepSpec::AgentEnvNode {
                    id: "draft".into(),
                    node: agentenv_node("transform_js"),
                    summary: None,
                }],
            },
            review: AutomationReviewSpec::default(),
        }
    }

    fn puffer_agent_spec_with_runtime_config(key: &str) -> AutomationSpec {
        let mut agent = agentenv_node("puffer_agent");
        agent
            .config
            .insert(key.into(), Value::String("runtime-value".into()));
        let mut spec = sample_spec();
        spec.flow.steps = vec![AutomationStepSpec::AgentEnvNode {
            id: "agent".into(),
            node: agent,
            summary: Some("Run the Agent".into()),
        }];
        spec
    }

    fn load_store(path: &Path) -> AutomationStore {
        AutomationStore::load(path).unwrap()
    }

    #[test]
    fn create_initializes_metadata_and_roundtrips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("automations.json");
        let store = load_store(&path);

        let record = store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();

        assert_eq!(record.revision, 1);
        assert_eq!(record.runtime.status, AutomationRuntimeStatus::NotCompiled);
        assert_eq!(record.runtime.spec_hash, None);
        assert_eq!(record.runtime.compiled_revision, None);

        let reopened = load_store(&path);
        assert_eq!(reopened.list(), vec![record.clone()]);
        assert_eq!(reopened.get("reply-helper").unwrap(), record);
    }

    #[test]
    fn create_rejects_duplicate_ids() {
        let dir = tempdir().unwrap();
        let store = load_store(&dir.path().join("automations.json"));

        store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();
        let error = store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap_err();

        assert!(matches!(error, AutomationStoreError::Conflict(_)));
    }

    #[test]
    fn create_rejects_puffer_agent_runtime_config() {
        let dir = tempdir().unwrap();
        let store = load_store(&dir.path().join("automations.json"));

        let error = store
            .create(
                "reply-helper",
                puffer_agent_spec_with_runtime_config("content"),
                AutomationStatus::Paused,
            )
            .unwrap_err();

        match error {
            AutomationStoreError::Invalid(message) => {
                assert!(message.contains("puffer_agent"));
                assert!(message.contains("content"));
                assert!(message.contains("persisted product semantics"));
            }
            other => panic!("expected invalid automation record, got {other:?}"),
        }
    }

    #[test]
    fn load_rejects_legacy_puffer_agent_runtime_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("automations.json");
        let record = AutomationRecord {
            id: "reply-helper".into(),
            status: AutomationStatus::Paused,
            revision: 1,
            spec: puffer_agent_spec_with_runtime_config("agentId"),
            runtime: AutomationRuntimeState::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        let body = serde_json::to_vec_pretty(&StoreFile {
            version: 0,
            records: vec![record],
        })
        .unwrap();
        std::fs::write(&path, body).unwrap();

        let error = match AutomationStore::load(&path) {
            Ok(_) => panic!("expected invalid puffer_agent runtime config load failure"),
            Err(error) => error,
        };

        match error {
            AutomationStoreError::Invalid(message) => {
                assert!(message.contains("puffer_agent"));
                assert!(message.contains("agentId"));
                assert!(message.contains("persisted product semantics"));
            }
            other => panic!("expected invalid automation record, got {other:?}"),
        }
    }

    #[test]
    fn load_rejects_duplicate_ids() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("automations.json");
        let store = load_store(&path);
        let record = store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();
        let body = serde_json::to_vec_pretty(&StoreFile {
            version: 0,
            records: vec![record.clone(), record],
        })
        .unwrap();
        std::fs::write(&path, body).unwrap();

        let error = match AutomationStore::load(&path) {
            Ok(_) => panic!("expected duplicate id load failure"),
            Err(error) => error,
        };

        assert!(matches!(error, AutomationStoreError::Conflict(_)));
    }

    #[test]
    fn save_spec_uses_expected_revision_and_stales_existing_runtime() {
        let dir = tempdir().unwrap();
        let store = load_store(&dir.path().join("automations.json"));
        let record = store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();
        let runtime = AutomationRuntimeState {
            spec_hash: Some(automation_spec_hash(&record.spec).unwrap()),
            compiled_revision: Some(record.revision),
            status: AutomationRuntimeStatus::Deployed,
            agentenv_workflows: vec![CompiledAgentEnvWorkflow {
                role: CompiledWorkflowRole::Root,
                workflow_id: Some("Wf_01HX.Runtime:123".into()),
                definition_hash: None,
                deployed: true,
            }],
            puffer_bindings: vec![CompiledPufferBinding {
                trigger_id: "incoming".into(),
                binding_slug: "reply-helper-binding".into(),
            }],
            last_error: Some("old compile error".into()),
        };
        store
            .replace_runtime("reply-helper", record.revision, runtime)
            .unwrap();

        let mut updated_spec = sample_spec();
        updated_spec.instructions = "Draft a shorter reply for review.".into();
        let updated = store
            .save_spec("reply-helper", record.revision, updated_spec)
            .unwrap();

        assert_eq!(updated.revision, 2);
        assert_eq!(updated.runtime.status, AutomationRuntimeStatus::Stale);
        assert_eq!(updated.runtime.last_error, None);
        assert!(updated.runtime.spec_hash.is_some());
        assert!(updated.runtime.agentenv_workflows.is_empty());
        assert!(updated.runtime.puffer_bindings.is_empty());

        let error = store
            .save_spec("reply-helper", record.revision, sample_spec())
            .unwrap_err();
        assert!(matches!(
            error,
            AutomationStoreError::RevisionConflict { .. }
        ));
    }

    #[test]
    fn save_spec_without_hash_change_keeps_revision() {
        let dir = tempdir().unwrap();
        let store = load_store(&dir.path().join("automations.json"));
        let record = store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();

        let updated = store
            .save_spec("reply-helper", record.revision, record.spec.clone())
            .unwrap();

        assert_eq!(updated.revision, record.revision);
    }

    #[test]
    fn save_spec_without_runtime_artifacts_keeps_runtime_not_compiled() {
        let dir = tempdir().unwrap();
        let store = load_store(&dir.path().join("automations.json"));
        let record = store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();

        let mut updated_spec = record.spec.clone();
        updated_spec.instructions = "Draft a shorter reply for review.".into();
        let updated = store
            .save_spec("reply-helper", record.revision, updated_spec)
            .unwrap();

        assert_eq!(updated.runtime.status, AutomationRuntimeStatus::NotCompiled);
        assert_eq!(updated.runtime.spec_hash, None);
    }

    #[test]
    fn save_spec_rejects_puffer_agent_runtime_config() {
        let dir = tempdir().unwrap();
        let store = load_store(&dir.path().join("automations.json"));
        let record = store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();

        let error = store
            .save_spec(
                "reply-helper",
                record.revision,
                puffer_agent_spec_with_runtime_config("timeoutSeconds"),
            )
            .unwrap_err();

        match error {
            AutomationStoreError::Invalid(message) => {
                assert!(message.contains("puffer_agent"));
                assert!(message.contains("timeoutSeconds"));
                assert!(message.contains("persisted product semantics"));
            }
            other => panic!("expected invalid automation record, got {other:?}"),
        }
    }

    #[test]
    fn save_spec_can_delete_referenced_trigger_without_persisting_orphan_bindings() {
        let dir = tempdir().unwrap();
        let store = load_store(&dir.path().join("automations.json"));
        let record = store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();
        let runtime = AutomationRuntimeState {
            spec_hash: Some(automation_spec_hash(&record.spec).unwrap()),
            compiled_revision: Some(record.revision),
            status: AutomationRuntimeStatus::Deployed,
            agentenv_workflows: Vec::new(),
            puffer_bindings: vec![CompiledPufferBinding {
                trigger_id: "incoming".into(),
                binding_slug: "reply-helper-binding".into(),
            }],
            last_error: None,
        };
        store
            .replace_runtime("reply-helper", record.revision, runtime)
            .unwrap();

        let mut updated_spec = sample_spec();
        updated_spec.triggers = vec![AutomationTriggerSpec::AgentEnvNode {
            id: "webhook".into(),
            node: agentenv_node("webhook"),
            summary: None,
        }];
        let updated = store
            .save_spec("reply-helper", record.revision, updated_spec)
            .unwrap();

        assert_eq!(updated.runtime.status, AutomationRuntimeStatus::Stale);
        assert!(updated.runtime.puffer_bindings.is_empty());
    }

    #[test]
    fn save_spec_rejects_puffer_agent_trigger() {
        let dir = tempdir().unwrap();
        let store = load_store(&dir.path().join("automations.json"));
        let record = store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();

        let mut updated_spec = sample_spec();
        updated_spec.triggers = vec![AutomationTriggerSpec::AgentEnvNode {
            id: "agent".into(),
            node: agentenv_node("puffer_agent"),
            summary: Some("Run the Agent".into()),
        }];

        let error = store
            .save_spec("reply-helper", record.revision, updated_spec)
            .unwrap_err();

        match error {
            AutomationStoreError::Invalid(message) => {
                assert!(message.contains("puffer_agent"));
                assert!(message.contains("execution step"));
            }
            other => panic!("expected invalid automation record, got {other:?}"),
        }
    }

    #[test]
    fn replace_runtime_rejects_revision_and_hash_mismatches() {
        let dir = tempdir().unwrap();
        let store = load_store(&dir.path().join("automations.json"));
        let record = store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();

        let runtime = AutomationRuntimeState {
            spec_hash: Some(
                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            ),
            compiled_revision: Some(record.revision),
            status: AutomationRuntimeStatus::DraftSynced,
            agentenv_workflows: Vec::new(),
            puffer_bindings: Vec::new(),
            last_error: None,
        };

        let error = store
            .replace_runtime("reply-helper", record.revision + 1, runtime.clone())
            .unwrap_err();
        assert!(matches!(
            error,
            AutomationStoreError::RevisionConflict { .. }
        ));

        let error = store
            .replace_runtime("reply-helper", record.revision, runtime)
            .unwrap_err();
        assert!(matches!(error, AutomationStoreError::Invalid(_)));
    }

    #[test]
    fn replace_runtime_rejects_orphan_artifact_references() {
        let dir = tempdir().unwrap();
        let store = load_store(&dir.path().join("automations.json"));
        let record = store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();
        let runtime = AutomationRuntimeState {
            spec_hash: Some(automation_spec_hash(&record.spec).unwrap()),
            compiled_revision: Some(record.revision),
            status: AutomationRuntimeStatus::DraftSynced,
            agentenv_workflows: Vec::new(),
            puffer_bindings: vec![CompiledPufferBinding {
                trigger_id: "missing".into(),
                binding_slug: "reply-helper-binding".into(),
            }],
            last_error: None,
        };

        let error = store
            .replace_runtime("reply-helper", record.revision, runtime)
            .unwrap_err();

        assert!(matches!(error, AutomationStoreError::Invalid(_)));
    }

    #[test]
    fn set_status_archive_delete_and_not_found() {
        let dir = tempdir().unwrap();
        let store = load_store(&dir.path().join("automations.json"));
        store
            .create("reply-helper", sample_spec(), AutomationStatus::Paused)
            .unwrap();

        let enabled = store
            .set_status("reply-helper", AutomationStatus::Enabled)
            .unwrap();
        assert_eq!(enabled.status, AutomationStatus::Enabled);

        let archived = store.archive("reply-helper").unwrap();
        assert_eq!(archived.status, AutomationStatus::Archived);

        store.delete("reply-helper").unwrap();
        assert!(matches!(
            store.get("reply-helper").unwrap_err(),
            AutomationStoreError::NotFound(_)
        ));
    }
}
