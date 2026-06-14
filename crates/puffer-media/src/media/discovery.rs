use super::resolver::{CachedImageMediaModel, MediaDiscoveryCache};
use anyhow::{anyhow, Context, Result};
use puffer_provider_registry::{
    AuthStore, MediaDiscoveryKind, MediaExecutionKind, MediaModelDescriptor, MediaOperation,
    ProviderDescriptor, ProviderRegistry, StoredCredential, Variant, Variants,
};
use reqwest::blocking::{Client, RequestBuilder};
use serde_json::Value;
use std::time::Duration;

const TRUSTED_IMAGE_DISCOVERY_PROVIDER_IDS: &[&str] = &["openrouter", "vercel-ai-gateway"];
const MAX_TRUSTED_DISCOVERY_CONCURRENCY: usize = 2;
const TRUSTED_DISCOVERY_TIMEOUT_MS: u64 = 5_000;

/// Performs trusted image-output media discovery for router/gateway providers.
#[derive(Debug, Clone)]
pub(crate) struct TrustedImageDiscoveryClient {
    client: Client,
}

impl TrustedImageDiscoveryClient {
    /// Creates a trusted media discovery client with bounded request timeouts.
    pub(crate) fn new() -> Self {
        let client = Client::builder()
            .connect_timeout(Duration::from_millis(TRUSTED_DISCOVERY_TIMEOUT_MS))
            .timeout(Duration::from_millis(TRUSTED_DISCOVERY_TIMEOUT_MS))
            .build()
            .unwrap_or_default();
        Self { client }
    }

    /// Discovers trusted image-output models for eligible connected providers.
    pub(crate) fn discover(
        &self,
        registry: &ProviderRegistry,
        auth_store: &AuthStore,
    ) -> MediaDiscoveryCache {
        let eligible = registry
            .providers()
            .filter(|provider| self.provider_is_eligible(provider, auth_store))
            .cloned()
            .collect::<Vec<_>>();
        let mut cache = MediaDiscoveryCache::default();
        for chunk in eligible.chunks(MAX_TRUSTED_DISCOVERY_CONCURRENCY) {
            let results = std::thread::scope(|scope| {
                let handles = chunk
                    .iter()
                    .map(|provider| {
                        let provider = provider.clone();
                        let client = self.clone();
                        scope.spawn(move || client.discover_provider(&provider, auth_store))
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|handle| handle.join().expect("media discovery thread panicked"))
                    .collect::<Vec<_>>()
            });
            for models in results.into_iter().flatten() {
                cache.image_models.extend(models);
            }
        }
        cache
    }

    fn provider_is_eligible(&self, provider: &ProviderDescriptor, auth_store: &AuthStore) -> bool {
        if !TRUSTED_IMAGE_DISCOVERY_PROVIDER_IDS.contains(&provider.id.as_str()) {
            return false;
        }
        if provider.auth_modes.is_empty() {
            return true;
        }
        if !auth_store.has_auth(&provider.id) {
            return false;
        }
        let Some(image) = provider
            .media
            .as_ref()
            .and_then(|media| media.image.as_ref())
        else {
            return false;
        };
        let Some(discovery) = image.discovery.as_ref() else {
            return false;
        };
        if discovery.adapter != MediaDiscoveryKind::TrustedImageOutput {
            return false;
        }
        image.execution.as_ref().is_some_and(|execution| {
            execution.adapter == MediaExecutionKind::ChatImageOutput
                && execution_adapter_is_available(execution.adapter)
        })
    }

    fn discover_provider(
        &self,
        provider: &ProviderDescriptor,
        auth_store: &AuthStore,
    ) -> Result<Vec<CachedImageMediaModel>> {
        let image = provider
            .media
            .as_ref()
            .and_then(|media| media.image.as_ref())
            .context("provider missing image media descriptor")?;
        let discovery = image
            .discovery
            .as_ref()
            .context("provider missing image media discovery descriptor")?;
        let url = media_discovery_url(provider, discovery.path.as_deref(), &discovery.query)?;
        let mut request = self.client.get(url.clone());
        for (name, value) in &provider.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        request = apply_bearer_auth(request, provider, auth_store);
        let response = request
            .send()
            .with_context(|| format!("fetch trusted image models from {url}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!(
                "trusted image model discovery for {} failed with {status}",
                provider.id
            ));
        }
        let payload = response
            .json::<Value>()
            .with_context(|| format!("parse trusted image model discovery response from {url}"))?;
        Ok(parse_trusted_image_models(provider, &payload))
    }
}

fn media_discovery_url(
    provider: &ProviderDescriptor,
    path: Option<&str>,
    query: &std::collections::BTreeMap<String, String>,
) -> Result<reqwest::Url> {
    let path = path.unwrap_or("/models");
    let base = format!("{}/", provider.base_url.trim_end_matches('/'));
    let path = normalized_media_discovery_path(&provider.base_url, path);
    let mut url = reqwest::Url::parse(&base)
        .and_then(|base| base.join(path.trim_start_matches('/')))
        .with_context(|| {
            format!(
                "build trusted image discovery URL from {} and {}",
                provider.base_url, path
            )
        })?;
    {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in &provider.query_params {
            pairs.append_pair(name, value);
        }
        for (name, value) in query {
            pairs.append_pair(name, value);
        }
    }
    Ok(url)
}

fn normalized_media_discovery_path(base_url: &str, path: &str) -> String {
    if base_url.trim_end_matches('/').ends_with("/v1") && path.starts_with("/v1/") {
        path[3..].to_string()
    } else {
        path.to_string()
    }
}

fn apply_bearer_auth(
    request: RequestBuilder,
    provider: &ProviderDescriptor,
    auth_store: &AuthStore,
) -> RequestBuilder {
    match auth_store.get(provider.id.as_str()) {
        Some(StoredCredential::ApiKey { key }) => request.bearer_auth(key),
        Some(StoredCredential::OAuth(credential)) => request.bearer_auth(&credential.access_token),
        None => request,
    }
}

fn parse_trusted_image_models(
    provider: &ProviderDescriptor,
    payload: &Value,
) -> Vec<CachedImageMediaModel> {
    let Some(items) = payload.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| trusted_image_model(provider, item))
        .collect()
}

fn trusted_image_model(
    provider: &ProviderDescriptor,
    item: &Value,
) -> Option<CachedImageMediaModel> {
    if !has_image_output_metadata(item) {
        return None;
    }
    let id = item.get("id").and_then(Value::as_str)?.trim();
    if id.is_empty() || id.eq_ignore_ascii_case("auto") || has_wildcard_or_regex_marker(id) {
        return None;
    }
    let display_name = item
        .get("name")
        .or_else(|| item.get("display_name"))
        .or_else(|| item.get("displayName"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(id);
    Some(CachedImageMediaModel {
        provider_id: provider.id.clone(),
        model: MediaModelDescriptor {
            id: id.to_string(),
            display_name: Some(display_name.to_string()),
            max_outputs: None,
            execution: None,
            operations: vec![MediaOperation::Generate],
            axes: Vec::new(),
            media_map: None,
            variants: Variants::Single(Variant {
                model_id: id.to_string(),
                base_params: std::collections::BTreeMap::new(),
            }),
        },
        source: "provider_discovery".to_string(),
    })
}

fn has_image_output_metadata(item: &Value) -> bool {
    image_array_contains_image(item.get("output_modalities"))
        || image_array_contains_image(item.get("outputModalities"))
        || image_array_contains_image(item.pointer("/architecture/output_modalities"))
        || image_array_contains_image(item.pointer("/architecture/outputModalities"))
        || image_array_contains_image(item.pointer("/modalities/output"))
        || image_array_contains_image(item.pointer("/capabilities/output_modalities"))
        || image_array_contains_image(item.pointer("/capabilities/outputModalities"))
}

fn image_array_contains_image(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(value_is_image_string))
}

fn value_is_image_string(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| text.eq_ignore_ascii_case("image"))
}

fn execution_adapter_is_available(adapter: MediaExecutionKind) -> bool {
    matches!(
        adapter,
        MediaExecutionKind::ImagesJson
            | MediaExecutionKind::GeminiGenerateContent
            | MediaExecutionKind::ChatImageOutput
            | MediaExecutionKind::MinimaxImage
    )
}

fn has_wildcard_or_regex_marker(value: &str) -> bool {
    value.chars().any(|ch| {
        matches!(
            ch,
            '*' | '?' | '[' | ']' | '(' | ')' | '{' | '}' | '|' | '^' | '$' | '\\'
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use puffer_provider_registry::{
        AuthMode, AuthStore, MediaDiscoveryDescriptor, MediaDiscoveryKind,
        MediaExecutionDescriptor, MediaExecutionKind, MediaKindDescriptor, ModelDescriptor,
        ProviderDescriptor, ProviderMediaDescriptor, ProviderRegistry,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn provider(id: &str, base_url: String) -> ProviderDescriptor {
        ProviderDescriptor {
            id: id.to_string(),
            display_name: id.to_string(),
            base_url,
            default_api: "openai-completions".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            chat_completions_path: None,
            discovery: None,
            media: Some(ProviderMediaDescriptor {
                image: Some(MediaKindDescriptor {
                    discovery: Some(MediaDiscoveryDescriptor {
                        adapter: MediaDiscoveryKind::TrustedImageOutput,
                        path: Some("/models".to_string()),
                        query: BTreeMap::from([(
                            "output_modalities".to_string(),
                            "image".to_string(),
                        )]),
                    }),
                    execution: Some(MediaExecutionDescriptor {
                        adapter: MediaExecutionKind::ChatImageOutput,
                        base_url: None,
                        path: "/chat/completions".to_string(),
                        batch: puffer_provider_registry::MediaBatchDescriptor::default(),
                        prompt_format: Default::default(),
                    }),
                    models: Vec::new(),
                }),
                video: None,
            }),
            models: Vec::<ModelDescriptor>::new(),
        }
    }

    fn auth_store(provider_id: &str) -> AuthStore {
        let mut auth = AuthStore::default();
        auth.set_api_key(provider_id, "sk-test");
        auth
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut buffer = [0_u8; 8192];
        let size = stream.read(&mut buffer).expect("read request");
        String::from_utf8_lossy(&buffer[..size]).to_string()
    }

    #[test]
    fn trusted_image_output_discovery_filters_to_concrete_image_models() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let request_text = read_http_request(&mut stream);
            let body = json!({
                "data": [
                    {
                        "id": "openai/gpt-image-chat",
                        "name": "GPT Image Chat",
                        "architecture": {"output_modalities": ["text", "image"]}
                    },
                    {
                        "id": "auto",
                        "name": "Auto",
                        "architecture": {"output_modalities": ["image"]}
                    },
                    {
                        "id": "text-only",
                        "name": "Text Only",
                        "architecture": {"output_modalities": ["text"]}
                    },
                    {
                        "id": "bad*wildcard",
                        "name": "Bad Wildcard",
                        "architecture": {"output_modalities": ["image"]}
                    }
                ]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("response");
            request_text
        });
        let mut registry = ProviderRegistry::new();
        registry.register(provider("openrouter", format!("http://{address}")));

        let cache =
            TrustedImageDiscoveryClient::new().discover(&registry, &auth_store("openrouter"));

        let request_text = server.join().expect("server");
        assert!(request_text.contains("GET /models?output_modalities=image HTTP/1.1"));
        assert!(request_text.contains("authorization: Bearer sk-test"));
        assert_eq!(cache.image_models.len(), 1);
        assert_eq!(cache.image_models[0].provider_id, "openrouter");
        assert_eq!(cache.image_models[0].model.id, "openai/gpt-image-chat");
        assert_eq!(
            cache.image_models[0].model.display_name.as_deref(),
            Some("GPT Image Chat")
        );
        assert_eq!(cache.image_models[0].source, "provider_discovery");
    }

    #[test]
    fn trusted_image_output_discovery_ignores_untrusted_providers() {
        let mut registry = ProviderRegistry::new();
        registry.register(provider("worldrouter", "http://127.0.0.1:9".to_string()));

        let cache =
            TrustedImageDiscoveryClient::new().discover(&registry, &auth_store("worldrouter"));

        assert!(cache.image_models.is_empty());
    }

    #[test]
    fn trusted_image_output_discovery_requires_chat_image_output_execution() {
        let mut provider = provider("openrouter", "http://127.0.0.1:9".to_string());
        let image = provider
            .media
            .as_mut()
            .and_then(|media| media.image.as_mut())
            .expect("image media");
        image.execution = Some(MediaExecutionDescriptor {
            adapter: MediaExecutionKind::ImagesJson,
            base_url: None,
            path: "/images/generations".to_string(),
            batch: puffer_provider_registry::MediaBatchDescriptor::default(),
            prompt_format: Default::default(),
        });

        assert!(!TrustedImageDiscoveryClient::new()
            .provider_is_eligible(&provider, &auth_store("openrouter")));
    }
}
