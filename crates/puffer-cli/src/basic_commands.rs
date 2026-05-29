use crate::cli_args::AutoModeCommand;
use anyhow::Result;
use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_core::{
    lambda_skill_doctor_warning_lines, load_agent_catalog, render_lambda_skill_doctor_status,
};
use puffer_provider_registry::{AuthMode, AuthStore, ProviderRegistry};
use puffer_resources::{LoadedResources, SourceKind};
use std::fmt::Write as _;
use std::path::Path;

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
    let _ = writeln!(
        &mut text,
        "internal_tools={}",
        resources.internal_tools.len()
    );
    let _ = writeln!(&mut text, "prompts={}", resources.prompts.len());
    let _ = writeln!(&mut text, "skills={}", resources.skills.len());
    let _ = writeln!(&mut text, "plugins={}", resources.plugins.len());
    let _ = writeln!(&mut text, "mcp_servers={}", resources.mcp_servers.len());
    let _ = writeln!(&mut text, "ides={}", resources.ides.len());
    let _ = writeln!(&mut text, "hooks={}", resources.hooks.len());
    let _ = writeln!(
        &mut text,
        "{}",
        render_lambda_skill_doctor_status(resources)
    );
    let lambda_warnings = lambda_skill_doctor_warning_lines(resources);
    if !lambda_warnings.is_empty() {
        let _ = writeln!(&mut text, "lambda_skill_warnings:");
        for warning in lambda_warnings {
            let _ = writeln!(&mut text, "- {warning}");
        }
    }
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

/// Runs one top-level auto-mode command.
pub(crate) fn run_auto_mode_command(
    command: Option<AutoModeCommand>,
    paths: &ConfigPaths,
) -> Result<()> {
    match command.unwrap_or(AutoModeCommand::Config) {
        AutoModeCommand::Config => {
            let value = serde_json::json!({
                "supported": true,
                "mode": "acl",
                "permissions_acl": paths.workspace_config_dir.join("permissions.acl"),
                "message": "Auto mode uses the same project ACL evaluator as normal permission prompts."
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        AutoModeCommand::Defaults => {
            let value = serde_json::json!({
                "supported": true,
                "mode": "acl",
                "allow_rules": [
                    "cwd read/write",
                    "preapproved shell commands",
                    "project ACL allow rules"
                ],
                "deny_rules": [],
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        AutoModeCommand::Critique => {
            let mut text = String::from("Auto-mode permission critique:\n");
            let _ = writeln!(
                &mut text,
                "permissions_acl={}",
                paths.workspace_config_dir.join("permissions.acl").display()
            );
            let _ = writeln!(&mut text, "read_write_default=cwd");
            let _ = writeln!(&mut text, "bash_default=preapproved commands only");
            let _ = writeln!(&mut text, "browser_default=ask per domain/action");
            print!("{text}");
            Ok(())
        }
    }
}

/// Runs the top-level install command.
pub(crate) fn run_install_command(target: Option<&str>, force: bool, cwd: &Path) -> Result<()> {
    let target = target.unwrap_or("shell-integration");
    let mut command = String::from("cargo install --path crates/puffer-cli");
    if force {
        command.push_str(" --force");
    }
    anyhow::bail!(
        "Puffer does not ship a self-installer.\nrequested_target={target}\nworkspace={}\nrun:\n{command}",
        cwd.display()
    )
}

/// Runs the top-level update command.
pub(crate) fn run_update_command(cwd: &Path) -> Result<()> {
    anyhow::bail!(
        "Puffer does not include a self-updater.\nworkspace={}\nupdate steps:\n1. git pull --ff-only\n2. cargo install --path crates/puffer-cli --force",
        cwd.display()
    )
}

/// Stores an API key through the setup-token compatibility command.
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

/// Returns whether the provider supports the requested auth mode.
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

fn ensure_provider_supports_auth_mode(
    provider: &str,
    providers: &ProviderRegistry,
    mode: AuthMode,
    command: &str,
) -> Result<()> {
    if provider_supports_auth_mode(provider, providers, &mode) {
        return Ok(());
    }
    let mode_label = match mode {
        AuthMode::ApiKey => "API key",
        AuthMode::OAuth => "OAuth",
        AuthMode::SessionIngress => "session ingress",
    };
    anyhow::bail!("provider `{provider}` does not support {mode_label} auth for `{command}`")
}

fn source_kind_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Builtin => "builtin",
        SourceKind::User => "user",
        SourceKind::Workspace => "workspace",
    }
}
