use crate::approval_overlay::ApprovalOverlay;
use crate::btw_overlay::BtwOverlay;
use anyhow::Result;
use puffer_config::{save_user_config, ConfigPaths};
use puffer_core::{
    command_surface, dispatch_command, execute_user_turn,
    execute_user_turn_streaming_with_permissions, reload_runtime_resources, render_config_summary,
    render_context_panel, render_copy_actions, render_doctor_report, render_hooks_actions,
    render_ide_actions, render_mcp_actions, render_permissions_panel, render_plugin_actions,
    render_sandbox_actions, render_skills_panel, run_resource_hooks, AppState, MessageRole,
    PermissionPromptAction, PermissionPromptRequest, ToolInvocation, TurnStreamEvent,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, TranscriptEvent};
use puffer_tools::{ToolInput, ToolRegistry};
use std::io;
use std::path::Path;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
#[path = "flow_auth.rs"]
mod flow_auth;
#[path = "flow_pickers.rs"]
mod flow_pickers;
use crate::onboarding;
use crate::session_overlay::SessionOverlay;
use crate::state::{
    PendingPermissionRequest, PendingSubmit, PendingSubmitEvent, PendingSubmitResult,
};
use crate::task_overlay::open_task_overlay;
use crate::{
    status_overlay::StatusOverlay, task_panels::task_text_overlay, text_overlay::TextOverlay,
};
use crate::{OverlayState, TuiState};
pub(crate) use flow_auth::{handle_auth_command, run_embedded_auth_login};
#[path = "flow_loop.rs"]
mod flow_loop;
use flow_loop::try_handle_loop_command;
pub(crate) use flow_loop::{advance_loop_after_turn, check_loop_interval};

/// Opens a TUI overlay for slash commands that map to picker UI.
pub(crate) fn try_open_overlay(
    state: &AppState,
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &AuthStore,
    session_store: &SessionStore,
    tui: &mut TuiState,
    submitted: &str,
) -> Result<bool> {
    let without_slash = submitted.trim_start_matches('/');
    let (name, args) = without_slash
        .split_once(' ')
        .map(|(name, args)| (name, args.trim()))
        .unwrap_or((without_slash, ""));
    if name == "btw" && !args.is_empty() {
        set_overlay_state(
            tui,
            Some(BtwOverlay::open(
                state, resources, providers, auth_store, args,
            )),
        );
        return Ok(true);
    }
    if matches!((name, args.is_empty()), ("help", true) | ("?", true)) {
        set_overlay_state(tui, Some(OverlayState::Help));
        return Ok(true);
    }
    if name == "status" && args.is_empty() {
        set_overlay_state(
            tui,
            Some(StatusOverlay::open(state, resources, providers, auth_store)),
        );
        return Ok(true);
    }
    if name == "fast" && args.is_empty() {
        if let Some(overlay) = onboarding::fast_mode_picker_for_current_selection(state, providers)
        {
            set_overlay_state(tui, Some(overlay));
            return Ok(true);
        }
    }
    if name == "tasks" && args.is_empty() {
        set_overlay_state(tui, Some(open_task_overlay(state)?));
        return Ok(true);
    }
    if name == "tasks" && !args.is_empty() {
        if let Some(overlay) = task_text_overlay(state, args)? {
            set_overlay_state(tui, Some(overlay));
            return Ok(true);
        }
    }
    if name == "copy" {
        if let Ok(Some(actions)) = render_copy_actions(state, args) {
            let entries = actions
                .into_iter()
                .map(|entry| crate::ModelPickerEntry {
                    selector: entry.label,
                    description: entry.description,
                    command: Some(entry.command),
                })
                .collect::<Vec<_>>();
            if flow_pickers::open_command_picker(tui, "Copy", entries) {
                return Ok(true);
            }
        }
    }
    let text_overlay = match (name, args.is_empty()) {
        ("config", true) => Some(TextOverlay::open("Config", render_config_summary(state)?)),
        ("context", true) => Some(TextOverlay::open(
            "Context",
            render_context_panel(state, resources, providers)?,
        )),
        ("debug", true) => {
            let raw = puffer_core::render_debug_context(state, resources, providers)?;
            Some(TextOverlay::open_styled(
                "Debug Context",
                crate::text_overlay::colorize_debug_context(&raw),
            ))
        }
        ("doctor", true) => Some(TextOverlay::open(
            "Doctor",
            render_doctor_report(state, resources, providers, auth_store)?,
        )),
        ("permissions", true) | ("allowed-tools", true) => Some(TextOverlay::open(
            "Permissions",
            render_permissions_panel(state, resources)?,
        )),
        ("skills", true) => Some(TextOverlay::open("Skills", render_skills_panel(resources))),
        _ => None,
    };
    if let Some(overlay) = text_overlay {
        set_overlay_state(tui, Some(overlay));
        return Ok(true);
    }
    if matches!(
        (name, args.is_empty()),
        ("plugin", true) | ("plugins", true) | ("marketplace", true)
    ) {
        let entries = render_plugin_actions(state, resources)?
            .into_iter()
            .map(|entry| crate::ModelPickerEntry {
                selector: entry.command.clone(),
                description: entry.description,
                command: Some(entry.command),
            })
            .collect::<Vec<_>>();
        if flow_pickers::open_command_picker(tui, "Plugins", entries) {
            return Ok(true);
        }
    }
    if matches!((name, args.is_empty()), ("mcp", true)) {
        let entries = flow_pickers::command_picker_entries(render_mcp_actions(state, resources)?);
        if flow_pickers::open_command_picker(tui, "MCP", entries) {
            return Ok(true);
        }
    }
    if matches!((name, args.is_empty()), ("ide", true)) {
        let entries = flow_pickers::command_picker_entries(render_ide_actions(state, resources)?);
        if flow_pickers::open_command_picker(tui, "IDE", entries) {
            return Ok(true);
        }
    }
    if matches!((name, args.is_empty()), ("hooks", true)) {
        let entries = flow_pickers::command_picker_entries(render_hooks_actions(state, resources)?);
        if flow_pickers::open_command_picker(tui, "Hooks", entries) {
            return Ok(true);
        }
    }
    if matches!((name, args.is_empty()), ("sandbox", true)) {
        let entries = flow_pickers::command_picker_entries(render_sandbox_actions(state)?);
        if flow_pickers::open_command_picker(tui, "Sandbox", entries) {
            return Ok(true);
        }
    }
    if matches!(
        (name, args.is_empty()),
        ("session", true) | ("remote", true)
    ) {
        set_overlay_state(tui, Some(SessionOverlay::open(state)));
        return Ok(true);
    }
    if name == "rewind" && args.is_empty() {
        let entries = flow_pickers::rewind_picker_entries(state);
        if flow_pickers::open_command_picker(tui, "Rewind", entries) {
            return Ok(true);
        }
    }
    if name == "memory" && args.is_empty() {
        let entries = flow_pickers::memory_picker_entries(state);
        if flow_pickers::open_command_picker(tui, "Memory", entries) {
            return Ok(true);
        }
    }
    if name == "tag" && flow_pickers::open_tag_confirmation_picker(state, tui, args) {
        return Ok(true);
    }
    if let Some(overlay) =
        onboarding::overlay_from_command(state, providers, auth_store, session_store, submitted)?
    {
        set_overlay_state(tui, Some(overlay));
        return Ok(true);
    }
    Ok(false)
}

/// Replaces the active overlay and clears the overlay query buffer.
pub(crate) fn set_overlay_state(tui: &mut TuiState, overlay: Option<OverlayState>) {
    tui.overlay = overlay;
    tui.input.clear();
    tui.cursor = 0;
    tui.slash_selection = 0;
}

/// Returns true when the submitted text should run through the async provider path.
pub(crate) fn is_provider_prompt_input(submitted: &str) -> bool {
    let submitted = submitted.trim();
    !submitted.is_empty()
        && !submitted.starts_with('/')
        && parse_shell_shortcut(submitted).is_none()
        && !is_auth_command_input(submitted)
}

/// Handles one prompt submission from the interactive composer.
pub(crate) fn handle_prompt_submit(
    state: &mut AppState,
    resources: &mut LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    tui: &mut TuiState,
    submitted: String,
    no_alt_screen: bool,
) -> Result<()> {
    let submitted = submitted.trim().to_string();
    if submitted.is_empty() {
        return Ok(());
    }
    if try_handle_loop_command(state, session_store, tui, &submitted)? {
        return Ok(());
    }
    if tui.has_pending_submit() && is_provider_prompt_input(&submitted) {
        tui.enqueue_prompt(submitted);
        return Ok(());
    }
    if !is_provider_prompt_input(&submitted) {
        let had_transcript = !state.transcript.is_empty();
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
        // Clear active loop if transcript was wiped (/compact, /clear).
        if had_transcript && state.transcript.is_empty() && tui.active_loop.is_some() {
            tui.active_loop = None;
            tui.queued_prompts.clear();
        }
        return Ok(());
    }
    if tui.has_pending_submit() {
        return Ok(());
    }

    state.push_message(MessageRole::User, submitted.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: submitted.clone(),
        },
    )?;

    let mut worker_state = state.clone();
    let worker_resources = resources.clone();
    let worker_providers = providers.clone();
    let worker_prompt = submitted.clone();
    let mut worker_auth_store = auth_store.clone();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let event_sender = sender.clone();
        let permission_sender = sender.clone();
        let outcome = execute_user_turn_streaming_with_permissions(
            &mut worker_state,
            &worker_resources,
            &worker_providers,
            &mut worker_auth_store,
            &worker_prompt,
            None,
            |event| match event {
                TurnStreamEvent::ThinkingDelta(delta) => {
                    let _ = event_sender.send(PendingSubmitEvent::ThinkingDelta(delta));
                }
                TurnStreamEvent::TextDelta(delta) => {
                    let _ = event_sender.send(PendingSubmitEvent::TextDelta(delta));
                }
                TurnStreamEvent::ToolCallsRequested(requests) => {
                    let _ = event_sender.send(PendingSubmitEvent::ToolCallsRequested(requests));
                }
                TurnStreamEvent::ToolInvocations(invocations) => {
                    let _ = event_sender.send(PendingSubmitEvent::ToolInvocations(invocations));
                }
                TurnStreamEvent::ReflectionCheckpoint(summary) => {
                    let _ = event_sender.send(PendingSubmitEvent::ReflectionCheckpoint(summary));
                }
                // Trace events ride the stream for incremental consumers
                // but we drain them from `turn.reflection_traces` in the
                // main thread below (persisting them requires `session_store`,
                // which isn't moved into the worker thread). No persistence
                // work on the stream side — avoids double-write.
                TurnStreamEvent::ReflectionTrace(_) => {}
                TurnStreamEvent::RetryAttempt {
                    attempt,
                    max_attempts,
                    error,
                } => {
                    let _ = event_sender.send(PendingSubmitEvent::RetryAttempt {
                        attempt,
                        max_attempts,
                        error,
                    });
                }
                TurnStreamEvent::Usage(report) => {
                    let _ = event_sender.send(PendingSubmitEvent::Usage(report));
                }
            },
            move |request: PermissionPromptRequest| {
                let (response_tx, response_rx) = mpsc::channel();
                let _ = permission_sender
                    .send(PendingSubmitEvent::PermissionRequest(request, response_tx));
                response_rx.recv().unwrap_or(PermissionPromptAction::Deny)
            },
        )
        .map_err(|error| error.to_string());
        let _ = sender.send(PendingSubmitEvent::Finished(PendingSubmitResult {
            outcome,
            auth_store: worker_auth_store,
            session_tool_permissions: worker_state.session_tool_permissions.clone(),
            session_allow_all: worker_state.session_allow_all,
        }));
    });
    tui.pending_submit = Some(PendingSubmit {
        prompt: submitted,
        receiver,
        pending_tool_calls: Vec::new(),
        rendered_tool_invocations: 0,
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: None,
    });
    Ok(())
}

/// Cancels the in-flight provider turn and discards future worker output.
pub(crate) fn cancel_pending_submit(
    state: &mut AppState,
    session_store: &SessionStore,
    tui: &mut TuiState,
) -> Result<bool> {
    if tui.pending_submit.take().is_none() {
        return Ok(false);
    }
    let message = "Interrupted by user.".to_string();
    state.push_message(MessageRole::System, message.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::SystemMessage { text: message },
    )?;
    Ok(true)
}

/// Starts the next queued prompt when no turn is currently running.
pub(crate) fn submit_next_queued_prompt(
    state: &mut AppState,
    resources: &mut LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    tui: &mut TuiState,
    no_alt_screen: bool,
) -> Result<bool> {
    let Some(prompt) = tui.dequeue_prompt() else {
        return Ok(false);
    };
    handle_prompt_submit(
        state,
        resources,
        providers,
        auth_store,
        auth_path,
        session_store,
        tui,
        prompt,
        no_alt_screen,
    )?;
    Ok(true)
}

/// Applies any completed async provider turn to session and transcript state.
pub(crate) fn poll_pending_submit(
    state: &mut AppState,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    tui: &mut TuiState,
) -> Result<bool> {
    let Some(pending) = tui.pending_submit.as_mut() else {
        return Ok(false);
    };
    let mut completed = false;
    loop {
        let event = match pending.receiver.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => PendingSubmitEvent::Finished(PendingSubmitResult {
                outcome: Err("background request disconnected".to_string()),
                auth_store: auth_store.clone(),
                session_tool_permissions: std::collections::HashMap::new(),
                session_allow_all: false,
            }),
        };
        match event {
            PendingSubmitEvent::ThinkingDelta(delta) => {
                pending.thinking_active = true;
                append_thinking_delta(state, &delta);
            }
            PendingSubmitEvent::TextDelta(delta) => {
                pending.thinking_active = false;
                append_assistant_delta(state, &delta);
            }
            PendingSubmitEvent::ToolCallsRequested(requests) => {
                pending.pending_tool_calls.extend(requests);
                break;
            }
            PendingSubmitEvent::ToolInvocations(invocations) => {
                let completed = invocations.len().min(pending.pending_tool_calls.len());
                pending.pending_tool_calls.drain(0..completed);
                pending.rendered_tool_invocations += invocations.len();
                append_tool_messages(state, session_store, &invocations)?;
            }
            PendingSubmitEvent::ReflectionCheckpoint(summary) => {
                pending.status_hint = Some(summary);
            }
            PendingSubmitEvent::RetryAttempt {
                attempt,
                max_attempts,
                error: _,
            } => {
                pending.status_hint =
                    Some(format!("Retrying ({}/{})\u{2026}", attempt, max_attempts));
            }
            PendingSubmitEvent::Usage(report) => {
                state.update_cache_stats(report.input_tokens, report.cache_read_tokens);
                pending.status_hint = None; // clear retry hint on success
            }
            PendingSubmitEvent::PermissionRequest(request, response_tx) => {
                tui.pending_permission_request = Some(PendingPermissionRequest { response_tx });
                tui.overlay = Some(OverlayState::PermissionPrompt {
                    overlay: ApprovalOverlay::new(request),
                });
                break;
            }
            PendingSubmitEvent::Finished(result) => {
                completed = true;
                let rendered_tool_invocations = pending.rendered_tool_invocations;
                let previous_auth_store = auth_store.clone();
                *auth_store = result.auth_store;
                // Sync session permissions from worker clone back to main state.
                state
                    .session_tool_permissions
                    .extend(result.session_tool_permissions);
                if result.session_allow_all {
                    state.session_allow_all = true;
                }
                match result.outcome {
                    Ok(turn) => {
                        if rendered_tool_invocations < turn.tool_invocations.len() {
                            append_tool_messages(
                                state,
                                session_store,
                                &turn.tool_invocations[rendered_tool_invocations..],
                            )?;
                        }
                        // TurnExecution carries every trace event produced
                        // during the turn (both streaming and non-streaming
                        // paths populate it). Persist them via the shared
                        // sidecar helper so interactive sessions land trace
                        // data in the same place as benchmark and slash
                        // command runs.
                        puffer_core::append_trace_events(
                            session_store,
                            state.session.id,
                            &turn.reflection_traces,
                        );
                        finalize_assistant_text(state, session_store, &turn.assistant_text)?;
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
                if *auth_store != previous_auth_store {
                    auth_store.save(auth_path)?;
                }
                session_store.append_event(state.session.id, state.snapshot_event())?;
                break;
            }
        }
    }
    if completed {
        tui.pending_submit = None;
    }
    Ok(completed)
}

/// Resolves the active permission prompt and unblocks the worker thread.
pub(crate) fn respond_to_permission_prompt(
    tui: &mut TuiState,
    action: PermissionPromptAction,
) -> bool {
    let Some(pending) = tui.pending_permission_request.take() else {
        return false;
    };
    let _ = pending.response_tx.send(action);
    set_overlay_state(tui, None);
    true
}

/// Submits prompt/auth/shell input from the TUI prompt.
pub(crate) fn handle_submit(
    state: &mut AppState,
    resources: &mut LoadedResources,
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
        let previous_auth_store = auth_store.clone();
        if command_requires_terminal_restore(&submitted) {
            run_with_terminal_restored(no_alt_screen, || {
                dispatch_command(
                    state,
                    &command_surface(resources),
                    resources,
                    providers,
                    auth_store,
                    session_store,
                    &submitted,
                )
            })?;
        } else {
            dispatch_command(
                state,
                &command_surface(resources),
                resources,
                providers,
                auth_store,
                session_store,
                &submitted,
            )?;
        }
        if *auth_store != previous_auth_store {
            auth_store.save(auth_path)?;
        }
        maybe_apply_requested_reload(state, resources, providers, auth_store, session_store)?;
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

    let previous_auth_store = auth_store.clone();
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
    if *auth_store != previous_auth_store {
        auth_store.save(auth_path)?;
    }
    session_store.append_event(state.session.id, state.snapshot_event())?;

    Ok(())
}

fn is_auth_command_input(submitted: &str) -> bool {
    matches!(submit_command_name(submitted), "login" | "logout")
}

fn append_thinking_delta(state: &mut AppState, delta: &str) {
    if delta.is_empty() {
        return;
    }
    if let Some(last) = state.transcript.last_mut() {
        if last.role == MessageRole::Assistant {
            last.thinking
                .get_or_insert_with(String::new)
                .push_str(delta);
            return;
        }
    }
    // No existing assistant message yet — create one with thinking content.
    state.push_message(MessageRole::Assistant, String::new());
    if let Some(last) = state.transcript.last_mut() {
        last.thinking = Some(delta.to_string());
    }
}

fn append_assistant_delta(state: &mut AppState, delta: &str) {
    if delta.is_empty() {
        return;
    }
    if let Some(last) = state.transcript.last_mut() {
        if last.role == MessageRole::Assistant {
            last.text.push_str(delta);
            return;
        }
    }
    state.push_message(MessageRole::Assistant, delta.to_string());
}

fn finalize_assistant_text(
    state: &mut AppState,
    session_store: &SessionStore,
    assistant_text: &str,
) -> Result<()> {
    if let Some(last) = state.transcript.last_mut() {
        if last.role == MessageRole::Assistant {
            last.text = assistant_text.to_string();
        } else {
            state.push_message(MessageRole::Assistant, assistant_text.to_string());
        }
    } else {
        state.push_message(MessageRole::Assistant, assistant_text.to_string());
    }
    session_store.append_event(
        state.session.id,
        TranscriptEvent::AssistantMessage {
            text: assistant_text.to_string(),
        },
    )?;
    Ok(())
}

fn submit_command_name(submitted: &str) -> &str {
    submitted
        .trim()
        .trim_start_matches('/')
        .split_once(' ')
        .map(|(name, _)| name)
        .unwrap_or_else(|| submitted.trim().trim_start_matches('/'))
}

/// Persists the selected provider and clears any selected model until the user chooses one.
pub(crate) fn apply_selected_provider(state: &mut AppState, provider_id: &str) -> Result<()> {
    state.current_provider = Some(provider_id.to_string());
    state.current_model = None;
    state.config.default_provider = Some(provider_id.to_string());
    state.config.default_model = None;
    persist_user_config(state)
}

/// Persists the current user config to `~/.puffer/config.toml`.
pub(crate) fn persist_user_config(state: &AppState) -> Result<()> {
    let paths = ConfigPaths::discover(&state.cwd);
    save_user_config(&paths, &state.config)
}

/// Returns the builtin OpenAI base URL from loaded provider resources.
pub(crate) fn builtin_openai_base_url(resources: &LoadedResources) -> Option<String> {
    resources
        .providers
        .iter()
        .find(|provider| provider.value.id == "openai")
        .map(|provider| provider.value.base_url.clone())
}

/// Returns builtin OpenAI headers from loaded provider resources.
pub(crate) fn builtin_openai_headers(
    resources: &LoadedResources,
) -> indexmap::IndexMap<String, String> {
    resources
        .providers
        .iter()
        .find(|provider| provider.value.id == "openai")
        .map(|provider| provider.value.headers.clone())
        .unwrap_or_default()
}

/// Returns builtin OpenAI query params from loaded provider resources.
pub(crate) fn builtin_openai_query_params(
    resources: &LoadedResources,
) -> indexmap::IndexMap<String, String> {
    resources
        .providers
        .iter()
        .find(|provider| provider.value.id == "openai")
        .map(|provider| provider.value.query_params.clone())
        .unwrap_or_default()
}

/// Re-enters onboarding when needed or submits any queued prompt once setup is complete.
pub(crate) fn submit_queued_prompt_if_ready(
    state: &mut AppState,
    resources: &mut LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    tui: &mut TuiState,
    no_alt_screen: bool,
) -> Result<()> {
    if tui.has_pending_submit() {
        return Ok(());
    }
    if tui
        .deferred_prompt
        .as_deref()
        .map(str::trim)
        .is_some_and(allow_prompt_before_onboarding)
    {
        if let Some(prompt) = tui.take_deferred_prompt() {
            handle_startup_bypass_prompt(
                state,
                resources,
                providers,
                auth_store,
                auth_path,
                session_store,
                tui,
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
    if let Some(prompt) = state.take_pending_query_prompt() {
        handle_prompt_submit(
            state,
            resources,
            providers,
            auth_store,
            auth_path,
            session_store,
            tui,
            prompt,
            no_alt_screen,
        )?;
        return Ok(());
    }
    if let Some(prompt) = tui.take_deferred_prompt() {
        handle_prompt_submit(
            state,
            resources,
            providers,
            auth_store,
            auth_path,
            session_store,
            tui,
            prompt,
            no_alt_screen,
        )?;
    }
    Ok(())
}

/// Reloads runtime resources when commands request it and emits the reload summary.
pub(crate) fn maybe_apply_requested_reload(
    state: &mut AppState,
    resources: &mut LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &AuthStore,
    session_store: &SessionStore,
) -> Result<()> {
    if !state.reload_resources_requested {
        return Ok(());
    }
    state.reload_resources_requested = false;
    let summary = reload_runtime_resources(state, resources, providers, auth_store)?;
    emit_system_message(state, session_store, summary)
}

/// Appends one system message to the in-memory transcript and persisted session log.
pub(crate) fn emit_system_message(
    state: &mut AppState,
    session_store: &SessionStore,
    text: String,
) -> Result<()> {
    state.push_message(MessageRole::System, text.clone());
    session_store.append_event(state.session.id, TranscriptEvent::SystemMessage { text })?;
    Ok(())
}

/// Routes startup-safe slash commands through overlay handling before falling back to submission.
pub(crate) fn handle_startup_bypass_prompt(
    state: &mut AppState,
    resources: &mut LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    session_store: &SessionStore,
    tui: &mut TuiState,
    submitted: String,
    no_alt_screen: bool,
) -> Result<()> {
    if submitted.trim_start().starts_with('/')
        && try_open_overlay(
            state,
            resources,
            providers,
            auth_store,
            session_store,
            tui,
            &submitted,
        )?
    {
        return Ok(());
    }
    handle_submit(
        state,
        resources,
        providers,
        auth_store,
        auth_path,
        session_store,
        submitted,
        no_alt_screen,
    )
}

/// Records tool invocations into transcript/task/session state.
pub(crate) fn append_tool_messages(
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
        state.push_tool_invocation(
            &invocation.call_id,
            &invocation.tool_id,
            &invocation.input,
            &invocation.output,
            invocation.success,
        );
        session_store.append_event(
            state.session.id,
            TranscriptEvent::ToolInvocation {
                call_id: invocation.call_id.clone(),
                tool_id: invocation.tool_id.clone(),
                input: invocation.input.clone(),
                output: invocation.output.clone(),
                success: invocation.success,
            },
        )?;
    }
    Ok(())
}

/// Executes a `!cmd` shell shortcut and records the result into the transcript.
pub(crate) fn execute_shell_shortcut(
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
    run_resource_hooks(
        resources,
        &state.cwd,
        "tool_start",
        &[
            ("PUFFER_TOOL_ID", "bash".to_string()),
            (
                "PUFFER_TOOL_INPUT",
                format!("{{\"command\":\"{}\"}}", shell_command.replace('"', "\\\"")),
            ),
            ("PUFFER_TOOL_SUCCESS", String::new()),
            ("PUFFER_TOOL_STDOUT", String::new()),
            ("PUFFER_TOOL_STDERR", String::new()),
        ],
    );
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

/// Parses the `!cmd` shell shortcut form used by Claude/Codex-style CLIs.
pub(crate) fn parse_shell_shortcut(input: &str) -> Option<&str> {
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

/// Returns true for slash commands that should bypass startup onboarding.
pub(crate) fn allow_prompt_before_onboarding(prompt: &str) -> bool {
    matches!(
        prompt.trim(),
        "/help" | "/?" | "/theme" | "/doctor" | "/status" | "/usage" | "/context"
    )
}

fn command_requires_terminal_restore(submitted: &str) -> bool {
    let trimmed = submitted.trim();
    matches!(
        trimmed,
        "/plan open"
            | "/memory open"
            | "/memory open project"
            | "/memory open workspace"
            | "/memory open user"
            | "/memory edit"
            | "/memory edit project"
            | "/memory edit workspace"
            | "/memory edit user"
    ) || trimmed == "/plugin open"
        || trimmed == "/plugin edit"
        || trimmed.starts_with("/plugin open ")
        || trimmed.starts_with("/plugin edit ")
        || trimmed == "/mcp open"
        || trimmed == "/mcp edit"
        || trimmed.starts_with("/mcp open ")
        || trimmed.starts_with("/mcp edit ")
}

fn run_with_terminal_restored<T>(
    no_alt_screen: bool,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if !no_alt_screen {
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    }
    let result = action();
    if !no_alt_screen {
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    }
    result
}

#[cfg(test)]
#[path = "flow_tests.rs"]
mod flow_tests;
