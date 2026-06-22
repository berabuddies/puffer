mod attachment_bridge;
mod auth_credentials;
mod auth_provider;
mod authflow;
mod basic_commands;
mod benchmark_reflection;
mod benchmark_run;
mod browser;
mod browser_args;
mod browser_output;
mod browser_profiles;
mod cli_args;
mod command_surface;
mod connect;
mod connectors;
mod daemon;
mod daemon_browser;
mod daemon_browser_settings;
mod daemon_contacts;
mod daemon_files;
mod daemon_fs_watch;
mod daemon_gcal_browser_setup;
mod daemon_gmail_browser_setup;
#[path = "daemon_lark_browser_setup.rs"]
mod daemon_lark_browser_setup;
mod daemon_lambda_skills;
mod daemon_local_model;
mod daemon_lsp;
mod daemon_pty;
mod daemon_secrets;
mod daemon_singleton;
mod daemon_telegram_ranking;
mod daemon_title;
mod daemon_turn_recovery;
mod daemon_turn_routing;
mod daemon_ui_state;
#[cfg(unix)]
mod daemon_wechat_browser_setup;
mod daemon_workflows;
mod desktop_activity;
mod desktop_api;
mod desktop_api_types;
mod gcal_browser;
mod gmail_browser;
mod gmail_browser_log;
mod heartbeat;
mod internal_tools;
mod lark_connector;
#[path = "lark_browser.rs"]
mod lark_browser;
#[path = "lark_browser_script.rs"]
mod lark_browser_script;
mod media_internal_tools;
mod non_interactive;
mod project_metadata;
mod resource_fs;
mod runner_selection;
mod subscriber_tool_args;
mod subscriber_tools;
mod subscriptions;
#[cfg(unix)]
mod wechat_connector;
mod workflow_runtime;
mod workflows;

use anyhow::{Context, Result};
use basic_commands::{
    provider_supports_auth_mode, run_agents_command, run_auto_mode_command, run_doctor_command,
    run_install_command, run_setup_token_command, run_update_command,
};
use benchmark_run::run_benchmark_command;
use clap::Parser;
use cli_args::{AuthCommand, Cli, Command, SessionCommand, ToolCommand};
use command_surface::{run_mcp_command, run_plugin_command};
use non_interactive::run_non_interactive_command;
use puffer_config::{ensure_workspace_dirs, load_config, ConfigPaths};
use puffer_core::{resolve_resume_launch, supported_commands, AppState, ResumeLaunchResolution};
use puffer_provider_openai::{
    exchange_authorization_code as exchange_openai_code,
    parse_authorization_input as parse_openai_authorization_input,
    refresh_oauth_token as refresh_openai_oauth_token,
};
use puffer_provider_registry::{
    canonical_provider_id, AuthMode, AuthStore, ProviderRegistry, StoredCredential,
};
use puffer_resources::load_resources;
use puffer_session_store::{SessionMetadata, SessionStore};
use puffer_tools::ToolRegistry;
use puffer_transport_anthropic::{
    build_messages_request, exchange_authorization_code as exchange_anthropic_code,
    parse_authorization_input as parse_anthropic_authorization_input,
    refresh_oauth_token as refresh_anthropic_oauth_token, AnthropicAuth, AnthropicMessage,
    AnthropicModelRequest, AnthropicRequestConfig, ANTHROPIC_API_BASE_URL,
    ANTHROPIC_MANUAL_REDIRECT_URL,
};
use puffer_tui::StartupAction;
use std::io::Read as _;
use std::io::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::auth_credentials::{
    anthropic_refresh_scopes, inferred_anthropic_redirect_uri,
    registry_to_anthropic_oauth_credential, set_stored_credential, store_anthropic_credential,
    store_ready_credential_from_anthropic, to_registry_oauth_credential_openai,
};
use crate::auth_provider::{
    oauth_family_for_provider, oauth_login_bundle_for_provider, oauth_start_bundle_for_provider,
    OauthFamily,
};

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Hidden subscriber subcommand dispatches to a baked-in skill driver
    // before any other Puffer state is loaded. The supervisor invokes
    // `puffer __subscriber <id>`; that process is dedicated to the
    // subscriber loop and has no business loading resources, providers,
    // or the subscription manager itself.
    if let Some(Command::Subscriber { id }) = &cli.subcommand {
        return run_subscriber(id);
    }

    // Hidden connector-protocol bridge. The subscription manager invokes
    // `puffer __connector <id> <op> ...` (per the connector template `command`
    // argv) to run `auth-ok`, `act`, or `subscribe` against an external tool.
    // Like `__subscriber`, this process is dedicated to the bridge and must not
    // load resources, providers, or the subscription manager.
    if let Some(Command::Connector { id, args }) = &cli.subcommand {
        return run_connector_bridge(id, args);
    }

    let cwd = std::env::current_dir()?;
    let paths = ConfigPaths::discover(&cwd);
    ensure_workspace_dirs(&paths)?;
    let config = load_config(&paths)?;
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::load(&auth_path)?;
    let mut resources = load_resources(&paths, &puffer_runner_local::LocalToolRunner::new())?;

    // Durable default logging: tracing events (connect/RPC/browser/errors)
    // roll into ~/.puffer/logs/puffer.log so failures are debuggable after the
    // fact. The telegram-user subscriber path returned earlier and keeps its
    // own per-account log. The guard must live for the whole process.
    let _log_guard = puffer_logging::init("puffer");

    // Observability: opt-in via OTEL_EXPORTER_OTLP_ENDPOINT (+ optional
    // LANGFUSE_PUBLIC_KEY/SECRET_KEY for Basic auth). When unset,
    // [`puffer_observability::ObservabilityHandle::try_init_from_env`]
    // returns `None` and the agent loop's span helpers short-circuit
    // to no-ops. See `docs/observability/langfuse-design.md`.
    match puffer_observability::ObservabilityHandle::try_init_from_env() {
        Ok(Some(handle)) => puffer_core::install_observability(handle),
        Ok(None) => {}
        Err(error) => {
            eprintln!("warning: observability disabled — {error:?}");
        }
    }

    let mut providers = ProviderRegistry::new();
    for provider in &resources.providers {
        providers.register_with_source(
            provider.value.clone().into_descriptor(),
            provider.source_info.as_provider_source(),
        );
    }
    providers.apply_openai_base_url_override(config.openai_base_url.as_deref());
    if let Some(display_name) = config.openai_display_name.as_deref() {
        providers.set_openai_display_name(display_name);
    }
    if !config.openai_headers.is_empty() {
        providers.set_openai_headers(
            config
                .openai_headers
                .clone()
                .into_iter()
                .collect::<indexmap::IndexMap<_, _>>(),
        );
    }
    if !config.openai_query_params.is_empty() {
        providers.set_openai_query_params(
            config
                .openai_query_params
                .clone()
                .into_iter()
                .collect::<indexmap::IndexMap<_, _>>(),
        );
    }
    // Discovery: synchronously apply whatever the cache already knows, then
    // probe stale providers on a detached thread so an unreachable endpoint
    // (e.g. a slow custom `openai_base_url`) cannot delay startup. Newly
    // discovered models become available on the next launch.
    let stale_provider_ids = providers.apply_discovery_cache();
    let _discovery_handle = if let Ok(client) = proxy_discovery_client(&config) {
        providers.start_background_discovery_with_discovery_client(
            stale_provider_ids,
            &auth_store,
            client,
        )
    } else {
        providers.start_background_discovery(stale_provider_ids, &auth_store)
    };

    // Boot background automation only for long-lived processes. Short CLI
    // commands like `workflow register` and `workflow ls` must not fire cron
    // triggers as a side effect of inspecting state.
    let anthropic_base = providers.provider("anthropic").map(|p| p.base_url.clone());
    let start_background_runtimes = should_start_background_runtimes(&cli.subcommand);
    // Non-interactive turns still need the subscription manager so agent
    // tools (Telegram, Email, …) can dispatch through it, but they MUST
    // NOT spin up the cron / heartbeat threads — those are intended for
    // long-lived processes only.
    let install_subscription_manager =
        start_background_runtimes || matches!(cli.subcommand, Some(Command::NonInteractive(_)));
    let mut daemon_host_guard = if matches!(cli.subcommand, Some(Command::Daemon { .. })) {
        Some(daemon_singleton::acquire(&paths)?)
    } else {
        None
    };
    let _heartbeat = if start_background_runtimes {
        match heartbeat::start_from_env() {
            Ok(handle) => handle,
            Err(error) => {
                eprintln!("heartbeat disabled: {error:#}");
                None
            }
        }
    } else {
        None
    };
    let _workflow_runtime = if start_background_runtimes {
        match workflow_runtime::install(&paths, &config, &resources, &providers, &auth_store) {
            Ok(rt) => Some(rt),
            Err(error) => {
                eprintln!("workflow runtime failed to start: {error:#}");
                None
            }
        }
    } else {
        None
    };
    let _subscription_runtime = if install_subscription_manager {
        match subscriptions::install(&paths, &auth_store, anthropic_base.as_deref()) {
            Ok(rt) => Some(rt),
            Err(error) => {
                eprintln!("subscription manager failed to start: {error:#}");
                None
            }
        }
    } else {
        None
    };

    let result = match cli.subcommand {
        Some(Command::Subscriber { .. }) => {
            // Already handled above; here only to satisfy exhaustiveness.
            unreachable!("__subscriber dispatched before main match")
        }
        Some(Command::Connector { .. }) => {
            // Already handled above; here only to satisfy exhaustiveness.
            unreachable!("__connector dispatched before main match")
        }
        Some(Command::Agents { setting_sources }) => {
            run_agents_command(&paths, &config, setting_sources.as_deref())
        }
        Some(Command::Auth { command }) => run_auth_command(
            command,
            &config,
            config.default_provider.as_deref(),
            &mut auth_store,
            &auth_path,
            &providers,
        ),
        Some(Command::AutoMode { command }) => run_auto_mode_command(command, &paths),
        Some(Command::Doctor) => {
            run_doctor_command(&config, &resources, &providers, &auth_store, &cwd)
        }
        Some(Command::Install { target, force }) => {
            run_install_command(target.as_deref(), force, &cwd)
        }
        Some(Command::Mcp { command }) => {
            run_mcp_command(command, &paths, &config, &resources, &cwd)
        }
        Some(Command::Plugin { command }) => run_plugin_command(command, &paths, &resources),
        Some(Command::Workflow { command }) => workflows::run_workflow_command(command, &paths),
        Some(Command::SetupToken {
            provider,
            token,
            stdin,
        }) => {
            let provider =
                resolve_provider_arg(&providers, provider, config.default_provider.as_deref());
            let token = if stdin {
                read_secret_from_stdin()?
            } else if let Some(token) = token {
                token
            } else {
                read_line_with_prompt("Paste the API token: ")?
            };
            run_setup_token_command(&provider, token, &mut auth_store, &auth_path, &providers)
        }
        Some(Command::Update) => run_update_command(&cwd),
        Some(Command::Commands) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "builtin": supported_commands(),
                    "skills": resources
                        .skills
                        .iter()
                        .map(|skill| serde_json::json!({
                            "name": skill.value.name,
                            "description": skill.value.description,
                            "command": format!("/skill:{}", skill.value.name),
                        }))
                        .collect::<Vec<_>>(),
                }))?
            );
            Ok(())
        }
        Some(Command::Providers) => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &providers
                        .provider_entries()
                        .map(|entry| {
                            serde_json::json!({
                                "provider": entry.descriptor,
                                "source": entry.source,
                            })
                        })
                        .collect::<Vec<_>>(),
                )?
            );
            Ok(())
        }
        Some(Command::Sessions { command }) => run_session_command(command, &paths),
        Some(Command::Resources) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "prompts": resources.prompts,
                    "tools": resources.tools,
                    "internal_tools": resources.internal_tools,
                    "skills": resources.skills,
                    "plugins": resources.plugins,
                    "mcp_servers": resources.mcp_servers,
                    "ides": resources.ides,
                    "mascots": resources.mascots,
                }))?
            );
            Ok(())
        }
        Some(Command::DesktopApi { command }) => desktop_api::run_desktop_api(
            command,
            &paths,
            &config,
            &resources,
            &providers,
            &mut auth_store,
        ),
        Some(Command::Daemon {
            bind,
            handshake_file,
            token,
            print_handshake,
            no_browser,
            system_prompt_1,
            disable_auto_title,
            yolo,
        }) => daemon::run(daemon::DaemonOptions {
            host_guard: daemon_host_guard
                .take()
                .expect("daemon host guard acquired before daemon startup"),
            bind,
            handshake_file,
            token,
            print_handshake,
            no_browser,
            system_prompt_1,
            disable_auto_title,
            yolo,
        }),
        Some(Command::Browser(args)) => browser::run_browser_command(&cwd, &paths, args),
        Some(Command::Connect { command }) => connect::run_connect_command(&paths, command),
        Some(Command::InternalTool { command }) => {
            internal_tools::run_internal_tool_command(&cwd, &paths, command)
        }
        Some(Command::BenchmarkRun {
            prompt,
            prompt_file,
            stdin,
            provider,
            model,
            effort,
            fast,
            result_json,
            trajectory_json,
            deny_tools,
        }) => run_benchmark_command(
            &cwd,
            &config,
            &resources,
            &mut providers,
            &mut auth_store,
            benchmark_run::BenchmarkRunArgs {
                prompt,
                prompt_file,
                stdin,
                provider,
                model,
                effort,
                fast,
                result_json,
                trajectory_json,
                deny_tools,
            },
        ),
        Some(Command::NonInteractive(args)) => run_non_interactive_command(
            &cwd,
            &config,
            &resources,
            &mut providers,
            &mut auth_store,
            &paths,
            args,
        ),
        Some(Command::AnthropicRequestFixture) => {
            let request = build_messages_request(
                &AnthropicRequestConfig {
                    base_url: providers
                        .provider("anthropic")
                        .map(|provider| provider.base_url.clone())
                        .unwrap_or_else(|| "https://api.anthropic.com".to_string()),
                    session_id: "fixture-session".to_string(),
                    custom_headers: indexmap::IndexMap::new(),
                    remote_container_id: None,
                    remote_session_id: None,
                    client_app: None,
                    entrypoint: "cli".to_string(),
                    user_type: "external".to_string(),
                    version: "0.1.0".to_string(),
                    workload: None,
                    additional_protection: false,
                    cch_enabled: true,
                    auth: AnthropicAuth::ApiKey("test-key".to_string()),
                    beta_header: None,
                    client_request_id: Some("fixture-request".to_string()),
                },
                &AnthropicModelRequest {
                    model: config
                        .default_model
                        .clone()
                        .unwrap_or_else(|| "claude-sonnet-4-5".to_string()),
                    max_tokens: 1024,
                    messages: vec![AnthropicMessage {
                        role: "user".to_string(),
                        content: "hello".to_string(),
                    }],
                },
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "method": request.method,
                    "url": request.url,
                    "headers": request.headers,
                    "attribution_prefix_block": request.attribution_prefix_block,
                    "body": serde_json::from_str::<serde_json::Value>(&request.body)?,
                }))?
            );
            Ok(())
        }
        Some(Command::Serve {
            config: config_override,
        }) => run_serve_command(
            config_override.as_deref(),
            &cwd,
            config,
            resources,
            providers,
            auth_store,
            auth_path,
            &paths,
        ),
        Some(Command::Tool { command }) => run_tool_command(command, &resources, &cwd),
        Some(Command::Remote {
            target,
            cwd,
            no_alt_screen,
            prompt,
        }) => run_remote_tui(
            &target,
            cwd.as_deref(),
            (!prompt.is_empty()).then(|| prompt.join(" ")),
            cli.no_alt_screen || no_alt_screen || config.ui.no_alt_screen,
        ),
        Some(Command::Resume { session_id }) => run_existing_session_tui(
            resolve_session_query(&SessionStore::from_paths(&paths)?, &session_id)?,
            cli.prompt,
            &cwd,
            &config,
            &mut resources,
            &mut providers,
            &mut auth_store,
            &auth_path,
            &paths,
            cli.no_alt_screen || config.ui.no_alt_screen,
        ),
        Some(Command::Fork { session_id }) => {
            let session_store = SessionStore::from_paths(&paths)?;
            let source = resolve_session_query(&session_store, &session_id)?;
            let session = session_store.fork_session(source, std::env::current_dir()?)?;
            let record = session_store.load_session(session.id)?;
            let mut state = AppState::from_session_record(config.clone(), record);
            puffer_tui::run_app(
                &mut state,
                &mut resources,
                &mut providers,
                &mut auth_store,
                &auth_path,
                &session_store,
                StartupAction::None,
                None,
                cli.no_alt_screen || config.ui.no_alt_screen,
            )
        }
        None => {
            let session_store = SessionStore::from_paths(&paths)?;
            match resolve_resume_launch(&session_store, &cwd, cli.resume.as_deref())? {
                ResumeLaunchResolution::Exact(session) => run_existing_session_tui(
                    session.id,
                    cli.prompt,
                    &cwd,
                    &config,
                    &mut resources,
                    &mut providers,
                    &mut auth_store,
                    &auth_path,
                    &paths,
                    cli.no_alt_screen || config.ui.no_alt_screen,
                ),
                ResumeLaunchResolution::Picker { query, .. } if cli.resume.is_some() => {
                    let runner = crate::runner_selection::select_tool_runner(
                        &config,
                        &resources,
                        cwd.clone(),
                    );
                    let mut state = AppState::new(
                        config.clone(),
                        cwd.clone(),
                        transient_resume_picker_session(cwd),
                    )
                    .with_tool_runner(runner);
                    if let Some(prompt) = cli.prompt {
                        state.queue_pending_query_prompt(prompt);
                    }
                    puffer_tui::run_app(
                        &mut state,
                        &mut resources,
                        &mut providers,
                        &mut auth_store,
                        &auth_path,
                        &session_store,
                        StartupAction::ResumePicker { query },
                        None,
                        cli.no_alt_screen || config.ui.no_alt_screen,
                    )
                }
                ResumeLaunchResolution::NotFound { query } if cli.resume.is_some() => {
                    if let Some(query) = query {
                        anyhow::bail!(
                            "No conversations matched `{query}`.\nRun `puffer --resume` to pick from recent sessions."
                        );
                    }
                    anyhow::bail!("No conversations found to resume.");
                }
                _ => {
                    let session = session_store.create_session(cwd.clone())?;
                    let runner = crate::runner_selection::select_tool_runner(
                        &config,
                        &resources,
                        cwd.clone(),
                    );
                    let mut state =
                        AppState::new(config.clone(), cwd, session).with_tool_runner(runner);
                    puffer_tui::run_app(
                        &mut state,
                        &mut resources,
                        &mut providers,
                        &mut auth_store,
                        &auth_path,
                        &session_store,
                        StartupAction::None,
                        cli.prompt,
                        cli.no_alt_screen || config.ui.no_alt_screen,
                    )
                }
            }
        }
    };

    // Force-flush observability spans (and other runtime services) before
    // process exit. Without this, short-running commands like
    // `benchmark-run` can race the OTLP exporter and lose the last few
    // spans in transit.
    let _ = puffer_core::shutdown_runtime_services();
    result
}

fn should_start_background_runtimes(subcommand: &Option<Command>) -> bool {
    matches!(
        subcommand,
        None | Some(Command::Resume { .. })
            | Some(Command::Fork { .. })
            | Some(Command::Daemon { .. })
            | Some(Command::Serve { .. })
    )
}

fn run_existing_session_tui(
    session_id: Uuid,
    initial_prompt: Option<String>,
    cwd: &std::path::Path,
    config: &puffer_config::PufferConfig,
    resources: &mut puffer_resources::LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    auth_path: &std::path::Path,
    paths: &ConfigPaths,
    no_alt_screen: bool,
) -> Result<()> {
    let session_store = SessionStore::from_paths(paths)?;
    let session = session_store.load_session(session_id)?;
    let mut state = AppState::from_session_record(config.clone(), session);
    if state.cwd.as_os_str().is_empty() {
        state.cwd = cwd.to_path_buf();
        state.session.cwd = cwd.to_path_buf();
    }
    let runner = crate::runner_selection::select_tool_runner(config, resources, state.cwd.clone());
    state = state.with_tool_runner(runner);
    puffer_tui::run_app(
        &mut state,
        resources,
        providers,
        auth_store,
        auth_path,
        &session_store,
        StartupAction::None,
        initial_prompt,
        no_alt_screen,
    )
}

fn transient_resume_picker_session(cwd: std::path::PathBuf) -> SessionMetadata {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    SessionMetadata {
        id: Uuid::nil(),
        display_name: Some("Resume picker".to_string()),
        generated_title: None,
        cwd,
        created_at_ms: now,
        updated_at_ms: now,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    }
}

fn run_tool_command(
    command: ToolCommand,
    resources: &puffer_resources::LoadedResources,
    cwd: &std::path::Path,
) -> Result<()> {
    match command {
        ToolCommand::List => {
            let registry = ToolRegistry::from_resources(resources);
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &registry
                        .tools()
                        .map(|tool| {
                            serde_json::json!({
                                "id": tool.spec.id,
                                "name": tool.spec.name,
                                "description": tool.spec.description,
                                "handler": tool.spec.handler,
                                "handler_args": tool.spec.handler_args,
                                "approval_policy": tool.spec.policy.approval_policy,
                                "sandbox_policy": tool.spec.policy.sandbox_policy,
                                "shared_lib": tool.spec.shared_lib,
                                "enabled_if": tool.spec.enabled_if,
                                "display": tool.spec.display,
                                "input_schema": tool.spec.input_schema.as_json_schema(),
                            })
                        })
                        .collect::<Vec<_>>(),
                )?
            );
        }
        ToolCommand::Describe { tool_id } => {
            let registry = ToolRegistry::from_resources(resources);
            let tool = registry
                .tool(&tool_id)
                .ok_or_else(|| anyhow::anyhow!("unknown tool {tool_id}"))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "id": tool.spec.id,
                    "name": tool.spec.name,
                    "description": tool.spec.description,
                    "handler": tool.spec.handler,
                    "handler_args": tool.spec.handler_args,
                    "approval_policy": tool.spec.policy.approval_policy,
                    "sandbox_policy": tool.spec.policy.sandbox_policy,
                    "shared_lib": tool.spec.shared_lib,
                    "enabled_if": tool.spec.enabled_if,
                    "display": tool.spec.display,
                    "input_schema": tool.spec.input_schema.as_json_schema(),
                }))?
            );
        }
        ToolCommand::Run { tool_id, args } => {
            let registry = ToolRegistry::from_resources(resources);
            let payload: serde_json::Value = serde_json::from_str(&args)?;
            let result = registry.execute_json(&tool_id, cwd, payload)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ToolCommand::Bash { command } => {
            let registry = ToolRegistry::from_resources(resources);
            let result = registry.execute(
                "bash",
                cwd,
                puffer_tools::ToolInput::Bash {
                    command,
                    timeout: None,
                    run_in_background: false,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ToolCommand::Read { path } => {
            let registry = ToolRegistry::from_resources(resources);
            let result = registry.execute(
                "read_file",
                cwd,
                puffer_tools::ToolInput::ReadFile {
                    path: path.into(),
                    offset: None,
                    limit: None,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ToolCommand::Write { path, contents } => {
            let registry = ToolRegistry::from_resources(resources);
            let result = registry.execute(
                "write_file",
                cwd,
                puffer_tools::ToolInput::WriteFile {
                    path: path.into(),
                    contents,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ToolCommand::Replace {
            path,
            old,
            new,
            replace_all,
        } => {
            let registry = ToolRegistry::from_resources(resources);
            let result = registry.execute(
                "replace_in_file",
                cwd,
                puffer_tools::ToolInput::ReplaceInFile {
                    path: path.into(),
                    old,
                    new,
                    replace_all,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ToolCommand::Move { from, to } => {
            let registry = ToolRegistry::from_resources(resources);
            let result = registry.execute(
                "move_path",
                cwd,
                puffer_tools::ToolInput::MovePath {
                    from: from.into(),
                    to: to.into(),
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ToolCommand::Remove { path, recursive } => {
            let registry = ToolRegistry::from_resources(resources);
            let result = registry.execute(
                "remove_path",
                cwd,
                puffer_tools::ToolInput::RemovePath {
                    path: path.into(),
                    recursive,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ToolCommand::ListDir { path } => {
            let registry = ToolRegistry::from_resources(resources);
            let result = registry.execute(
                "list_dir",
                cwd,
                puffer_tools::ToolInput::ListDir {
                    path: path.map(Into::into),
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ToolCommand::SearchText { query, path } => {
            let registry = ToolRegistry::from_resources(resources);
            let result = registry.execute(
                "search_text",
                cwd,
                puffer_tools::ToolInput::SearchText {
                    query,
                    path: path.map(Into::into),
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(())
}

fn run_session_command(command: Option<SessionCommand>, paths: &ConfigPaths) -> Result<()> {
    let session_store = SessionStore::from_paths(paths)?;
    match command.unwrap_or(SessionCommand::List) {
        SessionCommand::List => {
            println!(
                "{}",
                serde_json::to_string_pretty(&session_store.list_sessions()?)?
            );
        }
        SessionCommand::Note {
            session_id,
            note,
            clear,
        } => {
            let id = session_id.parse()?;
            if clear {
                session_store.set_note(id, None)?;
                println!("cleared note for session {session_id}");
            } else {
                let note = note.ok_or_else(|| {
                    anyhow::anyhow!("note text is required unless --clear is used")
                })?;
                session_store.set_note(id, Some(note))?;
                println!("updated note for session {session_id}");
            }
        }
        SessionCommand::TagAdd { session_id, tag } => {
            session_store.add_tag(session_id.parse()?, tag.clone())?;
            println!("added tag `{tag}` to session {session_id}");
        }
        SessionCommand::TagRemove { session_id, tag } => {
            session_store.remove_tag(session_id.parse()?, &tag)?;
            println!("removed tag `{tag}` from session {session_id}");
        }
    }
    Ok(())
}

fn run_auth_command(
    command: AuthCommand,
    config: &puffer_config::PufferConfig,
    default_provider: Option<&str>,
    auth_store: &mut AuthStore,
    auth_path: &std::path::Path,
    providers: &ProviderRegistry,
) -> Result<()> {
    match command {
        AuthCommand::Status { json, text } => {
            if json && text {
                anyhow::bail!("choose either --json or --text");
            }
            let credentials = auth_store
                .provider_ids()
                .map(|provider_id| {
                    let kind = match auth_store.get(provider_id) {
                        Some(StoredCredential::ApiKey { .. }) => "api_key",
                        Some(StoredCredential::OAuth(_)) => "oauth",
                        None => "unknown",
                    };
                    serde_json::json!({
                        "provider": provider_id,
                        "kind": kind,
                    })
                })
                .collect::<Vec<_>>();
            if text {
                if credentials.is_empty() {
                    println!("No providers are authenticated.");
                } else {
                    println!("Authenticated providers:");
                    for credential in credentials {
                        println!(
                            "- {} ({})",
                            credential["provider"].as_str().unwrap_or("<unknown>"),
                            credential["kind"].as_str().unwrap_or("<unknown>")
                        );
                    }
                }
                return Ok(());
            }
            println!("{}", serde_json::to_string_pretty(&credentials)?);
        }
        AuthCommand::Login {
            provider,
            value,
            stdin,
        } => {
            let provider = resolve_provider_arg(providers, provider, default_provider);
            if !provider_supports_auth_mode(&provider, providers, &AuthMode::OAuth) {
                anyhow::bail!(
                    "provider `{provider}` does not support OAuth; use `puffer setup-token {provider}` or `puffer auth set-api-key {provider}`"
                );
            }
            run_login_flow(
                &provider, value, stdin, auth_store, auth_path, providers, config,
            )?;
        }
        AuthCommand::Logout { provider } => {
            let provider = resolve_provider_arg(providers, provider, default_provider);
            let removed = auth_store.remove(&provider);
            auth_store.save(auth_path)?;
            if removed.is_some() {
                println!("cleared credentials for {provider}");
            } else {
                println!("no credentials were stored for {provider}");
            }
        }
        AuthCommand::SetApiKey {
            provider,
            api_key,
            stdin,
        } => {
            let provider = resolve_provider_id(providers, &provider);
            let key = if stdin {
                read_secret_from_stdin()?
            } else {
                api_key.ok_or_else(|| {
                    anyhow::anyhow!("api key value is required unless --stdin is used")
                })?
            };
            auth_store.set_api_key(provider.clone(), key);
            auth_store.save(auth_path)?;
            println!("stored api key for {provider}");
        }
        AuthCommand::Clear { provider } => {
            let provider = resolve_provider_id(providers, &provider);
            let removed = auth_store.remove(&provider);
            auth_store.save(auth_path)?;
            if removed.is_some() {
                println!("cleared credentials for {provider}");
            } else {
                println!("no credentials were stored for {provider}");
            }
        }
        AuthCommand::OauthUrl { provider } => {
            let provider = resolve_provider_id(providers, &provider);
            let bundle = oauth_start_bundle_for_provider(providers, &provider)?;
            println!("{}", bundle.authorization_url);
        }
        AuthCommand::OauthStart { provider } => {
            let provider = resolve_provider_id(providers, &provider);
            let bundle = oauth_start_bundle_for_provider(providers, &provider)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "provider": provider,
                    "authorization_url": bundle.authorization_url,
                    "code_verifier": bundle.verifier,
                    "state": bundle.state,
                    "redirect_uri": bundle.redirect_uri,
                    "manual_redirect_uri": bundle.manual_redirect_uri,
                    "automatic_authorization_url": bundle.automatic_authorization_url,
                }))?
            );
        }
        AuthCommand::OauthExchange {
            provider,
            value,
            verifier,
            state,
            stdin,
        } => {
            let provider = resolve_provider_id(providers, &provider);
            let input = if stdin {
                read_secret_from_stdin()?
            } else {
                value.ok_or_else(|| {
                    anyhow::anyhow!(
                        "authorization code or callback URL is required unless --stdin is used"
                    )
                })?
            };
            match oauth_family_for_provider(providers, &provider) {
                Some(OauthFamily::OpenAi) => {
                    let (code, parsed_state) = parse_openai_authorization_input(&input);
                    let code = code.ok_or_else(|| {
                        anyhow::anyhow!("could not extract an OpenAI authorization code")
                    })?;
                    if let Some(expected_state) = state.as_deref() {
                        if parsed_state.as_deref().unwrap_or_default() != expected_state {
                            anyhow::bail!("oauth state mismatch for openai");
                        }
                    }
                    let credential = match proxy_oauth_client(
                        config,
                        puffer_provider_openai::OPENAI_TOKEN_URL,
                    ) {
                        Ok(client) => {
                            puffer_provider_openai::exchange_authorization_code_with_client(
                                &client, &code, &verifier, None,
                            )?
                        }
                        Err(_) => exchange_openai_code(&code, &verifier, None)?,
                    };
                    auth_store.set_oauth(
                        provider.clone(),
                        to_registry_oauth_credential_openai(credential),
                    );
                }
                Some(OauthFamily::Anthropic) => {
                    let (code, parsed_state) = parse_anthropic_authorization_input(&input);
                    let code = code.ok_or_else(|| {
                        anyhow::anyhow!("could not extract an Anthropic authorization code")
                    })?;
                    let expected_state = state.ok_or_else(|| {
                        anyhow::anyhow!("--state is required for anthropic oauth-exchange")
                    })?;
                    if parsed_state.as_deref().unwrap_or_default() != expected_state {
                        anyhow::bail!("oauth state mismatch for anthropic");
                    }
                    let redirect_uri = inferred_anthropic_redirect_uri(&input)
                        .unwrap_or_else(|| ANTHROPIC_MANUAL_REDIRECT_URL.to_string());
                    let credential = match proxy_oauth_client(config, ANTHROPIC_API_BASE_URL) {
                        Ok(client) => {
                            puffer_transport_anthropic::exchange_authorization_code_with_client(
                                &client,
                                &code,
                                &verifier,
                                &expected_state,
                                Some(&redirect_uri),
                                Some(ANTHROPIC_API_BASE_URL),
                            )?
                        }
                        Err(_) => exchange_anthropic_code(
                            &code,
                            &verifier,
                            &expected_state,
                            Some(&redirect_uri),
                            Some(ANTHROPIC_API_BASE_URL),
                        )?,
                    };
                    store_anthropic_credential(auth_store, &provider, credential)?;
                }
                None => anyhow::bail!("oauth exchange is not implemented for {provider}"),
            }
            auth_store.save(auth_path)?;
            println!("stored oauth credentials for {provider}");
        }
        AuthCommand::OauthRefresh { provider } => {
            let provider = resolve_provider_id(providers, &provider);
            let credential = auth_store
                .get(&provider)
                .ok_or_else(|| anyhow::anyhow!("no credentials stored for {provider}"))?;
            let StoredCredential::OAuth(existing) = credential else {
                anyhow::bail!("stored credential for {provider} is not oauth");
            };
            let refreshed = match oauth_family_for_provider(providers, &provider) {
                Some(OauthFamily::OpenAi) => {
                    let credential = match proxy_oauth_client(
                        config,
                        puffer_provider_openai::OPENAI_TOKEN_URL,
                    ) {
                        Ok(client) => puffer_provider_openai::refresh_oauth_token_with_client(
                            &client,
                            &existing.refresh_token,
                        )?,
                        Err(_) => refresh_openai_oauth_token(&existing.refresh_token)?,
                    };
                    StoredCredential::OAuth(to_registry_oauth_credential_openai(credential))
                }
                Some(OauthFamily::Anthropic) => {
                    let credential = match proxy_oauth_client(config, ANTHROPIC_API_BASE_URL) {
                        Ok(client) => puffer_transport_anthropic::refresh_oauth_token_with_client(
                            &client,
                            &existing.refresh_token,
                            anthropic_refresh_scopes(existing).as_deref(),
                            Some(&registry_to_anthropic_oauth_credential(existing)),
                        )?,
                        Err(_) => refresh_anthropic_oauth_token(
                            &existing.refresh_token,
                            anthropic_refresh_scopes(existing).as_deref(),
                            Some(ANTHROPIC_API_BASE_URL),
                            Some(&registry_to_anthropic_oauth_credential(existing)),
                        )?,
                    };
                    store_ready_credential_from_anthropic(credential)?
                }
                None => anyhow::bail!("oauth refresh is not implemented for {provider}"),
            };
            set_stored_credential(auth_store, provider.clone(), refreshed);
            auth_store.save(auth_path)?;
            println!("refreshed oauth credentials for {provider}");
        }
    }
    Ok(())
}

fn resolve_provider_arg(
    providers: &ProviderRegistry,
    provider: Option<String>,
    default_provider: Option<&str>,
) -> String {
    let provider = provider
        .or_else(|| default_provider.map(ToOwned::to_owned))
        .unwrap_or_else(|| "anthropic".to_string());
    resolve_provider_id(providers, &provider)
}

fn resolve_provider_id(providers: &ProviderRegistry, provider: &str) -> String {
    providers
        .provider(provider)
        .map(|descriptor| descriptor.id.clone())
        .unwrap_or_else(|| canonical_provider_id(provider))
}

fn proxy_discovery_client(
    config: &puffer_config::PufferConfig,
) -> Result<puffer_provider_registry::ModelDiscoveryClient> {
    let client = puffer_core::blocking_client_for_url(
        &config.network.proxy,
        puffer_core::HttpPurpose::Discovery,
        "https://api.openai.com/v1/models",
        Duration::from_secs(8),
    )?;
    Ok(puffer_provider_registry::ModelDiscoveryClient::with_client(
        client,
    ))
}

fn proxy_oauth_client(
    config: &puffer_config::PufferConfig,
    url: &str,
) -> Result<reqwest::blocking::Client> {
    puffer_core::blocking_client_for_url(
        &config.network.proxy,
        puffer_core::HttpPurpose::OAuth,
        url,
        Duration::from_secs(60),
    )
}

fn run_login_flow(
    provider: &str,
    value: Option<String>,
    stdin: bool,
    auth_store: &mut AuthStore,
    auth_path: &std::path::Path,
    providers: &ProviderRegistry,
    config: &puffer_config::PufferConfig,
) -> Result<()> {
    let callback_listener = if stdin || value.is_some() {
        None
    } else {
        Some(authflow::CallbackListener::bind_localhost("/callback")?)
    };
    let bundle = if let Some(listener) = callback_listener.as_ref() {
        oauth_login_bundle_for_provider(providers, provider, listener.redirect_uri())?
    } else {
        oauth_start_bundle_for_provider(providers, provider)?
    };

    println!(
        "Open this URL in your browser:\n\n{}\n",
        bundle.authorization_url
    );
    if let Some(automatic_url) = bundle.automatic_authorization_url.as_deref() {
        if authflow::open_browser(automatic_url) {
            println!("Opened the automatic localhost callback flow in your browser.");
            println!("If it did not open correctly, use the URL above instead.\n");
        }
    }
    let input = if stdin {
        read_secret_from_stdin()?
    } else if let Some(value) = value {
        value
    } else if let Some(listener) = callback_listener.as_ref() {
        if let Some(callback) = listener.wait_for_callback_url(Duration::from_secs(120))? {
            callback
        } else {
            read_line_with_prompt("Paste the callback URL or authorization code: ")?
        }
    } else {
        read_line_with_prompt("Paste the callback URL or authorization code: ")?
    };

    match oauth_family_for_provider(providers, provider) {
        Some(OauthFamily::OpenAi) => {
            let (code, parsed_state) = parse_openai_authorization_input(&input);
            let code = code
                .ok_or_else(|| anyhow::anyhow!("could not extract an OpenAI authorization code"))?;
            if let Some(parsed_state) = parsed_state {
                if parsed_state != bundle.state {
                    anyhow::bail!("oauth state mismatch for openai");
                }
            }
            let credential =
                match proxy_oauth_client(config, puffer_provider_openai::OPENAI_TOKEN_URL) {
                    Ok(client) => puffer_provider_openai::exchange_authorization_code_with_client(
                        &client,
                        &code,
                        &bundle.verifier,
                        None,
                    )?,
                    Err(_) => exchange_openai_code(&code, &bundle.verifier, None)?,
                };
            auth_store.set_oauth(
                provider.to_string(),
                to_registry_oauth_credential_openai(credential),
            );
        }
        Some(OauthFamily::Anthropic) => {
            let (code, parsed_state) = parse_anthropic_authorization_input(&input);
            let code = code.ok_or_else(|| {
                anyhow::anyhow!("could not extract an Anthropic authorization code")
            })?;
            let parsed_state = parsed_state.unwrap_or_else(|| bundle.state.clone());
            if parsed_state != bundle.state {
                anyhow::bail!("oauth state mismatch for anthropic");
            }
            let redirect_uri = inferred_anthropic_redirect_uri(&input)
                .or_else(|| bundle.manual_redirect_uri.clone())
                .unwrap_or_else(|| ANTHROPIC_MANUAL_REDIRECT_URL.to_string());
            let credential = match proxy_oauth_client(config, ANTHROPIC_API_BASE_URL) {
                Ok(client) => puffer_transport_anthropic::exchange_authorization_code_with_client(
                    &client,
                    &code,
                    &bundle.verifier,
                    &bundle.state,
                    Some(&redirect_uri),
                    Some(ANTHROPIC_API_BASE_URL),
                )?,
                Err(_) => exchange_anthropic_code(
                    &code,
                    &bundle.verifier,
                    &bundle.state,
                    Some(&redirect_uri),
                    Some(ANTHROPIC_API_BASE_URL),
                )?,
            };
            store_anthropic_credential(auth_store, provider, credential)?;
        }
        None => anyhow::bail!("oauth login is not implemented for {provider}"),
    }

    auth_store.save(auth_path)?;
    println!("stored oauth credentials for {provider}");
    Ok(())
}

fn read_secret_from_stdin() -> Result<String> {
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer)?;
    let trimmed = buffer.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("stdin did not contain a credential value");
    }
    Ok(trimmed)
}

fn read_line_with_prompt(prompt: &str) -> Result<String> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let trimmed = line.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("no input received");
    }
    Ok(trimmed)
}

fn run_remote_tui(
    target: &str,
    remote_cwd: Option<&str>,
    prompt: Option<String>,
    no_alt_screen: bool,
) -> Result<()> {
    let mut remote_command = String::new();
    if let Some(remote_cwd) = remote_cwd.filter(|value| !value.trim().is_empty()) {
        remote_command.push_str("cd ");
        remote_command.push_str(&shell_quote(remote_cwd));
        remote_command.push_str(" && ");
    }
    let mut invocation = String::new();
    if no_alt_screen {
        invocation.push_str(" --no-alt-screen");
    }
    if let Some(prompt) = prompt.as_deref().filter(|value| !value.trim().is_empty()) {
        invocation.push(' ');
        invocation.push_str(&shell_quote(prompt));
    }
    remote_command.push_str("if command -v puffer >/dev/null 2>&1; then exec puffer");
    remote_command.push_str(&invocation);
    remote_command.push_str(
        " ; elif [ -x \"$HOME/.cargo/bin/puffer\" ]; then exec \"$HOME/.cargo/bin/puffer\"",
    );
    remote_command.push_str(&invocation);
    remote_command
        .push_str(" ; elif [ -x \"./target/debug/puffer\" ]; then exec \"./target/debug/puffer\"");
    remote_command.push_str(&invocation);
    remote_command.push_str(
        " ; elif [ -x \"$HOME/.cargo/bin/cargo\" ]; then exec \"$HOME/.cargo/bin/cargo\" run -q -p puffer-cli --",
    );
    remote_command.push_str(&invocation);
    remote_command.push_str(
        " ; elif command -v cargo >/dev/null 2>&1; then exec cargo run -q -p puffer-cli --",
    );
    remote_command.push_str(&invocation);
    remote_command.push_str(" ; else echo 'remote puffer command not found' >&2; exit 127; fi");

    let status = std::process::Command::new("ssh")
        .args([
            "-tt",
            "-o",
            "BatchMode=no",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "ConnectTimeout=15",
            target,
            &remote_command,
        ])
        .status()?;
    if !status.success() {
        anyhow::bail!("remote ssh session exited with {}", status);
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

/// Dispatcher for the hidden `__subscriber <id>` subcommand. Each
/// supported subscriber id maps to one baked-in driver crate. Unknown
/// ids exit with a clear error so the supervisor's restart loop does not
/// silently spin forever.
fn run_subscriber(id: &str) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name(format!("puffer-subscriber-{id}"))
        .build()
        .context("failed to build subscriber tokio runtime")?;
    runtime.block_on(async move {
        match id {
            "telegram-user" => puffer_subscriber_telegram_user::run().await,
            "email" => puffer_subscriber_email::run().await,
            "gcal-browser" => crate::gcal_browser::run_subscriber().await,
            "gmail-browser" => crate::gmail_browser::run_subscriber().await,
            "lark-browser" => crate::lark_browser::run_subscriber().await,
            other => Err(anyhow::anyhow!(
                "unknown subscriber id `{other}`; this puffer build does not bundle a driver for it"
            )),
        }
    })
}

/// Dispatcher for the hidden `__connector <id> <op> ...` bridge subcommand.
/// Each supported connector id maps to one baked-in protocol bridge. Unknown
/// ids exit with a clear error.
fn run_connector_bridge(id: &str, args: &[String]) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name(format!("puffer-connector-{id}"))
        .build()
        .context("failed to build connector bridge tokio runtime")?;
    runtime.block_on(async move {
        match id {
            "lark-user" => lark_connector::run("user", args).await,
            "lark-bot" => lark_connector::run("bot", args).await,
            #[cfg(unix)]
            "wechat-user" => wechat_connector::run("user", args).await,
            other => Err(anyhow::anyhow!(
                "unknown connector id `{other}`; this puffer build does not bundle a bridge for it"
            )),
        }
    })
}

fn resolve_session_query(session_store: &SessionStore, query: &str) -> Result<Uuid> {
    if let Ok(session_id) = query.parse() {
        return Ok(session_id);
    }
    let session = session_store
        .find_session(query)?
        .ok_or_else(|| anyhow::anyhow!("no session matched `{query}`"))?;
    Ok(session.id)
}

/// Runs `puffer serve` — headless connector-only mode. Blocks on
/// SIGINT/SIGTERM, then drives a graceful shutdown for every connector.
fn run_serve_command(
    config_override: Option<&str>,
    cwd: &std::path::Path,
    config: puffer_config::PufferConfig,
    resources: puffer_resources::LoadedResources,
    providers: ProviderRegistry,
    auth_store: AuthStore,
    auth_path: std::path::PathBuf,
    paths: &ConfigPaths,
) -> Result<()> {
    let connectors_config = match config_override {
        Some(path) => puffer_connector_core::ConnectorsConfig::load(std::path::Path::new(path))?,
        None => connectors::load_connectors_config(paths)?,
    };

    if !connectors_config.enabled {
        eprintln!("connectors are disabled (enabled = false); nothing to run");
        return Ok(());
    }

    let session_store = SessionStore::from_paths(paths)?;
    let map_path = connectors::session_map_path(paths);
    let runtime = connectors::build_runtime(
        config,
        resources,
        providers,
        auth_store,
        auth_path,
        session_store,
        cwd.to_path_buf(),
        &map_path,
    )?;

    let running = connectors::start_configured_connectors(runtime, &connectors_config)?;
    if running.is_empty() {
        eprintln!("no connectors started; add entries to connectors.toml or enable more features");
        return Ok(());
    }

    let ids = running.ids();
    eprintln!(
        "puffer serve: running {} connector(s): {}",
        ids.len(),
        ids.join(", ")
    );
    eprintln!("press Ctrl+C to stop");

    // Block on SIGINT. `ctrlc` installs the handler and signals via the
    // shared channel; we block on receive once it fires.
    let (signal_tx, signal_rx) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = signal_tx.send(());
    })
    .map_err(|error| anyhow::anyhow!("failed to install Ctrl+C handler: {error}"))?;
    let _ = signal_rx.recv();

    eprintln!("shutting down connectors…");
    let errors = running.shutdown();
    for (id, error) in errors {
        eprintln!("  connector `{id}` shutdown error: {error:#}");
    }
    puffer_core::shutdown_runtime_services().ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn cli_proxy_clients_accept_configured_proxy() {
        let config = puffer_config::PufferConfig {
            network: puffer_config::NetworkConfig {
                proxy: puffer_config::ProxyConfig {
                    enabled: false,
                    selected: None,
                    bypass: vec![],
                    proxies: vec![],
                },
            },
            ..puffer_config::PufferConfig::default()
        };

        let discovery = super::proxy_discovery_client(&config).expect("discovery client");
        let oauth = super::proxy_oauth_client(&config, puffer_provider_openai::OPENAI_TOKEN_URL)
            .expect("oauth client");
        let _ = (discovery, oauth);
    }
}
