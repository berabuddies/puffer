use super::*;
use crate::RenderedMessage;
use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use tempfile::tempdir;

#[test]
fn diff_command_includes_recent_command_snapshots() {
    let tempdir = tempdir().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(tempdir.path())
        .output()
        .unwrap();
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
    let mut auth_store = AuthStore::default();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut auth_store,
        &session_store,
        "/theme harbor",
    )
    .unwrap();
    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut auth_store,
        &session_store,
        "/diff",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Recent turn-by-turn diff snapshots:")
            && text.contains("/theme harbor")
    ));
}

#[test]
fn remote_control_command_persists_remote_session_metadata() {
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
    let mut auth_store = AuthStore::default();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut auth_store,
        &session_store,
        "/remote-control dockyard",
    )
    .unwrap();
    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut auth_store,
        &session_store,
        "/remote-env staging",
    )
    .unwrap();

    assert_eq!(state.remote_name.as_deref(), Some("dockyard"));
    assert_eq!(state.remote_environment.as_deref(), Some("staging"));
    let expected_remote_id = state.session.id.to_string();
    assert_eq!(
        state.remote_session_id.as_deref(),
        Some(expected_remote_id.as_str())
    );
    assert!(state
        .remote_session_url
        .as_deref()
        .unwrap_or_default()
        .contains("staging"));

    let record = session_store.load_session(state.session.id).unwrap();
    let restored = AppState::from_session_record(PufferConfig::default(), record);
    assert_eq!(restored.remote_name.as_deref(), Some("dockyard"));
    assert_eq!(restored.remote_environment.as_deref(), Some("staging"));
    assert!(restored.remote_session_url.is_some());
}

#[test]
fn branch_clears_active_remote_session_connection() {
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
    state.remote_name = Some("dockyard".to_string());
    state.remote_environment = Some("staging".to_string());
    state.remote_session_id = Some("remote-1".to_string());
    state.remote_session_url =
        Some("puffer://remote/remote-1?name=dockyard&env=staging".to_string());
    state.remote_session_status = Some("connected".to_string());
    state.push_message(MessageRole::User, "Inspect the remote environment");
    session_store
        .append_event(
            state.session.id,
            TranscriptEvent::UserMessage {
                text: "Inspect the remote environment".to_string(),
                actor: None,
            },
        )
        .unwrap();
    let mut auth_store = AuthStore::default();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut auth_store,
        &session_store,
        "/branch remote-fork",
    )
    .unwrap();

    assert!(state.remote_name.is_none());
    assert!(state.remote_session_id.is_none());
    assert!(state.remote_session_url.is_none());
    assert!(state.remote_session_status.is_none());
}

#[test]
fn rewind_command_can_target_a_specific_user_turn() {
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
        session.clone(),
    );
    state.push_message(MessageRole::User, "first");
    state.push_message(MessageRole::Assistant, "a1");
    state.push_message(MessageRole::User, "second");
    state.push_message(MessageRole::Assistant, "a2");
    let mut auth_store = AuthStore::default();
    session_store
        .append_event(
            session.id,
            TranscriptEvent::UserMessage {
                text: "first".to_string(),
                actor: None,
            },
        )
        .unwrap();
    session_store
        .append_event(
            session.id,
            TranscriptEvent::AssistantMessage {
                text: "a1".to_string(),
                actor: None,
            },
        )
        .unwrap();
    session_store
        .append_event(
            session.id,
            TranscriptEvent::UserMessage {
                text: "second".to_string(),
                actor: None,
            },
        )
        .unwrap();
    session_store
        .append_event(
            session.id,
            TranscriptEvent::AssistantMessage {
                text: "a2".to_string(),
                actor: None,
            },
        )
        .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut auth_store,
        &session_store,
        "/rewind 2",
    )
    .unwrap();

    assert_eq!(state.transcript.len(), 3);
    assert_eq!(state.transcript[0].text, "first");
    assert_eq!(state.transcript[1].text, "a1");
    assert!(state.transcript[2]
        .text
        .contains("Rewound conversation to before user turn 2."));

    let record = session_store.load_session(session.id).unwrap();
    let restored = AppState::from_session_record(PufferConfig::default(), record);
    assert_eq!(restored.transcript.len(), 3);
    assert_eq!(restored.transcript[0].text, "first");
    assert_eq!(restored.transcript[1].text, "a1");
}
