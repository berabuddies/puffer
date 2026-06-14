use puffer_session_store::{
    AttachmentState, MessageActor, SessionStore, StoredAttachment, StoredAttachmentKind,
};
use puffer_provider_registry::{AxisRole, ControlKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Describes one session row rendered in the desktop sidebar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionListItemDto {
    pub session_id: String,
    pub display_name: Option<String>,
    pub generated_title: Option<String>,
    pub title: String,
    pub cwd: String,
    pub folder_path: String,
    pub updated_at_ms: u64,
    pub created_at_ms: u64,
    pub event_count: usize,
    pub activity_status: String,
    pub slug: Option<String>,
    pub tags: Vec<String>,
    pub note: Option<String>,
    pub parent_session_id: Option<String>,
    pub provider_id: String,
    pub model_id: Option<String>,
}

/// Describes one grouped folder section in the desktop sidebar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderGroupDto {
    pub folder_id: String,
    pub folder_label: String,
    pub folder_path: String,
    pub session_count: usize,
    pub sessions: Vec<SessionListItemDto>,
}

/// Describes one captured diff snapshot for the inspector pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiffSummaryDto {
    pub id: String,
    pub source: String,
    pub command_label: String,
    pub status_text: String,
    pub unstaged_diffstat: String,
    pub staged_diffstat: String,
    pub patch: String,
    pub patch_excerpt: String,
}

/// Describes one open pull request related to the selected session repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepoPullRequestDto {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub is_draft: bool,
    pub merge_state_status: Option<String>,
    pub head_ref_name: Option<String>,
    pub base_ref_name: Option<String>,
}

/// Describes the repository state and action readiness for one session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepoStatusDto {
    pub session_id: String,
    pub cwd: String,
    pub repo_root: Option<String>,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub is_clean: bool,
    pub status_lines: Vec<String>,
    pub has_gh: bool,
    pub gh_authenticated: bool,
    pub can_create_pull_request: bool,
    pub can_merge_pull_request: bool,
    pub create_pull_request_reason: Option<String>,
    pub merge_pull_request_reason: Option<String>,
    pub open_pull_request: Option<RepoPullRequestDto>,
    pub warnings: Vec<String>,
}

/// Describes one chat attachment rendered with a timeline user message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatAttachmentDto {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    pub extension: String,
    pub kind: String,
    pub state: String,
    pub source: ChatAttachmentSourceDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ChatAttachmentSourceDto {
    LocalFile {
        path: String,
    },
    RemoteUrl {
        url: String,
    },
    GeneratedMedia {
        #[serde(rename = "jobId")]
        job_id: String,
        #[serde(rename = "artifactId")]
        artifact_id: String,
        index: usize,
        #[serde(rename = "localPath", skip_serializing_if = "Option::is_none")]
        local_path: Option<String>,
        #[serde(rename = "remoteSourceUrl", skip_serializing_if = "Option::is_none")]
        remote_source_url: Option<String>,
    },
}

impl ChatAttachmentDto {
    /// Builds a desktop attachment DTO from stored metadata and file availability.
    pub(crate) fn from_stored(
        store: &SessionStore,
        session_id: Uuid,
        attachment: &StoredAttachment,
    ) -> Self {
        let state = match store.attachment_state(session_id, attachment) {
            AttachmentState::Available => "available",
            AttachmentState::Missing => "missing",
        };
        Self {
            id: attachment.id.clone(),
            name: attachment.name.clone(),
            mime_type: attachment.mime_type.clone(),
            size: attachment.size,
            extension: attachment.extension.clone(),
            kind: match attachment.kind {
                StoredAttachmentKind::Image => "image".to_string(),
                StoredAttachmentKind::File => "file".to_string(),
            },
            state: state.to_string(),
            source: ChatAttachmentSourceDto::LocalFile {
                path: store
                    .attachment_original_path(session_id, attachment)
                    .display()
                    .to_string(),
            },
        }
    }
}

/// Describes the outcome of one repo action initiated from the desktop shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepoActionResultDto {
    pub ok: bool,
    pub action: String,
    pub message: String,
    pub repo_status: RepoStatusDto,
    pub pull_request: Option<RepoPullRequestDto>,
}

/// Describes one normalized timeline item rendered in the conversation pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TimelineItemDto {
    UserMessage {
        id: String,
        text: String,
        attachments: Vec<ChatAttachmentDto>,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    AssistantMessage {
        id: String,
        text: String,
        attachments: Vec<ChatAttachmentDto>,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    SystemMessage {
        id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    Command {
        id: String,
        command_name: String,
        command_args: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    ToolCall {
        id: String,
        tool_id: String,
        status: String,
        summary: Option<String>,
        input_text: String,
        input_json: Option<Value>,
        output_text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
        #[serde(skip_serializing_if = "Option::is_none")]
        subject: Option<MessageActor>,
    },
    PermissionDialog {
        id: String,
        tool_id: String,
        state: String,
        summary: Option<String>,
        reason: String,
        input_text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    DiffSnapshot {
        id: String,
        snapshot: DiffSummaryDto,
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
    },
}

/// Describes the full desktop session view returned for one selected session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionDetailDto {
    pub session_id: String,
    pub display_name: Option<String>,
    pub generated_title: Option<String>,
    pub title: String,
    pub cwd: String,
    pub folder_path: String,
    pub updated_at_ms: u64,
    pub created_at_ms: u64,
    pub event_count: usize,
    pub activity_status: String,
    pub slug: Option<String>,
    pub tags: Vec<String>,
    pub note: Option<String>,
    pub parent_session_id: Option<String>,
    pub provider_id: String,
    pub model_id: Option<String>,
    pub timeline: Vec<TimelineItemDto>,
    pub latest_diff: Option<DiffSummaryDto>,
    pub diff_history: Vec<DiffSummaryDto>,
    pub repo_status: RepoStatusDto,
    pub agent_diff: AgentDiffDto,
    pub divergence: DivergenceReportDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentDiffDto {
    pub files: Vec<AgentDiffFileDto>,
    pub entries: Vec<AgentDiffEntryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentDiffFileDto {
    pub path: String,
    pub latest_kind: String,
    pub edit_count: usize,
    pub latest_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentDiffEntryDto {
    pub call_id: String,
    pub tool_id: String,
    pub kind: String,
    pub path: String,
    pub success: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DivergenceReportDto {
    pub agent_only: Vec<String>,
    pub git_only: Vec<String>,
    pub agent_total: usize,
    pub git_total: usize,
}

/// Describes the loaded runtime configuration snapshot for the desktop settings page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsSnapshotDto {
    pub workspace_root: String,
    pub workspace_config_file: String,
    pub user_config_file: String,
    pub auth_store_file: String,
    pub builtin_resources_dir: String,
    pub config: SettingsConfigDto,
    pub resources: ResourceCountsDto,
    pub sessions: SettingsSessionSummaryDto,
    pub auth: Vec<AuthProviderStatusDto>,
    pub providers: Vec<ProviderSummaryDto>,
    pub browser: BrowserSettingsDto,
    pub secrets: SecretsSettingsDto,
}

/// Describes browser extension settings for the settings page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserSettingsDto {
    pub extensions_enabled: bool,
    pub extensions: Vec<BrowserExtensionDto>,
    pub captcha: BrowserCaptchaSettingsDto,
}

/// Describes one user-added browser extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserExtensionDto {
    pub id: String,
    pub display_name: String,
    pub path: String,
    pub enabled: bool,
    pub manifest_present: bool,
    pub source: String,
}

/// Describes captcha extension settings for browser sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserCaptchaSettingsDto {
    pub enabled: bool,
    pub selected_solver: String,
    pub solvers: Vec<BrowserCaptchaSolverDto>,
}

/// Describes one built-in captcha solver extension package.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserCaptchaSolverDto {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub enabled: bool,
    pub base_url: String,
    pub api_key_secret_id: Option<String>,
    pub has_api_key: bool,
    pub version: String,
    pub bundled: bool,
    pub extension_path: String,
    pub release_url: String,
    pub download_url: String,
    pub sha256: String,
    pub license: String,
}

/// Describes the encrypted secret store status for the settings page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecretsSettingsDto {
    pub store_file: String,
    pub key_source: String,
    pub chrome_import_supported: bool,
    pub items: Vec<SecretSummaryDto>,
}

/// Describes non-secret metadata for one saved secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecretSummaryDto {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub username: Option<String>,
    pub origin: Option<String>,
    pub source: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Describes the effective loaded Puffer config values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsConfigDto {
    pub app_name: String,
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub openai_base_url: Option<String>,
    pub theme: String,
    pub media: MediaSettingsDto,
    pub mascot_id: String,
    pub mascot_display_name: String,
    pub mascot_enabled: bool,
    pub ui_no_alt_screen: bool,
    pub ui_tmux_golden_mode: bool,
}

/// Describes persisted image and video generation defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaSettingsDto {
    pub image: Option<MediaGenerationSettingsDto>,
    pub video: Option<MediaGenerationSettingsDto>,
}

/// Describes one persisted media generation default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaGenerationSettingsDto {
    pub provider_id: String,
    pub logical_model_id: String,
    pub selections: BTreeMap<String, String>,
}

/// Describes one verified media generation capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaCapabilityInfoDto {
    pub provider_id: String,
    pub provider_display_name: String,
    pub model_id: String,
    pub model_display_name: String,
    pub kind: String,
    pub operation: String,
    pub adapter: String,
    pub axes: Vec<MediaCapabilityAxisDto>,
    pub status: String,
    pub source: String,
    pub reason: Option<String>,
    pub checked_at_ms: u64,
}

/// Describes one selectable media axis.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaCapabilityAxisDto {
    pub id: String,
    pub label: String,
    pub role: AxisRole,
    pub control: ControlKind,
}

/// Describes aggregate loaded resource counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceCountsDto {
    pub providers: usize,
    pub tools: usize,
    pub agents: usize,
    pub prompts: usize,
    pub hooks: usize,
    pub skills: usize,
    pub mascots: usize,
    pub plugins: usize,
    pub mcp_servers: usize,
    pub ides: usize,
}

/// Describes session inventory summary for the settings page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsSessionSummaryDto {
    pub total_sessions: usize,
    pub folder_groups: usize,
}

/// Describes one provider auth status in the settings page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthProviderStatusDto {
    pub provider_id: String,
    pub kind: String,
    pub email: Option<String>,
    pub expires_at_ms: Option<u64>,
    pub scopes: Vec<String>,
    pub plan_type: Option<String>,
    pub organization_name: Option<String>,
}

/// Describes one registered provider summary in the settings page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderSummaryDto {
    pub id: String,
    pub display_name: String,
    pub base_url: String,
    pub default_api: String,
    pub model_count: usize,
    pub auth_modes: Vec<String>,
    pub source_kind: String,
    pub source_path: Option<String>,
}

/// Describes one importable credential discovered on disk (`~/.claude`,
/// `~/.codex`, …) that the desktop can adopt without an interactive flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalCredentialDto {
    pub provider_id: String,
    pub source: String,
    pub kind: String,
    pub description: String,
    pub source_path: String,
}

/// Describes one remote scratchpad command or file operation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteOperationDto {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}
