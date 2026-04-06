use serde::{Deserialize, Serialize};

/// Represents plan-mode metadata captured in runtime state snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimePlanState {
    pub mode_open: bool,
    pub summary: Option<String>,
    pub updated_at_ms: u64,
}

/// Describes the persisted status of one runtime task entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTaskStatus {
    Completed,
    Failed,
}

/// Stores one runtime task snapshot entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeTask {
    pub id: u64,
    pub label: String,
    pub detail: String,
    pub status: RuntimeTaskStatus,
    pub plan_summary: Option<String>,
    pub agent_id: Option<String>,
    pub worktree: String,
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
    CommandInvoked {
        name: String,
        args: String,
    },
    SessionRenamed {
        name: String,
    },
    StateSnapshot {
        current_model: Option<String>,
        current_provider: Option<String>,
        theme: String,
        prompt_color: String,
        effort_level: String,
        fast_mode: bool,
        sandbox_mode: String,
        remote_name: Option<String>,
        remote_environment: Option<String>,
        statusline_enabled: bool,
        working_dirs: Vec<String>,
    },
    RuntimeState {
        plan: RuntimePlanState,
        tasks: Vec<RuntimeTask>,
        next_task_id: u64,
        active_agent_id: Option<String>,
        active_worktree: Option<String>,
    },
}
