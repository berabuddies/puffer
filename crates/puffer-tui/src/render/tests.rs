use super::*;
use crate::usage::UsageOverlay;
use crate::ModelPickerEntry;
use insta::assert_snapshot;
use puffer_config::PufferConfig;
use puffer_core::CommandKind;
use puffer_core::ToolCallRequest;
use puffer_provider_openai::OpenAIUsageSummary;
use puffer_provider_registry::{
    AuthMode, ModelDescriptor, OAuthCredential, ProviderDescriptor, ProviderRegistry,
};
use puffer_resources::{LoadedItem, SourceInfo, SourceKind, ToolSpec};
use puffer_session_store::SessionMetadata;
use puffer_test_support::{assert_normalized_snapshot, read_normalized_snapshot};
use puffer_transport_anthropic::{AnthropicExtraUsage, AnthropicRateLimit, AnthropicUtilization};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::PathBuf;
use uuid::Uuid;

#[test]
fn session_lines_include_lineage_tags_and_note() {
    let state = sample_state();
    let lines = session_lines(&state).join("\n");
    assert!(lines.contains("Name: Shipyard"));
    assert!(lines.contains("Parent: abcdefab"));
    assert!(lines.contains("Tags: review,tmux"));
    assert!(lines.contains("Note: finish tool parity"));
}

#[test]
fn header_snapshot_reports_compact_status() {
    let state = sample_state();
    let resources = sample_resources();
    let auth_store = sample_auth_store();
    let registry = ToolRegistry::from_resources(&resources);
    let snapshot = header_lines(
        &state,
        &resources,
        &auth_store,
        &registry,
        &ProviderRegistry::new(),
    )
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>()
    .join("\n");
    assert_snapshot!(
                                                                                                snapshot,
                                                                                                @r"
Puffer Code
Account   anthropic via API key
Model     anthropic/claude-sonnet-4-5 · tools 3/4
Session   Shipyard · 12345678-1234-5678-1234-567812345678
Mode      effort high · fast · vim
Context   puffer · 2 msgs · 2 wds · dockyard@staging
"
                                                                                            );
}

#[test]
fn footer_snapshot_reports_compact_prompt_rail() {
    let state = sample_state();
    let providers = sample_providers();
    let snapshot = footer_lines(&state, &providers)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_snapshot!(
                                                                                                snapshot,
                                                                                                @r"
anthropic/claude-sonnet-4-5 · 99% left · /tmp/puffer · sandbox workspace-write
"
                                                                                            );
}

#[test]
fn header_snapshot_includes_oauth_identity_when_available() {
    let mut state = sample_state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let resources = sample_resources();
    let auth_store = sample_auth_store();
    let registry = ToolRegistry::from_resources(&resources);
    let snapshot = header_lines(
        &state,
        &resources,
        &auth_store,
        &registry,
        &ProviderRegistry::new(),
    )
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>()
    .join("\n");
    assert_snapshot!(
                                                                                                snapshot,
                                                                                                @r"
Puffer Code
Account   dev@example.com · plan Pro · acct acct-1
Model     openai/gpt-5 · tools 3/4
Session   Shipyard · 12345678-1234-5678-1234-567812345678
Mode      effort high · fast · vim
Context   puffer · 2 msgs · 2 wds · dockyard@staging
"
                                                                                            );
}

#[test]
fn render_draws_sparse_transcript_and_popup() {
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    terminal
        .draw(|frame| {
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "/re",
                3,
                0,
                0,
                &sample_commands(),
            )
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("Puffer Code"));
    assert!(rendered.contains("Account"));
    assert!(rendered.contains("Session"));
    assert!(rendered.contains("working tree clean"));
    assert!(rendered.contains("/review"));
}

#[test]
fn render_layout_includes_header_body_and_composer() {
    let backend = TestBackend::new(100, 36);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    terminal
        .draw(|frame| {
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            )
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    let lines: Vec<&str> = rendered.lines().collect();
    assert!(rendered.contains("Puffer Code"));
    assert!(lines.iter().any(|line| line.contains("Account")));
    assert!(lines.iter().any(|line| line.contains("Session")));
    assert!(lines.iter().any(|line| line.contains("working tree clean")));
    assert!(rendered.contains("anthropic/claude-sonnet-4-5"));
    assert!(rendered.contains("sandbox workspace-write"));
}

#[test]
fn render_post_send_turn_matches_snapshot() {
    let terminal = render_post_send_terminal();
    assert_normalized_snapshot(
        &terminal,
        &snapshot_path("render_post_send_turn_snapshot.txt"),
    )
    .unwrap();
}

#[test]
fn render_pending_submit_shows_loading_below_prompt() {
    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = sample_state();
    state.transcript.clear();
    state.push_message(
        MessageRole::User,
        "Review the current worktree and call out any risks.",
    );
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    terminal
        .draw(|frame| {
            set_pending_submit_state(
                Some("Review the current worktree and call out any risks.".to_string()),
                Vec::new(),
                Vec::new(),
                Some(std::time::Instant::now()),
                false,
                None,
            );
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            );
            set_pending_submit_state(None, Vec::new(), Vec::new(), None, false, None);
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("⎿ Loading..."));
    assert!(rendered.contains("Review the current worktree and call out any risks."));
    assert!(
        rendered
            .find("Review the current worktree and call out any risks.")
            .unwrap()
            < rendered.find("Loading...").unwrap()
    );
}

#[test]
fn render_pending_submit_shows_tool_call_before_output() {
    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = sample_state();
    state.transcript.clear();
    state.push_message(MessageRole::User, "check python");
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    terminal
        .draw(|frame| {
            set_pending_submit_state(
                Some("check python".to_string()),
                vec![ToolCallRequest {
                    call_id: "call_python_version".to_string(),
                    tool_id: "bash".to_string(),
                    input: "{\"command\":\"python --version 2>&1; python3 --version 2>&1\"}"
                        .to_string(),
                }],
                Vec::new(),
                Some(std::time::Instant::now()),
                false,
                None,
            );
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            );
            set_pending_submit_state(None, Vec::new(), Vec::new(), None, false, None);
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("Bash python --version 2>&1; python3 --version 2>&1"));
    assert!(rendered.contains("Loading..."));
}

#[test]
fn render_pending_submit_shows_queued_prompts() {
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = sample_state();
    state.transcript.clear();
    state.push_message(MessageRole::User, "first prompt");
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    terminal
        .draw(|frame| {
            set_pending_submit_state(
                Some("first prompt".to_string()),
                Vec::new(),
                vec!["second prompt".to_string(), "third prompt".to_string()],
                Some(std::time::Instant::now()),
                false,
                None,
            );
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            );
            set_pending_submit_state(None, Vec::new(), Vec::new(), None, false, None);
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("⎿ Loading..."));
    assert!(rendered.contains("› second prompt"));
    assert!(rendered.contains("⎿ Queued"));
    assert!(rendered.contains("› third prompt"));
}

#[test]
fn render_post_send_compared_with_claude_reference_matches_snapshot() {
    let puffer = render_post_send_terminal();
    let claude =
        read_normalized_snapshot(&snapshot_path("claude_post_send_reference_snapshot.txt"))
            .unwrap();
    assert_normalized_snapshot(
        &post_send_comparison_report(&claude, &puffer),
        &snapshot_path("render_post_send_vs_claude_reference_snapshot.txt"),
    )
    .unwrap();
}

#[test]
fn render_empty_state_shows_transcript_guidance() {
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = sample_state();
    state.transcript.clear();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    terminal
        .draw(|frame| {
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            )
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("Puffer Code"));
    assert!(rendered.contains("Account"));
    assert!(has_top_panel(&rendered));
    assert!(!rendered.contains("Welcome to Puffer Code"));
    assert!(!rendered.contains("Tips to get started"));
}

#[test]
fn render_empty_state_compacts_on_narrow_width() {
    let backend = TestBackend::new(72, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = sample_state();
    state.transcript.clear();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    terminal
        .draw(|frame| {
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            )
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("Puffer Code"));
    assert!(has_top_panel(&rendered));
    assert!(!rendered.contains("Welcome to Puffer Code"));
    assert!(!rendered.contains("██"));
    assert!(rendered.contains("100% left"));
    assert!(rendered.contains("Review changes, ask a question, or type /"));
}

#[test]
fn render_onboarding_empty_state_keeps_fixed_top_panel() {
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = sample_state();
    state.transcript.clear();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    terminal
        .draw(|frame| {
            set_active_overlay(Some(OverlayState::ThemePicker {
                entries: vec![
                    ModelPickerEntry {
                        selector: "puffer".to_string(),
                        description: "Default Puffer text style".to_string(),
                        command: None,
                    },
                    ModelPickerEntry {
                        selector: "harbor".to_string(),
                        description: "Calmer contrast for long sessions".to_string(),
                        command: None,
                    },
                ],
                selection: 0,
            }));
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            );
            set_active_overlay(None);
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("Puffer Code"));
    assert!(has_top_panel(&rendered));
}

#[test]
fn render_very_narrow_header_hides_puffer_and_truncates_text() {
    let backend = TestBackend::new(50, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = sample_state();
    state.transcript.clear();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    terminal
        .draw(|frame| {
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            )
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("Puffer Code"));
    assert!(!rendered.contains("██"));
    assert!(rendered
        .lines()
        .any(|line| line.contains("Session") && line.contains("...")));
}

#[test]
fn render_help_pane_reflows_on_narrow_width() {
    let backend = TestBackend::new(72, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = sample_state();
    state.push_message(
        MessageRole::System,
        "Supported commands:\n/help Show help and available commands".to_string(),
    );
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    terminal
        .draw(|frame| {
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            )
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("Supported commands"));
    assert!(rendered.contains("/help"));
    assert!(rendered.contains("Esc to cancel"));
}

#[test]
fn render_usage_overlay_shows_loading_state() {
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    let overlay = OverlayState::Usage(UsageOverlay::loading_for_test());

    terminal
        .draw(|frame| {
            set_active_overlay(Some(overlay.clone()));
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            );
            set_active_overlay(None);
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("Usage"));
    assert!(rendered.contains("Loading usage data..."));
    assert!(rendered.contains("Esc cancel"));
}

#[test]
fn render_usage_overlay_shows_claude_style_sections() {
    let backend = TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    let overlay = OverlayState::Usage(UsageOverlay::ready_anthropic_for_test(
        Some("max"),
        AnthropicUtilization {
            five_hour: Some(AnthropicRateLimit {
                utilization: Some(42.0),
                resets_at: Some("2030-04-06T12:00:00Z".to_string()),
            }),
            seven_day: Some(AnthropicRateLimit {
                utilization: Some(64.0),
                resets_at: Some("2030-04-08T12:00:00Z".to_string()),
            }),
            seven_day_oauth_apps: None,
            seven_day_opus: None,
            seven_day_sonnet: Some(AnthropicRateLimit {
                utilization: Some(17.0),
                resets_at: Some("2030-04-08T12:00:00Z".to_string()),
            }),
            extra_usage: Some(AnthropicExtraUsage {
                is_enabled: true,
                monthly_limit: Some(5_000.0),
                used_credits: Some(1_234.0),
                utilization: Some(24.0),
            }),
        },
    ));

    terminal
        .draw(|frame| {
            set_active_overlay(Some(overlay.clone()));
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            );
            set_active_overlay(None);
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("Current session"));
    assert!(rendered.contains("Current week (all models)"));
    assert!(rendered.contains("Current week (Sonnet only)"));
    assert!(rendered.contains("Extra usage"));
    assert!(rendered.contains("$12.34 / $50.00 spent"));
    assert!(rendered.contains("Claude subscription usage"));
}

#[test]
fn render_usage_overlay_shows_error_retry_hint() {
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let state = sample_state();
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    let overlay = OverlayState::Usage(UsageOverlay::error_for_test("Failed to load usage data"));

    terminal
        .draw(|frame| {
            set_active_overlay(Some(overlay.clone()));
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            );
            set_active_overlay(None);
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("Error:"));
    assert!(rendered.contains("r retry"));
}

#[test]
fn render_usage_overlay_shows_openai_summary() {
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = sample_state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();
    let overlay = OverlayState::Usage(UsageOverlay::ready_openai_for_test(
        vec![
            "Provider: OpenAI".to_string(),
            "Authentication: OAuth".to_string(),
            "Logged in as: dev@example.com".to_string(),
            "Account ID: acct-1".to_string(),
            "Plan: Pro".to_string(),
        ],
        OpenAIUsageSummary {
            window_start_s: 0,
            window_end_s: 1,
            input_tokens: 1234,
            output_tokens: 567,
            input_cached_tokens: 89,
            num_model_requests: 7,
            total_cost_usd: Some(4.25),
        },
    ));

    terminal
        .draw(|frame| {
            set_active_overlay(Some(overlay.clone()));
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            );
            set_active_overlay(None);
        })
        .unwrap();

    let rendered = terminal_view(&terminal);
    assert!(rendered.contains("OpenAI/Codex account usage"));
    assert!(rendered.contains("Authentication: OAuth"));
    assert!(rendered.contains("Input tokens: 1,234"));
    assert!(rendered.contains("Output tokens: 567"));
    assert!(rendered.contains("Cost: $4.25"));
}

pub(super) fn sample_state() -> AppState {
    let mut config = PufferConfig::default();
    config.theme = "harbor".to_string();
    config.default_provider = Some("anthropic".to_string());
    config.default_model = Some("anthropic/claude-sonnet-4-5".to_string());
    config.mascot.display_name = "Clawd".to_string();

    let cwd = PathBuf::from("/tmp/puffer");
    let mut state = AppState::new(
        config,
        cwd.clone(),
        SessionMetadata {
            id: Uuid::parse_str("12345678-1234-5678-1234-567812345678").unwrap(),
            display_name: Some("Shipyard".to_string()),
            generated_title: None,
            cwd,
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: Some(
                Uuid::parse_str("abcdefab-1111-2222-3333-444455556666").unwrap(),
            ),
            slug: Some("shipyard-run".to_string()),
            tags: vec!["review".to_string(), "tmux".to_string()],
            note: Some("finish tool parity".to_string()),
        },
    );
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
    state.prompt_color = "cyan".to_string();
    state.effort_level = "high".to_string();
    state.fast_mode = true;
    state.vim_mode = true;
    state.remote_name = Some("dockyard".to_string());
    state.remote_environment = Some("staging".to_string());
    state.sandbox_mode = "workspace-write".to_string();
    state.working_dirs = vec![
        PathBuf::from("/tmp/puffer"),
        PathBuf::from("/tmp/puffer/ref"),
    ];
    state.push_message(MessageRole::User, "!git status");
    state.push_message(MessageRole::Assistant, "working tree clean");
    state
}

pub(super) fn sample_resources() -> LoadedResources {
    LoadedResources {
        tools: vec![
            tool_spec("bash", "bash", Some("on-request"), Some("workspace-write")),
            tool_spec("read_file", "read_file", None, Some("workspace-write")),
            tool_spec(
                "write_file",
                "write_file",
                Some("on-request"),
                Some("workspace-write"),
            ),
            tool_spec("custom_fetch", "custom_fetch", None, None),
        ],
        prompts: vec![
            loaded_prompt("review", "Review current work"),
            loaded_prompt("usage", "Show usage"),
        ],
        skills: vec![
            loaded_skill("rust-review", "Rust review guidance"),
            loaded_skill("tmux-parity", "tmux parity guidance"),
        ],
        plugins: vec![loaded_plugin("git", "Git plugin")],
        mcp_servers: vec![loaded_mcp("docs", "Docs server")],
        ides: vec![loaded_ide("zed", "Zed integration")],
        ..LoadedResources::default()
    }
}

pub(super) fn sample_providers() -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(ProviderDescriptor {
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
    });
    registry.register(ProviderDescriptor {
        id: "openai".to_string(),
        display_name: "OpenAI".to_string(),
        base_url: "https://api.openai.com".to_string(),
        default_api: "responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            provider: "openai".to_string(),
            api: "responses".to_string(),
            context_window: 128_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
    });
    registry
}

pub(super) fn sample_auth_store() -> AuthStore {
    let mut store = AuthStore::default();
    store.set_api_key("anthropic", "sk-ant");
    store.set_oauth(
        "openai",
        OAuthCredential {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at_ms: 42,
            account_id: Some("acct-1".to_string()),
            organization_id: None,
            email: Some("dev@example.com".to_string()),
            plan_type: Some("pro".to_string()),
            rate_limit_tier: None,
            scopes: vec!["openid".to_string()],
            organization_name: None,
            organization_role: None,
            workspace_role: None,
        },
    );
    store
}

pub(super) fn sample_commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            name: "review".to_string(),
            aliases: Vec::new(),
            description: "Review current changes".to_string(),
            argument_hint: None,
            kind: CommandKind::Prompt,
            hidden: false,
        },
        CommandSpec {
            name: "rewind".to_string(),
            aliases: vec!["checkpoint".to_string()],
            description: "Restore to a prior point".to_string(),
            argument_hint: None,
            kind: CommandKind::Ui,
            hidden: false,
        },
        CommandSpec {
            name: "usage".to_string(),
            aliases: Vec::new(),
            description: "Show usage limits".to_string(),
            argument_hint: None,
            kind: CommandKind::Local,
            hidden: false,
        },
    ]
}

fn tool_spec(
    id: &str,
    handler: &str,
    approval_policy: Option<&str>,
    sandbox_policy: Option<&str>,
) -> LoadedItem<ToolSpec> {
    LoadedItem {
        value: ToolSpec {
            id: id.to_string(),
            name: id.to_string(),
            description: format!("{id} tool"),
            handler: handler.to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            approval_policy: approval_policy.map(str::to_string),
            sandbox_policy: sandbox_policy.map(str::to_string),
            shared_lib: None,
            enabled_if: None,
            input_schema: None,
            metadata: Default::default(),
            display: Default::default(),
        },
        source_info: SourceInfo {
            path: PathBuf::from(format!("{id}.yaml")),
            kind: SourceKind::Builtin,
        },
    }
}

fn loaded_prompt(id: &str, description: &str) -> LoadedItem<puffer_resources::PromptTemplate> {
    LoadedItem {
        value: puffer_resources::PromptTemplate {
            id: id.to_string(),
            description: description.to_string(),
            template: "template".to_string(),
            variables: Vec::new(),
            allowed_tools: Vec::new(),
            provider_override: None,
            model_override: None,
            mode: None,
            chained_from: Vec::new(),
            for_provider: None,
            for_model: None,
        },
        source_info: SourceInfo {
            path: PathBuf::from(format!("{id}.yaml")),
            kind: SourceKind::Builtin,
        },
    }
}

fn loaded_skill(id: &str, description: &str) -> LoadedItem<puffer_resources::SkillSpec> {
    LoadedItem {
        value: puffer_resources::SkillSpec {
            name: id.to_string(),
            description: description.to_string(),
            content: "content".to_string(),
            disable_model_invocation: false,
            ..puffer_resources::SkillSpec::default()
        },
        source_info: SourceInfo {
            path: PathBuf::from(format!("{id}.yaml")),
            kind: SourceKind::Builtin,
        },
    }
}

fn loaded_plugin(id: &str, display_name: &str) -> LoadedItem<puffer_resources::PluginSpec> {
    LoadedItem {
        value: puffer_resources::PluginSpec {
            id: id.to_string(),
            display_name: display_name.to_string(),
            description: "plugin".to_string(),
            commands: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            mcp_servers: Vec::new(),
            lsp_servers: Vec::new(),
        },
        source_info: SourceInfo {
            path: PathBuf::from(format!("{id}.yaml")),
            kind: SourceKind::Builtin,
        },
    }
}

fn loaded_mcp(id: &str, display_name: &str) -> LoadedItem<puffer_resources::McpServerSpec> {
    LoadedItem {
        value: puffer_resources::McpServerSpec {
            id: id.to_string(),
            display_name: display_name.to_string(),
            transport: "stdio".to_string(),
            endpoint: String::new(),
            target: "cargo run".to_string(),
            description: "mcp".to_string(),
            headers: Default::default(),
            oauth: None,
        },
        source_info: SourceInfo {
            path: PathBuf::from(format!("{id}.yaml")),
            kind: SourceKind::Builtin,
        },
    }
}

fn loaded_ide(id: &str, display_name: &str) -> LoadedItem<puffer_resources::IdeSpec> {
    LoadedItem {
        value: puffer_resources::IdeSpec {
            id: id.to_string(),
            display_name: display_name.to_string(),
            description: "ide".to_string(),
        },
        source_info: SourceInfo {
            path: PathBuf::from(format!("{id}.yaml")),
            kind: SourceKind::Builtin,
        },
    }
}

pub(super) fn terminal_view(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    buffer
        .content
        .chunks(width)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol())
                .collect::<Vec<_>>()
                .join("")
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_post_send_terminal() -> String {
    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = sample_state();
    state.transcript.clear();
    state.push_message(
        MessageRole::User,
        "Review the current worktree and call out any risks.",
    );
    state.push_message(
        MessageRole::Assistant,
        "The working tree is clean.\n\nNo pending changes are waiting for review.",
    );
    let resources = sample_resources();
    let providers = sample_providers();
    let auth_store = sample_auth_store();

    terminal
        .draw(|frame| {
            render(
                frame,
                &state,
                &resources,
                &providers,
                &auth_store,
                "",
                0,
                0,
                0,
                &sample_commands(),
            )
        })
        .unwrap();

    terminal_view(&terminal)
}

fn post_send_comparison_report(claude: &str, puffer: &str) -> String {
    let comparisons: [(&str, fn(&str) -> bool); 5] = [
        ("top_panel", has_top_panel),
        ("boxed_footer", has_boxed_footer),
        ("user_prefix", has_user_prefix),
        ("response_connector", has_response_connector),
        ("shortcut_rail", has_shortcut_rail),
    ];
    let feature_lines = comparisons
        .into_iter()
        .map(|(label, detector)| {
            format!(
                "{label}: claude={} puffer={}",
                detector(claude),
                detector(puffer)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "Claude reference (derived from references/claude-code):\n{claude}\n\nPuffer current:\n{puffer}\n\nFeature comparison:\n{feature_lines}"
    )
}

fn has_top_panel(text: &str) -> bool {
    text.contains("╭ Puffer Code")
}

fn has_boxed_footer(text: &str) -> bool {
    text.lines()
        .rev()
        .take(4)
        .any(|line| line.contains("╭") || line.contains("╰"))
}

fn has_user_prefix(text: &str) -> bool {
    text.lines().any(|line| line.starts_with("› "))
}

fn has_response_connector(text: &str) -> bool {
    text.contains("⎿")
}

fn has_shortcut_rail(text: &str) -> bool {
    text.contains("? for shortcuts")
}

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(name)
}
