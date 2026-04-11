use anyhow::{Context, Result};
use puffer_config::{ConfigPaths, PufferConfig};
use puffer_provider_registry::{AuthMode, AuthStore, ProviderRegistry, StoredCredential};
use puffer_resources::LoadedResources;
use puffer_session_store::{GitDiffSnapshot, SessionRecord, SessionStore, TranscriptEvent};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::cli_args::DesktopApiCommand;
use crate::desktop_api_types::{
    AuthProviderStatusDto, DiffSummaryDto, FolderGroupDto, ProviderSummaryDto, RepoActionResultDto,
    RepoPullRequestDto, RepoStatusDto, ResourceCountsDto, SessionDetailDto, SessionListItemDto,
    SettingsConfigDto, SettingsSessionSummaryDto, SettingsSnapshotDto, TimelineItemDto,
};

/// Runs one hidden desktop JSON command for SSH-backed desktop integrations.
pub(crate) fn run_desktop_api(
    command: DesktopApiCommand,
    paths: &ConfigPaths,
    config: &PufferConfig,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
) -> Result<()> {
    match command {
        DesktopApiCommand::SessionGroups => {
            let session_store = SessionStore::from_paths(paths)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&list_grouped_sessions(&session_store)?)?
            );
        }
        DesktopApiCommand::SessionDetail { session_id } => {
            let session_store = SessionStore::from_paths(paths)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&load_session_detail(&session_store, &session_id)?)?
            );
        }
        DesktopApiCommand::RepoStatus { session_id } => {
            let session_store = SessionStore::from_paths(paths)?;
            let cwd = load_session_cwd(&session_store, &session_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&repo_status(&session_id, &cwd))?
            );
        }
        DesktopApiCommand::CreatePullRequest {
            session_id,
            title,
            body,
        } => {
            let session_store = SessionStore::from_paths(paths)?;
            let cwd = load_session_cwd(&session_store, &session_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&create_pull_request(&session_id, &cwd, title, body))?
            );
        }
        DesktopApiCommand::MergePullRequest {
            session_id,
            pull_request_number,
            merge_method,
        } => {
            let session_store = SessionStore::from_paths(paths)?;
            let cwd = load_session_cwd(&session_store, &session_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&merge_pull_request(
                    &session_id,
                    &cwd,
                    pull_request_number,
                    merge_method,
                ))?
            );
        }
        DesktopApiCommand::LoginApiKey { provider, api_key } => {
            store_api_key(
                auth_store,
                providers,
                &paths.user_config_dir.join("auth.json"),
                &provider,
                &api_key,
            )?;
            let session_store = SessionStore::from_paths(paths)?;
            let snapshot = load_settings_snapshot(
                paths,
                config,
                resources,
                providers,
                auth_store,
                &session_store,
            )?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
        DesktopApiCommand::StoreCredential {
            provider,
            credential_json,
        } => {
            store_credential(
                auth_store,
                providers,
                &paths.user_config_dir.join("auth.json"),
                &provider,
                &credential_json,
            )?;
            let session_store = SessionStore::from_paths(paths)?;
            let snapshot = load_settings_snapshot(
                paths,
                config,
                resources,
                providers,
                auth_store,
                &session_store,
            )?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
        DesktopApiCommand::Logout { provider } => {
            auth_store.remove(&provider);
            auth_store
                .save(&paths.user_config_dir.join("auth.json"))
                .context("failed to save auth store")?;
            let session_store = SessionStore::from_paths(paths)?;
            let snapshot = load_settings_snapshot(
                paths,
                config,
                resources,
                providers,
                auth_store,
                &session_store,
            )?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
        DesktopApiCommand::SettingsSnapshot => {
            let session_store = SessionStore::from_paths(paths)?;
            let snapshot = load_settings_snapshot(
                paths,
                config,
                resources,
                providers,
                auth_store,
                &session_store,
            )?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
    }
    Ok(())
}

fn list_grouped_sessions(session_store: &SessionStore) -> Result<Vec<FolderGroupDto>> {
    let sessions = session_store.list_sessions()?;
    let mut groups = BTreeMap::<String, Vec<SessionListItemDto>>::new();
    for session in sessions {
        let folder_path = session_group_root(&session.cwd).display().to_string();
        groups
            .entry(folder_path.clone())
            .or_default()
            .push(SessionListItemDto {
                session_id: session.id.to_string(),
                display_name: session.display_name.clone(),
                title: session_title(
                    session.display_name.as_ref(),
                    session.slug.as_ref(),
                    &session.cwd,
                    &session.id.to_string(),
                ),
                cwd: session.cwd.display().to_string(),
                folder_path: folder_path.clone(),
                updated_at_ms: session.updated_at_ms,
                created_at_ms: session.created_at_ms,
                event_count: session.event_count,
                slug: session.slug.clone(),
                tags: session.tags.clone(),
                note: session.note.clone(),
                parent_session_id: session.parent_session_id.map(|value| value.to_string()),
            });
    }
    let mut folders = groups
        .into_iter()
        .map(|(folder_path, mut sessions)| {
            sessions.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
            FolderGroupDto {
                folder_id: folder_path.clone(),
                folder_label: folder_label(Path::new(&folder_path)),
                folder_path: folder_path.clone(),
                session_count: sessions.len(),
                sessions,
            }
        })
        .collect::<Vec<_>>();
    folders.sort_by(|left, right| left.folder_label.cmp(&right.folder_label));
    Ok(folders)
}

fn load_session_detail(session_store: &SessionStore, session_id: &str) -> Result<SessionDetailDto> {
    let session_uuid = Uuid::parse_str(session_id).context("invalid session id")?;
    let record = session_store.load_session(session_uuid)?;
    let folder_path = session_group_root(&record.metadata.cwd)
        .display()
        .to_string();
    let diff_history = diff_history(&record);
    let latest_diff = diff_history.first().cloned();
    let repo_status = repo_status(&record.metadata.id.to_string(), &record.metadata.cwd);
    Ok(SessionDetailDto {
        session_id: record.metadata.id.to_string(),
        display_name: record.metadata.display_name.clone(),
        title: session_title(
            record.metadata.display_name.as_ref(),
            record.metadata.slug.as_ref(),
            &record.metadata.cwd,
            &record.metadata.id.to_string(),
        ),
        cwd: record.metadata.cwd.display().to_string(),
        folder_path,
        updated_at_ms: record.metadata.updated_at_ms,
        created_at_ms: record.metadata.created_at_ms,
        slug: record.metadata.slug.clone(),
        tags: record.metadata.tags.clone(),
        note: record.metadata.note.clone(),
        parent_session_id: record
            .metadata
            .parent_session_id
            .map(|value| value.to_string()),
        timeline: timeline_items(&record),
        latest_diff,
        diff_history,
        repo_status,
    })
}

fn load_session_cwd(session_store: &SessionStore, session_id: &str) -> Result<PathBuf> {
    let session_uuid = Uuid::parse_str(session_id).context("invalid session id")?;
    let record = session_store.load_session(session_uuid)?;
    Ok(record.metadata.cwd)
}

fn load_settings_snapshot(
    paths: &ConfigPaths,
    config: &PufferConfig,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    session_store: &SessionStore,
) -> Result<SettingsSnapshotDto> {
    let sessions = session_store.list_sessions()?;
    let folder_groups = sessions
        .iter()
        .map(|session| {
            session
                .cwd
                .parent()
                .unwrap_or(session.cwd.as_path())
                .to_path_buf()
        })
        .collect::<std::collections::BTreeSet<PathBuf>>()
        .len();
    Ok(SettingsSnapshotDto {
        workspace_root: paths.workspace_root.display().to_string(),
        workspace_config_file: paths.workspace_config_file().display().to_string(),
        user_config_file: paths.user_config_file().display().to_string(),
        auth_store_file: paths
            .user_config_dir
            .join("auth.json")
            .display()
            .to_string(),
        builtin_resources_dir: paths.builtin_resources_dir.display().to_string(),
        config: SettingsConfigDto {
            app_name: config.app_name.clone(),
            default_provider: config.default_provider.clone(),
            default_model: config.default_model.clone(),
            openai_base_url: config.openai_base_url.clone(),
            theme: config.theme.clone(),
            mascot_id: config.mascot.id.clone(),
            mascot_display_name: config.mascot.display_name.clone(),
            mascot_enabled: config.mascot.enabled,
            ui_no_alt_screen: config.ui.no_alt_screen,
            ui_tmux_golden_mode: config.ui.tmux_golden_mode,
        },
        resources: ResourceCountsDto {
            providers: resources.providers.len(),
            tools: resources.tools.len(),
            agents: resources.agents.len(),
            prompts: resources.prompts.len(),
            hooks: resources.hooks.len(),
            skills: resources.skills.len(),
            mascots: resources.mascots.len(),
            plugins: resources.plugins.len(),
            mcp_servers: resources.mcp_servers.len(),
            ides: resources.ides.len(),
        },
        sessions: SettingsSessionSummaryDto {
            total_sessions: sessions.len(),
            folder_groups,
        },
        auth: auth_statuses(auth_store),
        providers: providers.provider_entries().map(provider_summary).collect(),
    })
}

fn auth_statuses(auth_store: &AuthStore) -> Vec<AuthProviderStatusDto> {
    auth_store
        .provider_ids()
        .filter_map(|provider_id| {
            auth_store
                .get(provider_id)
                .map(|credential| match credential {
                    StoredCredential::ApiKey { .. } => AuthProviderStatusDto {
                        provider_id: provider_id.to_string(),
                        kind: "api_key".to_string(),
                        email: None,
                        expires_at_ms: None,
                        scopes: Vec::new(),
                        plan_type: None,
                        organization_name: None,
                    },
                    StoredCredential::OAuth(credential) => AuthProviderStatusDto {
                        provider_id: provider_id.to_string(),
                        kind: "oauth".to_string(),
                        email: credential.email.clone(),
                        expires_at_ms: Some(credential.expires_at_ms),
                        scopes: credential.scopes.clone(),
                        plan_type: credential.plan_type.clone(),
                        organization_name: credential.organization_name.clone(),
                    },
                })
        })
        .collect()
}

fn provider_summary(entry: &puffer_provider_registry::RegisteredProvider) -> ProviderSummaryDto {
    ProviderSummaryDto {
        id: entry.descriptor.id.clone(),
        display_name: entry.descriptor.display_name.clone(),
        base_url: entry.descriptor.base_url.clone(),
        default_api: entry.descriptor.default_api.clone(),
        model_count: entry.descriptor.models.len(),
        auth_modes: entry
            .descriptor
            .auth_modes
            .iter()
            .map(auth_mode_label)
            .collect(),
        source_kind: format!("{:?}", entry.source.kind).to_lowercase(),
        source_path: entry.source.path.clone(),
    }
}

fn auth_mode_label(mode: &AuthMode) -> String {
    match mode {
        AuthMode::ApiKey => "api_key".to_string(),
        AuthMode::OAuth => "oauth".to_string(),
        AuthMode::SessionIngress => "session_ingress".to_string(),
    }
}

fn store_api_key(
    auth_store: &mut AuthStore,
    providers: &ProviderRegistry,
    auth_path: &Path,
    provider_id: &str,
    api_key: &str,
) -> Result<()> {
    let Some(provider) = providers.provider(provider_id) else {
        anyhow::bail!("unknown provider `{provider_id}`");
    };
    if !provider.auth_modes.contains(&AuthMode::ApiKey) {
        anyhow::bail!("provider `{provider_id}` does not support API keys");
    }
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        anyhow::bail!("api key cannot be empty");
    }
    auth_store.set_api_key(provider_id.to_string(), trimmed.to_string());
    auth_store
        .save(auth_path)
        .context("failed to save auth store")
}

fn store_credential(
    auth_store: &mut AuthStore,
    providers: &ProviderRegistry,
    auth_path: &Path,
    provider_id: &str,
    credential_json: &str,
) -> Result<()> {
    let Some(provider) = providers.provider(provider_id) else {
        anyhow::bail!("unknown provider `{provider_id}`");
    };
    let credential: StoredCredential =
        serde_json::from_str(credential_json).context("failed to parse credential payload")?;
    match &credential {
        StoredCredential::ApiKey { key } => {
            if !provider.auth_modes.contains(&AuthMode::ApiKey) {
                anyhow::bail!("provider `{provider_id}` does not support API keys");
            }
            auth_store.set_api_key(provider_id.to_string(), key.clone());
        }
        StoredCredential::OAuth(credential) => {
            if !provider.auth_modes.contains(&AuthMode::OAuth) {
                anyhow::bail!("provider `{provider_id}` does not support OAuth");
            }
            auth_store.set_oauth(provider_id.to_string(), credential.clone());
        }
    }
    auth_store
        .save(auth_path)
        .context("failed to save auth store")
}

fn session_group_root(cwd: &Path) -> PathBuf {
    cwd.parent().unwrap_or(cwd).to_path_buf()
}

fn session_title(
    display_name: Option<&String>,
    slug: Option<&String>,
    cwd: &Path,
    fallback: &str,
) -> String {
    display_name
        .cloned()
        .or_else(|| slug.cloned())
        .or_else(|| {
            cwd.file_name()
                .map(|value| value.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| fallback.to_string())
}

fn folder_label(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

fn diff_history(record: &SessionRecord) -> Vec<DiffSummaryDto> {
    record
        .events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            TranscriptEvent::GitDiffSnapshot { snapshot } => Some(diff_summary(index, snapshot)),
            _ => None,
        })
        .rev()
        .collect()
}

fn diff_summary(index: usize, snapshot: &GitDiffSnapshot) -> DiffSummaryDto {
    DiffSummaryDto {
        id: format!("diff-{index}"),
        source: "session_history".to_string(),
        command_label: snapshot.command.clone(),
        status_text: snapshot.status.clone(),
        unstaged_diffstat: snapshot.unstaged_diffstat.clone(),
        staged_diffstat: snapshot.staged_diffstat.clone(),
        patch: if snapshot.patch.is_empty() {
            snapshot.patch_excerpt.clone()
        } else {
            snapshot.patch.clone()
        },
        patch_excerpt: snapshot.patch_excerpt.clone(),
    }
}

fn timeline_items(record: &SessionRecord) -> Vec<TimelineItemDto> {
    let mut items = Vec::new();
    for (index, event) in record.events.iter().enumerate() {
        match event {
            TranscriptEvent::UserMessage { text } => items.push(TimelineItemDto::UserMessage {
                id: format!("timeline-{index}"),
                text: text.clone(),
            }),
            TranscriptEvent::AssistantMessage { text } => {
                items.push(TimelineItemDto::AssistantMessage {
                    id: format!("timeline-{index}"),
                    text: text.clone(),
                })
            }
            TranscriptEvent::SystemMessage { text } => {
                items.extend(parse_system_message(index, text))
            }
            TranscriptEvent::CommandInvoked { name, args } => {
                items.push(TimelineItemDto::Command {
                    id: format!("timeline-{index}"),
                    command_name: name.clone(),
                    command_args: args.clone(),
                })
            }
            TranscriptEvent::GitDiffSnapshot { snapshot } => {
                items.push(TimelineItemDto::DiffSnapshot {
                    id: format!("timeline-{index}"),
                    snapshot: diff_summary(index, snapshot),
                })
            }
            TranscriptEvent::SessionRenamed { name } => {
                items.push(TimelineItemDto::SystemMessage {
                    id: format!("timeline-{index}"),
                    text: format!("Session renamed to {name}."),
                })
            }
            TranscriptEvent::ToolInvocation {
                call_id: _,
                tool_id,
                input,
                output,
                success,
            } => {
                let status = if *success { "ok" } else { "error" };
                let text = format!("Tool {tool_id} [{status}]\ninput: {input}\n{output}");
                items.extend(parse_system_message(index, &text))
            }
            TranscriptEvent::TranscriptRewritten { .. } | TranscriptEvent::StateSnapshot { .. } => {
            }
        }
    }
    items
}

fn parse_system_message(index: usize, text: &str) -> Vec<TimelineItemDto> {
    if let Some(parsed) = parse_tool_message(text) {
        let summary = summarize_tool_input(&parsed.tool_id, &parsed.input_text);
        let mut items = vec![TimelineItemDto::ToolCall {
            id: format!("timeline-{index}"),
            tool_id: parsed.tool_id.clone(),
            status: parsed.status,
            summary: summary.clone(),
            input_text: parsed.input_text.clone(),
            input_json: parsed.input_json,
            output_text: parsed.output_text.clone(),
        }];
        if let Some((state, reason)) = permission_state(&parsed.output_text) {
            items.push(TimelineItemDto::PermissionDialog {
                id: format!("timeline-{index}-permission"),
                tool_id: parsed.tool_id,
                state: state.to_string(),
                summary,
                reason: reason.to_string(),
                input_text: Some(parsed.input_text),
            });
        }
        return items;
    }
    vec![TimelineItemDto::SystemMessage {
        id: format!("timeline-{index}"),
        text: text.to_string(),
    }]
}

struct ParsedToolMessage {
    tool_id: String,
    status: String,
    input_text: String,
    input_json: Option<Value>,
    output_text: String,
}

fn parse_tool_message(text: &str) -> Option<ParsedToolMessage> {
    let (header, rest) = text.split_once('\n')?;
    let header = header.strip_prefix("Tool ")?;
    let (tool_id, status) = header.rsplit_once(" [")?;
    let status = status.strip_suffix(']')?;
    let input = rest.strip_prefix("input: ")?;
    let (input_text, output_text) = input
        .split_once('\n')
        .map(|(left, right)| (left.to_string(), right.to_string()))
        .unwrap_or_else(|| (input.to_string(), String::new()));
    Some(ParsedToolMessage {
        tool_id: tool_id.to_string(),
        status: status.to_string(),
        input_json: serde_json::from_str(&input_text).ok(),
        input_text,
        output_text,
    })
}

fn permission_state(output_text: &str) -> Option<(&'static str, &str)> {
    let trimmed = output_text.trim();
    if let Some(reason) = trimmed.strip_prefix("Permission required:") {
        return Some(("required", reason.trim()));
    }
    if let Some(reason) = trimmed.strip_prefix("Permission denied:") {
        return Some(("denied", reason.trim()));
    }
    None
}

fn summarize_tool_input(tool_id: &str, input_text: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(input_text).ok()?;
    match tool_id {
        "Bash" | "PowerShell" => parsed
            .get("command")
            .and_then(Value::as_str)
            .map(|value| format!("Command: {value}")),
        "Config" => parsed
            .get("setting")
            .and_then(Value::as_str)
            .map(|value| format!("Setting: {value}")),
        "WebSearch" => parsed
            .get("query")
            .and_then(Value::as_str)
            .map(|value| format!("Query: {value}")),
        "SendMessage" => parsed
            .get("to")
            .and_then(Value::as_str)
            .map(|value| format!("Recipient: {value}")),
        "AskUserQuestion" => parsed
            .get("questions")
            .and_then(Value::as_array)
            .map(|value| format!("Questions: {}", value.len())),
        "Read" | "Edit" | "Write" => parsed
            .get("file_path")
            .or_else(|| parsed.get("path"))
            .and_then(Value::as_str)
            .map(|value| format!("Path: {value}")),
        _ => None,
    }
}

fn repo_status(session_id: &str, cwd: &Path) -> RepoStatusDto {
    let cwd_text = cwd.display().to_string();
    let repo_root = git_output(cwd, &["rev-parse", "--show-toplevel"]).ok();
    let branch = git_output(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).ok();
    let head_sha = git_output(cwd, &["rev-parse", "HEAD"]).ok();
    let status_text = git_output(cwd, &["status", "--short"]).unwrap_or_default();
    let status_lines = status_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let has_gh = command_exists("gh");
    let gh_authenticated = has_gh
        && std::process::Command::new("gh")
            .args(["auth", "status"])
            .current_dir(cwd)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
    let open_pull_request = if has_gh && gh_authenticated {
        repo_root
            .as_ref()
            .map(PathBuf::from)
            .as_deref()
            .and_then(|root| current_pull_request(root).ok().flatten())
    } else {
        None
    };
    let (can_create_pull_request, create_pull_request_reason) = create_pull_request_state(
        repo_root.as_deref(),
        has_gh,
        gh_authenticated,
        &open_pull_request,
    );
    let (can_merge_pull_request, merge_pull_request_reason) = merge_pull_request_state(
        repo_root.as_deref(),
        has_gh,
        gh_authenticated,
        &open_pull_request,
    );
    let mut warnings = Vec::new();
    if repo_root.is_none() {
        warnings.push("Current session is not inside a git repository.".to_string());
    }
    if !has_gh {
        warnings.push("GitHub CLI (`gh`) is not installed.".to_string());
    } else if !gh_authenticated {
        warnings.push("GitHub CLI is not authenticated.".to_string());
    }
    RepoStatusDto {
        session_id: session_id.to_string(),
        cwd: cwd_text,
        repo_root,
        branch,
        head_sha,
        is_clean: status_lines.is_empty(),
        status_lines,
        has_gh,
        gh_authenticated,
        can_create_pull_request,
        can_merge_pull_request,
        create_pull_request_reason,
        merge_pull_request_reason,
        open_pull_request,
        warnings,
    }
}

fn create_pull_request(
    session_id: &str,
    cwd: &Path,
    title: Option<String>,
    body: Option<String>,
) -> RepoActionResultDto {
    let action = "create_pull_request".to_string();
    let current_status = repo_status(session_id, cwd);
    let Some(repo_root) = current_status.repo_root.as_deref().map(PathBuf::from) else {
        return failure_result(
            action,
            "Current session is not inside a git repository.",
            current_status,
        );
    };
    if !current_status.has_gh {
        return failure_result(
            action,
            "GitHub CLI (`gh`) is not installed.",
            current_status,
        );
    }
    if !current_status.gh_authenticated {
        return failure_result(action, "GitHub CLI is not authenticated.", current_status);
    }
    if !current_status.can_create_pull_request {
        let reason = current_status
            .create_pull_request_reason
            .clone()
            .unwrap_or_else(|| "Pull request creation is not currently available.".to_string());
        return failure_result(action, &reason, current_status);
    }
    let mut command = std::process::Command::new("gh");
    command.arg("pr").arg("create");
    if let Some(title) = title {
        command.arg("--title").arg(title);
        command.arg("--body").arg(body.unwrap_or_default());
    } else if let Some(body) = body {
        command.arg("--fill");
        command.arg("--body").arg(body);
    } else {
        command.arg("--fill");
    }
    command.current_dir(&repo_root);
    match command.output() {
        Ok(output) if output.status.success() => {
            let refreshed = repo_status(session_id, cwd);
            RepoActionResultDto {
                ok: true,
                action,
                message: String::from_utf8_lossy(&output.stdout).trim().to_string(),
                pull_request: refreshed.open_pull_request.clone(),
                repo_status: refreshed,
            }
        }
        Ok(output) => {
            let refreshed = repo_status(session_id, cwd);
            RepoActionResultDto {
                ok: false,
                action,
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                pull_request: refreshed.open_pull_request.clone(),
                repo_status: refreshed,
            }
        }
        Err(error) => failure_result(
            action,
            &format!("failed to execute gh pr create: {error}"),
            current_status,
        ),
    }
}

fn merge_pull_request(
    session_id: &str,
    cwd: &Path,
    pull_request_number: Option<u64>,
    merge_method: Option<String>,
) -> RepoActionResultDto {
    let action = "merge_pull_request".to_string();
    let current_status = repo_status(session_id, cwd);
    let Some(repo_root) = current_status.repo_root.as_deref().map(PathBuf::from) else {
        return failure_result(
            action,
            "Current session is not inside a git repository.",
            current_status,
        );
    };
    if !current_status.has_gh {
        return failure_result(
            action,
            "GitHub CLI (`gh`) is not installed.",
            current_status,
        );
    }
    if !current_status.gh_authenticated {
        return failure_result(action, "GitHub CLI is not authenticated.", current_status);
    }
    if !current_status.can_merge_pull_request {
        let reason = current_status
            .merge_pull_request_reason
            .clone()
            .unwrap_or_else(|| "Pull request merge is not currently available.".to_string());
        return failure_result(action, &reason, current_status);
    }
    let method = merge_method.unwrap_or_else(|| "merge".to_string());
    let mut command = std::process::Command::new("gh");
    command.arg("pr").arg("merge");
    if let Some(number) = pull_request_number {
        command.arg(number.to_string());
    }
    match method.as_str() {
        "merge" => {
            command.arg("--merge");
        }
        "squash" => {
            command.arg("--squash");
        }
        "rebase" => {
            command.arg("--rebase");
        }
        other => {
            return failure_result(
                action,
                &format!("Unsupported merge method `{other}`. Use merge, squash, or rebase."),
                current_status,
            );
        }
    }
    command.arg("--delete-branch");
    command.current_dir(&repo_root);
    match command.output() {
        Ok(output) if output.status.success() => {
            let refreshed = repo_status(session_id, cwd);
            RepoActionResultDto {
                ok: true,
                action,
                message: String::from_utf8_lossy(&output.stdout).trim().to_string(),
                pull_request: refreshed.open_pull_request.clone(),
                repo_status: refreshed,
            }
        }
        Ok(output) => {
            let refreshed = repo_status(session_id, cwd);
            RepoActionResultDto {
                ok: false,
                action,
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                pull_request: refreshed.open_pull_request.clone(),
                repo_status: refreshed,
            }
        }
        Err(error) => failure_result(
            action,
            &format!("failed to execute gh pr merge: {error}"),
            current_status,
        ),
    }
}

fn current_pull_request(repo_root: &Path) -> Result<Option<RepoPullRequestDto>> {
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "view",
            "--json",
            "number,title,url,state,isDraft,mergeStateStatus,headRefName,baseRefName",
        ])
        .current_dir(repo_root)
        .output()
        .context("failed to execute gh pr view")?;
    if !output.status.success() {
        return Ok(None);
    }
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("failed to parse gh pr view JSON")?;
    Ok(Some(RepoPullRequestDto {
        number: parsed
            .get("number")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        title: parsed
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Pull Request")
            .to_string(),
        url: parsed
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        state: parsed
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN")
            .to_string(),
        is_draft: parsed
            .get("isDraft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        merge_state_status: parsed
            .get("mergeStateStatus")
            .and_then(Value::as_str)
            .map(str::to_string),
        head_ref_name: parsed
            .get("headRefName")
            .and_then(Value::as_str)
            .map(str::to_string),
        base_ref_name: parsed
            .get("baseRefName")
            .and_then(Value::as_str)
            .map(str::to_string),
    }))
}

fn create_pull_request_state(
    repo_root: Option<&str>,
    has_gh: bool,
    gh_authenticated: bool,
    open_pull_request: &Option<RepoPullRequestDto>,
) -> (bool, Option<String>) {
    if repo_root.is_none() {
        return (
            false,
            Some("Current session is not inside a git repository.".to_string()),
        );
    }
    if !has_gh {
        return (
            false,
            Some("GitHub CLI (`gh`) is not installed.".to_string()),
        );
    }
    if !gh_authenticated {
        return (false, Some("GitHub CLI is not authenticated.".to_string()));
    }
    if open_pull_request.is_some() {
        return (
            false,
            Some("An open pull request already exists for the current branch.".to_string()),
        );
    }
    (true, None)
}

fn merge_pull_request_state(
    repo_root: Option<&str>,
    has_gh: bool,
    gh_authenticated: bool,
    open_pull_request: &Option<RepoPullRequestDto>,
) -> (bool, Option<String>) {
    if repo_root.is_none() {
        return (
            false,
            Some("Current session is not inside a git repository.".to_string()),
        );
    }
    if !has_gh {
        return (
            false,
            Some("GitHub CLI (`gh`) is not installed.".to_string()),
        );
    }
    if !gh_authenticated {
        return (false, Some("GitHub CLI is not authenticated.".to_string()));
    }
    if open_pull_request.is_none() {
        return (
            false,
            Some("No open pull request exists for the current branch.".to_string()),
        );
    }
    (true, None)
}

fn failure_result(
    action: String,
    message: &str,
    repo_status: RepoStatusDto,
) -> RepoActionResultDto {
    RepoActionResultDto {
        ok: false,
        action,
        message: message.to_string(),
        pull_request: repo_status.open_pull_request.clone(),
        repo_status,
    }
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn command_exists(command: &str) -> bool {
    std::process::Command::new(command)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
