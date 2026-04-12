use super::*;
use crate::flow::{handle_auth_command, parse_shell_shortcut};
use crate::state::AuthPickerEntry;
use puffer_config::{ensure_workspace_dirs, save_user_config, ConfigPaths, PufferConfig};
use puffer_core::{supported_commands, MessageRole};
use puffer_provider_registry::{
    AuthMode, ExternalImportCandidate, ExternalImportFamily, ExternalImportSource, ModelDescriptor,
    OAuthCredential, ProviderDescriptor, StoredCredential,
};
use puffer_resources::{
    IdeSpec, LoadedItem, MascotSpec, McpServerSpec, PluginCommandSpec, PluginSpec, PromptTemplate,
    ProviderPack, SkillSpec, SourceInfo, SourceKind, ToolSpec,
};
use puffer_session_store::{SessionMetadata, SessionStore};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tempfile::tempdir;
use uuid::Uuid;
mod help;
mod model_selection;
mod overlays;
mod panels;
mod permission_prompt;
mod status;
mod support;
mod tag;
use support::*;

fn puffer_home_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

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
    tui.follow_output = false;
    tui.scroll_down(5, 3, 1);
    assert_eq!(tui.scroll_offset, 2);
    tui.scroll_up(1, 3, 1);
    assert_eq!(tui.scroll_offset, 1);
}

#[test]
fn try_open_overlay_builds_skills_panel() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let state = sample_state();
    let resources = sample_resources();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &resources,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/skills",
    )
    .unwrap();
    assert!(opened);
    assert!(matches!(tui.overlay, Some(OverlayState::Text(_))));
}

#[test]
fn startup_resume_action_seeds_picker_query() {
    let tempdir = tempdir().unwrap();
    let repo_root = tempdir.path().join("repo");
    let current_cwd = repo_root.join("current");
    let sibling_cwd = repo_root.join("dockyard");
    std::fs::create_dir_all(&current_cwd).unwrap();
    std::fs::create_dir_all(&sibling_cwd).unwrap();
    let output = std::process::Command::new("git")
        .arg("init")
        .arg(&repo_root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let current = session_store.create_session(current_cwd.clone()).unwrap();
    let other = session_store.create_session(sibling_cwd).unwrap();
    session_store
        .rename_session(other.id, "dockyard".to_string())
        .unwrap();

    let mut state = sample_state();
    state.session.id = current.id;
    state.cwd = current_cwd;
    state.session.cwd = state.cwd.clone();
    let resources = sample_resources();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();

    apply_startup_action(
        &state,
        &resources,
        &mut providers,
        &auth_store,
        &session_store,
        &StartupAction::ResumePicker {
            query: Some("dock".to_string()),
        },
        &mut tui,
    )
    .unwrap();

    assert_eq!(tui.input, "dock");
    assert_eq!(tui.cursor, 4);
    match tui.overlay {
        Some(OverlayState::SessionPicker {
            sessions,
            selection,
        }) => {
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[selection].id, other.id);
        }
        _ => panic!("resume picker"),
    }
}

#[test]
fn task_picker_enter_opens_dashboard_overlay() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let auth_path = paths.user_config_dir.join("auth.json");

    let mut state = sample_state();
    state.cwd = tempdir.path().to_path_buf();
    state.session.cwd = tempdir.path().to_path_buf();
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    let mut tui = TuiState::default();

    assert!(try_open_overlay(
        &state,
        &resources,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/tasks",
    )
    .unwrap());
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::CommandPicker { .. })
    ));

    handle_overlay_key(
        KeyEvent::from(KeyCode::Enter),
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

    assert!(matches!(tui.overlay, Some(OverlayState::Text(_))));
}

#[test]
fn try_open_overlay_builds_login_picker() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let state = sample_state();
    let resources = sample_resources();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &resources,
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
fn login_picker_query_selects_matching_provider() {
    let mut overlay = OverlayState::LoginPicker {
        entries: vec![
            ModelPickerEntry {
                selector: "anthropic".to_string(),
                description: "Anthropic".to_string(),
                command: None,
            },
            ModelPickerEntry {
                selector: "openai".to_string(),
                description: "OpenAI".to_string(),
                command: None,
            },
        ],
        selection: 0,
    };

    overlay.select_matching_query("open");
    assert_eq!(overlay.selected_provider(), Some("openai"));
}

#[test]
fn try_open_overlay_builds_logout_picker() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let state = sample_state();
    let resources = sample_resources();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &resources,
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
fn logout_clears_active_provider_selection() {
    let tempdir = tempdir().unwrap();
    let _lock = puffer_home_lock().lock().unwrap();
    let old_home = std::env::var_os("PUFFER_HOME");
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::env::set_var("PUFFER_HOME", &home);

    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("anthropic".to_string());
    config.default_model = Some("anthropic/claude-sonnet-4-5".to_string());
    save_user_config(&paths, &config).unwrap();
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = sample_auth_store();
    auth_store.save(&auth_path).unwrap();

    handle_auth_command(
        &mut state,
        &mut auth_store,
        &auth_path,
        &session_store,
        "/logout anthropic",
        true,
    )
    .unwrap();

    assert_eq!(state.current_provider, None);
    assert_eq!(state.current_model, None);
    assert_eq!(state.config.default_provider, None);
    assert_eq!(state.config.default_model, None);
    assert!(!auth_store.has_auth("anthropic"));

    if let Some(value) = old_home {
        std::env::set_var("PUFFER_HOME", value);
    } else {
        std::env::remove_var("PUFFER_HOME");
    }
}

#[test]
fn logout_clears_selection_when_model_provider_matches_logged_out_provider() {
    let tempdir = tempdir().unwrap();
    let _lock = puffer_home_lock().lock().unwrap();
    let old_home = std::env::var_os("PUFFER_HOME");
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::env::set_var("PUFFER_HOME", &home);

    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("anthropic".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    save_user_config(&paths, &config).unwrap();
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = sample_auth_store();
    auth_store.set_api_key("openai", "sk-openai");
    auth_store.save(&auth_path).unwrap();

    handle_auth_command(
        &mut state,
        &mut auth_store,
        &auth_path,
        &session_store,
        "/logout openai",
        true,
    )
    .unwrap();

    assert_eq!(state.current_provider, None);
    assert_eq!(state.current_model, None);
    assert_eq!(state.config.default_provider, None);
    assert_eq!(state.config.default_model, None);
    assert!(!auth_store.has_auth("openai"));

    let mut providers = sample_providers();
    let mut resources = sample_resources();
    let mut tui = TuiState::default();
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
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::ProviderPicker {
            onboarding: true,
            ..
        })
    ));

    if let Some(value) = old_home {
        std::env::set_var("PUFFER_HOME", value);
    } else {
        std::env::remove_var("PUFFER_HOME");
    }
}

#[test]
fn missing_auth_for_selected_provider_reopens_auth_picker() {
    let tempdir = tempdir().unwrap();
    let _lock = puffer_home_lock().lock().unwrap();
    let old_home = std::env::var_os("PUFFER_HOME");
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::env::set_var("PUFFER_HOME", &home);

    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.default_provider = Some("openai".to_string());
    config.default_model = Some("openai/gpt-5".to_string());
    save_user_config(&paths, &config).unwrap();
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let mut providers = sample_providers();
    let mut resources = sample_resources();
    let mut tui = TuiState::default();

    submit_queued_prompt_if_ready(
        &mut state,
        &mut resources,
        &mut providers,
        &mut AuthStore::default(),
        &paths.user_config_dir.join("auth.json"),
        &session_store,
        &mut tui,
        true,
    )
    .unwrap();

    assert!(matches!(
        tui.overlay,
        Some(OverlayState::AuthPicker { ref provider_id, .. }) if provider_id == "openai"
    ));

    if let Some(value) = old_home {
        std::env::set_var("PUFFER_HOME", value);
    } else {
        std::env::remove_var("PUFFER_HOME");
    }
}

#[test]
fn try_open_overlay_builds_theme_picker() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let state = sample_state();
    let resources = sample_resources();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &resources,
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
fn try_open_overlay_builds_fast_mode_picker_for_current_model() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();

    let state = sample_state();
    let resources = sample_resources();
    let mut providers = sample_providers();
    let auth_store = sample_auth_store();
    let mut tui = TuiState::default();
    let opened = try_open_overlay(
        &state,
        &resources,
        &mut providers,
        &auth_store,
        &session_store,
        &mut tui,
        "/fast",
    )
    .unwrap();
    assert!(opened);
    assert!(matches!(
        tui.overlay,
        Some(OverlayState::FastModePicker { .. })
    ));
}

#[test]
fn codex_import_without_base_url_clears_previous_openai_override() {
    let tempdir = tempdir().unwrap();
    let _lock = puffer_home_lock().lock().unwrap();
    let old_home = std::env::var_os("PUFFER_HOME");
    let home = tempdir.path().join("home");
    let workspace = tempdir.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::env::set_var("PUFFER_HOME", &home);

    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(workspace.clone()).unwrap();
    let mut config = PufferConfig::default();
    config.openai_base_url = Some("https://stale.example/v1".to_string());
    config.openai_headers = BTreeMap::from([("x-stale".to_string(), "present".to_string())]);
    config.openai_query_params = BTreeMap::from([("api-version".to_string(), "stale".to_string())]);
    save_user_config(&paths, &config).unwrap();
    let mut state = AppState::new(config, workspace, session);
    state.current_provider = Some("openai".to_string());
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::default();
    let mut providers = sample_providers();
    providers.set_openai_base_url("https://stale.example/v1");
    providers.set_openai_headers(indexmap::IndexMap::from([(
        "x-stale".to_string(),
        "present".to_string(),
    )]));
    providers.set_openai_query_params(indexmap::IndexMap::from([(
        "api-version".to_string(),
        "stale".to_string(),
    )]));
    let mut resources = openai_provider_resources();
    let mut tui = TuiState {
        overlay: Some(OverlayState::AuthPicker {
            provider_id: "openai".to_string(),
            entries: vec![AuthPickerEntry {
                label: "import-codex".to_string(),
                description: "Import Codex OAuth".to_string(),
                action: AuthPickerAction::Import(ExternalImportCandidate {
                    source: ExternalImportSource::Codex,
                    family: ExternalImportFamily::OpenAi,
                    description: "Import Codex OAuth".to_string(),
                    source_path: PathBuf::from("/tmp/codex/auth.json"),
                    credential: StoredCredential::ApiKey {
                        key: "sk-openai".to_string(),
                    },
                    openai_base_url: None,
                    openai_headers: BTreeMap::new(),
                    openai_query_params: BTreeMap::new(),
                }),
            }],
            selection: 0,
            onboarding: false,
        }),
        ..TuiState::default()
    };

    handle_overlay_key(
        KeyEvent::from(KeyCode::Enter),
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

    let saved = puffer_config::load_config(&paths).unwrap();
    assert_eq!(saved.openai_base_url, None);
    assert!(saved.openai_headers.is_empty());
    assert!(saved.openai_query_params.is_empty());
    assert_eq!(
        providers
            .provider("openai")
            .map(|provider| provider.base_url.as_str()),
        Some("https://api.openai.com")
    );
    assert!(providers
        .provider("openai")
        .map(|provider| provider.headers.is_empty())
        .unwrap_or(false));
    assert!(providers
        .provider("openai")
        .map(|provider| provider.query_params.is_empty())
        .unwrap_or(false));

    if let Some(value) = old_home {
        std::env::set_var("PUFFER_HOME", value);
    } else {
        std::env::remove_var("PUFFER_HOME");
    }
}

#[test]
fn model_overlay_selected_command_uses_selector() {
    let overlay = OverlayState::ModelPicker {
        provider_id: "anthropic".to_string(),
        entries: vec![ModelPickerEntry {
            selector: "anthropic/claude-sonnet-4-5".to_string(),
            description: "Claude Sonnet 4.5".to_string(),
            command: None,
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
    assert!(rendered.contains("anthropic"));
    assert!(rendered.contains("sandbox workspace-write"));
}

#[test]
fn render_shows_overlay_query_in_prompt_row() {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    let overlay = OverlayState::LoginPicker {
        entries: vec![
            ModelPickerEntry {
                selector: "anthropic".to_string(),
                description: "Anthropic".to_string(),
                command: None,
            },
            ModelPickerEntry {
                selector: "openai".to_string(),
                description: "OpenAI".to_string(),
                command: None,
            },
        ],
        selection: 1,
    };
    terminal
        .draw(|frame| {
            render::set_active_overlay(Some(overlay.clone()));
            render::render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "open",
                4,
                0,
                0,
                &supported_commands(),
            )
        })
        .unwrap();
    let rendered = buffer_to_string(terminal.backend().buffer());
    assert!(rendered.contains("open"));
    assert!(rendered.contains("Select Provider"));
    assert!(rendered.contains("Type") || rendered.contains("Typing jumps"));
}

#[test]
fn composer_separator_uses_shiny_orange_border() {
    let backend = TestBackend::new(120, 30);
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

    let buffer = terminal.backend().buffer();
    let viewport = render::current_transcript_viewport();
    let separator_y = viewport.y + viewport.height;

    assert_eq!(buffer[(0, separator_y)].symbol(), "─");
    assert_eq!(buffer[(0, separator_y)].fg, Color::Indexed(214));
    assert_eq!(
        buffer[(buffer.area().width / 2, separator_y)].fg,
        Color::Indexed(214)
    );
}

#[test]
fn ctrl_o_toggles_tool_detail_expansion() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let mut state = sample_state();
    let mut resources = sample_resources();
    let mut providers = sample_providers();
    let mut auth_store = sample_auth_store();
    let auth_path = paths.user_config_dir.join("auth.json");
    let commands = supported_commands();
    let mut tui = TuiState::default();

    assert!(!tui.tool_details_expanded);
    handle_key(
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        &mut state,
        &mut resources,
        &mut providers,
        &mut auth_store,
        &auth_path,
        &session_store,
        &commands,
        &mut tui,
        true,
    )
    .unwrap();
    assert!(tui.tool_details_expanded);
}
