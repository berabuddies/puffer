use crate::AppState;
use anyhow::{Context, Result};
use fslock::LockFile;
use puffer_config::ConfigPaths;
use puffer_provider_openai::refresh_oauth_token as refresh_openai_oauth_token;
use puffer_provider_registry::{AuthStore, OAuthCredential, ProviderDescriptor, StoredCredential};
use puffer_transport_anthropic::{
    refresh_oauth_token as refresh_anthropic_oauth_token, AnthropicOAuthCredentials,
    ANTHROPIC_API_BASE_URL, ANTHROPIC_CLAUDE_AI_SCOPES,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Loads the freshest available auth store for one provider, refreshing OAuth when needed.
pub(crate) fn load_request_auth_store(
    state: &AppState,
    provider: &ProviderDescriptor,
    fallback: &AuthStore,
) -> Result<AuthStore> {
    let Some(StoredCredential::OAuth(existing)) = fallback.get(&provider.id) else {
        return Ok(fallback.clone());
    };
    if !credential_needs_refresh(existing) {
        return Ok(fallback.clone());
    }

    let auth_path = auth_path_for_state(state);
    if !auth_path.exists() {
        return Ok(fallback.clone());
    }

    let mut lock = lock_file_for_auth_path(&auth_path)?;
    lock.lock()
        .map_err(anyhow::Error::new)
        .context("failed to acquire OAuth refresh lock")?;

    let mut auth_store = AuthStore::load(&auth_path)?;
    let Some(StoredCredential::OAuth(current)) = auth_store.get(&provider.id).cloned() else {
        return Ok(fallback.clone());
    };
    if !credential_needs_refresh(&current) {
        return Ok(auth_store);
    }

    let refreshed = refresh_provider_oauth(provider, &current)?;
    auth_store.set_oauth(provider.id.clone(), refreshed);
    auth_store.save(&auth_path)?;
    AuthStore::load(&auth_path)
}

/// Attempts a one-shot OAuth refresh after a server-side auth failure.
pub(crate) fn recover_from_oauth_failure(
    state: &AppState,
    provider: &ProviderDescriptor,
    fallback: &AuthStore,
    failed_access_token: &str,
) -> Result<AuthStore> {
    let Some(StoredCredential::OAuth(_)) = fallback.get(&provider.id) else {
        return Ok(fallback.clone());
    };

    let auth_path = auth_path_for_state(state);
    if !auth_path.exists() {
        return Ok(fallback.clone());
    }

    let mut lock = lock_file_for_auth_path(&auth_path)?;
    lock.lock()
        .map_err(anyhow::Error::new)
        .context("failed to acquire OAuth recovery lock")?;

    let mut auth_store = AuthStore::load(&auth_path)?;
    let Some(StoredCredential::OAuth(current)) = auth_store.get(&provider.id).cloned() else {
        return Ok(fallback.clone());
    };
    if current.access_token != failed_access_token {
        return Ok(auth_store);
    }

    let refreshed = refresh_provider_oauth(provider, &current)?;
    auth_store.set_oauth(provider.id.clone(), refreshed);
    auth_store.save(&auth_path)?;
    AuthStore::load(&auth_path)
}

fn auth_path_for_state(state: &AppState) -> PathBuf {
    ConfigPaths::discover(&state.cwd)
        .user_config_dir
        .join("auth.json")
}

fn lock_file_for_auth_path(auth_path: &Path) -> Result<LockFile> {
    let lock_path = auth_path.with_extension("oauth-refresh.lock");
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create OAuth refresh lock dir {}",
                parent.display()
            )
        })?;
    }
    LockFile::open(&lock_path)
        .map_err(anyhow::Error::new)
        .with_context(|| format!("failed to open OAuth refresh lock {}", lock_path.display()))
}

fn credential_needs_refresh(credential: &OAuthCredential) -> bool {
    if credential.refresh_token.trim().is_empty() {
        return false;
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;
    credential.expires_at_ms <= now_ms + 5 * 60 * 1000
}

fn refresh_provider_oauth(
    provider: &ProviderDescriptor,
    credential: &OAuthCredential,
) -> Result<OAuthCredential> {
    match provider.default_api.as_str() {
        "anthropic-messages" => {
            let refreshed = refresh_anthropic_oauth_token(
                &credential.refresh_token,
                anthropic_refresh_scopes(credential).as_deref(),
                Some(ANTHROPIC_API_BASE_URL),
                Some(&registry_to_anthropic_oauth_credential(credential)),
            )?;
            Ok(OAuthCredential {
                access_token: refreshed.access_token,
                refresh_token: refreshed.refresh_token,
                expires_at_ms: refreshed.expires_at_ms,
                account_id: refreshed.account_uuid,
                organization_id: refreshed.organization_uuid,
                email: refreshed.email_address,
                plan_type: refreshed.plan_type,
                rate_limit_tier: refreshed.rate_limit_tier,
                scopes: refreshed.scopes,
                organization_name: refreshed.organization_name,
                organization_role: refreshed.organization_role,
                workspace_role: refreshed.workspace_role,
            })
        }
        "openai-responses"
        | "openai-completions"
        | "azure-openai-responses"
        | "openai-codex-responses" => {
            let refreshed = refresh_openai_oauth_token(&credential.refresh_token)?;
            Ok(OAuthCredential {
                access_token: refreshed.access_token,
                refresh_token: refreshed.refresh_token,
                expires_at_ms: refreshed.expires_at_ms,
                account_id: refreshed
                    .account_id
                    .or_else(|| credential.account_id.clone()),
                organization_id: credential.organization_id.clone(),
                email: refreshed.email.or_else(|| credential.email.clone()),
                plan_type: refreshed.plan_type.or_else(|| credential.plan_type.clone()),
                rate_limit_tier: credential.rate_limit_tier.clone(),
                scopes: credential.scopes.clone(),
                organization_name: credential.organization_name.clone(),
                organization_role: credential.organization_role.clone(),
                workspace_role: credential.workspace_role.clone(),
            })
        }
        _ => Ok(credential.clone()),
    }
}

fn anthropic_refresh_scopes(credential: &OAuthCredential) -> Option<Vec<String>> {
    if credential
        .scopes
        .iter()
        .any(|scope| scope == "user:inference")
    {
        None
    } else if credential.scopes.is_empty() {
        Some(
            ANTHROPIC_CLAUDE_AI_SCOPES
                .split_whitespace()
                .map(ToString::to_string)
                .collect(),
        )
    } else {
        Some(credential.scopes.clone())
    }
}

fn registry_to_anthropic_oauth_credential(
    credential: &OAuthCredential,
) -> AnthropicOAuthCredentials {
    AnthropicOAuthCredentials {
        access_token: credential.access_token.clone(),
        refresh_token: credential.refresh_token.clone(),
        expires_at_ms: credential.expires_at_ms,
        scopes: credential.scopes.clone(),
        account_uuid: credential.account_id.clone(),
        email_address: credential.email.clone(),
        organization_uuid: credential.organization_id.clone(),
        plan_type: credential.plan_type.clone(),
        rate_limit_tier: credential.rate_limit_tier.clone(),
        organization_name: credential.organization_name.clone(),
        organization_role: credential.organization_role.clone(),
        workspace_role: credential.workspace_role.clone(),
    }
}
