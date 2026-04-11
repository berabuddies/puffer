use serde::{Deserialize, Serialize};

/// Describes one append-only transcript rewrite operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum TranscriptRewrite {
    Clear,
    PopLast {
        #[serde(default = "default_pop_count")]
        count: usize,
    },
}

fn default_pop_count() -> usize {
    1
}

/// Stores one append-only git diff snapshot for a command checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitDiffSnapshot {
    pub command: String,
    pub status: String,
    pub unstaged_diffstat: String,
    pub staged_diffstat: String,
    #[serde(default)]
    pub patch: String,
    pub patch_excerpt: String,
}

/// Stores Claude-style read metadata that must survive session restore.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ClaudeReadSnapshotEvent {
    pub path: String,
    pub timestamp_ms: u128,
    pub is_partial_view: bool,
}

/// Stores a transcript event in append-only session history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptEvent {
    UserMessage {
        text: String,
    },
    AssistantMessage {
        text: String,
    },
    SystemMessage {
        text: String,
    },
    /// Structured tool invocation — preserves call_id, tool name, input/output
    /// so the Responses API can reconstruct FunctionCall/FunctionCallOutput
    /// items instead of degrading to system messages.
    ToolInvocation {
        call_id: String,
        tool_id: String,
        input: String,
        output: String,
        success: bool,
    },
    CommandInvoked {
        name: String,
        args: String,
    },
    SessionRenamed {
        name: String,
    },
    GitDiffSnapshot {
        snapshot: GitDiffSnapshot,
    },
    TranscriptRewritten {
        #[serde(flatten)]
        rewrite: TranscriptRewrite,
    },
    StateSnapshot {
        current_model: Option<String>,
        current_provider: Option<String>,
        theme: String,
        prompt_color: String,
        effort_level: String,
        fast_mode: bool,
        #[serde(default)]
        plan_mode: bool,
        #[serde(default)]
        plan_mode_attachment_turns: usize,
        #[serde(default)]
        plan_mode_attachment_count: usize,
        #[serde(default)]
        plan_mode_has_exited: bool,
        #[serde(default)]
        plan_mode_needs_reentry_attachment: bool,
        #[serde(default)]
        plan_mode_needs_exit_attachment: bool,
        sandbox_mode: String,
        remote_name: Option<String>,
        remote_environment: Option<String>,
        #[serde(default)]
        remote_session_id: Option<String>,
        #[serde(default)]
        remote_session_url: Option<String>,
        #[serde(default)]
        remote_session_status: Option<String>,
        #[serde(default)]
        active_team_name: Option<String>,
        statusline_enabled: bool,
        working_dirs: Vec<String>,
        #[serde(default)]
        claude_read_state: Vec<ClaudeReadSnapshotEvent>,
    },
}
