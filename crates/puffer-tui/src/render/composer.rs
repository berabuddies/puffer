use super::onboarding_body_lines;
use super::overlay_content::{overlay_rows, overlay_title};
use super::overlay_list::{overlay_selection, visible_overlay_rows};
use crate::markdown::render_markdown;
use crate::popup::{popup_accepts_input, popup_rows};
use crate::user_question_overlay::UserQuestionOverlay;
use crate::OverlayState;
use puffer_core::CommandSpec;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
            | OverlayState::AutoDreamSuggestion { .. }
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
        Some(OverlayState::UserQuestionPrompt { overlay }) => overlay.prompt_placeholder(),
        Some(overlay) if !overlay.accepts_filter_input() => "Overlay open",
        _ => "Type to jump",
    }
}

/// Renders the composer input and returns the cursor row and column.
pub(super) fn multiline_prompt_text(
    input: &str,
    cursor: usize,
    prompt_width: u16,
) -> (Text<'static>, u16, u16) {
    if input.is_empty() {
        let line = Line::from(vec![
            Span::raw("❯ "),
            Span::styled(
                "Review changes, ask a question, or type /",
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]);
        return (Text::from(line), 0, 2);
    }

    let cursor = clamp_char_boundary(input, cursor.min(input.len()));
    let prefix = &input[..cursor];
    let cursor_row = prefix.matches('\n').count() as u16;
    let last_newline = prefix.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let cursor_in_row = cursor.saturating_sub(last_newline);
    let body_width = usize::from(prompt_width.saturating_sub(2));
    let mut cursor_col = 2u16;
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (idx, segment) in input.split('\n').enumerate() {
        let cursor_in_segment = if idx == usize::from(cursor_row) {
            Some(cursor_in_row)
        } else {
            None
        };
        let (body, body_cursor_col) =
            visible_prompt_segment(segment, cursor_in_segment, body_width);
        if cursor_in_segment.is_some() {
            cursor_col = 2 + body_cursor_col;
        }
        if idx == 0 {
            lines.push(Line::from(format!("❯ {body}")));
        } else {
            lines.push(Line::from(format!("  {body}")));
        }
    }
    (Text::from(lines), cursor_row, cursor_col)
}

/// Returns the inline overlay prompt row.
pub(super) fn overlay_prompt_line(input: &str, placeholder: &str) -> Line<'static> {
    if input.is_empty() {
        Line::from(vec![
            Span::raw("❯ "),
            Span::styled(
                placeholder.to_string(),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ])
    } else {
        Line::from(format!("❯ {input}"))
    }
}

/// Returns the hint row for inline overlay interactions.
pub(super) fn overlay_hint_line(
    input: &str,
    onboarding_active: bool,
    overlay: Option<&OverlayState>,
) -> String {
    if overlay
        .is_some_and(|overlay| !overlay.accepts_text_input() && !overlay.accepts_filter_input())
    {
        return "Esc closes · / starts a command".to_string();
    }
    let prefix = if input.is_empty() {
        "Type to jump"
    } else {
        "Typing jumps selection"
    };
    if onboarding_active {
        format!("{prefix} · Enter to continue · Esc to go back")
    } else {
        format!("{prefix} · Enter to select · Esc to close")
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

fn visible_prompt_segment(segment: &str, cursor: Option<usize>, max_width: usize) -> (String, u16) {
    if max_width == 0 {
        return (String::new(), 0);
    }
    let Some(cursor) = cursor else {
        return (take_display_prefix(segment, max_width), 0);
    };
    let cursor = clamp_char_boundary(segment, cursor.min(segment.len()));
    let before_cursor = &segment[..cursor];
    let start = scroll_start_for_cursor(before_cursor, max_width);
    let visible_before_cursor = &segment[start..cursor];
    let cursor_col = UnicodeWidthStr::width(visible_before_cursor).min(max_width) as u16;
    (
        take_display_prefix(&segment[start..], max_width),
        cursor_col,
    )
}

fn scroll_start_for_cursor(before_cursor: &str, max_width: usize) -> usize {
    let mut start = before_cursor.len();
    let mut width = 0usize;
    for (index, ch) in before_cursor.char_indices().rev() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(char_width) > max_width {
            break;
        }
        width = width.saturating_add(char_width);
        start = index;
    }
    start
}

fn take_display_prefix(value: &str, max_width: usize) -> String {
    let mut output = String::new();
    let mut width = 0usize;
    for ch in value.chars() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(char_width) > max_width {
            break;
        }
        width = width.saturating_add(char_width);
        output.push(ch);
    }
    output
}

fn clamp_char_boundary(value: &str, cursor: usize) -> usize {
    let mut index = cursor.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
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
    if popup_accepts_input(input) {
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
        .map(|(index, row)| {
            let selected = index == selected_index;
            command_selection_line(&row.name, &row.description, selected)
        })
        .collect()
}

fn overlay_dropdown_lines(overlay: &OverlayState) -> Vec<Line<'static>> {
    match overlay {
        OverlayState::PermissionPrompt { overlay } => overlay.dropdown_lines(),
        OverlayState::AutoDreamSuggestion {
            skill_name,
            purpose,
            ..
        } => autodream_suggestion_dropdown_lines(overlay, skill_name, purpose),
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
    replace_user_question_footer(&mut lines, &overlay.footer_hint());
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

fn replace_user_question_footer(lines: &mut [Line<'static>], hint: &str) {
    if let Some(line) = lines.last_mut() {
        *line = Line::from(Span::styled(
            hint.to_string(),
            Style::default().add_modifier(Modifier::DIM),
        ));
    }
}

fn autodream_suggestion_dropdown_lines(
    overlay: &OverlayState,
    skill_name: &str,
    purpose: &str,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "AutoDream found a reusable workflow.",
            Style::default().add_modifier(Modifier::DIM),
        )),
        Line::from(Span::styled(
            "Review it before creating a reusable skill draft.",
            Style::default().add_modifier(Modifier::DIM),
        )),
        Line::default(),
        Line::from(Span::styled(
            "Suggested skill:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("- Name: {skill_name}")),
        Line::from(format!("- Purpose: {purpose}")),
        Line::default(),
        Line::from(Span::styled(
            "Actions:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];
    let rows = visible_overlay_rows(
        overlay_rows(overlay),
        overlay_selection(overlay),
        MAX_INLINE_DROPDOWN_ROWS,
    );
    lines.extend(
        rows.into_iter()
            .map(|row| selection_line(row.text, row.selected)),
    );
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Use arrows · Enter to select · Esc to dismiss",
        Style::default().add_modifier(Modifier::DIM),
    )));
    lines
}

fn generic_overlay_dropdown_lines(overlay: &OverlayState) -> Vec<Line<'static>> {
    // A `Line` is one row, so a multi-line title (e.g. a login prompt with a URL
    // on its own line) must become one `Line` per `\n`-separated segment, or the
    // trailing lines (the URL) collapse into the first row and get truncated.
    let mut lines: Vec<Line<'static>> = overlay_title(overlay)
        .split('\n')
        .map(|segment| {
            Line::from(Span::styled(
                segment.to_string(),
                Style::default().add_modifier(Modifier::DIM),
            ))
        })
        .collect();
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
