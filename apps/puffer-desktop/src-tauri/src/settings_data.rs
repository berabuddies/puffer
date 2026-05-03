use crate::dtos::{
    AuthProviderStatusDto, ProviderSummaryDto, ResourceCountsDto, SettingsConfigDto,
    SettingsSessionSummaryDto, SettingsSnapshotDto,
};
use anyhow::Result;
use indexmap::IndexMap;
use puffer_config::{ensure_workspace_dirs, load_config, ConfigPaths};
use puffer_provider_registry::{AuthMode, AuthStore, ProviderRegistry, StoredCredential};
use puffer_resources::load_resources;
use puffer_session_store::SessionStore;
use std::path::PathBuf;

/// Loads a desktop-facing snapshot of config, auth, provider, resource, and session state.
pub(crate) fn load_settings_snapshot() -> Result<SettingsSnapshotDto> {
    let workspace_root = std::env::current_dir()?;
    let paths = ConfigPaths::discover(&workspace_root);
    ensure_workspace_dirs(&paths)?;
    let config = load_config(&paths)?;
    let auth_path = paths.user_config_dir.join("auth.json");
    let auth_store = AuthStore::load(&auth_path)?;
    let resources = load_resources(&paths, &puffer_runner_local::LocalToolRunner::new())?;

    let mut providers = ProviderRegistry::new();
    for provider in &resources.providers {
        providers.register_with_source(
            provider.value.clone().into_descriptor(),
            provider.source_info.as_provider_source(),
        );
    }
    providers.apply_openai_base_url_override(config.openai_base_url.as_deref());
    if !config.openai_headers.is_empty() {
        providers.set_openai_headers(
            config
                .openai_headers
                .clone()
                .into_iter()
                .collect::<IndexMap<_, _>>(),
        );
    }
    if !config.openai_query_params.is_empty() {
        providers.set_openai_query_params(
            config
                .openai_query_params
                .clone()
                .into_iter()
                .collect::<IndexMap<_, _>>(),
        );
    }
    let _ = providers.discover_and_merge_all(&auth_store);

    let session_store = SessionStore::from_paths(&paths)?;
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
        auth_store_file: auth_path.display().to_string(),
        builtin_resources_dir: paths.builtin_resources_dir.display().to_string(),
        config: SettingsConfigDto {
            app_name: config.app_name,
            default_provider: config.default_provider,
            default_model: config.default_model,
            openai_base_url: config.openai_base_url,
            theme: config.theme,
            mascot_id: config.mascot.id,
            mascot_display_name: config.mascot.display_name,
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
        auth: auth_statuses(&auth_store),
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
