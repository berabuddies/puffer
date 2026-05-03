use crate::auth::AuthStore;
use crate::discovery::{merge_discovered_models, ModelDiscoveryClient};
use crate::model::{
    ModelDescriptor, ProviderDescriptor, ProviderSource, ProviderSourceKind, RegisteredProvider,
};
use anyhow::{anyhow, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Stores all providers and models known to the application.
#[derive(Debug, Clone, Default)]
pub struct ProviderRegistry {
    providers: IndexMap<String, RegisteredProvider>,
}

impl ProviderRegistry {
    /// Creates an empty provider registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or replaces a provider descriptor using the builtin source kind.
    pub fn register(&mut self, provider: ProviderDescriptor) {
        self.register_with_source(
            provider,
            ProviderSource {
                kind: ProviderSourceKind::Builtin,
                path: None,
            },
        );
    }

    /// Registers or replaces a provider descriptor with explicit provenance.
    pub fn register_with_source(&mut self, provider: ProviderDescriptor, source: ProviderSource) {
        self.providers.insert(
            provider.id.clone(),
            RegisteredProvider {
                descriptor: provider,
                source,
            },
        );
    }

    /// Registers a sequence of providers into the registry.
    pub fn register_many(&mut self, providers: impl IntoIterator<Item = ProviderDescriptor>) {
        for provider in providers {
            self.register(provider);
        }
    }

    /// Applies an optional base URL override to the built-in `openai` provider.
    pub fn apply_openai_base_url_override(&mut self, base_url: Option<&str>) {
        let Some(base_url) = base_url.map(str::trim).filter(|value| !value.is_empty()) else {
            return;
        };
        self.set_openai_base_url(base_url);
    }

    /// Sets the built-in `openai` provider base URL to the provided absolute value.
    pub fn set_openai_base_url(&mut self, base_url: impl Into<String>) {
        if let Some(provider) = self.providers.get_mut("openai") {
            provider.descriptor.base_url = base_url.into();
        }
    }

    /// Replaces the built-in `openai` provider's static header map.
    pub fn set_openai_headers(&mut self, headers: impl Into<indexmap::IndexMap<String, String>>) {
        if let Some(provider) = self.providers.get_mut("openai") {
            provider.descriptor.headers = headers.into();
        }
    }

    /// Replaces the built-in `openai` provider's query parameter map.
    pub fn set_openai_query_params(
        &mut self,
        query_params: impl Into<indexmap::IndexMap<String, String>>,
    ) {
        if let Some(provider) = self.providers.get_mut("openai") {
            provider.descriptor.query_params = query_params.into();
        }
    }

    /// Returns an iterator over all registered provider descriptors in insertion order.
    pub fn providers(&self) -> impl Iterator<Item = &ProviderDescriptor> {
        self.providers.values().map(|provider| &provider.descriptor)
    }

    /// Returns an iterator over all registered providers including provenance.
    pub fn provider_entries(&self) -> impl Iterator<Item = &RegisteredProvider> {
        self.providers.values()
    }

    /// Looks up a provider descriptor by id.
    pub fn provider(&self, id: &str) -> Option<&ProviderDescriptor> {
        self.providers.get(id).map(|provider| &provider.descriptor)
    }

    /// Looks up a registered provider entry by id.
    pub fn provider_entry(&self, id: &str) -> Option<&RegisteredProvider> {
        self.providers.get(id)
    }

    /// Returns an iterator over all known models across all providers.
    pub fn models(&self) -> impl Iterator<Item = &ModelDescriptor> {
        self.providers
            .values()
            .flat_map(|provider| provider.descriptor.models.iter())
    }

    /// Resolves a model from a `provider/model` selector string.
    pub fn resolve_model(&self, value: &str) -> Option<&ModelDescriptor> {
        let (provider_id, model_id) = value.split_once('/')?;
        self.provider(provider_id)?
            .models
            .iter()
            .find(|model| model.id == model_id)
    }

    /// Discovers and merges runtime models for every provider that exposes discovery config.
    ///
    /// Uses a disk cache (`~/.puffer/model_discovery_cache.json`) to avoid
    /// redundant HTTP requests within a 1-hour window.  When the cache is stale
    /// or missing, providers are discovered in parallel using thread::scope.
    pub fn discover_and_merge_all(&mut self, auth_store: &AuthStore) -> Result<()> {
        let cache_path = discovery_cache_path();

        // --- Try loading from cache first ---
        if let Some(cache) = load_discovery_cache(&cache_path) {
            let now_ms = current_time_ms();
            let all_fresh = self.providers.values().all(|entry| {
                let id = &entry.descriptor.id;
                entry.descriptor.discovery.is_none()
                    || cache
                        .entries
                        .get(id.as_str())
                        .is_some_and(|e| now_ms.saturating_sub(e.cached_at_ms) < CACHE_TTL_MS)
            });
            if all_fresh {
                for entry in self.providers.values_mut() {
                    if let Some(cached) = cache.entries.get(entry.descriptor.id.as_str()) {
                        merge_discovered_models(
                            &mut entry.descriptor.models,
                            cached.models.clone(),
                        );
                    }
                }
                return Ok(());
            }
        }

        // --- Parallel discovery ---
        let eligible: Vec<_> = self
            .providers
            .values()
            .filter(|entry| entry.descriptor.discovery.is_some())
            .filter(|entry| {
                auth_store.get(entry.descriptor.id.as_str()).is_some()
                    || is_local_url(&entry.descriptor.base_url)
            })
            .map(|entry| entry.descriptor.clone())
            .collect();

        let results: Vec<(String, Result<Vec<ModelDescriptor>>)> = std::thread::scope(|s| {
            let handles: Vec<_> = eligible
                .iter()
                .map(|provider| {
                    let id = provider.id.clone();
                    let client = ModelDiscoveryClient::new();
                    s.spawn(move || {
                        let models = client.discover_models(provider, auth_store);
                        (id, models)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("discovery thread panicked"))
                .collect()
        });

        let mut failures = Vec::new();
        let mut cache_entries: HashMap<String, DiscoveryCacheEntry> = HashMap::new();
        let now_ms = current_time_ms();

        for (provider_id, result) in results {
            match result {
                Ok(discovered) => {
                    cache_entries.insert(
                        provider_id.clone(),
                        DiscoveryCacheEntry {
                            models: discovered.clone(),
                            cached_at_ms: now_ms,
                        },
                    );
                    if !discovered.is_empty() {
                        if let Some(entry) = self.providers.get_mut(&provider_id) {
                            merge_discovered_models(&mut entry.descriptor.models, discovered);
                        }
                    }
                }
                Err(error) => {
                    failures.push(format!("{provider_id}: {error}"));
                }
            }
        }

        // --- Persist cache ---
        if !cache_entries.is_empty() {
            let _ = save_discovery_cache(
                &cache_path,
                &DiscoveryCache {
                    entries: cache_entries,
                },
            );
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(
                "model discovery failed for {} provider(s): {}",
                failures.len(),
                failures.join("; ")
            ))
        }
    }

    /// Discovers and merges runtime models for one provider when discovery is configured.
    pub fn discover_and_merge_provider(
        &mut self,
        provider_id: &str,
        auth_store: &AuthStore,
    ) -> Result<()> {
        let client = ModelDiscoveryClient::new();
        self.discover_and_merge_provider_with_client(provider_id, auth_store, &client)
    }
}

impl ProviderRegistry {
    fn discover_and_merge_provider_with_client(
        &mut self,
        provider_id: &str,
        auth_store: &AuthStore,
        client: &ModelDiscoveryClient,
    ) -> Result<()> {
        let Some(provider) = self.providers.get(provider_id).cloned() else {
            return Err(anyhow!("provider {provider_id} is not registered"));
        };
        if provider.descriptor.discovery.is_none() {
            return Ok(());
        }
        // Skip discovery for remote providers that have no credentials — the
        // request would almost certainly fail with 401/403 anyway.
        if auth_store.get(provider_id).is_none() && !is_local_url(&provider.descriptor.base_url) {
            return Ok(());
        }
        let discovered = client.discover_models(&provider.descriptor, auth_store)?;
        if discovered.is_empty() {
            return Ok(());
        }
        if let Some(entry) = self.providers.get_mut(provider_id) {
            merge_discovered_models(&mut entry.descriptor.models, discovered);
        }
        Ok(())
    }
}

/// Cache TTL: 1 hour.
const CACHE_TTL_MS: u64 = 3_600_000;

#[derive(Debug, Serialize, Deserialize)]
struct DiscoveryCache {
    entries: HashMap<String, DiscoveryCacheEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiscoveryCacheEntry {
    models: Vec<ModelDescriptor>,
    cached_at_ms: u64,
}

fn discovery_cache_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".puffer").join("model_discovery_cache.json")
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn load_discovery_cache(path: &PathBuf) -> Option<DiscoveryCache> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_discovery_cache(path: &PathBuf, cache: &DiscoveryCache) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string(cache)?;
    std::fs::write(path, data)?;
    Ok(())
}

fn is_local_url(url: &str) -> bool {
    let url = url.to_lowercase();
    url.contains("://localhost") || url.contains("://127.0.0.1") || url.contains("://[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthMode;
    use crate::model::{ModelDiscoveryConfig, ModelDiscoveryFormat};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn provider_descriptor() -> ProviderDescriptor {
        ProviderDescriptor {
            id: "anthropic".to_string(),
            display_name: "Anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            default_api: "anthropic-messages".to_string(),
            auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            discovery: None,
            models: vec![ModelDescriptor {
                id: "claude-sonnet-4-5".to_string(),
                display_name: "Claude Sonnet 4.5".to_string(),
                provider: "anthropic".to_string(),
                api: "anthropic-messages".to_string(),
                context_window: 200_000,
                max_output_tokens: 8_192,
                supports_reasoning: true,
                compat: None,
                input: vec![crate::model::Modality::Text],
                cost: None,
            }],
        }
    }

    #[test]
    fn registry_tracks_provider_sources() {
        let mut registry = ProviderRegistry::new();
        registry.register_with_source(
            provider_descriptor(),
            ProviderSource {
                kind: ProviderSourceKind::ResourcePack,
                path: Some("resources/providers/anthropic.yaml".to_string()),
            },
        );

        let entry = registry
            .provider_entry("anthropic")
            .expect("provider entry");
        assert_eq!(entry.source.kind, ProviderSourceKind::ResourcePack);
        assert_eq!(
            registry
                .resolve_model("anthropic/claude-sonnet-4-5")
                .expect("model")
                .display_name,
            "Claude Sonnet 4.5"
        );
    }

    #[test]
    fn parse_openai_discovery_response_maps_models() {
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
                    { "id": "gpt-5-mini" }
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
        let mut provider = provider_descriptor();
        provider.id = "openai".to_string();
        provider.base_url = format!("http://{address}");
        provider.default_api = "openai-responses".to_string();
        provider.discovery = Some(ModelDiscoveryConfig {
            path: "/v1/models".to_string(),
            response: ModelDiscoveryFormat::OpenAiModels,
            api: "openai-responses".to_string(),
            context_window: 272_000,
            max_output_tokens: 16_384,
            supports_reasoning: true,
            items_field: "data".to_string(),
            id_field: "id".to_string(),
            display_name_field: None,
            headers: IndexMap::from([("x-test-header".to_string(), "present".to_string())]),
        });
        let mut auth = AuthStore::default();
        auth.set_api_key("openai", "sk-openai");
        let mut registry = ProviderRegistry::new();
        registry.register(provider);

        registry
            .discover_and_merge_provider("openai", &auth)
            .expect("discovery succeeds");

        let request = server.join().expect("server thread");
        let entry = registry.provider("openai").expect("provider");
        assert!(entry.models.iter().any(|model| model.id == "gpt-5"));
        assert!(entry.models.iter().any(|model| model.id == "gpt-5-mini"));
        assert!(request.contains("GET /v1/models HTTP/1.1"));
        assert!(request.contains("authorization: Bearer sk-openai"));
        assert!(request.contains("x-test-header: present"));
    }

    #[test]
    fn openai_base_url_override_updates_builtin_provider() {
        let mut registry = ProviderRegistry::new();
        registry.register(ProviderDescriptor {
            id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            base_url: "https://api.openai.com".to_string(),
            default_api: "openai-responses".to_string(),
            auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            discovery: None,
            models: Vec::new(),
        });

        registry.apply_openai_base_url_override(Some("https://proxy.example/v1"));

        assert_eq!(
            registry
                .provider("openai")
                .map(|provider| provider.base_url.as_str()),
            Some("https://proxy.example/v1")
        );
    }

    #[test]
    fn openai_header_override_updates_builtin_provider() {
        let mut registry = ProviderRegistry::new();
        registry.register(ProviderDescriptor {
            id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            base_url: "https://api.openai.com".to_string(),
            default_api: "openai-responses".to_string(),
            auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            discovery: None,
            models: Vec::new(),
        });

        registry.set_openai_headers(IndexMap::from([(
            "x-imported".to_string(),
            "present".to_string(),
        )]));

        assert_eq!(
            registry
                .provider("openai")
                .and_then(|provider| provider.headers.get("x-imported"))
                .map(String::as_str),
            Some("present")
        );
    }

    #[test]
    fn openai_query_param_override_updates_builtin_provider() {
        let mut registry = ProviderRegistry::new();
        registry.register(ProviderDescriptor {
            id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            base_url: "https://api.openai.com".to_string(),
            default_api: "openai-responses".to_string(),
            auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            discovery: None,
            models: Vec::new(),
        });

        registry.set_openai_query_params(IndexMap::from([(
            "api-version".to_string(),
            "2025-01-01".to_string(),
        )]));

        assert_eq!(
            registry
                .provider("openai")
                .and_then(|provider| provider.query_params.get("api-version"))
                .map(String::as_str),
            Some("2025-01-01")
        );
    }

    #[test]
    fn discover_and_merge_all_continues_after_provider_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("connection");
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer).expect("request");
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
        });

        let mut failing = provider_descriptor();
        failing.id = "anthropic".to_string();
        failing.base_url = "http://127.0.0.1:1".to_string();
        failing.discovery = Some(ModelDiscoveryConfig {
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
        });

        let mut succeeding = provider_descriptor();
        succeeding.id = "openai".to_string();
        succeeding.base_url = format!("http://{address}/v1");
        succeeding.default_api = "openai-responses".to_string();
        succeeding.discovery = Some(ModelDiscoveryConfig {
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
        succeeding.models = vec![ModelDescriptor {
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            provider: "openai".to_string(),
            api: "openai-responses".to_string(),
            context_window: 272_000,
            max_output_tokens: 16_384,
            supports_reasoning: true,
            compat: None,
            input: vec![crate::model::Modality::Text],
            cost: None,
        }];

        let mut registry = ProviderRegistry::new();
        registry.register(failing);
        registry.register(succeeding);
        let mut auth = AuthStore::default();
        auth.set_api_key("openai", "sk-test");

        let error = registry
            .discover_and_merge_all(&auth)
            .expect_err("one provider should fail");
        assert!(error.to_string().contains("anthropic"));
        let openai = registry.provider("openai").expect("openai provider");
        assert!(openai.models.iter().any(|model| model.id == "gpt-5.4"));

        server.join().expect("server");
    }
}
