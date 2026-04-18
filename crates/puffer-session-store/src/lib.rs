mod events;
mod metadata;
mod store;

pub use events::ClaudeReadSnapshotEvent;
pub use events::GitDiffSnapshot;
pub use events::TranscriptEvent;
pub use events::TranscriptRewrite;
pub use metadata::{SessionMetadata, SessionRecord, SessionSummary};
pub use store::SessionStore;

/// Trace-name tag used by the reflection runtime for the per-session sidecar
/// JSONL file (`{session-id}.runtime_trace.jsonl`). Kept as a shared constant
/// so every producer/consumer references the same string.
pub const TRACE_RUNTIME: &str = "runtime_trace";
