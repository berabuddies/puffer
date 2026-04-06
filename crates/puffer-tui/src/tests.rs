use super::*;
use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_provider_registry::{AuthMode, ModelDescriptor, OAuthCredential, ProviderDescriptor};
use puffer_resources::{
    IdeSpec, LoadedItem, MascotSpec, McpServerSpec, PluginCommandSpec, PluginSpec, PromptTemplate,
    SkillSpec, SourceInfo, SourceKind, ToolSpec,
};
use puffer_session_store::{SessionMetadata, SessionStore};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use std::path::PathBuf;
use tempfile::tempdir;
use uuid::Uuid;

#[test]
fn render_shows_command_popup_for_slash_input() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    terminal
        .draw(|frame| {
            render::render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "/rev",
                4,
                0,
                0,
                &supported_commands(),
            )
        })
        .unwrap();
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains("/review"));
    assert!(rendered.contains("Review the current worktree"));
}

#[test]
fn parse_shell_shortcut_accepts_bang_prefix() {
    assert_eq!(parse_shell_shortcut("!printf hi"), Some("printf hi"));
    assert_eq!(parse_shell_shortcut("!!printf hi"), Some("printf hi"));
    assert_eq!(parse_shell_shortcut("/help"), None);
}

#[test]
fn slash_completion_appends_argument_space_for_commands_with_args() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_char('/', &commands);
    tui.insert_char('m', &commands);
    tui.insert_char('o', &commands);
    assert!(tui.apply_selected_command(&commands));
    assert_eq!(tui.input, "/model ");
    assert_eq!(tui.cursor, tui.input.len());
}

#[test]
fn enter_completion_prefers_selected_slash_command() {
    let commands = supported_commands();
    let mut tui = TuiState::default();
    tui.insert_char('/', &commands);
    tui.insert_char('r', &commands);
    tui.insert_char('e', &commands);
    tui.insert_char('v', &commands);
    assert!(tui.complete_on_enter(&commands));
    assert_eq!(tui.input, "/review");
}

#[test]
fn transcript_scroll_clamps_to_available_lines() {
    let mut tui = TuiState::default();
    tui.scroll_down(5, 3);
    assert_eq!(tui.scroll_offset, 2);
    tui.scroll_up(1);
    assert_eq!(tui.scroll_offset, 1);
}

#[test]
fn try_open_overlay_builds_resume_picker() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    session_store
        .create_session(tempdir.path().join("dockyard"))
        .unwrap();

    let state = sample_state();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/resume",
    )
    .unwrap();
    assert!(opened);
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::SessionPicker { .. })
    ));
}

#[test]
fn try_open_overlay_builds_agent_picker() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let mut state = sample_state();
    state.cwd = tempdir.path().to_path_buf();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/agents",
    )
    .unwrap();
    assert!(opened);
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::AgentPicker { .. })
    ));
}

#[test]
fn try_open_overlay_builds_login_picker() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let state = sample_state();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/login",
    )
    .unwrap();
    assert!(opened);
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::ProviderPicker { .. }) | Some(OverlayState::LoginPicker { .. })
    ));
    let entries = match &tui.overlay {
        Some(OverlayState::ProviderPicker { entries, .. })
        | Some(OverlayState::LoginPicker { entries, .. }) => entries,
        _ => unreachable!("login picker"),
    };
    assert!(entries.len() >= 2);
    assert!(entries.iter().any(|entry| entry.selector == "anthropic"));
    assert!(entries.iter().any(|entry| entry.selector == "openai"));
}

#[test]
fn try_open_overlay_builds_logout_picker() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let state = sample_state();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/logout",
    )
    .unwrap();
    assert!(opened);
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::LogoutPicker { .. })
    ));
}

#[test]
fn try_open_overlay_builds_theme_picker() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let state = sample_state();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/theme",
    )
    .unwrap();
    assert!(opened);
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::ThemePicker { .. })
    ));
}

#[test]
fn model_overlay_selected_command_uses_selector() {
    let overlay = OverlayState::ModelPicker {
        provider_id: "anthropic".to_string(),
        entries: vec![ModelPickerEntry {
            selector: "anthropic/claude-sonnet-4-5".to_string(),
            description: "Claude Sonnet 4.5".to_string(),
        }],
        selection: 0,
        onboarding: false,
    };
    assert_eq!(
        overlay.selected_command().as_deref(),
        Some("/model anthropic/claude-sonnet-4-5")
    );
}

#[test]
fn render_shows_status_line_when_enabled() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    terminal
        .draw(|frame| {
            render::render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &supported_commands(),
            )
        })
        .unwrap();
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains("anthropic · anthropic/claude-sonnet-4-5 · auth"));
    assert!(rendered.contains("/help · /review · !pwd"));
    assert!(rendered.contains("workspace-write"));
}

fn sample_state() -> AppState {
    let mut state = AppState::new(
        PufferConfig::default(),
        PathBuf::from("/workspace/puffer"),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: Some("demo".to_string()),
            cwd: PathBuf::from("/workspace/puffer"),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: Some("demo-session".to_string()),
            tags: vec!["review".to_string()],
            note: Some("Focus on transport parity".to_string()),
        },
    );
    state.statusline_enabled = true;
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
    state.prompt_color = "amber".to_string();
    state.effort_level = "high".to_string();
    state.fast_mode = true;
    state.sandbox_mode = "workspace-write".to_string();
    state.remote_name = Some("buildbox".to_string());
    state.remote_environment = Some("linux".to_string());
    state.push_message(MessageRole::User, "/review");
    state.push_message(
        MessageRole::Assistant,
        "# Review\n- Check command coverage\n- Tighten request parity",
    );
    state
}

fn sample_resources() -> LoadedResources {
    LoadedResources {
        tools: vec![
            loaded_item(
                "tools/bash.yaml",
                ToolSpec {
                    id: "bash".to_string(),
                    name: "bash".to_string(),
                    description: "Run shell commands".to_string(),
                    handler: "bash".to_string(),
                    handler_args: Vec::new(),
                    approval_policy: Some("on-request".to_string()),
                    sandbox_policy: Some("workspace-write".to_string()),
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: None,
                    metadata: Default::default(),
                    display: Default::default(),
                },
            ),
            loaded_item(
                "tools/read_file.yaml",
                ToolSpec {
                    id: "read_file".to_string(),
                    name: "read_file".to_string(),
                    description: "Read a file".to_string(),
                    handler: "read_file".to_string(),
                    handler_args: Vec::new(),
                    approval_policy: Some("never".to_string()),
                    sandbox_policy: Some("read-only".to_string()),
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: None,
                    metadata: Default::default(),
                    display: Default::default(),
                },
            ),
            loaded_item(
                "tools/write_file.yaml",
                ToolSpec {
                    id: "write_file".to_string(),
                    name: "write_file".to_string(),
                    description: "Write a file".to_string(),
                    handler: "write_file".to_string(),
                    handler_args: Vec::new(),
                    approval_policy: Some("on-request".to_string()),
                    sandbox_policy: Some("workspace-write".to_string()),
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: None,
                    metadata: Default::default(),
                    display: Default::default(),
                },
            ),
        ],
        prompts: vec![loaded_item(
            "prompts/review.yaml",
            PromptTemplate {
                id: "review".to_string(),
                description: "Review pending changes".to_string(),
                template: "Review $ARGUMENTS".to_string(),
                variables: Vec::new(),
                provider_override: None,
                model_override: None,
                mode: None,
                chained_from: Vec::new(),
            },
        )],
        skills: vec![loaded_item(
            "skills/reviewer.yaml",
            SkillSpec {
                name: "reviewer".to_string(),
                description: "Code review helper".to_string(),
                content: "Review code carefully".to_string(),
                disable_model_invocation: false,
            },
        )],
        mascots: vec![loaded_item(
            "mascots/clawd.yaml",
            MascotSpec {
                id: "clawd".to_string(),
                display_name: "Clawd".to_string(),
                introduction: "A diligent pufferfish".to_string(),
            },
        )],
        plugins: vec![loaded_item(
            "plugins/git.yaml",
            PluginSpec {
                id: "git".to_string(),
                display_name: "Git".to_string(),
                description: "Git helpers".to_string(),
                commands: vec![PluginCommandSpec {
                    name: "review".to_string(),
                    description: "Review a diff".to_string(),
                }],
                skills: vec!["reviewer".to_string()],
                mcp_servers: vec![McpServerSpec {
                    id: "git-mcp".to_string(),
                    display_name: "Git MCP".to_string(),
                    transport: "stdio".to_string(),
                    endpoint: String::new(),
                    target: "git".to_string(),
                    description: "Git bridge".to_string(),
                }],
            },
        )],
        mcp_servers: vec![loaded_item(
            "mcp_servers/local.yaml",
            McpServerSpec {
                id: "local".to_string(),
                display_name: "Local MCP".to_string(),
                transport: "stdio".to_string(),
                endpoint: String::new(),
                target: "local".to_string(),
                description: "Local tool bridge".to_string(),
            },
        )],
        ides: vec![loaded_item(
            "ides/vscode.yaml",
            IdeSpec {
                id: "vscode".to_string(),
                display_name: "VS Code".to_string(),
                description: "VS Code bridge".to_string(),
            },
        )],
        ..LoadedResources::default()
    }
}

fn sample_providers() -> ProviderRegistry {
    let mut providers = ProviderRegistry::default();
    providers.register(ProviderDescriptor {
        id: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        default_api: "anthropic-messages".to_string(),
        auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
        headers: Default::default(),
        discovery: None,
        models: vec![
            ModelDescriptor {
                id: "claude-sonnet-4-5".to_string(),
                display_name: "Claude Sonnet 4.5".to_string(),
                provider: "anthropic".to_string(),
                api: "anthropic-messages".to_string(),
                context_window: 200_000,
                max_output_tokens: 8_192,
                supports_reasoning: true,
            },
            ModelDescriptor {
                id: "claude-opus-4-1".to_string(),
                display_name: "Claude Opus 4.1".to_string(),
                provider: "anthropic".to_string(),
                api: "anthropic-messages".to_string(),
                context_window: 200_000,
                max_output_tokens: 8_192,
                supports_reasoning: true,
            },
        ],
    });
    providers.register(ProviderDescriptor {
        id: "openai".to_string(),
        display_name: "OpenAI".to_string(),
        base_url: "https://api.openai.com".to_string(),
        default_api: "responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
        headers: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            provider: "openai".to_string(),
            api: "responses".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
        }],
    });
    providers.register(ProviderDescriptor {
        id: "ollama".to_string(),
        display_name: "Ollama".to_string(),
        base_url: "http://127.0.0.1:11434".to_string(),
        default_api: "openai-completions".to_string(),
        auth_modes: Vec::new(),
        headers: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            id: "qwen3:14b".to_string(),
            display_name: "qwen3:14b".to_string(),
            provider: "ollama".to_string(),
            api: "openai-completions".to_string(),
            context_window: 32_768,
            max_output_tokens: 8_192,
            supports_reasoning: false,
        }],
    });
    providers
}

fn sample_auth_store() -> AuthStore {
    let mut auth_store = AuthStore::default();
    auth_store.set_oauth(
        "anthropic",
        OAuthCredential {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at_ms: 100,
            account_id: Some("acct".to_string()),
            organization_id: None,
            email: Some("operator@example.com".to_string()),
            plan_type: None,
            rate_limit_tier: None,
            scopes: vec!["org:create_api_key".to_string()],
            organization_name: None,
            organization_role: None,
            workspace_role: None,
        },
    );
    auth_store
}

fn loaded_item<T>(path: &str, value: T) -> LoadedItem<T> {
    LoadedItem {
        value,
        source_info: SourceInfo {
            path: PathBuf::from(path),
            kind: SourceKind::Builtin,
        },
    }
}

fn buffer_to_string(buffer: &Buffer) -> String {
    let area = buffer.area();
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
