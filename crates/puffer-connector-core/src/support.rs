//! Shared utilities used by platform connectors.
//!
//! These helpers make it easy for every connector to do the right thing
//! on a handful of cross-cutting concerns (message splitting, retry,
//! mention parsing) without re-implementing them per crate.

use std::time::Duration;

/// Describes one inbound message after a platform has extracted the
/// transport-specific fields. Connectors pass this to the handler so
/// mention-gating, user segmentation, and thread-reply bookkeeping all
/// live in one shared place instead of six.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMessage {
    /// Conversation id (channel/chat/thread root/email message-id).
    pub conversation_id: String,
    /// Sender identity — `None` when the platform doesn't expose one
    /// (anonymous webhooks, some email forwards, etc.).
    pub user_id: Option<String>,
    /// Text body after trimming and mention stripping.
    pub text: String,
    /// Thread identifier within a conversation (Slack thread_ts,
    /// Discord forum post id, email References chain, Matrix m.relates_to).
    /// Replies are sent back with the same thread_id.
    pub thread_id: Option<String>,
    /// Whether this conversation is a group chat (channel / room with
    /// multiple users) rather than a direct message. Drives mention
    /// gating and user-scoped session keying.
    pub is_group: bool,
    /// Whether the bot was explicitly mentioned or replied-to. Always
    /// treated as `true` in DMs.
    pub bot_mentioned: bool,
    /// Whether the inbound message was authored by the bot itself
    /// (so handlers can short-circuit before hitting dispatch).
    pub from_bot: bool,
}

impl InboundMessage {
    /// Convenience constructor for DMs — `is_group = false`,
    /// `bot_mentioned = true`, `from_bot = false`.
    pub fn dm(
        conversation_id: impl Into<String>,
        user_id: Option<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            user_id,
            text: text.into(),
            thread_id: None,
            is_group: false,
            bot_mentioned: true,
            from_bot: false,
        }
    }
}

/// Platform-specific policy for splitting long assistant replies.
///
/// Each platform has its own per-message character/byte limit; sending
/// a reply that exceeds it typically fails silently or with a confusing
/// error. This helper produces a series of chunks that fit under
/// `max_chars` while preserving Markdown code-fence balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageSplitter {
    pub max_chars: usize,
    pub annotate_parts: bool,
}

impl MessageSplitter {
    pub const fn new(max_chars: usize) -> Self {
        Self {
            max_chars,
            annotate_parts: true,
        }
    }

    /// Useful platform defaults — Telegram's MarkdownV2 permits 4096
    /// UTF-16 code units, but we use a conservative 3500 chars to leave
    /// room for the chunk indicator and account for wide characters.
    pub const TELEGRAM: Self = Self::new(3500);
    pub const DISCORD: Self = Self::new(1950);
    pub const SLACK: Self = Self::new(38000);
    pub const MATRIX: Self = Self::new(32000);
    /// Email has no practical per-message limit; we still split huge
    /// replies so a single email isn't megabytes long.
    pub const EMAIL: Self = Self::new(100_000);

    /// Splits `body` into chunks. Returns a single element when `body`
    /// already fits. Chunk indicators like `(1/3)` are appended only
    /// when more than one chunk is produced and `annotate_parts` is set.
    pub fn split(&self, body: &str) -> Vec<String> {
        if body.chars().count() <= self.max_chars {
            return vec![body.to_string()];
        }

        // Reserve space for the largest possible annotation so we never
        // overshoot after tacking it on.
        let annotation_budget = if self.annotate_parts { 12 } else { 0 };
        let budget = self.max_chars.saturating_sub(annotation_budget).max(1);

        let mut chunks = Vec::new();
        let mut buffer = String::new();
        let mut fenced = false;

        for line in body.split_inclusive('\n') {
            // Peek at code-fence transitions so we can re-open a fence
            // across a chunk boundary.
            let toggles_fence = line.trim_start().starts_with("```");

            if buffer.chars().count() + line.chars().count() > budget && !buffer.is_empty() {
                if fenced {
                    buffer.push_str("```\n");
                }
                chunks.push(std::mem::take(&mut buffer));
                if fenced {
                    buffer.push_str("```\n");
                }
            }

            buffer.push_str(line);
            if toggles_fence {
                fenced = !fenced;
            }
        }
        if !buffer.is_empty() {
            chunks.push(buffer);
        }

        // Hard-fallback split in case a single line busted our budget:
        // walk through any over-budget chunk and cut it at char
        // boundaries.
        let mut refined = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            if chunk.chars().count() <= budget {
                refined.push(chunk);
                continue;
            }
            let mut rest = chunk.as_str();
            while rest.chars().count() > budget {
                let split_idx = rest
                    .char_indices()
                    .nth(budget)
                    .map(|(idx, _)| idx)
                    .unwrap_or(rest.len());
                refined.push(rest[..split_idx].to_string());
                rest = &rest[split_idx..];
            }
            if !rest.is_empty() {
                refined.push(rest.to_string());
            }
        }

        if self.annotate_parts && refined.len() > 1 {
            let total = refined.len();
            refined
                .into_iter()
                .enumerate()
                .map(|(index, chunk)| format!("{chunk}\n\n({}/{total})", index + 1))
                .collect()
        } else {
            refined
        }
    }
}

/// Runs `op` with exponential backoff. Used by live drivers when
/// sending replies — transient network errors shouldn't drop the user
/// on the floor.
///
/// * Retries up to `max_attempts` times (including the initial attempt).
/// * Sleeps `base * 2^n + jitter` between attempts where jitter is 0-250ms.
pub fn retry_with_backoff<T, E>(
    max_attempts: u32,
    base: Duration,
    mut op: impl FnMut() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    debug_assert!(max_attempts >= 1);
    let mut attempt = 0u32;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(error) => {
                attempt += 1;
                if attempt >= max_attempts {
                    return Err(error);
                }
                let delay_ms = base.as_millis() as u64 * (1u64 << (attempt - 1)).min(16)
                    + jitter_ms();
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
        }
    }
}

fn jitter_ms() -> u64 {
    // Cheap pseudo-random jitter without adding a dep: use the low
    // nanosecond bits of the current time. Accuracy doesn't matter —
    // this just needs to be noisy.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    now % 250
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitter_returns_single_chunk_when_body_fits() {
        let splitter = MessageSplitter::new(100);
        let body = "hello world";
        let chunks = splitter.split(body);
        assert_eq!(chunks, vec![body.to_string()]);
    }

    #[test]
    fn splitter_splits_long_body_and_annotates_parts() {
        let splitter = MessageSplitter::new(30);
        let body = "line one\nline two\nline three\nline four\nline five\n";
        let chunks = splitter.split(body);
        assert!(chunks.len() >= 2);
        for (idx, chunk) in chunks.iter().enumerate() {
            assert!(
                chunk.contains(&format!("({}/{})", idx + 1, chunks.len())),
                "chunk {idx} missing annotation: {chunk}"
            );
        }
    }

    #[test]
    fn splitter_preserves_code_fence_balance_across_chunks() {
        // Make the body clearly exceed the budget so we split at least
        // once. Budget 50 is tight enough that the fenced block has to
        // straddle a chunk boundary.
        let splitter = MessageSplitter::new(50);
        let body = "prose line 1\n```rust\nfn a() {}\nfn b() {}\nfn c() {}\n```\ntail\n";
        let chunks = splitter.split(body);
        assert!(chunks.len() >= 2, "expected multi-chunk, got {chunks:?}");
        // Every chunk should have balanced (even-count) fences; odd
        // counts would surface as broken markdown in the client.
        for chunk in &chunks {
            let count = chunk.matches("```").count();
            assert!(
                count % 2 == 0,
                "chunk `{chunk}` has {count} fences (must be even)"
            );
        }
    }

    #[test]
    fn splitter_falls_back_to_hard_cut_on_runaway_line() {
        let splitter = MessageSplitter {
            max_chars: 20,
            annotate_parts: false,
        };
        let body = "a".repeat(200);
        let chunks = splitter.split(&body);
        assert!(chunks.iter().all(|c| c.chars().count() <= 20));
        assert_eq!(chunks.join("").chars().count(), 200);
    }

    #[test]
    fn retry_returns_first_success_without_retrying() {
        let mut attempts = 0;
        let result: std::result::Result<i32, &str> =
            retry_with_backoff(5, Duration::from_millis(0), || {
                attempts += 1;
                Ok(42)
            });
        assert_eq!(result, Ok(42));
        assert_eq!(attempts, 1);
    }

    #[test]
    fn retry_surfaces_last_error_after_exhausting_attempts() {
        let mut attempts = 0;
        let result: std::result::Result<i32, &str> =
            retry_with_backoff(3, Duration::from_millis(0), || {
                attempts += 1;
                Err("boom")
            });
        assert_eq!(result, Err("boom"));
        assert_eq!(attempts, 3);
    }

    #[test]
    fn retry_recovers_after_transient_failure() {
        let mut attempts = 0;
        let result: std::result::Result<i32, &str> =
            retry_with_backoff(4, Duration::from_millis(0), || {
                attempts += 1;
                if attempts < 3 {
                    Err("transient")
                } else {
                    Ok(99)
                }
            });
        assert_eq!(result, Ok(99));
        assert_eq!(attempts, 3);
    }

    #[test]
    fn inbound_message_dm_helper_sets_expected_flags() {
        let msg = InboundMessage::dm("u-1", Some("alice".to_string()), "hi");
        assert!(!msg.is_group);
        assert!(msg.bot_mentioned);
        assert!(!msg.from_bot);
        assert_eq!(msg.text, "hi");
        assert_eq!(msg.user_id.as_deref(), Some("alice"));
    }
}
