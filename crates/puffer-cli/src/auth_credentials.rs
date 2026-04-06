use anyhow::Result;
use puffer_provider_registry::{AuthStore, OAuthCredential, StoredCredential};
use puffer_transport_anthropic::{
    create_api_key as create_anthropic_api_key, should_use_claude_ai_auth,
    AnthropicOAuthCredentials, ANTHROPIC_API_BASE_URL, ANTHROPIC_CLAUDE_AI_SCOPES,
};

/// Infers the Anthropic redirect URI from a pasted callback URL.
pub(crate) fn inferred_anthropic_redirect_uri(input: &str) -> Option<String> {
    let mut url = url::Url::parse(input.trim()).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

/// Returns the Anthropic refresh scopes to request for a stored credential.
pub(crate) fn anthropic_refresh_scopes(existing: &OAuthCredential) -> Option<Vec<String>> {
    if existing
        .scopes
        .iter()
        .any(|scope| scope == "user:inference")
    {
        None
    } else if existing.scopes.is_empty() {
        Some(
            ANTHROPIC_CLAUDE_AI_SCOPES
                .split_whitespace()
                .map(ToString::to_string)
                .collect(),
        )
    } else {
        Some(existing.scopes.clone())
    }
}

/// Converts OpenAI OAuth credentials into the registry storage shape.
pub(crate) fn to_registry_oauth_credential_openai(
    credential: puffer_provider_openai::OpenAIOAuthCredentials,
) -> OAuthCredential {
    OAuthCredential {
        access_token: credential.access_token,
        refresh_token: credential.refresh_token,
        expires_at_ms: credential.expires_at_ms,
        account_id: credential.account_id,
        organization_id: None,
        email: credential.email,
        plan_type: credential.plan_type,
        rate_limit_tier: None,
        scopes: Vec::new(),
        organization_name: None,
        organization_role: None,
        workspace_role: None,
    }
}

/// Converts Anthropic OAuth credentials into the registry storage shape.
pub(crate) fn to_registry_oauth_credential_anthropic(
    credential: AnthropicOAuthCredentials,
) -> OAuthCredential {
    OAuthCredential {
        access_token: credential.access_token,
        refresh_token: credential.refresh_token,
        expires_at_ms: credential.expires_at_ms,
        account_id: credential.account_uuid,
        organization_id: credential.organization_uuid,
        email: credential.email_address,
        plan_type: credential.plan_type,
        rate_limit_tier: credential.rate_limit_tier,
        scopes: credential.scopes,
        organization_name: credential.organization_name,
        organization_role: credential.organization_role,
        workspace_role: credential.workspace_role,
    }
}

/// Converts registry OAuth credentials back into the Anthropic transport shape.
pub(crate) fn registry_to_anthropic_oauth_credential(
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

/// Stores Anthropic auth as OAuth for subscriber scopes or as API key for Console scopes.
pub(crate) fn store_anthropic_credential(
    auth_store: &mut AuthStore,
    provider: &str,
    credential: AnthropicOAuthCredentials,
) -> Result<()> {
    let stored = store_ready_credential_from_anthropic(credential)?;
    set_stored_credential(auth_store, provider.to_string(), stored);
    Ok(())
}

/// Returns the final stored credential for an Anthropic auth result.
pub(crate) fn store_ready_credential_from_anthropic(
    credential: AnthropicOAuthCredentials,
) -> Result<StoredCredential> {
    if should_use_claude_ai_auth(&credential.scopes) {
        Ok(StoredCredential::OAuth(
            to_registry_oauth_credential_anthropic(credential),
        ))
    } else {
        let api_key = create_anthropic_api_key(ANTHROPIC_API_BASE_URL, &credential.access_token)?;
        Ok(StoredCredential::ApiKey { key: api_key })
    }
}

/// Writes a resolved stored credential into the auth store.
pub(crate) fn set_stored_credential(
    auth_store: &mut AuthStore,
    provider: String,
    credential: StoredCredential,
) {
    match credential {
        StoredCredential::ApiKey { key } => auth_store.set_api_key(provider, key),
        StoredCredential::OAuth(credential) => auth_store.set_oauth(provider, credential),
    }
}
