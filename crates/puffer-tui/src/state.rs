use crate::approval_overlay::ApprovalOverlay;
use crate::btw_overlay::BtwOverlay;
use crate::popup::popup_rows;
use crate::session_overlay::SessionOverlay;
use crate::status_overlay::StatusOverlay;
use crate::text_overlay::TextOverlay;
use crate::usage::UsageOverlay;
use crate::user_question_overlay::UserQuestionOverlay;
use puffer_core::{
    CommandSpec, PermissionPromptAction, PermissionPromptRequest, SessionPermissionState,
    ToolCallRequest, ToolInvocation, TurnExecution, TurnUsageReport, UserQuestionPromptRequest,
    UserQuestionPromptResponse,
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
    pub(crate) pending_user_question_request: Option<PendingUserQuestionRequest>,
    pub(crate) deferred_prompt: Option<String>,
    pub(crate) pending_submit: Option<PendingSubmit>,
    pub(crate) queued_prompts: VecDeque<String>,
    pub(crate) active_loop: Option<LoopState>,
    /// Timestamp of last Ctrl+C press. Second press within 2s exits.
    pub(crate) last_ctrl_c: Option<std::time::Instant>,
    /// Transient hint shown in the status bar (auto-clears after 2s).
    pub(crate) status_hint: Option<(String, std::time::Instant)>,
    /// Pasted-text placeholders displayed in `input`, paired with their full
    /// content. Expanded back into the prompt when the user submits.
    pub(crate) pending_pastes: Vec<(String, String)>,
    /// Counter used to generate `[Pasted text #N ...]` placeholder ids.
    pub(crate) next_paste_id: usize,
    /// Recently submitted or cleared prompts, oldest first. Recalled with
    /// the Up/Down arrow keys when the input is empty or currently being
    /// navigated.
    pub(crate) history: VecDeque<String>,
    /// Index into `history` while the user is navigating it. `None` means
    /// the user is editing fresh input and not browsing history.
    pub(crate) history_cursor: Option<usize>,
    /// Snapshot of the user's in-progress input at the moment they started
    /// navigating history, restored when they move past the newest entry.
    pub(crate) history_draft: Option<String>,
    /// Timestamp of the most recent user keystroke. Auto-recap fires when
    /// `now - last_user_input_at > config.recap.idle_secs` and the rest of
    /// `puffer_core::recap::should_auto_trigger` passes.
    pub(crate) last_user_input_at: std::time::Instant,
    /// Count of user messages observed at the time the last recap (manual or
    /// auto) was triggered. Feeds the cooldown check.
    pub(crate) last_recap_user_msg_count: Option<usize>,
}

/// Carries one completed background provider turn back to the UI thread.
pub(crate) struct PendingSubmitResult {
    pub(crate) outcome: std::result::Result<TurnExecution, String>,
    pub(crate) auth_store: AuthStore,
    /// Session-level permission state accumulated on the worker clone.
    pub(crate) session_permission_state: SessionPermissionState,
    /// Whether the user chose "allow all" during this turn.
    pub(crate) session_allow_all: bool,
    /// Project-memory review turns accumulated on the worker clone.
    pub(crate) project_memory_review_turns: usize,
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
    UserQuestionRequest(
        UserQuestionPromptRequest,
        Sender<UserQuestionPromptResponse>,
    ),
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

/// Stores the response channel for the currently visible user question prompt.
pub(crate) struct PendingUserQuestionRequest {
    pub(crate) response_tx: Sender<UserQuestionPromptResponse>,
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
            pending_user_question_request: None,
            deferred_prompt: None,
            pending_submit: None,
            queued_prompts: VecDeque::new(),
            active_loop: None,
            last_ctrl_c: None,
            status_hint: None,
            pending_pastes: Vec::new(),
            next_paste_id: 1,
            history: VecDeque::new(),
            history_cursor: None,
            history_draft: None,
            last_user_input_at: std::time::Instant::now(),
            last_recap_user_msg_count: None,
        }
    }
}

/// Threshold above which a paste is replaced with a `[Pasted text #N ...]`
/// placeholder instead of being inserted directly. Multi-line pastes are
/// always replaced regardless of length.
pub(crate) const PASTE_PLACEHOLDER_CHAR_THRESHOLD: usize = 200;

/// Maximum number of prompts retained for Up/Down recall.
pub(crate) const MAX_INPUT_HISTORY: usize = 200;

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
        self.pending_pastes.clear();
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
        self.exit_history_navigation();
        self.input.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.sync(commands);
    }

    /// Inserts a literal newline at the cursor (Shift+Enter / Alt+Enter / Ctrl+J).
    pub(crate) fn insert_newline(&mut self, commands: &[CommandSpec]) {
        self.insert_char('\n', commands);
    }

    /// Inserts a verbatim string (typically from a non-placeholder paste).
    pub(crate) fn insert_str(&mut self, text: &str, commands: &[CommandSpec]) {
        self.exit_history_navigation();
        self.input.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.sync(commands);
    }

    /// Handles a bracketed paste: inserts a placeholder for multi-line or
    /// long pastes and stashes the original for expansion at submit time.
    /// Short single-line pastes are inserted verbatim.
    pub(crate) fn handle_paste(&mut self, raw: &str, commands: &[CommandSpec]) {
        // Many terminals deliver paste newlines as `\r` or `\r\n`; normalize.
        let pasted = raw.replace("\r\n", "\n").replace('\r', "\n");
        let multiline = pasted.contains('\n');
        let char_count = pasted.chars().count();
        if !multiline && char_count <= PASTE_PLACEHOLDER_CHAR_THRESHOLD {
            self.insert_str(&pasted, commands);
            return;
        }
        let line_count = pasted.matches('\n').count() + 1;
        let placeholder = self.allocate_paste_placeholder(line_count);
        self.pending_pastes
            .push((placeholder.clone(), pasted.clone()));
        self.insert_str(&placeholder, commands);
    }

    fn allocate_paste_placeholder(&mut self, line_count: usize) -> String {
        let id = self.next_paste_id;
        self.next_paste_id = self.next_paste_id.saturating_add(1);
        if line_count > 1 {
            format!("[Pasted text #{id} +{line_count} lines]")
        } else {
            format!("[Pasted text #{id}]")
        }
    }

    /// Replaces every `[Pasted text #N ...]` placeholder in `text` with its
    /// stored content, returning the expanded prompt.
    pub(crate) fn expand_paste_placeholders(&self, text: &str) -> String {
        if self.pending_pastes.is_empty() || text.is_empty() {
            return text.to_string();
        }
        let mut expanded = text.to_string();
        for (placeholder, content) in &self.pending_pastes {
            if expanded.contains(placeholder) {
                expanded = expanded.replace(placeholder, content);
            }
        }
        expanded
    }

    /// Removes the character immediately before the cursor. If the cursor is
    /// at the end of a `[Pasted text #N ...]` placeholder, removes the entire
    /// placeholder and drops its stored content.
    pub(crate) fn backspace(&mut self, commands: &[CommandSpec]) {
        if self.cursor == 0 {
            return;
        }
        self.exit_history_navigation();
        if let Some((start, placeholder)) = self.paste_placeholder_ending_at(self.cursor) {
            self.input.drain(start..self.cursor);
            self.cursor = start;
            self.pending_pastes.retain(|(name, _)| *name != placeholder);
            self.sync(commands);
            return;
        }
        let start = previous_boundary(&self.input, self.cursor);
        self.input.drain(start..self.cursor);
        self.cursor = start;
        self.sync(commands);
    }

    /// Returns `(start_byte, placeholder)` when `end_byte` is the closing
    /// boundary of a tracked paste placeholder.
    fn paste_placeholder_ending_at(&self, end: usize) -> Option<(usize, String)> {
        let prefix = self.input.get(..end)?;
        for (placeholder, _) in &self.pending_pastes {
            if prefix.ends_with(placeholder) {
                let start = end - placeholder.len();
                return Some((start, placeholder.clone()));
            }
        }
        None
    }

    /// Removes the character immediately after the cursor.
    pub(crate) fn delete(&mut self, commands: &[CommandSpec]) {
        if self.cursor >= self.input.len() {
            return;
        }
        self.exit_history_navigation();
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

    /// Takes the current input, expanding any `[Pasted text #N]` placeholders
    /// back to their full content, and resets the prompt buffer. The expanded
    /// prompt is also recorded in the recall history so the user can press
    /// Up to bring it back later.
    pub(crate) fn take_input(&mut self) -> String {
        self.cursor = 0;
        self.slash_selection = 0;
        self.exit_history_navigation();
        let raw = std::mem::take(&mut self.input);
        let expanded = self.expand_paste_placeholders(&raw);
        self.pending_pastes.clear();
        self.push_history(&expanded);
        expanded
    }

    /// Clears the current input and stores it in history so the user can
    /// press Up to recall it. Returns `true` when something was cleared, so
    /// callers (e.g., the Ctrl+C handler) can suppress the secondary
    /// "press again to exit" behavior on this keystroke.
    pub(crate) fn clear_input_to_history(&mut self, commands: &[CommandSpec]) -> bool {
        if self.input.is_empty() && self.pending_pastes.is_empty() {
            return false;
        }
        let raw = std::mem::take(&mut self.input);
        let expanded = self.expand_paste_placeholders(&raw);
        self.pending_pastes.clear();
        self.cursor = 0;
        self.slash_selection = 0;
        self.exit_history_navigation();
        self.push_history(&expanded);
        self.sync(commands);
        true
    }

    /// Pushes one entry onto the recall history, deduplicating against the
    /// most recent entry and trimming the queue to [`MAX_INPUT_HISTORY`].
    pub(crate) fn push_history(&mut self, entry: &str) {
        if entry.trim().is_empty() {
            return;
        }
        if self.history.back().map(String::as_str) == Some(entry) {
            return;
        }
        self.history.push_back(entry.to_string());
        while self.history.len() > MAX_INPUT_HISTORY {
            self.history.pop_front();
        }
    }

    /// Returns true when the user is currently navigating recall history.
    pub(crate) fn is_navigating_history(&self) -> bool {
        self.history_cursor.is_some()
    }

    /// Loads the previous (older) history entry into the input. Returns
    /// `false` when there is nothing to recall.
    pub(crate) fn history_recall_prev(&mut self, commands: &[CommandSpec]) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let next_index = match self.history_cursor {
            None => {
                self.history_draft = Some(self.input.clone());
                self.history.len() - 1
            }
            Some(0) => return true,
            Some(idx) => idx - 1,
        };
        self.history_cursor = Some(next_index);
        self.input = self.history[next_index].clone();
        self.cursor = self.input.len();
        self.pending_pastes.clear();
        self.sync(commands);
        true
    }

    /// Loads the next (newer) history entry. Past the newest entry, the
    /// pre-history draft is restored and history navigation ends. Returns
    /// `false` when not currently navigating.
    pub(crate) fn history_recall_next(&mut self, commands: &[CommandSpec]) -> bool {
        let Some(idx) = self.history_cursor else {
            return false;
        };
        if idx + 1 >= self.history.len() {
            self.input = self.history_draft.take().unwrap_or_default();
            self.cursor = self.input.len();
            self.history_cursor = None;
            self.pending_pastes.clear();
            self.sync(commands);
            return true;
        }
        let next = idx + 1;
        self.history_cursor = Some(next);
        self.input = self.history[next].clone();
        self.cursor = self.input.len();
        self.pending_pastes.clear();
        self.sync(commands);
        true
    }

    fn exit_history_navigation(&mut self) {
        self.history_cursor = None;
        self.history_draft = None;
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
    UserQuestionPrompt {
        overlay: UserQuestionOverlay,
    },
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
            Self::UserQuestionPrompt { overlay } => overlay.select_previous(),
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
            Self::UserQuestionPrompt { overlay } => overlay.select_next(),
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
            Self::UserQuestionPrompt { overlay } => overlay.page_up(),
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
            Self::UserQuestionPrompt { overlay } => overlay.page_down(),
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
                | Self::UserQuestionPrompt { .. }
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
            | Self::UserQuestionPrompt { .. }
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
            | Self::UserQuestionPrompt { .. }
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
            Self::UserQuestionPrompt { .. } => {}
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

    #[test]
    fn handle_paste_replaces_multiline_with_placeholder() {
        let mut tui = TuiState::default();
        tui.handle_paste("line one\nline two\nline three", &[]);
        assert_eq!(tui.input, "[Pasted text #1 +3 lines]");
        assert_eq!(tui.pending_pastes.len(), 1);
        let submitted = tui.take_input();
        assert_eq!(submitted, "line one\nline two\nline three");
        assert!(tui.input.is_empty());
        assert!(tui.pending_pastes.is_empty());
    }

    #[test]
    fn handle_paste_inserts_short_single_line_verbatim() {
        let mut tui = TuiState::default();
        tui.handle_paste("hello world", &[]);
        assert_eq!(tui.input, "hello world");
        assert!(tui.pending_pastes.is_empty());
    }

    #[test]
    fn handle_paste_normalizes_carriage_returns() {
        let mut tui = TuiState::default();
        tui.handle_paste("a\r\nb\rc", &[]);
        // \r\n -> \n, lone \r -> \n => "a\nb\nc" => 3 lines
        let placeholder = tui.input.clone();
        assert!(placeholder.starts_with("[Pasted text #1 +3 lines"));
        assert_eq!(tui.expand_paste_placeholders(&placeholder), "a\nb\nc");
    }

    #[test]
    fn backspace_at_end_of_placeholder_removes_whole_paste() {
        let mut tui = TuiState::default();
        tui.handle_paste("one\ntwo", &[]);
        let len = tui.input.len();
        tui.cursor = len;
        tui.backspace(&[]);
        assert!(tui.input.is_empty());
        assert!(tui.pending_pastes.is_empty());
    }

    #[test]
    fn insert_newline_grows_input() {
        let mut tui = TuiState::default();
        tui.insert_char('a', &[]);
        tui.insert_newline(&[]);
        tui.insert_char('b', &[]);
        assert_eq!(tui.input, "a\nb");
        assert_eq!(tui.cursor, 3);
    }

    #[test]
    fn clear_input_to_history_records_and_recalls() {
        let mut tui = TuiState::default();
        tui.insert_char('h', &[]);
        tui.insert_char('i', &[]);
        assert!(tui.clear_input_to_history(&[]));
        assert!(tui.input.is_empty());

        // First Up press loads the most recent entry.
        assert!(tui.history_recall_prev(&[]));
        assert_eq!(tui.input, "hi");
        assert_eq!(tui.cursor, 2);

        // Down past the end restores the empty draft.
        assert!(tui.history_recall_next(&[]));
        assert!(tui.input.is_empty());
        assert!(!tui.is_navigating_history());
    }

    #[test]
    fn take_input_records_submission_in_history() {
        let mut tui = TuiState::default();
        tui.insert_char('y', &[]);
        tui.insert_char('o', &[]);
        let _ = tui.take_input();
        assert_eq!(tui.history.back().map(String::as_str), Some("yo"));
    }

    #[test]
    fn history_dedupes_consecutive_duplicates() {
        let mut tui = TuiState::default();
        tui.push_history("same");
        tui.push_history("same");
        tui.push_history("same");
        assert_eq!(tui.history.len(), 1);
    }

    #[test]
    fn editing_input_exits_history_navigation() {
        let mut tui = TuiState::default();
        tui.push_history("alpha");
        tui.push_history("beta");
        assert!(tui.history_recall_prev(&[]));
        assert_eq!(tui.input, "beta");
        tui.insert_char('!', &[]);
        assert!(!tui.is_navigating_history());
        assert_eq!(tui.input, "beta!");
    }

    #[test]
    fn clear_input_skips_when_empty() {
        let mut tui = TuiState::default();
        assert!(!tui.clear_input_to_history(&[]));
        assert!(tui.history.is_empty());
    }
}
