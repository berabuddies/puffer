use puffer_session_store::{
    AttachmentState, MessageActor, SessionStore, StoredAttachment, StoredAttachmentKind,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionListItemDto {
    pub(crate) session_id: String,
    pub(crate) display_name: Option<String>,
    pub(crate) generated_title: Option<String>,
    pub(crate) title: String,
    pub(crate) cwd: String,
    pub(crate) folder_path: String,
    pub(crate) updated_at_ms: u64,
    pub(crate) created_at_ms: u64,
    pub(crate) event_count: usize,
    pub(crate) activity_status: String,
    pub(crate) slug: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) note: Option<String>,
    pub(crate) parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderGroupDto {
    pub(crate) folder_id: String,
    pub(crate) folder_label: String,
    pub(crate) folder_path: String,
    pub(crate) session_count: usize,
    pub(crate) sessions: Vec<SessionListItemDto>,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionGroupsPageDto {
    pub(crate) groups: Vec<FolderGroupDto>,
    pub(crate) offset: usize,
    pub(crate) limit: usize,
    pub(crate) returned_sessions: usize,
    pub(crate) total_sessions: usize,
    pub(crate) has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiffSummaryDto {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) command_label: String,
    pub(crate) status_text: String,
    pub(crate) unstaged_diffstat: String,
    pub(crate) staged_diffstat: String,
    pub(crate) patch: String,
    pub(crate) patch_excerpt: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepoPullRequestDto {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) state: String,
    pub(crate) is_draft: bool,
    pub(crate) merge_state_status: Option<String>,
    pub(crate) head_ref_name: Option<String>,
    pub(crate) base_ref_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepoStatusDto {
    pub(crate) session_id: String,
    pub(crate) cwd: String,
    pub(crate) repo_root: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) head_sha: Option<String>,
    pub(crate) is_clean: bool,
    pub(crate) status_lines: Vec<String>,
    pub(crate) has_gh: bool,
    pub(crate) gh_authenticated: bool,
    pub(crate) can_create_pull_request: bool,
    pub(crate) can_merge_pull_request: bool,
    pub(crate) create_pull_request_reason: Option<String>,
    pub(crate) merge_pull_request_reason: Option<String>,
    pub(crate) open_pull_request: Option<RepoPullRequestDto>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatAttachmentDto {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) mime_type: String,
    pub(crate) size: u64,
    pub(crate) extension: String,
    pub(crate) kind: String,
    pub(crate) state: String,
    pub(crate) source: ChatAttachmentSourceDto,
}

#[derive(Debug, Clone, Serialize)]
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
        session_store: &SessionStore,
        session_id: Uuid,
        attachment: &StoredAttachment,
    ) -> Self {
        let state = match session_store.attachment_state(session_id, attachment) {
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
                path: session_store
                    .attachment_original_path(session_id, attachment)
                    .display()
                    .to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepoActionResultDto {
    pub(crate) ok: bool,
    pub(crate) action: String,
    pub(crate) message: String,
    pub(crate) repo_status: RepoStatusDto,
    pub(crate) pull_request: Option<RepoPullRequestDto>,
}

#[derive(Debug, Clone, Serialize)]
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
        metadata: Option<Value>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionDetailDto {
    pub(crate) session_id: String,
    pub(crate) display_name: Option<String>,
    pub(crate) generated_title: Option<String>,
    pub(crate) title: String,
    pub(crate) cwd: String,
    pub(crate) folder_path: String,
    pub(crate) updated_at_ms: u64,
    pub(crate) created_at_ms: u64,
    pub(crate) event_count: usize,
    pub(crate) activity_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) active_turn_id: Option<Option<String>>,
    pub(crate) slug: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) note: Option<String>,
    pub(crate) parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model_id: Option<String>,
    pub(crate) timeline: Vec<TimelineItemDto>,
    pub(crate) latest_diff: Option<DiffSummaryDto>,
    pub(crate) diff_history: Vec<DiffSummaryDto>,
    pub(crate) repo_status: RepoStatusDto,
    /// Reconstructed from the transcript's editing tool calls — what the
    /// agent thinks it changed, in order. Independent of `latest_diff`,
    /// which is the on-disk git view; pairing the two surfaces drift
    /// (e.g. a hand-edit between turns or a hook rolling work back).
    pub(crate) agent_diff: AgentDiffDto,
    /// Set difference between agent-touched paths and git-touched paths.
    /// Empty `agent_only` + empty `git_only` means the two views agree
    /// on which files moved; differences mean something to look at.
    pub(crate) divergence: DivergenceReportDto,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentDiffDto {
    /// Files the agent edited at least once during this session, with the
    /// most recent intent per file. Sorted alphabetically.
    pub(crate) files: Vec<AgentDiffFileDto>,
    /// Per-call breakdown — preserves the order edits happened so the UI
    /// can show "edit 3 of 7" style context if it wants to.
    pub(crate) entries: Vec<AgentDiffEntryDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentDiffFileDto {
    pub(crate) path: String,
    pub(crate) latest_kind: String,
    pub(crate) edit_count: usize,
    /// Last successful edit summary — a unified-diff-ish snippet
    /// reconstructed from the tool's input (e.g. old → new for
    /// replace_in_file, full contents for write_file). Suitable for
    /// rendering in a code block.
    pub(crate) latest_summary: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentDiffEntryDto {
    pub(crate) call_id: String,
    pub(crate) tool_id: String,
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) success: bool,
    pub(crate) summary: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DivergenceReportDto {
    /// Files the agent claims to have edited that don't appear in the
    /// current git diff. Could mean the agent's edit was reverted,
    /// formatted away, or never landed (failed apply).
    pub(crate) agent_only: Vec<String>,
    /// Files git sees as changed that no agent tool call touched. Could
    /// mean a hook rewrote the file, the user hand-edited between turns,
    /// or a build artifact slipped in.
    pub(crate) git_only: Vec<String>,
    /// Total files in the agent diff (denominator for badges).
    pub(crate) agent_total: usize,
    /// Total files in the git diff (denominator for badges).
    pub(crate) git_total: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsSnapshotDto {
    pub(crate) workspace_root: String,
    pub(crate) workspace_config_file: String,
    pub(crate) user_config_file: String,
    pub(crate) auth_store_file: String,
    pub(crate) builtin_resources_dir: String,
    pub(crate) config: SettingsConfigDto,
    pub(crate) resources: ResourceCountsDto,
    pub(crate) sessions: SettingsSessionSummaryDto,
    pub(crate) auth: Vec<AuthProviderStatusDto>,
    pub(crate) providers: Vec<ProviderSummaryDto>,
    pub(crate) browser: BrowserSettingsDto,
    pub(crate) network_proxy: NetworkProxySettingsDto,
    pub(crate) secrets: SecretsSettingsDto,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserSettingsDto {
    pub(crate) extensions_enabled: bool,
    pub(crate) extensions: Vec<BrowserExtensionDto>,
    pub(crate) captcha: BrowserCaptchaSettingsDto,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserExtensionDto {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) path: String,
    pub(crate) enabled: bool,
    pub(crate) manifest_present: bool,
    pub(crate) source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserCaptchaSettingsDto {
    pub(crate) enabled: bool,
    pub(crate) selected_solver: String,
    pub(crate) solvers: Vec<BrowserCaptchaSolverDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserCaptchaSolverDto {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) description: String,
    pub(crate) enabled: bool,
    pub(crate) base_url: String,
    pub(crate) api_key_secret_id: Option<String>,
    pub(crate) has_api_key: bool,
    pub(crate) version: String,
    pub(crate) bundled: bool,
    pub(crate) extension_path: String,
    pub(crate) release_url: String,
    pub(crate) download_url: String,
    pub(crate) sha256: String,
    pub(crate) license: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecretsSettingsDto {
    pub(crate) store_file: String,
    pub(crate) key_source: String,
    pub(crate) chrome_import_supported: bool,
    pub(crate) items: Vec<SecretSummaryDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecretSummaryDto {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) description: Option<String>,
    pub(crate) username: Option<String>,
    pub(crate) origin: Option<String>,
    pub(crate) source: String,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NetworkProxySettingsDto {
    pub(crate) enabled: bool,
    pub(crate) selected: Option<String>,
    pub(crate) bypass: Vec<String>,
    pub(crate) proxies: Vec<SanitizedProxyEndpointDto>,
    pub(crate) last_test: Option<ProxyTestResultDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SanitizedProxyEndpointDto {
    pub(crate) id: String,
    pub(crate) scheme: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) username: Option<String>,
    pub(crate) has_password: bool,
    pub(crate) uri: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProxyTestResultDto {
    pub(crate) proxy_id: Option<String>,
    pub(crate) ok: bool,
    pub(crate) message: String,
    pub(crate) latency_ms: Option<u64>,
    pub(crate) status_code: Option<u16>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveProxySettingsParams {
    pub(crate) enabled: bool,
    pub(crate) selected: Option<String>,
    pub(crate) bypass: Vec<String>,
    pub(crate) proxies: Vec<ProxyEndpointInputDto>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveBrowserSettingsParams {
    pub(crate) extensions_enabled: bool,
    #[serde(default)]
    pub(crate) extensions: Vec<SaveBrowserExtensionParams>,
    pub(crate) captcha: SaveBrowserCaptchaSettingsParams,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveBrowserExtensionParams {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) path: String,
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveBrowserCaptchaSettingsParams {
    pub(crate) enabled: bool,
    pub(crate) selected_solver: String,
    #[serde(default)]
    pub(crate) solvers: Vec<SaveBrowserCaptchaSolverParams>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveBrowserCaptchaSolverParams {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) base_url: Option<String>,
    #[serde(default)]
    pub(crate) api_key_secret_id: Option<String>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProxyEndpointInputDto {
    pub(crate) id: String,
    pub(crate) scheme: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) username: Option<String>,
    pub(crate) password: Option<String>,
    #[serde(default)]
    pub(crate) keep_password: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsConfigDto {
    pub(crate) app_name: String,
    pub(crate) default_provider: Option<String>,
    pub(crate) default_model: Option<String>,
    pub(crate) openai_base_url: Option<String>,
    pub(crate) theme: String,
    pub(crate) media: MediaSettingsDto,
    pub(crate) mascot_id: String,
    pub(crate) mascot_display_name: String,
    pub(crate) mascot_enabled: bool,
    pub(crate) ui_no_alt_screen: bool,
    pub(crate) ui_tmux_golden_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaSettingsDto {
    pub(crate) image: Option<MediaGenerationSettingsDto>,
    pub(crate) video: Option<MediaGenerationSettingsDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaGenerationSettingsDto {
    pub(crate) provider_id: String,
    pub(crate) logical_model_id: String,
    pub(crate) selections: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaCapabilityInfoDto {
    pub(crate) provider_id: String,
    pub(crate) provider_display_name: String,
    pub(crate) model_id: String,
    pub(crate) model_display_name: String,
    pub(crate) kind: String,
    pub(crate) operation: String,
    pub(crate) adapter: String,
    pub(crate) axes: Vec<MediaCapabilityAxisDto>,
    pub(crate) status: String,
    pub(crate) source: String,
    pub(crate) reason: Option<String>,
    pub(crate) checked_at_ms: u64,
}

/// One user-facing axis of a logical media model. `control` carries the typed
/// `ControlKind` JSON (`{"enum": {...}}` / `{"range": {...}}` / `{"bool": {...}}`),
/// while provider request fields remain backend-only resolver details.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaCapabilityAxisDto {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) role: String,
    pub(crate) control: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceCountsDto {
    pub(crate) providers: usize,
    pub(crate) tools: usize,
    pub(crate) internal_tools: usize,
    pub(crate) agents: usize,
    pub(crate) prompts: usize,
    pub(crate) hooks: usize,
    pub(crate) skills: usize,
    pub(crate) mascots: usize,
    pub(crate) plugins: usize,
    pub(crate) mcp_servers: usize,
    pub(crate) ides: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsSessionSummaryDto {
    pub(crate) total_sessions: usize,
    pub(crate) folder_groups: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthProviderStatusDto {
    pub(crate) provider_id: String,
    pub(crate) kind: String,
    pub(crate) email: Option<String>,
    pub(crate) expires_at_ms: Option<u64>,
    pub(crate) scopes: Vec<String>,
    pub(crate) plan_type: Option<String>,
    pub(crate) organization_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderSummaryDto {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) base_url: String,
    pub(crate) default_api: String,
    pub(crate) model_count: usize,
    pub(crate) auth_modes: Vec<String>,
    pub(crate) source_kind: String,
    pub(crate) source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalCredentialDto {
    pub(crate) provider_id: String,
    pub(crate) source: String,
    pub(crate) kind: String,
    pub(crate) description: String,
    pub(crate) source_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpServerDto {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) description: String,
    pub(crate) transport: String,
    pub(crate) endpoint: String,
    pub(crate) target: String,
    pub(crate) source_kind: String,
    pub(crate) source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThinkingOptionDto {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) is_default: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelDescriptorDto {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) provider: String,
    pub(crate) api: String,
    pub(crate) context_window: u32,
    pub(crate) max_output_tokens: u32,
    pub(crate) supports_reasoning: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) thinking_options: Vec<ThinkingOptionDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) default_thinking_option_id: Option<String>,
}
