use anyhow::Result;
use puffer_core::{AppState, MessageRole};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_session_store::{SessionStore, TranscriptEvent};
use std::sync::mpsc;
use std::thread;

use crate::state::{PendingSubmit, PendingSubmitEvent};
use crate::TuiState;

/// Starts a `/ultrareview <pr>` run on a background thread so the multi-agent
/// pipeline never blocks the interactive loop. Phase progress streams back as
/// status-hint updates; the final markdown lands as a system message.
pub(crate) fn execute_ultrareview(
    state: &mut AppState,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    session_store: &SessionStore,
    tui: &mut TuiState,
    submitted: &str,
    pr_arg: &str,
) -> Result<()> {
    if tui.has_pending_submit() {
        return emit_system_line(
            state,
            session_store,
            "/ultrareview: a task is already running; wait for it to finish.",
        );
    }

    state.push_message(MessageRole::User, submitted.to_string());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: submitted.to_string(),
            actor: Some(state.user_actor()),
        },
    )?;

    if pr_arg.is_empty() {
        return emit_system_line(
            state,
            session_store,
            "/ultrareview requires a PR url or number, e.g. `/ultrareview 1234`.",
        );
    }

    let (base_url, api_key) = puffer_core::ultrareview::resolve_credentials(providers, auth_store);
    if api_key.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return emit_system_line(
            state,
            session_store,
            "/ultrareview: no credentials for the `openai` provider. \
             Sign in (puffer imports ~/.codex automatically, or run `puffer auth`).",
        );
    }

    let cwd = state.cwd.clone();
    let pr_arg = pr_arg.to_string();
    let (event_tx, event_rx) = mpsc::channel();
    let progress_tx = event_tx.clone();
    // Shared cancel token: the original stays on PendingSubmit so ESC can flip
    // it; the clone lets the worker stop at the next phase boundary.
    let cancel = puffer_core::CancelToken::new();
    let worker_cancel = cancel.clone();
    thread::spawn(move || {
        let progress = move |line: String| {
            let _ = progress_tx.send(PendingSubmitEvent::UltrareviewProgress(line));
        };
        let result = puffer_core::ultrareview::run_review_blocking(
            &cwd,
            &pr_arg,
            base_url,
            api_key,
            &progress,
            &worker_cancel,
        )
        .map_err(|error| error.to_string());
        let _ = event_tx.send(PendingSubmitEvent::UltrareviewFinished(result));
    });

    tui.pending_submit = Some(PendingSubmit {
        prompt: submitted.to_string(),
        receiver: event_rx,
        transcript_persisted_len: state.transcript.len(),
        rendered_tool_invocations: 0,
        pending_tool_calls: Vec::new(),
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: Some("ultrareview: starting…".to_string()),
        cancel,
    });
    Ok(())
}

fn emit_system_line(state: &mut AppState, session_store: &SessionStore, text: &str) -> Result<()> {
    state.push_message(MessageRole::System, text.to_string());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::SystemMessage {
            text: text.to_string(),
            actor: Some(state.system_actor()),
        },
    )?;
    Ok(())
}
