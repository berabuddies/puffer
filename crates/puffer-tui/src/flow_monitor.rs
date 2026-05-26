use anyhow::Result;
use puffer_core::{with_user_question_prompt_handler, AppState, UserQuestionPromptRequest};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, TranscriptEvent};
use std::sync::mpsc;
use std::thread;

use super::{empty_user_question_response, parsed_slash_command};
use crate::state::{PendingSubmit, PendingSubmitEvent, PendingSubmitResult};
use crate::TuiState;

/// Runs `/monitor` on a background thread so `AskUserQuestion` can render in the TUI.
pub(super) fn execute_monitor_command(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    session_store: &SessionStore,
    tui: &mut TuiState,
    submitted: String,
) -> Result<()> {
    let (_, args) = parsed_slash_command(&submitted);
    session_store.append_event(
        state.session.id,
        TranscriptEvent::CommandInvoked {
            name: "monitor".to_string(),
            args: args.to_string(),
            actor: Some(state.user_actor()),
        },
    )?;
    let mut worker_state = state.clone();
    let worker_resources = resources.clone();
    let worker_providers = providers.clone();
    let mut worker_auth_store = auth_store.clone();
    let worker_args = args.to_string();
    let (sender, receiver) = mpsc::channel();
    let cancel = puffer_core::CancelToken::new();
    thread::spawn(move || {
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
            puffer_core::execute_monitor_flow(
                &mut worker_state,
                &worker_resources,
                &worker_providers,
                &mut worker_auth_store,
                &worker_args,
            )
        })
        .or_else(|error| {
            Ok(puffer_core::TurnExecution {
                assistant_text: format!("/monitor failed: {error}"),
                tool_invocations: Vec::new(),
                reflection_traces: Vec::new(),
            })
        });
        let _ = sender.send(PendingSubmitEvent::Finished(PendingSubmitResult {
            outcome,
            auth_store: worker_auth_store,
            session_permission_state: worker_state.session_permission_state().clone(),
            session_allow_all: worker_state.session_permission_state().allow_all_tools(),
            project_memory_review_turns: worker_state.project_memory_review_turns,
        }));
    });
    tui.pending_submit = Some(PendingSubmit {
        prompt: submitted,
        receiver,
        transcript_persisted_len: state.transcript.len(),
        pending_tool_calls: Vec::new(),
        rendered_tool_invocations: 0,
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: Some("Setting up monitor...".to_string()),
        cancel,
    });
    Ok(())
}
