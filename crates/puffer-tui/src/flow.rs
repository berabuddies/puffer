use crate::approval_overlay::ApprovalOverlay;
use crate::btw_overlay::BtwOverlay;
use anyhow::Result;
use puffer_config::{save_user_config, ConfigPaths};
use puffer_core::{
    command_surface, dispatch_command, execute_user_turn,
    execute_user_turn_streaming_with_permissions_and_cancel, reload_runtime_resources,
    render_config_summary, render_context_panel, render_copy_actions, render_doctor_report,
    render_hooks_actions, render_ide_actions, render_mcp_actions, render_permissions_panel,
    render_plugin_actions, render_sandbox_actions, render_session_panel, render_skills_panel,
    resumable_sessions_for_picker, with_user_question_prompt_handler, AppState, MessageRole,
    PermissionPromptAction, PermissionPromptRequest, ToolInvocation, TurnStreamEvent,
    UserQuestionPromptRequest, UserQuestionPromptResponse,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, SessionSummary, TranscriptEvent};
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
    PendingUserQuestionRequest,
};
use crate::task_overlay::open_task_overlay;
use crate::user_question_overlay::UserQuestionOverlay;
use crate::{
    status_overlay::StatusOverlay, task_panels::task_text_overlay, text_overlay::TextOverlay,
};
use crate::{OverlayState, TuiState};
pub(crate) use flow_auth::{handle_auth_command, run_embedded_auth_login};
#[path = "flow_loop.rs"]
mod flow_loop;
use flow_loop::try_handle_loop_command;
pub(crate) use flow_loop::{advance_loop_after_turn, check_loop_interval};
#[path = "flow_shell.rs"]
mod flow_shell;
pub(crate) use flow_shell::parse_shell_shortcut;
use flow_shell::{
    execute_shell_shortcut, execute_shell_shortcut_inline, finalize_shell_shortcut_result,
};

fn parsed_slash_command(submitted: &str) -> (&str, &str) {
    let trimmed = submitted.trim();
    let without_slash = trimmed.strip_prefix('/').unwrap_or(trimmed);
    without_slash
        .split_once(char::is_whitespace)
        .map(|(name, args)| (name, args.trim()))
        .unwrap_or((without_slash, ""))
}

fn canonical_overlay_command_name(name: &str) -> &str {
    match name {
        "settings" => "config",
        "bashes" => "tasks",
        "checkpoint" => "rewind",
        _ => name,
    }
}

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
    let (name, args) = parsed_slash_command(submitted);
    let name = canonical_overlay_command_name(name);
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
        ("cost", true) => Some(TextOverlay::open(
            "Cost",
            puffer_core::render_cost_summary(state),
        )),
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
        ("session", true) => Some(TextOverlay::open("Session", render_session_panel(state))),
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
    if matches!((name, args.is_empty()), ("remote", true)) {
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

/// Returns true when input should wait for the active provider turn to finish.
pub(crate) fn should_defer_while_turn_is_running(submitted: &str) -> bool {
    let submitted = submitted.trim();
    if submitted.is_empty() {
        return false;
    }
    if is_provider_prompt_input(submitted) || parse_shell_shortcut(submitted).is_some() {
        return true;
    }
    submitted.starts_with('/') && !is_read_only_pending_slash_command(submitted)
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
    if tui.has_pending_submit() && should_defer_while_turn_is_running(&submitted) {
        tui.enqueue_prompt(submitted);
        return Ok(());
    }
    if is_loop_command_input(&submitted) {
        ensure_persistent_session_for_prompt_submit(state, session_store, &submitted)?;
    }
    if try_handle_loop_command(state, session_store, tui, &submitted)? {
        return Ok(());
    }
    if let Some(shell_command) = parse_shell_shortcut(&submitted) {
        ensure_persistent_session_for_prompt_submit(state, session_store, &submitted)?;
        execute_shell_shortcut(state, resources, session_store, tui, shell_command)?;
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
    if providers.providers().next().is_some() {
        if let Some(overlay) = onboarding::prompt_submission_overlay(state, providers, auth_store)?
        {
            tui.defer_prompt(Some(submitted));
            tui.overlay = Some(overlay);
            return Ok(());
        }
    }

    ensure_persistent_session_for_prompt_submit(state, session_store, &submitted)?;
    state.push_message(MessageRole::User, submitted.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: submitted.clone(),
            actor: Some(state.user_actor()),
        },
    )?;
    let transcript_start_len = state.transcript.len();

    let mut worker_state = state.clone();
    let worker_resources = resources.clone();
    let worker_providers = providers.clone();
    let worker_prompt = submitted.clone();
    let mut worker_auth_store = auth_store.clone();
    let (sender, receiver) = mpsc::channel();
    // Cancel handle: cloned into the worker thread, original kept on
    // PendingSubmit so ESC can flip it from the main thread.
    let cancel = puffer_core::CancelToken::new();
    let worker_cancel = cancel.clone();
    thread::spawn(move || {
        let event_sender = sender.clone();
        let permission_sender = sender.clone();
        let question_sender = sender.clone();
        let on_user_question = move |request: UserQuestionPromptRequest| {
            let (response_tx, response_rx) = mpsc::channel();
            if question_sender
                .send(PendingSubmitEvent::UserQuestionRequest(
                    request,
                    response_tx,
                ))
                .is_err()
            {
                return empty_user_question_response();
            }
            response_rx
                .recv()
                .unwrap_or_else(|_| empty_user_question_response())
        };
        let outcome = with_user_question_prompt_handler(on_user_question, || {
            execute_user_turn_streaming_with_permissions_and_cancel(
                &mut worker_state,
                &worker_resources,
                &worker_providers,
                &mut worker_auth_store,
                &worker_prompt,
                None,
                &worker_cancel,
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
                        let _ =
                            event_sender.send(PendingSubmitEvent::ReflectionCheckpoint(summary));
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
        })
        .map_err(|error| error.to_string());
        if let Ok(turn) = &outcome {
            worker_state.push_message(MessageRole::Assistant, turn.assistant_text.clone());
            if puffer_core::project_memory_turn_completed(&mut worker_state) {
                puffer_core::spawn_project_memory_review(
                    &worker_state,
                    &worker_resources,
                    &worker_providers,
                    &worker_auth_store,
                );
            }
        }
        let _ = sender.send(PendingSubmitEvent::Finished(PendingSubmitResult {
            outcome,
            auth_store: worker_auth_store,
            session_permission_state: worker_state.session_permission_state().clone(),
            session_allow_all: worker_state.session_allow_all,
            project_memory_review_turns: worker_state.project_memory_review_turns,
        }));
    });
    tui.pending_submit = Some(PendingSubmit {
        prompt: submitted,
        receiver,
        transcript_persisted_len: transcript_start_len,
        pending_tool_calls: Vec::new(),
        rendered_tool_invocations: 0,
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: None,
        cancel,
    });
    Ok(())
}

/// Cancels the in-flight provider turn. Flips the worker's
/// `CancelToken` so the agent loop returns `Err("cancelled")` at the
/// next turn boundary (between provider calls or tool batches), then
/// drops the receiver so any straggling events are ignored. Without
/// the token flip the worker would silently run to completion against
/// a dropped channel — burning tokens and continuing to spawn tools.
pub(crate) fn cancel_pending_submit(
    state: &mut AppState,
    session_store: &SessionStore,
    tui: &mut TuiState,
) -> Result<bool> {
    let Some(pending) = tui.pending_submit.take() else {
        return Ok(false);
    };
    pending.cancel.cancel();
    drop(pending);
    let message = "Interrupted by user.".to_string();
    state.push_message(MessageRole::System, message.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::SystemMessage {
            text: message,
            actor: Some(state.system_actor()),
        },
    )?;
    Ok(true)
}

/// Starts the next queued prompt when no turn or overlay is currently active.
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
    if tui.has_pending_submit() || tui.overlay.is_some() {
        return Ok(false);
    }
    let Some(prompt) = tui.dequeue_prompt() else {
        return Ok(false);
    };
    if try_open_overlay(
        state,
        resources,
        providers,
        auth_store,
        session_store,
        tui,
        &prompt,
    )? {
        return Ok(true);
    }
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
                session_permission_state: state.session_permission_state().clone(),
                session_allow_all: false,
                project_memory_review_turns: state.project_memory_review_turns,
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
                persist_pending_assistant_drafts(
                    state,
                    session_store,
                    pending.transcript_persisted_len,
                )?;
                let completed = invocations.len().min(pending.pending_tool_calls.len());
                pending.pending_tool_calls.drain(0..completed);
                pending.rendered_tool_invocations += invocations.len();
                append_tool_messages(state, session_store, &invocations)?;
                pending.transcript_persisted_len = state.transcript.len();
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
            PendingSubmitEvent::UserQuestionRequest(request, response_tx) => {
                match UserQuestionOverlay::from_value(request.questions) {
                    Ok(overlay) => {
                        tui.pending_user_question_request =
                            Some(PendingUserQuestionRequest { response_tx });
                        tui.overlay = Some(OverlayState::UserQuestionPrompt { overlay });
                        break;
                    }
                    Err(error) => {
                        let _ = response_tx.send(empty_user_question_response());
                        let message =
                            format!("AskUserQuestion prompt could not be rendered: {error}");
                        state.push_message(MessageRole::System, message.clone());
                        session_store.append_event(
                            state.session.id,
                            TranscriptEvent::SystemMessage {
                                text: message,
                                actor: Some(state.system_actor()),
                            },
                        )?;
                    }
                }
            }
            PendingSubmitEvent::ShellShortcutFinished(result) => {
                completed = true;
                finalize_shell_shortcut_result(state, session_store, result)?;
                break;
            }
            PendingSubmitEvent::Finished(result) => {
                completed = true;
                let rendered_tool_invocations = pending.rendered_tool_invocations;
                let previous_auth_store = auth_store.clone();
                *auth_store = result.auth_store;
                // Sync the full typed session permission state from the worker
                // clone so category grants survive the worker/UI round-trip.
                // This also avoids depending on AppState's legacy rebuild path,
                // where sync_session_permission_state() assumes
                // session_allow_all was updated before reconstruction.
                let _ = result.session_allow_all;
                state.replace_session_permission_state(result.session_permission_state);
                state.project_memory_review_turns = result.project_memory_review_turns;
                match result.outcome {
                    Ok(turn) => {
                        if rendered_tool_invocations < turn.tool_invocations.len() {
                            persist_pending_assistant_drafts(
                                state,
                                session_store,
                                pending.transcript_persisted_len,
                            )?;
                            append_tool_messages(
                                state,
                                session_store,
                                &turn.tool_invocations[rendered_tool_invocations..],
                            )?;
                            pending.transcript_persisted_len = state.transcript.len();
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
                        discard_pending_assistant_drafts(state, pending.transcript_persisted_len);
                        let message = format!("Provider request failed: {error}");
                        state.push_message(MessageRole::System, message.clone());
                        session_store.append_event(
                            state.session.id,
                            TranscriptEvent::SystemMessage {
                                text: message,
                                actor: Some(state.system_actor()),
                            },
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
    close_prompt_overlay_preserving_input(tui);
    true
}

/// Resolves the active user question prompt and unblocks the worker thread.
pub(crate) fn respond_to_user_question(
    tui: &mut TuiState,
    response: UserQuestionPromptResponse,
) -> bool {
    let Some(pending) = tui.pending_user_question_request.take() else {
        return false;
    };
    let _ = pending.response_tx.send(response);
    close_prompt_overlay_preserving_input(tui);
    true
}

fn close_prompt_overlay_preserving_input(tui: &mut TuiState) {
    tui.overlay = None;
    tui.slash_selection = 0;
}

fn empty_user_question_response() -> UserQuestionPromptResponse {
    UserQuestionPromptResponse {
        answers: serde_json::Map::new(),
        annotations: serde_json::Map::new(),
    }
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
    if resume_transient_picker_selection(state, session_store, &submitted)? {
        return Ok(());
    }
    ensure_persistent_session_for_direct_submit(state, session_store)?;

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
        execute_shell_shortcut_inline(state, resources, session_store, shell_command)?;
        return Ok(());
    }

    state.push_message(MessageRole::User, submitted.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: submitted.clone(),
            actor: Some(state.user_actor()),
        },
    )?;

    let previous_auth_store = auth_store.clone();
    match execute_user_turn(state, resources, providers, auth_store, &submitted) {
        Ok(turn) => {
            append_tool_messages(state, session_store, &turn.tool_invocations)?;
            finalize_assistant_text(state, session_store, &turn.assistant_text)?;
            if puffer_core::project_memory_turn_completed(state) {
                puffer_core::spawn_project_memory_review(state, resources, providers, auth_store);
            }
        }
        Err(error) => {
            let message = format!("Provider request failed: {error}");
            state.push_message(MessageRole::System, message.clone());
            session_store.append_event(
                state.session.id,
                TranscriptEvent::SystemMessage {
                    text: message,
                    actor: Some(state.system_actor()),
                },
            )?;
        }
    }
    if *auth_store != previous_auth_store {
        auth_store.save(auth_path)?;
    }
    session_store.append_event(state.session.id, state.snapshot_event())?;

    Ok(())
}

fn ensure_persistent_session_for_prompt_submit(
    state: &mut AppState,
    session_store: &SessionStore,
    submitted: &str,
) -> Result<()> {
    if opens_resume_picker(submitted) {
        return Ok(());
    }
    ensure_persistent_session(state, session_store)
}

fn ensure_persistent_session_for_direct_submit(
    state: &mut AppState,
    session_store: &SessionStore,
) -> Result<()> {
    ensure_persistent_session(state, session_store)
}

fn ensure_persistent_session(state: &mut AppState, session_store: &SessionStore) -> Result<()> {
    if !state.session.id.is_nil() {
        return Ok(());
    }
    state.session = session_store.create_session(state.cwd.clone())?;
    Ok(())
}

fn opens_resume_picker(submitted: &str) -> bool {
    let (name, args) = parsed_slash_command(submitted);
    matches!(canonical_overlay_command_name(name), "resume" | "continue") && args.is_empty()
}

fn resume_transient_picker_selection(
    state: &mut AppState,
    session_store: &SessionStore,
    submitted: &str,
) -> Result<bool> {
    if !state.session.id.is_nil() {
        return Ok(false);
    }
    let Some(summary) = current_scope_resume_selection(state, session_store, submitted)? else {
        return Ok(false);
    };
    let record = session_store.load_session(summary.id)?;
    let pending_query_prompt = state.take_pending_query_prompt();
    let config = state.config.clone();
    *state = AppState::from_session_record(config, record);
    if let Some(prompt) = pending_query_prompt {
        state.queue_pending_query_prompt(prompt);
    }
    session_store.append_event(
        state.session.id,
        TranscriptEvent::CommandInvoked {
            name: "resume".to_string(),
            args: summary.id.to_string(),
            actor: Some(state.user_actor()),
        },
    )?;
    emit_system_message(
        state,
        session_store,
        format!(
            "Resumed session {} [{}].",
            state.session.id,
            state.session.display_name.as_deref().unwrap_or("<unnamed>")
        ),
    )?;
    Ok(true)
}

fn current_scope_resume_selection(
    state: &AppState,
    session_store: &SessionStore,
    submitted: &str,
) -> Result<Option<SessionSummary>> {
    let (name, args) = parsed_slash_command(submitted);
    if !matches!(canonical_overlay_command_name(name), "resume" | "continue") {
        return Ok(None);
    }
    let target_id = args.trim();
    Ok(
        resumable_sessions_for_picker(session_store, state.session.id, &state.cwd)?
            .iter()
            .find(|session| session.id.to_string() == target_id)
            .cloned(),
    )
}

fn is_loop_command_input(submitted: &str) -> bool {
    let (name, _) = parsed_slash_command(submitted);
    matches!(name, "loop" | "maximize" | "max" | "minimize" | "min")
}

fn is_auth_command_input(submitted: &str) -> bool {
    matches!(submit_command_name(submitted), "login" | "logout")
}

fn is_read_only_pending_slash_command(submitted: &str) -> bool {
    let (name, args) = parsed_slash_command(submitted);
    let name = canonical_overlay_command_name(name);
    if name == "tasks" {
        return is_read_only_tasks_command(args);
    }
    if !args.is_empty() {
        return false;
    }
    matches!(
        name,
        "?" | "help"
            | "status"
            | "usage"
            | "cost"
            | "config"
            | "context"
            | "debug"
            | "doctor"
            | "files"
            | "hooks"
            | "ide"
            | "marketplace"
            | "mcp"
            | "memory"
            | "permissions"
            | "allowed-tools"
            | "plugin"
            | "plugins"
            | "sandbox"
            | "skills"
            | "session"
            | "remote"
    )
}

fn is_read_only_tasks_command(args: &str) -> bool {
    let trimmed = args.trim();
    matches!(
        trimmed,
        "" | "show" | "list" | "path" | "agents" | "teams" | "worktrees" | "todos"
    ) || trimmed.starts_with("show ")
        || trimmed.starts_with("get ")
        || trimmed.starts_with("output ")
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
    if assistant_text.trim().is_empty() {
        if state.transcript.last().is_some_and(|message| {
            message.role == MessageRole::Assistant
                && message.text.trim().is_empty()
                && message
                    .thinking
                    .as_deref()
                    .is_none_or(|thinking| thinking.trim().is_empty())
        }) {
            state.transcript.pop();
        }
        return Ok(());
    }
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
            actor: Some(state.assistant_actor()),
        },
    )?;
    Ok(())
}

fn discard_pending_assistant_drafts(state: &mut AppState, transcript_start_len: usize) {
    let mut index = transcript_start_len.min(state.transcript.len());
    while index < state.transcript.len() {
        if state.transcript[index].role == MessageRole::Assistant {
            state.transcript.remove(index);
        } else {
            index += 1;
        }
    }
}

fn persist_pending_assistant_drafts(
    state: &AppState,
    session_store: &SessionStore,
    transcript_persisted_len: usize,
) -> Result<()> {
    for message in state
        .transcript
        .iter()
        .skip(transcript_persisted_len.min(state.transcript.len()))
    {
        if message.role == MessageRole::Assistant && !message.text.trim().is_empty() {
            session_store.append_event(
                state.session.id,
                TranscriptEvent::AssistantMessage {
                    text: message.text.clone(),
                    actor: Some(state.assistant_actor()),
                },
            )?;
        }
    }
    Ok(())
}

fn submit_command_name(submitted: &str) -> &str {
    let (name, _) = parsed_slash_command(submitted);
    name
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

/// Reloads runtime resources when commands or the resource watcher request
/// it and emits the reload summary. Coalesces the in-loop flag and the
/// cross-thread filesystem watcher signal so the watcher and `/reload-plugins`
/// hit the same code path.
///
/// A reload error (e.g. transient invalid YAML from an editor's atomic-save
/// of a partially-written file under `.puffer/resources/`) is surfaced as a
/// system message rather than propagated; the watcher-driven path must not
/// be able to kill the TUI session and discard the in-memory transcript
/// just because a SKILL.md is mid-edit. The user can re-save to retry — the
/// watcher will fire again.
pub(crate) fn maybe_apply_requested_reload(
    state: &mut AppState,
    resources: &mut LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &AuthStore,
    session_store: &SessionStore,
) -> Result<()> {
    if !state.take_reload_request() {
        return Ok(());
    }
    let message = match reload_runtime_resources(state, resources, providers, auth_store) {
        Ok(summary) => summary,
        Err(err) => format!("Resource hot-reload failed: {err:#}"),
    };
    emit_system_message(state, session_store, message)
}

/// Appends one system message to the in-memory transcript and persisted session log.
pub(crate) fn emit_system_message(
    state: &mut AppState,
    session_store: &SessionStore,
    text: String,
) -> Result<()> {
    state.push_message(MessageRole::System, text.clone());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::SystemMessage {
            text,
            actor: Some(state.system_actor()),
        },
    )?;
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
                actor: Some(state.assistant_actor()),
                subject: state.tool_subject_actor(&invocation.tool_id, &invocation.output),
            },
        )?;
    }
    Ok(())
}

/// Returns true for slash commands that should bypass startup onboarding.
pub(crate) fn allow_prompt_before_onboarding(prompt: &str) -> bool {
    let trimmed = prompt.trim();
    if !trimmed.starts_with('/') {
        return false;
    }
    let (name, args) = parsed_slash_command(trimmed);
    let name = canonical_overlay_command_name(name);
    args.is_empty()
        && matches!(
            name,
            "?" | "agents"
                | "config"
                | "context"
                | "cost"
                | "debug"
                | "diff"
                | "doctor"
                | "files"
                | "help"
                | "hooks"
                | "ide"
                | "marketplace"
                | "mcp"
                | "memory"
                | "permissions"
                | "allowed-tools"
                | "plugin"
                | "plugins"
                | "remote"
                | "sandbox"
                | "session"
                | "skills"
                | "status"
                | "tasks"
                | "theme"
                | "usage"
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
