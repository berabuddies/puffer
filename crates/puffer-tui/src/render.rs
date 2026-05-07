mod composer;
mod helpers;
pub(crate) mod loop_status;
mod overlay_content;
mod overlay_list;
mod panes;
mod summary;
mod tool_messages;
mod top_panel;
use self::composer::{
    composer_area_height, inline_dropdown_height, overlay_prompt_cursor, overlay_prompt_input,
    overlay_prompt_placeholder, overlay_renders_inline_dropdown, prompt_line_count,
    render_inline_dropdown,
};
use self::helpers::{help_pane_active, separator_line};
#[cfg(test)]
use self::overlay_content::render_model_entry;
use self::overlay_content::{masked_secret, overlay_rows, overlay_title, OverlayRow};
use self::overlay_list::{onboarding_fixed_line_count, overlay_selection, visible_overlay_rows};
use self::panes::render_help_pane;
#[cfg(test)]
use self::summary::{footer_lines, header_lines, session_lines};
use self::summary::{footer_status_line, top_panel_height};
use self::tool_messages::render_tool_message;
pub(crate) use self::top_panel::initialize_top_panel_image_state;
use self::top_panel::{render_fixed_top_panel, top_panel_lines};
use crate::approval_overlay::render_permission_overlay;
use crate::btw_overlay::render_btw_overlay;
use crate::markdown::render_markdown;
use crate::session_overlay::render_session_overlay;
use crate::status_overlay::render_status_overlay;
use crate::task_overlay::{is_task_overlay, render_task_overlay};
use crate::text_overlay::render_text_overlay;
use crate::usage::render_usage_overlay;
use crate::OverlayState;
use puffer_core::{AppState, CommandSpec, MessageRole, RenderedMessage, ToolCallRequest};
use puffer_provider_registry::AuthStore;
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use std::cell::RefCell;
use unicode_width::UnicodeWidthStr;
const COMPOSER_SEPARATOR_COLOR: Color = Color::Indexed(214);

#[derive(Default)]
struct PendingSubmitRenderState {
    loading_prompt: Option<String>,
    pending_tool_calls: Vec<ToolCallRequest>,
    queued_prompts: Vec<String>,
    started_at: Option<std::time::Instant>,
    /// True when the model is actively producing thinking/reasoning tokens.
    thinking_active: bool,
    /// Transient status hint (e.g. "Retrying (2/3)…").
    status_hint: Option<String>,
}
thread_local! {
    static ACTIVE_OVERLAY: RefCell<Option<OverlayState>> = const { RefCell::new(None) };
    static ACTIVE_PENDING_SUBMIT: RefCell<PendingSubmitRenderState> = RefCell::new(PendingSubmitRenderState::default());
    static ACTIVE_TOOL_DETAILS_EXPANDED: RefCell<bool> = const { RefCell::new(false) };
    static ACTIVE_FOLLOW_OUTPUT: RefCell<bool> = const { RefCell::new(true) };
    static ACTIVE_TRANSCRIPT_VIEWPORT: RefCell<Option<Rect>> = const { RefCell::new(None) };
    static ACTIVE_LOOP_STATE: RefCell<Option<crate::state::LoopState>> = RefCell::new(None);
    static ACTIVE_STATUS_HINT: RefCell<Option<(String, std::time::Instant)>> = RefCell::new(None);
}

/// Sets the active overlay rendered by the TUI on the next draw.
pub(crate) fn set_active_overlay(overlay: Option<OverlayState>) {
    ACTIVE_OVERLAY.with(|value| *value.borrow_mut() = overlay);
}

/// Sets the pending submit render state for the current frame.
pub(crate) fn set_pending_submit_state(
    loading_prompt: Option<String>,
    pending_tool_calls: Vec<ToolCallRequest>,
    queued_prompts: Vec<String>,
    started_at: Option<std::time::Instant>,
    thinking_active: bool,
    status_hint: Option<String>,
) {
    ACTIVE_PENDING_SUBMIT.with(|value| {
        *value.borrow_mut() = PendingSubmitRenderState {
            loading_prompt,
            pending_tool_calls,
            queued_prompts,
            started_at,
            thinking_active,
            status_hint,
        };
    });
}

/// Sets whether transcript tool messages should render their raw details.
pub(crate) fn set_tool_details_expanded(expanded: bool) {
    ACTIVE_TOOL_DETAILS_EXPANDED.with(|value| *value.borrow_mut() = expanded);
}

/// Sets whether the transcript should stay pinned to the latest output.
pub(crate) fn set_follow_output(follow_output: bool) {
    ACTIVE_FOLLOW_OUTPUT.with(|value| *value.borrow_mut() = follow_output);
}

/// Sets a transient hint shown in the status bar area.
pub(crate) fn set_status_hint(hint: Option<(String, std::time::Instant)>) {
    ACTIVE_STATUS_HINT.with(|value| *value.borrow_mut() = hint);
}

/// Sets the active loop state for the current frame.
pub(crate) fn set_active_loop_state(loop_state: Option<crate::state::LoopState>) {
    ACTIVE_LOOP_STATE.with(|value| *value.borrow_mut() = loop_state);
}

/// Returns the current transcript viewport measured during the last draw.
pub(crate) fn current_transcript_viewport() -> Rect {
    ACTIVE_TRANSCRIPT_VIEWPORT.with(|value| value.borrow().unwrap_or_default())
}

/// Returns the viewport height needed for the current frame.
pub(crate) fn desired_height(
    width: u16,
    height: u16,
    state: &AppState,
    resources: &LoadedResources,
    providers: &puffer_provider_registry::ProviderRegistry,
    auth_store: &AuthStore,
    input: &str,
    commands: &[CommandSpec],
) -> u16 {
    let active_overlay = ACTIVE_OVERLAY.with(|value| value.borrow().clone());
    let help_active = help_pane_active(state, &active_overlay);
    let full_panel_overlay = active_overlay
        .as_ref()
        .is_some_and(|overlay| !overlay_renders_inline_dropdown(overlay));
    let dropdown_height =
        inline_dropdown_height(active_overlay.as_ref(), input, 0, commands, width);
    let prompt_lines = prompt_line_count(input);
    let footer_height = composer_area_height(help_active, dropdown_height, prompt_lines);
    let loop_box_height =
        ACTIVE_LOOP_STATE.with(|value| loop_status::loop_status_height(&value.borrow()));
    let tool_registry = ToolRegistry::from_resources(resources);
    let pending_submit = pending_submit_state();
    let fixed_top_panel = use_fixed_top_panel_surface(state, help_active, &pending_submit);
    let header_height = if fixed_top_panel {
        top_panel_height(
            state,
            resources,
            auth_store,
            &tool_registry,
            providers,
            width,
        )
        .min(
            height
                .saturating_sub(loop_box_height)
                .saturating_sub(footer_height)
                .saturating_sub(1),
        )
    } else {
        0
    };
    let transcript_height = transcript_line_count_with_width(
        width.max(1),
        state,
        resources,
        providers,
        auth_store,
        pending_submit.loading_prompt.is_some() || !pending_submit.queued_prompts.is_empty(),
    )
    .max(1);
    let body_height = if help_active || full_panel_overlay {
        height
            .saturating_sub(header_height)
            .saturating_sub(loop_box_height)
            .saturating_sub(footer_height)
            .max(1)
    } else {
        transcript_height
    };

    header_height
        .saturating_add(body_height)
        .saturating_add(loop_box_height)
        .saturating_add(footer_height)
        .min(height.max(1))
}
/// Renders the current application frame.
pub(crate) fn render(
    frame: &mut Frame<'_>,
    state: &AppState,
    resources: &LoadedResources,
    providers: &puffer_provider_registry::ProviderRegistry,
    auth_store: &AuthStore,
    input: &str,
    cursor: usize,
    slash_selection: usize,
    scroll_offset: u16,
    commands: &[CommandSpec],
) {
    let tool_registry = ToolRegistry::from_resources(resources);
    let active_overlay = ACTIVE_OVERLAY.with(|value| value.borrow().clone());
    let onboarding_active = active_overlay
        .as_ref()
        .map(OverlayState::is_onboarding)
        .unwrap_or(false);
    let help_active = help_pane_active(state, &active_overlay);
    let overlay_active = active_overlay.is_some();
    let dropdown_height = inline_dropdown_height(
        active_overlay.as_ref(),
        input,
        slash_selection,
        commands,
        frame.area().width,
    );
    let custom_status_line = state.config.ui.status_line.as_ref().and_then(|config| {
        state.status_line_text.as_ref().map(|text| {
            let padding = " ".repeat(config.padding as usize);
            format!("{padding}{text}{padding}")
        })
    });
    let prompt_lines = prompt_line_count(input);
    let footer_height = composer_area_height(help_active, dropdown_height, prompt_lines);
    let body_min_height = 1;
    let loop_box_height =
        ACTIVE_LOOP_STATE.with(|value| loop_status::loop_status_height(&value.borrow()));
    let pending_submit = pending_submit_state();
    let fixed_top_panel = use_fixed_top_panel_surface(state, help_active, &pending_submit);
    let header_height = if fixed_top_panel {
        top_panel_height(
            state,
            resources,
            auth_store,
            &tool_registry,
            providers,
            frame.area().width,
        )
        .min(
            frame
                .area()
                .height
                .saturating_sub(footer_height + body_min_height),
        )
    } else {
        0
    };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Min(body_min_height),
            Constraint::Length(loop_box_height),
            Constraint::Length(footer_height),
        ])
        .split(frame.area());
    ACTIVE_TRANSCRIPT_VIEWPORT.with(|value| {
        *value.borrow_mut() = Some(layout[1]);
    });

    if header_height > 0 && fixed_top_panel {
        render_fixed_top_panel(
            frame,
            layout[0],
            state,
            resources,
            auth_store,
            &tool_registry,
            providers,
        );
    }

    if help_active {
        render_help_pane(frame, layout[1], state, commands, resources);
    } else {
        let follow_output = ACTIVE_FOLLOW_OUTPUT.with(|value| *value.borrow());
        let body_scroll_offset = if follow_output {
            transcript_line_count(
                state,
                resources,
                providers,
                auth_store,
                pending_submit.loading_prompt.is_some()
                    || !pending_submit.queued_prompts.is_empty(),
            )
            .saturating_sub(layout[1].height.max(1))
        } else {
            scroll_offset
        };
        frame.render_widget(
            Paragraph::new(transcript_text(
                layout[1].width.max(1),
                state,
                resources,
                providers,
                auth_store,
                &tool_registry,
                pending_submit,
            ))
            .scroll((body_scroll_offset, 0))
            .wrap(Wrap { trim: false }),
            layout[1],
        );
    }

    let mut footer_constraints = vec![Constraint::Length(1), Constraint::Length(prompt_lines)];
    if dropdown_height > 0 {
        footer_constraints.push(Constraint::Length(dropdown_height));
    } else if !help_active {
        footer_constraints.push(Constraint::Length(1));
    }
    let footer = Layout::default()
        .direction(Direction::Vertical)
        .constraints(footer_constraints)
        .split(layout[3]);

    if loop_box_height > 0 {
        ACTIVE_LOOP_STATE.with(|value| {
            if let Some(ref ls) = *value.borrow() {
                loop_status::render_loop_status_box(frame, layout[2], ls);
            }
        });
    }

    frame.render_widget(
        Paragraph::new(separator_line(layout[3].width)).style(
            Style::default()
                .fg(COMPOSER_SEPARATOR_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
        footer[0],
    );

    let prompt_row = footer[1];
    let dropdown_row = if dropdown_height > 0 {
        Some(footer[2])
    } else {
        None
    };
    let hint_row = if dropdown_height > 0 || help_active {
        None
    } else {
        Some(footer[2])
    };
    if overlay_active {
        let overlay_input = overlay_prompt_input(input, active_overlay.as_ref());
        frame.render_widget(
            Paragraph::new(overlay_prompt_line(
                &overlay_input,
                overlay_prompt_placeholder(active_overlay.as_ref()),
            )),
            prompt_row,
        );
        let overlay_cursor = overlay_prompt_cursor(cursor, active_overlay.as_ref());
        let display_cursor = overlay_input
            .get(..overlay_cursor)
            .map_or(0, UnicodeWidthStr::width);
        let max_cursor = usize::from(prompt_row.width.saturating_sub(3));
        let cursor_x = prompt_row.x + 2 + display_cursor.min(max_cursor) as u16;
        let cursor_y = prompt_row.y;
        let buf = frame.area();
        if cursor_x < buf.width && cursor_y < buf.height {
            frame.set_cursor_position((cursor_x, cursor_y));
        }
        if let Some(dropdown_row) = dropdown_row {
            render_inline_dropdown(
                frame,
                dropdown_row,
                active_overlay.as_ref(),
                input,
                slash_selection,
                commands,
            );
        } else if let Some(hint_row) = hint_row {
            frame.render_widget(
                Paragraph::new(overlay_hint_line(input, onboarding_active))
                    .style(Style::default().add_modifier(Modifier::DIM)),
                hint_row,
            );
        }
    } else {
        let (prompt_text, cursor_row, cursor_col) = if help_active && input.is_empty() {
            (
                Text::from(Line::from("❯ /help")),
                0u16,
                "❯ /help".len() as u16,
            )
        } else {
            multiline_prompt_text(input, cursor)
        };
        // When the input is taller than the prompt area, scroll so the cursor
        // line stays visible.
        let scroll = cursor_row.saturating_sub(prompt_lines.saturating_sub(1));
        frame.render_widget(Paragraph::new(prompt_text).scroll((scroll, 0)), prompt_row);
        let max_cursor = usize::from(prompt_row.width.saturating_sub(1));
        let display_cursor = usize::from(cursor_col).min(max_cursor);
        let cursor_x = prompt_row.x + display_cursor as u16;
        let cursor_y = prompt_row.y + cursor_row.saturating_sub(scroll);
        let buf = frame.area();
        if cursor_x < buf.width && cursor_y < buf.height {
            frame.set_cursor_position((cursor_x, cursor_y));
        }

        if let Some(dropdown_row) = dropdown_row {
            render_inline_dropdown(frame, dropdown_row, None, input, slash_selection, commands);
        } else if let Some(hint_row) = hint_row {
            let hint_text = ACTIVE_STATUS_HINT.with(|h| {
                h.borrow()
                    .as_ref()
                    .filter(|(_, t)| t.elapsed().as_secs() < 2)
                    .map(|(text, _)| text.clone())
            });
            if let Some(text) = hint_text {
                frame.render_widget(
                    Paragraph::new(Line::from(text)).style(Style::default().fg(Color::Yellow)),
                    hint_row,
                );
            } else {
                let footer_line = custom_status_line
                    .clone()
                    .unwrap_or_else(|| footer_status_line(state, providers));
                frame.render_widget(
                    Paragraph::new(footer_line).style(Style::default().add_modifier(Modifier::DIM)),
                    hint_row,
                );
            }
        }
    }

    if let Some(overlay) = active_overlay.as_ref() {
        match overlay {
            OverlayState::Help => {}
            OverlayState::Usage(usage) => render_usage_overlay(frame, frame.area(), usage),
            OverlayState::Btw(btw) => render_btw_overlay(frame, frame.area(), btw),
            OverlayState::Session(session) => render_session_overlay(frame, frame.area(), session),
            OverlayState::Status(status) => render_status_overlay(frame, frame.area(), status),
            OverlayState::Text(text) => render_text_overlay(frame, frame.area(), text),
            _ if is_task_overlay(overlay) => {
                render_task_overlay(frame, frame.area(), state, overlay)
            }
            _ if overlay_renders_inline_dropdown(overlay) => {}
            _ => render_overlay(frame, layout[1], overlay),
        }
    }
}

fn transcript_text(
    width: u16,
    state: &AppState,
    resources: &LoadedResources,
    providers: &puffer_provider_registry::ProviderRegistry,
    auth_store: &AuthStore,
    tool_registry: &ToolRegistry,
    pending_submit: PendingSubmitRenderState,
) -> Text<'static> {
    let mut lines = if state.transcript.is_empty() {
        Vec::new()
    } else {
        top_panel_lines(
            width.max(1),
            state,
            resources,
            auth_store,
            tool_registry,
            providers,
        )
    };
    for message in &state.transcript {
        lines.extend(render_transcript_message(message, false));
    }
    if pending_submit.loading_prompt.is_some() || !pending_submit.queued_prompts.is_empty() {
        lines.extend(pending_submit_lines(&pending_submit));
    }
    Text::from(lines)
}

/// Returns true when the current surface should keep the top panel fixed.
fn use_fixed_top_panel_surface(
    state: &AppState,
    help_active: bool,
    pending_submit: &PendingSubmitRenderState,
) -> bool {
    !help_active
        && state.transcript.is_empty()
        && pending_submit.loading_prompt.is_none()
        && pending_submit.queued_prompts.is_empty()
}

pub(crate) fn transcript_line_count(
    state: &AppState,
    resources: &LoadedResources,
    providers: &puffer_provider_registry::ProviderRegistry,
    auth_store: &AuthStore,
    pending_submit: bool,
) -> u16 {
    transcript_line_count_with_width(
        current_transcript_viewport().width.max(1),
        state,
        resources,
        providers,
        auth_store,
        pending_submit,
    )
}

fn transcript_line_count_with_width(
    width: u16,
    state: &AppState,
    resources: &LoadedResources,
    providers: &puffer_provider_registry::ProviderRegistry,
    auth_store: &AuthStore,
    pending_submit: bool,
) -> u16 {
    let pending = if pending_submit {
        pending_submit_state()
    } else {
        PendingSubmitRenderState::default()
    };
    let tool_registry = ToolRegistry::from_resources(resources);
    Paragraph::new(transcript_text(
        width,
        state,
        resources,
        providers,
        auth_store,
        &tool_registry,
        pending,
    ))
    .wrap(Wrap { trim: false })
    .line_count(width)
    .min(u16::MAX as usize) as u16
}

fn render_transcript_message(message: &RenderedMessage, pulse_tool: bool) -> Vec<Line<'static>> {
    let expanded = ACTIVE_TOOL_DETAILS_EXPANDED.with(|value| *value.borrow());
    if message.role == MessageRole::System {
        if let Some(lines) = render_tool_message(&message.text, expanded, pulse_tool) {
            return lines;
        }
    }
    // Structured tool invocations: ToolCall is the input, ToolResult is the output.
    // Merge them into the "Tool <id> [status]" format that render_tool_message expects.
    if message.role == MessageRole::ToolResult {
        if let Some(tool_id) = &message.tool_id {
            let status = if message.success.unwrap_or(true) {
                "ok"
            } else {
                "error"
            };
            let input = message.tool_input.as_deref().unwrap_or("{}");
            let formatted = format!(
                "Tool {} [{}]\ninput: {}\n{}",
                tool_id, status, input, message.text
            );
            if let Some(lines) = render_tool_message(&formatted, expanded, false) {
                return lines;
            }
        }
    }
    // ToolCall input is rendered as part of the ToolResult header — skip it.
    if message.role == MessageRole::ToolCall {
        return Vec::new();
    }
    let (first_prefix, continuation_prefix) = match message.role {
        MessageRole::User => ("› ", "  "),
        MessageRole::Assistant => ("", "  "),
        MessageRole::System | MessageRole::ToolCall | MessageRole::ToolResult => ("· ", "  "),
    };
    let mut lines = Vec::new();
    // Render thinking block before the main text for assistant messages.
    if message.role == MessageRole::Assistant {
        if let Some(thinking) = &message.thinking {
            if !thinking.is_empty() {
                lines.extend(render_thinking_block(thinking));
            }
        }
    }
    let text_lines: Vec<Line<'static>> = render_markdown(&message.text)
        .lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 && !first_prefix.is_empty() {
                let mut spans = vec![Span::raw(first_prefix.to_string())];
                spans.extend(line.spans);
                Line::from(spans)
            } else if index > 0 && !continuation_prefix.is_empty() {
                let mut spans = vec![Span::raw(continuation_prefix.to_string())];
                spans.extend(line.spans);
                Line::from(spans)
            } else {
                line
            }
        })
        .collect();
    // Only append text lines if there is actual text content.
    if !message.text.is_empty() {
        lines.extend(text_lines);
    }
    lines
}

/// Renders a thinking/reasoning block as dimmed indented text with a header.
fn render_thinking_block(thinking: &str) -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let dim_italic = Style::default()
        .add_modifier(Modifier::DIM)
        .add_modifier(Modifier::ITALIC);
    let mut lines = Vec::new();
    // Header line
    lines.push(Line::from(vec![
        Span::styled("  ⎿ ", dim),
        Span::styled("Thinking", dim_italic),
    ]));
    // Thinking content — show each line indented and dimmed
    for text_line in thinking.lines() {
        lines.push(Line::from(vec![
            Span::styled("    ", dim),
            Span::styled(text_line.to_string(), dim),
        ]));
    }
    lines
}

fn pending_submit_state() -> PendingSubmitRenderState {
    ACTIVE_PENDING_SUBMIT.with(|value| PendingSubmitRenderState {
        loading_prompt: value.borrow().loading_prompt.clone(),
        pending_tool_calls: value.borrow().pending_tool_calls.clone(),
        queued_prompts: value.borrow().queued_prompts.clone(),
        started_at: value.borrow().started_at,
        thinking_active: value.borrow().thinking_active,
        status_hint: value.borrow().status_hint.clone(),
    })
}

fn pending_submit_lines(pending_submit: &PendingSubmitRenderState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for tool_call in &pending_submit.pending_tool_calls {
        let placeholder = format!(
            "Tool {} [running]\ninput: {}",
            tool_call.tool_id, tool_call.input
        );
        if let Some(tool_lines) = render_tool_message(
            &placeholder,
            ACTIVE_TOOL_DETAILS_EXPANDED.with(|value| *value.borrow()),
            true,
        ) {
            lines.extend(tool_lines);
        }
    }
    if pending_submit.loading_prompt.is_some() {
        let base_label = if let Some(hint) = &pending_submit.status_hint {
            hint.as_str()
        } else if pending_submit.thinking_active {
            "Thinking..."
        } else {
            "Loading..."
        };
        let label = if let Some(started) = pending_submit.started_at {
            let elapsed = started.elapsed().as_secs_f64();
            format!("{base_label} ({elapsed:.1}s)")
        } else {
            base_label.to_string()
        };
        lines.push(Line::from(vec![
            Span::styled("  ⎿ ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled(label, Style::default().add_modifier(Modifier::DIM)),
        ]));
    }
    for prompt in &pending_submit.queued_prompts {
        lines.push(Line::from(format!("› {prompt}")));
        lines.push(Line::from(vec![
            Span::styled("  ⎿ ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled("Queued", Style::default().add_modifier(Modifier::DIM)),
        ]));
    }
    lines
}

/// Renders the composer input as a possibly multi-line `Text`, and returns
/// the cursor's row and column inside that rendered block. The first line is
/// prefixed with `"❯ "`; continuation lines are indented two spaces so the
/// text aligns under the prompt arrow.
fn multiline_prompt_text(input: &str, cursor: usize) -> (Text<'static>, u16, u16) {
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

    let cursor = cursor.min(input.len());
    let prefix = &input[..cursor];
    let cursor_row = prefix.matches('\n').count() as u16;
    let last_newline = prefix.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let row_prefix = &prefix[last_newline..];
    // Both the first line ("❯ ") and continuation lines ("  ") are 2 cols wide.
    let cursor_col = (UnicodeWidthStr::width(row_prefix) + 2) as u16;

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (idx, segment) in input.split('\n').enumerate() {
        let body = segment.to_string();
        if idx == 0 {
            lines.push(Line::from(format!("❯ {body}")));
        } else {
            lines.push(Line::from(format!("  {body}")));
        }
    }
    (Text::from(lines), cursor_row, cursor_col)
}

fn overlay_prompt_line(input: &str, placeholder: &str) -> Line<'static> {
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

fn overlay_hint_line(input: &str, onboarding_active: bool) -> String {
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

fn render_overlay(frame: &mut Frame<'_>, viewport: Rect, overlay: &OverlayState) {
    if is_onboarding_overlay(overlay) {
        render_onboarding_overlay(frame, viewport, overlay);
        return;
    }
    if let OverlayState::PermissionPrompt { overlay } = overlay {
        render_permission_overlay(frame, viewport, overlay);
        return;
    }
    if matches!(overlay, OverlayState::LoginPicker { .. }) {
        render_login_overlay(frame, viewport, overlay);
        return;
    }
    let width = viewport.width.saturating_sub(8).min(72);
    let max_height = viewport.height.saturating_sub(2).max(3);
    let rows = visible_overlay_rows(
        overlay_rows(overlay),
        overlay_selection(overlay),
        usize::from(max_height.saturating_sub(2)),
    )
    .into_iter()
    .map(|row| {
        ListItem::new(row.text).style(if row.selected {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        })
    })
    .collect::<Vec<_>>();
    let height = rows.len() as u16 + 2;
    let area = Rect {
        x: viewport.x + viewport.width.saturating_sub(width) / 2,
        y: viewport.y + viewport.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        List::new(rows).block(
            Block::default()
                .title(overlay_title(overlay))
                .borders(Borders::ALL)
                .border_set(border::ROUNDED)
                .border_style(accent_border_style()),
        ),
        area,
    );
}

fn render_login_overlay(frame: &mut Frame<'_>, viewport: Rect, overlay: &OverlayState) {
    let rows = overlay_rows(overlay)
        .into_iter()
        .map(|row| {
            ListItem::new(row.text).style(if row.selected {
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            })
        })
        .collect::<Vec<_>>();
    let width = viewport.width.saturating_sub(8).min(72);
    let height = rows.len() as u16 + 4;
    let area = Rect {
        x: viewport.x + viewport.width.saturating_sub(width) / 2,
        y: viewport.y + viewport.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let block = Block::default()
        .title(" Login ")
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(accent_border_style());
    frame.render_widget(Clear, area);
    frame.render_widget(&block, area);
    let inner = block.inner(area);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    frame.render_widget(
        Paragraph::new("Select Provider").style(Style::default().add_modifier(Modifier::DIM)),
        sections[0],
    );
    frame.render_widget(List::new(rows), sections[1]);
}

fn accent_border_style() -> Style {
    Style::default().fg(Color::Cyan)
}

pub(super) fn prompt_border_style(state: &AppState) -> Style {
    let color = match state.prompt_color.as_str() {
        "amber" | "yellow" => Color::Yellow,
        "teal" | "cyan" => Color::Cyan,
        "green" => Color::Green,
        "blue" => Color::Blue,
        "red" => Color::Red,
        "magenta" | "pink" => Color::Magenta,
        _ => Color::Cyan,
    };
    Style::default().fg(color)
}

fn is_onboarding_overlay(overlay: &OverlayState) -> bool {
    overlay.is_onboarding()
}

fn render_onboarding_overlay(frame: &mut Frame<'_>, viewport: Rect, overlay: &OverlayState) {
    let max_height = viewport.height.saturating_sub(2).max(8);
    let max_rows =
        usize::from(max_height.saturating_sub((onboarding_fixed_line_count(overlay) + 2) as u16))
            .max(1);
    let body_lines = onboarding_body_lines(overlay, max_rows);
    let width = viewport.width.saturating_sub(12).min(76).max(34);
    let height = (body_lines.len() as u16 + 2).min(max_height);
    let area = Rect {
        x: viewport.x + viewport.width.saturating_sub(width) / 2,
        y: viewport.y + viewport.height.saturating_sub(height) / 3,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(Text::from(body_lines))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(accent_border_style()),
            ),
        area,
    );
    if let OverlayState::ApiKeyPrompt { value, cursor, .. } = overlay {
        let display_cursor = value.get(..*cursor).map_or(0, UnicodeWidthStr::width);
        let cursor_x = area.x + 2 + (display_cursor as u16).min(area.width.saturating_sub(4));
        let cursor_y = area.y + height.saturating_sub(3);
        frame.set_cursor_position((cursor_x, cursor_y));
        if value.is_empty() && area.width > 6 {
            frame.render_widget(
                Paragraph::new("Paste code here if prompted >")
                    .style(Style::default().add_modifier(Modifier::DIM)),
                Rect {
                    x: area.x + 2,
                    y: cursor_y,
                    width: area.width.saturating_sub(4),
                    height: 1,
                },
            );
        }
    }
}

fn onboarding_body_lines(overlay: &OverlayState, max_rows: usize) -> Vec<Line<'static>> {
    let (title, subtitle, note, rows, footer) = match overlay {
        OverlayState::ThemePicker { .. } => (
            "Let's get started.",
            "Choose the text style that looks best with your terminal.",
            Some("You can run /theme later."),
            overlay_rows(overlay),
            "Enter to continue",
        ),
        OverlayState::ProviderPicker { .. } => (
            "Choose a provider.",
            "Pick where Puffer should sign in and load models from.",
            None,
            overlay_rows(overlay),
            "Enter to continue",
        ),
        OverlayState::AuthPicker { provider_id, .. } => (
            "Select login method.",
            "Choose how to connect your account for this provider.",
            Some(provider_id.as_str()),
            overlay_rows(overlay),
            "Enter to continue · Esc to go back",
        ),
        OverlayState::ModelPicker {
            provider_id,
            onboarding: true,
            ..
        } => (
            "Select model.",
            "Only models from the selected provider are shown here.",
            Some(provider_id.as_str()),
            overlay_rows(overlay),
            "Enter to confirm · Esc to go back",
        ),
        OverlayState::EffortPicker {
            provider_id,
            onboarding: true,
            ..
        } => (
            "Select effort.",
            "Choose the provider-specific reasoning level.",
            Some(provider_id.as_str()),
            overlay_rows(overlay),
            "Enter to continue · Esc to go back",
        ),
        OverlayState::FastModePicker {
            provider_id,
            onboarding: true,
            ..
        } => (
            "Select fast mode.",
            "Choose whether to enable the provider-specific fast mode.",
            Some(provider_id.as_str()),
            overlay_rows(overlay),
            "Enter to confirm · Esc to go back",
        ),
        OverlayState::ApiKeyPrompt {
            provider_id, value, ..
        } => {
            let key_line = format!("> {}", masked_secret(value));
            return vec![
                Line::from(Span::styled(
                    "Let's get started.",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::default(),
                Line::from("Paste an API key."),
                Line::from(Span::styled(
                    format!("{provider_id} will use this key for API requests."),
                    Style::default().add_modifier(Modifier::DIM),
                )),
                Line::default(),
                Line::from(key_line),
                Line::default(),
                Line::from(Span::styled(
                    "Enter to continue · Esc to go back",
                    Style::default().add_modifier(Modifier::DIM),
                )),
            ];
        }
        _ => return Vec::new(),
    };

    let mut lines = vec![
        Line::from(Span::styled(
            "Puffer Code",
            Style::default().add_modifier(Modifier::DIM),
        )),
        Line::default(),
        Line::from(Span::styled(
            title.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            subtitle.to_string(),
            Style::default().add_modifier(Modifier::DIM),
        )),
    ];
    if let Some(note) = note {
        lines.push(Line::from(Span::styled(
            note.to_string(),
            Style::default().add_modifier(Modifier::DIM),
        )));
    }
    lines.push(Line::default());
    for row in visible_overlay_rows(rows, overlay_selection(overlay), max_rows) {
        let marker = if row.selected { "› " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(
                marker.to_string(),
                if row.selected {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ),
            Span::styled(
                row.text,
                if row.selected {
                    Style::default()
                        .add_modifier(Modifier::BOLD)
                        .add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                },
            ),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        footer.to_string(),
        Style::default().add_modifier(Modifier::DIM),
    )));
    lines
}

#[cfg(test)]
mod overlay_tests;
#[cfg(test)]
mod scroll_tests;
#[cfg(test)]
mod tests;
