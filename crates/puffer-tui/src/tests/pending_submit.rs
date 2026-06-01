use super::*;
use crate::flow::poll_pending_submit;
use crate::state::{PendingSubmit, PendingSubmitEvent, PendingSubmitResult};
use puffer_core::{ToolInvocation, TurnExecution};
use puffer_provider_registry::AuthStore;
use puffer_session_store::TranscriptEvent;
use std::sync::mpsc;

#[test]
fn failed_pending_turn_discards_streamed_assistant_draft() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    state.push_message(MessageRole::User, "fail after partial".to_string());
    session_store
        .append_event(
            state.session.id,
            TranscriptEvent::UserMessage {
                text: "fail after partial".to_string(),
                actor: Some(state.user_actor()),
            },
        )
        .unwrap();
    let transcript_start_len = state.transcript.len();
    let (sender, receiver) = mpsc::channel();
    tui.pending_submit = Some(PendingSubmit {
        prompt: "fail after partial".to_string(),
        receiver,
        transcript_persisted_len: transcript_start_len,
        pending_tool_calls: Vec::new(),
        rendered_tool_invocations: 0,
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: None,
        cancel: puffer_core::CancelToken::new(),
    });

    sender
        .send(PendingSubmitEvent::TextDelta("partial answer".to_string()))
        .unwrap();
    sender
        .send(PendingSubmitEvent::Finished(PendingSubmitResult {
            outcome: Err("network down".to_string()),
            auth_store: auth_store.clone(),
            session_permission_state: Default::default(),
            session_allow_all: false,
            project_memory_review_turns: 0,
            autodream_review_turns: 0,
            autodream_suggest_skill: false,
        }))
        .unwrap();

    let completed = poll_pending_submit(
        &mut state,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
    )
    .unwrap();

    assert!(completed);
    assert!(!state.transcript.iter().any(|message| {
        message.role == MessageRole::Assistant && message.text.contains("partial answer")
    }));
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::System && message.text.contains("network down")
    }));

    let record = session_store.load_session(state.session.id).unwrap();
    assert!(!record.events.iter().any(|event| {
        matches!(
            event,
            TranscriptEvent::AssistantMessage { text, .. } if text.contains("partial answer")
        )
    }));
    assert!(record.events.iter().any(|event| {
        matches!(
            event,
            TranscriptEvent::SystemMessage { text, .. } if text.contains("network down")
        )
    }));
}

#[test]
fn successful_tool_turn_persists_pre_tool_assistant_draft() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    state.push_message(MessageRole::User, "inspect the file".to_string());
    session_store
        .append_event(
            state.session.id,
            TranscriptEvent::UserMessage {
                text: "inspect the file".to_string(),
                actor: Some(state.user_actor()),
            },
        )
        .unwrap();
    let transcript_start_len = state.transcript.len();
    let (sender, receiver) = mpsc::channel();
    tui.pending_submit = Some(PendingSubmit {
        prompt: "inspect the file".to_string(),
        receiver,
        transcript_persisted_len: transcript_start_len,
        pending_tool_calls: Vec::new(),
        rendered_tool_invocations: 0,
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: None,
        cancel: puffer_core::CancelToken::new(),
    });

    let invocation = ToolInvocation {
        call_id: "tool-call-1".to_string(),
        tool_id: "read_file".to_string(),
        input: "{\"path\":\"src/main.rs\"}".to_string(),
        output: "fn main() {}".to_string(),
        success: true,
        metadata: serde_json::Value::Null,
        terminate: false,
    };
    sender
        .send(PendingSubmitEvent::TextDelta(
            "I'll inspect that first.".to_string(),
        ))
        .unwrap();
    sender
        .send(PendingSubmitEvent::ToolInvocations(
            vec![invocation.clone()],
        ))
        .unwrap();
    sender
        .send(PendingSubmitEvent::TextDelta("Done.".to_string()))
        .unwrap();
    sender
        .send(PendingSubmitEvent::Finished(PendingSubmitResult {
            outcome: Ok(TurnExecution {
                assistant_text: "Done.".to_string(),
                tool_invocations: vec![invocation],
                reflection_traces: Vec::new(),
            }),
            auth_store: auth_store.clone(),
            session_permission_state: Default::default(),
            session_allow_all: false,
            project_memory_review_turns: 0,
            autodream_review_turns: 0,
            autodream_suggest_skill: false,
        }))
        .unwrap();

    let completed = poll_pending_submit(
        &mut state,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
    )
    .unwrap();

    assert!(completed);
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::Assistant && message.text.contains("I'll inspect that first.")
    }));

    let record = session_store.load_session(state.session.id).unwrap();
    let events = record.events;
    let pre_tool_index = events
        .iter()
        .position(|event| {
            matches!(
                event,
                TranscriptEvent::AssistantMessage { text, .. }
                    if text.contains("I'll inspect that first.")
            )
        })
        .expect("pre-tool assistant text persisted");
    let tool_index = events
        .iter()
        .position(|event| {
            matches!(
                event,
                TranscriptEvent::ToolInvocation { call_id, .. } if call_id == "tool-call-1"
            )
        })
        .expect("tool invocation persisted");
    let final_index = events
        .iter()
        .position(|event| {
            matches!(
                event,
                TranscriptEvent::AssistantMessage { text, .. } if text == "Done."
            )
        })
        .expect("final assistant text persisted");
    assert!(pre_tool_index < tool_index);
    assert!(tool_index < final_index);
}
