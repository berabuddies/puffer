//! Historical run storage for connection-triggered workflow bindings.
//!
//! Native AgentFlow workflows persist runs in `puffer-workflow`. Direct
//! connection workflows live in this crate, so their trigger/action history
//! is stored here.

use crate::action::ActionResult;
use crate::spec::{ActionSpec, WorkflowBindingSpec};
use puffer_subscriber_runtime::EventEnvelope;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

/// Status for a recorded direct workflow run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowBindingRunStatus {
    /// The action completed successfully.
    Completed,
    /// The action failed.
    Failed,
}

/// One action log entry for a direct workflow run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowActionLog {
    /// Action kind, for example `connector_act` or `graph`.
    pub action: String,
    /// Action status.
    pub status: WorkflowBindingRunStatus,
    /// Human-readable action summary.
    pub summary: String,
    /// Start timestamp in milliseconds since UNIX epoch.
    pub started_at_ms: i128,
    /// End timestamp in milliseconds since UNIX epoch.
    pub ended_at_ms: i128,
}

/// Persisted run record for a connection-triggered workflow binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowBindingRun {
    /// Global monotonically assigned run index.
    pub idx: u64,
    /// Random run id.
    pub run_id: String,
    /// Workflow binding slug.
    pub workflow_slug: String,
    /// Trigger metadata captured from the connector event.
    pub trigger_info: Value,
    /// Compact action summary for list views.
    pub action_summary: Value,
    /// Per-action log entries.
    pub action_log: Vec<WorkflowActionLog>,
    /// Overall run status.
    pub status: WorkflowBindingRunStatus,
    /// Start timestamp in milliseconds since UNIX epoch.
    pub started_at_ms: i128,
    /// End timestamp in milliseconds since UNIX epoch.
    pub ended_at_ms: i128,
}

/// Errors returned by [`WorkflowHistoryStore`].
#[derive(Debug, Error)]
pub enum WorkflowHistoryStoreError {
    /// I/O failed while reading or writing run state.
    #[error("workflow history store io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON failed to parse or encode.
    #[error("workflow history store json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryFile {
    #[serde(default = "default_next_idx")]
    next_idx: u64,
    #[serde(default)]
    runs: Vec<WorkflowBindingRun>,
}

impl Default for HistoryFile {
    fn default() -> Self {
        Self {
            next_idx: default_next_idx(),
            runs: Vec::new(),
        }
    }
}

fn default_next_idx() -> u64 {
    1
}

/// File-backed store for direct workflow run history.
pub struct WorkflowHistoryStore {
    path: PathBuf,
    inner: Mutex<HistoryFile>,
}

impl WorkflowHistoryStore {
    /// Loads a workflow history store. Missing files are treated as empty.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, WorkflowHistoryStoreError> {
        let path = path.into();
        let inner = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            if raw.trim().is_empty() {
                HistoryFile::default()
            } else {
                serde_json::from_str(&raw)?
            }
        } else {
            HistoryFile::default()
        };
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    /// Appends a direct workflow run produced by a routed connector event.
    pub fn append_action_result(
        &self,
        binding: &WorkflowBindingSpec,
        envelope: &EventEnvelope,
        action: &ActionSpec,
        result: &ActionResult,
        started_at_ms: i128,
        ended_at_ms: i128,
    ) -> Result<WorkflowBindingRun, WorkflowHistoryStoreError> {
        let status = if result.success {
            WorkflowBindingRunStatus::Completed
        } else {
            WorkflowBindingRunStatus::Failed
        };
        let log = WorkflowActionLog {
            action: action_kind(action).to_string(),
            status,
            summary: result.summary.clone(),
            started_at_ms,
            ended_at_ms,
        };
        let run = WorkflowBindingRun {
            idx: 0,
            run_id: Uuid::new_v4().to_string(),
            workflow_slug: binding.slug.clone(),
            trigger_info: trigger_info(binding, envelope),
            action_summary: json!({
                "status": status,
                "action": log.action,
                "summary": result.summary,
            }),
            action_log: vec![log],
            status,
            started_at_ms,
            ended_at_ms,
        };
        self.append(run)
    }

    /// Appends a prebuilt workflow run and assigns its index.
    pub fn append(
        &self,
        mut run: WorkflowBindingRun,
    ) -> Result<WorkflowBindingRun, WorkflowHistoryStoreError> {
        let mut guard = self.inner.lock().unwrap();
        run.idx = guard.next_idx;
        guard.next_idx += 1;
        guard.runs.push(run.clone());
        write_atomic(&self.path, &*guard)?;
        Ok(run)
    }

    /// Returns all direct workflow runs, newest first.
    pub fn list(&self) -> Vec<WorkflowBindingRun> {
        let mut runs = self.inner.lock().unwrap().runs.clone();
        runs.sort_by(|a, b| b.idx.cmp(&a.idx));
        runs
    }

    /// Returns direct workflow runs for one binding slug, newest first.
    pub fn list_for(&self, workflow_slug: &str) -> Vec<WorkflowBindingRun> {
        self.list()
            .into_iter()
            .filter(|run| run.workflow_slug == workflow_slug)
            .collect()
    }

    /// Returns one run by numeric index.
    pub fn get_by_idx(&self, idx: u64) -> Option<WorkflowBindingRun> {
        self.list().into_iter().find(|run| run.idx == idx)
    }

    /// Returns one run by random run id.
    pub fn get_by_run_id(&self, run_id: &str) -> Option<WorkflowBindingRun> {
        self.list().into_iter().find(|run| run.run_id == run_id)
    }
}

fn trigger_info(binding: &WorkflowBindingSpec, envelope: &EventEnvelope) -> Value {
    json!({
        "connection_slug": binding.connection_slug,
        "connector_slug": binding.connector_slug,
        "envelope_id": envelope.envelope_id,
        "received_at_ms": envelope.received_at_ms,
        "topic": envelope.event.topic,
        "kind": envelope.event.kind,
        "dedup_key": envelope.event.dedup_key,
        "text": envelope.event.text,
        "payload": envelope.event.payload,
    })
}

fn action_kind(action: &ActionSpec) -> &'static str {
    match action {
        ActionSpec::SqliteInsert { .. } => "sqlite_insert",
        ActionSpec::FileAppend { .. } => "file_append",
        ActionSpec::ForwardMessage { .. } => "forward_message",
        ActionSpec::RunWorkflow { .. } => "run_workflow",
        ActionSpec::ConnectorAct { .. } => "connector_act",
        ActionSpec::ToolCall { .. } => "tool_call",
        ActionSpec::TriageAgent { .. } => "triage_agent",
        ActionSpec::Graph { .. } => "graph",
        ActionSpec::Unknown => "unknown",
    }
}

fn write_atomic(path: &Path, store: &HistoryFile) -> Result<(), WorkflowHistoryStoreError> {
    let tmp = path.with_extension("tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tmp, serde_json::to_vec_pretty(store)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Returns the current UNIX timestamp in milliseconds.
pub fn now_ms() -> i128 {
    OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::WorkflowBindingStatus;
    use puffer_subscriber_runtime::Event;
    use serde_json::json;

    fn binding() -> WorkflowBindingSpec {
        WorkflowBindingSpec {
            slug: "demo".into(),
            description: "demo".into(),
            connection_slug: "telegram-user".into(),
            connector_slug: Some("telegram-login".into()),
            status: WorkflowBindingStatus::Enabled,
            filter: None,
            ignore_filters: Vec::new(),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::RunWorkflow {
                slug: "native".into(),
            },
            created_at_ms: 0,
        }
    }

    fn envelope() -> EventEnvelope {
        EventEnvelope {
            envelope_id: "env".into(),
            subscriber_id: "telegram-user".into(),
            received_at_ms: 1,
            event: Event {
                topic: "telegram-user".into(),
                kind: "message".into(),
                control: false,
                dedup_key: Some("d".into()),
                text: "gm".into(),
                payload: json!({"from":"Tony"}),
            },
        }
    }

    #[test]
    fn stores_action_result_history() {
        let temp = tempfile::tempdir().unwrap();
        let store = WorkflowHistoryStore::load(temp.path().join("history.json")).unwrap();
        let result = ActionResult {
            success: true,
            summary: "ok".into(),
        };
        let run = store
            .append_action_result(
                &binding(),
                &envelope(),
                &ActionSpec::RunWorkflow {
                    slug: "native".into(),
                },
                &result,
                1,
                2,
            )
            .unwrap();

        assert_eq!(run.idx, 1);
        assert_eq!(run.status, WorkflowBindingRunStatus::Completed);
        assert_eq!(store.list_for("demo").len(), 1);
    }
}
