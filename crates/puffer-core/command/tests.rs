use super::*;
use crate::RenderedMessage;
use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry};
use puffer_resources::{LoadedItem, LoadedResources, PromptTemplate, SourceInfo, SourceKind};
use puffer_session_store::SessionStore;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use tempfile::tempdir;

mod artifacts;
mod basics;
mod commit;
mod connectors;
mod context;
mod doctor;
mod files;
mod login_auth;
mod mcp;
mod misc;
mod model_scope;
mod parity;
mod plugin;
mod recap;
mod remote_history;
mod sandbox;
mod session_local;
mod status;
mod tag;
mod tasks;
mod terminal_setup;
mod usage_buddy;
mod workflows;

pub(super) fn puffer_home_lock() -> &'static Mutex<()> {
    crate::test_locks::env_lock()
}

pub(super) fn lock_puffer_home() -> MutexGuard<'static, ()> {
    puffer_home_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) struct ScopedPufferHome {
    _override: puffer_config::PufferHomeOverride,
}

impl ScopedPufferHome {
    pub(super) fn set(path: &std::path::Path) -> Self {
        Self {
            _override: puffer_config::set_puffer_home_override(path),
        }
    }
}

#[test]
fn config_command_supports_model_and_camel_case_aliases() {
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/config set statusLineCommand echo-status",
    )
    .unwrap();
    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/config set model openai/gpt-5",
    )
    .unwrap();

    assert_eq!(state.current_provider.as_deref(), Some("openai"));
    assert_eq!(state.current_model.as_deref(), Some("openai/gpt-5"));
    assert_eq!(
        state
            .config
            .ui
            .status_line
            .as_ref()
            .map(|status_line| status_line.command.as_str()),
        Some("echo-status")
    );
}

#[test]
fn theme_vim_fast_and_effort_commands_persist_user_settings() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut state = AppState::new(PufferConfig::default(), workspace.clone(), session);
    state.current_provider = Some("openai".to_string());

    for command in ["/theme harbor", "/vim", "/fast on", "/effort xhigh"] {
        dispatch_command(
            &mut state,
            &supported_commands(),
            &LoadedResources::default(),
            &mut ProviderRegistry::new(),
            &mut AuthStore::default(),
            &session_store,
            command,
        )
        .unwrap();
    }

    let loaded = puffer_config::load_config(&paths).unwrap();
    assert_eq!(loaded.theme, "harbor");
    assert_eq!(loaded.editor_mode, "vim");
    assert!(loaded.fast_mode);
    assert_eq!(loaded.effort_level.as_deref(), Some("xhigh"));
}

#[test]
fn keybindings_command_creates_workspace_file() {
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/keybindings",
    )
    .unwrap();

    let keybindings_path = paths.workspace_config_dir.join("keybindings.toml");
    let keybindings = std::fs::read_to_string(keybindings_path).unwrap();
    assert!(keybindings.contains("submit = \"enter\""));
}

#[test]
fn permissions_command_creates_workspace_permissions_file() {
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
    let mut resources = LoadedResources::default();
    resources.tools.push(LoadedItem {
        value: puffer_resources::ToolSpec {
            id: "bash".to_string(),
            name: "bash".to_string(),
            description: "Run shell".to_string(),
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
        source_info: puffer_resources::SourceInfo {
            path: "tools/bash.yaml".into(),
            kind: puffer_resources::SourceKind::Builtin,
        },
    });

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/permissions",
    )
    .unwrap();

    let permissions_path = paths.workspace_config_dir.join("permissions.toml");
    let contents = std::fs::read_to_string(permissions_path).unwrap();
    assert!(contents.contains("[tools]"));
    assert!(contents.contains("bash = \"on-request\""));
}

#[test]
fn hooks_command_creates_workspace_file() {
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
    let mut resources = LoadedResources::default();
    resources.hooks.push(puffer_resources::LoadedItem {
        value: puffer_resources::HookSpec {
            id: "tool-end".to_string(),
            event: "tool_end".to_string(),
            command: "echo hook".to_string(),
        },
        source_info: puffer_resources::SourceInfo {
            path: "hooks/tool_end_echo.yaml".into(),
            kind: puffer_resources::SourceKind::Builtin,
        },
    });

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/hooks",
    )
    .unwrap();

    let hooks_path = paths
        .workspace_config_dir
        .join("resources/hooks/tool_end.yaml");
    let hooks = std::fs::read_to_string(hooks_path).unwrap();
    assert!(hooks.contains("tool_end"));
    assert!(hooks.contains("PUFFER_TOOL_ID"));
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("loaded_hooks=1") && text.contains("tool-end")
    ));
}

#[test]
fn doctor_reports_discovery_and_diagnostics() {
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
    state.record_task("bash", "printf hi", true);
    let mut resources = LoadedResources::default();
    resources
        .diagnostics
        .push("prompt `review` overrides builtin".to_string());
    let mut providers = ProviderRegistry::new();
    providers.register(ProviderDescriptor {
        id: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        default_api: "anthropic-messages".to_string(),
        auth_modes: vec![puffer_provider_registry::AuthMode::ApiKey],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: Some(puffer_provider_registry::ModelDiscoveryConfig {
            path: "/v1/models".to_string(),
            response: puffer_provider_registry::ModelDiscoveryFormat::AnthropicModels,
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            items_field: "data".to_string(),
            id_field: "id".to_string(),
            display_name_field: Some("display_name".to_string()),
            headers: Default::default(),
        }),
        models: Vec::new(),
        chat_completions_path: None,
    });
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("anthropic", "sk-ant");

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut providers,
        &mut auth_store,
        &session_store,
        "/doctor",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Puffer doctor")
            && text.contains("auth=api-key")
            && text.contains("discovery=configured")
            && text.contains("resource_diagnostics=1")
            && text.contains("recorded_tasks=1")
    ));
}

#[test]
fn ide_command_creates_workspace_ide_file() {
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/ide",
    )
    .unwrap();

    let ide_path = paths
        .workspace_config_dir
        .join("resources/ides/workspace.yaml");
    assert!(ide_path.exists());
}

#[test]
fn agents_command_creates_workspace_file() {
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/agents",
    )
    .unwrap();

    let agents_path = paths
        .workspace_config_dir
        .join("resources/agents/workspace.yaml");
    let agents = std::fs::read_to_string(agents_path).unwrap();
    assert!(agents.contains("id: default"));
    assert!(agents.contains("prompt: \"You are a coding subagent for Puffer Code."));
}

#[test]
fn agents_command_can_list_and_use_agent_presets() {
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
    std::fs::create_dir_all(paths.workspace_config_dir.join("resources/agents")).unwrap();
    std::fs::write(
        paths.workspace_config_dir.join("resources/agents/reviewer.yaml"),
        "id: reviewer\ndescription: Reviews code carefully.\nprompt: |\n  You are a reviewer.\ntools:\n  - Read\nmodel: openai/gpt-5\n",
    )
    .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/agents use reviewer",
    )
    .unwrap();

    assert_eq!(state.current_model.as_deref(), Some("openai/gpt-5"));
    assert_eq!(state.current_provider.as_deref(), Some("openai"));
}

#[test]
fn agents_command_lists_builtin_resource_agents() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    std::fs::create_dir_all(tempdir.path().join("resources/agents")).unwrap();
    std::fs::write(
        tempdir.path().join("resources/agents/explore.yaml"),
        "id: explore\ndescription: Read-only exploration agent.\nprompt: |\n  Explore the repository.\ntools:\n  - Read\n",
    )
    .unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/agents list",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("explore [builtin]")
    ));
}

#[test]
fn prompt_commands_append_user_message_and_surface_runtime_failures() {
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/review",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.first(),
        Some(RenderedMessage {
            role: MessageRole::User,
            ..
        })
    ));
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Prompt command /review failed")
    ));
}

#[test]
fn plan_command_enters_plan_mode_and_submits_raw_arguments() {
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/plan stabilize slash-command parity",
    )
    .unwrap();

    assert!(state.plan_mode);
    assert!(
        state.transcript.iter().any(|message| {
            message.role == MessageRole::System && message.text == "Enabled plan mode"
        }),
        "{:?}",
        state.transcript
    );
    assert_eq!(
        state.take_pending_query_prompt().as_deref(),
        Some("stabilize slash-command parity")
    );
}

#[test]
fn pr_comments_command_uses_reference_prompt_text_from_resources() {
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
    let resources = LoadedResources {
        prompts: vec![LoadedItem {
            value: serde_yaml::from_str::<PromptTemplate>(include_str!(
                "../../../resources/prompts/pr-comments.yaml"
            ))
            .unwrap(),
            source_info: SourceInfo {
                path: PathBuf::from("resources/prompts/pr-comments.yaml"),
                kind: SourceKind::Builtin,
            },
        }],
        ..LoadedResources::default()
    };

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/pr-comments 123",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.first(),
        Some(RenderedMessage {
            role: MessageRole::User,
            text, ..
        }) if text.contains("gh pr view --json number,headRepository")
            && text.contains("gh api /repos/{owner}/{repo}/issues/{number}/comments")
            && text.contains("Return ONLY the formatted comments")
            && text.contains("Additional user input: 123")
            && !text.contains("Command mode:")
    ));
}

#[test]
fn skill_command_executes_rendered_skill_prompt_instead_of_dumping_skill_body() {
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
    let resources = LoadedResources {
        skills: vec![LoadedItem {
            value: puffer_resources::SkillSpec {
                name: "verify".to_string(),
                description: "Verify changes".to_string(),
                content: "Check $ARGUMENTS".to_string(),
                allowed_tools: vec!["Read".to_string()],
                ..puffer_resources::SkillSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from("/tmp/work/.puffer/resources/skills/verify/SKILL.md"),
                kind: SourceKind::Workspace,
            },
        }],
        ..LoadedResources::default()
    };

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/skill:verify cargo test",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.first(),
        Some(RenderedMessage {
            role: MessageRole::User,
            text, ..
        }) if text.contains("Base directory for this skill:")
            && text.contains("Check cargo test")
    ));
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Skill command /skill:verify failed")
    ));
}

#[test]
fn direct_skill_command_executes_rendered_skill_prompt_via_builtin_surface_fallback() {
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
    let resources = LoadedResources {
        skills: vec![LoadedItem {
            value: puffer_resources::SkillSpec {
                name: "verify".to_string(),
                description: "Verify changes".to_string(),
                content: "Check $ARGUMENTS".to_string(),
                allowed_tools: vec!["Read".to_string()],
                ..puffer_resources::SkillSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from("/tmp/work/.puffer/resources/skills/verify/SKILL.md"),
                kind: SourceKind::Workspace,
            },
        }],
        ..LoadedResources::default()
    };

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/verify cargo test",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.first(),
        Some(RenderedMessage {
            role: MessageRole::User,
            text, ..
        }) if text.contains("Base directory for this skill:")
            && text.contains("Check cargo test")
    ));
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Skill command /skill:verify failed")
    ));
}

#[test]
fn skill_command_rejects_model_only_skills_for_direct_invocation() {
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
    let resources = LoadedResources {
        skills: vec![LoadedItem {
            value: puffer_resources::SkillSpec {
                name: "hidden".to_string(),
                description: "Hidden".to_string(),
                content: "Hidden".to_string(),
                user_invocable: false,
                ..puffer_resources::SkillSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from("/tmp/work/.puffer/resources/skills/hidden/SKILL.md"),
                kind: SourceKind::Workspace,
            },
        }],
        ..LoadedResources::default()
    };

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/skill:hidden",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("can only be invoked by Claude")
    ));
}

#[test]
fn hidden_telegram_skill_does_not_activate_direct_telegram_command() {
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
    let resources = LoadedResources {
        skills: vec![LoadedItem {
            value: puffer_resources::SkillSpec {
                name: "telegram".to_string(),
                description: "Hidden Telegram helper".to_string(),
                content: "Hidden".to_string(),
                user_invocable: false,
                ..puffer_resources::SkillSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from("/tmp/work/.puffer/resources/skills/telegram/SKILL.md"),
                kind: SourceKind::Workspace,
            },
        }],
        ..LoadedResources::default()
    };

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/telegram search-peers hi",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text,
            ..
        }) if text == "Unknown command: /telegram"
    ));
}

#[test]
fn session_command_can_list_and_update_note() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let second = session_store
        .create_session(workspace.join("secondary"))
        .unwrap();
    session_store
        .rename_session(second.id, "dockyard".to_string())
        .unwrap();
    let mut state = AppState::new(PufferConfig::default(), workspace, session);

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/session note keep-shipping",
    )
    .unwrap();
    assert_eq!(state.session.note.as_deref(), Some("keep-shipping"));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/session list",
    )
    .unwrap();
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("dockyard")
    ));
}

#[test]
fn session_command_can_update_note_and_slug() {
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/session note keep shipping",
    )
    .unwrap();
    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/session slug dockyard",
    )
    .unwrap();

    let record = session_store.load_session(state.session.id).unwrap();
    assert_eq!(state.session.note.as_deref(), Some("keep shipping"));
    assert_eq!(state.session.slug.as_deref(), Some("dockyard"));
    assert_eq!(record.metadata.note.as_deref(), Some("keep shipping"));
    assert_eq!(record.metadata.slug.as_deref(), Some("dockyard"));
}

#[test]
fn session_command_lists_saved_sessions() {
    let tempdir = tempdir().unwrap();
    let _lock = lock_puffer_home();
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _home = ScopedPufferHome::set(&home);
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let current = workspace.join("current");
    let session = session_store.create_session(current.clone()).unwrap();
    let listed = session_store
        .create_session(workspace.join("listed"))
        .unwrap();
    session_store
        .rename_session(listed.id, "dockyard".to_string())
        .unwrap();
    let mut state = AppState::new(PufferConfig::default(), current, session);

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/session list",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("dockyard")
    ));
}
