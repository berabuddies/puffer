/// One selectable row in a list-style modal selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionItem {
    pub(crate) label: String,
    pub(crate) description: Option<String>,
    pub(crate) shortcuts: Vec<char>,
}

/// Minimal Codex-style list selection primitive for modal overlays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListSelectionView {
    items: Vec<SelectionItem>,
    selection: usize,
}

impl ListSelectionView {
    /// Creates a new selection view with the first item selected.
    pub(crate) fn new(items: Vec<SelectionItem>) -> Self {
        Self {
            items,
            selection: 0,
        }
    }

    /// Returns the currently selected item index.
    pub(crate) fn selection(&self) -> usize {
        self.selection
    }

    /// Moves the selection one item upward.
    pub(crate) fn select_previous(&mut self) {
        self.selection = self.selection.saturating_sub(1);
    }

    /// Moves the selection one item downward.
    pub(crate) fn select_next(&mut self) {
        self.selection = (self.selection + 1).min(self.items.len().saturating_sub(1));
    }

    /// Moves the selection to the final item.
    pub(crate) fn select_last(&mut self) {
        self.selection = self.items.len().saturating_sub(1);
    }

    /// Moves the selection to a specific item when present.
    pub(crate) fn select_index(&mut self, index: usize) {
        self.selection = index.min(self.items.len().saturating_sub(1));
    }

    /// Moves the selection upward by a page-sized jump.
    pub(crate) fn page_up(&mut self) {
        for _ in 0..10 {
            self.select_previous();
        }
    }

    /// Moves the selection downward by a page-sized jump.
    pub(crate) fn page_down(&mut self) {
        for _ in 0..10 {
            self.select_next();
        }
    }

    /// Selects the first item that advertises the provided shortcut.
    pub(crate) fn select_shortcut(&mut self, key: char) -> Option<usize> {
        // Try exact (case-sensitive) match first, then fall back to case-insensitive.
        let index = self
            .items
            .iter()
            .position(|item| item.shortcuts.iter().copied().any(|s| s == key))
            .or_else(|| {
                let lower = key.to_ascii_lowercase();
                self.items.iter().position(|item| {
                    item.shortcuts
                        .iter()
                        .copied()
                        .any(|s| s.to_ascii_lowercase() == lower)
                })
            })?;
        self.selection = index;
        Some(index)
    }

    /// Returns all rows for rendering with selection metadata.
    pub(crate) fn rows(&self) -> Vec<(bool, &SelectionItem)> {
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| (index == self.selection, item))
            .collect()
    }
}
