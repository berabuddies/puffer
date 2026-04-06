mod markdown;
#[path = "onboarding/mod.rs"]
mod onboarding;
mod popup;
mod render;
#[path = "state.rs"]
mod state;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as terminal_size, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use puffer_config::{save_user_config, ConfigPaths};
use puffer_core::{
    dispatch_command, execute_user_turn, run_resource_hooks, supported_commands, AppState,
    CommandSpec, MessageRole, ToolInvocation,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry, StoredCredential};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, TranscriptEvent};
use puffer_tools::{ToolInput, ToolRegistry};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use ratatui::{TerminalOptions, Viewport};
use std::io;
use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use state::TuiState;
pub(crate) use state::{AuthPickerAction, ModelPickerEntry, OverlayState};

/// Runs the interactive Puffer TUI until the user exits.
pub fn run_app(
    state: &mut AppState,
    resources: &LoadedResources,
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
    enable_raw_mode()?;
    if !no_alt_screen {
        execute!(io::stdout(), EnterAlternateScreen)?;
    }

    let mut terminal = if no_alt_screen {
        let (_, height) = terminal_size()?;
        Terminal::with_options(
            CrosstermBackend::new(io::stdout()),
            TerminalOptions {
                viewport: Viewport::Inline(height.max(1)),
            },
        )?
    } else {
        Terminal::new(CrosstermBackend::new(io::stdout()))?
    };
    let mut tui = TuiState::default();
    tui.defer_prompt(
        initial_prompt
            .clone()
            .filter(|prompt| !prompt.trim().is_empty())
            .filter(|_| !bypass_onboarding),
    );
    let commands = supported_commands();
    if let Some(prompt) = initial_prompt.filter(|prompt| allow_prompt_before_onboarding(prompt)) {
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

    loop {
        terminal.draw(|frame| {
            render::set_active_overlay(tui.overlay.clone());
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

    disable_raw_mode()?;
    if !no_alt_screen {
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    }
    Ok(())
}

fn handle_key(
    key: KeyEvent,
    state: &mut AppState,
    resources: &LoadedResources,
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
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.should_exit = true;
            return Ok(true);
        }
        KeyCode::Esc => tui.clear(commands),
        KeyCode::Left => tui.move_left(),
        KeyCode::Right => tui.move_right(),
        KeyCode::Home => tui.move_home(),
        KeyCode::End => tui.move_end(),
        KeyCode::Up => {
            if tui.input.starts_with('/') {
                tui.select_previous(commands)
            } else {
                tui.scroll_up(1);
            }
        }
        KeyCode::Down => {
            if tui.input.starts_with('/') {
                tui.select_next(commands)
            } else {
                tui.scroll_down(1, render::transcript_line_count(state));
            }
        }
        KeyCode::PageUp => tui.scroll_up(10),
        KeyCode::PageDown => tui.scroll_down(10, render::transcript_line_count(state)),
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
                providers,
                auth_store,
                session_store,
                tui,
                &current_input,
            )? {
                return Ok(false);
            }
            let submitted = tui.take_input();
            handle_submit(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                submitted,
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
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    tui: &mut TuiState,
    no_alt_screen: bool,
) -> Result<bool> {
    let Some(overlay) = tui.overlay.as_mut() else {
        return Ok(false);
    };

    if overlay.accepts_text_input() {
        match key.code {
            KeyCode::Esc => {
                tui.overlay = onboarding::back_overlay(overlay, providers, auth_store)?;
            }
            KeyCode::Left => overlay.move_left(),
            KeyCode::Right => overlay.move_right(),
            KeyCode::Home => overlay.move_home(),
            KeyCode::End => overlay.move_end(),
            KeyCode::Backspace => overlay.backspace(),
            KeyCode::Delete => overlay.delete(),
            KeyCode::Enter => {
                let Some(provider_id) = overlay.selected_provider().map(str::to_string) else {
                    tui.overlay = None;
                    return Ok(false);
                };
                let key_value = overlay.api_key_value().unwrap_or("").trim().to_string();
                if key_value.is_empty() {
                    tui.overlay = onboarding::back_overlay(overlay, providers, auth_store)?;
                    emit_system_message(
                        state,
                        session_store,
                        format!("No API key was entered for {provider_id}."),
                    )?;
                    return Ok(false);
                }
                auth_store.set_api_key(provider_id.clone(), key_value);
                auth_store.save(auth_path)?;
                tui.overlay =
                    onboarding::provider_setup_overlay(providers, auth_store, &provider_id)?;
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
                state.should_exit = true;
                return Ok(true);
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                overlay.insert_char(ch)
            }
            _ => {}
        }
        return Ok(false);
    }

    match key.code {
        KeyCode::Esc => {
            tui.overlay = onboarding::back_overlay(overlay, providers, auth_store)?;
        }
        KeyCode::Up => overlay.select_previous(),
        KeyCode::Down => overlay.select_next(),
        KeyCode::PageUp => overlay.page_up(),
        KeyCode::PageDown => overlay.page_down(),
        KeyCode::Enter => {
            if matches!(overlay, OverlayState::ThemePicker { .. })
                && onboarding::initial_overlay(state, providers, auth_store)?.is_some()
            {
                let Some(command) = overlay.selected_command() else {
                    tui.overlay = None;
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
                tui.overlay = onboarding::initial_provider_overlay(providers);
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
            if let Some(provider_id) = overlay.selected_provider().map(str::to_string) {
                match overlay {
                    OverlayState::ProviderPicker { .. } | OverlayState::LoginPicker { .. } => {
                        apply_selected_provider(state, &provider_id)?;
                        tui.overlay = onboarding::provider_setup_overlay(
                            providers,
                            auth_store,
                            &provider_id,
                        )?;
                    }
                    OverlayState::AuthPicker { .. } => {
                        let Some(action) = overlay.selected_auth_action().cloned() else {
                            tui.overlay = None;
                            return Ok(false);
                        };
                        apply_selected_provider(state, &provider_id)?;
                        match action {
                            AuthPickerAction::OAuth => {
                                match run_embedded_auth_login(
                                    &provider_id,
                                    auth_store,
                                    auth_path,
                                    no_alt_screen,
                                ) {
                                    Ok(()) => {
                                        tui.overlay = onboarding::provider_setup_overlay(
                                            providers,
                                            auth_store,
                                            &provider_id,
                                        )?;
                                    }
                                    Err(error) => {
                                        tui.overlay = onboarding::back_overlay(
                                            overlay, providers, auth_store,
                                        )?;
                                        emit_system_message(
                                            state,
                                            session_store,
                                            format!("Login failed for {provider_id}: {error}"),
                                        )?;
                                    }
                                }
                            }
                            AuthPickerAction::ApiKey => {
                                tui.overlay = Some(OverlayState::ApiKeyPrompt {
                                    provider_id,
                                    value: String::new(),
                                    cursor: 0,
                                    onboarding: overlay.is_onboarding(),
                                });
                            }
                            AuthPickerAction::Import(candidate) => {
                                match candidate.credential {
                                    StoredCredential::ApiKey { key } => {
                                        auth_store.set_api_key(provider_id.clone(), key);
                                    }
                                    StoredCredential::OAuth(credential) => {
                                        auth_store.set_oauth(provider_id.clone(), credential);
                                    }
                                }
                                auth_store.save(auth_path)?;
                                tui.overlay = onboarding::provider_setup_overlay(
                                    providers,
                                    auth_store,
                                    &provider_id,
                                )?;
                            }
                            AuthPickerAction::UseStored | AuthPickerAction::NoneRequired => {
                                tui.overlay = onboarding::provider_setup_overlay(
                                    providers,
                                    auth_store,
                                    &provider_id,
                                )?;
                            }
                        }
                    }
                    OverlayState::ModelPicker {
                        onboarding: true, ..
                    } => {
                        let Some(model_id) = overlay.selected_model().map(str::to_string) else {
                            tui.overlay = None;
                            return Ok(false);
                        };
                        state.current_provider = Some(provider_id.clone());
                        state.current_model = Some(format!("{provider_id}/{model_id}"));
                        state.config.default_provider = Some(provider_id.clone());
                        state.config.default_model = Some(format!("{provider_id}/{model_id}"));
                        persist_user_config(state)?;
                        emit_system_message(
                            state,
                            session_store,
                            format!("Active model set to {provider_id}/{model_id}."),
                        )?;
                        tui.overlay = None;
                    }
                    _ => {
                        if let Some(command) = overlay.selected_command() {
                            tui.overlay = None;
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
                            tui.overlay = None;
                        }
                    }
                }
            } else if let Some(command) = overlay.selected_command() {
                tui.overlay = None;
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
                tui.overlay = None;
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
            state.should_exit = true;
            return Ok(true);
        }
        _ => {}
    }
    Ok(false)
}

fn try_open_overlay(
    state: &AppState,
    providers: &mut ProviderRegistry,
    auth_store: &AuthStore,
    session_store: &SessionStore,
    tui: &mut TuiState,
    submitted: &str,
) -> Result<bool> {
    if let Some(overlay) =
        onboarding::overlay_from_command(state, providers, auth_store, session_store, submitted)?
    {
        tui.overlay = Some(overlay);
        tui.input.clear();
        tui.cursor = 0;
        tui.slash_selection = 0;
        return Ok(true);
    }
    Ok(false)
}

fn handle_submit(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    submitted: String,
    no_alt_screen: bool,
) -> Result<()> {
    let submitted = submitted.trim().to_string();
    if submitted.is_empty() {
        return Ok(());
    }

    if handle_auth_command(
        state,
        auth_store,
        auth_path,
        session_store,
        &submitted,
        no_alt_screen,
    )? {
        return Ok(());
    }

    if submitted.starts_with('/') {
        dispatch_command(
            state,
            &supported_commands(),
            resources,
            providers,
            auth_store,
            session_store,
            &submitted,
        )?;
        return Ok(());
    }

    if let Some(shell_command) = parse_shell_shortcut(&submitted) {
        execute_shell_shortcut(state, resources, session_store, shell_command)?;
        return Ok(());
    }

    state.push_message(MessageRole::User, submitted.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: submitted.clone(),
        },
    )?;

    match execute_user_turn(state, resources, providers, auth_store, &submitted) {
        Ok(turn) => {
            append_tool_messages(state, session_store, &turn.tool_invocations)?;
            state.push_message(MessageRole::Assistant, turn.assistant_text.clone());
            session_store.append_event(
                state.session.id,
                TranscriptEvent::AssistantMessage {
                    text: turn.assistant_text,
                },
            )?;
        }
        Err(error) => {
            let message = format!("Provider request failed: {error}");
            state.push_message(MessageRole::System, message.clone());
            session_store.append_event(
                state.session.id,
                TranscriptEvent::SystemMessage { text: message },
            )?;
        }
    }

    Ok(())
}

fn apply_selected_provider(state: &mut AppState, provider_id: &str) -> Result<()> {
    state.current_provider = Some(provider_id.to_string());
    state.current_model = None;
    state.config.default_provider = Some(provider_id.to_string());
    state.config.default_model = None;
    persist_user_config(state)
}

fn persist_user_config(state: &AppState) -> Result<()> {
    let paths = ConfigPaths::discover(&state.cwd);
    save_user_config(&paths, &state.config)
}

fn submit_queued_prompt_if_ready(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    tui: &mut TuiState,
    no_alt_screen: bool,
) -> Result<()> {
    if tui
        .deferred_prompt
        .as_deref()
        .map(str::trim)
        .is_some_and(|prompt| prompt == "/help" || prompt == "/?")
    {
        if let Some(prompt) = tui.take_deferred_prompt() {
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
        return Ok(());
    }
    if tui.overlay.is_some() {
        return Ok(());
    }
    if let Some(overlay) = onboarding::initial_overlay(state, providers, auth_store)? {
        tui.overlay = Some(overlay);
        return Ok(());
    }
    if let Some(prompt) = tui.take_deferred_prompt() {
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
    Ok(())
}

fn emit_system_message(
    state: &mut AppState,
    session_store: &SessionStore,
    text: String,
) -> Result<()> {
    state.push_message(MessageRole::System, text.clone());
    session_store.append_event(state.session.id, TranscriptEvent::SystemMessage { text })?;
    Ok(())
}

fn handle_auth_command(
    state: &mut AppState,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    submitted: &str,
    no_alt_screen: bool,
) -> Result<bool> {
    let without_slash = submitted.trim_start_matches('/');
    let (name, args) = without_slash
        .split_once(' ')
        .map(|(name, args)| (name, args.trim()))
        .unwrap_or((without_slash, ""));
    if name == "login" {
        let provider = if args.is_empty() {
            state.current_provider.as_deref().unwrap_or("anthropic")
        } else {
            args
        };
        run_embedded_auth_login(provider, auth_store, auth_path, no_alt_screen)?;
        let message = format!("Completed login flow for {provider}.");
        state.push_message(MessageRole::System, message.clone());
        session_store.append_event(
            state.session.id,
            TranscriptEvent::SystemMessage { text: message },
        )?;
        return Ok(true);
    }

    if name != "logout" {
        return Ok(false);
    }

    let provider = if args.is_empty() {
        state.current_provider.as_deref().unwrap_or("anthropic")
    } else {
        args
    };
    let message = if auth_store.remove(provider).is_some() {
        auth_store.save(auth_path)?;
        format!("Removed stored credentials for {provider}.")
    } else {
        format!("No stored credentials exist for {provider}.")
    };
    state.push_message(MessageRole::System, message.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::SystemMessage { text: message },
    )?;
    Ok(true)
}

fn run_embedded_auth_login(
    provider: &str,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    no_alt_screen: bool,
) -> Result<()> {
    if !no_alt_screen {
        disable_raw_mode()?;
        execute!(io::stdout(), LeaveAlternateScreen)?;
    }

    let status = Command::new(std::env::current_exe()?)
        .arg("auth")
        .arg("login")
        .arg(provider)
        .status()?;

    if !no_alt_screen {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
    }

    if !status.success() {
        anyhow::bail!("login flow for {provider} exited with {}", status);
    }

    *auth_store = AuthStore::load(auth_path)?;
    Ok(())
}

fn append_tool_messages(
    state: &mut AppState,
    session_store: &SessionStore,
    invocations: &[ToolInvocation],
) -> Result<()> {
    for invocation in invocations {
        state.record_task(
            invocation.tool_id.clone(),
            invocation.input.clone(),
            invocation.success,
        );
        let rendered = render_tool_invocation(invocation);
        state.push_message(MessageRole::System, rendered.clone());
        session_store.append_event(
            state.session.id,
            TranscriptEvent::SystemMessage { text: rendered },
        )?;
    }
    Ok(())
}

fn render_tool_invocation(invocation: &ToolInvocation) -> String {
    let status = if invocation.success { "ok" } else { "error" };
    let output = invocation.output.trim();
    if output.is_empty() {
        format!(
            "Tool {} [{}]\ninput: {}",
            invocation.tool_id, status, invocation.input
        )
    } else {
        format!(
            "Tool {} [{}]\ninput: {}\n{}",
            invocation.tool_id, status, invocation.input, output
        )
    }
}

fn execute_shell_shortcut(
    state: &mut AppState,
    resources: &LoadedResources,
    session_store: &SessionStore,
    shell_command: &str,
) -> Result<()> {
    let rendered_command = format!("!{shell_command}");
    state.push_message(MessageRole::User, rendered_command.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: rendered_command,
        },
    )?;

    let registry = ToolRegistry::from_resources(resources);
    let result = registry.execute(
        "bash",
        &state.cwd,
        ToolInput::Bash {
            command: shell_command.to_string(),
            timeout: None,
            run_in_background: false,
            dangerously_disable_sandbox: false,
        },
    )?;
    state.record_task("bash", shell_command.to_string(), result.success);
    run_resource_hooks(
        resources,
        &state.cwd,
        "tool_end",
        &[
            ("PUFFER_TOOL_ID", "bash".to_string()),
            (
                "PUFFER_TOOL_INPUT",
                format!("{{\"command\":\"{}\"}}", shell_command.replace('"', "\\\"")),
            ),
            (
                "PUFFER_TOOL_SUCCESS",
                if result.success { "true" } else { "false" }.to_string(),
            ),
            ("PUFFER_TOOL_STDOUT", result.output.stdout.clone()),
            ("PUFFER_TOOL_STDERR", result.output.stderr.clone()),
        ],
    );

    let reply = if result.output.stderr.is_empty() {
        result.output.stdout
    } else if result.output.stdout.is_empty() {
        result.output.stderr
    } else {
        format!("{}\n{}", result.output.stdout, result.output.stderr)
    };
    let role = if result.success {
        MessageRole::Assistant
    } else {
        MessageRole::System
    };
    state.push_message(role, reply.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::AssistantMessage { text: reply },
    )?;
    Ok(())
}

fn parse_shell_shortcut(input: &str) -> Option<&str> {
    let command = input
        .strip_prefix("!!")
        .or_else(|| input.strip_prefix('!'))?;
    let trimmed = command.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn allow_prompt_before_onboarding(prompt: &str) -> bool {
    matches!(
        prompt.trim(),
        "/help" | "/theme" | "/doctor" | "/status" | "/usage"
    )
}

#[cfg(test)]
mod tests;
