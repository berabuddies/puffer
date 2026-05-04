use crate::cli_args::{AutoModeCommand, McpCommand, McpTransport, PluginCommand, ResourceScope};
use crate::resource_fs::{
    disabled_variant, enabled_variant, find_yaml_file_by_id, is_disabled_yaml_path,
    plugin_manifest_path, remove_if_exists, sorted_dir_entries,
};
use anyhow::{Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_core::load_agent_catalog;
use puffer_mcp_oauth::PersistedTokens;
use puffer_provider_registry::{AuthMode, AuthStore, ProviderRegistry};
use puffer_resources::{LoadedResources, McpServerSpec, PluginSpec, SourceKind};
use puffer_runner_api::{OAuthStatus, OAuthTokensPayload, ToolRunner};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DISABLED_PLUGIN_PLACEHOLDER_PREFIX: &str =
    "Disabled plugin placeholder created by `puffer plugin disable`.";

/// Lists the configured workspace agents file in a CLI-friendly format.
pub(crate) fn run_agents_command(
    paths: &ConfigPaths,
    config: &PufferConfig,
    setting_sources: Option<&str>,
) -> Result<()> {
    ensure_workspace_dirs(paths)?;
    let agents = load_agent_catalog(&paths.workspace_root, config.default_model.as_deref())?;
    let agents_path = paths
        .workspace_config_dir
        .join("resources/agents/workspace.yaml");
    let mut text = String::new();
    if let Some(setting_sources) = setting_sources {
        let _ = writeln!(
            &mut text,
            "setting_sources={setting_sources} (Puffer loads builtin, user, and workspace agent resources; project/local map to workspace)"
        );
    }
    if agents.is_empty() {
        let _ = writeln!(&mut text, "No configured agents.");
    } else {
        let _ = writeln!(&mut text, "Configured agents:");
        for agent in agents {
            let _ = writeln!(
                &mut text,
                "- {} [{}] model={} {}",
                agent.selector,
                source_kind_label(agent.source_kind),
                agent.model.as_deref().unwrap_or("<inherit>"),
                agent.description
            );
        }
    }
    let _ = writeln!(&mut text, "path={}", agents_path.display());
    print!("{text}");
    Ok(())
}

/// Prints a non-interactive doctor summary for the current workspace.
pub(crate) fn run_doctor_command(
    config: &PufferConfig,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    cwd: &Path,
) -> Result<()> {
    let mut text = String::from("Puffer doctor summary:\n");
    let _ = writeln!(&mut text, "version={}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(&mut text, "cwd={}", cwd.display());
    let _ = writeln!(
        &mut text,
        "default_provider={}",
        config.default_provider.as_deref().unwrap_or("<unset>")
    );
    let _ = writeln!(
        &mut text,
        "default_model={}",
        config.default_model.as_deref().unwrap_or("<unset>")
    );
    let _ = writeln!(&mut text, "providers={}", providers.providers().count());
    let _ = writeln!(
        &mut text,
        "stored_auth_providers={}",
        auth_store.provider_ids().count()
    );
    let _ = writeln!(&mut text, "tools={}", resources.tools.len());
    let _ = writeln!(&mut text, "prompts={}", resources.prompts.len());
    let _ = writeln!(&mut text, "skills={}", resources.skills.len());
    let _ = writeln!(&mut text, "plugins={}", resources.plugins.len());
    let _ = writeln!(&mut text, "mcp_servers={}", resources.mcp_servers.len());
    let _ = writeln!(&mut text, "ides={}", resources.ides.len());
    let _ = writeln!(&mut text, "hooks={}", resources.hooks.len());
    let _ = writeln!(
        &mut text,
        "resource_diagnostics={}",
        resources.diagnostics.len()
    );
    if !resources.diagnostics.is_empty() {
        let _ = writeln!(&mut text, "diagnostics:");
        for diagnostic in &resources.diagnostics {
            let _ = writeln!(&mut text, "- {diagnostic}");
        }
    }
    print!("{text}");
    Ok(())
}

/// Handles top-level MCP management commands.
///
/// Login / login-status / logout drive the configured tool runner so that
/// in remote-runner deployments tokens land on the runner's machine, not
/// the puffer-cli host. Local-mode is unchanged: the local runner writes
/// to `<config>/puffer/mcp-tokens/...` exactly as `puffer mcp login` did
/// before pass 1.5f.
pub(crate) fn run_mcp_command(
    command: Option<McpCommand>,
    paths: &ConfigPaths,
    config: &PufferConfig,
    resources: &LoadedResources,
    cwd: &Path,
) -> Result<()> {
    match command.unwrap_or(McpCommand::List) {
        McpCommand::List => print_mcp_list(resources),
        McpCommand::Get { name } => print_mcp_details(resources, &name),
        McpCommand::Add {
            name,
            command_or_url,
            args,
            scope,
            transport,
        } => add_mcp_server(paths, scope, transport, &name, &command_or_url, &args),
        McpCommand::AddJson { name, json, scope } => {
            add_mcp_server_from_json(paths, scope, &name, &json)
        }
        McpCommand::AddFromClaudeDesktop => {
            println!("Import from Claude Desktop is not implemented in Puffer yet.");
            Ok(())
        }
        McpCommand::Remove { name, scope } => remove_mcp_server(paths, scope, &name),
        McpCommand::ResetProjectChoices => {
            println!("Puffer does not persist project MCP approval state yet.");
            Ok(())
        }
        McpCommand::Serve => {
            println!("Puffer does not expose an MCP server bridge yet.");
            Ok(())
        }
        McpCommand::Login { name, no_browser } => {
            let runner =
                crate::runner_selection::select_tool_runner(config, resources, cwd.to_path_buf());
            run_mcp_login(resources, &name, no_browser, runner)
        }
        McpCommand::LoginStatus { name } => {
            let runner =
                crate::runner_selection::select_tool_runner(config, resources, cwd.to_path_buf());
            print_mcp_login_status(&name, runner)
        }
        McpCommand::Logout { name } => {
            let runner =
                crate::runner_selection::select_tool_runner(config, resources, cwd.to_path_buf());
            run_mcp_logout(&name, runner)
        }
    }
}

/// Drive the OAuth interactive login for `name`. Resolves the server
/// spec, runs discovery + DCR + browser-redirect + token-exchange on the
/// puffer-cli host, then ships the resulting bundle to `runner` over the
/// `ToolRunner::push_oauth_tokens` trait method.
///
/// The browser flow always runs locally — opening browsers belongs on the
/// user's puffer machine, not on the (possibly headless / multi-tenant)
/// runner. Pass `--no-browser` to skip the `webbrowser::open` attempt
/// entirely (still binds the local callback receiver).
fn run_mcp_login(
    resources: &LoadedResources,
    name: &str,
    no_browser: bool,
    runner: Arc<dyn ToolRunner>,
) -> Result<()> {
    let spec = find_oauth_server(resources, name)?;
    let url = http_url_from_spec(&spec)?;
    let oauth_spec = spec
        .oauth
        .as_ref()
        .filter(|o| o.enabled())
        .ok_or_else(|| anyhow::anyhow!("MCP server `{}` is not opted into OAuth", spec.id))?;

    // The CLI uses a CLI-local token dir to host the discovery + DCR + token
    // exchange via puffer-mcp-oauth. After the flow completes we read the
    // resulting bundle off disk and ship it to the runner via the trait;
    // the runner persists it through its own credential store. For
    // local-mode (`LocalToolRunner` over the same process / config) this
    // round-trip is a no-op in practice — both directories resolve to the
    // same `<config>/puffer/mcp-tokens` path.
    let cli_token_dir = puffer_mcp_oauth::default_token_dir();
    let oauth_config = puffer_mcp_oauth::OAuthConfig {
        server_id: spec.id.clone(),
        server_url: url.clone(),
        scopes: oauth_spec.scopes(),
        client_name: oauth_spec
            .client_name()
            .unwrap_or_else(|| format!("puffer-{}", spec.id)),
        token_dir: cli_token_dir.clone(),
    };
    let service = puffer_mcp_oauth::OAuthService::new(oauth_config);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .context("build login runtime")?;
    rt.block_on(async {
        service
            .interactive_login(None, move |url| {
                eprintln!("Authorization URL: {url}");
                if no_browser {
                    eprintln!(
                        "(--no-browser set; open the URL above in any browser to continue)"
                    );
                    return Ok::<(), std::io::Error>(());
                }
                eprintln!("Opening browser to: {url}");
                webbrowser::open(url).map(|_| ()).or_else(|_| {
                    eprintln!(
                        "(could not auto-open browser; please copy the URL above into any browser)"
                    );
                    Ok::<(), std::io::Error>(())
                })
            })
            .await
    })
    .map_err(|e| anyhow::anyhow!("OAuth login failed: {e}"))?;

    // Read the freshly-minted bundle off the CLI-local store, then push
    // it to the runner. For local-mode this is a no-op (the runner store
    // is the same path); for remote-mode the gRPC RPC carries the token
    // bundle to the runner host, where it lands in the runner's
    // mcp-tokens directory.
    let persisted = PersistedTokens::read_from(&cli_token_dir, &spec.id, &url)
        .map_err(|e| {
            anyhow::anyhow!(
                "OAuth flow completed but token file is unreadable: {e}; \
                 expected at {}",
                cli_token_dir.display()
            )
        })?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "OAuth flow completed but no token file was written under {}",
                cli_token_dir.display()
            )
        })?;
    let payload = persisted_to_payload(&persisted);
    runner
        .push_oauth_tokens(name, payload)
        .map_err(|e| anyhow::anyhow!("failed to push OAuth tokens to runner: {e}"))?;

    println!("OAuth login complete for `{name}`. Tokens persisted on the runner.");
    Ok(())
}

fn print_mcp_login_status(name: &str, runner: Arc<dyn ToolRunner>) -> Result<()> {
    let status = runner
        .oauth_status(name)
        .map_err(|e| anyhow::anyhow!("failed to query OAuth status: {e}"))?;
    match status {
        OAuthStatus::Absent => println!("not logged in (no tokens stored on the runner)"),
        OAuthStatus::Present {
            expires_at_ms,
            has_refresh,
            scopes,
        } => {
            println!("logged in (server={name})");
            if let Some(expires_at_ms) = expires_at_ms {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                if expires_at_ms > now_ms {
                    let remaining = (expires_at_ms - now_ms) / 1000;
                    println!("  access token expires in {remaining}s");
                } else {
                    println!("  access token EXPIRED — will refresh on next call");
                }
            }
            if has_refresh {
                println!("  refresh token present");
            }
            if !scopes.is_empty() {
                println!("  scopes: {}", scopes.join(", "));
            }
        }
    }
    Ok(())
}

fn run_mcp_logout(name: &str, runner: Arc<dyn ToolRunner>) -> Result<()> {
    runner
        .clear_oauth_tokens(name)
        .map_err(|e| anyhow::anyhow!("failed to clear OAuth tokens: {e}"))?;
    println!("OAuth tokens cleared for `{name}`.");
    Ok(())
}

fn persisted_to_payload(p: &PersistedTokens) -> OAuthTokensPayload {
    OAuthTokensPayload {
        server_id: p.server_id.clone(),
        server_url: p.server_url.clone(),
        client_id: p.client_id.clone(),
        client_secret: p.client_secret.clone(),
        access_token: p.access_token.clone(),
        token_type: p.token_type.clone(),
        refresh_token: p.refresh_token.clone(),
        scopes: p.scopes.clone(),
        expires_at_ms: p.expires_at_ms,
    }
}

fn find_oauth_server(
    resources: &LoadedResources,
    name: &str,
) -> Result<McpServerSpec> {
    if let Some(server) = resources
        .mcp_servers
        .iter()
        .find(|s| s.value.id == name)
    {
        return Ok(server.value.clone());
    }
    for plugin in &resources.plugins {
        if let Some(server) = plugin.value.mcp_servers.iter().find(|s| s.id == name) {
            return Ok(server.clone());
        }
    }
    anyhow::bail!("no MCP server named `{name}` is configured")
}

fn http_url_from_spec(spec: &McpServerSpec) -> Result<String> {
    let raw = if !spec.endpoint.trim().is_empty() {
        spec.endpoint.trim()
    } else {
        spec.target.trim()
    };
    if raw.is_empty() {
        anyhow::bail!("MCP server `{}` has no HTTP target/endpoint", spec.id);
    }
    Ok(raw.to_string())
}

/// Handles top-level plugin management commands.
pub(crate) fn run_plugin_command(
    command: Option<PluginCommand>,
    paths: &ConfigPaths,
    resources: &LoadedResources,
) -> Result<()> {
    match command.unwrap_or(PluginCommand::List) {
        PluginCommand::List => print_plugin_list(paths, resources),
        PluginCommand::Marketplace => print_builtin_plugin_marketplace(resources),
        PluginCommand::Validate { path } => validate_plugin_manifest(Path::new(&path)),
        PluginCommand::Install { plugin, scope } => {
            install_plugin(paths, resources, scope, &plugin)
        }
        PluginCommand::Uninstall { plugin, scope } => {
            uninstall_plugin(paths, resources, scope, &plugin)
        }
        PluginCommand::Disable { plugin, scope } => {
            let plugin = plugin.ok_or_else(|| anyhow::anyhow!("plugin id is required"))?;
            disable_plugin(paths, resources, scope, &plugin)
        }
        PluginCommand::Enable { plugin, scope } => enable_plugin(paths, resources, scope, &plugin),
        PluginCommand::Update { plugin, scope } => update_plugin(paths, scope, &plugin),
    }
}

/// Prints Puffer's current auto-mode compatibility status.
pub(crate) fn run_auto_mode_command(
    command: Option<AutoModeCommand>,
    paths: &ConfigPaths,
) -> Result<()> {
    match command.unwrap_or(AutoModeCommand::Config) {
        AutoModeCommand::Config => {
            let value = serde_json::json!({
                "supported": false,
                "mode": "manual",
                "sandbox": load_sandbox_settings(paths)?,
                "message": "Puffer does not implement Claude-style auto-mode classification yet."
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        AutoModeCommand::Defaults => {
            let value = serde_json::json!({
                "supported": false,
                "mode": "manual",
                "sandbox": SandboxSettings::default(),
                "allow_rules": [],
                "deny_rules": [],
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        AutoModeCommand::Critique => {
            let settings = load_sandbox_settings(paths)?;
            let mut text =
                String::from("Puffer does not support Claude-style auto-mode rule critique yet.\n");
            let _ = writeln!(&mut text, "sandbox_mode={}", settings.mode);
            let _ = writeln!(&mut text, "auto_allow={}", settings.auto_allow);
            let _ = writeln!(
                &mut text,
                "allow_unsandboxed_fallback={}",
                settings.allow_unsandboxed_fallback
            );
            if settings.excluded_commands.is_empty() {
                let _ = writeln!(&mut text, "excluded_commands=<none>");
            } else {
                let _ = writeln!(
                    &mut text,
                    "excluded_commands={}",
                    settings.excluded_commands.join(", ")
                );
            }
            print!("{text}");
            Ok(())
        }
    }
}

/// Prints source-install instructions for the current checkout.
pub(crate) fn run_install_command(target: Option<&str>, force: bool, cwd: &Path) -> Result<()> {
    let mut command = String::from("cargo install --path crates/puffer-cli");
    if force {
        command.push_str(" --force");
    }
    anyhow::bail!(
        "Puffer does not ship a self-installer.\nrequested_target={}\nworkspace={}\nrun:\n{command}",
        target.unwrap_or("stable"),
        cwd.display()
    )
}

/// Prints manual update instructions for the current checkout.
pub(crate) fn run_update_command(cwd: &Path) -> Result<()> {
    anyhow::bail!(
        "Puffer does not include a self-updater.\nworkspace={}\nupdate steps:\n1. git pull --ff-only\n2. cargo install --path crates/puffer-cli --force",
        cwd.display()
    )
}

/// Stores a long-lived API token for a provider.
pub(crate) fn run_setup_token_command(
    provider: &str,
    token: String,
    auth_store: &mut AuthStore,
    auth_path: &Path,
    providers: &ProviderRegistry,
) -> Result<()> {
    ensure_provider_supports_auth_mode(provider, providers, AuthMode::ApiKey, "setup-token")?;
    auth_store.set_api_key(provider.to_string(), token);
    auth_store.save(auth_path)?;
    println!("stored api token for {provider}");
    Ok(())
}

/// Returns true when the provider explicitly supports the requested auth mode.
pub(crate) fn provider_supports_auth_mode(
    provider: &str,
    providers: &ProviderRegistry,
    mode: &AuthMode,
) -> bool {
    providers
        .provider(provider)
        .map(|descriptor| {
            descriptor
                .auth_modes
                .iter()
                .any(|candidate| candidate == mode)
        })
        .unwrap_or(false)
}

fn print_mcp_list(resources: &LoadedResources) -> Result<()> {
    let mut text = String::from("MCP servers:\n");
    let mut count = 0usize;
    for server in &resources.mcp_servers {
        count += 1;
        let target = if server.value.target.is_empty() {
            server.value.endpoint.as_str()
        } else {
            server.value.target.as_str()
        };
        let _ = writeln!(
            &mut text,
            "- {} [{}] source={} target={} path={}",
            server.value.id,
            server.value.transport,
            source_kind_label(server.source_info.kind),
            target,
            server.source_info.path.display()
        );
    }
    for plugin in &resources.plugins {
        for server in &plugin.value.mcp_servers {
            count += 1;
            let target = if server.target.is_empty() {
                server.endpoint.as_str()
            } else {
                server.target.as_str()
            };
            let _ = writeln!(
                &mut text,
                "- {}:{} [{}] source=plugin:{} target={}",
                plugin.value.id, server.id, server.transport, plugin.value.id, target
            );
        }
    }
    if count == 0 {
        text.push_str("<none>\n");
    }
    print!("{text}");
    Ok(())
}

fn print_mcp_details(resources: &LoadedResources, name: &str) -> Result<()> {
    if let Some((plugin_id, server_id)) = name.split_once(':') {
        for plugin in &resources.plugins {
            if plugin.value.id != plugin_id {
                continue;
            }
            if let Some(server) = plugin
                .value
                .mcp_servers
                .iter()
                .find(|server| server.id == server_id)
            {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "id": format!("{plugin_id}:{server_id}"),
                        "plugin_id": plugin_id,
                        "server_id": server_id,
                        "display_name": server.display_name,
                        "transport": server.transport,
                        "endpoint": server.endpoint,
                        "target": server.target,
                        "description": server.description,
                        "source_kind": format!("plugin:{}", plugin.value.id),
                        "source_path": plugin.source_info.path,
                    }))?
                );
                return Ok(());
            }
        }
    }
    for server in &resources.mcp_servers {
        if server.value.id == name {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "id": server.value.id,
                    "display_name": server.value.display_name,
                    "transport": server.value.transport,
                    "endpoint": server.value.endpoint,
                    "target": server.value.target,
                    "description": server.value.description,
                    "source_kind": source_kind_label(server.source_info.kind),
                    "source_path": server.source_info.path,
                }))?
            );
            return Ok(());
        }
    }
    for plugin in &resources.plugins {
        if let Some(server) = plugin
            .value
            .mcp_servers
            .iter()
            .find(|server| server.id == name)
        {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "id": server.id,
                    "display_name": server.display_name,
                    "transport": server.transport,
                    "endpoint": server.endpoint,
                    "target": server.target,
                    "description": server.description,
                    "source_kind": format!("plugin:{}", plugin.value.id),
                    "source_path": plugin.source_info.path,
                }))?
            );
            return Ok(());
        }
    }
    anyhow::bail!("unknown MCP server {name}")
}

fn add_mcp_server(
    paths: &ConfigPaths,
    scope: ResourceScope,
    transport: McpTransport,
    name: &str,
    command_or_url: &str,
    args: &[String],
) -> Result<()> {
    let spec = match transport {
        McpTransport::Stdio => McpServerSpec {
            id: name.to_string(),
            display_name: name.to_string(),
            transport: "stdio".to_string(),
            endpoint: String::new(),
            target: stdio_target(command_or_url, args),
            description: format!("Workspace MCP server `{name}`"),
            headers: Default::default(),
            oauth: None,
        },
        McpTransport::Sse | McpTransport::Http => {
            if !args.is_empty() {
                anyhow::bail!("additional arguments are only supported for stdio MCP servers");
            }
            McpServerSpec {
                id: name.to_string(),
                display_name: name.to_string(),
                transport: match transport {
                    McpTransport::Sse => "sse",
                    McpTransport::Http => "http",
                    McpTransport::Stdio => unreachable!(),
                }
                .to_string(),
                endpoint: command_or_url.to_string(),
                target: String::new(),
                description: format!("Workspace MCP server `{name}`"),
                headers: Default::default(),
                oauth: None,
            }
        }
    };
    let dir = resource_dir(paths, scope, "mcp_servers");
    fs::create_dir_all(&dir)?;
    write_yaml(&dir.join(format!("{name}.yaml")), &spec)?;
    println!("wrote MCP server `{name}` to {}", dir.display());
    Ok(())
}

fn add_mcp_server_from_json(
    paths: &ConfigPaths,
    scope: ResourceScope,
    name: &str,
    json: &str,
) -> Result<()> {
    let input: McpServerJsonInput = serde_json::from_str(json)
        .with_context(|| format!("failed to parse MCP server JSON for `{name}`"))?;
    let transport = input.transport.unwrap_or_else(|| "stdio".to_string());
    if transport != "stdio" && transport != "sse" && transport != "http" {
        anyhow::bail!("unsupported MCP transport `{transport}`");
    }
    let spec = McpServerSpec {
        id: name.to_string(),
        display_name: input.display_name.unwrap_or_else(|| name.to_string()),
        transport,
        endpoint: input.endpoint.unwrap_or_default(),
        target: input.target.unwrap_or_default(),
        description: input.description.unwrap_or_default(),
        headers: input.headers.unwrap_or_default(),
        oauth: None,
    };
    let dir = resource_dir(paths, scope, "mcp_servers");
    fs::create_dir_all(&dir)?;
    write_yaml(&dir.join(format!("{name}.yaml")), &spec)?;
    println!("wrote MCP server `{name}` to {}", dir.display());
    Ok(())
}

fn remove_mcp_server(paths: &ConfigPaths, scope: ResourceScope, name: &str) -> Result<()> {
    let dir = resource_dir(paths, scope, "mcp_servers");
    let path = find_mcp_file(&dir, name)?
        .ok_or_else(|| anyhow::anyhow!("no MCP server named `{name}` in {}", dir.display()))?;
    fs::remove_file(&path)?;
    println!("removed MCP server `{name}` from {}", path.display());
    Ok(())
}

fn print_plugin_list(paths: &ConfigPaths, resources: &LoadedResources) -> Result<()> {
    let mut text = String::from("Plugins:\n");
    let mut seen = std::collections::BTreeSet::new();
    for plugin in &resources.plugins {
        seen.insert(plugin.value.id.clone());
        let status = if is_disabled_placeholder(&plugin.value) {
            "disabled"
        } else {
            "enabled"
        };
        let _ = writeln!(
            &mut text,
            "- {} [{}] source={} path={}",
            plugin.value.id,
            status,
            source_kind_label(plugin.source_info.kind),
            plugin.source_info.path.display()
        );
    }
    for scope in [ResourceScope::User, ResourceScope::Local] {
        for path in scan_disabled_plugin_files(&resource_dir(paths, scope, "plugins"))? {
            let plugin: PluginSpec = serde_yaml::from_str(&fs::read_to_string(&path)?)?;
            if seen.insert(plugin.id.clone()) {
                let _ = writeln!(
                    &mut text,
                    "- {} [disabled] source={} path={}",
                    plugin.id,
                    scope_label(scope),
                    path.display()
                );
            }
        }
    }
    if seen.is_empty() {
        text.push_str("<none>\n");
    }
    print!("{text}");
    Ok(())
}

fn print_builtin_plugin_marketplace(resources: &LoadedResources) -> Result<()> {
    let mut text = String::from("Builtin plugins:\n");
    let mut count = 0usize;
    for plugin in &resources.plugins {
        if plugin.source_info.kind != SourceKind::Builtin {
            continue;
        }
        count += 1;
        let _ = writeln!(
            &mut text,
            "- {}: {}",
            plugin.value.id, plugin.value.description
        );
    }
    if count == 0 {
        text.push_str("<none>\n");
    }
    print!("{text}");
    Ok(())
}

fn validate_plugin_manifest(path: &Path) -> Result<()> {
    let plugin: PluginSpec = serde_yaml::from_str(&fs::read_to_string(path)?)
        .with_context(|| format!("failed to parse plugin manifest {}", path.display()))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "id": plugin.id,
            "display_name": plugin.display_name,
            "command_count": plugin.commands.len(),
            "skill_count": plugin.skills.len(),
            "mcp_server_count": plugin.mcp_servers.len(),
        }))?
    );
    Ok(())
}

fn install_plugin(
    paths: &ConfigPaths,
    resources: &LoadedResources,
    scope: ResourceScope,
    plugin_ref: &str,
) -> Result<()> {
    let dir = resource_dir(paths, scope, "plugins");
    fs::create_dir_all(&dir)?;
    let (plugin, raw) = resolve_plugin_install_source(resources, plugin_ref)?;
    let enabled_path = plugin_manifest_path(&dir, &plugin.id);
    let disabled_path = disabled_variant(&enabled_path);
    write_string(&enabled_path, &raw)?;
    remove_if_exists(&disabled_path)?;
    println!("installed plugin `{}` into {}", plugin.id, dir.display());
    Ok(())
}

fn uninstall_plugin(
    paths: &ConfigPaths,
    resources: &LoadedResources,
    scope: ResourceScope,
    plugin_id: &str,
) -> Result<()> {
    let dir = resource_dir(paths, scope, "plugins");
    let enabled_path = find_plugin_file(&dir, plugin_id)?;
    let disabled_path = find_disabled_plugin_file(&dir, plugin_id)?;

    if let Some(path) = enabled_path.as_ref() {
        fs::remove_file(&path)?;
    }
    if let Some(path) = disabled_path.as_ref() {
        fs::remove_file(&path)?;
    }
    if enabled_path.is_some() || disabled_path.is_some() {
        println!(
            "removed local plugin state for `{plugin_id}` in {}",
            dir.display()
        );
        return Ok(());
    }
    if resources.plugins.iter().any(|plugin| {
        plugin.value.id == plugin_id && plugin.source_info.kind == SourceKind::Builtin
    }) {
        anyhow::bail!(
            "builtin plugin `{plugin_id}` cannot be uninstalled; use `puffer plugin disable`"
        );
    }
    anyhow::bail!("no plugin `{plugin_id}` found in {}", dir.display())
}

fn disable_plugin(
    paths: &ConfigPaths,
    resources: &LoadedResources,
    scope: ResourceScope,
    plugin_id: &str,
) -> Result<()> {
    let dir = resource_dir(paths, scope, "plugins");
    fs::create_dir_all(&dir)?;
    let enabled_path =
        find_plugin_file(&dir, plugin_id)?.unwrap_or_else(|| plugin_manifest_path(&dir, plugin_id));
    let disabled_path = disabled_variant(&enabled_path);

    if enabled_path.exists() {
        let plugin: PluginSpec = serde_yaml::from_str(&fs::read_to_string(&enabled_path)?)?;
        if is_disabled_placeholder(&plugin) {
            println!(
                "plugin `{plugin_id}` is already disabled at {}",
                enabled_path.display()
            );
            return Ok(());
        }
        remove_if_exists(&disabled_path)?;
        fs::rename(&enabled_path, &disabled_path)?;
        write_yaml(&enabled_path, &disabled_placeholder_for(&plugin))?;
        println!(
            "disabled plugin `{plugin_id}` at {}",
            enabled_path.display()
        );
        return Ok(());
    }
    let plugin = resources
        .plugins
        .iter()
        .find(|plugin| plugin.value.id == plugin_id)
        .map(|plugin| plugin.value.clone())
        .ok_or_else(|| anyhow::anyhow!("unknown plugin `{plugin_id}`"))?;
    let placeholder = PluginSpec {
        id: plugin.id,
        display_name: plugin.display_name,
        description: format!(
            "{DISABLED_PLUGIN_PLACEHOLDER_PREFIX} Original description: {}",
            plugin.description
        ),
        commands: Vec::new(),
        skills: Vec::new(),
        agents: Vec::new(),
        mcp_servers: Vec::new(),
        lsp_servers: Vec::new(),
    };
    write_yaml(&enabled_path, &placeholder)?;
    println!(
        "disabled plugin `{plugin_id}` with override {}",
        enabled_path.display()
    );
    Ok(())
}

fn enable_plugin(
    paths: &ConfigPaths,
    resources: &LoadedResources,
    scope: ResourceScope,
    plugin_id: &str,
) -> Result<()> {
    let dir = resource_dir(paths, scope, "plugins");
    let enabled_path = find_plugin_file(&dir, plugin_id)?;
    let disabled_path = find_disabled_plugin_file(&dir, plugin_id)?;

    if let Some(path) = enabled_path.as_ref() {
        let plugin: PluginSpec = serde_yaml::from_str(&fs::read_to_string(&path)?)?;
        if is_disabled_placeholder(&plugin) {
            if let Some(disabled_path) = disabled_path.as_ref() {
                fs::remove_file(path)?;
                fs::rename(disabled_path, path)?;
                println!("enabled plugin `{plugin_id}` at {}", path.display());
            } else {
                fs::remove_file(path)?;
                println!(
                    "re-enabled builtin plugin `{plugin_id}` by removing {}",
                    path.display()
                );
            }
            return Ok(());
        }
        println!(
            "plugin `{plugin_id}` is already enabled at {}",
            path.display()
        );
        return Ok(());
    }
    if let Some(path) = disabled_path.as_ref() {
        let enabled_path = enabled_variant(path);
        fs::rename(path, &enabled_path)?;
        println!("enabled plugin `{plugin_id}` at {}", enabled_path.display());
        return Ok(());
    }
    if resources
        .plugins
        .iter()
        .any(|plugin| plugin.value.id == plugin_id && !is_disabled_placeholder(&plugin.value))
    {
        println!("plugin `{plugin_id}` is already enabled.");
        return Ok(());
    }
    anyhow::bail!("unknown plugin `{plugin_id}`")
}

fn update_plugin(paths: &ConfigPaths, scope: ResourceScope, plugin_id: &str) -> Result<()> {
    let dir = resource_dir(paths, scope, "plugins");
    fs::create_dir_all(&dir)?;
    let (source_path, raw) = resolve_plugin_update_source(paths, plugin_id)?.ok_or_else(|| {
        anyhow::anyhow!("no builtin or user plugin source is available for `{plugin_id}`")
    })?;
    let enabled_path =
        find_plugin_file(&dir, plugin_id)?.unwrap_or_else(|| plugin_manifest_path(&dir, plugin_id));
    let disabled_path = find_disabled_plugin_file(&dir, plugin_id)?;
    if enabled_path.exists() {
        let plugin: PluginSpec = serde_yaml::from_str(&fs::read_to_string(&enabled_path)?)?;
        if is_disabled_placeholder(&plugin) {
            if let Some(disabled_path) = disabled_path {
                write_string(&disabled_path, &raw)?;
                println!(
                    "updated disabled plugin `{plugin_id}` from {}",
                    source_path.display()
                );
                return Ok(());
            }
            println!(
                "plugin `{plugin_id}` remains disabled; no local copy required refreshing from {}",
                source_path.display()
            );
            return Ok(());
        }
    }
    if let Some(disabled_path) = disabled_path {
        write_string(&disabled_path, &raw)?;
        println!(
            "updated disabled plugin `{plugin_id}` from {}",
            source_path.display()
        );
        return Ok(());
    }
    write_string(&enabled_path, &raw)?;
    println!(
        "updated plugin `{plugin_id}` from {}",
        source_path.display()
    );
    Ok(())
}

fn load_sandbox_settings(paths: &ConfigPaths) -> Result<SandboxSettings> {
    let path = paths.workspace_config_dir.join("sandbox.toml");
    if !path.exists() {
        return Ok(SandboxSettings::default());
    }
    Ok(toml::from_str(&fs::read_to_string(path)?)?)
}

fn ensure_provider_supports_auth_mode(
    provider: &str,
    providers: &ProviderRegistry,
    mode: AuthMode,
    command_name: &str,
) -> Result<()> {
    if provider_supports_auth_mode(provider, providers, &mode) {
        return Ok(());
    }
    let mode_name = match mode {
        AuthMode::ApiKey => "API key",
        AuthMode::OAuth => "OAuth",
        AuthMode::SessionIngress => "session ingress",
    };
    anyhow::bail!("{command_name} requires {mode_name} support for provider `{provider}`")
}

fn resource_dir(paths: &ConfigPaths, scope: ResourceScope, kind: &str) -> PathBuf {
    match scope {
        ResourceScope::User => paths.user_config_dir.join("resources").join(kind),
        ResourceScope::Local | ResourceScope::Project => {
            paths.workspace_config_dir.join("resources").join(kind)
        }
    }
}

fn write_yaml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_string(path, &serde_yaml::to_string(value)?)
}

fn write_string(path: &Path, raw: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, raw)?;
    Ok(())
}

fn resolve_plugin_install_source(
    resources: &LoadedResources,
    plugin_ref: &str,
) -> Result<(PluginSpec, String)> {
    let path = Path::new(plugin_ref);
    if path.exists() {
        let raw = fs::read_to_string(path)?;
        let plugin: PluginSpec = serde_yaml::from_str(&raw)?;
        return Ok((plugin, raw));
    }
    let plugin = resources
        .plugins
        .iter()
        .find(|plugin| plugin.value.id == plugin_ref)
        .ok_or_else(|| anyhow::anyhow!("unknown plugin `{plugin_ref}`"))?;
    Ok((
        plugin.value.clone(),
        fs::read_to_string(&plugin.source_info.path)?,
    ))
}

fn resolve_plugin_update_source(
    paths: &ConfigPaths,
    plugin_id: &str,
) -> Result<Option<(PathBuf, String)>> {
    for dir in [
        paths.user_config_dir.join("resources/plugins"),
        paths.builtin_resources_dir.join("plugins"),
    ] {
        if let Some(path) = find_plugin_file(&dir, plugin_id)? {
            return Ok(Some((path.clone(), fs::read_to_string(path)?)));
        }
    }
    Ok(None)
}

fn find_mcp_file(dir: &Path, name: &str) -> Result<Option<PathBuf>> {
    find_yaml_file_by_id::<McpServerSpec>(dir, name)
}

fn find_plugin_file(dir: &Path, plugin_id: &str) -> Result<Option<PathBuf>> {
    find_yaml_file_by_id::<PluginSpec>(dir, plugin_id)
}

fn find_disabled_plugin_file(dir: &Path, plugin_id: &str) -> Result<Option<PathBuf>> {
    if !dir.exists() {
        return Ok(None);
    }
    for path in scan_disabled_plugin_files(dir)? {
        let plugin: PluginSpec = serde_yaml::from_str(&fs::read_to_string(&path)?)?;
        if plugin.id == plugin_id {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn scan_disabled_plugin_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in sorted_dir_entries(dir)? {
        let path = entry.path();
        if is_disabled_yaml_path(&path) {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn stdio_target(command_or_url: &str, args: &[String]) -> String {
    std::iter::once(command_or_url.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn source_kind_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Builtin => "builtin",
        SourceKind::User => "user",
        SourceKind::Workspace => "workspace",
    }
}

fn scope_label(scope: ResourceScope) -> &'static str {
    match scope {
        ResourceScope::User => "user",
        ResourceScope::Local | ResourceScope::Project => "workspace",
    }
}

fn is_disabled_placeholder(plugin: &PluginSpec) -> bool {
    plugin
        .description
        .starts_with(DISABLED_PLUGIN_PLACEHOLDER_PREFIX)
}

fn disabled_placeholder_for(plugin: &PluginSpec) -> PluginSpec {
    PluginSpec {
        id: plugin.id.clone(),
        display_name: plugin.display_name.clone(),
        description: format!(
            "{DISABLED_PLUGIN_PLACEHOLDER_PREFIX} Original description: {}",
            plugin.description
        ),
        commands: Vec::new(),
        skills: Vec::new(),
        agents: Vec::new(),
        mcp_servers: Vec::new(),
        lsp_servers: Vec::new(),
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SandboxSettings {
    #[serde(default = "default_sandbox_mode")]
    mode: String,
    #[serde(default)]
    auto_allow: bool,
    #[serde(default)]
    allow_unsandboxed_fallback: bool,
    #[serde(default)]
    excluded_commands: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct McpServerJsonInput {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    transport: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    headers: Option<std::collections::BTreeMap<String, String>>,
}

fn default_sandbox_mode() -> String {
    "workspace-write".to_string()
}
