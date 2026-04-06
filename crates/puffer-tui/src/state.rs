use crate::popup::popup_rows;
use anyhow::Result;
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_core::CommandSpec;
use puffer_provider_registry::ExternalImportCandidate;
use puffer_session_store::SessionSummary;
use serde::Deserialize;
use std::fs;
use std::path::Path;

/// Stores editable TUI input state plus the active overlay and deferred prompt.
#[derive(Debug, Clone, Default)]
pub(crate) struct TuiState {
    pub(crate) input: String,
    pub(crate) cursor: usize,
    pub(crate) slash_selection: usize,
    pub(crate) scroll_offset: u16,
    pub(crate) overlay: Option<OverlayState>,
    pub(crate) deferred_prompt: Option<String>,
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
        if command.name == trimmed || command.aliases.contains(&trimmed) {
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
    pub(crate) fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Scrolls the transcript downward by the requested number of lines.
    pub(crate) fn scroll_down(&mut self, amount: u16, line_count: u16) {
        let max_offset = line_count.saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + amount).min(max_offset);
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
}

/// Represents one interactive picker or onboarding panel inside the TUI.
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
}

impl OverlayState {
    /// Moves the active selection upward for list-based overlays.
    pub(crate) fn select_previous(&mut self) {
        match self {
            Self::SessionPicker { selection, .. }
            | Self::AgentPicker { selection, .. }
            | Self::ModelPicker { selection, .. }
            | Self::LoginPicker { selection, .. }
            | Self::ProviderPicker { selection, .. }
            | Self::AuthPicker { selection, .. }
            | Self::LogoutPicker { selection, .. }
            | Self::ThemePicker { selection, .. } => {
                *selection = selection.saturating_sub(1);
            }
            Self::ApiKeyPrompt { .. } => {}
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
            | Self::ModelPicker { entries, selection, .. }
            | Self::LoginPicker { entries, selection }
            | Self::ProviderPicker { entries, selection, .. }
            | Self::LogoutPicker { entries, selection }
            | Self::ThemePicker { entries, selection } => {
                *selection = (*selection + 1).min(entries.len().saturating_sub(1));
            }
            Self::AuthPicker {
                entries,
                selection,
                ..
            } => {
                *selection = (*selection + 1).min(entries.len().saturating_sub(1));
            }
            Self::ApiKeyPrompt { .. } => {}
        }
    }

    /// Moves up by a larger page-sized jump.
    pub(crate) fn page_up(&mut self) {
        for _ in 0..10 {
            self.select_previous();
        }
    }

    /// Moves down by a larger page-sized jump.
    pub(crate) fn page_down(&mut self) {
        for _ in 0..10 {
            self.select_next();
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
            Self::LoginPicker { entries, selection } => entries
                .get(*selection)
                .map(|entry| format!("/login {}", entry.selector)),
            Self::LogoutPicker { entries, selection } => entries
                .get(*selection)
                .map(|entry| format!("/logout {}", entry.selector)),
            Self::ThemePicker { entries, selection } => entries
                .get(*selection)
                .map(|entry| format!("/theme {}", entry.selector)),
            Self::ProviderPicker { .. }
            | Self::AuthPicker { .. }
            | Self::ApiKeyPrompt { .. } => None,
        }
    }

    /// Returns the currently selected provider id when present.
    pub(crate) fn selected_provider(&self) -> Option<&str> {
        match self {
            Self::LoginPicker { entries, selection }
            | Self::ProviderPicker { entries, selection, .. } => {
                entries.get(*selection).map(|entry| entry.selector.as_str())
            }
            Self::ModelPicker { provider_id, .. }
            | Self::AuthPicker { provider_id, .. }
            | Self::ApiKeyPrompt { provider_id, .. } => Some(provider_id.as_str()),
            Self::SessionPicker { .. }
            | Self::AgentPicker { .. }
            | Self::LogoutPicker { .. }
            | Self::ThemePicker { .. } => None,
        }
    }

    /// Returns the currently selected model id when the overlay is model-backed.
    pub(crate) fn selected_model(&self) -> Option<&str> {
        match self {
            Self::ModelPicker { entries, selection, .. } => {
                entries.get(*selection).map(|entry| entry.selector.as_str())
            }
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
            } | Self::ApiKeyPrompt {
                onboarding: true,
                ..
            } | Self::ThemePicker { .. }
        )
    }

    /// Returns true when the overlay accepts inline text input.
    pub(crate) fn accepts_text_input(&self) -> bool {
        matches!(self, Self::ApiKeyPrompt { .. })
    }

    /// Moves the text cursor left for inline API-key input.
    pub(crate) fn move_left(&mut self) {
        match self {
            Self::ApiKeyPrompt { value, cursor, .. } => {
                *cursor = previous_boundary(value, *cursor);
            }
            _ => {}
        }
    }

    /// Moves the text cursor right for inline API-key input.
    pub(crate) fn move_right(&mut self) {
        match self {
            Self::ApiKeyPrompt { value, cursor, .. } => {
                *cursor = next_boundary(value, *cursor);
            }
            _ => {}
        }
    }

    /// Moves the text cursor to the beginning for inline API-key input.
    pub(crate) fn move_home(&mut self) {
        match self {
            Self::ApiKeyPrompt { cursor, .. } => {
                *cursor = 0;
            }
            _ => {}
        }
    }

    /// Moves the text cursor to the end for inline API-key input.
    pub(crate) fn move_end(&mut self) {
        match self {
            Self::ApiKeyPrompt { value, cursor, .. } => {
                *cursor = value.len();
            }
            _ => {}
        }
    }

    /// Inserts one character into the API-key prompt.
    pub(crate) fn insert_char(&mut self, ch: char) {
        match self {
            Self::ApiKeyPrompt { value, cursor, .. } => {
                value.insert(*cursor, ch);
                *cursor += ch.len_utf8();
            }
            _ => {}
        }
    }

    /// Deletes the previous character from the API-key prompt.
    pub(crate) fn backspace(&mut self) {
        match self {
            Self::ApiKeyPrompt { value, cursor, .. } => {
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
            Self::ApiKeyPrompt { value, cursor, .. } => {
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
            _ => None,
        }
    }
}

/// Loads agent picker entries from the workspace `agents.yaml` file.
pub(crate) fn load_agent_picker_entries(
    cwd: &Path,
    current_model: Option<&str>,
) -> Result<Vec<ModelPickerEntry>> {
    let paths = ConfigPaths::discover(cwd);
    ensure_workspace_dirs(&paths)?;
    let agents_path = paths.workspace_config_dir.join("agents.yaml");
    if !agents_path.exists() {
        fs::write(&agents_path, default_agents_contents(current_model))?;
    }
    let raw = fs::read_to_string(&agents_path)?;
    let parsed: AgentFile = serde_yaml::from_str(&raw)?;
    Ok(parsed
        .agents
        .into_iter()
        .map(|agent| ModelPickerEntry {
            selector: agent.id,
            description: format!("role={} model={}", agent.role, agent.model),
        })
        .collect())
}

fn default_agents_contents(model: Option<&str>) -> String {
    format!(
        "agents:\n  - id: default\n    role: coding\n    model: {}\n",
        model.unwrap_or("anthropic/claude-sonnet-4-5")
    )
}

#[derive(Debug, Clone, Deserialize)]
struct AgentFile {
    agents: Vec<AgentPickerRecord>,
}

#[derive(Debug, Clone, Deserialize)]
struct AgentPickerRecord {
    id: String,
    role: String,
    model: String,
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
