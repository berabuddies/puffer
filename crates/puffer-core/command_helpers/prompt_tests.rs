use super::{
    build_night_directive, execute_compact_prompt_command, handle_plan_command,
    plan_mode_context_message, prepare_btw_prompt_command, prepare_commit_prompt_command,
    prepare_compact_prompt_command, prepare_plan_prompt_command, prepare_pr_comments_prompt_command,
    prepare_prompt_command_specialization, prepare_security_review_prompt_command,
    prepare_statusline_prompt_command, PromptCommandPreparation,
};
use crate::plans::{ensure_plan_file, persist_plan_output, plan_file_path};
use crate::{AppState, MessageRole};
use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_provider_registry::{AuthStore, ModelDescriptor, ProviderDescriptor, ProviderRegistry};
use puffer_resources::{
    LoadedItem, LoadedResources, PromptTemplate, SourceInfo, SourceKind, ToolSpec,
};
use puffer_session_store::SessionStore;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use tempfile::tempdir;
use tempfile::TempDir;

#[test]
fn compact_specialization_returns_prompt_override() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let session_store = fixture.session_store;
    state.push_message(MessageRole::User, "Ship this change.");
    state.push_message(MessageRole::Assistant, "Implemented and tested.");

    let outcome =
        prepare_compact_prompt_command(&mut state, &session_store, "focus on tests").unwrap();

    match outcome {
        PromptCommandPreparation::PromptOverride(prompt) => {
            assert!(prompt.contains("Summarize the conversation into a compact context block"));
            assert!(prompt.contains("Additional instruction: focus on tests"));
        }
        PromptCommandPreparation::DirectPrompt(_)
        | PromptCommandPreparation::HandledLocally
        | PromptCommandPreparation::SideQuestion(_)
        | PromptCommandPreparation::VariableOverrides(_) => {
            panic!("expected compact prompt override")
        }
    }
}

#[test]
fn execute_compact_command_flushes_project_memory_before_rewriting_transcript() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let memory_file = fixture.tempdir.path().join("MEMORY.md");
    state.project_memory = Some(
        crate::memory::ProjectMemoryContext::from_parts(
            "demo".to_string(),
            fixture.tempdir.path().to_path_buf(),
            memory_file.clone(),
            6_000,
        )
        .unwrap(),
    );
    state.current_provider = Some("local-anthropic".to_string());
    state.current_model = Some("local-anthropic/claude-sonnet-4-5".to_string());
    state.push_message(MessageRole::User, "original context");
    state.push_message(MessageRole::Assistant, "working on it");
    let resources = LoadedResources {
        tools: vec![memory_tool_resource()],
        ..LoadedResources::default()
    };
    let mut providers = ProviderRegistry::new();
    providers.register(local_provider(spawn_json_server(vec![
        json!({
            "id": "msg_flush_tool",
            "model": "claude-sonnet-4-5",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "toolu_flush_1",
                "name": "Memory",
                "input": {"action": "add", "content": "durable fact from compact flush"}
            }],
            "stop_reason": "tool_use"
        }),
        json!({
            "id": "msg_flush_done",
            "model": "claude-sonnet-4-5",
            "role": "assistant",
            "content": [{"type": "text", "text": "flush complete"}],
            "stop_reason": "end_turn"
        }),
        json!({
            "id": "msg_compact_done",
            "model": "claude-sonnet-4-5",
            "role": "assistant",
            "content": [{"type": "text", "text": "summary body"}],
            "stop_reason": "end_turn"
        }),
    ])));
    let mut auth_store = AuthStore::default();

    execute_compact_prompt_command(
        &mut state,
        &resources,
        &providers,
        &mut auth_store,
        &fixture.session_store,
        "Summarize now",
        None,
    )
    .unwrap();

    let memory_contents = std::fs::read_to_string(&memory_file).unwrap();
    assert!(memory_contents.contains("durable fact from compact flush"));
    assert!(
        matches!(state.transcript.first(), Some(message) if message.role == MessageRole::User && message.text.contains("[Conversation compacted"))
    );
    assert!(state.transcript.iter().any(|message| {
        message.role == MessageRole::System
            && message
                .text
                .contains("Conversation compacted. Summary preserved in context.")
    }));
}

#[test]
fn btw_specialization_requires_a_question() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let session_store = fixture.session_store;

    let outcome = prepare_btw_prompt_command(&mut state, &session_store, "").unwrap();

    assert_eq!(outcome, PromptCommandPreparation::HandledLocally);
    assert!(state
        .transcript
        .last()
        .unwrap()
        .text
        .contains("Usage: /btw <your question>"));
}

#[test]
fn btw_specialization_uses_side_question_variant() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let session_store = fixture.session_store;

    let outcome = prepare_btw_prompt_command(&mut state, &session_store, "what changed?").unwrap();

    assert_eq!(
        outcome,
        PromptCommandPreparation::SideQuestion("what changed?".to_string())
    );
}

#[test]
fn commit_specialization_collects_git_context() {
    let fixture = sample_state();
    let repo = fixture.tempdir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(Command::new("git")
        .arg("init")
        .arg(&repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "user.name", "Test User"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "user.email", "test@example.com"])
        .status()
        .unwrap()
        .success());
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["add", "README.md"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["commit", "-m", "init"])
        .status()
        .unwrap()
        .success());
    std::fs::write(repo.join("README.md"), "hello\nupdated\n").unwrap();

    let mut state = fixture.state;
    state.cwd = repo;
    let outcome = prepare_commit_prompt_command(&state).unwrap();

    match outcome {
        PromptCommandPreparation::VariableOverrides(variables) => {
            assert!(variables.contains_key("GIT_STATUS"));
            assert!(variables.contains_key("GIT_DIFF"));
            assert!(variables.contains_key("CURRENT_BRANCH"));
            assert!(variables.contains_key("RECENT_COMMITS"));
        }
        PromptCommandPreparation::DirectPrompt(_)
        | PromptCommandPreparation::HandledLocally
        | PromptCommandPreparation::SideQuestion(_)
        | PromptCommandPreparation::PromptOverride(_) => {
            panic!("expected commit variable overrides")
        }
    }
}

#[test]
fn plan_specialization_enables_mode_without_creating_a_plan_file() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let session_store = fixture.session_store;
    let plan_path = plan_file_path(&state).unwrap();

    let show_outcome = prepare_plan_prompt_command(&mut state, &session_store, "").unwrap();
    assert_eq!(show_outcome, PromptCommandPreparation::HandledLocally);
    assert!(state.plan_mode);
    assert!(!plan_path.exists());
    assert!(state
        .transcript
        .last()
        .unwrap()
        .text
        .contains("Enabled plan mode"));
}

#[test]
fn plan_specialization_with_description_submits_raw_prompt_after_enabling_mode() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let session_store = fixture.session_store;
    let plan_path = plan_file_path(&state).unwrap();
    let outcome =
        prepare_plan_prompt_command(&mut state, &session_store, "stabilize slash-command parity")
            .unwrap();

    assert_eq!(outcome, PromptCommandPreparation::HandledLocally);
    assert_eq!(
        state.take_pending_query_prompt().as_deref(),
        Some("stabilize slash-command parity")
    );
    assert!(state.plan_mode);
    assert!(!plan_path.exists());
    assert!(state
        .transcript
        .last()
        .unwrap()
        .text
        .contains("Enabled plan mode"));
}

#[test]
fn plan_specialization_shows_existing_plan_when_already_active() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let session_store = fixture.session_store;
    state.plan_mode = true;
    persist_plan_output(&state, "# Current Plan\n\n1. Verify tooling\n").unwrap();

    let outcome =
        prepare_plan_prompt_command(&mut state, &session_store, "next-step ignored").unwrap();

    assert_eq!(outcome, PromptCommandPreparation::HandledLocally);
    assert!(state
        .transcript
        .last()
        .unwrap()
        .text
        .contains("Current Plan"));
    assert!(state
        .transcript
        .last()
        .unwrap()
        .text
        .contains("Verify tooling"));
}

#[test]
fn plan_open_reports_missing_plan_when_no_plan_exists() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let session_store = fixture.session_store;
    state.plan_mode = true;

    let outcome = prepare_plan_prompt_command(&mut state, &session_store, "open").unwrap();

    assert_eq!(outcome, PromptCommandPreparation::HandledLocally);
    assert!(!plan_file_path(&state).unwrap().exists());
    assert!(state
        .transcript
        .last()
        .unwrap()
        .text
        .contains("Already in plan mode. No plan written yet."));
}

#[test]
fn plan_specialization_reports_missing_plan_when_already_active() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let session_store = fixture.session_store;
    state.plan_mode = true;

    let outcome = prepare_plan_prompt_command(&mut state, &session_store, "").unwrap();

    assert_eq!(outcome, PromptCommandPreparation::HandledLocally);
    assert!(state
        .transcript
        .last()
        .unwrap()
        .text
        .contains("Already in plan mode. No plan written yet."));
}

#[test]
fn plan_specialization_treats_default_scaffold_as_missing_plan() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let session_store = fixture.session_store;
    state.plan_mode = true;
    ensure_plan_file(&state).unwrap();

    let outcome = prepare_plan_prompt_command(&mut state, &session_store, "").unwrap();

    assert_eq!(outcome, PromptCommandPreparation::HandledLocally);
    assert!(state
        .transcript
        .last()
        .unwrap()
        .text
        .contains("Already in plan mode. No plan written yet."));
}

#[test]
fn handle_plan_command_queues_prompt_after_entering_plan_mode() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let session_store = fixture.session_store;

    handle_plan_command(
        &mut state,
        &LoadedResources::default(),
        &ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "stabilize slash-command parity",
    )
    .unwrap();

    assert!(state.plan_mode);
    assert!(state
        .transcript
        .iter()
        .any(|message| message.text == "Enabled plan mode"));
    assert_eq!(
        state.take_pending_query_prompt().as_deref(),
        Some("stabilize slash-command parity")
    );
}

#[test]
fn pr_comments_specialization_supplies_optional_input_block() {
    let empty = prepare_pr_comments_prompt_command("");
    let targeted = prepare_pr_comments_prompt_command("123");

    match empty {
        PromptCommandPreparation::VariableOverrides(variables) => {
            assert_eq!(
                variables.get("ADDITIONAL_USER_INPUT_BLOCK"),
                Some(&String::new())
            );
        }
        PromptCommandPreparation::DirectPrompt(_)
        | PromptCommandPreparation::HandledLocally
        | PromptCommandPreparation::SideQuestion(_)
        | PromptCommandPreparation::PromptOverride(_) => {
            panic!("expected variable overrides")
        }
    }
    match targeted {
        PromptCommandPreparation::VariableOverrides(variables) => {
            assert_eq!(
                variables.get("ADDITIONAL_USER_INPUT_BLOCK"),
                Some(&"Additional user input: 123".to_string())
            );
        }
        PromptCommandPreparation::DirectPrompt(_)
        | PromptCommandPreparation::HandledLocally
        | PromptCommandPreparation::SideQuestion(_)
        | PromptCommandPreparation::PromptOverride(_) => {
            panic!("expected variable overrides")
        }
    }
}

#[test]
fn security_review_specialization_collects_git_context() {
    let fixture = sample_state();
    let state = fixture.state;
    let outcome = prepare_security_review_prompt_command(&state).unwrap();

    match outcome {
        PromptCommandPreparation::VariableOverrides(variables) => {
            assert!(variables.contains_key("GIT_STATUS"));
            assert!(variables.contains_key("FILES_MODIFIED"));
            assert!(variables.contains_key("COMMITS"));
            assert!(variables.contains_key("DIFF_CONTENT"));
        }
        PromptCommandPreparation::DirectPrompt(_)
        | PromptCommandPreparation::HandledLocally
        | PromptCommandPreparation::SideQuestion(_)
        | PromptCommandPreparation::PromptOverride(_) => {
            panic!("expected variable overrides")
        }
    }
}

#[test]
fn statusline_specialization_uses_agent_setup_prompt() {
    let outcome = prepare_statusline_prompt_command("").unwrap();
    match outcome {
        PromptCommandPreparation::VariableOverrides(variables) => {
            assert_eq!(
                variables.get("STATUSLINE_PROMPT_JSON"),
                Some(&"\"Configure my statusLine from my shell PS1 configuration\"".to_string())
            );
        }
        PromptCommandPreparation::DirectPrompt(_)
        | PromptCommandPreparation::HandledLocally
        | PromptCommandPreparation::SideQuestion(_)
        | PromptCommandPreparation::PromptOverride(_) => panic!("expected variable overrides"),
    }
}

#[test]
fn dispatcher_helper_routes_known_prompt_specializations() {
    let fixture = sample_state();
    let mut state = fixture.state;
    let session_store = fixture.session_store;
    state.push_message(MessageRole::User, "summarize this");
    let repo = fixture.tempdir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(Command::new("git")
        .arg("init")
        .arg(&repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "user.name", "Test User"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "user.email", "test@example.com"])
        .status()
        .unwrap()
        .success());
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["add", "README.md"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["commit", "-m", "init"])
        .status()
        .unwrap()
        .success());
    state.cwd = repo;

    let pr_comments =
        prepare_prompt_command_specialization(&mut state, &session_store, "pr-comments", "")
            .unwrap();
    match pr_comments {
        Some(PromptCommandPreparation::VariableOverrides(variables)) => {
            assert!(variables.contains_key("ADDITIONAL_USER_INPUT_BLOCK"));
        }
        _ => panic!("expected pr-comments prompt variable overrides"),
    }

    let statusline =
        prepare_prompt_command_specialization(&mut state, &session_store, "statusline", "")
            .unwrap();
    match statusline {
        Some(PromptCommandPreparation::VariableOverrides(variables)) => {
            assert!(variables.contains_key("STATUSLINE_PROMPT_JSON"));
        }
        _ => panic!("expected statusline prompt variable overrides"),
    }

    let security_review =
        prepare_prompt_command_specialization(&mut state, &session_store, "security-review", "")
            .unwrap();
    match security_review {
        Some(PromptCommandPreparation::VariableOverrides(variables)) => {
            assert!(variables.contains_key("DIFF_CONTENT"));
        }
        _ => panic!("expected security-review variable overrides"),
    }

    let commit =
        prepare_prompt_command_specialization(&mut state, &session_store, "commit", "").unwrap();
    match commit {
        Some(PromptCommandPreparation::VariableOverrides(variables)) => {
            assert!(variables.contains_key("GIT_STATUS"));
            assert!(variables.contains_key("GIT_DIFF"));
        }
        _ => panic!("expected commit prompt variable overrides"),
    }

    state.project_memory = None;
    let init =
        prepare_prompt_command_specialization(&mut state, &session_store, "init", "").unwrap();
    match init {
        Some(PromptCommandPreparation::VariableOverrides(variables)) => {
            assert!(variables.is_empty());
            let context = state
                .project_memory
                .as_ref()
                .expect("/init should initialize project memory");
            assert!(context.memory_file.ends_with("MEMORY.md"));
            assert!(context.memory_file.exists());
        }
        _ => panic!("expected /init prompt variable overrides"),
    }

    let plan =
        prepare_prompt_command_specialization(&mut state, &session_store, "plan", "").unwrap();
    assert!(plan.is_none());

    let none =
        prepare_prompt_command_specialization(&mut state, &session_store, "review", "").unwrap();
    assert!(none.is_none());

    let btw =
        prepare_prompt_command_specialization(&mut state, &session_store, "btw", "what changed?")
            .unwrap();
    match btw {
        Some(PromptCommandPreparation::SideQuestion(question)) => {
            assert_eq!(question, "what changed?");
        }
        _ => panic!("expected /btw side-question specialization"),
    }

    let compact =
        prepare_prompt_command_specialization(&mut state, &session_store, "compact", "").unwrap();
    match compact {
        Some(PromptCommandPreparation::PromptOverride(prompt)) => {
            assert!(prompt.contains("Summarize the conversation into a compact context block"));
        }
        _ => panic!("expected /compact prompt override specialization"),
    }
}

#[test]
fn plan_mode_context_does_not_create_a_default_plan_file() {
    let fixture = sample_state();
    let mut state = fixture.state;
    state.plan_mode = true;

    let context = plan_mode_context_message(&state, &plan_mode_resources())
        .unwrap()
        .expect("plan mode context");

    assert!(context.contains("Plan mode is active."));
    assert!(context.contains("No plan file exists yet."));
    assert!(!plan_file_path(&state).unwrap().exists());
}

fn plan_mode_resources() -> LoadedResources {
    LoadedResources {
        prompts: vec![
            loaded_prompt(
                "resources/prompts/plan-mode-interview.yaml",
                include_str!("../../../resources/prompts/plan-mode-interview.yaml"),
            ),
            loaded_prompt(
                "resources/prompts/plan-mode-sparse.yaml",
                include_str!("../../../resources/prompts/plan-mode-sparse.yaml"),
            ),
            loaded_prompt(
                "resources/prompts/plan-mode-reentry.yaml",
                include_str!("../../../resources/prompts/plan-mode-reentry.yaml"),
            ),
            loaded_prompt(
                "resources/prompts/plan-mode-exited.yaml",
                include_str!("../../../resources/prompts/plan-mode-exited.yaml"),
            ),
        ],
        ..LoadedResources::default()
    }
}

fn loaded_prompt(path: &str, contents: &str) -> LoadedItem<PromptTemplate> {
    LoadedItem {
        value: serde_yaml::from_str::<PromptTemplate>(contents).unwrap(),
        source_info: SourceInfo {
            path: PathBuf::from(path),
            kind: SourceKind::Builtin,
        },
    }
}

struct TestFixture {
    #[allow(dead_code)]
    tempdir: TempDir,
    state: AppState,
    session_store: SessionStore,
}

struct ScopedPufferHome {
    _override: puffer_config::PufferHomeOverride,
}

impl ScopedPufferHome {
    fn set(path: &std::path::Path) -> Self {
        Self {
            _override: puffer_config::set_puffer_home_override(path),
        }
    }
}

fn sample_state() -> TestFixture {
    let tempdir = tempdir().unwrap();
    let _lock = crate::test_locks::env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempdir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    TestFixture {
        tempdir,
        state,
        session_store,
    }
}

fn memory_tool_resource() -> LoadedItem<ToolSpec> {
    LoadedItem {
        value: ToolSpec {
            id: "Memory".to_string(),
            name: "Memory".to_string(),
            description: "Project memory tool".to_string(),
            handler: "runtime:project_memory".to_string(),
            approval_policy: Some("auto".to_string()),
            sandbox_policy: Some("workspace-write".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string"},
                    "content": {"type": "string"},
                    "old_text": {"type": "string"}
                },
                "required": ["action"]
            })),
            ..ToolSpec::default()
        },
        source_info: SourceInfo {
            path: PathBuf::from("memory.yaml"),
            kind: SourceKind::Builtin,
        },
    }
}

fn local_provider(base_url: String) -> ProviderDescriptor {
    ProviderDescriptor {
        id: "local-anthropic".to_string(),
        display_name: "Local Anthropic".to_string(),
        base_url,
        default_api: "anthropic-messages".to_string(),
        auth_modes: Vec::new(),
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            provider: "local-anthropic".to_string(),
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            input: Vec::new(),
            cost: None,
            compat: None,
        }],
        chat_completions_path: None,
    }
}

fn spawn_json_server(responses: Vec<Value>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for response_body in responses {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0_u8; 8192];
            let _ = stream.read(&mut buffer);
            let body = response_body.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(), body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        }
    });
    format!("http://{address}")
}

#[test]
fn night_directive_carries_guardrails_budget_and_gates_pr() {
    // Fork-PR off (experimental default): all guardrails + budget + fenced leads.
    let off = build_night_directive("lead: extend the recall index", false, 1_000_000);
    assert!(off.contains(".worktree/"), "must mandate worktree isolation");
    assert!(off.contains("NON-DESTRUCTIVE"));
    assert!(off.contains("NO DRIFT"));
    assert!(off.contains("SUBAGENTS"));
    assert!(off.contains("end-to-end") && off.contains("SCREENSHOTS"));
    // Leads are embedded but FENCED as untrusted (not framed as commands).
    assert!(off.contains("<untrusted-leads>"), "leads must be fenced as untrusted");
    assert!(off.contains("UNTRUSTED"));
    assert!(off.contains("lead: extend the recall index"), "autodream leads embedded");
    // Budget/goal section present.
    assert!(off.contains("budget") && off.contains("1000000"));
    assert!(off.contains("Fork-PR is DISABLED"));
    assert!(!off.contains("Fork-PR is ENABLED"));

    // Fork-PR on + empty leads -> "(none)" inside the fence + enabled clause.
    let on = build_night_directive("   ", true, 500_000);
    assert!(on.contains("Fork-PR is ENABLED"));
    assert!(on.contains("USER'S FORK"));
    assert!(on.contains("<untrusted-leads>\n(none)\n</untrusted-leads>"));
    assert!(on.contains("500000"));
}
