mod app_helpers;
mod approval_overlay;
mod btw_overlay;
mod flow;
mod key_handler;
mod list_selection_view;
mod markdown;
mod markdown_render;
#[path = "onboarding/mod.rs"]
mod onboarding;
mod permission_prompt_flow;
mod popup;
mod prompt_history_store;
mod render;
mod session_overlay;
#[path = "state.rs"]
mod state;
mod status_overlay;
mod statusline;
mod task_overlay;
mod task_panels;
mod text_overlay;
mod usage;
use crate::flow::{
    advance_loop_after_turn, allow_prompt_before_onboarding, check_loop_interval, handle_submit,
    poll_pending_submit, submit_next_queued_prompt, submit_queued_prompt_if_ready,
    try_open_overlay,
};
use crate::key_handler::handle_key;
use crate::prompt_history_store::PromptHistoryStore;
use crate::render::initialize_top_panel_image_state;
use crate::statusline::refresh_status_line;
use anyhow::Result;
use app_helpers::should_use_inline_viewport;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as terminal_size, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use puffer_core::{command_surface, shutdown_runtime_services, AppState};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};
#[cfg(test)]
pub(crate) use state::AuthPickerAction;
use state::TuiState;
pub(crate) use state::{ModelPickerEntry, OverlayState};
use std::io::{self, IsTerminal};
use std::path::Path;
use std::time::Duration;
/// Runs the interactive Puffer TUI until the user exits.
pub fn run_app(
    state: &mut AppState,
    resources: &mut LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    initial_prompt: Option<String>,
    no_alt_screen: bool,
) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        if let Some(prompt) = initial_prompt.filter(|prompt| !prompt.trim().is_empty()) {
            handle_submit(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                prompt,
                no_alt_screen,
            )?;
            return Ok(());
        }
        anyhow::bail!("interactive TUI requires stdin and stdout to be terminals");
    }

    let bypass_onboarding = initial_prompt
        .as_deref()
        .map(allow_prompt_before_onboarding)
        .unwrap_or(false);
    let use_content_viewport = should_use_inline_viewport(no_alt_screen);
    let no_alt_screen = use_content_viewport;
    enable_raw_mode()?;
    if !no_alt_screen {
        execute!(io::stdout(), EnterAlternateScreen)?;
    }
    let mut tui = TuiState::default();
    if let Err(error) =
        tui.configure_prompt_history_store(PromptHistoryStore::from_auth_path(auth_path))
    {
        tui.status_hint = Some((
            format!("Prompt history unavailable: {error}"),
            std::time::Instant::now(),
        ));
    }
    initialize_top_panel_image_state();
    tui.defer_prompt(
        initial_prompt
            .clone()
            .filter(|prompt| !prompt.trim().is_empty())
            .filter(|_| !bypass_onboarding),
    );
    let commands = command_surface(resources);
    if let Some(prompt) = initial_prompt.filter(|prompt| allow_prompt_before_onboarding(prompt)) {
        if !try_open_overlay(
            state,
            resources,
            providers,
            auth_store,
            session_store,
            &mut tui,
            &prompt,
        )? {
            handle_submit(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                prompt,
                no_alt_screen,
            )?;
        }
    }
    if !bypass_onboarding {
        submit_queued_prompt_if_ready(
            state,
            resources,
            providers,
            auth_store,
            auth_path,
            session_store,
            &mut tui,
            no_alt_screen,
        )?;
    }

    sync_render_state(&tui);
    let mut viewport_height = if use_content_viewport {
        let (width, height) = terminal_size()?;
        render::desired_height(
            width, height, state, resources, providers, auth_store, &tui.input, &commands,
        )
        .max(1)
    } else {
        0
    };
    let mut terminal = if use_content_viewport {
        Terminal::with_options(
            CrosstermBackend::new(io::stdout()),
            TerminalOptions {
                viewport: Viewport::Inline(viewport_height),
            },
        )?
    } else {
        Terminal::new(CrosstermBackend::new(io::stdout()))?
    };

    loop {
        if poll_pending_submit(state, auth_store, auth_path, session_store, &mut tui)? {
            advance_loop_after_turn(state, session_store, &mut tui)?;
            submit_queued_prompt_if_ready(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                &mut tui,
                no_alt_screen,
            )?;
            submit_next_queued_prompt(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                &mut tui,
                no_alt_screen,
            )?;
        }
        check_loop_interval(&mut tui);
        if !tui.has_pending_submit() && !tui.queued_prompts.is_empty() {
            submit_next_queued_prompt(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                &mut tui,
                no_alt_screen,
            )?;
        }
        refresh_status_line(state)?;
        sync_render_state(&tui);
        if use_content_viewport {
            let size = terminal.size()?;
            let next_height = render::desired_height(
                size.width,
                size.height,
                state,
                resources,
                providers,
                auth_store,
                &tui.input,
                &commands,
            )
            .max(1);
            if next_height != viewport_height {
                terminal.clear()?;
                terminal = Terminal::with_options(
                    CrosstermBackend::new(io::stdout()),
                    TerminalOptions {
                        viewport: Viewport::Inline(next_height),
                    },
                )?;
                viewport_height = next_height;
            } else {
                terminal.autoresize()?;
            }
        }
        terminal.draw(|frame| {
            render::render(
                frame,
                state,
                resources,
                providers,
                auth_store,
                &tui.input,
                tui.cursor,
                tui.slash_selection,
                tui.scroll_offset,
                &commands,
            )
        })?;
        if state.should_exit {
            break;
        }
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if handle_key(
                        key,
                        state,
                        resources,
                        providers,
                        auth_store,
                        auth_path,
                        session_store,
                        &commands,
                        &mut tui,
                        no_alt_screen,
                    )? {
                        break;
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }

    let _ = shutdown_runtime_services();
    disable_raw_mode()?;
    if !no_alt_screen {
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    }
    Ok(())
}

fn sync_render_state(tui: &TuiState) {
    render::set_active_overlay(tui.overlay.clone());
    render::set_pending_submit_state(
        tui.pending_submit
            .as_ref()
            .map(|pending| pending.prompt.clone()),
        tui.pending_submit
            .as_ref()
            .map(|pending| pending.pending_tool_calls.clone())
            .unwrap_or_default(),
        tui.queued_prompts.iter().cloned().collect(),
    );
    render::set_tool_details_expanded(tui.tool_details_expanded);
    render::set_follow_output(tui.follow_output);
    render::set_active_loop_state(tui.active_loop.clone());
    render::set_status_hint(tui.status_hint.clone());
}
#[cfg(test)]
mod tests;
