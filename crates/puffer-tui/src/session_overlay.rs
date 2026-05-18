use crate::OverlayState;
use puffer_core::{
    render_session_overlay as build_session_overlay_view, AppState, SessionOverlayView,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::fmt;
use std::sync::{Arc, Mutex};

const MIN_OVERLAY_WIDTH: u16 = 40;
const MAX_OVERLAY_WIDTH: u16 = 88;

/// Stores the mutable interactive `/session` overlay state.
#[derive(Clone)]
pub(crate) struct SessionOverlay {
    shared: Arc<Mutex<SessionOverlayState>>,
}

#[derive(Debug, Clone)]
struct SessionOverlayState {
    view: SessionOverlayView,
    scroll: u16,
}

impl SessionOverlay {
    /// Builds the current `/session` overlay for the active session.
    pub(crate) fn open(state: &AppState) -> OverlayState {
        OverlayState::Session(SessionOverlay {
            shared: Arc::new(Mutex::new(SessionOverlayState {
                view: build_session_overlay_view(state),
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

    fn snapshot(&self) -> SessionOverlayState {
        self.shared
            .lock()
            .map(|state| state.clone())
            .unwrap_or(SessionOverlayState {
                view: SessionOverlayView {
                    remote_url: None,
                    remote_status: None,
                    qr: None,
                    notice: Some("Session overlay is unavailable.".to_string()),
                },
                scroll: 0,
            })
    }
}

impl PartialEq for SessionOverlay {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl Eq for SessionOverlay {}

impl fmt::Debug for SessionOverlay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionOverlay")
            .finish_non_exhaustive()
    }
}

/// Renders the remote-session overlay.
pub(crate) fn render_session_overlay(
    frame: &mut Frame<'_>,
    viewport: Rect,
    overlay: &SessionOverlay,
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
        height: viewport.height.saturating_sub(2).max(8),
    };
    frame.render_widget(Clear, area);
    let body = session_overlay_text(&snapshot.view);
    let visible_rows = usize::from(area.height.saturating_sub(2));
    let max_scroll = body
        .lines
        .len()
        .saturating_sub(visible_rows)
        .min(u16::MAX as usize) as u16;
    let scroll = snapshot.scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(body).scroll((scroll, 0)).block(
            Block::default()
                .title("Session")
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

fn session_overlay_text(view: &SessionOverlayView) -> Text<'static> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Remote session",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::default(),
    ];
    if let Some(status) = view.remote_status.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("Status: ", Style::default().add_modifier(Modifier::DIM)),
            Span::raw(status.to_string()),
        ]));
    }
    if let Some(url) = view.remote_url.as_deref() {
        lines.push(Line::from(vec![
            Span::styled(
                "Open in browser: ",
                Style::default().add_modifier(Modifier::DIM),
            ),
            Span::styled(url.to_string(), Style::default().fg(Color::Cyan)),
        ]));
    }
    if let Some(notice) = view.notice.as_deref() {
        if lines.len() > 2 {
            lines.push(Line::default());
        }
        let color = if view.remote_url.is_some() {
            Color::DarkGray
        } else {
            Color::Yellow
        };
        lines.push(Line::from(Span::styled(
            notice.to_string(),
            Style::default().fg(color),
        )));
    }
    if let Some(qr) = view.qr.as_deref() {
        lines.push(Line::default());
        lines.extend(qr.lines().map(|line| Line::from(line.to_string())));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Esc closes · Up/Down scroll · PgUp/PgDn page",
        Style::default().add_modifier(Modifier::DIM),
    )));
    Text::from(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn render_session_overlay_clamps_overscroll() {
        let backend = TestBackend::new(72, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let overlay = SessionOverlay {
            shared: Arc::new(Mutex::new(SessionOverlayState {
                view: SessionOverlayView {
                    remote_url: Some("https://puffer.local/session".to_string()),
                    remote_status: Some("online".to_string()),
                    qr: Some(
                        (0..20)
                            .map(|index| format!("qr-line-{index:02}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    notice: None,
                },
                scroll: 0,
            })),
        };

        for _ in 0..100 {
            overlay.page_down();
        }

        terminal
            .draw(|frame| {
                render_session_overlay(frame, frame.area(), &overlay);
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Session"));
        assert!(rendered.contains("qr-line-19"));
    }
}
