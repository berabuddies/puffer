use crate::list_selection_view::{ListSelectionView, SelectionItem};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use puffer_core::{PermissionPromptAction, PermissionPromptRequest};

/// One selectable approval option in the permission overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApprovalOption {
    pub(crate) label: String,
    pub(crate) action: PermissionPromptAction,
    pub(crate) shortcuts: Vec<char>,
    pub(crate) description: Option<String>,
}

/// Codex-style modal overlay for one permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApprovalOverlay {
    request: PermissionPromptRequest,
    options: Vec<ApprovalOption>,
    list: ListSelectionView,
}

impl ApprovalOverlay {
    /// Creates a permissions approval overlay with Codex-style options.
    pub(crate) fn new(request: PermissionPromptRequest) -> Self {
        let options = permissions_options(&request);
        let list = ListSelectionView::new(
            options
                .iter()
                .map(|option| SelectionItem {
                    label: option.label.clone(),
                    description: option.description.clone(),
                    shortcuts: option.shortcuts.clone(),
                })
                .collect(),
        );
        Self {
            request,
            options,
            list,
        }
    }

    /// Returns the currently selected index.
    pub(crate) fn selection(&self) -> usize {
        self.list.selection()
    }

    /// Moves the selection upward.
    pub(crate) fn select_previous(&mut self) {
        self.list.select_previous();
    }

    /// Moves the selection downward.
    pub(crate) fn select_next(&mut self) {
        self.list.select_next();
    }

    /// Moves the selection upward by one page.
    pub(crate) fn page_up(&mut self) {
        self.list.page_up();
    }

    /// Moves the selection downward by one page.
    pub(crate) fn page_down(&mut self) {
        self.list.page_down();
    }

    /// Selects an option by shortcut and returns the matching approval action.
    pub(crate) fn activate_shortcut(&mut self, key: char) -> Option<PermissionPromptAction> {
        let index = self.list.select_shortcut(key)?;
        self.options.get(index).map(|option| option.action)
    }

    /// Returns the currently highlighted approval action.
    pub(crate) fn selected_action(&self) -> PermissionPromptAction {
        self.options
            .get(self.selection())
            .map(|option| option.action)
            .unwrap_or(PermissionPromptAction::Deny)
    }

    /// Builds the line-based representation used by the inline composer dropdown.
    pub(crate) fn dropdown_lines(&self) -> Vec<Line<'static>> {
        let mut lines = self.body_lines();
        lines.push(Line::default());
        for (selected, item) in self.list.rows() {
            let mut spans = vec![Span::raw(if selected { "› " } else { "  " })];
            spans.push(Span::raw(item.label.clone()));
            if let Some(description) = &item.description {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    description.clone(),
                    Style::default().add_modifier(Modifier::DIM),
                ));
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::default());
        lines.push(self.footer_hint());
        lines
    }

    /// Renders the overlay into the given viewport.
    pub(crate) fn render(&self, frame: &mut Frame<'_>, viewport: Rect) {
        let width = viewport.width.saturating_sub(8).min(92);
        let body_lines = self.body_lines();
        let option_lines = self
            .list
            .rows()
            .into_iter()
            .map(|(selected, item)| {
                let mut label = item.label.clone();
                if let Some(description) = &item.description {
                    label.push_str("  ");
                    label.push_str(description);
                }
                ListItem::new(label).style(if selected {
                    Style::default()
                        .add_modifier(Modifier::BOLD)
                        .add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                })
            })
            .collect::<Vec<_>>();
        let height = (body_lines.len() as u16)
            .saturating_add(option_lines.len() as u16)
            .saturating_add(5)
            .min(viewport.height.saturating_sub(2).max(6));
        let area = Rect {
            x: viewport.x + viewport.width.saturating_sub(width) / 2,
            y: viewport.y + viewport.height.saturating_sub(height) / 2,
            width,
            height,
        };
        let block = Block::default()
            .title(" Permissions ")
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(Color::Cyan));
        frame.render_widget(Clear, area);
        frame.render_widget(&block, area);
        let inner = block.inner(area);
        let sections = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                Constraint::Length(body_lines.len() as u16),
                Constraint::Min(option_lines.len() as u16),
                Constraint::Length(1),
            ])
            .split(inner);
        frame.render_widget(
            Paragraph::new(body_lines).wrap(Wrap { trim: false }),
            sections[0],
        );
        frame.render_widget(List::new(option_lines), sections[1]);
        frame.render_widget(
            Paragraph::new(self.footer_hint()).style(Style::default().add_modifier(Modifier::DIM)),
            sections[2],
        );
    }

    fn body_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from("Would you like to grant these permissions?")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            Line::from(""),
            Line::from(vec![
                Span::styled("Action: ", Style::default().add_modifier(Modifier::DIM)),
                Span::raw(self.request.summary.clone()),
            ]),
        ];
        let is_browser_evaluate = self.request.browser.as_ref().is_some_and(|browser| {
            matches!(
                browser.action_set,
                puffer_core::BrowserPermissionPromptActionSet::Evaluate
            )
        });
        if is_browser_evaluate {
            if let Some(reason) = &self.request.reason {
                lines.push(Line::from(vec![
                    Span::styled("Reason: ", Style::default().add_modifier(Modifier::DIM)),
                    Span::raw(reason.clone()),
                ]));
            }
        } else if self.request.browser.is_none() {
            if let Some(reason) = &self.request.reason {
                lines.push(Line::from(vec![
                    Span::styled("Reason: ", Style::default().add_modifier(Modifier::DIM)),
                    Span::raw(reason.clone()),
                ]));
            }
        }
        lines
    }

    fn footer_hint(&self) -> Line<'static> {
        let mut spans = vec![
            Span::styled("y", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" approve once  "),
            Span::styled("a", Style::default().add_modifier(Modifier::BOLD)),
        ];
        if self.request.browser.is_some() {
            spans.push(Span::raw(" always allow context  "));
        } else {
            spans.push(Span::raw(" always allow  "));
            spans.push(Span::styled(
                "A",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" always allow all  "));
        }
        spans.push(Span::styled(
            "n",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" deny  "));
        spans.push(Span::styled(
            "Esc",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" deny"));
        Line::from(spans)
    }
}

fn permissions_options(request: &PermissionPromptRequest) -> Vec<ApprovalOption> {
    let mut options = vec![
        ApprovalOption {
            label: "Approve once".to_string(),
            action: PermissionPromptAction::AllowOnce,
            shortcuts: vec!['y'],
            description: None,
        },
        ApprovalOption {
            label: if request.browser.is_some() {
                "Always allow this browser context".to_string()
            } else {
                "Always allow this request".to_string()
            },
            action: PermissionPromptAction::AllowSession,
            shortcuts: vec!['a'],
            description: None,
        },
    ];
    options.push(ApprovalOption {
        label: "No, continue without permissions".to_string(),
        action: PermissionPromptAction::Deny,
        shortcuts: vec!['n'],
        description: None,
    });
    options
}

/// Renders the active permission overlay.
pub(crate) fn render_permission_overlay(
    frame: &mut Frame<'_>,
    viewport: Rect,
    overlay: &ApprovalOverlay,
) {
    overlay.render(frame, viewport)
}
