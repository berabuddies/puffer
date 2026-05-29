//! Event-envelope helpers for the email subscriber.
//!
//! All outbound ndjson lines are emitted via [`emit_event`], which serializes
//! one [`Event`] to stdout followed by `\n` and flushes. Stdout is reserved
//! for the runtime bus; nothing else must ever be written on it.

use std::io::Write as _;

use anyhow::Context as _;
use puffer_subscriber_runtime::Event;
use serde_json::Value;

/// Writes one [`Event`] to stdout as a single JSON line and flushes.
///
/// The helper locks stdout for the duration of the write so lines stay atomic
/// with respect to any other writer in the same process.
pub fn emit_event(event: &Event) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer(&mut handle, event).context("serialize event to stdout")?;
    handle.write_all(b"\n").context("write newline to stdout")?;
    handle.flush().context("flush stdout")?;
    Ok(())
}

/// Emits a control-flow event (config state, send results, errors, ...) on
/// the subscriber's configured topic. The payload is attached verbatim; the
/// `text` field is always empty for control events.
pub fn emit_control(topic: &str, kind: &str, payload: Value) -> anyhow::Result<()> {
    let event = Event {
        topic: topic.to_string(),
        kind: kind.to_string(),
        control: true,
        dedup_key: None,
        text: String::new(),
        payload,
    };
    emit_event(&event)
}
