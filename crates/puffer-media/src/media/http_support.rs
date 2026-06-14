use anyhow::{bail, Context, Result};
use puffer_provider_registry::{
    canonical_provider_id, AuthStore, MediaExecutionDescriptor, ProviderDescriptor,
    StoredCredential,
};
use reqwest::blocking::Client;

/// Controls whether OpenAI media execution can use legacy Codex credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialAliasMode {
    Strict,
    OpenAiCodexAlias,
}

/// Builds an absolute provider media execution URL.
pub(crate) fn provider_execution_url(
    provider: &ProviderDescriptor,
    execution: &MediaExecutionDescriptor,
    label: &str,
) -> Result<reqwest::Url> {
    let base_url = execution.base_url.as_deref().unwrap_or(&provider.base_url);
    let base = format!("{}/", base_url.trim_end_matches('/'));
    let path = execution.path.trim_start_matches('/');
    let mut url = reqwest::Url::parse(&base)
        .and_then(|base| base.join(path))
        .with_context(|| format!("build {label} URL from {} and {}", base_url, execution.path))?;
    if !provider.query_params.is_empty() {
        let mut query = url.query_pairs_mut();
        for (key, value) in &provider.query_params {
            query.append_pair(key, value);
        }
    }
    Ok(url)
}

/// Returns the bearer token for an authenticated provider.
pub(crate) fn bearer_token(
    provider: &ProviderDescriptor,
    auth_store: &AuthStore,
    alias_mode: CredentialAliasMode,
) -> Result<Option<String>> {
    if provider.auth_modes.is_empty() {
        return Ok(None);
    }
    let Some(credential) = provider_credential(provider, auth_store, alias_mode) else {
        bail!(
            "missing credentials configured for provider {}",
            provider.id
        );
    };
    match credential {
        StoredCredential::ApiKey { key } => non_empty_token(key, &provider.id).map(Some),
        StoredCredential::OAuth(credential) => {
            non_empty_token(&credential.access_token, &provider.id).map(Some)
        }
    }
}

/// Collects secret-bearing provider values for error redaction.
pub(crate) fn provider_error_secrets(
    provider: &ProviderDescriptor,
    auth_store: &AuthStore,
    alias_mode: CredentialAliasMode,
) -> Vec<String> {
    let mut secrets = Vec::new();
    if let Some(credential) = provider_credential(provider, auth_store, alias_mode) {
        match credential {
            StoredCredential::ApiKey { key } => secrets.push(key.clone()),
            StoredCredential::OAuth(credential) => {
                secrets.push(credential.access_token.clone());
                secrets.push(credential.refresh_token.clone());
            }
        }
    }
    secrets.extend(provider.headers.values().cloned());
    secrets.extend(provider.query_params.values().cloned());
    secrets
        .into_iter()
        .map(|secret| secret.trim().to_string())
        .filter(|secret| !secret.is_empty())
        .collect()
}

/// Redacts all known provider secrets from an error string.
pub(crate) fn redact_secrets(text: &str, secrets: &[String]) -> String {
    secrets.iter().fold(text.to_string(), |redacted, secret| {
        redacted.replace(secret, "[redacted]")
    })
}

/// Downloads an image URL while rejecting unsafe plain HTTP targets.
pub(crate) fn download_image_url(client: &Client, url: &str, label: &str) -> Result<Vec<u8>> {
    let parsed =
        reqwest::Url::parse(url).with_context(|| format!("{label} URL must be absolute"))?;
    match parsed.scheme() {
        "https" => {}
        "http" if url_host_is_loopback(&parsed) => {}
        other => bail!("unsupported {label} URL scheme `{other}`"),
    }
    let response = client
        .get(parsed)
        .send()
        .with_context(|| format!("download {label}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("download {label} failed with status {}", status.as_u16());
    }
    Ok(response
        .bytes()
        .with_context(|| format!("read {label} bytes"))?
        .to_vec())
}

fn provider_credential<'a>(
    provider: &ProviderDescriptor,
    auth_store: &'a AuthStore,
    alias_mode: CredentialAliasMode,
) -> Option<&'a StoredCredential> {
    let canonical = canonical_provider_id(&provider.id);
    auth_store
        .get(&provider.id)
        .or_else(|| {
            (canonical != provider.id.as_str())
                .then(|| auth_store.get(&canonical))
                .flatten()
        })
        .or_else(|| {
            (alias_mode == CredentialAliasMode::OpenAiCodexAlias && canonical == "openai")
                .then(|| auth_store.get("codex"))
                .flatten()
        })
}

fn non_empty_token(value: &str, provider_id: &str) -> Result<String> {
    let token = value.trim();
    if token.is_empty() {
        bail!("empty credentials configured for provider {provider_id}");
    }
    Ok(token.to_string())
}

fn url_host_is_loopback(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}
