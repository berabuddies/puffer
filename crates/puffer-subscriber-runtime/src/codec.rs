//! Newline-delimited JSON framing helpers over Tokio's async I/O.
//!
//! Subscribers write one JSON value per line to stdout; the runtime reads
//! them with [`read_lines`]. Control messages travel in the other direction
//! via [`write_line`] over the child's stdin.

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// Spawns a task that reads `reader` one line at a time and forwards each
/// line (without the trailing newline) to the returned receiver. Empty
/// lines are skipped. The task ends cleanly on EOF or read error; errors
/// are emitted on `error_sink` when provided.
///
/// The generic bound is `AsyncRead + Unpin + Send + 'static`, which fits
/// `ChildStdout` and `ChildStderr`.
pub fn read_lines<R>(reader: R) -> mpsc::UnboundedReceiver<String>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let mut buf = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match buf.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if tx.send(trimmed).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "subscriber line reader failed");
                    break;
                }
            }
        }
    });
    rx
}

/// Serializes `value` as JSON and writes it followed by a newline to `writer`.
/// The writer is flushed after every line so children see control messages
/// promptly.
pub async fn write_line<W, T>(writer: &mut W, value: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut payload = serde_json::to_vec(value).context("serialize ndjson line")?;
    payload.push(b'\n');
    writer
        .write_all(&payload)
        .await
        .context("write ndjson line")?;
    writer.flush().await.context("flush ndjson writer")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn read_lines_yields_each_trimmed_line() {
        let (mut write_end, read_end) = duplex(1024);
        tokio::spawn(async move {
            write_end.write_all(b"hello\nworld\n\n").await.unwrap();
        });
        let mut rx = read_lines(read_end);
        assert_eq!(rx.recv().await.unwrap(), "hello");
        assert_eq!(rx.recv().await.unwrap(), "world");
    }

    #[tokio::test]
    async fn write_line_emits_json_and_newline() {
        let (write_end, read_end) = duplex(1024);
        let mut w = write_end;
        write_line(&mut w, &serde_json::json!({"ping": 1}))
            .await
            .unwrap();
        drop(w);
        let mut rx = read_lines(read_end);
        assert_eq!(rx.recv().await.unwrap(), "{\"ping\":1}");
    }
}
