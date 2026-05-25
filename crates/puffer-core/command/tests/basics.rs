use super::*;
use puffer_config::{MascotConfig, UiConfig};
use puffer_session_store::SessionMetadata;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn command_registry_contains_review_usage_and_resume_alias() {
    let commands = supported_commands();
    assert!(find_command(&commands, "review").is_some());
    assert!(find_command(&commands, "tag").is_some());
    assert!(find_command(&commands, "usage").is_some());
    assert!(find_command(&commands, "continue").is_some());
    assert!(find_command(&commands, "?").is_some());
}

#[test]
fn reflect_command_is_registered_as_local() {
    let commands = supported_commands();
    let reflect = find_command(&commands, "reflect").expect("reflect command");
    assert_eq!(reflect.kind, CommandKind::Local);
    assert!(reflect
        .argument_hint
        .as_deref()
        .is_some_and(|hint| hint.contains("lang")));
}

#[test]
fn btw_stays_local_and_compact_runs_as_provider_prompt() {
    let commands = supported_commands();
    assert_eq!(
        find_command(&commands, "btw").map(|command| command.kind),
        Some(CommandKind::Local)
    );
    assert_eq!(
        find_command(&commands, "compact").map(|command| command.kind),
        Some(CommandKind::Prompt)
    );
}

#[test]
fn connect_command_is_registered_as_local_command() {
    let commands = supported_commands();
    let connect = find_command(&commands, "connect").expect("connect command");
    assert_eq!(connect.kind, CommandKind::Local);
    assert_eq!(
        connect.argument_hint.as_deref(),
        Some("<connector-slug> <connection-name>")
    );
}

#[test]
fn workflows_command_is_registered_as_local_command() {
    let commands = supported_commands();
    let workflows = find_command(&commands, "workflows").expect("workflows command");
    assert_eq!(workflows.kind, CommandKind::Local);
    assert_eq!(
        workflows.argument_hint.as_deref(),
        Some("[list|new|connections|connectors|tasks|runs] [query]")
    );
    assert_eq!(
        find_command(&commands, "pipelines").map(|command| command.name.as_str()),
        Some("workflows")
    );
}

#[test]
fn workflows_command_summarizes_native_workflows() {
    use puffer_workflow::{RegisterOptions, TriggerSpec, WorkflowStore};
    use serde_json::json;

    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let telegram_manifest = paths
        .builtin_resources_dir
        .join("subscribers/telegram-user");
    std::fs::create_dir_all(&telegram_manifest).unwrap();
    std::fs::write(
        telegram_manifest.join("manifest.toml"),
        "manifest_version = 1\nid = \"telegram-user\"\nkind = \"subscriber\"\ntopic = \"telegram-user\"\n[run]\ncmd = [\"true\"]\n",
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
    WorkflowStore::new(&paths.workspace_config_dir)
        .register_json(
            json!({
                "name": "Message triage",
                "nodes": [{"id": "triage", "prompt": "Summarize the incoming message."}]
            }),
            RegisterOptions {
                slug: Some("message-triage".to_string()),
                trigger: Some(TriggerSpec::Connection {
                    connection_slug: "telegram-user".to_string(),
                    filter: None,
                    pattern: Some("hi".to_string()),
                    classify_prompt: None,
                }),
            },
        )
        .unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/workflows",
    )
    .unwrap();

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("Workflow dashboard"));
    assert!(text.contains("monitor_tasks=0/0"));
    assert!(text.contains("message-triage"));
    assert!(text.contains("trigger=connection:telegram-user"));
    assert!(text.contains("none configured; run /connect"));
    assert!(text.contains("Monitor tasks"));
    assert!(text.contains("telegram-login"));
    assert!(!text.contains("telegram-bot"));
}

#[test]
fn workflows_new_creates_disabled_starter_workflow() {
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
        "/workflows new Customer-Triage",
    )
    .unwrap();

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("Created disabled workflow draft."));
    assert!(text.contains("slug: customer-triage"));
    assert!(text.contains("trigger: subscription:workspace.task.created"));
    let workflows = puffer_workflow::WorkflowStore::new(&paths.workspace_config_dir)
        .list()
        .unwrap();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].slug, "customer-triage");
    assert!(!workflows[0].enabled);
    assert_eq!(workflows[0].pipeline.name, "Customer Triage");
    assert_eq!(workflows[0].pipeline.nodes[0].id, "codex-task");
}

#[test]
fn workflows_tasks_filter_shows_monitor_task_commands() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let task_path = paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
        .join("monitor_tasks.json");
    std::fs::create_dir_all(task_path.parent().unwrap()).unwrap();
    std::fs::write(
        &task_path,
        serde_json::to_string_pretty(&json!({
            "tasks": [
                {
                    "task_id": "monitor-1",
                    "subject": "Reply to Telegram support ping",
                    "description": "Alice asked whether the deployment is finished.",
                    "status": "pending",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "actions": [
                            {
                                "actionName": "Draft reply",
                                "actionPrompt": "Draft a concise reply to Alice."
                            }
                        ],
                        "possibleIgnoreReasons": ["duplicate support ping"]
                    }
                },
                {
                    "task_id": "monitor-2",
                    "subject": "Ignored reminder",
                    "description": "Already handled.",
                    "status": "completed",
                    "metadata": {
                        "_monitor": true,
                        "monitor_connection": "telegram-user",
                        "monitor_connector": "telegram-login",
                        "ignored": true
                    }
                }
            ]
        }))
        .unwrap(),
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
        "/workflows tasks support ping",
    )
    .unwrap();

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("monitor_tasks=1/2"));
    assert!(text.contains("showing 1/2 monitor tasks for query=\"support ping\""));
    assert!(
        text.contains("- monitor-1 [pending] connection=telegram-user connector=telegram-login")
    );
    assert!(text.contains("actions=Draft reply"));
    assert!(text.contains("show=/tasks show monitor-1"));
    assert!(text.contains("ignore=/tasks ignore monitor-1 duplicate support ping"));
    assert!(!text.contains("monitor-2"));
}

#[test]
fn workflows_connectors_filter_shows_connect_commands() {
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
        "/workflows connectors telegram",
    )
    .unwrap();

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("showing 2/22 connectors for query=\"telegram\""));
    assert!(text.contains("telegram-login"));
    assert!(text.contains("actions=send_message"));
    assert!(text.contains("connect=/connect telegram-login telegram-user"));
    assert!(!text.contains("- email ["));
}

#[test]
fn workflows_connectors_filter_matches_action_terms() {
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
        "/workflows connectors vote poll",
    )
    .unwrap();

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("telegram-login"));
    assert!(text.contains("vote_poll"));
    assert!(!text.contains("- slack-login ["));
}

#[test]
fn workflows_connectors_filter_presets_use_stable_capability_terms() {
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
        "/workflows connectors has-actions",
    )
    .unwrap();

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("filters: trigger-ready | no-trigger | draft | has-actions"));
    assert!(text.contains("showing 7/22 connectors for query=\"has-actions\""));
    assert!(text.contains("- telegram-login [auth,events,no-trigger,actions]"));
    assert!(text.contains("actions=send_message,"));
    assert!(!text.contains("- slack-bot ["));
}

#[test]
fn workflows_connectors_catalog_includes_serve_connectors_as_non_triggers() {
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
        "/workflows connectors discord",
    )
    .unwrap();

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("showing 1/22 connectors for query=\"discord\""));
    assert!(text.contains("discord-bot"));
    assert!(text.contains("connect=/connect discord-bot discord-bot"));
    assert!(text.contains("[auth,no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_github_webhook_preset() {
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
        "/workflows connectors github",
    )
    .unwrap();

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("showing 1/22 connectors for query=\"github\""));
    assert!(text.contains("github-webhook"));
    assert!(text.contains("connect=/connect github-webhook github-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_gitlab_webhook_preset() {
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
        "/workflows connectors gitlab merge request",
    )
    .unwrap();

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("showing 1/22 connectors for query=\"gitlab merge request\""));
    assert!(text.contains("gitlab-webhook") && text.contains("/connect gitlab-webhook"));
    assert!(text.contains("runtime=serve") && text.contains("[no-trigger]"));
}

#[test]
fn workflows_connectors_catalog_includes_linear_webhook_preset() {
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
        "/workflows connectors linear issue",
    )
    .unwrap();

    let text = &state.transcript.last().unwrap().text;
    assert!(text.contains("showing 1/22 connectors for query=\"linear issue\""));
    assert!(text.contains("linear-webhook"));
    assert!(text.contains("connect=/connect linear-webhook linear-webhook"));
    assert!(text.contains("runtime=serve"));
    assert!(text.contains("[no-trigger]"));
}

#[test]
fn telegram_command_is_registered_as_local_command() {
    let commands = supported_commands();
    let telegram = find_command(&commands, "telegram").expect("telegram command");
    assert_eq!(telegram.kind, CommandKind::Local);
    assert!(telegram
        .argument_hint
        .as_deref()
        .is_some_and(|hint| hint.contains("list-messages")));
}

#[test]
fn command_surface_includes_user_invocable_skills_and_skill_aliases() {
    let resources = LoadedResources {
        skills: vec![
            LoadedItem {
                value: puffer_resources::SkillSpec {
                    name: "verify".to_string(),
                    description: "Verify changes".to_string(),
                    argument_hint: Some("<target>".to_string()),
                    ..puffer_resources::SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: PathBuf::from("/tmp/work/.puffer/resources/skills/verify/SKILL.md"),
                    kind: SourceKind::Workspace,
                },
            },
            LoadedItem {
                value: puffer_resources::SkillSpec {
                    name: "hidden".to_string(),
                    description: "Hidden".to_string(),
                    user_invocable: false,
                    ..puffer_resources::SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: PathBuf::from("/tmp/work/.puffer/resources/skills/hidden/SKILL.md"),
                    kind: SourceKind::Workspace,
                },
            },
        ],
        ..LoadedResources::default()
    };

    let commands = command_surface(&resources);
    let verify = find_command(&commands, "verify").expect("verify skill command");
    assert_eq!(verify.argument_hint.as_deref(), Some("<target>"));
    assert!(find_command(&commands, "skill:verify").is_some());
    assert!(find_command(&commands, "hidden").is_none());
}

#[test]
fn command_surface_keeps_builtin_telegram_a_local_command() {
    let resources = LoadedResources {
        skills: vec![LoadedItem {
            value: puffer_resources::SkillSpec {
                name: "telegram".to_string(),
                description: "Prompt-backed Telegram helper".to_string(),
                ..puffer_resources::SkillSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from("/tmp/work/.puffer/resources/skills/telegram/SKILL.md"),
                kind: SourceKind::Workspace,
            },
        }],
        ..LoadedResources::default()
    };

    let commands = command_surface(&resources);
    let telegram = find_command(&commands, "telegram").expect("telegram command");
    assert_eq!(telegram.kind, CommandKind::Local);
    assert!(find_command(&commands, "skill:telegram").is_none());
}

#[test]
fn app_state_defaults_expose_command_state() {
    let state = AppState::new(
        PufferConfig {
            app_name: "Puffer".to_string(),
            default_model: None,
            default_provider: Some("anthropic".to_string()),
            openai_base_url: None,
            openai_headers: BTreeMap::new(),
            openai_query_params: BTreeMap::new(),
            theme: "puffer".to_string(),
            editor_mode: "normal".to_string(),
            fast_mode: false,
            effort_level: None,
            copy_full_response: false,
            memory: puffer_config::MemoryConfig::default(),
            recap: puffer_config::RecapConfig::default(),
            mascot: MascotConfig {
                id: "clawd".to_string(),
                display_name: "Clawd".to_string(),
                enabled: true,
            },
            ui: UiConfig {
                no_alt_screen: false,
                tmux_golden_mode: false,
                status_line: None,
            },
            remote_runner: None,
        },
        PathBuf::from("."),
        SessionMetadata {
            id: uuid::Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: PathBuf::from("."),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        },
    );
    assert_eq!(state.prompt_color, "default");
    assert_eq!(state.effort_level, "auto");
    assert_eq!(state.sandbox_mode, "workspace-write");
    assert!(state.statusline_enabled);
}

#[test]
fn local_commands_append_state_snapshots() {
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/theme harbor",
    )
    .unwrap();

    let record = session_store.load_session(session.id).unwrap();
    assert!(matches!(
        record.events.iter().rev().nth(1),
        Some(TranscriptEvent::StateSnapshot { theme, .. }) if theme == "harbor"
    ));
}

#[test]
fn add_dir_requires_an_explicit_directory_argument() {
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
        "/add-dir",
    )
    .unwrap();

    assert!(state.working_dirs.is_empty());
    assert!(state
        .transcript
        .last()
        .is_some_and(|message| message.text == "Usage: /add-dir <directory>"));
}

#[test]
fn add_dir_validates_and_canonicalizes_new_working_directories() {
    let tempdir = tempdir().unwrap();
    let cwd = tempdir.path().join("repo");
    fs::create_dir_all(&cwd).unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(cwd.clone()).unwrap();
    let mut state = AppState::new(PufferConfig::default(), cwd, session);
    let extra = tempdir.path().join("extra");
    fs::create_dir_all(&extra).unwrap();

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        &format!("/add-dir {}", extra.display()),
    )
    .unwrap();

    assert_eq!(state.working_dirs, vec![extra.clone()]);
    assert!(state
        .transcript
        .last()
        .is_some_and(|message| message.text.contains("working directory for this session")));

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        &format!("/add-dir {}", extra.display()),
    )
    .unwrap();

    assert_eq!(state.working_dirs, vec![extra.clone()]);
    assert!(state.transcript.last().is_some_and(|message| message
        .text
        .contains("already accessible within the existing working directory")));
}

#[test]
fn resume_switches_to_matching_session_record() {
    let tempdir = tempdir().unwrap();
    let repo_root = tempdir.path().join("repo");
    let primary_cwd = repo_root.join("primary");
    let secondary_cwd = repo_root.join("secondary");
    std::fs::create_dir_all(&primary_cwd).unwrap();
    std::fs::create_dir_all(&secondary_cwd).unwrap();
    init_git_repo(&repo_root);
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let primary = session_store.create_session(primary_cwd.clone()).unwrap();
    let secondary = session_store.create_session(secondary_cwd.clone()).unwrap();
    session_store
        .rename_session(secondary.id, "dockyard".to_string())
        .unwrap();
    session_store
        .append_event(
            secondary.id,
            TranscriptEvent::StateSnapshot {
                current_model: Some("anthropic/claude-sonnet-4-5".to_string()),
                current_provider: Some("anthropic".to_string()),
                theme: "lagoon".to_string(),
                prompt_color: "teal".to_string(),
                effort_level: "high".to_string(),
                fast_mode: true,
                plan_mode: false,
                plan_mode_attachment_turns: 0,
                plan_mode_attachment_count: 0,
                plan_mode_has_exited: false,
                plan_mode_needs_reentry_attachment: false,
                plan_mode_needs_exit_attachment: false,
                sandbox_mode: "workspace-write".to_string(),
                remote_name: None,
                remote_environment: None,
                remote_session_id: None,
                remote_session_url: None,
                remote_session_status: None,
                active_team_name: None,
                statusline_enabled: true,
                working_dirs: vec![tempdir.path().join("secondary").display().to_string()],
                claude_read_state: Vec::new(),
                session_allow_all: false,
                session_tool_permissions: std::collections::HashMap::new(),
            },
        )
        .unwrap();

    let mut state = AppState::new(PufferConfig::default(), primary_cwd, primary);
    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/resume dockyard",
    )
    .unwrap();

    assert_eq!(state.session.id, secondary.id);
    assert_eq!(state.config.theme, "lagoon");
    assert_eq!(
        state.current_model.as_deref(),
        Some("anthropic/claude-sonnet-4-5")
    );
}

#[test]
fn resume_matches_session_slug() {
    let tempdir = tempdir().unwrap();
    let repo_root = tempdir.path().join("repo");
    let primary_cwd = repo_root.join("primary");
    let secondary_cwd = repo_root.join("secondary");
    std::fs::create_dir_all(&primary_cwd).unwrap();
    std::fs::create_dir_all(&secondary_cwd).unwrap();
    init_git_repo(&repo_root);
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let primary = session_store.create_session(primary_cwd.clone()).unwrap();
    let secondary = session_store.create_session(secondary_cwd).unwrap();
    session_store
        .set_slug(secondary.id, Some("dockyard-run".to_string()))
        .unwrap();

    let mut state = AppState::new(PufferConfig::default(), primary_cwd, primary);
    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/resume dockyard-run",
    )
    .unwrap();

    assert_eq!(state.session.id, secondary.id);
}

#[test]
fn resume_reports_ambiguous_matches_without_switching_sessions() {
    let tempdir = tempdir().unwrap();
    let repo_root = tempdir.path().join("repo");
    let primary_cwd = repo_root.join("primary");
    let first_cwd = repo_root.join("dockyard-a");
    let second_cwd = repo_root.join("dockyard-b");
    std::fs::create_dir_all(&primary_cwd).unwrap();
    std::fs::create_dir_all(&first_cwd).unwrap();
    std::fs::create_dir_all(&second_cwd).unwrap();
    init_git_repo(&repo_root);
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let primary = session_store.create_session(primary_cwd.clone()).unwrap();
    let first = session_store.create_session(first_cwd).unwrap();
    let second = session_store.create_session(second_cwd).unwrap();
    session_store
        .rename_session(first.id, "Dockyard review".to_string())
        .unwrap();
    session_store
        .rename_session(second.id, "Dockyard follow-up".to_string())
        .unwrap();

    let mut state = AppState::new(PufferConfig::default(), primary_cwd, primary.clone());
    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/resume dockyard",
    )
    .unwrap();

    assert_eq!(state.session.id, primary.id);
    assert!(state
        .transcript
        .last()
        .unwrap()
        .text
        .contains("Found 2 sessions matching `dockyard`"));
}

#[test]
fn resume_listing_only_shows_sessions_from_same_repo_scope() {
    let tempdir = tempdir().unwrap();
    let repo_root = tempdir.path().join("repo");
    let current_cwd = repo_root.join("current");
    let sibling_cwd = repo_root.join("sibling");
    let other_root = tempdir.path().join("other");
    std::fs::create_dir_all(&current_cwd).unwrap();
    std::fs::create_dir_all(&sibling_cwd).unwrap();
    std::fs::create_dir_all(&other_root).unwrap();
    init_git_repo(&repo_root);
    init_git_repo(&other_root);

    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let current = session_store.create_session(current_cwd.clone()).unwrap();
    let sibling = session_store.create_session(sibling_cwd).unwrap();
    let other = session_store.create_session(other_root.clone()).unwrap();
    session_store
        .rename_session(sibling.id, "Sibling review".to_string())
        .unwrap();
    session_store
        .rename_session(other.id, "Other project".to_string())
        .unwrap();

    let mut state = AppState::new(PufferConfig::default(), current_cwd, current);
    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/resume",
    )
    .unwrap();

    let listing = &state.transcript.last().unwrap().text;
    assert!(listing.contains("Sibling review"));
    assert!(!listing.contains("Other project"));
}

#[test]
fn resume_cross_project_session_prints_rerun_command_instead_of_switching() {
    let tempdir = tempdir().unwrap();
    let current_root = tempdir.path().join("current");
    let other_root = tempdir.path().join("other");
    std::fs::create_dir_all(&current_root).unwrap();
    std::fs::create_dir_all(&other_root).unwrap();
    init_git_repo(&current_root);
    init_git_repo(&other_root);

    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let current = session_store.create_session(current_root.clone()).unwrap();
    let other = session_store.create_session(other_root.clone()).unwrap();

    let mut state = AppState::new(PufferConfig::default(), current_root, current.clone());
    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        format!("/resume {}", other.id).as_str(),
    )
    .unwrap();

    assert_eq!(state.session.id, current.id);
    let message = &state.transcript.last().unwrap().text;
    assert!(message.contains("different directory"));
    assert!(message.contains("puffer resume"));
    assert!(message.contains(other.id.to_string().as_str()));
    assert!(message.contains(other_root.display().to_string().as_str()));
}

#[test]
fn tag_toggles_session_tags() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().join("primary"))
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().join("primary"),
        session,
    );

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/tag bugfix",
    )
    .unwrap();
    assert_eq!(state.session.tags, vec!["bugfix".to_string()]);

    dispatch_command(
        &mut state,
        &supported_commands(),
        &LoadedResources::default(),
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/tag bugfix",
    )
    .unwrap();
    assert!(state.session.tags.is_empty());
}

fn init_git_repo(path: &std::path::Path) {
    let output = Command::new("git").arg("init").arg(path).output().unwrap();
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
