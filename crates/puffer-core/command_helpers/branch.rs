use super::emit_system;
use crate::plans::copy_plan_for_fork;
use crate::{AppState, MessageRole};
use anyhow::Result;
use puffer_session_store::{SessionStore, SessionSummary};

const DEFAULT_BRANCH_TITLE: &str = "Branched conversation";

/// Forks the current session and switches the active state to the new branch.
pub(crate) fn handle_branch_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let explicit_title = args.trim();
    if state.transcript.is_empty() && explicit_title.is_empty() {
        return emit_system(
            state,
            session_store,
            "Failed to branch conversation: No conversation to branch".to_string(),
        );
    }

    let original_session_id = state.session.id;
    let base_name = if explicit_title.is_empty() {
        derive_branch_base_name(state)
    } else {
        explicit_title.to_string()
    };
    let branch_name = unique_branch_name(&base_name, &session_store.list_sessions()?);

    let fork = session_store.fork_session(original_session_id, state.cwd.clone())?;
    session_store.rename_session(fork.id, branch_name.clone())?;
    let record = session_store.load_session(fork.id)?;
    let config = state.config.clone();
    let original_state = state.clone();
    *state = AppState::from_session_record(config, record);
    let _ = copy_plan_for_fork(&original_state, state)?;
    state.remote_name = None;
    state.remote_session_id = None;
    state.remote_session_url = None;
    state.remote_session_status = None;

    let message = if explicit_title.is_empty() {
        format!(
            "Branched conversation. You are now in the branch.\nTo resume the original: /resume {original_session_id}"
        )
    } else {
        format!(
            "Branched conversation \"{explicit_title}\". You are now in the branch.\nTo resume the original: /resume {original_session_id}"
        )
    };
    emit_system(state, session_store, message)
}

fn derive_branch_base_name(state: &AppState) -> String {
    let Some(first_user_message) = state
        .transcript
        .iter()
        .find(|message| message.role == MessageRole::User)
    else {
        return DEFAULT_BRANCH_TITLE.to_string();
    };
    let collapsed = first_user_message
        .text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        return DEFAULT_BRANCH_TITLE.to_string();
    }
    let mut shortened = String::new();
    for ch in collapsed.chars().take(100) {
        shortened.push(ch);
    }
    if shortened.is_empty() {
        DEFAULT_BRANCH_TITLE.to_string()
    } else {
        shortened
    }
}

fn unique_branch_name(base_name: &str, sessions: &[SessionSummary]) -> String {
    let preferred = format!("{base_name} (Branch)");
    if !sessions
        .iter()
        .any(|session| session.display_name.as_deref() == Some(preferred.as_str()))
    {
        return preferred;
    }

    let mut next_number = 2usize;
    loop {
        let candidate = format!("{base_name} (Branch {next_number})");
        if !sessions
            .iter()
            .any(|session| session.display_name.as_deref() == Some(candidate.as_str()))
        {
            return candidate;
        }
        next_number += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{derive_branch_base_name, unique_branch_name, DEFAULT_BRANCH_TITLE};
    use crate::{AppState, MessageRole};
    use puffer_config::PufferConfig;
    use puffer_session_store::{SessionMetadata, SessionSummary};
    use std::path::PathBuf;
    use uuid::Uuid;

    fn state_with_messages(messages: &[(&str, MessageRole)]) -> AppState {
        let mut state = AppState::new(
            PufferConfig::default(),
            PathBuf::from("."),
            SessionMetadata {
                id: Uuid::nil(),
                display_name: None,
                cwd: PathBuf::from("."),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        );
        for (text, role) in messages {
            state.push_message(role.clone(), (*text).to_string());
        }
        state
    }

    fn summary(name: &str) -> SessionSummary {
        SessionSummary {
            id: Uuid::new_v4(),
            display_name: Some(name.to_string()),
            cwd: PathBuf::from("."),
            created_at_ms: 0,
            updated_at_ms: 0,
            event_count: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        }
    }

    #[test]
    fn derive_branch_base_name_uses_first_user_message() {
        let state = state_with_messages(&[
            ("system note", MessageRole::System),
            ("  Start   with\nthis prompt  ", MessageRole::User),
            ("assistant", MessageRole::Assistant),
        ]);
        assert_eq!(derive_branch_base_name(&state), "Start with this prompt");
    }

    #[test]
    fn derive_branch_base_name_falls_back_when_missing_user_message() {
        let state = state_with_messages(&[("assistant", MessageRole::Assistant)]);
        assert_eq!(derive_branch_base_name(&state), DEFAULT_BRANCH_TITLE);
    }

    #[test]
    fn unique_branch_name_adds_number_for_collisions() {
        let sessions = vec![summary("Dockyard (Branch)"), summary("Dockyard (Branch 2)")];
        assert_eq!(
            unique_branch_name("Dockyard", &sessions),
            "Dockyard (Branch 3)"
        );
    }
}
