use crate::ids::{NodeId, RunId, TraceId};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TraceEventType {
    RunStarted,
    ContractLoaded,
    GoalParsed,
    NodeAdded,
    ModelCandidatesProposed,
    RewriteApplied,
    ActionScored,
    ActionSelected,
    ParallelBatchSelected,
    SafetyBlocked,
    ActionExecuted,
    ObservationAttached,
    FailureClassified,
    RepairAdded,
    VerificationPerformed,
    GoalVerificationPerformed,
    CompletionDeclared,
    RunFinished,
    MutationProposed,
    MutationEvaluated,
    MutationPromoted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TraceEvent {
    pub(crate) trace_id: TraceId,
    pub(crate) run_id: RunId,
    pub(crate) timestamp_ms: u64,
    pub(crate) event_type: TraceEventType,
    pub(crate) plan_node_id: Option<NodeId>,
    pub(crate) contract_id: Option<String>,
    pub(crate) action_name: Option<String>,
    pub(crate) input: Value,
    pub(crate) output: Value,
    pub(crate) success: Option<bool>,
    pub(crate) failure_mode: Option<String>,
    pub(crate) cost: Option<f64>,
    pub(crate) latency_ms: Option<u64>,
    pub(crate) metadata: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct JsonlTraceStore {
    root: PathBuf,
}

impl JsonlTraceStore {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn append(&self, event: &TraceEvent) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create trace dir {}", self.root.display()))?;
        let path = self.path_for_run(event.run_id);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open trace file {}", path.display()))?;
        serde_json::to_writer(&mut file, event)?;
        writeln!(file)?;
        Ok(())
    }

    pub(crate) fn events_for_run(&self, run_id: RunId) -> Result<Vec<TraceEvent>> {
        let path = self.path_for_run(run_id);
        read_trace_file(&path)
    }

    pub(crate) fn graph_snapshot_path_for_run(&self, run_id: RunId) -> PathBuf {
        self.root
            .parent()
            .unwrap_or(&self.root)
            .join("graphs")
            .join(format!("{}.json", run_id.0))
    }

    pub(crate) fn list_runs(&self) -> Result<Vec<RunId>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut runs = Vec::new();
        for entry in fs::read_dir(&self.root)
            .with_context(|| format!("failed to read {}", self.root.display()))?
        {
            let path = entry?.path();
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(uuid) = uuid::Uuid::parse_str(stem) {
                runs.push(RunId(uuid));
            }
        }
        runs.sort_by_key(|run| run.0);
        Ok(runs)
    }

    fn path_for_run(&self, run_id: RunId) -> PathBuf {
        self.root.join(format!("{}.jsonl", run_id.0))
    }
}

pub(crate) fn trace_event(
    run_id: RunId,
    event_type: TraceEventType,
    plan_node_id: Option<NodeId>,
    input: Value,
    output: Value,
) -> TraceEvent {
    TraceEvent {
        trace_id: TraceId::new(),
        run_id,
        timestamp_ms: now_ms(),
        event_type,
        plan_node_id,
        contract_id: None,
        action_name: None,
        input,
        output,
        success: None,
        failure_mode: None,
        cost: None,
        latency_ms: None,
        metadata: Value::Null,
    }
}

pub(crate) fn read_trace_file(path: &Path) -> Result<Vec<TraceEvent>> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to open trace file {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        events.push(serde_json::from_str(&line)?);
    }
    Ok(events)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jsonl_trace_store_round_trips_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlTraceStore::new(dir.path().to_path_buf());
        let run_id = RunId::new();
        let event = trace_event(
            run_id,
            TraceEventType::RunStarted,
            None,
            json!({"goal": "x"}),
            json!({}),
        );
        store.append(&event).unwrap();
        let loaded = store.events_for_run(run_id).unwrap();
        assert_eq!(loaded[0].event_type, TraceEventType::RunStarted);
        assert_eq!(store.list_runs().unwrap(), vec![run_id]);
    }
}
