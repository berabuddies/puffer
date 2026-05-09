use crate::auth::{AuthStore, StoredCredential};
use crate::model::{
    ModelDescriptor, ModelDiscoveryConfig, ModelDiscoveryFormat, ProviderDescriptor,
};
use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use reqwest::blocking::RequestBuilder;
use serde_json::Value;

const OPENAI_CODEX_ORIGINATOR: &str = "codex_cli_rs";
const OPENAI_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const OPENAI_CODEX_COMPAT_VERSION: &str = "0.125.0";

/// Performs runtime provider model-discovery requests.
#[derive(Debug, Clone)]
pub struct ModelDiscoveryClient {
    client: Client,
}

impl Default for ModelDiscoveryClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelDiscoveryClient {
    /// Creates a discovery client with sensible timeouts for startup discovery.
    pub fn new() -> Self {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_default();
        Self { client }
    }

    /// Fetches and parses discovery results for a provider descriptor.
    pub fn discover_models(
        &self,
        provider: &ProviderDescriptor,
        auth_store: &AuthStore,
    ) -> Result<Vec<ModelDescriptor>> {
        let Some(discovery) = provider.discovery.as_ref() else {
            return Ok(Vec::new());
        };
        let discovery_mode = discovery_mode(provider, auth_store);
        let url = discovery_url(provider, discovery, &discovery_mode);
        let mut request = self.client.get(&url);
        request = apply_discovery_headers(request, provider, discovery, &discovery_mode);
        request = apply_discovery_auth(request, provider, auth_store, &discovery_mode);
        let response = request
            .send()
            .with_context(|| format!("failed to fetch models from {url}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!(
                "model discovery for {} failed with {status}",
                provider.id
            ));
        }
        let payload = response
            .json::<Value>()
            .with_context(|| format!("failed to parse discovery response from {url}"))?;
        match discovery_mode {
            DiscoveryMode::Standard => parse_discovered_models(provider, discovery, &payload),
            DiscoveryMode::OpenAiOAuthCodex => {
                parse_codex_discovered_models(provider, discovery, &payload)
            }
        }
    }
}

/// Merges discovered models into an existing model list without replacing existing ids.
pub fn merge_discovered_models(
    existing: &mut Vec<ModelDescriptor>,
    discovered: Vec<ModelDescriptor>,
) {
    for model in discovered {
        if existing.iter().any(|current| current.id == model.id) {
            continue;
        }
        existing.push(model);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryMode {
    Standard,
    OpenAiOAuthCodex,
}

fn discovery_mode(provider: &ProviderDescriptor, auth_store: &AuthStore) -> DiscoveryMode {
    match auth_store.get(provider.id.as_str()) {
        Some(StoredCredential::OAuth(_)) if provider.id == "openai" => {
            DiscoveryMode::OpenAiOAuthCodex
        }
        _ => DiscoveryMode::Standard,
    }
}

fn discovery_url(
    provider: &ProviderDescriptor,
    discovery: &ModelDiscoveryConfig,
    mode: &DiscoveryMode,
) -> String {
    match mode {
        DiscoveryMode::Standard => {
            let url = format!(
                "{}{}",
                provider.base_url.trim_end_matches('/'),
                normalized_discovery_path(&provider.base_url, &discovery.path)
            );
            append_query_params(url, &provider.query_params)
        }
        DiscoveryMode::OpenAiOAuthCodex => {
            let url = format!(
                "{}/models",
                openai_oauth_discovery_base_url(provider).trim_end_matches('/')
            );
            let mut parsed = reqwest::Url::parse(&url).unwrap_or_else(|_| {
                reqwest::Url::parse(OPENAI_CHATGPT_BASE_URL)
                    .expect("default OpenAI ChatGPT base URL should be valid")
            });
            {
                let mut pairs = parsed.query_pairs_mut();
                for (key, value) in &provider.query_params {
                    pairs.append_pair(key, value);
                }
                pairs.append_pair("client_version", OPENAI_CODEX_COMPAT_VERSION);
            }
            parsed.to_string()
        }
    }
}

fn normalized_discovery_path(base_url: &str, path: &str) -> String {
    if base_url.trim_end_matches('/').ends_with("/v1") && path.starts_with("/v1/") {
        path[3..].to_string()
    } else {
        path.to_string()
    }
}

fn apply_discovery_headers(
    mut request: RequestBuilder,
    provider: &ProviderDescriptor,
    discovery: &ModelDiscoveryConfig,
    mode: &DiscoveryMode,
) -> RequestBuilder {
    for (key, value) in &provider.headers {
        request = request.header(key, value);
    }
    for (key, value) in &discovery.headers {
        request = request.header(key, value);
    }
    if matches!(mode, DiscoveryMode::OpenAiOAuthCodex) {
        request = request.header("originator", OPENAI_CODEX_ORIGINATOR);
        request = request.header(
            reqwest::header::USER_AGENT,
            format!("{OPENAI_CODEX_ORIGINATOR}/{OPENAI_CODEX_COMPAT_VERSION}"),
        );
        request = request.header("version", OPENAI_CODEX_COMPAT_VERSION);
    }
    request
}

fn apply_discovery_auth(
    mut request: reqwest::blocking::RequestBuilder,
    provider: &ProviderDescriptor,
    auth_store: &AuthStore,
    mode: &DiscoveryMode,
) -> reqwest::blocking::RequestBuilder {
    let auth_family = provider_auth_family(provider);
    match auth_store.get(provider.id.as_str()) {
        Some(StoredCredential::ApiKey { key }) if auth_family == "anthropic" => {
            request = request.header("x-api-key", key);
            request.header("anthropic-version", "2023-06-01")
        }
        Some(StoredCredential::ApiKey { key }) => {
            request.header("Authorization", format!("Bearer {key}"))
        }
        Some(StoredCredential::OAuth(credential)) => {
            request = request.header(
                "Authorization",
                format!("Bearer {}", credential.access_token),
            );
            if matches!(mode, DiscoveryMode::OpenAiOAuthCodex) {
                if let Some(account_id) = credential.account_id.as_deref() {
                    request = request.header("ChatGPT-Account-ID", account_id);
                }
            }
            request
        }
        None => request,
    }
}

fn provider_auth_family(provider: &ProviderDescriptor) -> &'static str {
    if provider.default_api == "anthropic-messages"
        || provider
            .discovery
            .as_ref()
            .map(|discovery| discovery.api.as_str() == "anthropic-messages")
            .unwrap_or(false)
    {
        "anthropic"
    } else {
        "bearer"
    }
}

fn append_query_params(url: String, query_params: &indexmap::IndexMap<String, String>) -> String {
    if query_params.is_empty() {
        return url;
    }
    let mut parsed = match reqwest::Url::parse(&url) {
        Ok(url) => url,
        Err(_) => return url,
    };
    {
        let mut pairs = parsed.query_pairs_mut();
        for (key, value) in query_params {
            pairs.append_pair(key, value);
        }
    }
    parsed.to_string()
}

fn parse_discovered_models(
    provider: &ProviderDescriptor,
    discovery: &ModelDiscoveryConfig,
    payload: &Value,
) -> Result<Vec<ModelDescriptor>> {
    let items = payload
        .get(&discovery.items_field)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            anyhow!(
                "discovery response for {} missing {} array",
                provider.id,
                discovery.items_field
            )
        })?;
    let mut models = Vec::new();
    for item in items {
        let id = item
            .get(&discovery.id_field)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow!(
                    "discovery model for {} missing {}",
                    provider.id,
                    discovery.id_field
                )
            })?;
        let display_name = discovery
            .display_name_field
            .as_deref()
            .and_then(|field| item.get(field))
            .and_then(Value::as_str)
            .or_else(|| default_display_name(item, &discovery.response))
            .unwrap_or(id);
        models.push(ModelDescriptor {
            id: id.to_string(),
            display_name: display_name.to_string(),
            provider: provider.id.clone(),
            api: discovery.api.clone(),
            context_window: discovery.context_window,
            max_output_tokens: discovery.max_output_tokens,
            supports_reasoning: discovery.supports_reasoning,
            compat: None,
            input: vec![crate::Modality::Text],
            cost: None,
        });
    }
    Ok(models)
}

fn default_display_name<'a>(item: &'a Value, format: &ModelDiscoveryFormat) -> Option<&'a str> {
    match format {
        ModelDiscoveryFormat::AnthropicModels => item
            .get("display_name")
            .or_else(|| item.get("name"))
            .and_then(Value::as_str),
        ModelDiscoveryFormat::OpenAiModels => item.get("name").and_then(Value::as_str),
        ModelDiscoveryFormat::OllamaModels => item
            .get("name")
            .or_else(|| item.get("model"))
            .and_then(Value::as_str),
    }
}

fn parse_codex_discovered_models(
    provider: &ProviderDescriptor,
    discovery: &ModelDiscoveryConfig,
    payload: &Value,
) -> Result<Vec<ModelDescriptor>> {
    let items = payload
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            anyhow!(
                "discovery response for {} missing models array",
                provider.id
            )
        })?;
    let mut models = Vec::new();
    for item in items {
        if item.get("visibility").and_then(Value::as_str) == Some("hide") {
            continue;
        }
        if item.get("visibility").and_then(Value::as_str) == Some("none") {
            continue;
        }
        if item
            .get("supported_in_api")
            .and_then(Value::as_bool)
            .is_some_and(|supported| !supported)
        {
            continue;
        }
        let id = item
            .get("slug")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("discovery model for {} missing slug", provider.id))?;
        let display_name = item
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(id);
        let context_window = item
            .get("context_window")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(discovery.context_window);
        let supports_reasoning = item
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .map(|levels| !levels.is_empty())
            .unwrap_or(discovery.supports_reasoning);
        models.push(ModelDescriptor {
            id: id.to_string(),
            display_name: display_name.to_string(),
            provider: provider.id.clone(),
            api: discovery.api.clone(),
            context_window,
            max_output_tokens: discovery.max_output_tokens,
            supports_reasoning,
            compat: None,
            input: vec![crate::Modality::Text],
            cost: None,
        });
    }
    Ok(models)
}

fn openai_oauth_discovery_base_url(provider: &ProviderDescriptor) -> String {
    let trimmed = provider.base_url.trim_end_matches('/');
    if trimmed.contains("/backend-api") || trimmed.contains("/api/codex") {
        trimmed.to_string()
    } else {
        OPENAI_CHATGPT_BASE_URL.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthMode, OAuthCredential};
    use indexmap::IndexMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn provider(discovery: ModelDiscoveryConfig) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "custom".to_string(),
            display_name: "Custom".to_string(),
            base_url: "https://example.invalid".to_string(),
            default_api: "custom-api".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            discovery: Some(discovery),
            models: Vec::new(),
            chat_completions_path: None,
        }
    }

    #[test]
    fn merge_discovered_models_only_adds_missing_ids() {
        let mut models = vec![ModelDescriptor {
            id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            provider: "anthropic".to_string(),
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            compat: None,
            input: vec![crate::Modality::Text],
            cost: None,
        }];

        merge_discovered_models(
            &mut models,
            vec![
                ModelDescriptor {
                    id: "claude-sonnet-4-5".to_string(),
                    display_name: "Claude Sonnet 4.5".to_string(),
                    provider: "anthropic".to_string(),
                    api: "anthropic-messages".to_string(),
                    context_window: 200_000,
                    max_output_tokens: 8_192,
                    supports_reasoning: true,
                    compat: None,
                    input: vec![crate::Modality::Text],
                    cost: None,
                },
                ModelDescriptor {
                    id: "claude-opus-4-1".to_string(),
                    display_name: "Claude Opus 4.1".to_string(),
                    provider: "anthropic".to_string(),
                    api: "anthropic-messages".to_string(),
                    context_window: 200_000,
                    max_output_tokens: 8_192,
                    supports_reasoning: true,
                    compat: None,
                    input: vec![crate::Modality::Text],
                    cost: None,
                },
            ],
        );

        assert_eq!(models.len(), 2);
        assert!(models.iter().any(|model| model.id == "claude-opus-4-1"));
    }

    #[test]
    fn discovery_uses_custom_field_names() {
        let discovery = ModelDiscoveryConfig {
            path: "/models".to_string(),
            response: ModelDiscoveryFormat::OpenAiModels,
            api: "custom-api".to_string(),
            context_window: 32_000,
            max_output_tokens: 4_096,
            supports_reasoning: false,
            items_field: "items".to_string(),
            id_field: "slug".to_string(),
            display_name_field: Some("label".to_string()),
            headers: IndexMap::new(),
        };
        let payload = serde_json::json!({
            "items": [
                { "slug": "reasoner", "label": "Reasoner" }
            ]
        });
        let provider = provider(discovery.clone());
        let models =
            parse_discovered_models(&provider, provider.discovery.as_ref().unwrap(), &payload)
                .expect("models");
        assert_eq!(models[0].id, "reasoner");
        assert_eq!(models[0].display_name, "Reasoner");
    }

    #[test]
    fn discovery_parses_ollama_model_lists() {
        let discovery = ModelDiscoveryConfig {
            path: "/api/tags".to_string(),
            response: ModelDiscoveryFormat::OllamaModels,
            api: "openai-completions".to_string(),
            context_window: 32_768,
            max_output_tokens: 8_192,
            supports_reasoning: false,
            items_field: "models".to_string(),
            id_field: "name".to_string(),
            display_name_field: None,
            headers: IndexMap::new(),
        };
        let payload = serde_json::json!({
            "models": [
                { "name": "qwen3:14b", "model": "qwen3:14b" }
            ]
        });
        let provider = provider(discovery);
        let models =
            parse_discovered_models(&provider, provider.discovery.as_ref().unwrap(), &payload)
                .expect("models");
        assert_eq!(models[0].id, "qwen3:14b");
        assert_eq!(models[0].display_name, "qwen3:14b");
        assert_eq!(models[0].api, "openai-completions");
    }

    #[test]
    fn anthropic_family_discovery_uses_api_key_header_for_custom_provider() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || -> String {
            let (mut stream, _) = listener.accept().expect("connection");
            let mut buffer = [0_u8; 4096];
            let size = std::io::Read::read(&mut stream, &mut buffer).expect("request");
            let request = String::from_utf8_lossy(&buffer[..size]).to_string();
            let body = serde_json::json!({
                "data": [
                    { "id": "claude-compatible" }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).expect("response");
            request
        });
        let provider = ProviderDescriptor {
            id: "custom-anthropic".to_string(),
            display_name: "Custom Anthropic".to_string(),
            base_url: format!("http://{address}"),
            default_api: "anthropic-messages".to_string(),
            auth_modes: vec![crate::auth::AuthMode::ApiKey],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            discovery: Some(ModelDiscoveryConfig {
                path: "/v1/models".to_string(),
                response: ModelDiscoveryFormat::AnthropicModels,
                api: "anthropic-messages".to_string(),
                context_window: 200_000,
                max_output_tokens: 8_192,
                supports_reasoning: true,
                items_field: "data".to_string(),
                id_field: "id".to_string(),
                display_name_field: None,
                headers: IndexMap::new(),
            }),
            models: Vec::new(),
            chat_completions_path: None,
        };
        let mut auth = AuthStore::default();
        auth.set_api_key("custom-anthropic", "sk-ant-custom");
        let client = ModelDiscoveryClient::new();
        let discovered = client
            .discover_models(&provider, &auth)
            .expect("discovered");
        let request = server.join().expect("server");
        assert_eq!(discovered[0].id, "claude-compatible");
        assert!(request.contains("x-api-key: sk-ant-custom"));
        assert!(request.contains("anthropic-version: 2023-06-01"));
        assert!(!request.contains("authorization: Bearer"));
    }

    #[test]
    fn openai_oauth_discovery_uses_codex_models_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || -> String {
            let (mut stream, _) = listener.accept().expect("connection");
            let mut buffer = [0_u8; 4096];
            let size = stream.read(&mut buffer).expect("request");
            let request = String::from_utf8_lossy(&buffer[..size]).to_string();
            let body = serde_json::json!({
                "models": [
                    {
                        "slug": "gpt-5.3-codex",
                        "display_name": "gpt-5.3-codex",
                        "supported_in_api": true,
                        "visibility": "list",
                        "supported_reasoning_levels": [
                            { "effort": "medium", "description": "medium" }
                        ],
                        "context_window": 272000
                    }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("response write");
            request
        });
        let mut auth = AuthStore::default();
        auth.set_oauth(
            "openai",
            OAuthCredential {
                access_token: "token-123".to_string(),
                refresh_token: "refresh-123".to_string(),
                expires_at_ms: 0,
                account_id: Some("acct-123".to_string()),
                organization_id: None,
                email: None,
                plan_type: None,
                rate_limit_tier: None,
                scopes: Vec::new(),
                organization_name: None,
                organization_role: None,
                workspace_role: None,
            },
        );
        let mut provider = provider(ModelDiscoveryConfig {
            path: "/v1/models".to_string(),
            response: ModelDiscoveryFormat::OpenAiModels,
            api: "openai-responses".to_string(),
            context_window: 272_000,
            max_output_tokens: 16_384,
            supports_reasoning: true,
            items_field: "data".to_string(),
            id_field: "id".to_string(),
            display_name_field: None,
            headers: IndexMap::new(),
        });
        provider.id = "openai".to_string();
        provider.base_url = format!("http://{address}/api/codex");
        let models = ModelDiscoveryClient::new()
            .discover_models(&provider, &auth)
            .expect("discovery should succeed");

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5.3-codex");
        let request = server.join().expect("request");
        assert!(request.contains("GET /api/codex/models?client_version=0.125.0"));
        assert!(request.contains("authorization: Bearer token-123"));
        assert!(request.contains("chatgpt-account-id: acct-123"));
        assert!(request.contains("originator: codex_cli_rs"));
        assert!(request.contains("user-agent: codex_cli_rs/0.125.0"));
        assert!(request.contains("version: 0.125.0"));
    }

    #[test]
    fn openai_oauth_default_base_url_uses_chatgpt_backend() {
        let provider = ProviderDescriptor {
            id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            base_url: "https://api.openai.com".to_string(),
            default_api: "openai-responses".to_string(),
            auth_modes: vec![AuthMode::OAuth],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            discovery: None,
            models: Vec::new(),
            chat_completions_path: None,
        };

        assert_eq!(
            openai_oauth_discovery_base_url(&provider),
            OPENAI_CHATGPT_BASE_URL
        );
    }

    #[test]
    fn standard_discovery_avoids_duplicate_v1_prefix() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || -> String {
            let (mut stream, _) = listener.accept().expect("connection");
            let mut buffer = [0_u8; 4096];
            let size = stream.read(&mut buffer).expect("request");
            let request = String::from_utf8_lossy(&buffer[..size]).to_string();
            let body = serde_json::json!({
                "data": [
                    { "id": "gpt-5" },
                    { "id": "gpt-5.4" }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("response write");
            request
        });
        let provider = ProviderDescriptor {
            id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            base_url: format!("http://{address}/v1"),
            default_api: "openai-responses".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            discovery: Some(ModelDiscoveryConfig {
                path: "/v1/models".to_string(),
                response: ModelDiscoveryFormat::OpenAiModels,
                api: "openai-responses".to_string(),
                context_window: 272_000,
                max_output_tokens: 16_384,
                supports_reasoning: true,
                items_field: "data".to_string(),
                id_field: "id".to_string(),
                display_name_field: None,
                headers: IndexMap::new(),
            }),
            models: Vec::new(),
            chat_completions_path: None,
        };
        let mut auth = AuthStore::default();
        auth.set_api_key("openai", "sk-test");

        let models = ModelDiscoveryClient::new()
            .discover_models(&provider, &auth)
            .expect("discovery should succeed");

        assert_eq!(models.len(), 2);
        let request = server.join().expect("request");
        assert!(request.contains("GET /v1/models HTTP/1.1"));
        assert!(!request.contains("GET /v1/v1/models HTTP/1.1"));
    }
}
