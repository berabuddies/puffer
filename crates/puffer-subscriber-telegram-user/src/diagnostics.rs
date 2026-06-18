//! Shared writer for the subscriber's append-only ndjson diagnostic logs
//! (`message-diagnostics.ndjson`, `update-diagnostics.ndjson`).
//!
//! Replaces the two near-identical, silently-failing writers that previously
//! lived in `delivery.rs` and `updates.rs`. Improvements:
//!   - one implementation instead of two copies;
//!   - **size-based rotation** (rotate at 5 MiB, keep three generations) so
//!     the files no longer grow unbounded;
//!   - write failures surface via a throttled `tracing::warn!` instead of being
//!     swallowed, so a read-only/full state dir is visible instead of silent.
//!
//! Still best-effort: a diagnostic write never blocks or fails message delivery.

use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

/// Rotate a diagnostic file once it reaches this size.
const MAX_BYTES: u64 = 5 * 1024 * 1024;
const MAX_GENERATIONS: usize = 3;

/// Serializes diagnostic writes across both files. They are infrequent and the
/// payloads are tiny, so a single lock is simpler than per-file locks and avoids
/// interleaved lines.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Last time a write-failure warning was emitted (epoch ms), to throttle the
/// warning to at most once per minute under a persistent failure (e.g. disk full).
static LAST_WARN_MS: AtomicI64 = AtomicI64::new(0);

/// Appends `record` as one ndjson line to `path`, rotating first if needed.
/// Best-effort: failures are logged (throttled), never propagated.
pub(crate) fn append_ndjson(path: &Path, record: &Value) {
    if let Err(error) = try_append(path, record) {
        warn_throttled(path, &error);
    }
}

fn try_append(path: &Path, record: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Recover from a poisoned lock: the data it guards is just a file handle.
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    rotate_if_needed(path);
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{record}")
}

fn rotate_if_needed(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() >= MAX_BYTES {
            let oldest = rotated_path(path, MAX_GENERATIONS);
            let _ = std::fs::remove_file(oldest);
            for generation in (1..MAX_GENERATIONS).rev() {
                let from = rotated_path(path, generation);
                let to = rotated_path(path, generation + 1);
                if from.exists() {
                    let _ = std::fs::rename(from, to);
                }
            }
            let _ = std::fs::rename(path, rotated_path(path, 1));
        }
    }
}

fn rotated_path(path: &Path, generation: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.display(), generation))
}

fn warn_throttled(path: &Path, error: &std::io::Error) {
    let now = now_ms();
    let last = LAST_WARN_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) >= 60_000 {
        LAST_WARN_MS.store(now, Ordering::Relaxed);
        tracing::warn!(
            path = %path.display(),
            %error,
            "failed to write telegram diagnostic line (diagnostics are best-effort)"
        );
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn append_writes_one_ndjson_line_per_call() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.ndjson");
        append_ndjson(&path, &json!({"a": 1}));
        append_ndjson(&path, &json!({"a": 2}));
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().count(), 2);
        assert!(body.contains("\"a\":1") && body.contains("\"a\":2"));
    }

    #[test]
    fn append_creates_missing_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/d.ndjson");
        append_ndjson(&path, &json!({"ok": true}));
        assert!(path.exists());
    }

    #[test]
    fn rotates_to_backup_when_oversized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.ndjson");
        // Sparsely grow the file past the rotation threshold (instant, no real IO).
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_BYTES).unwrap();
        drop(file);

        append_ndjson(&path, &json!({"fresh": true}));

        let backup = PathBuf::from(format!("{}.1", path.display()));
        assert!(backup.exists(), "oversized file must rotate to a .1 backup");
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            body.lines().count(),
            1,
            "post-rotation file starts fresh with just the new line"
        );
    }

    #[test]
    fn rotation_keeps_three_generations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.ndjson");

        for value in ["first", "second", "third", "fourth"] {
            let file = std::fs::File::create(&path).unwrap();
            file.set_len(MAX_BYTES).unwrap();
            drop(file);
            append_ndjson(&path, &json!({ "value": value }));
        }

        assert!(PathBuf::from(format!("{}.1", path.display())).exists());
        assert!(PathBuf::from(format!("{}.2", path.display())).exists());
        assert!(PathBuf::from(format!("{}.3", path.display())).exists());
        assert!(
            !PathBuf::from(format!("{}.4", path.display())).exists(),
            "diagnostics rotation must keep only three generations"
        );
    }
}
