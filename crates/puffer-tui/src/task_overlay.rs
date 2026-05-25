use crate::{ModelPickerEntry, OverlayState};
use anyhow::Result;
use puffer_core::{render_task_actions, render_tasks_panel_text, AppState};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

const TASK_OVERLAY_TITLE: &str = "Background Tasks";
const MIN_OVERLAY_WIDTH: u16 = 56;
const MAX_OVERLAY_WIDTH: u16 = 108;
const MIN_OVERLAY_HEIGHT: u16 = 12;

/// Builds the dedicated `/tasks` overlay entry list for the current session.
pub(crate) fn open_task_overlay(state: &AppState) -> Result<OverlayState> {
    let mut preview_state = state.clone();
    let entries = render_task_actions(&mut preview_state)?
        .into_iter()
        .filter(|entry| should_include_task_entry(&entry.command))
        .map(|entry| {
            let selector = task_entry_selector(&entry.command);
            let description = task_entry_description(&entry.command, &selector, &entry.description);
            ModelPickerEntry {
                selector,
                description,
                command: Some(entry.command),
            }
        })
        .collect::<Vec<_>>();
    let selection = task_overlay_selection(&entries);
    Ok(OverlayState::CommandPicker {
        title: TASK_OVERLAY_TITLE.to_string(),
        entries,
        selection,
    })
}

/// Returns true when the overlay should use the dedicated background-task renderer.
pub(crate) fn is_task_overlay(overlay: &OverlayState) -> bool {
    matches!(
        overlay,
        OverlayState::CommandPicker { title, .. } if title == TASK_OVERLAY_TITLE
    )
}

/// Renders the dedicated background-task overlay with a picker plus preview pane.
pub(crate) fn render_task_overlay(
    frame: &mut Frame<'_>,
    viewport: Rect,
    state: &AppState,
    overlay: &OverlayState,
) {
    let OverlayState::CommandPicker {
        entries, selection, ..
    } = overlay
    else {
        return;
    };
    let width = viewport
        .width
        .saturating_sub(6)
        .clamp(MIN_OVERLAY_WIDTH, MAX_OVERLAY_WIDTH);
    let height = viewport.height.saturating_sub(2).max(MIN_OVERLAY_HEIGHT);
    let area = Rect {
        x: viewport.x + viewport.width.saturating_sub(width) / 2,
        y: viewport.y + viewport.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let outer = Block::default()
        .title(TASK_OVERLAY_TITLE)
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(accent_border_style());
    frame.render_widget(&outer, area);
    let inner = outer.inner(area);
    let sections = if inner.width >= 88 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(34), Constraint::Min(24)])
            .split(inner)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length((inner.height / 2).max(6)),
                Constraint::Min(6),
            ])
            .split(inner)
    };
    let list_rows = visible_entries(entries, *selection, sections[0].height.saturating_sub(2));
    let list_items = list_rows
        .into_iter()
        .map(|(entry, selected)| {
            let text = format!("{:<12} {}", entry.selector, entry.description);
            ListItem::new(text).style(if selected {
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            })
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(list_items).block(
            Block::default()
                .title("Views")
                .borders(Borders::ALL)
                .border_style(accent_border_style()),
        ),
        sections[0],
    );

    let preview_text = entries
        .get(*selection)
        .map(|entry| task_preview_text(state, entry))
        .unwrap_or_else(|| "No task data is available yet.".to_string());
    let preview_title = entries
        .get(*selection)
        .map(|entry| format!("Preview: {}", entry.selector))
        .unwrap_or_else(|| "Preview".to_string());
    frame.render_widget(
        Paragraph::new(format!(
            "{preview_text}\n\nEnter opens the selected view · Type to jump · Esc closes"
        ))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(preview_title)
                .borders(Borders::ALL)
                .border_style(accent_border_style()),
        ),
        sections[1],
    );
}

fn should_include_task_entry(command: &str) -> bool {
    if !command.starts_with("/tasks") {
        return true;
    }
    matches!(
        command,
        "/tasks show"
            | "/tasks todos"
            | "/tasks agents"
            | "/tasks teams"
            | "/tasks worktrees"
            | "/tasks path"
    ) || command.starts_with("/tasks show ")
        || command.starts_with("/tasks ignore ")
}

fn task_entry_selector(command: &str) -> String {
    if let Some(rest) = command.strip_prefix("Act on monitored task ") {
        return rest
            .split_once(':')
            .map(|(task_id, _)| task_id.trim().to_string())
            .unwrap_or_else(|| "action".to_string());
    }
    match command {
        "/tasks show" => "dashboard".to_string(),
        "/tasks todos" => "todos".to_string(),
        "/tasks agents" => "agents".to_string(),
        "/tasks teams" => "teams".to_string(),
        "/tasks worktrees" => "worktrees".to_string(),
        "/tasks path" => "paths".to_string(),
        _ => command
            .strip_prefix("/tasks show ")
            .or_else(|| command.strip_prefix("/tasks get "))
            .unwrap_or(command)
            .to_string(),
    }
}

fn task_entry_description(command: &str, selector: &str, description: &str) -> String {
    if command.starts_with("/tasks show ") {
        let repeated = format!("{selector} ");
        if let Some(trimmed) = description.strip_prefix(&repeated) {
            return trimmed.to_string();
        }
    }
    description.to_string()
}

fn task_overlay_selection(entries: &[ModelPickerEntry]) -> usize {
    let task_rows = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry.command.as_deref().is_some_and(|command| {
                command.starts_with("/tasks show ") && command != "/tasks show"
            })
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if task_rows.len() == 1 {
        task_rows[0]
    } else {
        0
    }
}

fn task_preview_text(state: &AppState, entry: &ModelPickerEntry) -> String {
    let Some(command) = entry.command.as_deref() else {
        return entry.description.clone();
    };
    let Some(args) = command.strip_prefix("/tasks") else {
        return entry.description.clone();
    };
    let mut preview_state = state.clone();
    match render_tasks_panel_text(&mut preview_state, args.trim()) {
        Ok(Some(text)) => text,
        Ok(None) | Err(_) => entry.description.clone(),
    }
}

fn visible_entries(
    entries: &[ModelPickerEntry],
    selection: usize,
    max_rows: u16,
) -> Vec<(&ModelPickerEntry, bool)> {
    let max_rows = usize::from(max_rows.max(1));
    if entries.len() <= max_rows {
        return entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry, index == selection))
            .collect();
    }
    let selection = selection.min(entries.len().saturating_sub(1));
    let start = selection
        .saturating_sub(max_rows / 2)
        .min(entries.len().saturating_sub(max_rows));
    let end = start + max_rows;
    entries[start..end]
        .iter()
        .enumerate()
        .map(|(offset, entry)| (entry, start + offset == selection))
        .collect()
}

fn accent_border_style() -> Style {
    Style::default().fg(Color::Cyan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::PufferConfig;
    use puffer_resources::LoadedResources;
    use puffer_session_store::SessionMetadata;
    use tempfile::tempdir;
    use uuid::Uuid;

    fn sample_state() -> (tempfile::TempDir, AppState) {
        let tempdir = tempdir().expect("tempdir");
        let cwd = tempdir.path().to_path_buf();
        let state = AppState::new(
            PufferConfig::default(),
            cwd.clone(),
            SessionMetadata {
                id: Uuid::nil(),
                display_name: Some("demo".to_string()),
                generated_title: None,
                cwd,
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        );
        (tempdir, state)
    }

    #[test]
    fn task_overlay_defaults_to_dashboard_when_no_background_rows_exist() {
        let (_tempdir, state) = sample_state();
        let overlay = open_task_overlay(&state).unwrap();
        let OverlayState::CommandPicker {
            title,
            entries,
            selection,
        } = overlay
        else {
            panic!("expected command picker");
        };
        assert_eq!(title, TASK_OVERLAY_TITLE);
        assert_eq!(selection, 0);
        assert_eq!(entries[0].selector, "dashboard");
        assert!(entries.iter().all(|entry| {
            entry
                .command
                .as_deref()
                .is_some_and(|command| !command.starts_with("/tasks output "))
        }));
    }

    #[test]
    fn task_overlay_preview_uses_read_only_task_panel_rendering() {
        let (_tempdir, state) = sample_state();
        let entry = ModelPickerEntry {
            selector: "dashboard".to_string(),
            description: "Show task dashboard".to_string(),
            command: Some("/tasks show".to_string()),
        };
        let preview = task_preview_text(&state, &entry);
        assert!(preview.contains("Task dashboard"));
    }

    #[test]
    fn task_overlay_includes_monitor_action_prompts() {
        let (_tempdir, mut state) = sample_state();
        let cwd = state.cwd.clone();
        puffer_core::execute_workflow_tool(
            &mut state,
            &LoadedResources::default(),
            &cwd,
            "TaskCreate",
            serde_json::json!({
                "subject": "Answer Slack thread",
                "description": "A teammate asked for release status in #ship.",
                "actions": [
                    {
                        "actionName": "Draft update",
                        "actionPrompt": "Draft a release status update for the Slack thread."
                    }
                ],
                "possibleIgnoreReasons": ["already answered"],
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": "slack-team",
                    "monitor_connector": "slack",
                    "monitor_memory_path": "/tmp/puffer-monitor-memory.md"
                }
            }),
            None,
        )
        .unwrap();

        let overlay = open_task_overlay(&state).unwrap();
        let OverlayState::CommandPicker { entries, .. } = overlay else {
            panic!("expected command picker");
        };
        assert!(entries.iter().any(|entry| {
            entry.selector == "monitor-1"
                && entry.command.as_deref().is_some_and(|command| {
                    command.starts_with("Act on monitored task monitor-1:")
                        && command.contains("Draft a release status update")
                })
        }));
        assert!(entries.iter().any(|entry| {
            entry
                .command
                .as_deref()
                .is_some_and(|command| command == "/tasks ignore monitor-1 already answered")
        }));
    }
}
