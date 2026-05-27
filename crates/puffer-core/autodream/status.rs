//! Persisted AutoDream scheduler and run-status helpers.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Scheduler state persisted between AutoDream background runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct AutoDreamStateFile {
    /// Timestamp of the last successful automatic consolidation.
    #[serde(default)]
    pub last_consolidated_at_ms: u64,
    /// Timestamp of the last session scan used by the scheduler gates.
    #[serde(default)]
    pub last_session_scan_at_ms: u64,
}

/// Structured summary of one Memory tool mutation made by AutoDream.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoDreamMemoryChange {
    /// Memory action requested by the side turn.
    pub action: String,
    /// Short sanitized new content, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Short sanitized replaced or removed text, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_text: Option<String>,
    /// Whether the Memory tool reported success.
    pub success: bool,
    /// Short sanitized Memory tool message or error output.
    pub message: String,
}

/// Reviewable GenSkill suggestion produced by a positive AutoDream run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutoDreamGenskillSuggestion {
    /// Stable suggestion identifier.
    pub id: String,
    /// Creation timestamp in Unix milliseconds.
    pub created_at_ms: u64,
    /// Short sanitized rationale from the AutoDream final response.
    pub rationale: String,
    /// Number of Memory tool mutations observed in the same pass.
    pub memory_changes: usize,
    /// Current review status.
    pub status: String,
}

/// Diagnostic status for the latest AutoDream background run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct AutoDreamRunStatusFile {
    /// Current run status: running, completed, failed, or skipped.
    pub status: String,
    /// Start timestamp in Unix milliseconds.
    pub started_at_ms: u64,
    /// Last update timestamp in Unix milliseconds.
    pub updated_at_ms: u64,
    /// Number of recent sessions packed as supporting context.
    pub sessions_reviewed: usize,
    /// Number of tool calls made by the side turn.
    pub tool_calls: usize,
    /// Whether the side turn produced a positive GenSkill marker.
    pub genskill_suggested: bool,
    /// Short sanitized final summary or status text.
    pub summary: String,
    /// Short sanitized error text for failed runs.
    pub error: Option<String>,
    /// Structured Memory tool changes from the run.
    #[serde(default)]
    pub memory_changes: Vec<AutoDreamMemoryChange>,
    /// Reviewable GenSkill suggestion queued by the run, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genskill_suggestion: Option<AutoDreamGenskillSuggestion>,
}

/// Reads persisted AutoDream scheduler state.
pub(super) fn read_autodream_state(session_root: &Path) -> Result<AutoDreamStateFile> {
    let path = autodream_state_path(session_root);
    if !path.exists() {
        return Ok(AutoDreamStateFile::default());
    }
    let bytes = fs::read(&path)
        .with_context(|| format!("failed to read AutoDream state {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse AutoDream state {}", path.display()))
}

/// Writes persisted AutoDream scheduler state.
pub(super) fn write_autodream_state(session_root: &Path, state: &AutoDreamStateFile) -> Result<()> {
    let path = autodream_state_path(session_root);
    write_json_file(&path, state, "AutoDream state")
}

/// Reads the latest persisted AutoDream run status.
pub(super) fn read_autodream_run_status(
    session_root: &Path,
) -> Result<Option<AutoDreamRunStatusFile>> {
    let path = autodream_run_status_path(session_root);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path)
        .with_context(|| format!("failed to read AutoDream run status {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .with_context(|| format!("failed to parse AutoDream run status {}", path.display()))
}

/// Writes the latest persisted AutoDream run status.
pub(super) fn write_autodream_run_status(
    session_root: &Path,
    status: &AutoDreamRunStatusFile,
) -> Result<()> {
    let path = autodream_run_status_path(session_root);
    write_json_file(&path, status, "AutoDream run status")
}

/// Returns the shared AutoDream metadata directory for a session store root.
pub(super) fn autodream_dir(session_root: &Path) -> PathBuf {
    session_root
        .parent()
        .unwrap_or(session_root)
        .join("autodream")
}

fn autodream_state_path(session_root: &Path) -> PathBuf {
    autodream_dir(session_root).join("state.json")
}

fn autodream_run_status_path(session_root: &Path) -> PathBuf {
    autodream_dir(session_root).join("run_status.json")
}

fn write_json_file<T: Serialize>(path: &Path, value: &T, label: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create AutoDream dir {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("failed to write {label} {}", path.display()))
}
