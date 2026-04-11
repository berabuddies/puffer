/// Returns the previous UTF-8 character boundary before the cursor.
pub(super) fn previous_boundary(input: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut index = cursor - 1;
    while index > 0 && !input.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Returns the next UTF-8 character boundary after the cursor.
pub(super) fn next_boundary(input: &str, cursor: usize) -> usize {
    if cursor >= input.len() {
        return input.len();
    }
    let mut index = cursor + 1;
    while index < input.len() && !input.is_char_boundary(index) {
        index += 1;
    }
    index.min(input.len())
}

/// Returns the largest legal bottom scroll offset for the transcript viewport.
pub(super) fn max_scroll_offset(line_count: u16, viewport_height: u16) -> u16 {
    line_count.saturating_sub(viewport_height.max(1))
}

#[cfg(test)]
mod tests {
    use super::super::TuiState;

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
