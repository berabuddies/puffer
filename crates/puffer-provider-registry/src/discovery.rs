use crate::auth::{AuthStore, StoredCredential};
use crate::model::{
    ModelDescriptor, ModelDiscoveryConfig, ModelDiscoveryFormat, ProviderDescriptor,
};
use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde_json::Value;

/// Performs runtime provider model-discovery requests.
#[derive(Debug, Clone, Default)]
pub struct ModelDiscoveryClient {
    client: Client,
}

impl ModelDiscoveryClient {
    /// Creates a discovery client backed by the default blocking HTTP client.
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
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
        let url = format!(
            "{}{}",
            provider.base_url.trim_end_matches('/'),
            discovery.path
        );
        let mut request = self.client.get(&url);
        for (key, value) in &provider.headers {
            request = request.header(key, value);
        }
        for (key, value) in &discovery.headers {
            request = request.header(key, value);
        }
        request = apply_discovery_auth(request, provider, auth_store);
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
        parse_discovered_models(provider, discovery, &payload)
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

fn apply_discovery_auth(
    mut request: reqwest::blocking::RequestBuilder,
    provider: &ProviderDescriptor,
    auth_store: &AuthStore,
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
        Some(StoredCredential::OAuth(credential)) => request.header(
            "Authorization",
            format!("Bearer {}", credential.access_token),
        ),
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

fn parse_discovered_models(
    provider: &ProviderDescriptor,
    discovery: &ModelDiscoveryConfig,
    payload: &Value,
) -> Result<Vec<ModelDescriptor>> {
    let items = match discovery.response {
        ModelDiscoveryFormat::MistralModels => payload.as_array().ok_or_else(|| {
            anyhow!(
                "discovery response for {} was not a top-level array",
                provider.id
            )
        })?,
        _ => payload
            .get(&discovery.items_field)
            .and_then(Value::as_array)
            .ok_or_else(|| {
                anyhow!(
                    "discovery response for {} missing {} array",
                    provider.id,
                    discovery.items_field
                )
            })?,
    };
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
        ModelDiscoveryFormat::MistralModels => item
            .get("name")
            .or_else(|| item.get("display_name"))
            .and_then(Value::as_str),
        ModelDiscoveryFormat::OpenAiModels => item.get("name").and_then(Value::as_str),
        ModelDiscoveryFormat::OllamaModels => item
            .get("name")
            .or_else(|| item.get("model"))
            .and_then(Value::as_str),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthMode;
    use indexmap::IndexMap;

    fn provider(discovery: ModelDiscoveryConfig) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "custom".to_string(),
            display_name: "Custom".to_string(),
            base_url: "https://example.invalid".to_string(),
            default_api: "custom-api".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: IndexMap::new(),
            discovery: Some(discovery),
            models: Vec::new(),
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
                },
                ModelDescriptor {
                    id: "claude-opus-4-1".to_string(),
                    display_name: "Claude Opus 4.1".to_string(),
                    provider: "anthropic".to_string(),
                    api: "anthropic-messages".to_string(),
                    context_window: 200_000,
                    max_output_tokens: 8_192,
                    supports_reasoning: true,
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
    fn discovery_parses_mistral_model_lists() {
        let discovery = ModelDiscoveryConfig {
            path: "/v1/models".to_string(),
            response: ModelDiscoveryFormat::MistralModels,
            api: "mistral-conversations".to_string(),
            context_window: 131_072,
            max_output_tokens: 32_768,
            supports_reasoning: false,
            items_field: "data".to_string(),
            id_field: "id".to_string(),
            display_name_field: Some("name".to_string()),
            headers: IndexMap::new(),
        };
        let payload = serde_json::json!([
            {
                "id": "devstral-medium-latest",
                "name": "Devstral Medium Latest"
            }
        ]);
        let provider = provider(discovery);
        let models =
            parse_discovered_models(&provider, provider.discovery.as_ref().unwrap(), &payload)
                .expect("models");
        assert_eq!(models[0].id, "devstral-medium-latest");
        assert_eq!(models[0].display_name, "Devstral Medium Latest");
        assert_eq!(models[0].api, "mistral-conversations");
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
}
