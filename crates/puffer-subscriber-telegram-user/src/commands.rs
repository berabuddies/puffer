//! Stdin command parser and dispatcher for the Telegram subscriber.
//!
//! Wraps async-line reading of the ndjson control protocol and produces
//! typed [`SubscriberCommand`] values that the main event loop consumes.

use anyhow::Result;
use puffer_subscriber_runtime::SubscriberCommand;
use tokio::io::{AsyncBufReadExt, BufReader, Stdin};
use tracing::warn;

/// Async line iterator over stdin that returns [`SubscriberCommand`] values.
///
/// Returns `Ok(None)` when stdin closes (no more input). Malformed lines are
/// logged at WARN level and skipped so a single bad control message does not
/// terminate the subscriber.
pub struct CommandStream {
    reader: BufReader<Stdin>,
    buf: String,
}

impl CommandStream {
    /// Wraps the process's stdin for line-oriented reads.
    pub fn new() -> Self {
        Self {
            reader: BufReader::new(tokio::io::stdin()),
            buf: String::new(),
        }
    }

    /// Awaits the next well-formed command. Lines that fail to parse are
    /// skipped with a warning. Resolves to `Ok(None)` when stdin is closed.
    pub async fn next(&mut self) -> Result<Option<SubscriberCommand>> {
        loop {
            self.buf.clear();
            let n = self.reader.read_line(&mut self.buf).await?;
            if n == 0 {
                return Ok(None);
            }
            let line = self.buf.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<SubscriberCommand>(line) {
                Ok(cmd) => return Ok(Some(cmd)),
                Err(err) => {
                    warn!(error = %err, line = %line, "malformed stdin command; skipping");
                    continue;
                }
            }
        }
    }
}
