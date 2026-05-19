use super::onboarding_body_lines;
use super::overlay_content::{overlay_rows, overlay_title};
use super::overlay_list::{overlay_selection, visible_overlay_rows};
use crate::markdown::render_markdown;
use crate::popup::popup_rows;
use crate::user_question_overlay::UserQuestionOverlay;
use crate::OverlayState;
use puffer_core::CommandSpec;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

const MAX_INLINE_DROPDOWN_ROWS: usize = 8;
const MAX_USER_QUESTION_PREVIEW_LINES: usize = 8;
/// Hard cap on how tall the input prompt is allowed to grow when the user
/// inserts newlines or pastes are inserted verbatim. Beyond this the prompt
/// is rendered with an internal scroll so the rest of the UI stays visible.
const MAX_PROMPT_LINES: u16 = 6;

/// Returns the number of display rows the prompt needs to show `input` —
/// one per `\n`-delimited line, capped at [`MAX_PROMPT_LINES`].
pub(super) fn prompt_line_count(input: &str) -> u16 {
    let logical_lines = input.matches('\n').count().saturating_add(1) as u16;
    logical_lines.clamp(1, MAX_PROMPT_LINES)
}

/// Returns the footer height required for the composer and any attached dropdown.
pub(super) fn composer_area_height(
    help_active: bool,
    dropdown_height: u16,
    prompt_lines: u16,
) -> u16 {
    let prompt_lines = prompt_lines.max(1);
    // 1 row of separator + the prompt lines + (dropdown rows OR a hint row).
    if dropdown_height > 0 {
        1 + prompt_lines + dropdown_height
    } else if help_active {
        1 + prompt_lines
    } else {
        1 + prompt_lines + 1
    }
}

/// Returns true when the overlay should render as a composer-attached dropdown.
pub(super) fn overlay_renders_inline_dropdown(overlay: &OverlayState) -> bool {
    matches!(
        overlay,
        OverlayState::SessionPicker { .. }
            | OverlayState::AgentPicker { .. }
            | OverlayState::ModelPicker { .. }
            | OverlayState::EffortPicker { .. }
            | OverlayState::FastModePicker { .. }
            | OverlayState::LoginPicker { .. }
            | OverlayState::ProviderPicker { .. }
            | OverlayState::AuthPicker { .. }
            | OverlayState::ApiKeyPrompt { .. }
            | OverlayState::LogoutPicker { .. }
            | OverlayState::ThemePicker { .. }
            | OverlayState::CommandPicker { .. }
            | OverlayState::PermissionPrompt { .. }
            | OverlayState::UserQuestionPrompt { .. }
            | OverlayState::OnboardingTheme { .. }
            | OverlayState::OnboardingProvider { .. }
            | OverlayState::OnboardingAuth { .. }
            | OverlayState::OnboardingModel { .. }
            | OverlayState::OnboardingApiKey { .. }
    )
}

/// Returns the wrapped height required by the inline dropdown.
pub(super) fn inline_dropdown_height(
    active_overlay: Option<&OverlayState>,
    input: &str,
    slash_selection: usize,
    commands: &[CommandSpec],
    width: u16,
) -> u16 {
    let Some(text) = inline_dropdown_text(active_overlay, input, slash_selection, commands) else {
        return 0;
    };
    Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .line_count(width.max(1))
        .min(u16::MAX as usize) as u16
}

/// Renders the active inline dropdown beneath the composer prompt row.
pub(super) fn render_inline_dropdown(
    frame: &mut Frame<'_>,
    area: Rect,
    active_overlay: Option<&OverlayState>,
    input: &str,
    slash_selection: usize,
    commands: &[CommandSpec],
) {
    let Some(text) = inline_dropdown_text(active_overlay, input, slash_selection, commands) else {
        return;
    };
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
}

/// Returns the prompt text shown while an inline overlay is active.
pub(super) fn overlay_prompt_input(input: &str, overlay: Option<&OverlayState>) -> String {
    match overlay {
        Some(OverlayState::ApiKeyPrompt { value, .. }) => "*".repeat(value.chars().count()),
        Some(OverlayState::OnboardingApiKey { input, .. }) => "*".repeat(input.chars().count()),
        Some(OverlayState::UserQuestionPrompt { overlay }) => overlay.custom_answer().to_string(),
        _ => input.to_string(),
    }
}

/// Returns the placeholder text shown while an inline overlay is active.
pub(super) fn overlay_prompt_placeholder(overlay: Option<&OverlayState>) -> &'static str {
    match overlay {
        Some(OverlayState::ApiKeyPrompt { .. } | OverlayState::OnboardingApiKey { .. }) => {
            "Paste API key"
        }
        Some(OverlayState::UserQuestionPrompt { .. }) => "Type custom answer",
        Some(overlay) if !overlay.accepts_filter_input() => "Overlay open",
        _ => "Type to jump",
    }
}

/// Returns the cursor position for inline overlay prompt input.
pub(super) fn overlay_prompt_cursor(cursor: usize, overlay: Option<&OverlayState>) -> usize {
    match overlay {
        Some(OverlayState::ApiKeyPrompt { cursor, .. }) => *cursor,
        Some(OverlayState::OnboardingApiKey { cursor, .. }) => *cursor,
        Some(OverlayState::UserQuestionPrompt { overlay }) => overlay.custom_answer().len(),
        _ => cursor,
    }
}

fn inline_dropdown_text(
    active_overlay: Option<&OverlayState>,
    input: &str,
    slash_selection: usize,
    commands: &[CommandSpec],
) -> Option<Text<'static>> {
    if let Some(overlay) = active_overlay {
        if overlay_renders_inline_dropdown(overlay) {
            return Some(Text::from(overlay_dropdown_lines(overlay)));
        }
        return None;
    }
    if input.starts_with('/') && !input.contains(' ') {
        return Some(Text::from(command_dropdown_lines(
            input,
            slash_selection,
            commands,
        )));
    }
    None
}

fn command_dropdown_lines(
    input: &str,
    slash_selection: usize,
    commands: &[CommandSpec],
) -> Vec<Line<'static>> {
    let rows = popup_rows(input, commands);
    if rows.is_empty() {
        return vec![Line::from(Span::styled(
            "  no matches",
            Style::default().add_modifier(Modifier::DIM),
        ))];
    }
    let selected_index = slash_selection.min(rows.len() - 1);
    rows.into_iter()
        .enumerate()
        .map(|(index, command)| {
            let selected = index == selected_index;
            let argument_hint = command
                .argument_hint
                .as_deref()
                .map(|value| format!("  {value}"))
                .unwrap_or_default();
            command_selection_line(
                &command.name,
                &format!("{}{}", command.description, argument_hint),
                selected,
            )
        })
        .collect()
}

fn overlay_dropdown_lines(overlay: &OverlayState) -> Vec<Line<'static>> {
    match overlay {
        OverlayState::PermissionPrompt { overlay } => overlay.dropdown_lines(),
        OverlayState::ApiKeyPrompt { provider_id, .. } => {
            api_key_dropdown_lines("Enter API Key", provider_id)
        }
        OverlayState::OnboardingApiKey { provider_name, .. } => {
            api_key_dropdown_lines("Let's get started.", provider_name)
        }
        OverlayState::UserQuestionPrompt { overlay } => user_question_dropdown_lines(overlay),
        _ if overlay.is_onboarding() => onboarding_body_lines(overlay, MAX_INLINE_DROPDOWN_ROWS),
        _ => generic_overlay_dropdown_lines(overlay),
    }
}

fn user_question_dropdown_lines(overlay: &UserQuestionOverlay) -> Vec<Line<'static>> {
    let mut lines = generic_overlay_dropdown_lines(&OverlayState::UserQuestionPrompt {
        overlay: overlay.clone(),
    });
    let Some(preview) = overlay.selected_preview() else {
        return lines;
    };
    let footer = lines.pop();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Preview",
        Style::default().add_modifier(Modifier::DIM),
    )));
    let mut preview_lines = render_markdown(preview).lines;
    let truncated = preview_lines.len() > MAX_USER_QUESTION_PREVIEW_LINES;
    preview_lines.truncate(MAX_USER_QUESTION_PREVIEW_LINES);
    lines.extend(preview_lines);
    if truncated {
        lines.push(Line::from(Span::styled(
            "... preview truncated",
            Style::default().add_modifier(Modifier::DIM),
        )));
    }
    if let Some(footer) = footer {
        lines.push(Line::default());
        lines.push(footer);
    }
    lines
}

fn generic_overlay_dropdown_lines(overlay: &OverlayState) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        overlay_title(overlay).into_owned(),
        Style::default().add_modifier(Modifier::DIM),
    ))];
    let rows = visible_overlay_rows(
        overlay_rows(overlay),
        overlay_selection(overlay),
        MAX_INLINE_DROPDOWN_ROWS,
    );
    if !rows.is_empty() {
        lines.push(Line::default());
        lines.extend(
            rows.into_iter()
                .map(|row| selection_line(row.text, row.selected)),
        );
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        if overlay.is_onboarding() {
            "Enter to continue · Esc to go back"
        } else if overlay.accepts_filter_input() {
            "Typing jumps selection · Enter to select · Esc to close"
        } else {
            "Use arrows or shortcuts · Enter to select · Esc to close"
        },
        Style::default().add_modifier(Modifier::DIM),
    )));
    lines
}

fn api_key_dropdown_lines(title: &str, provider: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            title.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::default(),
        Line::from("Paste an API key into the composer."),
        Line::from(Span::styled(
            format!("{provider} will use this key for API requests."),
            Style::default().add_modifier(Modifier::DIM),
        )),
        Line::default(),
        Line::from(Span::styled(
            "Enter to continue · Esc to go back",
            Style::default().add_modifier(Modifier::DIM),
        )),
    ]
}

fn selection_line(text: String, selected: bool) -> Line<'static> {
    let prefix = if selected { "› " } else { "  " };
    Line::from(vec![
        Span::styled(
            prefix.to_string(),
            if selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
        Span::styled(
            text,
            if selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
    ])
}

fn command_selection_line(name: &str, description: &str, selected: bool) -> Line<'static> {
    let prefix = if selected { "› " } else { "  " };
    let base = if selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let cmd_style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled(prefix.to_string(), base),
        Span::styled(format!("/{name}"), cmd_style),
        Span::styled(format!("  {description}"), base),
    ])
}
