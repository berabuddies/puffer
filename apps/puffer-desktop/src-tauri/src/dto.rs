use puffer_session_store::MessageActor;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct FolderGroupDto {
    pub id: String,
    pub label: String,
    pub path: String,
    pub sessions: Vec<SessionListItemDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionListItemDto {
    pub id: String,
    pub title: String,
    pub display_name: Option<String>,
    pub generated_title: Option<String>,
    pub cwd: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub event_count: usize,
    pub parent_session_id: Option<String>,
    pub slug: Option<String>,
    pub tags: Vec<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionTimelineDto {
    pub session: SessionListItemDto,
    pub items: Vec<TimelineItemDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionDiffsDto {
    pub session_id: String,
    pub latest_diff: Option<DiffSnapshotDto>,
    pub history: Vec<DiffSnapshotDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineItemDto {
    UserMessage {
        id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    AssistantMessage {
        id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    SystemMessage {
        id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    ToolCall {
        id: String,
        tool_id: String,
        status: String,
        input_text: String,
        input_json: Option<Value>,
        output_text: String,
        permission_dialog: Option<PermissionDialogDto>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
        #[serde(skip_serializing_if = "Option::is_none")]
        subject: Option<MessageActor>,
    },
    CommandInvoked {
        id: String,
        name: String,
        args: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    SessionRenamed {
        id: String,
        name: String,
    },
    DiffSnapshot {
        id: String,
        snapshot: DiffSnapshotDto,
    },
    StateSnapshot {
        id: String,
        current_model: Option<String>,
        current_provider: Option<String>,
        effort_level: String,
        plan_mode: bool,
        sandbox_mode: String,
        remote_name: Option<String>,
        remote_environment: Option<String>,
        statusline_enabled: bool,
        working_dirs: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionDialogDto {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffSnapshotDto {
    pub command: String,
    pub status: String,
    pub unstaged_diffstat: String,
    pub staged_diffstat: String,
    pub patch: String,
    pub patch_excerpt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoStatusDto {
    pub cwd: String,
    pub is_git_repo: bool,
    pub branch: Option<String>,
    pub has_uncommitted_changes: bool,
    pub gh_available: bool,
    pub gh_authenticated: bool,
    pub create_pr_enabled: bool,
    pub create_pr_reason: Option<String>,
    pub merge_pr_enabled: bool,
    pub merge_pr_reason: Option<String>,
    pub active_pull_request: Option<PullRequestDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullRequestDto {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub merge_state_status: Option<String>,
    pub is_draft: bool,
    pub base_ref_name: Option<String>,
    pub head_ref_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoActionResultDto {
    pub success: bool,
    pub message: String,
    pub repo_status: RepoStatusDto,
    pub pull_request: Option<PullRequestDto>,
}
