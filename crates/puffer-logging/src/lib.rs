//! Default durable logging for puffer processes.
//!
//! Installs a global `tracing` subscriber writing to a daily-rotated file under
//! `~/.puffer/logs/<component>.log` (7 files kept), so connect/RPC/browser/error
//! events survive without any opt-in. This complements — and does not conflict
//! with — `puffer-observability`: that crate exports OpenTelemetry spans via a
//! handle stored in `puffer-core` and never installs a tracing subscriber.
//!
//! Knobs:
//! - `RUST_LOG` — level filter (default `info`).
//! - `PUFFER_LOG_DIR` — override the log directory (used by tests).
//! - `PUFFER_LOG_STDERR=1` — additionally mirror logs to stderr. Off by default
//!   so interactive TUI rendering is never corrupted.
//!
//! The telegram-user subscriber process keeps its own per-account subscriber
//! (`<state>/telegram.log`); it runs in a separate process spawned before this
//! init, so the two never compete for the global subscriber.

use std::path::PathBuf;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

const MAX_LOG_FILES: usize = 7;

/// Resolves the log directory: `PUFFER_LOG_DIR` override, else `~/.puffer/logs`.
fn log_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("PUFFER_LOG_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    dirs::home_dir().map(|home| home.join(".puffer").join("logs"))
}

/// Installs the process-wide tracing subscriber. Returns the non-blocking file
/// writer's [`WorkerGuard`] — the caller MUST keep it alive for the process
/// lifetime or buffered lines are dropped on exit. Returns `None` when the
/// rolling appender cannot be created (a stderr-only subscriber is still
/// installed when `PUFFER_LOG_STDERR=1`).
#[must_use]
pub fn init(component: &str) -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = std::env::var("PUFFER_LOG_STDERR")
        .map(|v| v == "1")
        .unwrap_or(false)
        .then(|| fmt::layer().with_writer(std::io::stderr).with_ansi(false).compact());

    let (file_layer, guard) = match log_dir() {
        Some(dir) => {
            let _ = std::fs::create_dir_all(&dir);
            match Builder::new()
                .rotation(Rotation::DAILY)
                .filename_prefix(component)
                .filename_suffix("log")
                .max_log_files(MAX_LOG_FILES)
                .build(&dir)
            {
                Ok(appender) => {
                    let (writer, guard) = tracing_appender::non_blocking(appender);
                    let layer = fmt::layer().with_writer(writer).with_ansi(false);
                    (Some(layer), Some(guard))
                }
                Err(_) => (None, None),
            }
        }
        None => (None, None),
    };

    // `try_init`: never panic if a subscriber is somehow already installed
    // (e.g. tests); our layers simply don't attach in that case.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init();

    guard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_writes_log_lines_to_component_file() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("PUFFER_LOG_DIR", dir.path());
        let guard = init("puffer-test");
        assert!(guard.is_some(), "rolling appender should build");

        tracing::info!(probe = "logging-smoke", "hello from puffer-logging test");
        drop(guard); // flush the non-blocking writer

        let mut found = false;
        for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("puffer-test") && name.ends_with("log") {
                let body = std::fs::read_to_string(entry.path()).unwrap();
                if body.contains("logging-smoke") {
                    found = true;
                }
            }
        }
        assert!(found, "log line must land in the component log file");
        std::env::remove_var("PUFFER_LOG_DIR");
    }
}
