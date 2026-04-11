use crate::app_helpers::{
    apply_model_selection_preferences, apply_selected_model, help_pane_active_without_overlay,
};
use crate::flow::{
    apply_selected_provider, builtin_openai_base_url, builtin_openai_headers,
    builtin_openai_query_params, cancel_pending_submit, emit_system_message, handle_prompt_submit,
    handle_submit, persist_user_config, run_embedded_auth_login, set_overlay_state,
    submit_next_queued_prompt, submit_queued_prompt_if_ready, try_open_overlay,
};
use crate::onboarding;
use crate::permission_prompt_flow::handle_permission_prompt_key;
use crate::prompt_history_store::PromptHistorySource;
use crate::render;
use crate::state::{AuthPickerAction, OverlayState, TuiState};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use puffer_core::{command_surface, AppState, CommandSpec};
use puffer_provider_registry::{AuthStore, ProviderRegistry, StoredCredential};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;
use std::path::Path;

const PROMPT_HISTORY_SEARCH_TITLE: &str = "Search Prompt History";

/// Handles one keyboard event while the main interactive composer is focused.
pub(crate) fn handle_key(
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
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(overlay) = tui.prompt_history_search_overlay()? {
                tui.stash_overlay_draft();
                set_overlay_state(tui, Some(overlay));
            } else {
                tui.status_hint =
                    Some(("No prompt history yet.".into(), std::time::Instant::now()));
            }
            return Ok(false);
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
            } else if tui.archive_current_input(PromptHistorySource::Cleared, &state.cwd)? {
                tui.clear(commands);
                tui.last_ctrl_c = None;
                tui.status_hint = Some(("Prompt cleared.".into(), std::time::Instant::now()));
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
                let _ = tui.archive_current_input(PromptHistorySource::Cleared, &state.cwd)?;
                tui.clear(commands);
            }
        }
        KeyCode::Left => tui.move_left(),
        KeyCode::Right => tui.move_right(),
        KeyCode::Home => tui.move_home(),
        KeyCode::End => tui.move_end(),
        KeyCode::Up => {
            if tui.is_prompt_history_active() {
                let _ = tui.recall_previous_prompt(commands)?;
            } else if tui.input.starts_with('/') {
                tui.select_previous(commands);
            } else if !tui.input.is_empty() || tui.has_prompt_history() {
                let _ = tui.recall_previous_prompt(commands)?;
            } else {
                scroll_transcript_up(state, resources, auth_store, tui, 1);
            }
        }
        KeyCode::Down => {
            if tui.is_prompt_history_active() {
                let _ = tui.recall_next_prompt(commands)?;
            } else if tui.input.starts_with('/') {
                tui.select_next(commands);
            } else if tui.input.is_empty() {
                scroll_transcript_down(state, resources, auth_store, tui, 1);
            }
        }
        KeyCode::PageUp => {
            if tui.is_prompt_history_active() {
                let _ = tui.recall_previous_prompt(commands)?;
            } else if tui.input.starts_with('/') {
                for _ in 0..10 {
                    tui.select_previous(commands);
                }
            } else if !tui.input.is_empty() || tui.has_prompt_history() {
                let _ = tui.recall_previous_prompt(commands)?;
            } else {
                scroll_transcript_up(state, resources, auth_store, tui, 10);
            }
        }
        KeyCode::PageDown => {
            if tui.is_prompt_history_active() {
                let _ = tui.recall_next_prompt(commands)?;
            } else if tui.input.starts_with('/') {
                for _ in 0..10 {
                    tui.select_next(commands);
                }
            } else {
                scroll_transcript_down(state, resources, auth_store, tui, 10);
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
                tui.remember_prompt_history(&current_input, PromptHistorySource::Sent, &state.cwd)?;
                return Ok(false);
            }
            tui.remember_prompt_history(&current_input, PromptHistorySource::Sent, &state.cwd)?;
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

/// Handles one keyboard event while an overlay is focused.
pub(crate) fn handle_overlay_key(
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
    let commands = command_surface(resources);

    if matches!(
        active_overlay,
        OverlayState::CommandPicker { title, .. } if title == PROMPT_HISTORY_SEARCH_TITLE
    ) {
        match key.code {
            KeyCode::Esc => {
                set_overlay_state(tui, None);
                tui.restore_overlay_draft(&commands);
                return Ok(false);
            }
            KeyCode::Enter => {
                let selected = match active_overlay {
                    OverlayState::CommandPicker {
                        entries, selection, ..
                    } => entries
                        .get(*selection)
                        .and_then(|entry| entry.command.clone())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                set_overlay_state(tui, None);
                tui.discard_overlay_draft();
                tui.set_composer_input(selected, &commands);
                return Ok(false);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                set_overlay_state(tui, None);
                tui.restore_overlay_draft(&commands);
                return Ok(false);
            }
            _ => {}
        }
    }

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
            tui.backspace(&commands);
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.select_matching_query(&tui.input);
            }
        }
        KeyCode::Delete => {
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
                    OverlayState::CommandPicker { title, .. } if title == "Background Tasks"
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
                set_overlay_state(tui, None);
                tui.insert_char(ch, &commands);
                return Ok(false);
            }
            tui.insert_char(ch, &commands);
            if let Some(overlay) = tui.overlay.as_mut() {
                overlay.select_matching_query(&tui.input);
            }
        }
        _ => {}
    }
    Ok(false)
}

fn scroll_transcript_up(
    state: &AppState,
    resources: &LoadedResources,
    auth_store: &AuthStore,
    tui: &mut TuiState,
    amount: u16,
) {
    let viewport = render::current_transcript_viewport();
    tui.scroll_up(
        amount,
        render::transcript_line_count(state, resources, auth_store, tui.has_pending_submit()),
        viewport.height,
    );
}

fn scroll_transcript_down(
    state: &AppState,
    resources: &LoadedResources,
    auth_store: &AuthStore,
    tui: &mut TuiState,
    amount: u16,
) {
    let viewport = render::current_transcript_viewport();
    tui.scroll_down(
        amount,
        render::transcript_line_count(state, resources, auth_store, tui.has_pending_submit()),
        viewport.height,
    );
}
