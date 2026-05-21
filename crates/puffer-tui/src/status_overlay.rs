use crate::OverlayState;
use puffer_core::{render_status_summary, AppState};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use std::fmt;
use std::sync::{Arc, Mutex};

const MIN_OVERLAY_WIDTH: u16 = 34;
const MAX_OVERLAY_WIDTH: u16 = 84;

/// Stores the mutable interactive `/status` overlay state.
#[derive(Clone)]
pub(crate) struct StatusOverlay {
    shared: Arc<Mutex<StatusOverlayState>>,
}

#[derive(Debug, Clone)]
struct StatusOverlayState {
    body: String,
    scroll: u16,
}

impl StatusOverlay {
    /// Builds the current status overlay for the active session.
    pub(crate) fn open(
        state: &AppState,
        resources: &LoadedResources,
        providers: &ProviderRegistry,
        auth_store: &AuthStore,
    ) -> OverlayState {
        OverlayState::Status(StatusOverlay {
            shared: Arc::new(Mutex::new(StatusOverlayState {
                body: render_status_summary(state, resources, providers, auth_store),
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
            state.scroll = state.scroll.saturating_sub(10);
        }
    }

    /// Scrolls the overlay downward by one page.
    pub(crate) fn page_down(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.scroll = state.scroll.saturating_add(10);
        }
    }

    fn snapshot(&self) -> StatusOverlayState {
        self.shared
            .lock()
            .map(|state| state.clone())
            .unwrap_or(StatusOverlayState {
                body: "Status overlay is unavailable.".to_string(),
                scroll: 0,
            })
    }
}

impl PartialEq for StatusOverlay {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl Eq for StatusOverlay {}

impl fmt::Debug for StatusOverlay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StatusOverlay")
            .finish_non_exhaustive()
    }
}

/// Renders the status overlay.
pub(crate) fn render_status_overlay(
    frame: &mut Frame<'_>,
    viewport: Rect,
    overlay: &StatusOverlay,
) {
    let snapshot = overlay.snapshot();
    let width = viewport
        .width
        .saturating_sub(8)
        .clamp(MIN_OVERLAY_WIDTH, MAX_OVERLAY_WIDTH);
    let area = Rect {
        x: viewport.x + viewport.width.saturating_sub(width) / 2,
        y: viewport.y + 1,
        width,
        height: viewport.height.saturating_sub(2).max(6),
    };
    frame.render_widget(Clear, area);
    let body = format!(
        "{}\n\n↑/↓ scroll · PgUp/PgDn page · Esc closes",
        snapshot.body
    );
    let content_width = area.width.saturating_sub(2).max(1);
    let visible_rows = usize::from(area.height.saturating_sub(2));
    let max_scroll = Paragraph::new(body.as_str())
        .wrap(Wrap { trim: false })
        .line_count(content_width)
        .saturating_sub(visible_rows)
        .min(u16::MAX as usize) as u16;
    let scroll = snapshot.scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(body)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title("Status")
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn render_status_overlay_clamps_overscroll() {
        let backend = TestBackend::new(72, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let overlay = StatusOverlay {
            shared: Arc::new(Mutex::new(StatusOverlayState {
                body: (0..20)
                    .map(|index| format!("status-line-{index:02}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                scroll: 0,
            })),
        };

        for _ in 0..100 {
            overlay.page_down();
        }

        terminal
            .draw(|frame| {
                render_status_overlay(frame, frame.area(), &overlay);
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Status"));
        assert!(rendered.contains("status-line-19"));
    }
}
