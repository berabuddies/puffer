//! Tracing setup for the telegram-user subscriber.
//!
//! The subscriber runs as its own process, so without a subscriber installed
//! here every `tracing` event (connect / auth / reconnect / error) is dropped —
//! the daemon does not persist the forwarded stderr lines either. We therefore
//! install our own subscriber that writes to BOTH:
//!   - a daily-rotated file at `<state>/telegram.log` (kept 7 days), so failures
//!     are durably recorded next to the account's session/cursor/diagnostics;
//!   - stderr, which the supervisor still forwards to the daemon.
//!
//! Level is controlled by `RUST_LOG` (default `info`).

use crate::state::SkillEnv;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initializes tracing for the subscriber. Returns the non-blocking writer's
/// [`WorkerGuard`] — the caller MUST keep it alive for the process lifetime,
/// otherwise buffered log lines are dropped on exit. Returns `None` when the
/// rolling file appender cannot be created (stderr logging is still installed).
#[must_use]
pub(crate) fn init(env: &SkillEnv) -> Option<WorkerGuard> {
    let _ = std::fs::create_dir_all(&env.state_dir);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .compact();

    let (file_layer, guard) = match Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("telegram")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&env.state_dir)
    {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = fmt::layer().with_writer(writer).with_ansi(false);
            (Some(layer), Some(guard))
        }
        Err(_) => (None, None),
    };

    // `try_init` so we never panic if a subscriber is somehow already set.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init();

    guard
}
