//! Stdin command parser for the email subscriber.
//!
//! Mirrors the telegram-user crate's `CommandStream`: an async line iterator
//! over stdin that yields typed [`SubscriberCommand`] values. Malformed lines
//! are logged at WARN and skipped so one bad control line cannot terminate
//! the subscriber.

use anyhow::Result;
use puffer_subscriber_runtime::SubscriberCommand;
use tokio::io::{AsyncBufReadExt, BufReader, Stdin};
use tracing::warn;

/// Async line iterator over stdin that produces [`SubscriberCommand`] values.
///
/// Returns `Ok(None)` when stdin closes (EOF). Lines that fail to parse as a
/// [`SubscriberCommand`] are logged and skipped.
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

    /// Awaits the next well-formed command. Resolves to `Ok(None)` on EOF.
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
