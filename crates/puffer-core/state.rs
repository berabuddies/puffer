use crate::memory::ProjectMemoryContext;
use crate::permissions::browser_action::browser_permission_value_for_tool_call;
use crate::permissions::browser_grants::BrowserGrantScopeKind;
use crate::permissions::SessionPermissionState;
use crate::runner_adapter::LocalToolRunner;
use crate::runtime::ReflectionConfig;
use puffer_config::PufferConfig;
use puffer_runner_api::ToolRunner;
use puffer_session_store::{
    ClaudeReadSnapshotEvent, MessageActor, MessageActorKind, SessionMetadata, SessionRecord,
    TranscriptEvent, TranscriptRewrite,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
    /// Model thinking / reasoning content shown before the main text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BrowserUrlCacheKey {
    root_session_id: String,
    tab_id: Option<String>,
}

/// Status of a session goal. Mirrors the four-state machine codex
/// uses (`codex/codex-rs/state/src/model/thread_goal.rs:11-36`):
/// `Active` is the sole non-terminal state; `Paused` is a transient
/// interruption (reserved for future Ctrl+C wiring); `BudgetLimited`
/// fires automatically when `tokens_used >= token_budget`; `Complete`
/// is set by the model via the `update_goal` tool. We deliberately
/// match codex's vocabulary so cross-runtime telemetry / serialization
/// can line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Active,
    Paused,
    BudgetLimited,
    Complete,
}

impl GoalStatus {
    /// Wire / display tag (matches codex `as_str` / wire camelCase).
    pub fn slug(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::BudgetLimited => "budget_limited",
            Self::Complete => "complete",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }

    /// Codex semantics: `BudgetLimited` and `Complete` are terminal;
    /// `Paused` is a transient interruption that resume can revive.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::BudgetLimited | Self::Complete)
    }
}

/// Session-scoped goal record. Borrows the public-facing shape of
/// Codex's per-thread goal (`codex/codex-rs/core/src/goals.rs`,
/// `codex-rs/state/src/model/thread_goal.rs`): an `objective`,
/// `status`, optional `token_budget`, and live `tokens_used`
/// counter. Persisted across turns within a session but not across
/// sessions (lives only on `AppState`, not in `SessionMetadata` —
/// codex uses SQLite for persistence; puffer is currently in-memory).
///
/// Lifecycle:
///   - `slash_set_goal` (slash command) creates with `status: Active`.
///   - `create_goal` tool (model) creates with `status: Active`,
///     fails if any goal already exists.
///   - `account_token_usage` increments `tokens_used` on every turn;
///     transitions to `BudgetLimited` when the cap is reached.
///   - `mark_complete` tool (model) flips to `Complete`. The
///     model-facing `update_goal` tool intentionally cannot pause /
///     budget-limit / clear — those transitions are reserved for the
///     runtime / user (codex parity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionGoal {
    /// Stable identifier for optimistic concurrency (uuid v4-ish
    /// hex). Mirrors codex's `goal_id` — used so future persistence
    /// can detect goal-replacement races.
    pub goal_id: String,
    /// The user-provided objective string. Trimmed at parse time.
    pub objective: String,
    /// Lifecycle status (see [`GoalStatus`]).
    pub status: GoalStatus,
    /// Optional token budget the user attached. `None` means
    /// open-ended (no auto-transition to `BudgetLimited`).
    pub token_budget: Option<u32>,
    /// Cumulative non-cached input + output tokens consumed since
    /// the goal entered `Active`. Bumped on every turn boundary by
    /// `runtime::goals::account_token_usage`.
    pub tokens_used: u64,
    /// Wall-clock timestamp (Unix ms) when the goal was created.
    pub created_at_ms: u128,
    /// Wall-clock timestamp (Unix ms) of the last status / tokens
    /// mutation. `/goal status` renders elapsed time from this.
    pub updated_at_ms: u128,
}

impl SessionGoal {
    /// Remaining budget if any. `None` means budget unset (or
    /// already exceeded — clamped at 0).
    pub fn remaining_tokens(&self) -> Option<u64> {
        let budget = self.token_budget? as u64;
        Some(budget.saturating_sub(self.tokens_used))
    }
}

/// Stores the mutable session and UI state for one interactive Puffer run.
#[derive(Debug, Clone)]
pub struct AppState {
    pub config: PufferConfig,
    pub cwd: PathBuf,
    pub working_dirs: Vec<PathBuf>,
    pub session: SessionMetadata,
    pub prompt_cache_key_override: Option<String>,
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
    pub current_actor: Option<MessageActor>,
    pub statusline_enabled: bool,
    pub status_line_text: Option<String>,
    pub project_memory: Option<ProjectMemoryContext>,
    pub project_memory_review_turns: usize,
    pub vim_mode: bool,
    /// Session-scoped reflection policy toggled via `/reflect`; `None` means off.
    pub reflection_config: Option<ReflectionConfig>,
    /// Optional persisted goal for the current session, set via `/goal`.
    /// Borrows the shape of Codex's per-thread goal (`codex/codex-rs/core/src/goals.rs`)
    /// — objective text + creation timestamp + optional token budget.
    /// Visible in `/status` so long-running tasks have a reference
    /// point on every check-in.
    pub session_goal: Option<SessionGoal>,
    pub should_exit: bool,
    pub reload_resources_requested: bool,
    /// Cross-thread signal raised by the filesystem watcher when a tracked
    /// skill/MCP/plugin file changes. Mirrors `reload_resources_requested`
    /// but is safe to flip from background threads (e.g. the resource
    /// watcher). The TUI loop ORs both sources together via
    /// [`AppState::take_reload_request`] before deciding to reload.
    pub resource_reload_signal: Arc<AtomicBool>,
    pub(crate) claude_read_state: HashMap<PathBuf, ClaudeReadState>,
    /// Canonical in-memory session permission state used for current-turn
    /// evaluation and worker/UI round-trips.
    pub(crate) session_permission_state: SessionPermissionState,
    browser_url_cache: HashMap<BrowserUrlCacheKey, String>,
    pub(crate) native_structured_output_unsupported: HashSet<String>,
    pub(crate) status_line_signature: Option<String>,
    pending_query_prompt: Option<String>,
    /// Last API-reported input token count (from `usage.input_tokens`).
    /// Updated after each Responses API call for accurate context-window display.
    pub last_input_tokens: Option<u32>,
    /// Wall-clock timestamp of the most recent committed assistant message.
    /// Set by `push_message` when role == Assistant. Consumed by the
    /// microcompact time-based trigger to mirror Claude Code's "gap since
    /// last assistant message" heuristic (`services/compact/microCompact.ts:421`).
    pub last_assistant_at: Option<std::time::SystemTime>,
    /// Cache hit ratio for the most recent provider request (0.0–1.0).
    pub last_cache_hit_ratio: Option<f64>,
    /// Running session-average cache hit ratio (0.0–1.0).
    pub session_cache_hit_ratio: Option<f64>,
    /// Number of turns with cache data, used to compute session average.
    session_cache_turn_count: u64,
    /// Cumulative cache-read tokens across the session.
    session_cache_read_total: u64,
    /// Cumulative input tokens across the session.
    session_input_total: u64,
    tasks: Vec<TaskRecord>,
    next_task_id: u64,
    /// Tool runner used by the claude-parity dispatcher and any subsystem
    /// that needs filesystem/process/MCP access through the trait surface.
    /// Defaults to a `LocalToolRunner`; tests and remote backends override
    /// it via [`AppState::with_tool_runner`].
    pub tool_runner: Arc<dyn ToolRunner>,
    /// In-memory store for interactive/background processes with PTY or pipe I/O.
    pub process_store: Arc<Mutex<crate::runtime::process_store::ProcessStore>>,
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
        let project_memory = if config.memory.enabled {
            ProjectMemoryContext::load(&cwd, config.memory.char_limit)
                .ok()
                .flatten()
        } else {
            None
        };
        // The Skill(project-memory) flow tells the model to call Read on the
        // memory file. The Read tool is sandboxed to working_dirs, so the
        // memory file's parent has to be reachable on the main turn — the
        // side-turn already does this in `prepare_project_memory_side_turn`,
        // but without this the documented Skill+Read flow fails on the main
        // turn and the model falls back to `Bash cat` (which bypasses the
        // sandbox entirely).
        let working_dirs = project_memory
            .as_ref()
            .and_then(|ctx| ctx.memory_file.parent().map(Path::to_path_buf))
            .map(|parent| vec![parent])
            .unwrap_or_default();
        let vim_mode = config.editor_mode == "vim";
        Self {
            current_model,
            current_provider,
            effort_level,
            fast_mode,
            config,
            cwd,
            working_dirs,
            session,
            prompt_cache_key_override: None,
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
            current_actor: None,
            statusline_enabled: true,
            status_line_text: None,
            project_memory,
            project_memory_review_turns: 0,
            vim_mode,
            reflection_config: None,
            session_goal: None,
            should_exit: false,
            reload_resources_requested: false,
            resource_reload_signal: Arc::new(AtomicBool::new(false)),
            claude_read_state: HashMap::new(),
            session_permission_state: SessionPermissionState::default(),
            browser_url_cache: HashMap::new(),
            native_structured_output_unsupported: HashSet::new(),
            status_line_signature: None,
            pending_query_prompt: None,
            last_input_tokens: None,
            last_assistant_at: None,
            last_cache_hit_ratio: None,
            session_cache_hit_ratio: None,
            session_cache_turn_count: 0,
            session_cache_read_total: 0,
            session_input_total: 0,
            tasks: Vec::new(),
            next_task_id: 1,
            tool_runner: Arc::new(LocalToolRunner::new()),
            process_store: Arc::new(Mutex::new(
                crate::runtime::process_store::ProcessStore::default(),
            )),
        }
    }

    /// Replaces the active tool runner. Use this from tests or remote
    /// backends; the default `AppState::new` already installs a local
    /// in-process runner.
    pub fn with_tool_runner(mut self, runner: Arc<dyn ToolRunner>) -> Self {
        self.tool_runner = runner;
        self
    }

    /// Returns true and clears both the command-driven flag and the
    /// background filesystem-watcher signal if either is set. The caller
    /// (typically `puffer-tui`) is then expected to invoke
    /// [`crate::reload_runtime_resources`] before continuing the loop so a
    /// turn never starts against stale resources.
    pub fn take_reload_request(&mut self) -> bool {
        let from_signal = self.resource_reload_signal.swap(false, Ordering::AcqRel);
        let from_flag = std::mem::replace(&mut self.reload_resources_requested, false);
        from_signal || from_flag
    }

    /// Returns a cheap clone of the cross-thread reload signal so background
    /// services (e.g. the filesystem watcher) can raise it from off-loop.
    pub fn reload_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.resource_reload_signal)
    }

    /// Restores application state from a persisted session record.
    ///
    /// Session-store replay currently restores transcript content plus the
    /// persisted UI/runtime snapshot fields carried by
    /// [`TranscriptEvent::StateSnapshot`]. It restores the legacy-compatible
    /// subset of session permission state from `session_allow_all` plus
    /// `session_tool_permissions`, but typed-only narrow grants still remain
    /// outside the durable snapshot schema.
    pub fn from_session_record(config: PufferConfig, session: SessionRecord) -> Self {
        let cwd = session.metadata.cwd.clone();
        let mut state = Self::new(config, cwd, session.metadata);
        for event in session.events {
            match event {
                TranscriptEvent::UserMessage { text, .. } => {
                    state.push_message(MessageRole::User, text)
                }
                TranscriptEvent::AssistantMessage { text, actor } => {
                    state.restore_current_actor(actor);
                    state.push_message(MessageRole::Assistant, text)
                }
                TranscriptEvent::SystemMessage { text, .. } => {
                    state.push_message(MessageRole::System, text)
                }
                TranscriptEvent::ToolInvocation {
                    call_id,
                    tool_id,
                    input,
                    output,
                    success,
                    actor,
                    ..
                } => {
                    state.restore_current_actor(actor);
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
                    session_allow_all,
                    session_tool_permissions,
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
                    state.session_permission_state =
                        SessionPermissionState::from_legacy_snapshot_projection(
                            session_allow_all,
                            &session_tool_permissions,
                        );
                }
            }
        }
        state
    }

    /// Updates per-turn and session-average cache hit ratios from a usage report.
    pub fn update_cache_stats(&mut self, input_tokens: u64, cache_read_tokens: u64) {
        if input_tokens == 0 {
            return;
        }
        let ratio = cache_read_tokens as f64 / input_tokens as f64;
        self.last_cache_hit_ratio = Some(ratio);
        self.session_cache_turn_count += 1;
        self.session_cache_read_total += cache_read_tokens;
        self.session_input_total += input_tokens;
        self.session_cache_hit_ratio =
            Some(self.session_cache_read_total as f64 / self.session_input_total as f64);
    }

    /// Appends a rendered message to the in-memory transcript.
    pub fn push_message(&mut self, role: MessageRole, text: impl Into<String>) {
        let was_assistant = matches!(role, MessageRole::Assistant);
        self.transcript.push(RenderedMessage {
            role,
            text: text.into(),
            thinking: None,
            call_id: None,
            tool_id: None,
            tool_input: None,
            success: None,
        });
        if was_assistant {
            self.last_assistant_at = Some(std::time::SystemTime::now());
        }
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
            thinking: None,
            call_id: Some(call_id.to_string()),
            tool_id: Some(tool_id.to_string()),
            tool_input: Some(input.to_string()),
            success: None,
        });
        self.transcript.push(RenderedMessage {
            role: MessageRole::ToolResult,
            text: output.to_string(),
            thinking: None,
            call_id: Some(call_id.to_string()),
            tool_id: Some(tool_id.to_string()),
            tool_input: Some(input.to_string()),
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
    pub fn memory_enabled(&self) -> bool {
        self.config.memory.enabled
    }

    pub fn memory_review_enabled(&self) -> bool {
        self.config.memory.background_review
    }

    pub fn memory_flush_enabled(&self) -> bool {
        self.config.memory.flush_on_compact
    }

    pub fn memory_review_nudge_interval(&self) -> usize {
        self.config.memory.review_nudge_interval
    }

    pub fn memory_flush_min_turns(&self) -> usize {
        self.config.memory.flush_min_turns
    }

    pub fn refresh_project_memory(&mut self) {
        self.project_memory = if self.config.memory.enabled {
            ProjectMemoryContext::load(&self.cwd, self.config.memory.char_limit)
                .ok()
                .flatten()
        } else {
            None
        };
        // Mirror the parent-dir push from `AppState::new`: when memory is
        // (re)enabled or the project changes, the model needs Read access to
        // the memory file from the main turn.
        if let Some(parent) = self
            .project_memory
            .as_ref()
            .and_then(|ctx| ctx.memory_file.parent())
        {
            let parent = parent.to_path_buf();
            if !self.working_dirs.iter().any(|p| p == &parent) {
                self.working_dirs.push(parent);
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
    ///
    /// This persists a legacy-compatible projection of session permission
    /// state so resume can restore the subset representable as session-wide
    /// allow-all plus whole-tool approvals. Narrower typed grants such as
    /// Browser action categories still remain in-memory only.
    pub fn snapshot_event(&self) -> TranscriptEvent {
        let (session_allow_all, session_tool_permissions) =
            self.session_permission_state.legacy_snapshot_projection();
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
            session_allow_all,
            session_tool_permissions,
        }
    }

    /// Returns the actor that owns assistant/tool output for this state.
    pub fn assistant_actor(&self) -> MessageActor {
        self.current_actor.clone().unwrap_or_else(|| {
            let parent_session_id = self.session.parent_session_id;
            let kind = if parent_session_id.is_some() {
                MessageActorKind::Subagent
            } else {
                MessageActorKind::Agent
            };
            MessageActor {
                kind,
                id: self.session.id.to_string(),
                agent_id: None,
                agent_type: None,
                name: None,
                team_name: self.active_team_name.clone(),
                session_id: Some(self.session.id),
                parent_session_id,
            }
        })
    }

    /// Returns the actor used for user-authored transcript events.
    pub fn user_actor(&self) -> MessageActor {
        MessageActor {
            kind: MessageActorKind::User,
            id: "user".to_string(),
            agent_id: None,
            agent_type: None,
            name: None,
            team_name: self.active_team_name.clone(),
            session_id: Some(self.session.id),
            parent_session_id: self.session.parent_session_id,
        }
    }

    /// Returns the actor used for runtime/system transcript events.
    pub fn system_actor(&self) -> MessageActor {
        MessageActor {
            kind: MessageActorKind::System,
            id: "system".to_string(),
            agent_id: None,
            agent_type: None,
            name: None,
            team_name: self.active_team_name.clone(),
            session_id: Some(self.session.id),
            parent_session_id: self.session.parent_session_id,
        }
    }

    /// Sets the actor for a spawned agent or teammate state.
    pub fn set_current_actor(&mut self, actor: MessageActor) {
        self.current_actor = Some(actor);
    }

    /// Returns the workflow message sender id for the current actor context.
    pub fn workflow_sender_id(&self) -> String {
        if let Some(actor) = self.current_actor.as_ref() {
            return actor.agent_id.clone().unwrap_or_else(|| actor.id.clone());
        }
        if let Some(team_name) = self.active_team_name.as_deref() {
            return format!("team-lead@{team_name}");
        }
        "user".to_string()
    }

    /// Returns a subject actor for tool calls that spawn or manage another agent.
    pub fn tool_subject_actor(&self, tool_id: &str, output: &str) -> Option<MessageActor> {
        let normalized_tool = tool_id.trim().to_ascii_lowercase();
        if !matches!(normalized_tool.as_str(), "agent" | "task") {
            return None;
        }
        let value = serde_json::from_str::<serde_json::Value>(output).ok()?;
        let agent_id = value
            .get("agentId")
            .or_else(|| value.get("agent_id"))
            .and_then(serde_json::Value::as_str)?
            .to_string();
        let agent_type = value
            .get("agentType")
            .or_else(|| value.get("agent_type"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned);
        let name = value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned);
        let team_name = value
            .get("teamName")
            .or_else(|| value.get("team_name"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned);
        Some(MessageActor {
            kind: MessageActorKind::Subagent,
            id: agent_id.clone(),
            agent_id: Some(agent_id),
            agent_type,
            name,
            team_name,
            session_id: None,
            parent_session_id: Some(self.session.id),
        })
    }

    fn restore_current_actor(&mut self, actor: Option<MessageActor>) {
        let Some(actor) = actor else {
            return;
        };
        if matches!(
            actor.kind,
            MessageActorKind::Agent | MessageActorKind::Subagent | MessageActorKind::TeamLead
        ) {
            self.current_actor = Some(actor);
        }
    }

    pub(crate) fn tasks(&self) -> &[TaskRecord] {
        &self.tasks
    }

    /// Returns the session id that desktop Browser panes use for this runtime.
    pub(crate) fn browser_root_session_id(&self) -> uuid::Uuid {
        self.session.parent_session_id.unwrap_or(self.session.id)
    }

    fn permission_session_id_for_tool(
        &self,
        definition: &puffer_tools::ToolDefinition,
    ) -> uuid::Uuid {
        if is_browser_tool_definition(definition) {
            self.browser_root_session_id()
        } else {
            self.session.id
        }
    }

    pub(crate) fn allow_tool_for_session(&mut self, tool_id: &str) {
        let definition = puffer_tools::ToolDefinition {
            id: tool_id.to_string(),
            name: tool_id.to_string(),
            description: String::new(),
            handler: String::new(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: puffer_tools::ToolKind::Custom,
            input_schema: puffer_tools::ToolInputSchema::default(),
            metadata: puffer_tools::ToolMetadata::default(),
            policy: puffer_tools::ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: puffer_tools::ToolDisplayHints::default(),
        };
        self.allow_permission_for_tool_call(&definition, &serde_json::Value::Null);
    }

    /// Grants session-scoped permission for one tool call.
    pub fn allow_permission_for_tool_call(
        &mut self,
        definition: &puffer_tools::ToolDefinition,
        input: &serde_json::Value,
    ) {
        let permission_session_id = self.permission_session_id_for_tool(definition);
        self.session_permission_state.grants_mut().grant_tool_call(
            definition,
            input,
            &permission_session_id,
        );
    }

    /// Grants Browser session-scoped permission for one tool call using the supplied Browser scope.
    pub(crate) fn allow_browser_permission_for_tool_call(
        &mut self,
        definition: &puffer_tools::ToolDefinition,
        input: &serde_json::Value,
        scope: BrowserGrantScopeKind,
    ) {
        if browser_permission_value_for_tool_call(&definition.id, input).is_none() {
            self.allow_permission_for_tool_call(definition, input);
            return;
        }
        let permission_session_id = self.browser_root_session_id();
        self.session_permission_state
            .grants_mut()
            .grant_browser_tool_call(definition, input, &permission_session_id, scope);
    }

    /// Grants filesystem access to one path prefix for the current session.
    pub fn allow_path_for_session(&mut self, path: PathBuf) {
        self.session_permission_state
            .grants_mut()
            .grant_path_prefix(path);
    }

    /// Stores the last resolved non-blank Browser URL for one root session and optional tab.
    pub(crate) fn remember_browser_url(
        &mut self,
        root_session_id: &str,
        tab_id: Option<&str>,
        url: &str,
    ) {
        let trimmed = url.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("about:blank") {
            return;
        }
        let key = BrowserUrlCacheKey {
            root_session_id: root_session_id.to_string(),
            tab_id: tab_id.map(ToString::to_string),
        };
        self.browser_url_cache.insert(key, trimmed.to_string());
    }

    /// Returns the last resolved non-blank Browser URL for one root session and optional tab.
    pub(crate) fn remembered_browser_url(
        &self,
        root_session_id: &str,
        tab_id: Option<&str>,
    ) -> Option<&str> {
        let key = BrowserUrlCacheKey {
            root_session_id: root_session_id.to_string(),
            tab_id: tab_id.map(ToString::to_string),
        };
        self.browser_url_cache.get(&key).map(String::as_str)
    }

    /// Returns the canonical typed session permission state.
    pub fn session_permission_state(&self) -> &SessionPermissionState {
        &self.session_permission_state
    }

    /// Replaces the canonical typed session permission state.
    pub fn replace_session_permission_state(&mut self, state: SessionPermissionState) {
        self.session_permission_state = state;
    }

    /// Grants all tool permissions for the current session.
    pub fn grant_all_tools_for_session(&mut self) {
        self.session_permission_state.set_allow_all_tools(true);
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

fn is_browser_tool_definition(definition: &puffer_tools::ToolDefinition) -> bool {
    definition.handler == "runtime:browser"
        || definition.id.eq_ignore_ascii_case("browser")
        || definition.name.eq_ignore_ascii_case("browser")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{EffectivePermissionProfile, PermissionsSettings, SandboxSettings};
    use puffer_session_store::TranscriptEvent;
    use uuid::Uuid;

    fn sample_metadata() -> SessionMetadata {
        SessionMetadata {
            id: Uuid::new_v4(),
            display_name: Some("sample".to_string()),
            generated_title: None,
            cwd: PathBuf::from("."),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        }
    }

    fn browser_tool_definition() -> puffer_tools::ToolDefinition {
        puffer_tools::ToolDefinition {
            id: "Browser".to_string(),
            name: "Browser".to_string(),
            description: "Managed browser".to_string(),
            handler: "runtime:browser".to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: puffer_tools::ToolKind::Custom,
            input_schema: puffer_tools::ToolInputSchema::default(),
            metadata: puffer_tools::ToolMetadata::default(),
            policy: puffer_tools::ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: puffer_tools::ToolDisplayHints::default(),
        }
    }

    #[test]
    fn browser_root_session_id_prefers_parent_session() {
        let parent = Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
        let child = Uuid::parse_str("b4f239fd-1493-4be7-a3a1-9e58fe612576").unwrap();
        let mut metadata = sample_metadata();
        metadata.id = child;
        metadata.parent_session_id = Some(parent);
        let state = AppState::new(PufferConfig::default(), PathBuf::from("."), metadata);

        assert_eq!(state.browser_root_session_id(), parent);
    }

    #[test]
    fn browser_session_grants_use_parent_session_for_nested_agents() {
        let parent = Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
        let child = Uuid::parse_str("b4f239fd-1493-4be7-a3a1-9e58fe612576").unwrap();
        let mut metadata = sample_metadata();
        metadata.id = child;
        metadata.parent_session_id = Some(parent);
        let mut state = AppState::new(PufferConfig::default(), PathBuf::from("."), metadata);

        state.allow_permission_for_tool_call(
            &browser_tool_definition(),
            &serde_json::json!({"action":"evaluate", "script":"document.title"}),
        );
        let profile = EffectivePermissionProfile::from_session_state(
            Path::new("."),
            &[],
            &PermissionsSettings::default(),
            &SandboxSettings::from_mode("workspace-write"),
            &parent,
            state.session_permission_state(),
            false,
            None,
            None,
        );

        assert!(profile.browser_session_grant_allows(&serde_json::json!({
            "action":"evaluate",
            "script":"window.location.href"
        })));
        assert!(!profile.browser_session_grant_allows(&serde_json::json!({
            "action":"evaluate",
            "sessionId": child.to_string(),
            "script":"window.location.href"
        })));
    }

    #[test]
    fn session_restore_replays_transcript_rewrite_events() {
        let record = SessionRecord {
            metadata: sample_metadata(),
            events: vec![
                TranscriptEvent::UserMessage {
                    text: "u1".to_string(),
                    actor: None,
                },
                TranscriptEvent::AssistantMessage {
                    text: "a1".to_string(),
                    actor: None,
                },
                TranscriptEvent::TranscriptRewritten {
                    rewrite: TranscriptRewrite::PopLast { count: 1 },
                },
                TranscriptEvent::SystemMessage {
                    text: "after-pop".to_string(),
                    actor: None,
                },
                TranscriptEvent::TranscriptRewritten {
                    rewrite: TranscriptRewrite::Clear,
                },
                TranscriptEvent::SystemMessage {
                    text: "after-clear".to_string(),
                    actor: None,
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
                    actor: None,
                },
                TranscriptEvent::CommandInvoked {
                    name: "help".to_string(),
                    args: String::new(),
                    actor: None,
                },
                TranscriptEvent::SystemMessage {
                    text: "done".to_string(),
                    actor: None,
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
    fn typed_browser_session_grants_remain_in_memory_only() {
        let mut state = AppState::new(
            PufferConfig::default(),
            PathBuf::from("."),
            sample_metadata(),
        );
        let browser = puffer_tools::ToolDefinition {
            id: "Browser".to_string(),
            name: "Browser".to_string(),
            description: String::new(),
            handler: String::new(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: puffer_tools::ToolKind::Custom,
            input_schema: puffer_tools::ToolInputSchema::default(),
            metadata: puffer_tools::ToolMetadata::default(),
            policy: puffer_tools::ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: puffer_tools::ToolDisplayHints::default(),
        };
        let input = serde_json::json!({
            "action": "evaluate",
            "script": "document.title"
        });

        state.allow_browser_permission_for_tool_call(
            &browser,
            &input,
            BrowserGrantScopeKind::AllowOnce,
        );

        assert!(state.session_permission_state().has_browser_grant());
    }

    #[test]
    fn session_restore_from_state_snapshot_restores_legacy_compatible_permissions() {
        let mut state = AppState::new(
            PufferConfig::default(),
            PathBuf::from("."),
            sample_metadata(),
        );
        let browser = puffer_tools::ToolDefinition {
            id: "Browser".to_string(),
            name: "Browser".to_string(),
            description: String::new(),
            handler: String::new(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: puffer_tools::ToolKind::Custom,
            input_schema: puffer_tools::ToolInputSchema::default(),
            metadata: puffer_tools::ToolMetadata::default(),
            policy: puffer_tools::ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: puffer_tools::ToolDisplayHints::default(),
        };
        state.allow_tool_for_session("Bash");
        state.allow_browser_permission_for_tool_call(
            &browser,
            &serde_json::json!({
                "action": "evaluate",
                "script": "document.title"
            }),
            BrowserGrantScopeKind::AllowOnce,
        );
        state.grant_all_tools_for_session();

        assert!(state.session_permission_state().allow_all_tools());
        assert!(state.session_permission_state().has_browser_grant());

        let record = SessionRecord {
            metadata: state.session.clone(),
            events: vec![state.snapshot_event()],
        };
        let restored = AppState::from_session_record(PufferConfig::default(), record);

        assert!(restored.session_permission_state().allow_all_tools());
        assert_eq!(
            restored.session_permission_state(),
            &SessionPermissionState::from_legacy_snapshot_projection(
                true,
                &std::collections::HashMap::from([("bash".to_string(), "allow".to_string())])
            )
        );
        assert!(!restored.session_permission_state().has_browser_grant());
    }

    #[test]
    fn assistant_actor_defaults_to_agent_for_root_sessions() {
        let state = AppState::new(
            PufferConfig::default(),
            PathBuf::from("."),
            sample_metadata(),
        );
        let actor = state.assistant_actor();
        assert_eq!(actor.kind, MessageActorKind::Agent);
        assert_eq!(actor.session_id, Some(state.session.id));
        assert_eq!(actor.parent_session_id, None);
    }

    #[test]
    fn assistant_actor_defaults_to_subagent_for_child_sessions() {
        let mut metadata = sample_metadata();
        let parent_id = Uuid::new_v4();
        metadata.parent_session_id = Some(parent_id);
        let state = AppState::new(PufferConfig::default(), PathBuf::from("."), metadata);
        let actor = state.assistant_actor();
        assert_eq!(actor.kind, MessageActorKind::Subagent);
        assert_eq!(actor.session_id, Some(state.session.id));
        assert_eq!(actor.parent_session_id, Some(parent_id));
    }

    #[test]
    fn workflow_sender_preserves_root_user_and_subagent_agent_ids() {
        let mut state = AppState::new(
            PufferConfig::default(),
            PathBuf::from("."),
            sample_metadata(),
        );
        assert_eq!(state.workflow_sender_id(), "user");

        state.active_team_name = Some("dockyard".to_string());
        assert_eq!(state.workflow_sender_id(), "team-lead@dockyard");

        state.set_current_actor(MessageActor {
            kind: MessageActorKind::Subagent,
            id: "agent-1".to_string(),
            agent_id: Some("agent-1".to_string()),
            agent_type: Some("reviewer".to_string()),
            name: Some("Reviewer".to_string()),
            team_name: Some("dockyard".to_string()),
            session_id: Some(state.session.id),
            parent_session_id: Some(Uuid::new_v4()),
        });
        assert_eq!(state.workflow_sender_id(), "agent-1");
    }

    #[test]
    fn session_restore_recovers_last_agent_actor() {
        let actor = MessageActor {
            kind: MessageActorKind::Subagent,
            id: "agent-1".to_string(),
            agent_id: Some("agent-1".to_string()),
            agent_type: Some("reviewer".to_string()),
            name: None,
            team_name: None,
            session_id: None,
            parent_session_id: None,
        };
        let record = SessionRecord {
            metadata: sample_metadata(),
            events: vec![TranscriptEvent::AssistantMessage {
                text: "done".to_string(),
                actor: Some(actor.clone()),
            }],
        };
        let state = AppState::from_session_record(PufferConfig::default(), record);
        assert_eq!(state.current_actor, Some(actor));
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
