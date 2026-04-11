use anyhow::{Context, Result};
use fslock::LockFile;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Identifies how one prompt-history item entered the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PromptHistorySource {
    Sent,
    Cleared,
}

/// Stores one JSONL prompt-history row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PromptHistoryRecord {
    pub(crate) text: String,
    pub(crate) source: PromptHistorySource,
    pub(crate) timestamp_ms: u128,
    pub(crate) cwd: String,
}

/// Manages the shared on-disk prompt-history JSONL file.
#[derive(Debug, Clone)]
pub(crate) struct PromptHistoryStore {
    path: PathBuf,
}

impl PromptHistoryStore {
    /// Creates a prompt-history store backed by the provided JSONL path.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Returns the default prompt-history path colocated with the auth store.
    pub(crate) fn from_auth_path(auth_path: &Path) -> Self {
        let history_path = auth_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("prompt_history.jsonl");
        Self::new(history_path)
    }

    /// Appends one prompt-history record while holding the shared file lock.
    pub(crate) fn append(&self, text: &str, source: PromptHistorySource, cwd: &Path) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        self.with_lock(|| {
            let mut file = OpenOptions::new()
                .append(true)
                .create(true)
                .open(&self.path)
                .with_context(|| {
                    format!("failed to open prompt history {}", self.path.display())
                })?;
            let record = PromptHistoryRecord {
                text: text.to_string(),
                source,
                timestamp_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or(Duration::ZERO)
                    .as_millis(),
                cwd: cwd.display().to_string(),
            };
            writeln!(file, "{}", serde_json::to_string(&record)?)?;
            Ok(())
        })
    }

    /// Loads the newest prompt texts from disk, trimming to the requested in-memory limit.
    pub(crate) fn load_recent_texts(&self, limit: usize) -> Result<VecDeque<String>> {
        if limit == 0 {
            return Ok(VecDeque::new());
        }
        self.with_lock(|| {
            if !self.path.exists() {
                return Ok(VecDeque::new());
            }
            let reader = BufReader::new(
                fs::File::open(&self.path)
                    .with_context(|| format!("failed to read {}", self.path.display()))?,
            );
            let mut entries = VecDeque::with_capacity(limit);
            for line in reader.lines() {
                let Ok(line) = line else {
                    continue;
                };
                let Ok(record) = serde_json::from_str::<PromptHistoryRecord>(&line) else {
                    continue;
                };
                let text = record.text.trim();
                if text.is_empty() {
                    continue;
                }
                if entries.len() >= limit {
                    entries.pop_front();
                }
                entries.push_back(text.to_string());
            }
            Ok(entries)
        })
    }

    fn with_lock<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create prompt history dir {}", parent.display())
            })?;
        }
        let mut lock = LockFile::open(&self.lock_path())
            .map_err(anyhow::Error::new)
            .with_context(|| {
                format!("failed to open prompt history lock {}", self.path.display())
            })?;
        lock.lock()
            .map_err(anyhow::Error::new)
            .with_context(|| format!("failed to lock prompt history {}", self.path.display()))?;
        operation()
    }

    fn lock_path(&self) -> PathBuf {
        self.path.with_extension("jsonl.lock")
    }
}

#[cfg(test)]
mod tests {
    use super::{PromptHistorySource, PromptHistoryStore};

    #[test]
    fn shared_store_appends_and_loads_records_across_instances() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("prompt_history.jsonl");
        let first = PromptHistoryStore::new(path.clone());
        let second = PromptHistoryStore::new(path);

        first
            .append("first prompt", PromptHistorySource::Sent, tempdir.path())
            .unwrap();
        second
            .append(
                "second prompt",
                PromptHistorySource::Cleared,
                tempdir.path(),
            )
            .unwrap();

        let entries = first.load_recent_texts(10).unwrap();
        assert_eq!(
            entries.into_iter().collect::<Vec<_>>(),
            vec!["first prompt".to_string(), "second prompt".to_string()]
        );
    }
}
