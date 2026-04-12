mod app_helpers;
mod approval_overlay;
mod btw_overlay;
mod flow;
mod list_selection_view;
mod markdown;
mod markdown_render;
#[path = "onboarding/mod.rs"]
mod onboarding;
mod permission_prompt_flow;
mod popup;
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
    advance_loop_after_turn, allow_prompt_before_onboarding, apply_selected_provider,
    builtin_openai_base_url, builtin_openai_headers, builtin_openai_query_params,
    cancel_pending_submit, check_loop_interval, emit_system_message, handle_prompt_submit,
    handle_submit, persist_user_config, poll_pending_submit, run_embedded_auth_login,
    set_overlay_state, submit_next_queued_prompt, submit_queued_prompt_if_ready, try_open_overlay,
};
use crate::permission_prompt_flow::handle_permission_prompt_key;
use crate::render::initialize_top_panel_image_state;
use crate::statusline::refresh_status_line;
use anyhow::Result;
use app_helpers::{
    apply_model_selection_preferences, apply_selected_model, help_pane_active_without_overlay,
    should_use_inline_viewport,
};
use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as terminal_size, Clear, ClearType,
    EnterAlternateScreen, LeaveAlternateScreen,
};
use puffer_core::{command_surface, shutdown_runtime_services, AppState, CommandSpec};
use puffer_provider_registry::{AuthStore, ProviderRegistry, StoredCredential};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};
use state::TuiState;
pub(crate) use state::{AuthPickerAction, ModelPickerEntry, OverlayState};
use std::io::{self, IsTerminal};
use std::path::Path;
use std::time::Duration;

/// Configures launch-time TUI behavior before the first render.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum StartupAction {
    /// Start a fresh TUI session with no extra overlay.
    #[default]
    None,
    /// Open the `/resume` picker on launch and optionally seed its search query.
    ResumePicker { query: Option<String> },
}

/// Runs the interactive Puffer TUI until the user exits.
pub fn run_app(
    state: &mut AppState,
    resources: &mut LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    startup_action: StartupAction,
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
    if use_content_viewport {
        execute!(io::stdout(), MoveTo(0, 0), Clear(ClearType::All))?;
    } else {
        execute!(io::stdout(), EnterAlternateScreen)?;
    }
    let mut tui = TuiState::default();
    initialize_top_panel_image_state();
    tui.defer_prompt(
        initial_prompt
            .clone()
            .filter(|prompt| !prompt.trim().is_empty())
            .filter(|_| !bypass_onboarding),
    );
    apply_startup_action(
        state,
        resources,
        providers,
        auth_store,
        session_store,
        &startup_action,
        &mut tui,
    )?;
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
        // Auto-wake: when the agent is idle and background tasks have completed,
        // enqueue a synthetic notification so the agent sees the completion.
        if !tui.has_pending_submit() && tui.queued_prompts.is_empty() {
            let completed = puffer_core::drain_background_task_completions(state);
            if !completed.is_empty() {
                let notice = format!(
                    "<task-notification>\n{}\nUse TaskOutput to retrieve the full output if needed.\n</task-notification>",
                    completed.join("\n")
                );
                tui.enqueue_prompt(notice);
            }
        }
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
        refresh_status_line(state, providers)?;
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
        if event::poll(Duration::from_millis(16))? {
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

fn apply_startup_action(
    state: &AppState,
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &AuthStore,
    session_store: &SessionStore,
    startup_action: &StartupAction,
    tui: &mut TuiState,
) -> Result<()> {
    let StartupAction::ResumePicker { query } = startup_action else {
        return Ok(());
    };
    if try_open_overlay(
        state,
        resources,
        providers,
        auth_store,
        session_store,
        tui,
        "/resume",
    )? {
        tui.input = query.clone().unwrap_or_default();
        tui.cursor = tui.input.len();
        if let Some(overlay) = tui.overlay.as_mut() {
            overlay.select_matching_query(&tui.input);
        }
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
        tui.pending_submit
            .as_ref()
            .map(|pending| pending.started_at),
    );
    render::set_tool_details_expanded(tui.tool_details_expanded);
    render::set_follow_output(tui.follow_output);
    render::set_active_loop_state(tui.active_loop.clone());
    render::set_status_hint(tui.status_hint.clone());
}

fn handle_key(
    key: KeyEvent,
    state: &mut AppState,
    resources: &mut LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    commands: &[CommandSpec],
    tui: &mut TuiState,
    no_alt_screen: bool,
) -> Result<bool> {
    if tui.overlay.is_some() {
        return handle_overlay_key(
            key,
            state,
            resources,
            providers,
            auth_store,
            auth_path,
            session_store,
            tui,
            no_alt_screen,
        );
    }

    match key.code {
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            tui.tool_details_expanded = !tui.tool_details_expanded;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if tui.active_loop.is_some() {
                cancel_pending_submit(state, session_store, tui)?;
                tui.active_loop = None;
                tui.queued_prompts.clear();
                tui.status_hint = Some(("Loop stopped.".into(), std::time::Instant::now()));
            } else if tui.has_pending_submit() {
                cancel_pending_submit(state, session_store, tui)?;
                tui.status_hint = Some((
                    "Interrupted. Press Ctrl+C again to exit.".into(),
                    std::time::Instant::now(),
                ));
                tui.last_ctrl_c = Some(std::time::Instant::now());
            } else if tui.should_exit_on_ctrl_c() {
                state.should_exit = true;
                return Ok(true);
            } else {
                tui.status_hint = Some((
                    "Press Ctrl+C again to exit.".into(),
                    std::time::Instant::now(),
                ));
            }
            return Ok(false);
        }
        KeyCode::Esc => {
            if cancel_pending_submit(state, session_store, tui)? {
                submit_next_queued_prompt(
                    state,
                    resources,
                    providers,
                    auth_store,
                    auth_path,
                    session_store,
                    tui,
                    no_alt_screen,
                )?;
            } else if help_pane_active_without_overlay(state, tui) {
                session_store.append_transcript_pop_last(state.session.id, 1)?;
                state.apply_transcript_rewrite(&puffer_session_store::TranscriptRewrite::PopLast {
                    count: 1,
                });
                tui.clear(commands);
            } else {
                tui.clear(commands);
            }
        }
        KeyCode::Left => tui.move_left(),
        KeyCode::Right => tui.move_right(),
        KeyCode::Home => tui.move_home(),
        KeyCode::End => tui.move_end(),
        KeyCode::Up => {
            if tui.input.starts_with('/') {
                tui.select_previous(commands)
            } else {
                let viewport = render::current_transcript_viewport();
                tui.scroll_up(
                    1,
                    render::transcript_line_count(
                        state,
                        resources,
                        auth_store,
                        tui.has_pending_submit(),
                    ),
                    viewport.height,
                );
            }
        }
        KeyCode::Down => {
            if tui.input.starts_with('/') {
                tui.select_next(commands)
            } else {
                let viewport = render::current_transcript_viewport();
                tui.scroll_down(
                    1,
                    render::transcript_line_count(
                        state,
                        resources,
                        auth_store,
                        tui.has_pending_submit(),
                    ),
                    viewport.height,
                );
            }
        }
        KeyCode::PageUp => {
            if tui.input.starts_with('/') {
                for _ in 0..10 {
                    tui.select_previous(commands);
                }
            } else {
                let viewport = render::current_transcript_viewport();
                tui.scroll_up(
                    10,
                    render::transcript_line_count(
                        state,
                        resources,
                        auth_store,
                        tui.has_pending_submit(),
                    ),
                    viewport.height,
                );
            }
        }
        KeyCode::PageDown => {
            if tui.input.starts_with('/') {
                for _ in 0..10 {
                    tui.select_next(commands);
                }
            } else {
                let viewport = render::current_transcript_viewport();
                tui.scroll_down(
                    10,
                    render::transcript_line_count(
                        state,
                        resources,
                        auth_store,
                        tui.has_pending_submit(),
                    ),
                    viewport.height,
                );
            }
        }
        KeyCode::Backspace => tui.backspace(commands),
        KeyCode::Delete => tui.delete(commands),
        KeyCode::Tab => {
            let _ = tui.apply_selected_command(commands);
        }
        KeyCode::Enter => {
            if tui.complete_on_enter(commands) {
                return Ok(false);
            }
            let current_input = tui.input.clone();
            if try_open_overlay(
                state,
                resources,
                providers,
                auth_store,
                session_store,
                tui,
                &current_input,
            )? {
                return Ok(false);
            }
            let submitted = tui.take_input();
            handle_prompt_submit(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                tui,
                submitted,
                no_alt_screen,
            )?;
            submit_queued_prompt_if_ready(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                tui,
                no_alt_screen,
            )?;
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            tui.insert_char(ch, commands)
        }
        _ => {}
    }
    Ok(false)
}

fn handle_overlay_key(
    key: KeyEvent,
    state: &mut AppState,
    resources: &mut LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    tui: &mut TuiState,
    no_alt_screen: bool,
) -> Result<bool> {
    if matches!(
        tui.overlay.as_ref(),
        Some(OverlayState::PermissionPrompt { .. })
    ) && handle_permission_prompt_key(key, tui)
    {
        return Ok(false);
    }

    let Some(active_overlay) = tui.overlay.as_ref() else {
        return Ok(false);
    };

    if matches!(active_overlay, OverlayState::Btw(..)) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ') => {
                set_overlay_state(tui, None);
            }
            KeyCode::Up => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.select_previous();
                }
            }
            KeyCode::Down => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.select_next();
                }
            }
            KeyCode::PageUp => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.page_up();
                }
            }
            KeyCode::PageDown => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.page_down();
                }
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.select_previous();
                }
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.select_next();
                }
            }
            KeyCode::Char('c') | KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                set_overlay_state(tui, None);
            }
            _ => {}
        }
        return Ok(false);
    }

    if active_overlay.accepts_text_input() {
        match key.code {
            KeyCode::Esc => {
                let next = onboarding::back_overlay(active_overlay, providers, auth_store)?;
                set_overlay_state(tui, next);
            }
            KeyCode::Left => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.move_left();
                }
            }
            KeyCode::Right => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.move_right();
                }
            }
            KeyCode::Home => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.move_home();
                }
            }
            KeyCode::End => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.move_end();
                }
            }
            KeyCode::Backspace => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.backspace();
                }
            }
            KeyCode::Delete => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.delete();
                }
            }
            KeyCode::Enter => {
                let Some(provider_id) = active_overlay.selected_provider().map(str::to_string)
                else {
                    set_overlay_state(tui, None);
                    return Ok(false);
                };
                let key_value = active_overlay
                    .api_key_value()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if key_value.is_empty() {
                    let next = onboarding::back_overlay(active_overlay, providers, auth_store)?;
                    set_overlay_state(tui, next);
                    emit_system_message(
                        state,
                        session_store,
                        format!("No API key was entered for {provider_id}."),
                    )?;
                    return Ok(false);
                }
                auth_store.set_api_key(provider_id.clone(), key_value);
                auth_store.save(auth_path)?;
                let next = onboarding::provider_setup_overlay(providers, auth_store, &provider_id)?;
                set_overlay_state(tui, next);
                submit_queued_prompt_if_ready(
                    state,
                    resources,
                    providers,
                    auth_store,
                    auth_path,
                    session_store,
                    tui,
                    no_alt_screen,
                )?;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if tui.should_exit_on_ctrl_c() {
                    state.should_exit = true;
                    return Ok(true);
                }
                emit_system_message(
                    state,
                    session_store,
                    "Press Ctrl+C again to exit.".to_string(),
                )?;
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(overlay) = tui.overlay.as_mut() {
                    overlay.insert_char(ch);
                }
            }
            _ => {}
        }
        return Ok(false);
    }

    let overlay_snapshot = active_overlay.clone();
    match key.code {
        KeyCode::Esc => {
            set_overlay_state(
                tui,
                onboarding::back_overlay(&overlay_snapshot, providers, auth_store)?,
            );
        }
        KeyCode::Up => {
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.select_previous();
            }
        }
        KeyCode::Down => {
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.select_next();
            }
        }
        KeyCode::PageUp => {
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.page_up();
            }
        }
        KeyCode::PageDown => {
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.page_down();
            }
        }
        KeyCode::Backspace => {
            let commands = command_surface(resources);
            tui.backspace(&commands);
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.select_matching_query(&tui.input);
            }
        }
        KeyCode::Delete => {
            let commands = command_surface(resources);
            tui.delete(&commands);
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.select_matching_query(&tui.input);
            }
        }
        KeyCode::Enter => {
            if matches!(overlay_snapshot, OverlayState::ThemePicker { .. })
                && onboarding::initial_overlay(state, providers, auth_store)?.is_some()
            {
                let Some(command) = overlay_snapshot.selected_command() else {
                    set_overlay_state(tui, None);
                    return Ok(false);
                };
                handle_submit(
                    state,
                    resources,
                    providers,
                    auth_store,
                    auth_path,
                    session_store,
                    command,
                    no_alt_screen,
                )?;
                set_overlay_state(tui, onboarding::initial_provider_overlay(providers));
                submit_queued_prompt_if_ready(
                    state,
                    resources,
                    providers,
                    auth_store,
                    auth_path,
                    session_store,
                    tui,
                    no_alt_screen,
                )?;
                return Ok(false);
            }
            if let Some(provider_id) = overlay_snapshot.selected_provider().map(str::to_string) {
                match &overlay_snapshot {
                    OverlayState::ProviderPicker { .. } | OverlayState::LoginPicker { .. } => {
                        apply_selected_provider(state, &provider_id)?;
                        let next = onboarding::provider_setup_overlay(
                            providers,
                            auth_store,
                            &provider_id,
                        )?;
                        set_overlay_state(tui, next);
                    }
                    OverlayState::AuthPicker { .. } => {
                        let Some(action) = overlay_snapshot.selected_auth_action().cloned() else {
                            set_overlay_state(tui, None);
                            return Ok(false);
                        };
                        apply_selected_provider(state, &provider_id)?;
                        match action {
                            AuthPickerAction::OAuth => {
                                match run_embedded_auth_login(
                                    state,
                                    &provider_id,
                                    auth_store,
                                    no_alt_screen,
                                ) {
                                    Ok(message) => {
                                        let next = onboarding::provider_setup_overlay(
                                            providers,
                                            auth_store,
                                            &provider_id,
                                        )?;
                                        set_overlay_state(tui, next);
                                        emit_system_message(state, session_store, message)?;
                                    }
                                    Err(error) => {
                                        let next = onboarding::back_overlay(
                                            &overlay_snapshot,
                                            providers,
                                            auth_store,
                                        )?;
                                        set_overlay_state(tui, next);
                                        emit_system_message(
                                            state,
                                            session_store,
                                            format!("Login failed for {provider_id}: {error}"),
                                        )?;
                                    }
                                }
                            }
                            AuthPickerAction::ApiKey => {
                                set_overlay_state(
                                    tui,
                                    Some(OverlayState::ApiKeyPrompt {
                                        provider_id,
                                        value: String::new(),
                                        cursor: 0,
                                        onboarding: overlay_snapshot.is_onboarding(),
                                    }),
                                );
                            }
                            AuthPickerAction::Import(candidate) => {
                                let imported_openai_base_url = candidate.openai_base_url.clone();
                                let imported_openai_headers = candidate.openai_headers.clone();
                                let imported_openai_query_params =
                                    candidate.openai_query_params.clone();
                                match candidate.credential {
                                    StoredCredential::ApiKey { key } => {
                                        auth_store.set_api_key(provider_id.clone(), key);
                                    }
                                    StoredCredential::OAuth(credential) => {
                                        auth_store.set_oauth(provider_id.clone(), credential);
                                    }
                                }
                                if provider_id == "openai" {
                                    state.config.openai_base_url = imported_openai_base_url;
                                    state.config.openai_headers = imported_openai_headers;
                                    state.config.openai_query_params = imported_openai_query_params;
                                    persist_user_config(state)?;
                                    let base_url = state
                                        .config
                                        .openai_base_url
                                        .clone()
                                        .or_else(|| builtin_openai_base_url(resources));
                                    if let Some(base_url) = base_url {
                                        providers.set_openai_base_url(base_url);
                                    }
                                    let headers = if state.config.openai_headers.is_empty() {
                                        builtin_openai_headers(resources)
                                    } else {
                                        state
                                            .config
                                            .openai_headers
                                            .clone()
                                            .into_iter()
                                            .collect::<indexmap::IndexMap<_, _>>()
                                    };
                                    providers.set_openai_headers(headers);
                                    let query_params =
                                        if state.config.openai_query_params.is_empty() {
                                            builtin_openai_query_params(resources)
                                        } else {
                                            state
                                                .config
                                                .openai_query_params
                                                .clone()
                                                .into_iter()
                                                .collect::<indexmap::IndexMap<_, _>>()
                                        };
                                    providers.set_openai_query_params(query_params);
                                }
                                auth_store.save(auth_path)?;
                                let next = onboarding::provider_setup_overlay(
                                    providers,
                                    auth_store,
                                    &provider_id,
                                )?;
                                set_overlay_state(tui, next);
                            }
                            AuthPickerAction::UseStored | AuthPickerAction::NoneRequired => {
                                let next = onboarding::provider_setup_overlay(
                                    providers,
                                    auth_store,
                                    &provider_id,
                                )?;
                                set_overlay_state(tui, next);
                            }
                        }
                    }
                    OverlayState::ModelPicker { onboarding, .. } => {
                        let Some(model_id) = overlay_snapshot.selected_model().map(str::to_string)
                        else {
                            set_overlay_state(tui, None);
                            return Ok(false);
                        };
                        if *onboarding {
                            set_overlay_state(
                                tui,
                                Some(onboarding::effort_picker(
                                    providers,
                                    &provider_id,
                                    &model_id,
                                    &state.effort_level,
                                    true,
                                )),
                            );
                        } else {
                            set_overlay_state(tui, None);
                            apply_selected_model(state, session_store, &provider_id, &model_id)?;
                        }
                    }
                    OverlayState::EffortPicker {
                        provider_id,
                        model_id,
                        onboarding,
                        ..
                    } => {
                        let Some(effort) = overlay_snapshot.selected_model().map(str::to_string)
                        else {
                            set_overlay_state(tui, None);
                            return Ok(false);
                        };
                        if *onboarding {
                            set_overlay_state(
                                tui,
                                Some(onboarding::fast_mode_picker(
                                    providers,
                                    provider_id,
                                    model_id,
                                    &effort,
                                    state.fast_mode,
                                    true,
                                )),
                            );
                        } else {
                            set_overlay_state(tui, None);
                            apply_model_selection_preferences(
                                state,
                                auth_store,
                                auth_path,
                                session_store,
                                provider_id,
                                model_id,
                                &effort,
                                state.fast_mode,
                            )?;
                        }
                    }
                    OverlayState::FastModePicker {
                        provider_id,
                        model_id,
                        effort,
                        ..
                    } => {
                        let fast_mode =
                            matches!(overlay_snapshot.selected_model(), Some("on" | "true" | "1"));
                        set_overlay_state(tui, None);
                        apply_model_selection_preferences(
                            state,
                            auth_store,
                            auth_path,
                            session_store,
                            &provider_id,
                            &model_id,
                            &effort,
                            fast_mode,
                        )?;
                    }
                    _ => {
                        if let Some(command) = overlay_snapshot.selected_command() {
                            set_overlay_state(tui, None);
                            handle_submit(
                                state,
                                resources,
                                providers,
                                auth_store,
                                auth_path,
                                session_store,
                                command,
                                no_alt_screen,
                            )?;
                        } else {
                            set_overlay_state(tui, None);
                        }
                    }
                }
            } else if let Some(command) = overlay_snapshot.selected_command() {
                if matches!(
                    &overlay_snapshot,
                    OverlayState::CommandPicker { title, .. }
                        if title == "Background Tasks"
                ) && command.starts_with("/tasks ")
                    && try_open_overlay(
                        state,
                        resources,
                        providers,
                        auth_store,
                        session_store,
                        tui,
                        &command,
                    )?
                {
                    submit_queued_prompt_if_ready(
                        state,
                        resources,
                        providers,
                        auth_store,
                        auth_path,
                        session_store,
                        tui,
                        no_alt_screen,
                    )?;
                    return Ok(false);
                }
                set_overlay_state(tui, None);
                handle_submit(
                    state,
                    resources,
                    providers,
                    auth_store,
                    auth_path,
                    session_store,
                    command,
                    no_alt_screen,
                )?;
            } else {
                set_overlay_state(tui, None);
            }
            submit_queued_prompt_if_ready(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                tui,
                no_alt_screen,
            )?;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if tui.active_loop.is_some() {
                cancel_pending_submit(state, session_store, tui)?;
                tui.active_loop = None;
                tui.queued_prompts.clear();
                emit_system_message(state, session_store, "Loop stopped.".to_string())?;
            } else if tui.has_pending_submit() {
                // First Ctrl+C during a running turn → cancel the turn
                cancel_pending_submit(state, session_store, tui)?;
                emit_system_message(
                    state,
                    session_store,
                    "Interrupted. Press Ctrl+C again to exit.".to_string(),
                )?;
                tui.last_ctrl_c = Some(std::time::Instant::now());
            } else if tui.should_exit_on_ctrl_c() {
                state.should_exit = true;
                return Ok(true);
            } else {
                emit_system_message(
                    state,
                    session_store,
                    "Press Ctrl+C again to exit.".to_string(),
                )?;
            }
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if ch == '/' && tui.input.is_empty() {
                let commands = command_surface(resources);
                set_overlay_state(tui, None);
                tui.insert_char(ch, &commands);
                return Ok(false);
            }
            let commands = command_surface(resources);
            tui.insert_char(ch, &commands);
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.select_matching_query(&tui.input);
            }
        }
        _ => {}
    }
    Ok(false)
}
#[cfg(test)]
mod tests;
