use crate::OverlayState;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use std::fmt;
use std::sync::{Arc, Mutex};

const MIN_OVERLAY_WIDTH: u16 = 34;
const MAX_OVERLAY_WIDTH: u16 = 200;

/// Stores a generic scrollable text overlay used for settings-style slash commands.
#[derive(Clone)]
pub(crate) struct TextOverlay {
    shared: Arc<Mutex<TextOverlayState>>,
}

#[derive(Debug, Clone)]
struct TextOverlayState {
    title: String,
    body: Text<'static>,
    scroll: u16,
}

impl TextOverlay {
    /// Builds a generic text overlay wrapped in `OverlayState`.
    pub(crate) fn open(title: impl Into<String>, body: impl Into<String>) -> OverlayState {
        let body_str = body.into();
        let mut lines: Vec<Line<'static>> = body_str
            .lines()
            .map(|l| Line::from(l.to_string()))
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "j/k ↑/↓ scroll · g/G top/bottom · ^U/^D half-page · PgUp/PgDn page · q/Esc closes",
            Style::default().fg(Color::DarkGray),
        )));
        OverlayState::Text(TextOverlay {
            shared: Arc::new(Mutex::new(TextOverlayState {
                title: title.into(),
                body: Text::from(lines),
                scroll: 0,
            })),
        })
    }

    /// Builds a styled text overlay from pre-colored lines.
    pub(crate) fn open_styled(title: impl Into<String>, body: Text<'static>) -> OverlayState {
        let mut lines = body.lines;
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "j/k ↑/↓ scroll · g/G top/bottom · ^U/^D half-page · PgUp/PgDn page · q/Esc closes",
            Style::default().fg(Color::DarkGray),
        )));
        OverlayState::Text(TextOverlay {
            shared: Arc::new(Mutex::new(TextOverlayState {
                title: title.into(),
                body: Text::from(lines),
                scroll: 0,
            })),
        })
    }

    /// Scrolls the overlay upward by one row.
    pub(crate) fn scroll_up(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = state.scroll.saturating_sub(1);
        }
    }

    /// Scrolls the overlay downward by one row.
    pub(crate) fn scroll_down(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = state.scroll.saturating_add(1);
        }
    }

    /// Scrolls the overlay upward by one page.
    pub(crate) fn page_up(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = state.scroll.saturating_sub(30);
        }
    }

    /// Scrolls the overlay downward by one page.
    pub(crate) fn page_down(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = state.scroll.saturating_add(30);
        }
    }

    /// Scrolls upward by half a page (Ctrl+u).
    pub(crate) fn half_page_up(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = state.scroll.saturating_sub(15);
        }
    }

    /// Scrolls downward by half a page (Ctrl+d).
    pub(crate) fn half_page_down(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = state.scroll.saturating_add(15);
        }
    }

    /// Jumps to the top (gg).
    pub(crate) fn scroll_to_top(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = 0;
        }
    }

    /// Jumps to the bottom (G).
    pub(crate) fn scroll_to_bottom(&self) {
        if let Ok(mut state) = self.shared.lock() {
            // The renderer clamps this sentinel once it knows the viewport height.
            state.scroll = u16::MAX / 2;
        }
    }

    fn snapshot(&self) -> TextOverlayState {
        self.shared
            .lock()
            .map(|state| state.clone())
            .unwrap_or(TextOverlayState {
                title: "Panel".to_string(),
                body: Text::from("Overlay unavailable."),
                scroll: 0,
            })
    }
}

impl PartialEq for TextOverlay {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl Eq for TextOverlay {}

impl fmt::Debug for TextOverlay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TextOverlay")
            .finish_non_exhaustive()
    }
}

/// Renders a generic scrollable text overlay.
pub(crate) fn render_text_overlay(frame: &mut Frame<'_>, viewport: Rect, overlay: &TextOverlay) {
    let snapshot = overlay.snapshot();
    let width = viewport
        .width
        .saturating_sub(4)
        .clamp(MIN_OVERLAY_WIDTH, MAX_OVERLAY_WIDTH);
    let area = Rect {
        x: viewport.x + viewport.width.saturating_sub(width) / 2,
        y: viewport.y + 1,
        width,
        height: viewport.height.saturating_sub(2).max(6),
    };
    frame.render_widget(Clear, area);
    let body_height = area.height.saturating_sub(2);
    let max_scroll = snapshot
        .body
        .lines
        .len()
        .saturating_sub(body_height as usize)
        .min(u16::MAX as usize) as u16;
    let scroll = snapshot.scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(snapshot.body)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(snapshot.title)
                    .borders(Borders::ALL)
                    .border_style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
            ),
        area,
    );
}

/// Colorizes the raw debug context string into styled Text for the overlay.
pub(crate) fn colorize_debug_context(raw: &str) -> Text<'static> {
    let body_style = Style::default().fg(Color::Rgb(140, 140, 140));
    let header_style = Style::default()
        .fg(Color::Rgb(100, 180, 220))
        .add_modifier(Modifier::BOLD);
    let section_style = Style::default().fg(Color::Rgb(80, 150, 180));
    let user_style = Style::default().fg(Color::Rgb(120, 200, 120));
    let assistant_style = Style::default().fg(Color::Rgb(180, 140, 220));
    let system_style = Style::default().fg(Color::Rgb(200, 180, 100));
    let tool_call_style = Style::default().fg(Color::Rgb(200, 140, 80));
    let tool_result_style = Style::default().fg(Color::Rgb(140, 180, 200));
    let json_key_style = Style::default().fg(Color::Rgb(130, 170, 200));
    let json_string_style = Style::default().fg(Color::Rgb(160, 140, 120));
    let json_punct_style = Style::default().fg(Color::Rgb(100, 100, 110));
    let thinking_style = Style::default()
        .fg(Color::Rgb(110, 110, 130))
        .add_modifier(Modifier::ITALIC);
    let input_style = Style::default().fg(Color::Rgb(120, 120, 120));
    let hint_style = Style::default().fg(Color::DarkGray);

    let mut in_tools = false;
    let lines: Vec<Line<'static>> = raw
        .lines()
        .map(|line| {
            // Track whether we're inside the TOOLS section for JSON coloring.
            if line.starts_with("┌─── TOOLS") {
                in_tools = true;
                return Line::from(Span::styled(line.to_string(), section_style));
            }
            if line.starts_with("└─── END TOOLS") {
                in_tools = false;
                return Line::from(Span::styled(line.to_string(), section_style));
            }

            if line.starts_with("━━━") {
                Line::from(Span::styled(line.to_string(), header_style))
            } else if line.starts_with("┌───") || line.starts_with("└───") {
                Line::from(Span::styled(line.to_string(), section_style))
            } else if in_tools {
                colorize_json_line(line, json_key_style, json_string_style, json_punct_style)
            } else if line.contains("] USER") {
                Line::from(Span::styled(line.to_string(), user_style))
            } else if line.contains("] ASSISTANT") {
                Line::from(Span::styled(line.to_string(), assistant_style))
            } else if line.contains("] SYSTEM") {
                Line::from(Span::styled(line.to_string(), system_style))
            } else if line.contains("] TOOL_CALL") {
                Line::from(Span::styled(line.to_string(), tool_call_style))
            } else if line.contains("] TOOL_RESULT") {
                Line::from(Span::styled(line.to_string(), tool_result_style))
            } else if line.starts_with("<thinking>")
                || line.starts_with("</thinking>")
                || line.starts_with("  input: ")
            {
                Line::from(Span::styled(line.to_string(), input_style))
            } else if line.contains("scroll") && line.contains("Esc") {
                Line::from(Span::styled(line.to_string(), hint_style))
            } else if line.starts_with("  ") {
                // Thinking content inside <thinking> blocks
                if line.trim().is_empty() {
                    Line::from("")
                } else {
                    Line::from(Span::styled(line.to_string(), thinking_style))
                }
            } else {
                Line::from(Span::styled(line.to_string(), body_style))
            }
        })
        .collect();

    Text::from(lines)
}

/// Applies lightweight JSON syntax coloring to a single line.
fn colorize_json_line(
    line: &str,
    key_style: Style,
    string_style: Style,
    punct_style: Style,
) -> Line<'static> {
    let trimmed = line.trim();
    // Pure punctuation lines: { } [ ] },
    if matches!(trimmed, "{" | "}" | "[" | "]" | "}," | "],") {
        return Line::from(Span::styled(line.to_string(), punct_style));
    }
    // Key-value lines like `  "name": "value"` or `  "name": {`
    if let Some(colon_pos) = find_json_colon(line) {
        let (key_part, rest) = line.split_at(colon_pos + 1);
        let mut spans = vec![Span::styled(key_part.to_string(), key_style)];
        let rest_trimmed = rest.trim();
        if rest_trimmed.starts_with('"') {
            spans.push(Span::styled(rest.to_string(), string_style));
        } else {
            spans.push(Span::styled(rest.to_string(), punct_style));
        }
        return Line::from(spans);
    }
    // Standalone string values in arrays
    if trimmed.starts_with('"') {
        return Line::from(Span::styled(line.to_string(), string_style));
    }
    Line::from(Span::styled(line.to_string(), punct_style))
}

/// Finds the position of the colon in a JSON key-value line (after the closing quote of the key).
fn find_json_colon(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('"') {
        return None;
    }
    // Find closing quote of key, then the colon.
    let offset = line.len() - trimmed.len();
    let mut chars = trimmed[1..].char_indices();
    let mut escaped = false;
    while let Some((i, ch)) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            // Look for ':' after this position.
            let after_quote = &trimmed[i + 2..];
            if let Some(colon_offset) = after_quote.find(':') {
                return Some(offset + 1 + i + 1 + colon_offset);
            }
            return None;
        }
    }
    None
}
