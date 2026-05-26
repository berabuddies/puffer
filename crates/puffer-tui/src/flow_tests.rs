use super::flow_loop::*;
use super::*;
use crate::state::{LoopKind, PendingSubmit, PendingSubmitEvent, PendingSubmitResult};
use puffer_config::{
    ensure_workspace_dirs, save_user_config, ConfigPaths, MemoryConfig, PufferConfig,
};
use puffer_core::{ToolInvocation, TurnExecution};
use puffer_provider_registry::{AuthMode, ModelDescriptor, ProviderDescriptor};
use puffer_resources::{LoadedItem, SourceInfo, SourceKind, ToolSpec};
use puffer_session_store::{SessionMetadata, TranscriptEvent};
use serde_json::{json, Map};
use std::sync::mpsc;
use tempfile::tempdir;

fn sample_state(session: SessionMetadata, cwd: &Path) -> AppState {
    AppState::new(PufferConfig::default(), cwd.to_path_buf(), session)
}

fn isolated_paths(tempdir: &tempfile::TempDir) -> ConfigPaths {
    let workspace = tempdir.path().join("workspace");
    ConfigPaths {
        workspace_root: workspace.clone(),
        workspace_config_dir: workspace.join(".puffer"),
        user_config_dir: tempdir.path().join(".home/.puffer"),
        builtin_resources_dir: tempdir.path().join("builtin-resources"),
    }
}

fn transient_session(cwd: &Path) -> SessionMetadata {
    SessionMetadata {
        id: Default::default(),
        display_name: Some("Resume picker".to_string()),
        generated_title: None,
        cwd: cwd.to_path_buf(),
        created_at_ms: 1,
        updated_at_ms: 1,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    }
}

fn user_question_answer(question: &str, answer: &str) -> UserQuestionPromptResponse {
    UserQuestionPromptResponse {
        answers: Map::from_iter([(question.to_string(), json!(answer))]),
        annotations: Map::new(),
    }
}

fn auth_required_provider_registry() -> ProviderRegistry {
    let mut providers = ProviderRegistry::default();
    providers.register(ProviderDescriptor {
        id: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        default_api: "anthropic-messages".to_string(),
        auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            provider: "anthropic".to_string(),
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    });
    providers
}

fn openai_provider_registry() -> ProviderRegistry {
    let mut providers = ProviderRegistry::default();
    providers.register(ProviderDescriptor {
        id: "openai".to_string(),
        display_name: "OpenAI".to_string(),
        base_url: "https://api.openai.com".to_string(),
        default_api: "openai-responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            provider: "openai".to_string(),
            api: "openai-responses".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    });
    providers
}

fn resources_with_bash_tool() -> LoadedResources {
    LoadedResources {
        tools: vec![LoadedItem {
            value: ToolSpec {
                id: "bash".to_string(),
                name: "bash".to_string(),
                description: "Run shell commands".to_string(),
                handler: "bash".to_string(),
                aliases: Vec::new(),
                handler_args: Vec::new(),
                approval_policy: Some("on-request".to_string()),
                sandbox_policy: Some("workspace-write".to_string()),
                shared_lib: None,
                enabled_if: None,
                input_schema: None,
                metadata: Default::default(),
                display: Default::default(),
            },
            source_info: SourceInfo {
                path: "tools/bash.yaml".into(),
                kind: SourceKind::Builtin,
            },
        }],
        ..LoadedResources::default()
    }
}

#[test]
fn provider_prompt_detection_matches_interactive_surface() {
    assert!(is_provider_prompt_input("henlo"));
    assert!(is_provider_prompt_input(" review this diff "));
    assert!(!is_provider_prompt_input(""));
    assert!(!is_provider_prompt_input("/help"));
    assert!(!is_provider_prompt_input("!pwd"));
    assert!(!is_provider_prompt_input("login openai"));
    assert!(!is_provider_prompt_input("/logout"));
}

#[test]
fn startup_bypass_includes_local_read_only_commands() {
    for command in ["/diff", "/session", "/remote", "/config", "/permissions"] {
        assert!(
            allow_prompt_before_onboarding(command),
            "{command} should run before provider auth is complete"
        );
    }

    assert!(!allow_prompt_before_onboarding("/diff staged"));
    assert!(!allow_prompt_before_onboarding("review this diff"));
    assert!(!allow_prompt_before_onboarding("/login"));
}

#[test]
fn queued_startup_diff_runs_before_missing_auth_onboarding() {
    let tempdir = tempdir().unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(tempdir.path())
        .arg("init")
        .arg("-q")
        .status()
        .expect("git init");
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    save_user_config(&paths, &PufferConfig::default()).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
    let mut resources = LoadedResources::default();
    let mut providers = auth_required_provider_registry();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();
    tui.defer_prompt(Some("/diff".to_string()));

    submit_queued_prompt_if_ready(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(tui.deferred_prompt.is_none());
    assert!(tui.overlay.is_none());
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::System && message.text.contains("Working tree is clean")
    }));
}

#[test]
fn queued_startup_session_panel_opens_before_missing_auth_onboarding() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    save_user_config(&paths, &PufferConfig::default()).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
    let mut resources = resources_with_bash_tool();
    let mut providers = auth_required_provider_registry();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();
    tui.defer_prompt(Some("/session".to_string()));

    submit_queued_prompt_if_ready(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(tui.deferred_prompt.is_none());
    assert!(matches!(tui.overlay, Some(OverlayState::Text(..))));
}

#[test]
fn provider_prompt_after_startup_overlay_reenters_missing_auth_onboarding() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
    let mut resources = LoadedResources::default();
    let mut providers = auth_required_provider_registry();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_startup_bypass_prompt(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "/help".to_string(),
        true,
    )
    .unwrap();
    assert!(matches!(tui.overlay, Some(OverlayState::Help)));
    tui.overlay = None;

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "hello after help".to_string(),
        true,
    )
    .unwrap();

    assert!(!tui.has_pending_submit());
    assert_eq!(tui.deferred_prompt.as_deref(), Some("hello after help"));
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::AuthPicker {
            onboarding: true,
            ..
        })
    ));
    assert!(state.transcript.is_empty());
}

#[test]
fn handle_prompt_submit_starts_async_provider_turn_and_polls_result() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "henlo".to_string(),
        true,
    )
    .unwrap();

    assert!(tui.has_pending_submit());
    assert!(matches!(state.transcript.first(), Some(message) if message.text == "henlo"));

    let mut completed = false;
    for _ in 0..20 {
        if poll_pending_submit(
            &mut state,
            &mut auth_store,
            &auth_path,
            &session_store,
            &mut tui,
        )
        .unwrap()
        {
            completed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(completed);
    assert!(!tui.has_pending_submit());
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::System && message.text.starts_with("Provider request failed:")
    }));
}

#[test]
fn handle_prompt_submit_routes_connect_through_user_question_worker() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "/connect".to_string(),
        true,
    )
    .unwrap();

    assert!(tui.has_pending_submit());
    assert_eq!(
        tui.pending_submit
            .as_ref()
            .and_then(|pending| pending.status_hint.as_deref()),
        Some("Connecting...")
    );

    let mut opened_connector_question = false;
    for _ in 0..100 {
        let completed = poll_pending_submit(
            &mut state,
            &mut auth_store,
            &auth_path,
            &session_store,
            &mut tui,
        )
        .unwrap();
        if let Some(OverlayState::UserQuestionPrompt { overlay }) = &tui.overlay {
            opened_connector_question = overlay.title().contains("Which connector should");
            break;
        }
        if completed {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(opened_connector_question);
    assert!(respond_to_user_question(
        &mut tui,
        user_question_answer(
            "Which connector should Puffer connect?",
            "missing-connector"
        ),
    ));

    let mut completed = false;
    for _ in 0..100 {
        let step_completed = poll_pending_submit(
            &mut state,
            &mut auth_store,
            &auth_path,
            &session_store,
            &mut tui,
        )
        .unwrap();
        if let Some(OverlayState::UserQuestionPrompt { overlay }) = &tui.overlay {
            assert!(!overlay.title().contains("exact connection name"));
        }
        if step_completed {
            completed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(completed);
    assert!(!tui.has_pending_submit());
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::Assistant && message.text.starts_with("/connect failed:")
    }));
    assert!(!state
        .transcript
        .iter()
        .any(|message| message.text.contains("Provider request failed")));
    let record = session_store.load_session(state.session.id).unwrap();
    assert!(record.events.iter().any(|event| {
        matches!(
            event,
            TranscriptEvent::CommandInvoked { name, args, .. }
                if name == "connect" && args.is_empty()
        )
    }));
}

#[test]
fn handle_prompt_submit_routes_monitor_through_user_question_worker() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "/monitor".to_string(),
        true,
    )
    .unwrap();

    assert!(tui.has_pending_submit());
    assert_eq!(
        tui.pending_submit
            .as_ref()
            .and_then(|pending| pending.status_hint.as_deref()),
        Some("Setting up monitor...")
    );
    assert!(state.transcript.iter().all(|message| {
        !message
            .text
            .contains("interactive AskUserQuestion response is required for /monitor")
    }));
    let record = session_store.load_session(state.session.id).unwrap();
    assert!(record.events.iter().any(|event| {
        matches!(
            event,
            TranscriptEvent::CommandInvoked { name, args, .. }
                if name == "monitor" && args.is_empty()
        )
    }));
}

#[test]
fn transient_resume_picker_selection_does_not_create_blank_session() {
    let tempdir = tempdir().unwrap();
    let paths = isolated_paths(&tempdir);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let target_session = session_store
        .create_session(paths.workspace_root.clone())
        .unwrap();
    let mut state = sample_state(
        transient_session(&paths.workspace_root),
        &paths.workspace_root,
    );
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();

    handle_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        format!("/resume {}", target_session.id),
        true,
    )
    .unwrap();

    assert_eq!(state.session.id, target_session.id);
    assert_eq!(session_store.list_sessions().unwrap().len(), 1);
}

#[test]
fn transient_resume_picker_new_prompt_creates_session_lazily() {
    let tempdir = tempdir().unwrap();
    let paths = isolated_paths(&tempdir);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let mut state = sample_state(
        transient_session(&paths.workspace_root),
        &paths.workspace_root,
    );
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "hello from a fresh thread".to_string(),
        true,
    )
    .unwrap();

    assert!(!state.session.id.is_nil());
    assert_eq!(session_store.list_sessions().unwrap().len(), 1);
    assert!(tui.has_pending_submit());

    let mut completed = false;
    for _ in 0..20 {
        if poll_pending_submit(
            &mut state,
            &mut auth_store,
            &auth_path,
            &session_store,
            &mut tui,
        )
        .unwrap()
        {
            completed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(completed);
    assert!(!tui.has_pending_submit());
}

#[test]
fn handle_prompt_submit_queues_prompt_while_turn_is_running() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "first".to_string(),
        true,
    )
    .unwrap();
    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "second".to_string(),
        true,
    )
    .unwrap();

    assert!(tui.has_pending_submit());
    assert_eq!(tui.queued_prompts.len(), 1);
    assert_eq!(
        tui.queued_prompts.front().map(String::as_str),
        Some("second")
    );
    assert!(matches!(state.transcript.first(), Some(message) if message.text == "first"));
}

#[test]
fn handle_prompt_submit_queues_mutating_slash_while_turn_is_running() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "first".to_string(),
        true,
    )
    .unwrap();
    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "/clear".to_string(),
        true,
    )
    .unwrap();

    assert!(tui.has_pending_submit());
    assert_eq!(tui.queued_prompts.len(), 1);
    assert_eq!(
        tui.queued_prompts.front().map(String::as_str),
        Some("/clear")
    );
    assert!(state
        .transcript
        .iter()
        .any(|message| message.role == MessageRole::User && message.text == "first"));
    assert!(!state
        .transcript
        .iter()
        .any(|message| message.text == "Transcript cleared."));
}

#[test]
fn handle_prompt_submit_queues_shell_shortcut_while_turn_is_running() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "first".to_string(),
        true,
    )
    .unwrap();
    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "!printf queued-shell".to_string(),
        true,
    )
    .unwrap();

    assert!(tui.has_pending_submit());
    assert_eq!(
        tui.queued_prompts.front().map(String::as_str),
        Some("!printf queued-shell")
    );
    assert!(!state
        .transcript
        .iter()
        .any(|message| message.text.contains("queued-shell")));
}

#[test]
fn handle_prompt_submit_allows_read_only_slash_while_turn_is_running() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "first".to_string(),
        true,
    )
    .unwrap();
    assert!(!should_defer_while_turn_is_running("/status"));
    let opened = try_open_overlay(
        &state,
        &resources,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/status",
    )
    .unwrap();

    assert!(opened);
    assert!(tui.has_pending_submit());
    assert!(tui.queued_prompts.is_empty());
    assert!(matches!(tui.overlay, Some(OverlayState::Status(..))));
}

#[test]
fn queued_empty_model_command_opens_picker_after_turn_finishes() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("gpt-5".to_string());
    let mut resources = LoadedResources::default();
    let mut providers = openai_provider_registry();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();
    let (_sender, receiver) = mpsc::channel();
    tui.pending_submit = Some(PendingSubmit {
        prompt: "first".to_string(),
        receiver,
        transcript_persisted_len: state.transcript.len(),
        pending_tool_calls: Vec::new(),
        rendered_tool_invocations: 0,
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: None,
        cancel: puffer_core::CancelToken::new(),
    });

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "/model".to_string(),
        true,
    )
    .unwrap();
    assert_eq!(
        tui.queued_prompts.front().map(String::as_str),
        Some("/model")
    );

    tui.pending_submit = None;
    assert!(submit_next_queued_prompt(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap());

    match tui.overlay {
        Some(OverlayState::ModelPicker {
            provider_id,
            entries,
            ..
        }) => {
            assert_eq!(provider_id, "openai");
            assert!(entries.iter().any(|entry| entry.selector == "gpt-5"));
        }
        other => panic!("expected model picker, got {other:?}"),
    }
    assert!(!state
        .transcript
        .iter()
        .any(|message| message.text.contains("Current model")));
}

#[test]
fn poll_pending_submit_syncs_project_memory_review_turns_back_to_main_state() {
    let tempdir = tempdir().unwrap();
    let _home = crate::test_env::ScopedPufferHome::new("memory-review-turn-sync");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    std::fs::write(
        paths.projects_file(),
        format!(
            "[[projects]]\nname = \"demo\"\npath = \"{}\"\n",
            workspace.display()
        ),
    )
    .unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(workspace.to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig {
            memory: MemoryConfig {
                review_nudge_interval: 2,
                ..MemoryConfig::default()
            },
            ..PufferConfig::default()
        },
        workspace.to_path_buf(),
        session,
    );
    assert!(state.project_memory.is_some());
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();
    let (sender, receiver) = mpsc::channel();
    tui.pending_submit = Some(PendingSubmit {
        prompt: "remember this".to_string(),
        receiver,
        transcript_persisted_len: state.transcript.len(),
        pending_tool_calls: Vec::new(),
        rendered_tool_invocations: 0,
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: None,
        cancel: puffer_core::CancelToken::new(),
    });
    sender
        .send(PendingSubmitEvent::Finished(PendingSubmitResult {
            outcome: Ok(TurnExecution {
                assistant_text: "ack".to_string(),
                tool_invocations: Vec::new(),
                reflection_traces: Vec::new(),
            }),
            auth_store: auth_store.clone(),
            session_permission_state: Default::default(),
            session_allow_all: false,
            project_memory_review_turns: 1,
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
    assert_eq!(state.project_memory_review_turns, 1);
    assert!(state
        .transcript
        .iter()
        .any(|message| message.role == MessageRole::Assistant && message.text == "ack"));
}

#[test]
fn slash_plan_with_arguments_starts_async_turn_after_local_handling() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "/plan stabilize slash-command parity".to_string(),
        true,
    )
    .unwrap();
    submit_queued_prompt_if_ready(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(state.plan_mode);
    assert!(tui.has_pending_submit());
    assert!(state
        .transcript
        .iter()
        .any(|message| message.text == "Enabled plan mode"));
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::User && message.text == "stabilize slash-command parity"
    }));
}

#[test]
fn cancel_pending_submit_records_interrupt_and_starts_next_queued_prompt() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "first".to_string(),
        true,
    )
    .unwrap();
    handle_prompt_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        "second".to_string(),
        true,
    )
    .unwrap();

    assert!(cancel_pending_submit(&mut state, &session_store, &mut tui).unwrap());
    assert!(!tui.has_pending_submit());
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::System && message.text == "Interrupted by user."
    }));

    assert!(submit_next_queued_prompt(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap());
    assert!(tui.has_pending_submit());
    assert!(tui.queued_prompts.is_empty());
    assert!(state
        .transcript
        .iter()
        .any(|message| { message.role == MessageRole::User && message.text == "second" }));
}

#[test]
fn submit_next_queued_prompt_waits_for_open_overlay() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState {
        overlay: Some(OverlayState::Help),
        ..TuiState::default()
    };
    tui.enqueue_prompt("second".to_string());

    assert!(!submit_next_queued_prompt(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap());
    assert!(!tui.has_pending_submit());
    assert_eq!(
        tui.queued_prompts.front().map(String::as_str),
        Some("second")
    );

    tui.overlay = None;
    assert!(submit_next_queued_prompt(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap());
    assert!(tui.has_pending_submit());
    assert!(tui.queued_prompts.is_empty());
    assert!(state
        .transcript
        .iter()
        .any(|message| { message.role == MessageRole::User && message.text == "second" }));
}

#[test]
fn submit_next_queued_prompt_starts_shell_shortcut_as_pending_submit() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = resources_with_bash_tool();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();
    tui.enqueue_prompt("!printf queued-shell".to_string());

    assert!(submit_next_queued_prompt(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
        true,
    )
    .unwrap());
    assert!(tui.has_pending_submit());
    assert!(tui.queued_prompts.is_empty());
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::User && message.text == "!printf queued-shell"
    }));
    assert!(!state
        .transcript
        .iter()
        .any(|message| message.text == "queued-shell"));

    let mut completed = false;
    for _ in 0..100 {
        if poll_pending_submit(
            &mut state,
            &mut auth_store,
            &auth_path,
            &session_store,
            &mut tui,
        )
        .unwrap()
        {
            completed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert!(completed);
    assert!(!tui.has_pending_submit());
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::Assistant && message.text == "queued-shell"
    }));
}

#[test]
fn failed_shell_shortcut_persists_as_system_message() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut resources = resources_with_bash_tool();
    let mut providers = ProviderRegistry::new();
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();

    handle_submit(
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        "!printf shell-failed >&2; exit 7".to_string(),
        true,
    )
    .unwrap();

    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::System && message.text.contains("shell-failed")
    }));

    let record = session_store.load_session(state.session.id).unwrap();
    assert!(record.events.iter().any(|event| {
        matches!(
            event,
            TranscriptEvent::SystemMessage { text, .. } if text.contains("shell-failed")
        )
    }));
    assert!(!record.events.iter().any(|event| {
        matches!(
            event,
            TranscriptEvent::AssistantMessage { text, .. } if text.contains("shell-failed")
        )
    }));
}

#[test]
fn poll_pending_submit_skips_empty_assistant_message_after_tool_only_turn() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();
    let (sender, receiver) = mpsc::channel();
    tui.pending_submit = Some(PendingSubmit {
        prompt: "run the tool".to_string(),
        receiver,
        transcript_persisted_len: state.transcript.len(),
        pending_tool_calls: Vec::new(),
        rendered_tool_invocations: 0,
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: None,
        cancel: puffer_core::CancelToken::new(),
    });

    sender
        .send(PendingSubmitEvent::Finished(PendingSubmitResult {
            outcome: Ok(TurnExecution {
                assistant_text: String::new(),
                tool_invocations: vec![ToolInvocation {
                    call_id: "tool-call-1".to_string(),
                    tool_id: "bash".to_string(),
                    input: "true".to_string(),
                    output: "ok".to_string(),
                    success: true,
                    terminate: true,
                }],
                reflection_traces: Vec::new(),
            }),
            auth_store: auth_store.clone(),
            session_permission_state: Default::default(),
            session_allow_all: false,
            project_memory_review_turns: 0,
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
    assert!(!state
        .transcript
        .iter()
        .any(|message| message.role == MessageRole::Assistant && message.text.is_empty()));

    let record = session_store.load_session(state.session.id).unwrap();
    assert!(record.events.iter().any(|event| {
        matches!(
            event,
            TranscriptEvent::ToolInvocation { call_id, .. } if call_id == "tool-call-1"
        )
    }));
    assert!(!record.events.iter().any(|event| {
        matches!(
            event,
            TranscriptEvent::AssistantMessage { text, .. } if text.is_empty()
        )
    }));
}

#[test]
fn poll_pending_submit_preserves_browser_category_session_grants() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut tui = TuiState::default();

    let browser = puffer_tools::ToolDefinition {
        id: "Browser".to_string(),
        name: "Browser".to_string(),
        description: String::new(),
        handler: String::new(),
        aliases: Vec::new(),
        handler_args: Vec::new(),
        kind: puffer_tools::ToolKind::Custom,
        input_schema: puffer_tools::ToolInputSchema::default(),
        metadata: puffer_tools::ToolMetadata::default(),
        policy: puffer_tools::ToolPolicyHints::default(),
        shared_lib: None,
        enabled_if: None,
        display: puffer_tools::ToolDisplayHints::default(),
    };
    state.allow_permission_for_tool_call(
        &browser,
        &serde_json::json!({"action": "evaluate", "script": "document.title"}),
    );
    let worker_permission_state = state.session_permission_state().clone();

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    event_tx
        .send(PendingSubmitEvent::Finished(PendingSubmitResult {
            outcome: Err("cancelled".to_string()),
            auth_store: auth_store.clone(),
            session_permission_state: worker_permission_state,
            session_allow_all: false,
            project_memory_review_turns: 0,
        }))
        .unwrap();

    tui.pending_submit = Some(PendingSubmit {
        prompt: "hi".to_string(),
        receiver: event_rx,
        transcript_persisted_len: state.transcript.len(),
        rendered_tool_invocations: 0,
        pending_tool_calls: Vec::new(),
        started_at: std::time::Instant::now(),
        thinking_active: false,
        status_hint: None,
        cancel: puffer_core::CancelToken::new(),
    });

    let completed = poll_pending_submit(
        &mut state,
        &mut auth_store,
        &auth_path,
        &session_store,
        &mut tui,
    )
    .unwrap();

    assert!(completed);
    assert!(state.session_permission_state().has_browser_grant());
    let projection = state
        .session_permission_state()
        .legacy_snapshot_projection();
    assert!(!projection.1.contains_key("browser"));
}

// ---------------------------------------------------------------------------
// Loop / Maximize / Minimize tests
// ---------------------------------------------------------------------------

#[test]
fn parse_loop_args_extracts_interval_and_prompt() {
    let (d, p) = parse_loop_args("5m check deploy");
    assert_eq!(d, std::time::Duration::from_secs(300));
    assert_eq!(p, "check deploy");

    let (d, p) = parse_loop_args("30s ping server");
    assert_eq!(d, std::time::Duration::from_secs(30));
    assert_eq!(p, "ping server");

    let (d, p) = parse_loop_args("2h run maintenance");
    assert_eq!(d, std::time::Duration::from_secs(7200));
    assert_eq!(p, "run maintenance");

    // No interval → defaults to 10 minutes, whole input is prompt.
    let (d, p) = parse_loop_args("check deploy");
    assert_eq!(d, std::time::Duration::from_secs(600));
    assert_eq!(p, "check deploy");
}

#[test]
fn parse_duration_handles_all_suffixes() {
    assert_eq!(
        parse_duration("10s"),
        Some(std::time::Duration::from_secs(10))
    );
    assert_eq!(
        parse_duration("5m"),
        Some(std::time::Duration::from_secs(300))
    );
    assert_eq!(
        parse_duration("1h"),
        Some(std::time::Duration::from_secs(3600))
    );
    assert_eq!(
        parse_duration("2d"),
        Some(std::time::Duration::from_secs(172800))
    );
    assert_eq!(parse_duration("abc"), None);
    assert_eq!(parse_duration(""), None);
}

#[test]
fn extract_metric_value_parses_marker() {
    let text = "I improved the test suite.\n[[METRIC:accuracy=0.85]]\nDone.";
    assert_eq!(extract_metric_value(text, "accuracy"), Some(0.85));

    let text = "[[METRIC:latency = 12.5 ]]";
    assert_eq!(extract_metric_value(text, "latency"), Some(12.5));

    let text = "No metric here";
    assert_eq!(extract_metric_value(text, "accuracy"), None);
}

#[test]
fn has_converged_detects_plateau() {
    assert!(!has_converged(&[]));
    assert!(!has_converged(&[1.0, 2.0]));
    assert!(!has_converged(&[1.0, 2.0, 3.0]));
    assert!(has_converged(&[3.0, 3.0, 3.0]));
    assert!(has_converged(&[1.0, 2.0, 3.0, 3.0, 3.0]));
}

#[test]
fn build_optimization_prompt_includes_context() {
    let prompt = build_optimization_prompt("fix tests", "accuracy", true, 3, &[0.5, 0.7]);
    assert!(prompt.contains("maximize"));
    assert!(prompt.contains("accuracy"));
    assert!(prompt.contains("iteration 3/"));
    assert!(prompt.contains("0.5000"));
    assert!(prompt.contains("0.7000"));
    assert!(prompt.contains("fix tests"));
    assert!(prompt.contains("[[METRIC:accuracy="));
    // User prompt should come before optimization context
    let user_pos = prompt.find("fix tests").unwrap();
    let ctx_pos = prompt.find("Optimization context").unwrap();
    assert!(
        user_pos < ctx_pos,
        "user prompt should precede optimization context"
    );
}

#[test]
fn try_handle_loop_command_creates_loop_state() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut tui = TuiState::default();

    // Non-loop command returns false.
    assert!(!try_handle_loop_command(&mut state, &session_store, &mut tui, "/help").unwrap());

    // Loop command creates state and enqueues prompt.
    assert!(
        try_handle_loop_command(&mut state, &session_store, &mut tui, "/loop 10s echo hi").unwrap()
    );
    assert!(tui.active_loop.is_some());
    let ls = tui.active_loop.as_ref().unwrap();
    assert!(matches!(ls.kind, LoopKind::Loop));
    assert_eq!(ls.prompt, "echo hi");
    assert_eq!(ls.interval, Some(std::time::Duration::from_secs(10)));
    assert_eq!(tui.queued_prompts.len(), 1);
}

#[test]
fn try_handle_loop_command_creates_maximize_state() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut tui = TuiState::default();

    assert!(try_handle_loop_command(
        &mut state,
        &session_store,
        &mut tui,
        "/maximize accuracy run bench"
    )
    .unwrap());
    assert!(tui.active_loop.is_some());
    let ls = tui.active_loop.as_ref().unwrap();
    assert!(matches!(ls.kind, LoopKind::Maximize(ref m) if m == "accuracy"));
    assert_eq!(ls.prompt, "run bench");
    assert_eq!(tui.queued_prompts.len(), 1);
    let enqueued = &tui.queued_prompts[0];
    assert!(enqueued.contains("maximize"));
    assert!(enqueued.contains("[[METRIC:accuracy="));
}

#[test]
fn maybe_apply_requested_reload_swallows_parse_errors_as_system_message() {
    // Regression: a malformed YAML file under `.puffer/resources/`
    // (e.g. user mid-edit, atomic-rename save catching the file in a
    // transient invalid state) must not propagate out of
    // `maybe_apply_requested_reload`. The watcher fires after every
    // save; a single bad save would otherwise kill the TUI and discard
    // the in-memory transcript. The reload error should surface as a
    // system message instead.
    let tempdir = tempdir().unwrap();
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let paths = ConfigPaths {
        workspace_root: workspace.clone(),
        workspace_config_dir: workspace.join(".puffer"),
        user_config_dir: tempdir.path().join(".home/.puffer"),
        builtin_resources_dir: tempdir.path().join("builtin-resources"),
    };
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut state = sample_state(session, &workspace);
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_store = AuthStore::default();

    // Drop a syntactically broken MCP manifest into the watched dir.
    let mcp_dir = paths.workspace_config_dir.join("resources/mcp_servers");
    std::fs::create_dir_all(&mcp_dir).unwrap();
    std::fs::write(
        mcp_dir.join("busted.yaml"),
        "id: broken\nthis is: : invalid: yaml\n  - structure\n",
    )
    .unwrap();

    // Simulate a watcher-driven reload request.
    state
        .reload_signal()
        .store(true, std::sync::atomic::Ordering::Release);

    // Must NOT propagate — the TUI loop calls this with `?` and a
    // returned `Err` would crash `run_app` and lose the session.
    let result = maybe_apply_requested_reload(
        &mut state,
        &mut resources,
        &mut providers,
        &auth_store,
        &session_store,
    );
    assert!(
        result.is_ok(),
        "reload parse error must be swallowed, got: {result:?}"
    );

    // The transcript should now contain a system message describing
    // the failure so the user knows what happened.
    let last = state.transcript.last().expect("system message appended");
    assert!(matches!(last.role, MessageRole::System));
    assert!(
        last.text.contains("Resource hot-reload failed"),
        "expected reload-failure system message, got: {}",
        last.text
    );

    // And the signal must have been consumed so we don't spin trying
    // to reload the same broken file every loop tick.
    assert!(!state.take_reload_request());
}

#[test]
fn maybe_apply_requested_reload_no_op_when_no_signal_pending() {
    // Sanity: when neither the in-loop flag nor the watcher signal is
    // set, the reload helper is a cheap no-op and doesn't touch the
    // transcript. This is the dominant code path on every TUI loop
    // tick — must stay free of side effects.
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let initial_len = state.transcript.len();
    let mut resources = LoadedResources::default();
    let mut providers = ProviderRegistry::new();
    let auth_store = AuthStore::default();

    maybe_apply_requested_reload(
        &mut state,
        &mut resources,
        &mut providers,
        &auth_store,
        &session_store,
    )
    .unwrap();
    assert_eq!(state.transcript.len(), initial_len);
}

#[test]
fn stop_command_clears_active_loop() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = sample_state(session, tempdir.path());
    let mut tui = TuiState::default();

    try_handle_loop_command(&mut state, &session_store, &mut tui, "/loop 10s echo hi").unwrap();
    assert!(tui.active_loop.is_some());

    try_handle_loop_command(&mut state, &session_store, &mut tui, "/loop stop").unwrap();
    assert!(tui.active_loop.is_none());
}
