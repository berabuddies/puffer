use anyhow::{Context, Result};
use puffer_config::{load_config, save_user_config, ConfigPaths, PufferConfig};
use puffer_provider_registry::{
    detect_import_candidates, AuthMode, AuthStore, ExternalImportCandidate, ExternalImportFamily,
    ExternalImportSource, ProviderRegistry, StoredCredential,
};
use puffer_resources::LoadedResources;
use puffer_session_store::{
    GitDiffSnapshot, MessageActor, SessionRecord, SessionStore, SessionSummary, TranscriptEvent,
    TranscriptRewrite,
};
use puffer_workflow::WorkflowStore;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::cli_args::DesktopApiCommand;
use crate::desktop_activity::session_activity_status;
use crate::desktop_api_types::{
    AgentDiffDto, AgentDiffEntryDto, AgentDiffFileDto, AuthProviderStatusDto, DiffSummaryDto,
    DivergenceReportDto, ExternalCredentialDto, FolderGroupDto, ProviderSummaryDto,
    RepoActionResultDto, RepoPullRequestDto, RepoStatusDto, ResourceCountsDto, SessionDetailDto,
    SessionGroupsPageDto, SessionListItemDto, SettingsConfigDto, SettingsSessionSummaryDto,
    SettingsSnapshotDto, TimelineItemDto,
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
            let project_tags = crate::project_metadata::ProjectMetadataStore::from_paths(paths)
                .all()
                .unwrap_or_default()
                .into_iter()
                .map(|(key, meta)| (key, meta.tags))
                .collect::<BTreeMap<String, Vec<String>>>();
            println!(
                "{}",
                serde_json::to_string_pretty(&list_grouped_sessions(
                    &session_store,
                    &project_tags
                )?)?
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
        DesktopApiCommand::WorkflowList => {
            let store = WorkflowStore::new(&paths.workspace_config_dir);
            println!("{}", serde_json::to_string_pretty(&store.snapshot()?)?);
        }
        DesktopApiCommand::WorkflowRunsList { workflow_slug } => {
            let store = WorkflowStore::new(&paths.workspace_config_dir);
            println!(
                "{}",
                serde_json::to_string_pretty(&store.list_runs_for(&workflow_slug)?)?
            );
        }
        DesktopApiCommand::WorkflowRunsShow { idx } => {
            let store = WorkflowStore::new(&paths.workspace_config_dir);
            println!("{}", serde_json::to_string_pretty(&store.get_run(idx)?)?);
        }
    }
    Ok(())
}

pub(crate) fn list_grouped_sessions(
    session_store: &SessionStore,
    project_tags: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<FolderGroupDto>> {
    let sessions = session_store.list_sessions()?;
    group_session_summaries(session_store, project_tags, sessions)
}

pub(crate) fn list_grouped_sessions_page(
    session_store: &SessionStore,
    project_tags: &BTreeMap<String, Vec<String>>,
    offset: usize,
    limit: usize,
) -> Result<SessionGroupsPageDto> {
    let page = session_store.list_sessions_page(offset, limit)?;
    let returned_sessions = page.sessions.len();
    let has_more = page.has_more();
    Ok(SessionGroupsPageDto {
        groups: group_session_summaries(session_store, project_tags, page.sessions)?,
        offset: page.offset,
        limit: page.limit,
        returned_sessions,
        total_sessions: page.total_sessions,
        has_more,
    })
}

fn group_session_summaries(
    session_store: &SessionStore,
    project_tags: &BTreeMap<String, Vec<String>>,
    sessions: Vec<SessionSummary>,
) -> Result<Vec<FolderGroupDto>> {
    let mut groups = BTreeMap::<String, Vec<SessionListItemDto>>::new();
    for session in sessions {
        let record = session_store.load_session(session.id)?;
        let folder_path = session_group_root(&session.cwd).display().to_string();
        groups
            .entry(folder_path.clone())
            .or_default()
            .push(SessionListItemDto {
                session_id: session.id.to_string(),
                display_name: session.display_name.clone(),
                generated_title: session.generated_title.clone(),
                title: session_title(
                    session.display_name.as_ref(),
                    session.generated_title.as_ref(),
                    session.slug.as_ref(),
                    &session.cwd,
                    &session.id.to_string(),
                ),
                cwd: session.cwd.display().to_string(),
                folder_path: folder_path.clone(),
                updated_at_ms: session.updated_at_ms,
                created_at_ms: session.created_at_ms,
                event_count: session.event_count,
                activity_status: session_activity_status(&record.events).to_string(),
                slug: session.slug.clone(),
                tags: session.tags.clone(),
                note: session.note.clone(),
                parent_session_id: session.parent_session_id.map(|value| value.to_string()),
                provider_id: None,
                model_id: None,
            });
    }
    let mut folders = groups
        .into_iter()
        .map(|(folder_path, mut sessions)| {
            sessions.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
            let tags = project_tags.get(&folder_path).cloned().unwrap_or_default();
            FolderGroupDto {
                folder_id: folder_path.clone(),
                folder_label: folder_label(Path::new(&folder_path)),
                folder_path: folder_path.clone(),
                session_count: sessions.len(),
                sessions,
                tags,
            }
        })
        .collect::<Vec<_>>();
    folders.sort_by(|left, right| {
        let left_latest = left
            .sessions
            .iter()
            .map(|session| session.updated_at_ms)
            .max()
            .unwrap_or(0);
        let right_latest = right
            .sessions
            .iter()
            .map(|session| session.updated_at_ms)
            .max()
            .unwrap_or(0);
        right_latest
            .cmp(&left_latest)
            .then_with(|| left.folder_label.cmp(&right.folder_label))
    });
    Ok(folders)
}

pub(crate) fn load_session_detail(
    session_store: &SessionStore,
    session_id: &str,
) -> Result<SessionDetailDto> {
    let session_uuid = Uuid::parse_str(session_id).context("invalid session id")?;
    let record = session_store.load_session(session_uuid)?;
    let folder_path = session_group_root(&record.metadata.cwd)
        .display()
        .to_string();
    let diff_history = diff_history(&record);
    let latest_diff = diff_history.first().cloned();
    let repo_status = deferred_repo_status(&record.metadata.id.to_string(), &record.metadata.cwd);
    let agent_diff = build_agent_diff(&record);
    let divergence = compute_divergence(&agent_diff, latest_diff.as_ref(), &record.metadata.cwd);
    Ok(SessionDetailDto {
        session_id: record.metadata.id.to_string(),
        display_name: record.metadata.display_name.clone(),
        generated_title: record.metadata.generated_title.clone(),
        title: session_title(
            record.metadata.display_name.as_ref(),
            record.metadata.generated_title.as_ref(),
            record.metadata.slug.as_ref(),
            &record.metadata.cwd,
            &record.metadata.id.to_string(),
        ),
        cwd: record.metadata.cwd.display().to_string(),
        folder_path,
        updated_at_ms: record.metadata.updated_at_ms,
        created_at_ms: record.metadata.created_at_ms,
        event_count: record.events.len(),
        activity_status: session_activity_status(&record.events).to_string(),
        slug: record.metadata.slug.clone(),
        tags: record.metadata.tags.clone(),
        note: record.metadata.note.clone(),
        parent_session_id: record
            .metadata
            .parent_session_id
            .map(|value| value.to_string()),
        provider_id: None,
        model_id: None,
        timeline: timeline_items(&record),
        latest_diff,
        diff_history,
        repo_status,
        agent_diff,
        divergence,
    })
}

pub(crate) fn load_session_cwd(session_store: &SessionStore, session_id: &str) -> Result<PathBuf> {
    let session_uuid = Uuid::parse_str(session_id).context("invalid session id")?;
    let record = session_store.load_session(session_uuid)?;
    Ok(record.metadata.cwd)
}

fn deferred_repo_status(session_id: &str, cwd: &Path) -> RepoStatusDto {
    RepoStatusDto {
        session_id: session_id.to_string(),
        cwd: cwd.display().to_string(),
        repo_root: None,
        branch: None,
        head_sha: None,
        is_clean: true,
        status_lines: Vec::new(),
        has_gh: false,
        gh_authenticated: false,
        can_create_pull_request: false,
        can_merge_pull_request: false,
        create_pull_request_reason: Some(
            "Repository status has not been refreshed yet.".to_string(),
        ),
        merge_pull_request_reason: Some(
            "Repository status has not been refreshed yet.".to_string(),
        ),
        open_pull_request: None,
        warnings: Vec::new(),
    }
}

pub(crate) fn load_settings_snapshot(
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
            internal_tools: resources.internal_tools.len(),
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

pub(crate) fn store_api_key(
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

/// Lists importable `.claude` and `.codex` credentials for all compatible providers.
pub(crate) fn list_external_credentials(
    providers: &ProviderRegistry,
) -> Result<Vec<ExternalCredentialDto>> {
    let mut out = Vec::new();
    for entry in providers.provider_entries() {
        let Some(family) = import_family(&entry.descriptor.default_api) else {
            continue;
        };
        let candidates = detect_import_candidates(family).unwrap_or_default();
        for candidate in candidates {
            out.push(ExternalCredentialDto {
                provider_id: entry.descriptor.id.clone(),
                source: source_label(candidate.source).to_string(),
                kind: credential_kind(&candidate.credential).to_string(),
                description: candidate.description.clone(),
                source_path: candidate.source_path.display().to_string(),
            });
        }
    }
    Ok(out)
}

/// Imports a `.claude` or `.codex` credential into Puffer's auth store.
pub(crate) fn import_external_credential(
    paths: &ConfigPaths,
    auth_store: &mut AuthStore,
    providers: &ProviderRegistry,
    auth_path: &Path,
    provider_id: &str,
    source: &str,
) -> Result<()> {
    let provider = providers
        .provider(provider_id)
        .with_context(|| format!("unknown provider `{provider_id}`"))?;
    let family = import_family(&provider.default_api)
        .with_context(|| format!("provider `{provider_id}` has no importable credential family"))?;
    let source_kind = match source {
        "claude" => ExternalImportSource::Claude,
        "codex" => ExternalImportSource::Codex,
        other => anyhow::bail!("unknown import source `{other}`"),
    };
    let candidate = detect_import_candidates(family)?
        .into_iter()
        .find(|candidate| candidate.source == source_kind)
        .with_context(|| {
            format!("no `{source}` credential available on disk for provider `{provider_id}`")
        })?;
    apply_imported_credential(paths, auth_store, provider_id, candidate)?;
    auth_store
        .save(auth_path)
        .context("failed to save auth store")
}

/// Ensures a freshly authenticated provider becomes the default route when needed.
pub(crate) fn ensure_default_routing(
    paths: &ConfigPaths,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    just_authed: &str,
) -> Result<()> {
    let mut config = load_config(paths)?;
    let mut changed = false;

    let default_has_creds = config
        .default_provider
        .as_deref()
        .map(|id| auth_store.get(id).is_some())
        .unwrap_or(false);
    if !default_has_creds {
        config.default_provider = Some(just_authed.to_string());
        changed = true;
    }

    let default_provider_id = config.default_provider.clone().unwrap_or_default();
    let provider_models = providers
        .provider(&default_provider_id)
        .map(|provider| provider.models.clone())
        .unwrap_or_default();
    let default_model_belongs = config
        .default_model
        .as_deref()
        .map(|model_id| provider_models.iter().any(|model| model.id == model_id))
        .unwrap_or(false);
    if !default_model_belongs {
        if let Some(first) = provider_models.first() {
            config.default_model = Some(first.id.clone());
            changed = true;
        }
    }

    if changed {
        save_user_config(paths, &config)?;
    }
    Ok(())
}

pub(crate) fn logout_provider(
    auth_store: &mut AuthStore,
    auth_path: &Path,
    provider_id: &str,
) -> Result<()> {
    auth_store.remove(provider_id);
    auth_store
        .save(auth_path)
        .context("failed to save auth store")
}

fn import_family(default_api: &str) -> Option<ExternalImportFamily> {
    match default_api {
        "anthropic-messages" => Some(ExternalImportFamily::Anthropic),
        "openai-responses"
        | "openai-completions"
        | "openai-codex-responses"
        | "azure-openai-responses" => Some(ExternalImportFamily::OpenAi),
        _ => None,
    }
}

fn source_label(source: ExternalImportSource) -> &'static str {
    match source {
        ExternalImportSource::Claude => "claude",
        ExternalImportSource::Codex => "codex",
    }
}

fn credential_kind(credential: &StoredCredential) -> &'static str {
    match credential {
        StoredCredential::ApiKey { .. } => "api_key",
        StoredCredential::OAuth(_) => "oauth",
    }
}

fn apply_imported_credential(
    paths: &ConfigPaths,
    auth_store: &mut AuthStore,
    provider_id: &str,
    candidate: ExternalImportCandidate,
) -> Result<()> {
    let openai_base_url = candidate.openai_base_url.clone();
    let openai_headers = candidate.openai_headers.clone();
    let openai_query_params = candidate.openai_query_params.clone();
    match candidate.credential {
        StoredCredential::ApiKey { key } => {
            auth_store.set_api_key(provider_id.to_string(), key);
        }
        StoredCredential::OAuth(credential) => {
            auth_store.set_oauth(provider_id.to_string(), credential);
        }
    }
    if provider_id == "openai" {
        let mut config = load_config(paths)?;
        config.openai_base_url = openai_base_url;
        config.openai_headers = openai_headers;
        config.openai_query_params = openai_query_params;
        save_user_config(paths, &config)?;
    }
    Ok(())
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

pub(crate) fn session_group_root(cwd: &Path) -> PathBuf {
    cwd.to_path_buf()
}

fn session_title(
    display_name: Option<&String>,
    generated_title: Option<&String>,
    slug: Option<&String>,
    cwd: &Path,
    fallback: &str,
) -> String {
    display_name
        .cloned()
        .or_else(|| generated_title.cloned())
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

/// Reconstructs the agent's intended edits from the transcript. Walks
/// every successful `ToolInvocation` event for builtin file-editing
/// tools and builds two views: an ordered per-call entry list (so the
/// UI can show edits in sequence) and a per-file rollup keyed by path.
///
/// Tool input is JSON we parse opportunistically — we never error out
/// on a malformed call, since transcripts can outlive schema changes.
/// Failed calls are still recorded (success=false) so the user can see
/// "the agent tried to edit X but the tool call failed".
fn build_agent_diff(record: &SessionRecord) -> AgentDiffDto {
    let mut entries: Vec<AgentDiffEntryDto> = Vec::new();
    let mut by_path: BTreeMap<String, AgentDiffFileDto> = BTreeMap::new();

    for event in record.events.iter() {
        let TranscriptEvent::ToolInvocation {
            call_id,
            tool_id,
            input,
            success,
            ..
        } = event
        else {
            continue;
        };
        let Some(intent) = agent_edit_intent(tool_id, input) else {
            continue;
        };

        let entry = AgentDiffEntryDto {
            call_id: call_id.clone(),
            tool_id: tool_id.clone(),
            kind: intent.kind.to_string(),
            path: intent.path.clone(),
            success: *success,
            summary: intent.summary.clone(),
        };

        // Only successful edits update the per-file rollup — a failed
        // edit didn't change anything, even if the agent tried.
        if *success {
            by_path
                .entry(intent.path.clone())
                .and_modify(|file| {
                    file.edit_count += 1;
                    file.latest_kind = intent.kind.to_string();
                    file.latest_summary = intent.summary.clone();
                })
                .or_insert_with(|| AgentDiffFileDto {
                    path: intent.path.clone(),
                    latest_kind: intent.kind.to_string(),
                    edit_count: 1,
                    latest_summary: intent.summary.clone(),
                });
        }

        entries.push(entry);
    }

    AgentDiffDto {
        files: by_path.into_values().collect(),
        entries,
    }
}

struct AgentEditIntent {
    kind: &'static str,
    path: String,
    summary: String,
}

fn agent_edit_intent(tool_id: &str, raw_input: &str) -> Option<AgentEditIntent> {
    let value: Value = serde_json::from_str(raw_input).ok()?;
    let obj = value.as_object()?;
    match tool_id {
        "write_file" | "Write" => {
            let path = obj
                .get("path")
                .or_else(|| obj.get("file_path"))
                .and_then(Value::as_str)?
                .to_string();
            let contents = obj
                .get("contents")
                .or_else(|| obj.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Some(AgentEditIntent {
                kind: "write",
                path,
                summary: render_write_summary(&contents),
            })
        }
        "replace_in_file" | "edit_file" | "Edit" => {
            let path = obj
                .get("path")
                .or_else(|| obj.get("file_path"))
                .and_then(Value::as_str)?
                .to_string();
            // `replace_in_file` uses old/new; some custom tools mirror
            // the Anthropic-style old_string/new_string. Accept both.
            let old = obj
                .get("old")
                .or_else(|| obj.get("old_string"))
                .or_else(|| obj.get("oldText"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let new_text = obj
                .get("new")
                .or_else(|| obj.get("new_string"))
                .or_else(|| obj.get("newText"))
                .and_then(Value::as_str)
                .unwrap_or("");
            Some(AgentEditIntent {
                kind: "replace",
                path,
                summary: render_replace_summary(old, new_text),
            })
        }
        "move_path" => {
            let from = obj.get("from").and_then(Value::as_str)?.to_string();
            let to = obj.get("to").and_then(Value::as_str)?.to_string();
            Some(AgentEditIntent {
                kind: "move",
                path: to.clone(),
                summary: format!("renamed {from} → {to}"),
            })
        }
        "remove_path" => {
            let path = obj.get("path").and_then(Value::as_str)?.to_string();
            Some(AgentEditIntent {
                kind: "remove",
                path: path.clone(),
                summary: format!("removed {path}"),
            })
        }
        _ => None,
    }
}

fn render_write_summary(contents: &str) -> String {
    // Show a unified-diff-ish prefix with `+` so it's obvious the agent
    // is writing the whole file. Cap at ~80 lines so a giant generated
    // file doesn't blow up the response.
    const MAX_LINES: usize = 80;
    let mut out = String::new();
    let mut lines = 0;
    for line in contents.lines() {
        if lines >= MAX_LINES {
            out.push_str("... (truncated)\n");
            break;
        }
        out.push('+');
        out.push_str(line);
        out.push('\n');
        lines += 1;
    }
    out
}

fn render_replace_summary(old: &str, new_text: &str) -> String {
    // Best-effort unified-diff rendering: line-by-line `-`/`+`. Like
    // git's --no-context output but with the safety cap from above.
    const MAX_LINES: usize = 200;
    let mut out = String::new();
    let mut lines = 0;
    for line in old.lines() {
        if lines >= MAX_LINES {
            out.push_str("... (truncated)\n");
            break;
        }
        out.push('-');
        out.push_str(line);
        out.push('\n');
        lines += 1;
    }
    for line in new_text.lines() {
        if lines >= MAX_LINES {
            out.push_str("... (truncated)\n");
            break;
        }
        out.push('+');
        out.push_str(line);
        out.push('\n');
        lines += 1;
    }
    out
}

/// Compares the agent's claimed file set with the git diff snapshot.
/// `git_only` files surface hand-edits, post-tool hook rewrites, or
/// build artifacts. `agent_only` files surface edits the agent made
/// that were rolled back, never landed, or were committed (and so are
/// no longer in the working-tree diff).
fn compute_divergence(
    agent_diff: &AgentDiffDto,
    latest_git_diff: Option<&DiffSummaryDto>,
    cwd: &Path,
) -> DivergenceReportDto {
    let agent_paths: BTreeSet<String> = agent_diff.files.iter().map(|f| f.path.clone()).collect();
    let git_paths = latest_git_diff
        .map(|d| extract_paths_from_patch(&d.patch, cwd))
        .unwrap_or_default();

    // Normalize agent paths through the same canonicalizer git uses
    // (relative to cwd) so a tool-call that took an absolute path
    // doesn't look "agent-only" just because git emits relative paths.
    let agent_relative: BTreeSet<String> = agent_paths
        .iter()
        .map(|p| relativize_path(p, cwd))
        .collect();

    let agent_only: Vec<String> = agent_relative.difference(&git_paths).cloned().collect();
    let git_only: Vec<String> = git_paths.difference(&agent_relative).cloned().collect();

    DivergenceReportDto {
        agent_total: agent_relative.len(),
        git_total: git_paths.len(),
        agent_only,
        git_only,
    }
}

fn relativize_path(path: &str, cwd: &Path) -> String {
    let trimmed = path.trim();
    if let Ok(stripped) = Path::new(trimmed).strip_prefix(cwd) {
        return stripped.display().to_string();
    }
    trimmed.to_string()
}

/// Pulls file paths out of a unified diff patch. Recognizes the `diff
/// --git a/<path> b/<path>` header git emits — the `b/` side is the
/// post-image, which is what we compare against (matches what's on
/// disk now). Renames produce two header paths; we keep the new one.
fn extract_paths_from_patch(patch: &str, _cwd: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in patch.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // "a/<path> b/<path>" — take the b/<path> side.
            if let Some(b_index) = rest.find(" b/") {
                let after = &rest[b_index + 3..];
                let path = after.split_whitespace().next().unwrap_or("").to_string();
                if !path.is_empty() {
                    out.insert(path);
                }
            }
        }
    }
    out
}

fn timeline_items(record: &SessionRecord) -> Vec<TimelineItemDto> {
    let mut items = Vec::new();
    let mut pending_assistant = None;
    for (index, event) in record.events.iter().enumerate() {
        match event {
            TranscriptEvent::UserMessage { text, actor } => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                items.push(TimelineItemDto::UserMessage {
                    id: format!("timeline-{index}"),
                    text: text.clone(),
                    actor: actor.clone(),
                });
            }
            TranscriptEvent::AssistantMessage { text, actor } => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                pending_assistant = Some(TimelineItemDto::AssistantMessage {
                    id: format!("timeline-{index}"),
                    text: text.clone(),
                    actor: actor.clone(),
                });
            }
            TranscriptEvent::SystemMessage { text, actor } => {
                let parsed = parse_system_message(index, text, actor.clone());
                if parse_tool_message(text).is_none() {
                    flush_pending_assistant(&mut items, &mut pending_assistant);
                }
                items.extend(parsed);
            }
            TranscriptEvent::CommandInvoked { name, args, actor } => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                items.push(TimelineItemDto::Command {
                    id: format!("timeline-{index}"),
                    command_name: name.clone(),
                    command_args: args.clone(),
                    actor: actor.clone(),
                })
            }
            TranscriptEvent::GitDiffSnapshot { snapshot } => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                items.push(TimelineItemDto::DiffSnapshot {
                    id: format!("timeline-{index}"),
                    snapshot: diff_summary(index, snapshot),
                })
            }
            TranscriptEvent::SessionRenamed { name } => {
                flush_pending_assistant(&mut items, &mut pending_assistant);
                items.push(TimelineItemDto::SystemMessage {
                    id: format!("timeline-{index}"),
                    text: format!("Session renamed to {name}."),
                    actor: None,
                })
            }
            TranscriptEvent::ToolInvocation {
                call_id,
                tool_id,
                input,
                output,
                success,
                actor,
                subject,
                metadata,
            } => {
                let status = if *success { "ok" } else { "error" };
                let summary = summarize_tool_input(tool_id, input);
                items.push(TimelineItemDto::ToolCall {
                    id: format!("timeline-{index}-{call_id}"),
                    tool_id: tool_id.clone(),
                    status: status.to_string(),
                    summary: summary.clone(),
                    input_text: input.clone(),
                    input_json: serde_json::from_str(input).ok(),
                    output_text: output.clone(),
                    metadata: metadata.clone(),
                    actor: actor.clone(),
                    subject: subject.clone(),
                });
                if let Some(text) = lambda_gate_timeline_text(metadata, tool_id) {
                    items.push(TimelineItemDto::SystemMessage {
                        id: format!("timeline-{index}-{call_id}-lambda-gate"),
                        text,
                        actor: actor.clone(),
                    });
                }
                if let Some((state, reason)) = permission_state(output) {
                    items.push(TimelineItemDto::PermissionDialog {
                        id: format!("timeline-{index}-{call_id}-permission"),
                        tool_id: tool_id.clone(),
                        state: state.to_string(),
                        summary,
                        reason: reason.to_string(),
                        input_text: Some(input.clone()),
                        actor: actor.clone(),
                    });
                }
            }
            TranscriptEvent::TranscriptRewritten { rewrite } => {
                apply_timeline_rewrite(&mut items, &mut pending_assistant, rewrite);
            }
            TranscriptEvent::StateSnapshot { .. } => {}
        }
    }
    flush_pending_assistant(&mut items, &mut pending_assistant);
    items
}

fn apply_timeline_rewrite(
    items: &mut Vec<TimelineItemDto>,
    pending_assistant: &mut Option<TimelineItemDto>,
    rewrite: &TranscriptRewrite,
) {
    match rewrite {
        TranscriptRewrite::Clear => {
            pending_assistant.take();
            items.clear();
        }
        TranscriptRewrite::PopLast { count } => {
            for _ in 0..*count {
                if pending_assistant.take().is_some() {
                    continue;
                }
                if items.pop().is_none() {
                    break;
                }
            }
        }
    }
}

fn flush_pending_assistant(
    items: &mut Vec<TimelineItemDto>,
    pending_assistant: &mut Option<TimelineItemDto>,
) {
    if let Some(item) = pending_assistant.take() {
        items.push(item);
    }
}

fn lambda_gate_timeline_text(metadata: &Option<Value>, tool_id: &str) -> Option<String> {
    let lambda = metadata.as_ref()?.get("lambda_skill")?;
    let event = lambda.get("event").and_then(Value::as_str)?;
    let host_tool = lambda.get("host_tool").and_then(Value::as_str);
    let concrete_tool = lambda.get("concrete_tool").and_then(Value::as_str);
    let host_tool_label = host_tool.unwrap_or("host call");
    let concrete_tool_label = concrete_tool.unwrap_or(tool_id);
    let mut lines = vec!["Verified Skill Gate".to_string(), format!("event: {event}")];
    if event == "gate_rejected" {
        lines.push(
            "check: Compared the attempted tool call against the active LambdaHostCall gate."
                .to_string(),
        );
        if let Some(reason) = lambda.get("reason").and_then(Value::as_str) {
            lines.push(format!("reason: {reason}"));
        }
        if let Some(retry) = lambda.get("retry_tool").and_then(Value::as_str) {
            lines.push(format!("retry_tool: {retry}"));
        }
        lines.push(
            "confirmation: Puffer rejected this call before committing the Lambda gate. Retry by opening LambdaHostCall with the formal host tool, host args, concrete tool, and exact concrete input."
                .to_string(),
        );
        return Some(lines.join("\n"));
    }
    if event == "host_call_committed" {
        lines.push(format!(
            "check: Confirmed the concrete {concrete_tool_label} call matched the pending LambdaHostCall bridge for formal host tool {host_tool_label}."
        ));
    } else {
        lines.push(format!(
            "check: Verified LambdaHostCall may bind formal host tool {host_tool_label} to concrete tool {concrete_tool_label}, and recorded the exact concrete input that must run next."
        ));
    }
    lines.push(format!("host_tool: {host_tool_label}"));
    if let Some(args) = lambda.get("host_args").map(compact_json_text) {
        lines.push(format!("host_args: {args}"));
    }
    lines.push(format!("concrete_tool: {concrete_tool_label}"));
    if let Some(input) = lambda.get("concrete_input").map(compact_json_text) {
        lines.push(format!("concrete_input: {input}"));
    }
    if let Some(facts) = lambda.get("registered_facts").map(compact_json_text) {
        lines.push(format!("registered_facts: {facts}"));
    }
    if event == "host_call_committed" {
        lines.push(
            "confirmation: Puffer observed the declared concrete tool succeed, then committed the Lambda gate and any registered facts."
                .to_string(),
        );
    } else {
        lines.push(
            "confirmation: Compare concrete_tool with the next activity row's tool name and concrete_input with that tool's input. Puffer only allows the next concrete call when both match exactly."
                .to_string(),
        );
    }
    Some(lines.join("\n"))
}

fn compact_json_text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn parse_system_message(
    index: usize,
    text: &str,
    actor: Option<MessageActor>,
) -> Vec<TimelineItemDto> {
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
            metadata: None,
            actor: actor.clone(),
            subject: None,
        }];
        if let Some((state, reason)) = permission_state(&parsed.output_text) {
            items.push(TimelineItemDto::PermissionDialog {
                id: format!("timeline-{index}-permission"),
                tool_id: parsed.tool_id,
                state: state.to_string(),
                summary,
                reason: reason.to_string(),
                input_text: Some(parsed.input_text),
                actor: actor.clone(),
            });
        }
        return items;
    }
    vec![TimelineItemDto::SystemMessage {
        id: format!("timeline-{index}"),
        text: text.to_string(),
        actor,
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

pub(crate) fn repo_status(session_id: &str, cwd: &Path) -> RepoStatusDto {
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

pub(crate) fn create_pull_request(
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

pub(crate) fn merge_pull_request(
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

#[cfg(test)]
mod tests {
    use super::{timeline_items, TimelineItemDto};
    use puffer_session_store::{
        SessionMetadata, SessionRecord, TranscriptEvent, TranscriptRewrite,
    };
    use serde_json::json;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn record(events: Vec<TranscriptEvent>) -> SessionRecord {
        SessionRecord {
            metadata: SessionMetadata {
                id: Uuid::nil(),
                display_name: None,
                generated_title: None,
                cwd: PathBuf::from("/tmp"),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
            events,
        }
    }

    fn item_kind(item: &TimelineItemDto) -> &'static str {
        match item {
            TimelineItemDto::UserMessage { .. } => "user",
            TimelineItemDto::AssistantMessage { .. } => "assistant",
            TimelineItemDto::SystemMessage { .. } => "system",
            TimelineItemDto::Command { .. } => "command",
            TimelineItemDto::ToolCall { .. } => "tool",
            TimelineItemDto::PermissionDialog { .. } => "permission",
            TimelineItemDto::DiffSnapshot { .. } => "diff",
        }
    }

    #[test]
    fn timeline_surfaces_lambda_gate_metadata_as_system_event() {
        let items = timeline_items(&record(vec![TranscriptEvent::ToolInvocation {
            call_id: "call-1".to_string(),
            tool_id: "LambdaHostCall".to_string(),
            input: "{}".to_string(),
            output: "admitted".to_string(),
            success: true,
            metadata: Some(json!({
                "lambda_skill": {
                    "event": "host_call_admitted",
                    "host_tool": "gh_pr_view",
                    "host_args": {"number": 42},
                    "concrete_tool": "Bash",
                    "concrete_input": {"command": "gh pr view 42"}
                }
            })),
            actor: None,
            subject: None,
        }]));

        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], TimelineItemDto::ToolCall { .. }));
        match &items[1] {
            TimelineItemDto::SystemMessage { text, .. } => {
                assert!(text.contains("Verified Skill Gate"));
                assert!(text.contains("event: host_call_admitted"));
                assert!(text.contains("host_tool: gh_pr_view"));
                assert!(text.contains(r#"host_args: {"number":42}"#));
                assert!(text.contains("concrete_tool: Bash"));
                assert!(text.contains(r#"concrete_input: {"command":"gh pr view 42"}"#));
                assert!(
                    text.contains("Compare concrete_tool with the next activity row's tool name")
                );
            }
            other => panic!("expected lambda gate system event, got {other:?}"),
        }
    }

    #[test]
    fn timeline_places_persisted_tool_invocations_before_assistant_text() {
        let items = timeline_items(&record(vec![
            TranscriptEvent::UserMessage {
                text: "inspect this".to_string(),
                actor: None,
            },
            TranscriptEvent::AssistantMessage {
                text: "Here is what I found.".to_string(),
                actor: None,
            },
            TranscriptEvent::ToolInvocation {
                call_id: "call-1".to_string(),
                tool_id: "Read".to_string(),
                input: r#"{"path":"Cargo.toml"}"#.to_string(),
                output: "contents".to_string(),
                success: true,
                metadata: None,
                actor: None,
                subject: None,
            },
        ]));

        let kinds = items.iter().map(item_kind).collect::<Vec<_>>();
        assert_eq!(kinds, vec!["user", "tool", "assistant"]);
    }

    #[test]
    fn timeline_keeps_new_tool_before_assistant_order() {
        let items = timeline_items(&record(vec![
            TranscriptEvent::UserMessage {
                text: "inspect this".to_string(),
                actor: None,
            },
            TranscriptEvent::ToolInvocation {
                call_id: "call-1".to_string(),
                tool_id: "Read".to_string(),
                input: r#"{"path":"Cargo.toml"}"#.to_string(),
                output: "contents".to_string(),
                success: true,
                metadata: None,
                actor: None,
                subject: None,
            },
            TranscriptEvent::AssistantMessage {
                text: "Here is what I found.".to_string(),
                actor: None,
            },
        ]));

        let kinds = items.iter().map(item_kind).collect::<Vec<_>>();
        assert_eq!(kinds, vec!["user", "tool", "assistant"]);
    }

    #[test]
    fn timeline_clear_discards_prior_messages() {
        let items = timeline_items(&record(vec![
            TranscriptEvent::UserMessage {
                text: "old prompt".to_string(),
                actor: None,
            },
            TranscriptEvent::AssistantMessage {
                text: "old answer".to_string(),
                actor: None,
            },
            TranscriptEvent::TranscriptRewritten {
                rewrite: TranscriptRewrite::Clear,
            },
            TranscriptEvent::SystemMessage {
                text: "Transcript cleared.".to_string(),
                actor: None,
            },
        ]));

        let kinds = items.iter().map(item_kind).collect::<Vec<_>>();
        assert_eq!(kinds, vec!["system"]);
        assert!(matches!(
            &items[0],
            TimelineItemDto::SystemMessage { text, .. } if text == "Transcript cleared."
        ));
    }

    #[test]
    fn timeline_pop_discards_latest_visible_message() {
        let items = timeline_items(&record(vec![
            TranscriptEvent::UserMessage {
                text: "keep prompt".to_string(),
                actor: None,
            },
            TranscriptEvent::AssistantMessage {
                text: "remove answer".to_string(),
                actor: None,
            },
            TranscriptEvent::TranscriptRewritten {
                rewrite: TranscriptRewrite::PopLast { count: 1 },
            },
            TranscriptEvent::SystemMessage {
                text: "Removed the latest rendered transcript item.".to_string(),
                actor: None,
            },
        ]));

        let kinds = items.iter().map(item_kind).collect::<Vec<_>>();
        assert_eq!(kinds, vec!["user", "system"]);
        assert!(matches!(
            &items[0],
            TimelineItemDto::UserMessage { text, .. } if text == "keep prompt"
        ));
    }
}
