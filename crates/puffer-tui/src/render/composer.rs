use super::onboarding_body_lines;
use super::overlay_content::{overlay_rows, overlay_title};
use super::overlay_list::{overlay_selection, visible_overlay_rows};
use crate::popup::popup_rows;
use crate::OverlayState;
use puffer_core::CommandSpec;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

const MAX_INLINE_DROPDOWN_ROWS: usize = 8;

/// Returns the footer height required for the composer and any attached dropdown.
pub(super) fn composer_area_height(help_active: bool, dropdown_height: u16) -> u16 {
    if dropdown_height > 0 {
        2 + dropdown_height
    } else if help_active {
        2
    } else {
        3
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
        _ => input.to_string(),
    }
}

/// Returns the placeholder text shown while an inline overlay is active.
pub(super) fn overlay_prompt_placeholder(overlay: Option<&OverlayState>) -> &'static str {
    match overlay {
        Some(OverlayState::ApiKeyPrompt { .. } | OverlayState::OnboardingApiKey { .. }) => {
            "Paste API key"
        }
        _ => "Type to jump",
    }
}

/// Returns the cursor position for inline overlay prompt input.
pub(super) fn overlay_prompt_cursor(cursor: usize, overlay: Option<&OverlayState>) -> usize {
    match overlay {
        Some(OverlayState::ApiKeyPrompt { cursor, .. }) => *cursor,
        Some(OverlayState::OnboardingApiKey { cursor, .. }) => *cursor,
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
    if input.starts_with('/') {
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
    rows.into_iter()
        .enumerate()
        .map(|(index, command)| {
            let argument_hint = command
                .argument_hint
                .as_deref()
                .map(|value| format!("  {value}"))
                .unwrap_or_default();
            selection_line(
                format!("/{}  {}{}", command.name, command.description, argument_hint),
                index == slash_selection,
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
        _ if overlay.is_onboarding() => onboarding_body_lines(overlay, MAX_INLINE_DROPDOWN_ROWS),
        _ => generic_overlay_dropdown_lines(overlay),
    }
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
        lines.extend(rows.into_iter().map(|row| selection_line(row.text, row.selected)));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        if overlay.is_onboarding() {
            "Enter to continue · Esc to go back"
        } else {
            "Typing jumps selection · Enter to select · Esc to close"
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
