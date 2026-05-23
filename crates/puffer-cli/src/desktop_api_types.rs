use puffer_session_store::MessageActor;
use serde::Serialize;
use serde_json::Value;

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
    Command {
        id: String,
        command_name: String,
        command_args: String,
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
        actor: Option<MessageActor>,
    },
    DiffSnapshot {
        id: String,
        snapshot: DiffSummaryDto,
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsConfigDto {
    pub(crate) app_name: String,
    pub(crate) default_provider: Option<String>,
    pub(crate) default_model: Option<String>,
    pub(crate) openai_base_url: Option<String>,
    pub(crate) theme: String,
    pub(crate) mascot_id: String,
    pub(crate) mascot_display_name: String,
    pub(crate) mascot_enabled: bool,
    pub(crate) ui_no_alt_screen: bool,
    pub(crate) ui_tmux_golden_mode: bool,
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
