use crate::approval_overlay::ApprovalOverlay;
use crate::btw_overlay::BtwOverlay;
use crate::popup::popup_rows;
use crate::session_overlay::SessionOverlay;
use crate::status_overlay::StatusOverlay;
use crate::text_overlay::TextOverlay;
use crate::usage::UsageOverlay;
use puffer_core::{
    CommandSpec, PermissionPromptAction, PermissionPromptRequest, ToolCallRequest, ToolInvocation,
    TurnExecution, TurnUsageReport,
};
use puffer_provider_registry::{AuthStore, ExternalImportCandidate};
use puffer_session_store::SessionSummary;
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

/// Distinguishes the three loop command flavors.
#[derive(Debug, Clone)]
pub(crate) enum LoopKind {
    /// Pure interval-based repeat: re-run the prompt every N seconds.
    Loop,
    /// Iteratively maximize a named metric.
    Maximize(String),
    /// Iteratively minimize a named metric.
    Minimize(String),
}

/// Tracks whether a loop is actively running, waiting, or finished.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum LoopStatus {
    Running,
    WaitingInterval,
    Paused,
    Completed(String),
}

/// Persistent state for an active `/loop`, `/maximize`, or `/minimize` session.
#[derive(Debug, Clone)]
pub(crate) struct LoopState {
    pub(crate) kind: LoopKind,
    pub(crate) prompt: String,
    pub(crate) iteration: usize,
    pub(crate) max_iterations: usize,
    /// Delay between iterations for `/loop`; None for optimize commands.
    pub(crate) interval: Option<Duration>,
    /// When the next `/loop` iteration should fire.
    pub(crate) next_fire: Option<Instant>,
    /// Recorded metric values for `/maximize` and `/minimize`.
    pub(crate) target_history: Vec<f64>,
    pub(crate) status: LoopStatus,
}

/// Stores editable TUI input state plus the active overlay and deferred prompt.
pub(crate) struct TuiState {
    pub(crate) input: String,
    pub(crate) cursor: usize,
    pub(crate) slash_selection: usize,
    pub(crate) scroll_offset: u16,
    pub(crate) follow_output: bool,
    pub(crate) tool_details_expanded: bool,
    pub(crate) overlay: Option<OverlayState>,
    pub(crate) pending_permission_request: Option<PendingPermissionRequest>,
    pub(crate) deferred_prompt: Option<String>,
    pub(crate) pending_submit: Option<PendingSubmit>,
    pub(crate) queued_prompts: VecDeque<String>,
    pub(crate) active_loop: Option<LoopState>,
    /// Timestamp of last Ctrl+C press. Second press within 2s exits.
    pub(crate) last_ctrl_c: Option<std::time::Instant>,
    /// Transient hint shown in the status bar (auto-clears after 2s).
    pub(crate) status_hint: Option<(String, std::time::Instant)>,
}

/// Carries one completed background provider turn back to the UI thread.
pub(crate) struct PendingSubmitResult {
    pub(crate) outcome: std::result::Result<TurnExecution, String>,
    pub(crate) auth_store: AuthStore,
    /// Session-level tool permissions accumulated on the worker clone.
    pub(crate) session_tool_permissions: std::collections::HashMap<String, String>,
    /// Whether the user chose "allow all" during this turn.
    pub(crate) session_allow_all: bool,
}

/// Carries one event emitted while a provider-backed turn is in flight.
pub(crate) enum PendingSubmitEvent {
    ThinkingDelta(String),
    TextDelta(String),
    ToolCallsRequested(Vec<ToolCallRequest>),
    ToolInvocations(Vec<ToolInvocation>),
    ReflectionCheckpoint(String),
    RetryAttempt {
        attempt: usize,
        max_attempts: usize,
        error: String,
    },
    Usage(TurnUsageReport),
    PermissionRequest(PermissionPromptRequest, Sender<PermissionPromptAction>),
    Finished(PendingSubmitResult),
}

/// Stores one in-flight provider prompt and the receiver for its completion event.
pub(crate) struct PendingSubmit {
    pub(crate) prompt: String,
    pub(crate) receiver: Receiver<PendingSubmitEvent>,
    pub(crate) pending_tool_calls: Vec<ToolCallRequest>,
    pub(crate) rendered_tool_invocations: usize,
    pub(crate) started_at: std::time::Instant,
    /// Set to true when the model is actively producing thinking/reasoning tokens.
    pub(crate) thinking_active: bool,
    /// Transient status message (e.g. "Retrying (2/3)...").
    pub(crate) status_hint: Option<String>,
    /// Cancel handle wired into the agent loop. Tripping it makes the
    /// worker thread exit at the next turn boundary instead of running
    /// to completion. Without this, ESC just drops the receiver and the
    /// worker keeps running (burning tokens) until the LLM and tool
    /// calls finish on their own.
    pub(crate) cancel: puffer_core::CancelToken,
}

/// Stores the response channel for the currently visible permission prompt.
pub(crate) struct PendingPermissionRequest {
    pub(crate) response_tx: Sender<PermissionPromptAction>,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            slash_selection: 0,
            scroll_offset: 0,
            follow_output: true,
            tool_details_expanded: false,
            overlay: None,
            pending_permission_request: None,
            deferred_prompt: None,
            pending_submit: None,
            queued_prompts: VecDeque::new(),
            active_loop: None,
            last_ctrl_c: None,
            status_hint: None,
        }
    }
}

impl TuiState {
    /// Returns true if this Ctrl+C should exit (second press within 2s).
    /// On first press, records the timestamp and returns false.
    pub(crate) fn should_exit_on_ctrl_c(&mut self) -> bool {
        let now = std::time::Instant::now();
        if let Some(last) = self.last_ctrl_c {
            if now.duration_since(last).as_secs() < 2 {
                return true; // Second press within 2s → exit
            }
        }
        self.last_ctrl_c = Some(now);
        false
    }

    /// Returns true when a provider-backed prompt is still running.
    pub(crate) fn has_pending_submit(&self) -> bool {
        self.pending_submit.is_some()
    }

    /// Adds one prompt to the back of the staged prompt queue.
    pub(crate) fn enqueue_prompt(&mut self, prompt: String) {
        self.queued_prompts.push_back(prompt);
    }

    /// Pops the next staged prompt in FIFO order.
    pub(crate) fn dequeue_prompt(&mut self) -> Option<String> {
        self.queued_prompts.pop_front()
    }
}

impl TuiState {
    /// Clears the inline prompt and resets slash completion state.
    pub(crate) fn clear(&mut self, commands: &[CommandSpec]) {
        self.input.clear();
        self.cursor = 0;
        self.slash_selection = 0;
        self.sync(commands);
    }

    /// Moves the input cursor left by one character boundary.
    pub(crate) fn move_left(&mut self) {
        self.cursor = previous_boundary(&self.input, self.cursor);
    }

    /// Moves the input cursor right by one character boundary.
    pub(crate) fn move_right(&mut self) {
        self.cursor = next_boundary(&self.input, self.cursor);
    }

    /// Moves the input cursor to the start of the prompt.
    pub(crate) fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Moves the input cursor to the end of the prompt.
    pub(crate) fn move_end(&mut self) {
        self.cursor = self.input.len();
    }

    /// Inserts one character into the prompt and refreshes slash suggestions.
    pub(crate) fn insert_char(&mut self, ch: char, commands: &[CommandSpec]) {
        self.input.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.sync(commands);
    }

    /// Removes the character immediately before the cursor.
    pub(crate) fn backspace(&mut self, commands: &[CommandSpec]) {
        if self.cursor == 0 {
            return;
        }
        let start = previous_boundary(&self.input, self.cursor);
        self.input.drain(start..self.cursor);
        self.cursor = start;
        self.sync(commands);
    }

    /// Removes the character immediately after the cursor.
    pub(crate) fn delete(&mut self, commands: &[CommandSpec]) {
        if self.cursor >= self.input.len() {
            return;
        }
        let end = next_boundary(&self.input, self.cursor);
        self.input.drain(self.cursor..end);
        self.sync(commands);
    }

    /// Selects the previous slash-command suggestion when present.
    pub(crate) fn select_previous(&mut self, commands: &[CommandSpec]) {
        let count = self.matching_rows(commands).len();
        if count == 0 {
            return;
        }
        self.slash_selection = self.slash_selection.saturating_sub(1);
    }

    /// Selects the next slash-command suggestion when present.
    pub(crate) fn select_next(&mut self, commands: &[CommandSpec]) {
        let count = self.matching_rows(commands).len();
        if count == 0 {
            return;
        }
        self.slash_selection = (self.slash_selection + 1).min(count.saturating_sub(1));
    }

    /// Applies the currently highlighted slash-command suggestion to the prompt.
    pub(crate) fn apply_selected_command(&mut self, commands: &[CommandSpec]) -> bool {
        let rows = self.matching_rows(commands);
        let Some(command) = rows.get(self.slash_selection).copied() else {
            return false;
        };
        self.input = format!("/{}", command.name);
        if command.argument_hint.is_some() {
            self.input.push(' ');
        }
        self.cursor = self.input.len();
        self.sync(commands);
        true
    }

    /// Completes a partial slash command when Enter should pick the highlighted result.
    pub(crate) fn complete_on_enter(&mut self, commands: &[CommandSpec]) -> bool {
        if !self.input.starts_with('/') || self.input.contains(' ') {
            return false;
        }
        let trimmed = self.input.trim_start_matches('/');
        if trimmed.is_empty() {
            return false;
        }
        let rows = self.matching_rows(commands);
        let Some(command) = rows.get(self.slash_selection).copied() else {
            return false;
        };
        if command.name == trimmed || command.aliases.iter().any(|alias| alias == trimmed) {
            return false;
        }
        self.apply_selected_command(commands)
    }

    /// Takes the current input and resets the prompt buffer.
    pub(crate) fn take_input(&mut self) -> String {
        self.cursor = 0;
        self.slash_selection = 0;
        std::mem::take(&mut self.input)
    }

    /// Stores a prompt to submit after onboarding finishes.
    pub(crate) fn defer_prompt(&mut self, prompt: Option<String>) {
        self.deferred_prompt = prompt;
    }

    /// Takes the queued prompt when one is pending.
    pub(crate) fn take_deferred_prompt(&mut self) -> Option<String> {
        self.deferred_prompt.take()
    }

    /// Scrolls the transcript upward by the requested number of lines.
    pub(crate) fn scroll_up(&mut self, amount: u16, line_count: u16, viewport_height: u16) {
        self.scroll_offset = self
            .effective_bottom_offset(line_count, viewport_height)
            .saturating_sub(amount);
        self.follow_output = false;
    }

    /// Scrolls the transcript downward by the requested number of lines.
    pub(crate) fn scroll_down(&mut self, amount: u16, line_count: u16, viewport_height: u16) {
        let max_offset = max_scroll_offset(line_count, viewport_height);
        self.scroll_offset = (self.scroll_offset + amount).min(max_offset);
        self.follow_output = self.scroll_offset >= max_offset;
    }

    fn effective_bottom_offset(&self, line_count: u16, viewport_height: u16) -> u16 {
        if self.follow_output {
            max_scroll_offset(line_count, viewport_height)
        } else {
            self.scroll_offset
        }
    }

    fn matching_rows<'a>(&self, commands: &'a [CommandSpec]) -> Vec<&'a CommandSpec> {
        if self.input.starts_with('/') {
            popup_rows(&self.input, commands)
        } else {
            Vec::new()
        }
    }

    fn sync(&mut self, commands: &[CommandSpec]) {
        self.cursor = self.cursor.min(self.input.len());
        let count = self.matching_rows(commands).len();
        if count == 0 {
            self.slash_selection = 0;
        } else {
            self.slash_selection = self.slash_selection.min(count - 1);
        }
    }
}

/// Represents one authentication action exposed during onboarding or `/login`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthPickerAction {
    OAuth,
    ApiKey,
    Import(ExternalImportCandidate),
    UseStored,
    NoneRequired,
}

/// Describes one option inside an auth-method picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthPickerEntry {
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) action: AuthPickerAction,
}

/// Describes one option inside a picker or onboarding step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelPickerEntry {
    pub(crate) selector: String,
    pub(crate) description: String,
    pub(crate) command: Option<String>,
}

/// Represents one interactive picker or onboarding panel inside the TUI.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayState {
    SessionPicker {
        sessions: Vec<SessionSummary>,
        selection: usize,
    },
    AgentPicker {
        entries: Vec<ModelPickerEntry>,
        selection: usize,
    },
    ModelPicker {
        provider_id: String,
        entries: Vec<ModelPickerEntry>,
        selection: usize,
        onboarding: bool,
    },
    EffortPicker {
        provider_id: String,
        model_id: String,
        entries: Vec<ModelPickerEntry>,
        selection: usize,
        onboarding: bool,
    },
    FastModePicker {
        provider_id: String,
        model_id: String,
        effort: String,
        entries: Vec<ModelPickerEntry>,
        selection: usize,
        onboarding: bool,
    },
    LoginPicker {
        entries: Vec<ModelPickerEntry>,
        selection: usize,
    },
    ProviderPicker {
        entries: Vec<ModelPickerEntry>,
        selection: usize,
        onboarding: bool,
    },
    AuthPicker {
        provider_id: String,
        entries: Vec<AuthPickerEntry>,
        selection: usize,
        onboarding: bool,
    },
    ApiKeyPrompt {
        provider_id: String,
        value: String,
        cursor: usize,
        onboarding: bool,
    },
    LogoutPicker {
        entries: Vec<ModelPickerEntry>,
        selection: usize,
    },
    ThemePicker {
        entries: Vec<ModelPickerEntry>,
        selection: usize,
    },
    CommandPicker {
        title: String,
        entries: Vec<ModelPickerEntry>,
        selection: usize,
    },
    Help,
    PermissionPrompt {
        overlay: ApprovalOverlay,
    },
    Btw(BtwOverlay),
    Session(SessionOverlay),
    Status(StatusOverlay),
    Text(TextOverlay),
    Usage(UsageOverlay),
    OnboardingTheme {
        entries: Vec<ModelPickerEntry>,
        selection: usize,
    },
    OnboardingProvider {
        entries: Vec<ModelPickerEntry>,
        selection: usize,
    },
    OnboardingAuth {
        provider_id: String,
        provider_name: String,
        entries: Vec<ModelPickerEntry>,
        selection: usize,
    },
    OnboardingModel {
        provider_id: String,
        provider_name: String,
        entries: Vec<ModelPickerEntry>,
        selection: usize,
    },
    OnboardingApiKey {
        provider_id: String,
        provider_name: String,
        input: String,
        cursor: usize,
    },
}

impl OverlayState {
    /// Moves the active selection upward for list-based overlays.
    pub(crate) fn select_previous(&mut self) {
        match self {
            Self::SessionPicker { selection, .. }
            | Self::AgentPicker { selection, .. }
            | Self::ModelPicker { selection, .. }
            | Self::EffortPicker { selection, .. }
            | Self::FastModePicker { selection, .. }
            | Self::LoginPicker { selection, .. }
            | Self::ProviderPicker { selection, .. }
            | Self::AuthPicker { selection, .. }
            | Self::LogoutPicker { selection, .. }
            | Self::ThemePicker { selection, .. }
            | Self::CommandPicker { selection, .. }
            | Self::OnboardingTheme { selection, .. }
            | Self::OnboardingProvider { selection, .. }
            | Self::OnboardingAuth { selection, .. }
            | Self::OnboardingModel { selection, .. } => {
                *selection = selection.saturating_sub(1);
            }
            Self::PermissionPrompt { overlay } => overlay.select_previous(),
            Self::Btw(overlay) => overlay.scroll_up(),
            Self::Session(overlay) => overlay.scroll_up(),
            Self::Status(overlay) => overlay.scroll_up(),
            Self::Text(overlay) => overlay.scroll_up(),
            Self::Help
            | Self::ApiKeyPrompt { .. }
            | Self::Usage(..)
            | Self::OnboardingApiKey { .. } => {}
        }
    }

    /// Moves the active selection downward for list-based overlays.
    pub(crate) fn select_next(&mut self) {
        match self {
            Self::SessionPicker {
                sessions,
                selection,
            } => {
                *selection = (*selection + 1).min(sessions.len().saturating_sub(1));
            }
            Self::AgentPicker { entries, selection }
            | Self::ModelPicker {
                entries, selection, ..
            }
            | Self::EffortPicker {
                entries, selection, ..
            }
            | Self::FastModePicker {
                entries, selection, ..
            }
            | Self::LoginPicker { entries, selection }
            | Self::ProviderPicker {
                entries, selection, ..
            }
            | Self::LogoutPicker { entries, selection }
            | Self::ThemePicker { entries, selection }
            | Self::CommandPicker {
                entries, selection, ..
            }
            | Self::OnboardingTheme { entries, selection }
            | Self::OnboardingProvider { entries, selection }
            | Self::OnboardingAuth {
                entries, selection, ..
            }
            | Self::OnboardingModel {
                entries, selection, ..
            } => {
                *selection = (*selection + 1).min(entries.len().saturating_sub(1));
            }
            Self::AuthPicker {
                entries, selection, ..
            } => {
                *selection = (*selection + 1).min(entries.len().saturating_sub(1));
            }
            Self::PermissionPrompt { overlay } => overlay.select_next(),
            Self::Btw(overlay) => overlay.scroll_down(),
            Self::Session(overlay) => overlay.scroll_down(),
            Self::Status(overlay) => overlay.scroll_down(),
            Self::Text(overlay) => overlay.scroll_down(),
            Self::Help
            | Self::ApiKeyPrompt { .. }
            | Self::Usage(..)
            | Self::OnboardingApiKey { .. } => {}
        }
    }

    /// Moves up by a larger page-sized jump.
    pub(crate) fn page_up(&mut self) {
        match self {
            Self::PermissionPrompt { overlay } => overlay.page_up(),
            Self::Btw(overlay) => overlay.page_up(),
            Self::Session(overlay) => overlay.page_up(),
            Self::Status(overlay) => overlay.page_up(),
            Self::Text(overlay) => overlay.page_up(),
            _ => {
                for _ in 0..10 {
                    self.select_previous();
                }
            }
        }
    }

    /// Moves down by a larger page-sized jump.
    pub(crate) fn page_down(&mut self) {
        match self {
            Self::PermissionPrompt { overlay } => overlay.page_down(),
            Self::Btw(overlay) => overlay.page_down(),
            Self::Session(overlay) => overlay.page_down(),
            Self::Status(overlay) => overlay.page_down(),
            Self::Text(overlay) => overlay.page_down(),
            _ => {
                for _ in 0..10 {
                    self.select_next();
                }
            }
        }
    }

    /// Returns `true` when this overlay is a scrollable text panel (not a picker).
    pub(crate) fn is_text_overlay(&self) -> bool {
        matches!(
            self,
            Self::Text(..)
                | Self::Btw(..)
                | Self::Session(..)
                | Self::Status(..)
                | Self::PermissionPrompt { .. }
        )
    }

    /// Scrolls upward by half a page (Vim Ctrl+u).
    pub(crate) fn half_page_up(&mut self) {
        match self {
            Self::Text(overlay) => overlay.half_page_up(),
            _ => self.page_up(),
        }
    }

    /// Scrolls downward by half a page (Vim Ctrl+d).
    pub(crate) fn half_page_down(&mut self) {
        match self {
            Self::Text(overlay) => overlay.half_page_down(),
            _ => self.page_down(),
        }
    }

    /// Jumps to the top (Vim gg).
    pub(crate) fn scroll_to_top(&mut self) {
        match self {
            Self::Text(overlay) => overlay.scroll_to_top(),
            _ => {}
        }
    }

    /// Jumps to the bottom (Vim G).
    pub(crate) fn scroll_to_bottom(&mut self) {
        match self {
            Self::Text(overlay) => overlay.scroll_to_bottom(),
            _ => {}
        }
    }

    /// Returns the direct slash-command submission for simple overlays.
    pub(crate) fn selected_command(&self) -> Option<String> {
        match self {
            Self::SessionPicker {
                sessions,
                selection,
            } => sessions
                .get(*selection)
                .map(|session| format!("/resume {}", session.id)),
            Self::AgentPicker { entries, selection } => entries
                .get(*selection)
                .map(|entry| format!("/agents use {}", entry.selector)),
            Self::ModelPicker {
                entries,
                selection,
                onboarding: false,
                ..
            } => entries
                .get(*selection)
                .map(|entry| format!("/model {}", entry.selector)),
            Self::ModelPicker {
                onboarding: true, ..
            } => None,
            Self::EffortPicker { .. } | Self::FastModePicker { .. } => None,
            Self::LoginPicker { entries, selection } => entries
                .get(*selection)
                .map(|entry| format!("/login {}", entry.selector)),
            Self::LogoutPicker { entries, selection } => entries
                .get(*selection)
                .map(|entry| format!("/logout {}", entry.selector)),
            Self::ThemePicker { entries, selection } => entries
                .get(*selection)
                .map(|entry| format!("/theme {}", entry.selector)),
            Self::CommandPicker {
                entries, selection, ..
            } => entries.get(*selection).and_then(|entry| {
                let command = entry.command.as_deref().unwrap_or(&entry.selector).trim();
                if command.is_empty() {
                    None
                } else {
                    Some(command.to_string())
                }
            }),
            Self::ProviderPicker { .. }
            | Self::AuthPicker { .. }
            | Self::ApiKeyPrompt { .. }
            | Self::Help
            | Self::PermissionPrompt { .. }
            | Self::Btw(..)
            | Self::Session(..)
            | Self::Status(..)
            | Self::Text(..)
            | Self::Usage(..)
            | Self::OnboardingTheme { .. }
            | Self::OnboardingProvider { .. }
            | Self::OnboardingAuth { .. }
            | Self::OnboardingModel { .. }
            | Self::OnboardingApiKey { .. } => None,
        }
    }

    /// Returns the currently selected provider id when present.
    pub(crate) fn selected_provider(&self) -> Option<&str> {
        match self {
            Self::LoginPicker { entries, selection }
            | Self::ProviderPicker {
                entries, selection, ..
            } => entries.get(*selection).map(|entry| entry.selector.as_str()),
            Self::ModelPicker { provider_id, .. }
            | Self::EffortPicker { provider_id, .. }
            | Self::FastModePicker { provider_id, .. }
            | Self::AuthPicker { provider_id, .. }
            | Self::ApiKeyPrompt { provider_id, .. }
            | Self::OnboardingAuth { provider_id, .. }
            | Self::OnboardingModel { provider_id, .. }
            | Self::OnboardingApiKey { provider_id, .. } => Some(provider_id.as_str()),
            Self::OnboardingProvider { entries, selection } => {
                entries.get(*selection).map(|entry| entry.selector.as_str())
            }
            Self::SessionPicker { .. }
            | Self::AgentPicker { .. }
            | Self::LogoutPicker { .. }
            | Self::ThemePicker { .. }
            | Self::CommandPicker { .. }
            | Self::Help
            | Self::PermissionPrompt { .. }
            | Self::Btw(..)
            | Self::Session(..)
            | Self::Status(..)
            | Self::Text(..)
            | Self::Usage(..)
            | Self::OnboardingTheme { .. } => None,
        }
    }

    /// Returns the currently selected model id when the overlay is model-backed.
    pub(crate) fn selected_model(&self) -> Option<&str> {
        match self {
            Self::ModelPicker {
                entries, selection, ..
            }
            | Self::EffortPicker {
                entries, selection, ..
            }
            | Self::FastModePicker {
                entries, selection, ..
            }
            | Self::OnboardingModel {
                entries, selection, ..
            } => entries.get(*selection).map(|entry| entry.selector.as_str()),
            _ => None,
        }
    }

    /// Returns the currently selected authentication action when present.
    pub(crate) fn selected_auth_action(&self) -> Option<&AuthPickerAction> {
        match self {
            Self::AuthPicker {
                entries, selection, ..
            } => entries.get(*selection).map(|entry| &entry.action),
            _ => None,
        }
    }

    /// Moves the selection to the first entry that matches the typed query.
    pub(crate) fn select_matching_query(&mut self, query: &str) {
        let query = query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return;
        }
        match self {
            Self::SessionPicker {
                sessions,
                selection,
            } => {
                if let Some(index) = sessions.iter().position(|session| {
                    session.id.to_string().to_ascii_lowercase().contains(&query)
                        || session
                            .display_name
                            .as_deref()
                            .map(|name| name.to_ascii_lowercase().contains(&query))
                            .unwrap_or(false)
                        || session
                            .slug
                            .as_deref()
                            .map(|slug| slug.to_ascii_lowercase().contains(&query))
                            .unwrap_or(false)
                        || session
                            .note
                            .as_deref()
                            .map(|note| note.to_ascii_lowercase().contains(&query))
                            .unwrap_or(false)
                        || session
                            .tags
                            .iter()
                            .any(|tag| tag.to_ascii_lowercase().contains(&query))
                        || session
                            .cwd
                            .display()
                            .to_string()
                            .to_ascii_lowercase()
                            .contains(&query)
                }) {
                    *selection = index;
                }
            }
            Self::AgentPicker { entries, selection }
            | Self::ModelPicker {
                entries, selection, ..
            }
            | Self::EffortPicker {
                entries, selection, ..
            }
            | Self::FastModePicker {
                entries, selection, ..
            }
            | Self::LoginPicker { entries, selection }
            | Self::ProviderPicker {
                entries, selection, ..
            }
            | Self::LogoutPicker { entries, selection }
            | Self::ThemePicker { entries, selection }
            | Self::CommandPicker {
                entries, selection, ..
            }
            | Self::OnboardingTheme { entries, selection }
            | Self::OnboardingProvider { entries, selection }
            | Self::OnboardingAuth {
                entries, selection, ..
            }
            | Self::OnboardingModel {
                entries, selection, ..
            } => {
                if let Some(index) = entries.iter().position(|entry| {
                    entry.selector.to_ascii_lowercase().contains(&query)
                        || entry.description.to_ascii_lowercase().contains(&query)
                }) {
                    *selection = index;
                }
            }
            Self::PermissionPrompt { .. }
            | Self::Help
            | Self::Btw(..)
            | Self::Session(..)
            | Self::Status(..)
            | Self::Text(..) => {}
            Self::AuthPicker {
                entries, selection, ..
            } => {
                if let Some(index) = entries.iter().position(|entry| {
                    entry.label.to_ascii_lowercase().contains(&query)
                        || entry.description.to_ascii_lowercase().contains(&query)
                }) {
                    *selection = index;
                }
            }
            Self::ApiKeyPrompt { .. } | Self::Usage(..) | Self::OnboardingApiKey { .. } => {}
        }
    }

    /// Returns the currently selected onboarding auth selector when present.
    #[allow(dead_code)]
    pub(crate) fn selected_auth_selector(&self) -> Option<&str> {
        match self {
            Self::OnboardingAuth {
                entries, selection, ..
            } => entries.get(*selection).map(|entry| entry.selector.as_str()),
            _ => None,
        }
    }

    /// Returns true when the overlay is part of the onboarding flow.
    pub(crate) fn is_onboarding(&self) -> bool {
        matches!(
            self,
            Self::ProviderPicker {
                onboarding: true,
                ..
            } | Self::AuthPicker {
                onboarding: true,
                ..
            } | Self::ModelPicker {
                onboarding: true,
                ..
            } | Self::EffortPicker {
                onboarding: true,
                ..
            } | Self::FastModePicker {
                onboarding: true,
                ..
            } | Self::ApiKeyPrompt {
                onboarding: true,
                ..
            } | Self::ThemePicker { .. }
                | Self::OnboardingTheme { .. }
                | Self::OnboardingProvider { .. }
                | Self::OnboardingAuth { .. }
                | Self::OnboardingModel { .. }
                | Self::OnboardingApiKey { .. }
        )
    }

    /// Returns true when the overlay accepts inline text input.
    pub(crate) fn accepts_text_input(&self) -> bool {
        matches!(
            self,
            Self::ApiKeyPrompt { .. } | Self::OnboardingApiKey { .. }
        )
    }

    /// Returns the selected permission action when the overlay is a permission prompt.
    pub(crate) fn selected_permission_action(&self) -> Option<PermissionPromptAction> {
        match self {
            Self::PermissionPrompt { overlay } => Some(overlay.selected_action()),
            _ => None,
        }
    }

    /// Returns the direct shortcut action for a permission prompt, if any.
    pub(crate) fn permission_shortcut_action(
        &mut self,
        ch: char,
    ) -> Option<PermissionPromptAction> {
        match self {
            Self::PermissionPrompt { overlay } => overlay.activate_shortcut(ch),
            _ => None,
        }
    }

    /// Moves the text cursor left for inline API-key input.
    pub(crate) fn move_left(&mut self) {
        match self {
            Self::ApiKeyPrompt { value, cursor, .. }
            | Self::OnboardingApiKey {
                input: value,
                cursor,
                ..
            } => {
                *cursor = previous_boundary(value, *cursor);
            }
            _ => {}
        }
    }

    /// Moves the text cursor right for inline API-key input.
    pub(crate) fn move_right(&mut self) {
        match self {
            Self::ApiKeyPrompt { value, cursor, .. }
            | Self::OnboardingApiKey {
                input: value,
                cursor,
                ..
            } => {
                *cursor = next_boundary(value, *cursor);
            }
            _ => {}
        }
    }

    /// Moves the text cursor to the beginning for inline API-key input.
    pub(crate) fn move_home(&mut self) {
        match self {
            Self::ApiKeyPrompt { cursor, .. } | Self::OnboardingApiKey { cursor, .. } => {
                *cursor = 0;
            }
            _ => {}
        }
    }

    /// Moves the text cursor to the end for inline API-key input.
    pub(crate) fn move_end(&mut self) {
        match self {
            Self::ApiKeyPrompt { value, cursor, .. }
            | Self::OnboardingApiKey {
                input: value,
                cursor,
                ..
            } => {
                *cursor = value.len();
            }
            _ => {}
        }
    }

    /// Inserts one character into the API-key prompt.
    pub(crate) fn insert_char(&mut self, ch: char) {
        match self {
            Self::ApiKeyPrompt { value, cursor, .. }
            | Self::OnboardingApiKey {
                input: value,
                cursor,
                ..
            } => {
                value.insert(*cursor, ch);
                *cursor += ch.len_utf8();
            }
            _ => {}
        }
    }

    /// Deletes the previous character from the API-key prompt.
    pub(crate) fn backspace(&mut self) {
        match self {
            Self::ApiKeyPrompt { value, cursor, .. }
            | Self::OnboardingApiKey {
                input: value,
                cursor,
                ..
            } => {
                if *cursor == 0 {
                    return;
                }
                let start = previous_boundary(value, *cursor);
                value.drain(start..*cursor);
                *cursor = start;
            }
            _ => {}
        }
    }

    /// Deletes the next character from the API-key prompt.
    pub(crate) fn delete(&mut self) {
        match self {
            Self::ApiKeyPrompt { value, cursor, .. }
            | Self::OnboardingApiKey {
                input: value,
                cursor,
                ..
            } => {
                if *cursor >= value.len() {
                    return;
                }
                let end = next_boundary(value, *cursor);
                value.drain(*cursor..end);
            }
            _ => {}
        }
    }

    /// Returns the current API-key prompt value when present.
    pub(crate) fn api_key_value(&self) -> Option<&str> {
        match self {
            Self::ApiKeyPrompt { value, .. } => Some(value.as_str()),
            Self::OnboardingApiKey { input, .. } => Some(input.as_str()),
            _ => None,
        }
    }
}

fn previous_boundary(input: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut index = cursor - 1;
    while index > 0 && !input.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn next_boundary(input: &str, cursor: usize) -> usize {
    if cursor >= input.len() {
        return input.len();
    }
    let mut index = cursor + 1;
    while index < input.len() && !input.is_char_boundary(index) {
        index += 1;
    }
    index.min(input.len())
}

fn max_scroll_offset(line_count: u16, viewport_height: u16) -> u16 {
    line_count.saturating_sub(viewport_height.max(1))
}

#[cfg(test)]
mod tests {
    use super::TuiState;

    #[test]
    fn scroll_up_detaches_from_follow_output() {
        let mut tui = TuiState::default();
        tui.scroll_up(1, 20, 5);
        assert!(!tui.follow_output);
        assert_eq!(tui.scroll_offset, 14);
    }

    #[test]
    fn scroll_down_reattaches_when_reaching_bottom() {
        let mut tui = TuiState::default();
        tui.follow_output = false;
        tui.scroll_offset = 14;
        tui.scroll_down(1, 20, 5);
        assert!(tui.follow_output);
        assert_eq!(tui.scroll_offset, 15);
    }
}
