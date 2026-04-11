use super::TuiState;
use crate::prompt_history_store::{PromptHistorySource, PromptHistoryStore};
use crate::{ModelPickerEntry, OverlayState};
use anyhow::Result;
use puffer_core::CommandSpec;
use std::collections::VecDeque;
use std::path::Path;

const INPUT_HISTORY_LIMIT: usize = 100;
pub(crate) const PROMPT_HISTORY_SEARCH_TITLE: &str = "Search Prompt History";

/// Stores submitted composer inputs plus the active history navigation state.
#[derive(Debug, Clone, Default)]
pub(crate) struct PromptHistory {
    entries: VecDeque<String>,
    index: Option<usize>,
    draft: Option<String>,
    store: Option<PromptHistoryStore>,
}

impl TuiState {
    /// Attaches the shared prompt-history store and eagerly loads recent entries.
    pub(crate) fn configure_prompt_history_store(
        &mut self,
        store: PromptHistoryStore,
    ) -> Result<()> {
        self.prompt_history.store = Some(store);
        self.refresh_prompt_history_from_disk()
    }

    /// Returns true when at least one prior composer input can be recalled.
    pub(crate) fn has_prompt_history(&self) -> bool {
        !self.prompt_history.entries.is_empty()
    }

    /// Returns true when the composer currently displays a recalled history entry.
    pub(crate) fn is_prompt_history_active(&self) -> bool {
        self.prompt_history.index.is_some()
    }

    /// Reloads recent prompt history from the shared JSONL store when configured.
    pub(crate) fn refresh_prompt_history_from_disk(&mut self) -> Result<()> {
        let Some(store) = self.prompt_history.store.clone() else {
            return Ok(());
        };
        let recalled_text = self
            .prompt_history
            .index
            .and_then(|index| self.prompt_history.entries.get(index).cloned());
        self.prompt_history.entries = store.load_recent_texts(INPUT_HISTORY_LIMIT)?;
        if let Some(recalled_text) = recalled_text {
            self.prompt_history.index = self
                .prompt_history
                .entries
                .iter()
                .rposition(|entry| entry == &recalled_text);
        }
        Ok(())
    }

    /// Builds the Ctrl+R prompt-history picker overlay when history exists.
    pub(crate) fn prompt_history_search_overlay(&mut self) -> Result<Option<OverlayState>> {
        self.refresh_prompt_history_from_disk()?;
        let entries = self
            .prompt_history
            .entries
            .iter()
            .rev()
            .map(|entry| prompt_history_picker_entry(entry))
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return Ok(None);
        }
        Ok(Some(OverlayState::CommandPicker {
            title: PROMPT_HISTORY_SEARCH_TITLE.to_string(),
            entries,
            selection: 0,
        }))
    }

    /// Records one submitted or cleared composer input for later Up/Down recall.
    pub(crate) fn remember_prompt_history(
        &mut self,
        input: &str,
        source: PromptHistorySource,
        cwd: &Path,
    ) -> Result<()> {
        let input = input.trim();
        if input.is_empty() {
            self.reset_prompt_history_navigation();
            return Ok(());
        }
        if let Some(store) = self.prompt_history.store.as_ref() {
            store.append(input, source, cwd)?;
            self.prompt_history.entries = store.load_recent_texts(INPUT_HISTORY_LIMIT)?;
            self.reset_prompt_history_navigation();
            return Ok(());
        }
        if self
            .prompt_history
            .entries
            .back()
            .is_some_and(|entry| entry == input)
        {
            self.reset_prompt_history_navigation();
            return Ok(());
        }
        if self.prompt_history.entries.len() >= INPUT_HISTORY_LIMIT {
            self.prompt_history.entries.pop_front();
        }
        self.prompt_history.entries.push_back(input.to_string());
        self.reset_prompt_history_navigation();
        Ok(())
    }

    /// Stores the current composer input in history unless it already matches the active entry.
    pub(crate) fn archive_current_input(
        &mut self,
        source: PromptHistorySource,
        cwd: &Path,
    ) -> Result<bool> {
        let input = self.input.trim();
        if input.is_empty() {
            self.reset_prompt_history_navigation();
            return Ok(false);
        }
        let matches_active_entry = self
            .prompt_history
            .index
            .and_then(|index| self.prompt_history.entries.get(index))
            .is_some_and(|entry| entry == input);
        if matches_active_entry {
            self.reset_prompt_history_navigation();
            return Ok(true);
        }
        let snapshot = self.input.clone();
        self.remember_prompt_history(&snapshot, source, cwd)?;
        Ok(true)
    }

    /// Selects the previous submitted composer input, preserving the current draft.
    pub(crate) fn recall_previous_prompt(&mut self, commands: &[CommandSpec]) -> Result<bool> {
        self.refresh_prompt_history_from_disk()?;
        let entry_count = self.prompt_history.entries.len();
        if entry_count == 0 {
            return Ok(false);
        }
        let next_index = match self.prompt_history.index {
            Some(index) => index.saturating_sub(1),
            None => {
                self.prompt_history.draft = Some(self.input.clone());
                entry_count - 1
            }
        };
        self.prompt_history.index = Some(next_index);
        Ok(self.set_prompt_from_history(next_index, commands))
    }

    /// Selects the next submitted composer input or restores the unsent draft.
    pub(crate) fn recall_next_prompt(&mut self, commands: &[CommandSpec]) -> Result<bool> {
        self.refresh_prompt_history_from_disk()?;
        let Some(current_index) = self.prompt_history.index else {
            return Ok(false);
        };
        if current_index + 1 < self.prompt_history.entries.len() {
            let next_index = current_index + 1;
            self.prompt_history.index = Some(next_index);
            return Ok(self.set_prompt_from_history(next_index, commands));
        }
        self.prompt_history.index = None;
        self.input = self.prompt_history.draft.take().unwrap_or_default();
        self.cursor = self.input.len();
        self.sync(commands);
        Ok(true)
    }

    /// Clears the transient selection/draft used while walking prompt history.
    pub(crate) fn reset_prompt_history_navigation(&mut self) {
        self.prompt_history.index = None;
        self.prompt_history.draft = None;
    }

    /// Saves the current composer draft so it can be restored after closing a search overlay.
    pub(crate) fn stash_overlay_draft(&mut self) {
        self.overlay_draft_input = Some(self.input.clone());
    }

    /// Restores the stashed composer draft after a search overlay closes.
    pub(crate) fn restore_overlay_draft(&mut self, commands: &[CommandSpec]) {
        let draft = self.overlay_draft_input.take().unwrap_or_default();
        self.set_composer_input(draft, commands);
    }

    /// Discards any stashed composer draft without restoring it.
    pub(crate) fn discard_overlay_draft(&mut self) {
        self.overlay_draft_input = None;
    }

    /// Replaces the composer buffer with the provided text and refreshes slash completion state.
    pub(crate) fn set_composer_input(&mut self, input: String, commands: &[CommandSpec]) {
        self.input = input;
        self.cursor = self.input.len();
        self.slash_selection = 0;
        self.reset_prompt_history_navigation();
        self.sync(commands);
    }

    fn set_prompt_from_history(&mut self, index: usize, commands: &[CommandSpec]) -> bool {
        let Some(entry) = self.prompt_history.entries.get(index) else {
            return false;
        };
        self.input = entry.clone();
        self.cursor = self.input.len();
        self.sync(commands);
        true
    }
}

fn prompt_history_picker_entry(text: &str) -> ModelPickerEntry {
    let trimmed = text.trim();
    let mut lines = trimmed.lines();
    let first_line = lines.next().unwrap_or_default().trim();
    let selector = truncate_inline_text(first_line, 72);
    let description = match lines.find(|line| !line.trim().is_empty()) {
        Some(line) => truncate_inline_text(line.trim(), 72),
        None if trimmed.lines().count() > 1 => format!("{} lines", trimmed.lines().count()),
        None => String::new(),
    };
    ModelPickerEntry {
        selector: if selector.is_empty() {
            "<blank prompt>".to_string()
        } else {
            selector
        },
        description,
        command: Some(text.to_string()),
    }
}

fn truncate_inline_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::super::TuiState;
    use super::PROMPT_HISTORY_SEARCH_TITLE;
    use crate::prompt_history_store::{PromptHistorySource, PromptHistoryStore};
    use crate::OverlayState;
    use std::path::Path;

    #[test]
    fn recalling_previous_prompt_loads_latest_and_restores_draft() {
        let mut tui = TuiState::default();
        tui.remember_prompt_history("first", PromptHistorySource::Sent, Path::new("."))
            .unwrap();
        tui.remember_prompt_history("second", PromptHistorySource::Sent, Path::new("."))
            .unwrap();
        tui.input = "draft".to_string();
        tui.cursor = tui.input.len();

        assert!(tui.recall_previous_prompt(&[]).unwrap());
        assert_eq!(tui.input, "second");
        assert_eq!(tui.cursor, tui.input.len());
        assert!(tui.is_prompt_history_active());

        assert!(tui.recall_next_prompt(&[]).unwrap());
        assert_eq!(tui.input, "draft");
        assert_eq!(tui.cursor, tui.input.len());
        assert!(!tui.is_prompt_history_active());
    }

    #[test]
    fn remembering_prompt_history_ignores_empty_and_consecutive_duplicates() {
        let mut tui = TuiState::default();
        tui.remember_prompt_history("", PromptHistorySource::Sent, Path::new("."))
            .unwrap();
        tui.remember_prompt_history("hello", PromptHistorySource::Sent, Path::new("."))
            .unwrap();
        tui.remember_prompt_history("hello", PromptHistorySource::Sent, Path::new("."))
            .unwrap();

        assert!(tui.has_prompt_history());
        assert_eq!(tui.prompt_history.entries.len(), 1);
        assert!(tui.recall_previous_prompt(&[]).unwrap());
        assert_eq!(tui.input, "hello");
    }

    #[test]
    fn archiving_current_input_skips_duplicate_active_history_entry() {
        let mut tui = TuiState::default();
        tui.remember_prompt_history("hello", PromptHistorySource::Sent, Path::new("."))
            .unwrap();
        assert!(tui.recall_previous_prompt(&[]).unwrap());
        assert!(tui
            .archive_current_input(PromptHistorySource::Cleared, Path::new("."))
            .unwrap());
        assert!(tui.recall_previous_prompt(&[]).unwrap());
        assert_eq!(tui.input, "hello");
        assert_eq!(tui.prompt_history.entries.len(), 1);
    }
    #[test]
    fn configured_store_refreshes_entries_written_by_another_tui() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = PromptHistoryStore::new(tempdir.path().join("prompt_history.jsonl"));
        let mut first = TuiState::default();
        let mut second = TuiState::default();
        first.configure_prompt_history_store(store.clone()).unwrap();
        second.configure_prompt_history_store(store).unwrap();

        first
            .remember_prompt_history("shared prompt", PromptHistorySource::Sent, tempdir.path())
            .unwrap();

        assert!(second.recall_previous_prompt(&[]).unwrap());
        assert_eq!(second.input, "shared prompt");
    }

    #[test]
    fn prompt_history_search_overlay_lists_newest_entries_first() {
        let mut tui = TuiState::default();
        tui.remember_prompt_history("first", PromptHistorySource::Sent, Path::new("."))
            .unwrap();
        tui.remember_prompt_history("second", PromptHistorySource::Sent, Path::new("."))
            .unwrap();

        let overlay = tui.prompt_history_search_overlay().unwrap().unwrap();
        let OverlayState::CommandPicker { title, entries, .. } = overlay else {
            panic!("expected command picker");
        };
        assert_eq!(title, PROMPT_HISTORY_SEARCH_TITLE);
        assert_eq!(entries[0].command.as_deref(), Some("second"));
        assert_eq!(entries[1].command.as_deref(), Some("first"));
    }
}
