use super::emit_system;
use crate::AppState;
use anyhow::Result;
use puffer_config::ConfigPaths;
use puffer_provider_registry::{
    AuthMode, AuthStore, ProviderRegistry, ProviderSourceKind, StoredCredential,
};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;
use puffer_tools::ToolRegistry;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LARGE_MEMORY_FILE_THRESHOLD_BYTES: u64 = 40_000;
const DOCTOR_DEPENDENCIES: &[&str] = &["git", "rg", "gh", "qrencode", "tmux", "bash"];

#[derive(Debug)]
struct GitStatusSummary {
    root: Option<PathBuf>,
    branch: Option<String>,
    dirty: Option<bool>,
    error: Option<String>,
}

#[derive(Debug)]
struct DependencyStatus {
    name: &'static str,
    path: Option<PathBuf>,
}

#[derive(Debug)]
struct DoctorWarning {
    summary: String,
    detail: Option<String>,
}

/// Renders the richer `/doctor` report used by the transcript command and TUI overlay.
pub(crate) fn render_doctor_report(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> Result<String> {
    let paths = ConfigPaths::discover(&state.cwd);
    let tool_registry = ToolRegistry::from_resources(resources);
    let git = collect_git_status(&state.cwd);
    let dependencies = collect_dependency_statuses();
    let warnings =
        collect_doctor_warnings(state, resources, providers, auth_store, &git, &dependencies);

    let mut text = String::new();
    writeln!(&mut text, "Puffer doctor")?;
    writeln!(&mut text, "version={}", env!("CARGO_PKG_VERSION"))?;
    writeln!(
        &mut text,
        "executable={}",
        std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "<unavailable>".to_string())
    )?;
    writeln!(&mut text)?;

    append_runtime_section(&mut text, state, &git)?;
    append_workspace_section(&mut text, state, &paths)?;
    append_provider_section(&mut text, state, providers, auth_store)?;
    append_resource_section(&mut text, state, resources, &tool_registry)?;
    append_dependency_section(&mut text, &dependencies)?;
    append_warning_section(&mut text, &warnings)?;

    Ok(text.trim_end().to_string())
}

/// Handles `/doctor` by emitting the current diagnostics report into the transcript.
pub(crate) fn run_doctor(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    session_store: &SessionStore,
) -> Result<()> {
    emit_system(
        state,
        session_store,
        render_doctor_report(state, resources, providers, auth_store)?,
    )
}

fn append_runtime_section(
    text: &mut String,
    state: &AppState,
    git: &GitStatusSummary,
) -> Result<()> {
    writeln!(text, "Runtime:")?;
    writeln!(text, "- cwd={}", state.cwd.display())?;
    writeln!(
        text,
        "- selected_provider={}",
        state.current_provider.as_deref().unwrap_or("<unset>")
    )?;
    writeln!(
        text,
        "- selected_model={}",
        state.current_model.as_deref().unwrap_or("<unset>")
    )?;
    writeln!(
        text,
        "- theme={} prompt_color={} effort={} fast_mode={} vim_mode={}",
        state.config.theme, state.prompt_color, state.effort_level, state.fast_mode, state.vim_mode
    )?;
    writeln!(
        text,
        "- plan_mode={} sandbox_mode={} statusline_enabled={}",
        state.plan_mode, state.sandbox_mode, state.statusline_enabled
    )?;
    writeln!(
        text,
        "- remote_name={} remote_env={} remote_status={}",
        display_optional(state.remote_name.as_deref()),
        display_optional(state.remote_environment.as_deref()),
        display_optional(state.remote_session_status.as_deref()),
    )?;
    writeln!(
        text,
        "- transcript_messages={} working_dirs={} files_in_context={} recorded_tasks={}",
        state.transcript.len(),
        state.working_dirs.len(),
        state.claude_read_state.len(),
        state.tasks().len()
    )?;
    match (&git.root, &git.branch, git.dirty, &git.error) {
        (Some(root), branch, Some(dirty), _) => {
            writeln!(
                text,
                "- git_root={} branch={} dirty={}",
                root.display(),
                branch.as_deref().unwrap_or("<unknown>"),
                dirty
            )?;
        }
        (_, _, _, Some(error)) => {
            writeln!(text, "- git_status={error}")?;
        }
        _ => {
            writeln!(text, "- git_status=not a git worktree")?;
        }
    }
    writeln!(text)?;
    Ok(())
}

fn append_workspace_section(
    text: &mut String,
    state: &AppState,
    paths: &ConfigPaths,
) -> Result<()> {
    let permissions_path = paths.workspace_config_dir.join("permissions.toml");
    let acl_path = crate::permissions::acl::acl_path(&state.cwd);
    let auth_path = paths.user_config_dir.join("auth.json");
    let status_line_command = state
        .config
        .ui
        .status_line
        .as_ref()
        .map(|status_line| status_line.command.as_str())
        .unwrap_or("<unset>");
    let status_line_padding = state
        .config
        .ui
        .status_line
        .as_ref()
        .map(|status_line| status_line.padding.to_string())
        .unwrap_or_else(|| "<unset>".to_string());

    writeln!(text, "Workspace:")?;
    writeln!(text, "- workspace_root={}", paths.workspace_root.display())?;
    writeln!(
        text,
        "- workspace_config_dir={} exists={}",
        paths.workspace_config_dir.display(),
        paths.workspace_config_dir.exists()
    )?;
    writeln!(
        text,
        "- user_config_dir={} exists={}",
        paths.user_config_dir.display(),
        paths.user_config_dir.exists()
    )?;
    writeln!(
        text,
        "- builtin_resources_dir={} exists={}",
        paths.builtin_resources_dir.display(),
        paths.builtin_resources_dir.exists()
    )?;
    writeln!(
        text,
        "- permissions_file={} exists={}",
        permissions_path.display(),
        permissions_path.exists()
    )?;
    writeln!(
        text,
        "- permission_acl={} exists={}",
        acl_path.display(),
        acl_path.exists()
    )?;
    writeln!(
        text,
        "- auth_store={} exists={}",
        auth_path.display(),
        auth_path.exists()
    )?;
    writeln!(
        text,
        "- no_alt_screen={} tmux_golden_mode={} status_line_command={} status_line_padding={}",
        state.config.ui.no_alt_screen,
        state.config.ui.tmux_golden_mode,
        status_line_command,
        status_line_padding,
    )?;
    writeln!(
        text,
        "- openai_base_url={} headers={} query_params={}",
        display_optional(state.config.openai_base_url.as_deref()),
        state.config.openai_headers.len(),
        state.config.openai_query_params.len()
    )?;
    writeln!(text)?;
    Ok(())
}

fn append_provider_section(
    text: &mut String,
    state: &AppState,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> Result<()> {
    writeln!(text, "Providers:")?;
    if providers.provider_entries().count() == 0 {
        writeln!(text, "- no providers are registered")?;
        writeln!(text)?;
        return Ok(());
    }
    for entry in providers.provider_entries() {
        let provider = &entry.descriptor;
        writeln!(
            text,
            "- {} [{}] auth={} models={} discovery={} api={}",
            provider.id,
            provider_source_label(entry.source.kind.clone()),
            provider_auth_label(provider, auth_store),
            provider.models.len(),
            if provider.discovery.is_some() {
                "configured"
            } else {
                "static"
            },
            provider.default_api
        )?;
        writeln!(text, "  base_url={}", provider.base_url)?;
        if !provider.headers.is_empty() || !provider.query_params.is_empty() {
            writeln!(
                text,
                "  static_headers={} static_query_params={}",
                provider.headers.len(),
                provider.query_params.len()
            )?;
        }
        if let Some(model) = state
            .current_model
            .as_deref()
            .filter(|selector| selector.starts_with(&format!("{}/", provider.id)))
        {
            writeln!(text, "  active_selection={model}")?;
        }
        if let Some(credential) = auth_store.get(&provider.id) {
            append_provider_credential_details(text, credential)?;
        }
    }
    writeln!(text)?;
    Ok(())
}

fn append_provider_credential_details(
    text: &mut String,
    credential: &StoredCredential,
) -> Result<()> {
    match credential {
        StoredCredential::ApiKey { .. } => {}
        StoredCredential::OAuth(credential) => {
            writeln!(
                text,
                "  oauth_email={} account_id={} plan={}",
                display_optional(credential.email.as_deref()),
                display_optional(credential.account_id.as_deref()),
                display_optional(credential.plan_type.as_deref())
            )?;
        }
    }
    Ok(())
}

fn append_resource_section(
    text: &mut String,
    state: &AppState,
    resources: &LoadedResources,
    tool_registry: &ToolRegistry,
) -> Result<()> {
    writeln!(text, "Resources:")?;
    writeln!(
        text,
        "- prompts={} tools={} internal_tools={} executable_tools={} internal_executable_tools={} skills={} plugins={} mcp_servers={} ides={} hooks={}",
        resources.prompts.len(),
        resources.tools.len(),
        resources.internal_tools.len(),
        tool_registry.tools().count(),
        tool_registry.internal_tools().count(),
        resources.skills.len(),
        resources.plugins.len(),
        resources.mcp_servers.len(),
        resources.ides.len(),
        resources.hooks.len(),
    )?;
    writeln!(
        text,
        "- providers={} agents_in_workspace={}",
        resources.providers.len(),
        crate::load_agent_catalog(&state.cwd, state.current_model.as_deref())
            .map(|agents| agents.len())
            .unwrap_or(0)
    )?;
    if resources.diagnostics.is_empty() {
        writeln!(text, "- resource_diagnostics=none")?;
    } else {
        writeln!(
            text,
            "- resource_diagnostics={}",
            resources.diagnostics.len()
        )?;
        for diagnostic in resources.diagnostics.iter().take(8) {
            writeln!(text, "  - {diagnostic}")?;
        }
        if resources.diagnostics.len() > 8 {
            writeln!(
                text,
                "  - ... {} more diagnostics",
                resources.diagnostics.len() - 8
            )?;
        }
    }
    writeln!(text)?;
    Ok(())
}

fn append_dependency_section(text: &mut String, dependencies: &[DependencyStatus]) -> Result<()> {
    let editor = env::var("EDITOR").ok();
    let visual = env::var("VISUAL").ok();
    writeln!(text, "Dependencies:")?;
    for dependency in dependencies {
        writeln!(
            text,
            "- {}={}",
            dependency.name,
            dependency
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<missing>".to_string())
        )?;
    }
    writeln!(text, "- EDITOR={}", display_optional(editor.as_deref()))?;
    writeln!(text, "- VISUAL={}", display_optional(visual.as_deref()))?;
    writeln!(text)?;
    Ok(())
}

fn append_warning_section(text: &mut String, warnings: &[DoctorWarning]) -> Result<()> {
    writeln!(text, "Warnings:")?;
    if warnings.is_empty() {
        writeln!(text, "- none")?;
        return Ok(());
    }
    for warning in warnings {
        writeln!(text, "- {}", warning.summary)?;
        if let Some(detail) = warning.detail.as_deref() {
            writeln!(text, "  {detail}")?;
        }
    }
    Ok(())
}

fn collect_git_status(cwd: &Path) -> GitStatusSummary {
    let root = run_git(cwd, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from);
    let branch = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).ok();
    let dirty = run_git(cwd, &["status", "--short"])
        .ok()
        .map(|output| !output.trim().is_empty());
    let error = if root.is_none() {
        run_git(cwd, &["rev-parse", "--show-toplevel"]).err()
    } else {
        None
    };
    GitStatusSummary {
        root,
        branch,
        dirty,
        error,
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|error| format!("git unavailable: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Err(format!(
                "git {} failed with {}",
                args.join(" "),
                output.status
            ))
        } else {
            Err(stderr)
        }
    }
}

fn collect_dependency_statuses() -> Vec<DependencyStatus> {
    DOCTOR_DEPENDENCIES
        .iter()
        .copied()
        .map(|name| DependencyStatus {
            name,
            path: command_on_path(name),
        })
        .collect()
}

fn command_on_path(command: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let candidates = env::split_paths(&path).flat_map(|dir| {
        #[cfg(windows)]
        {
            let pathext = env::var_os("PATHEXT")
                .unwrap_or_default()
                .to_string_lossy()
                .split(';')
                .filter(|value| !value.is_empty())
                .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
                .collect::<Vec<_>>();
            let mut options = vec![dir.join(command)];
            options.extend(
                pathext
                    .into_iter()
                    .map(|ext| dir.join(format!("{command}.{ext}"))),
            );
            options
        }
        #[cfg(not(windows))]
        {
            vec![dir.join(command)]
        }
    });
    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn collect_doctor_warnings(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    git: &GitStatusSummary,
    dependencies: &[DependencyStatus],
) -> Vec<DoctorWarning> {
    let mut warnings = Vec::new();

    if let Some(provider_id) = state.current_provider.as_deref() {
        match providers.provider(provider_id) {
            Some(provider)
                if provider
                    .auth_modes
                    .iter()
                    .any(|mode| !matches!(mode, AuthMode::SessionIngress))
                    && !auth_store.has_auth(provider_id) =>
            {
                warnings.push(DoctorWarning {
                    summary: format!(
                        "selected provider `{provider_id}` requires stored credentials but none are configured"
                    ),
                    detail: Some(format!(
                        "Run `/login {provider_id}` or `puffer auth login {provider_id}`."
                    )),
                });
            }
            None => warnings.push(DoctorWarning {
                summary: format!("selected provider `{provider_id}` is not registered"),
                detail: Some("Reload resources or select a different provider.".to_string()),
            }),
            _ => {}
        }
    }

    if let Some(model_selector) = state.current_model.as_deref() {
        if providers.resolve_model(model_selector).is_none() {
            warnings.push(DoctorWarning {
                summary: format!("selected model `{model_selector}` is not currently registered"),
                detail: Some("Run `/model refresh` or pick a model from `/model`.".to_string()),
            });
        }
    }

    if !resources.diagnostics.is_empty() {
        warnings.push(DoctorWarning {
            summary: format!(
                "{} resource diagnostic(s) are present",
                resources.diagnostics.len()
            ),
            detail: resources.diagnostics.first().cloned(),
        });
    }

    if git.root.is_none() {
        warnings.push(DoctorWarning {
            summary: "current working directory is not inside a git worktree".to_string(),
            detail: Some("Commands like `/review`, `/diff`, and `/security-review` work best inside a repository.".to_string()),
        });
    }

    for dependency in dependencies {
        if dependency.path.is_some() {
            continue;
        }
        let warning = match dependency.name {
            "rg" => Some(DoctorWarning {
                summary: "ripgrep (`rg`) is not on PATH".to_string(),
                detail: Some(
                    "Repository search will fall back to slower alternatives.".to_string(),
                ),
            }),
            "gh" => Some(DoctorWarning {
                summary: "GitHub CLI (`gh`) is not on PATH".to_string(),
                detail: Some("`/review` and `/pr-comments` expect the GitHub CLI.".to_string()),
            }),
            "qrencode" if state.remote_session_url.is_some() => Some(DoctorWarning {
                summary: "`qrencode` is not on PATH".to_string(),
                detail: Some(
                    "`/session` can still show the remote URL, but not a terminal QR code."
                        .to_string(),
                ),
            }),
            _ => None,
        };
        if let Some(warning) = warning {
            warnings.push(warning);
        }
    }

    warnings.extend(large_memory_file_warnings(&ConfigPaths::discover(
        &state.cwd,
    )));
    warnings
}

fn large_memory_file_warnings(paths: &ConfigPaths) -> Vec<DoctorWarning> {
    let mut warnings = Vec::new();
    for path in [
        paths.workspace_root.join("CLAUDE.md"),
        paths.workspace_config_dir.join("memory.md"),
        paths.user_config_dir.join("memory.md"),
    ] {
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.len() <= LARGE_MEMORY_FILE_THRESHOLD_BYTES {
            continue;
        }
        warnings.push(DoctorWarning {
            summary: format!(
                "large memory file detected at {} ({} bytes)",
                path.display(),
                metadata.len()
            ),
            detail: Some(format!(
                "Large memory files consume context; keep them under about {} bytes when possible.",
                LARGE_MEMORY_FILE_THRESHOLD_BYTES
            )),
        });
    }
    warnings
}

fn provider_auth_label(
    provider: &puffer_provider_registry::ProviderDescriptor,
    auth_store: &AuthStore,
) -> String {
    match auth_store.get(&provider.id) {
        Some(StoredCredential::ApiKey { .. }) => "api-key".to_string(),
        Some(StoredCredential::OAuth(_)) => "oauth".to_string(),
        None if provider.auth_modes.is_empty() => "none".to_string(),
        None => "missing".to_string(),
    }
}

fn provider_source_label(kind: ProviderSourceKind) -> &'static str {
    match kind {
        ProviderSourceKind::Builtin => "builtin",
        ProviderSourceKind::ResourcePack => "resource-pack",
        ProviderSourceKind::UserConfig => "user-config",
        ProviderSourceKind::WorkspaceConfig => "workspace-config",
    }
}

fn display_optional(value: Option<&str>) -> &str {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("<unset>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::PufferConfig;
    use puffer_provider_registry::{ModelDescriptor, OAuthCredential, ProviderDescriptor};
    use puffer_session_store::SessionMetadata;
    use std::collections::BTreeMap;

    fn provider(id: &str, auth_modes: Vec<AuthMode>) -> ProviderDescriptor {
        ProviderDescriptor {
            id: id.to_string(),
            display_name: id.to_ascii_uppercase(),
            base_url: format!("https://{}.example.test", id),
            default_api: if id == "anthropic" {
                "anthropic-messages".to_string()
            } else {
                "openai-responses".to_string()
            },
            auth_modes,
            headers: Default::default(),
            query_params: Default::default(),
            discovery: None,
            models: vec![ModelDescriptor {
                id: if id == "anthropic" {
                    "claude-sonnet-4-5".to_string()
                } else {
                    "gpt-5".to_string()
                },
                display_name: "primary".to_string(),
                provider: id.to_string(),
                api: if id == "anthropic" {
                    "anthropic-messages".to_string()
                } else {
                    "openai-responses".to_string()
                },
                context_window: 200_000,
                max_output_tokens: 8_192,
                supports_reasoning: true,
                compat: None,
                input: vec![puffer_provider_registry::Modality::Text],
                cost: None,
            }],
            chat_completions_path: None,
        }
    }

    fn state(cwd: &Path) -> AppState {
        AppState::new(
            PufferConfig {
                default_provider: Some("openai".to_string()),
                default_model: Some("openai/gpt-5".to_string()),
                openai_base_url: Some("https://proxy.example/v1".to_string()),
                openai_headers: BTreeMap::from([("x-test".to_string(), "1".to_string())]),
                openai_query_params: BTreeMap::from([(
                    "api-version".to_string(),
                    "2025-04-01".to_string(),
                )]),
                ..PufferConfig::default()
            },
            cwd.to_path_buf(),
            SessionMetadata {
                id: uuid::Uuid::nil(),
                display_name: Some("doctor".to_string()),
                generated_title: None,
                cwd: cwd.to_path_buf(),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        )
    }

    #[test]
    fn report_lists_providers_resources_and_sections() {
        let temp = tempfile::tempdir().unwrap();
        let state = state(temp.path());
        let resources = LoadedResources::default();
        let mut providers = ProviderRegistry::new();
        providers.register(provider("anthropic", vec![AuthMode::ApiKey]));
        providers.register(provider("openai", vec![AuthMode::ApiKey, AuthMode::OAuth]));
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("anthropic", "sk-ant");
        auth_store.set_oauth(
            "openai",
            OAuthCredential {
                email: Some("dev@example.com".to_string()),
                plan_type: Some("pro".to_string()),
                ..OAuthCredential::default()
            },
        );

        let report = render_doctor_report(&state, &resources, &providers, &auth_store).unwrap();
        assert!(report.contains("Puffer doctor"));
        assert!(report.contains("Runtime:"));
        assert!(report.contains("Workspace:"));
        assert!(report.contains("Providers:"));
        assert!(report.contains("- anthropic [builtin] auth=api-key"));
        assert!(report.contains("- openai [builtin] auth=oauth"));
        assert!(report.contains("Resources:"));
        assert!(report.contains("Dependencies:"));
        assert!(report.contains("Warnings:"));
    }

    #[test]
    fn report_warns_when_selected_provider_lacks_auth() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = state(temp.path());
        state.current_provider = Some("anthropic".to_string());
        state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
        let resources = LoadedResources::default();
        let mut providers = ProviderRegistry::new();
        providers.register(provider("anthropic", vec![AuthMode::ApiKey]));

        let report =
            render_doctor_report(&state, &resources, &providers, &AuthStore::default()).unwrap();
        assert!(report.contains("selected provider `anthropic` requires stored credentials"));
    }
}
