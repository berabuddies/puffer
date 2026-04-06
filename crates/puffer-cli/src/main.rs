mod auth_credentials;
mod auth_provider;
mod authflow;

use anyhow::Result;
use clap::{Parser, Subcommand};
use puffer_config::{ensure_workspace_dirs, load_config, ConfigPaths};
use puffer_core::{supported_commands, AppState};
use puffer_provider_openai::{
    exchange_authorization_code as exchange_openai_code,
    parse_authorization_input as parse_openai_authorization_input,
    refresh_oauth_token as refresh_openai_oauth_token,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry, StoredCredential};
use puffer_resources::load_resources;
use puffer_session_store::SessionStore;
use puffer_tools::ToolRegistry;
use puffer_transport_anthropic::{
    build_messages_request, exchange_authorization_code as exchange_anthropic_code,
    parse_authorization_input as parse_anthropic_authorization_input,
    refresh_oauth_token as refresh_anthropic_oauth_token, AnthropicAuth, AnthropicMessage,
    AnthropicModelRequest, AnthropicRequestConfig, ANTHROPIC_API_BASE_URL,
    ANTHROPIC_MANUAL_REDIRECT_URL,
};
use std::io::Read as _;
use std::io::Write as _;
use std::time::Duration;
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

#[derive(Debug, Parser)]
#[command(
    bin_name = "puffer",
    subcommand_negates_reqs = true,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    subcommand: Option<Command>,

    /// Optional prompt to submit immediately when launching the TUI.
    prompt: Option<String>,

    /// Disable alternate-screen mode for the TUI.
    #[arg(long = "no-alt-screen", default_value_t = false)]
    no_alt_screen: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Manage stored provider credentials.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// List the supported slash commands.
    Commands,
    /// List known providers and models.
    Providers,
    /// Inspect or modify stored sessions.
    Sessions {
        #[command(subcommand)]
        command: Option<SessionCommand>,
    },
    /// List loaded declarative resources.
    Resources,
    /// Print a sample Anthropic request fixture from the current config.
    AnthropicRequestFixture,
    /// Execute a built-in tool directly for local testing.
    Tool {
        #[command(subcommand)]
        command: ToolCommand,
    },
    /// Print a stored session as JSON.
    Resume {
        /// The session UUID to resume.
        session_id: String,
    },
    /// Fork a stored session into the current working directory.
    Fork {
        /// The session UUID to fork and open.
        session_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    /// Show stored credential status.
    Status,
    /// Store an API key for a provider.
    SetApiKey {
        /// Provider id, for example `anthropic` or `openai`.
        provider: String,
        /// API key value. Omit and use `--stdin` to read from stdin.
        api_key: Option<String>,
        /// Read the API key from stdin.
        #[arg(long = "stdin", default_value_t = false)]
        stdin: bool,
    },
    /// Remove any stored credentials for a provider.
    Clear {
        /// Provider id to clear.
        provider: String,
    },
    /// Print an OAuth authorization URL for a provider when supported.
    OauthUrl {
        /// Provider id to authorize.
        provider: String,
    },
    /// Run a terminal-guided OAuth login flow for a provider.
    Login {
        /// Provider id to authorize.
        provider: String,
        /// Optional pasted callback URL or authorization code.
        value: Option<String>,
        /// Read the callback URL or authorization code from stdin.
        #[arg(long = "stdin", default_value_t = false)]
        stdin: bool,
    },
    /// Generate a full PKCE OAuth start bundle for a provider.
    OauthStart {
        /// Provider id to authorize.
        provider: String,
    },
    /// Exchange a pasted OAuth authorization code for stored credentials.
    OauthExchange {
        /// Provider id to authorize.
        provider: String,
        /// Authorization code or callback URL.
        value: Option<String>,
        /// PKCE code verifier from the `oauth-start` output.
        #[arg(long = "verifier")]
        verifier: String,
        /// Explicit OAuth state for providers that require it.
        #[arg(long = "state")]
        state: Option<String>,
        /// Read the callback URL or code from stdin.
        #[arg(long = "stdin", default_value_t = false)]
        stdin: bool,
    },
    /// Refresh stored OAuth credentials for a provider.
    OauthRefresh {
        /// Provider id to refresh.
        provider: String,
    },
}

#[derive(Debug, Subcommand)]
enum ToolCommand {
    /// List the registered built-in tools and their policies.
    List,
    /// Show one registered tool and its policies.
    Describe {
        /// Tool id, for example `bash`, `read_file`, or `write_file`.
        tool_id: String,
    },
    /// Run a registered tool using a JSON argument payload.
    Run {
        /// Tool id, for example `bash`, `read_file`, or `write_file`.
        tool_id: String,
        /// JSON argument payload.
        args: String,
    },
    /// Run the built-in bash tool with a shell command.
    Bash {
        /// Shell command to execute.
        command: String,
    },
    /// Run the built-in read tool against a file path.
    Read {
        /// File path to read.
        path: String,
    },
    /// Run the built-in write tool against a file path.
    Write {
        /// File path to write.
        path: String,
        /// File contents to write.
        contents: String,
    },
    /// Run the built-in replace-in-file tool against a file path.
    Replace {
        /// File path to modify.
        path: String,
        /// Text to replace.
        old: String,
        /// Replacement text.
        new: String,
        /// Replace all occurrences instead of the first match only.
        #[arg(long = "all", default_value_t = false)]
        replace_all: bool,
    },
    /// Run the built-in move-path tool.
    Move {
        /// Source path to move.
        from: String,
        /// Destination path.
        to: String,
    },
    /// Run the built-in remove-path tool.
    Remove {
        /// Path to remove.
        path: String,
        /// Remove directories recursively.
        #[arg(long = "recursive", default_value_t = false)]
        recursive: bool,
    },
    /// Run the built-in directory listing tool.
    ListDir {
        /// Optional directory path to list. Defaults to the current working directory.
        path: Option<String>,
    },
    /// Run the built-in text search tool.
    SearchText {
        /// Query text to search for.
        query: String,
        /// Optional file or directory path to search. Defaults to the current working directory.
        path: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// List stored sessions.
    List,
    /// Set or clear the free-form note on a session.
    Note {
        /// Session UUID to update.
        session_id: String,
        /// Note text to store.
        note: Option<String>,
        /// Clear the note instead of setting it.
        #[arg(long = "clear", default_value_t = false)]
        clear: bool,
    },
    /// Add a tag to a session.
    TagAdd {
        /// Session UUID to update.
        session_id: String,
        /// Tag value to add.
        tag: String,
    },
    /// Remove a tag from a session.
    TagRemove {
        /// Session UUID to update.
        session_id: String,
        /// Tag value to remove.
        tag: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    let paths = ConfigPaths::discover(&cwd);
    ensure_workspace_dirs(&paths)?;
    let config = load_config(&paths)?;
    let auth_path = paths.user_config_dir.join("auth.json");
    let mut auth_store = AuthStore::load(&auth_path)?;
    let resources = load_resources(&paths)?;

    let mut providers = ProviderRegistry::new();
    for provider in &resources.providers {
        providers.register_with_source(
            provider.value.clone().into_descriptor(),
            provider.source_info.as_provider_source(),
        );
    }
    let _ = providers.discover_and_merge_all(&auth_store);

    match cli.subcommand {
        Some(Command::Auth { command }) => {
            run_auth_command(command, &mut auth_store, &auth_path, &providers)
        }
        Some(Command::Commands) => {
            println!("{}", serde_json::to_string_pretty(&supported_commands())?);
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
                    "skills": resources.skills,
                    "plugins": resources.plugins,
                    "mcp_servers": resources.mcp_servers,
                    "ides": resources.ides,
                    "mascots": resources.mascots,
                }))?
            );
            Ok(())
        }
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
        Some(Command::Tool { command }) => run_tool_command(command, &resources, &cwd),
        Some(Command::Resume { session_id }) => {
            let session_store = SessionStore::from_paths(&paths)?;
            let session =
                session_store.load_session(resolve_session_query(&session_store, &session_id)?)?;
            let mut state = AppState::from_session_record(config.clone(), session);
            puffer_tui::run_app(
                &mut state,
                &resources,
                &mut providers,
                &mut auth_store,
                &auth_path,
                &session_store,
                None,
                cli.no_alt_screen || config.ui.no_alt_screen,
            )
        }
        Some(Command::Fork { session_id }) => {
            let session_store = SessionStore::from_paths(&paths)?;
            let source = resolve_session_query(&session_store, &session_id)?;
            let session = session_store.fork_session(source, std::env::current_dir()?)?;
            let record = session_store.load_session(session.id)?;
            let mut state = AppState::from_session_record(config.clone(), record);
            puffer_tui::run_app(
                &mut state,
                &resources,
                &mut providers,
                &mut auth_store,
                &auth_path,
                &session_store,
                None,
                cli.no_alt_screen || config.ui.no_alt_screen,
            )
        }
        None => {
            let session_store = SessionStore::from_paths(&paths)?;
            let session = session_store.create_session(cwd.clone())?;
            let mut state = AppState::new(config.clone(), cwd, session);
            puffer_tui::run_app(
                &mut state,
                &resources,
                &mut providers,
                &mut auth_store,
                &auth_path,
                &session_store,
                cli.prompt,
                cli.no_alt_screen || config.ui.no_alt_screen,
            )
        }
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
            let result =
                registry.execute("bash", cwd, puffer_tools::ToolInput::Bash { command })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ToolCommand::Read { path } => {
            let registry = ToolRegistry::from_resources(resources);
            let result = registry.execute(
                "read_file",
                cwd,
                puffer_tools::ToolInput::ReadFile { path: path.into() },
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
    auth_store: &mut AuthStore,
    auth_path: &std::path::Path,
    providers: &ProviderRegistry,
) -> Result<()> {
    match command {
        AuthCommand::Status => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &auth_store
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
                        .collect::<Vec<_>>(),
                )?
            );
        }
        AuthCommand::SetApiKey {
            provider,
            api_key,
            stdin,
        } => {
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
            let removed = auth_store.remove(&provider);
            auth_store.save(auth_path)?;
            if removed.is_some() {
                println!("cleared credentials for {provider}");
            } else {
                println!("no credentials were stored for {provider}");
            }
        }
        AuthCommand::OauthUrl { provider } => {
            let bundle = oauth_start_bundle_for_provider(providers, &provider)?;
            println!("{}", bundle.authorization_url);
        }
        AuthCommand::Login {
            provider,
            value,
            stdin,
        } => {
            run_login_flow(&provider, value, stdin, auth_store, auth_path, providers)?;
        }
        AuthCommand::OauthStart { provider } => {
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
                    let credential = exchange_openai_code(&code, &verifier, None)?;
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
                    let credential = exchange_anthropic_code(
                        &code,
                        &verifier,
                        &expected_state,
                        Some(&redirect_uri),
                        Some(ANTHROPIC_API_BASE_URL),
                    )?;
                    store_anthropic_credential(auth_store, &provider, credential)?;
                }
                None => anyhow::bail!("oauth exchange is not implemented for {provider}"),
            }
            auth_store.save(auth_path)?;
            println!("stored oauth credentials for {provider}");
        }
        AuthCommand::OauthRefresh { provider } => {
            let credential = auth_store
                .get(&provider)
                .ok_or_else(|| anyhow::anyhow!("no credentials stored for {provider}"))?;
            let StoredCredential::OAuth(existing) = credential else {
                anyhow::bail!("stored credential for {provider} is not oauth");
            };
            let refreshed = match oauth_family_for_provider(providers, &provider) {
                Some(OauthFamily::OpenAi) => {
                    StoredCredential::OAuth(to_registry_oauth_credential_openai(
                        refresh_openai_oauth_token(&existing.refresh_token)?,
                    ))
                }
                Some(OauthFamily::Anthropic) => {
                    store_ready_credential_from_anthropic(refresh_anthropic_oauth_token(
                        &existing.refresh_token,
                        anthropic_refresh_scopes(existing).as_deref(),
                        Some(ANTHROPIC_API_BASE_URL),
                        Some(&registry_to_anthropic_oauth_credential(existing)),
                    )?)?
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

fn run_login_flow(
    provider: &str,
    value: Option<String>,
    stdin: bool,
    auth_store: &mut AuthStore,
    auth_path: &std::path::Path,
    providers: &ProviderRegistry,
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
            let credential = exchange_openai_code(&code, &bundle.verifier, None)?;
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
            let credential = exchange_anthropic_code(
                &code,
                &bundle.verifier,
                &bundle.state,
                Some(&redirect_uri),
                Some(ANTHROPIC_API_BASE_URL),
            )?;
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

fn resolve_session_query(session_store: &SessionStore, query: &str) -> Result<Uuid> {
    if let Ok(session_id) = query.parse() {
        return Ok(session_id);
    }
    let session = session_store
        .find_session(query)?
        .ok_or_else(|| anyhow::anyhow!("no session matched `{query}`"))?;
    Ok(session.id)
}
