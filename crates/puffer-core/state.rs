use puffer_config::PufferConfig;
use puffer_session_store::{
    ClaudeReadSnapshotEvent, SessionMetadata, SessionRecord, TranscriptEvent, TranscriptRewrite,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Describes the role of a rendered transcript message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    /// Structured tool call — preserves call_id and tool name for API reconstruction.
    ToolCall,
    /// Structured tool result — preserves call_id for API reconstruction.
    ToolResult,
}

/// Represents one rendered transcript message in the interactive UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedMessage {
    pub role: MessageRole,
    pub text: String,
    /// Tool call ID for ToolCall/ToolResult roles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// Tool name for ToolCall role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    /// Tool input JSON for ToolCall role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<String>,
    /// Whether the tool call succeeded (ToolResult role).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
}

/// Describes the completion state of one recorded task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TaskStatus {
    Completed,
    Failed,
}

/// Represents one recorded shell or tool task in the current session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRecord {
    pub id: u64,
    pub label: String,
    pub detail: String,
    pub status: TaskStatus,
}

/// Stores the last successful Claude-style read metadata for one absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudeReadState {
    pub(crate) timestamp_ms: u128,
    pub(crate) is_partial_view: bool,
}

/// Stores the mutable session and UI state for one interactive Puffer run.
#[derive(Debug, Clone)]
pub struct AppState {
    pub config: PufferConfig,
    pub cwd: PathBuf,
    pub working_dirs: Vec<PathBuf>,
    pub session: SessionMetadata,
    pub transcript: Vec<RenderedMessage>,
    pub current_model: Option<String>,
    pub current_provider: Option<String>,
    pub prompt_color: String,
    pub effort_level: String,
    pub fast_mode: bool,
    pub plan_mode: bool,
    pub(crate) plan_mode_attachment_turns: usize,
    pub(crate) plan_mode_attachment_count: usize,
    pub(crate) plan_mode_has_exited: bool,
    pub(crate) plan_mode_needs_reentry_attachment: bool,
    pub(crate) plan_mode_needs_exit_attachment: bool,
    pub sandbox_mode: String,
    pub remote_name: Option<String>,
    pub remote_environment: Option<String>,
    pub remote_session_id: Option<String>,
    pub remote_session_url: Option<String>,
    pub remote_session_status: Option<String>,
    pub active_team_name: Option<String>,
    pub statusline_enabled: bool,
    pub status_line_text: Option<String>,
    pub vim_mode: bool,
    pub should_exit: bool,
    pub reload_resources_requested: bool,
    pub(crate) claude_read_state: HashMap<PathBuf, ClaudeReadState>,
    pub session_tool_permissions: HashMap<String, String>,
    /// When true, all tool permission prompts are auto-allowed for the session.
    pub session_allow_all: bool,
    pub(crate) native_structured_output_unsupported: HashSet<String>,
    pub(crate) status_line_signature: Option<String>,
    pending_query_prompt: Option<String>,
    /// Last API-reported input token count (from `usage.input_tokens`).
    /// Updated after each Responses API call for accurate context-window display.
    pub last_input_tokens: Option<u32>,
    tasks: Vec<TaskRecord>,
    next_task_id: u64,
}

impl AppState {
    /// Creates a new application state for the active session.
    pub fn new(config: PufferConfig, cwd: PathBuf, session: SessionMetadata) -> Self {
        let current_model = config.default_model.clone();
        let current_provider = config.default_provider.clone();
        let effort_level = config
            .effort_level
            .clone()
            .unwrap_or_else(|| "auto".to_string());
        let fast_mode = config.fast_mode;
        let vim_mode = config.editor_mode == "vim";
        Self {
            current_model,
            current_provider,
            effort_level,
            fast_mode,
            config,
            cwd,
            working_dirs: Vec::new(),
            session,
            transcript: Vec::new(),
            prompt_color: "default".to_string(),
            plan_mode: false,
            plan_mode_attachment_turns: 0,
            plan_mode_attachment_count: 0,
            plan_mode_has_exited: false,
            plan_mode_needs_reentry_attachment: false,
            plan_mode_needs_exit_attachment: false,
            sandbox_mode: "workspace-write".to_string(),
            remote_name: None,
            remote_environment: None,
            remote_session_id: None,
            remote_session_url: None,
            remote_session_status: None,
            active_team_name: None,
            statusline_enabled: true,
            status_line_text: None,
            vim_mode,
            should_exit: false,
            reload_resources_requested: false,
            claude_read_state: HashMap::new(),
            session_tool_permissions: HashMap::new(),
            session_allow_all: false,
            native_structured_output_unsupported: HashSet::new(),
            status_line_signature: None,
            pending_query_prompt: None,
            last_input_tokens: None,
            tasks: Vec::new(),
            next_task_id: 1,
        }
    }

    /// Restores application state from a persisted session record.
    pub fn from_session_record(config: PufferConfig, session: SessionRecord) -> Self {
        let cwd = session.metadata.cwd.clone();
        let mut state = Self::new(config, cwd, session.metadata);
        for event in session.events {
            match event {
                TranscriptEvent::UserMessage { text } => {
                    state.push_message(MessageRole::User, text)
                }
                TranscriptEvent::AssistantMessage { text } => {
                    state.push_message(MessageRole::Assistant, text)
                }
                TranscriptEvent::SystemMessage { text } => {
                    state.push_message(MessageRole::System, text)
                }
                TranscriptEvent::ToolInvocation {
                    call_id,
                    tool_id,
                    input,
                    output,
                    success,
                } => {
                    state.push_tool_invocation(&call_id, &tool_id, &input, &output, success);
                }
                TranscriptEvent::CommandInvoked { .. } => {}
                TranscriptEvent::SessionRenamed { name } => {
                    state.session.display_name = Some(name);
                }
                TranscriptEvent::GitDiffSnapshot { .. } => {}
                TranscriptEvent::TranscriptRewritten { rewrite } => {
                    state.apply_transcript_rewrite(&rewrite);
                }
                TranscriptEvent::StateSnapshot {
                    current_model,
                    current_provider,
                    theme,
                    prompt_color,
                    effort_level,
                    fast_mode,
                    plan_mode,
                    plan_mode_attachment_turns,
                    plan_mode_attachment_count,
                    plan_mode_has_exited,
                    plan_mode_needs_reentry_attachment,
                    plan_mode_needs_exit_attachment,
                    sandbox_mode,
                    remote_name,
                    remote_environment,
                    remote_session_id,
                    remote_session_url,
                    remote_session_status,
                    active_team_name,
                    statusline_enabled,
                    working_dirs,
                    claude_read_state,
                } => {
                    state.current_model = current_model;
                    state.current_provider = current_provider;
                    state.config.theme = theme;
                    state.prompt_color = prompt_color;
                    state.effort_level = effort_level;
                    state.fast_mode = fast_mode;
                    state.plan_mode = plan_mode;
                    state.plan_mode_attachment_turns = plan_mode_attachment_turns;
                    state.plan_mode_attachment_count = plan_mode_attachment_count;
                    state.plan_mode_has_exited = plan_mode_has_exited;
                    state.plan_mode_needs_reentry_attachment = plan_mode_needs_reentry_attachment;
                    state.plan_mode_needs_exit_attachment = plan_mode_needs_exit_attachment;
                    state.sandbox_mode = sandbox_mode;
                    state.remote_name = remote_name;
                    state.remote_environment = remote_environment;
                    state.remote_session_id = remote_session_id;
                    state.remote_session_url = remote_session_url;
                    state.remote_session_status = remote_session_status;
                    state.active_team_name = active_team_name;
                    state.statusline_enabled = statusline_enabled;
                    state.working_dirs = working_dirs.into_iter().map(Into::into).collect();
                    state.claude_read_state = claude_read_state
                        .into_iter()
                        .map(|entry| {
                            (
                                PathBuf::from(entry.path),
                                ClaudeReadState {
                                    timestamp_ms: entry.timestamp_ms,
                                    is_partial_view: entry.is_partial_view,
                                },
                            )
                        })
                        .collect();
                }
            }
        }
        state
    }

    /// Appends a rendered message to the in-memory transcript.
    pub fn push_message(&mut self, role: MessageRole, text: impl Into<String>) {
        self.transcript.push(RenderedMessage {
            role,
            text: text.into(),
            call_id: None,
            tool_id: None,
            tool_input: None,
            success: None,
        });
    }

    /// Appends a structured tool invocation (call + result) to the transcript.
    pub fn push_tool_invocation(
        &mut self,
        call_id: &str,
        tool_id: &str,
        input: &str,
        output: &str,
        success: bool,
    ) {
        self.transcript.push(RenderedMessage {
            role: MessageRole::ToolCall,
            text: input.to_string(),
            call_id: Some(call_id.to_string()),
            tool_id: Some(tool_id.to_string()),
            tool_input: Some(input.to_string()),
            success: None,
        });
        self.transcript.push(RenderedMessage {
            role: MessageRole::ToolResult,
            text: output.to_string(),
            call_id: Some(call_id.to_string()),
            tool_id: Some(tool_id.to_string()),
            tool_input: None,
            success: Some(success),
        });
    }

    /// Applies one transcript rewrite operation to in-memory transcript state.
    pub fn apply_transcript_rewrite(&mut self, rewrite: &TranscriptRewrite) {
        match rewrite {
            TranscriptRewrite::Clear => self.transcript.clear(),
            TranscriptRewrite::PopLast { count } => {
                for _ in 0..*count {
                    if self.transcript.pop().is_none() {
                        break;
                    }
                }
            }
        }
    }

    /// Records one completed or failed task in the current runtime session state.
    pub fn record_task(
        &mut self,
        label: impl Into<String>,
        detail: impl Into<String>,
        success: bool,
    ) {
        let task = TaskRecord {
            id: self.next_task_id,
            label: label.into(),
            detail: detail.into(),
            status: if success {
                TaskStatus::Completed
            } else {
                TaskStatus::Failed
            },
        };
        self.next_task_id += 1;
        self.tasks.push(task);
    }

    /// Returns the cached status-line input signature, when one has been recorded.
    pub fn status_line_signature(&self) -> Option<&str> {
        self.status_line_signature.as_deref()
    }

    /// Replaces the cached status-line input signature for the active session.
    pub fn set_status_line_signature(&mut self, signature: Option<String>) {
        self.status_line_signature = signature;
    }

    /// Queues a follow-up provider prompt requested by a local command.
    pub fn queue_pending_query_prompt(&mut self, prompt: impl Into<String>) {
        let prompt = prompt.into();
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            self.pending_query_prompt = None;
            return;
        }
        self.pending_query_prompt = Some(trimmed.to_string());
    }

    /// Returns true when a local command queued a follow-up provider prompt.
    pub fn has_pending_query_prompt(&self) -> bool {
        self.pending_query_prompt.is_some()
    }

    /// Drains the queued follow-up provider prompt, if one exists.
    pub fn take_pending_query_prompt(&mut self) -> Option<String> {
        self.pending_query_prompt.take()
    }

    /// Builds a persisted snapshot event for the current mutable session state.
    pub fn snapshot_event(&self) -> TranscriptEvent {
        TranscriptEvent::StateSnapshot {
            current_model: self.current_model.clone(),
            current_provider: self.current_provider.clone(),
            theme: self.config.theme.clone(),
            prompt_color: self.prompt_color.clone(),
            effort_level: self.effort_level.clone(),
            fast_mode: self.fast_mode,
            plan_mode: self.plan_mode,
            plan_mode_attachment_turns: self.plan_mode_attachment_turns,
            plan_mode_attachment_count: self.plan_mode_attachment_count,
            plan_mode_has_exited: self.plan_mode_has_exited,
            plan_mode_needs_reentry_attachment: self.plan_mode_needs_reentry_attachment,
            plan_mode_needs_exit_attachment: self.plan_mode_needs_exit_attachment,
            sandbox_mode: self.sandbox_mode.clone(),
            remote_name: self.remote_name.clone(),
            remote_environment: self.remote_environment.clone(),
            remote_session_id: self.remote_session_id.clone(),
            remote_session_url: self.remote_session_url.clone(),
            remote_session_status: self.remote_session_status.clone(),
            active_team_name: self.active_team_name.clone(),
            statusline_enabled: self.statusline_enabled,
            working_dirs: self
                .working_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            claude_read_state: self
                .claude_read_state
                .iter()
                .map(|(path, snapshot)| ClaudeReadSnapshotEvent {
                    path: path.display().to_string(),
                    timestamp_ms: snapshot.timestamp_ms,
                    is_partial_view: snapshot.is_partial_view,
                })
                .collect(),
        }
    }

    pub(crate) fn tasks(&self) -> &[TaskRecord] {
        &self.tasks
    }

    pub(crate) fn allow_tool_for_session(&mut self, tool_id: &str) {
        self.session_tool_permissions
            .insert(tool_id.to_ascii_lowercase(), "allow".to_string());
    }

    pub(crate) fn native_structured_output_key(
        family: &str,
        provider_id: &str,
        model_id: &str,
        endpoint_id: &str,
    ) -> String {
        let endpoint_id = endpoint_id
            .trim()
            .trim_end_matches('/')
            .to_ascii_lowercase();
        format!(
            "{}::{}::{}::{}",
            family.trim().to_ascii_lowercase(),
            provider_id.trim().to_ascii_lowercase(),
            model_id.trim().to_ascii_lowercase(),
            endpoint_id
        )
    }

    pub(crate) fn is_native_structured_output_unsupported(
        &self,
        family: &str,
        provider_id: &str,
        model_id: &str,
        endpoint_id: &str,
    ) -> bool {
        self.native_structured_output_unsupported
            .contains(&Self::native_structured_output_key(
                family,
                provider_id,
                model_id,
                endpoint_id,
            ))
    }

    pub(crate) fn mark_native_structured_output_unsupported(
        &mut self,
        family: &str,
        provider_id: &str,
        model_id: &str,
        endpoint_id: &str,
    ) {
        self.native_structured_output_unsupported
            .insert(Self::native_structured_output_key(
                family,
                provider_id,
                model_id,
                endpoint_id,
            ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_session_store::TranscriptEvent;
    use uuid::Uuid;

    fn sample_metadata() -> SessionMetadata {
        SessionMetadata {
            id: Uuid::new_v4(),
            display_name: Some("sample".to_string()),
            cwd: PathBuf::from("."),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        }
    }

    #[test]
    fn session_restore_replays_transcript_rewrite_events() {
        let record = SessionRecord {
            metadata: sample_metadata(),
            events: vec![
                TranscriptEvent::UserMessage {
                    text: "u1".to_string(),
                },
                TranscriptEvent::AssistantMessage {
                    text: "a1".to_string(),
                },
                TranscriptEvent::TranscriptRewritten {
                    rewrite: TranscriptRewrite::PopLast { count: 1 },
                },
                TranscriptEvent::SystemMessage {
                    text: "after-pop".to_string(),
                },
                TranscriptEvent::TranscriptRewritten {
                    rewrite: TranscriptRewrite::Clear,
                },
                TranscriptEvent::SystemMessage {
                    text: "after-clear".to_string(),
                },
            ],
        };
        let state = AppState::from_session_record(PufferConfig::default(), record);
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.transcript[0].role, MessageRole::System);
        assert_eq!(state.transcript[0].text, "after-clear");
    }

    #[test]
    fn session_restore_ignores_command_invoked_for_transcript_reconstruction() {
        let record = SessionRecord {
            metadata: sample_metadata(),
            events: vec![
                TranscriptEvent::UserMessage {
                    text: "before".to_string(),
                },
                TranscriptEvent::CommandInvoked {
                    name: "help".to_string(),
                    args: String::new(),
                },
                TranscriptEvent::SystemMessage {
                    text: "done".to_string(),
                },
            ],
        };
        let state = AppState::from_session_record(PufferConfig::default(), record);
        let lines = state
            .transcript
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(lines, vec!["before", "done"]);
    }

    #[test]
    fn native_structured_output_cache_key_changes_by_endpoint() {
        let official = AppState::native_structured_output_key(
            "openai",
            "openai",
            "gpt-5",
            "https://api.openai.com/",
        );
        let proxy = AppState::native_structured_output_key(
            "openai",
            "openai",
            "gpt-5",
            "http://84.32.32.146:8317",
        );
        assert_ne!(official, proxy);
    }
}
