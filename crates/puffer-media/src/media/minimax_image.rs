use super::artifacts::MediaArtifact;
use super::http_support::{
    bearer_token, download_image_url, provider_error_secrets, provider_execution_url,
    redact_secrets, CredentialAliasMode,
};
use super::jobs::{MediaJob, MediaJobStatus};
use super::planner::plan_image_generation;
use super::resolver::{resolve_image_execution_descriptor, MediaDiscoveryCache};
use super::{MediaGenerationService, MediaKind};
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use puffer_provider_registry::{
    AuthStore, MediaExecutionDescriptor, ProviderDescriptor, ProviderRegistry,
};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const DEFAULT_MINIMAX_REQUEST_TIMEOUT_MS: u64 = 300_000;
const MINIMAX_ALLOWED_REQUEST_FIELDS: &[&str] = &["aspect_ratio", "size", "response_format"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MinimaxImageRequest {
    model: String,
    prompt: String,
    parameters: BTreeMap<String, String>,
}

impl MinimaxImageRequest {
    fn new(
        model: impl Into<String>,
        prompt: impl Into<String>,
        parameters: BTreeMap<String, String>,
    ) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            parameters,
        }
    }

    fn to_body(&self) -> Value {
        let mut body = Map::new();
        body.insert("model".to_string(), json!(self.model));
        body.insert("prompt".to_string(), json!(self.prompt));
        for (name, value) in &self.parameters {
            body.insert(name.clone(), json!(value));
        }
        Value::Object(body)
    }
}

/// Carries an exact MiniMax image generation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MinimaxImageGenerationRequest {
    pub(crate) provider_id: String,
    pub(crate) model_id: String,
    pub(crate) adapter: String,
    pub(crate) prompt: String,
    pub(crate) parameters: BTreeMap<String, String>,
    pub(crate) count: u8,
}

/// Carries persisted media records created by the MiniMax image adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MinimaxImageGenerationResult {
    pub(crate) job: MediaJob,
    pub(crate) artifacts: Vec<MediaArtifact>,
}

/// Executes descriptor-driven MiniMax image generation.
#[derive(Debug, Clone)]
pub(crate) struct MinimaxImageAdapter {
    client: Client,
}

impl MinimaxImageAdapter {
    /// Creates an adapter with a default blocking HTTP client.
    pub(crate) fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_millis(DEFAULT_MINIMAX_REQUEST_TIMEOUT_MS))
            .build()
            .context("build MiniMax image generation HTTP client")?;
        Ok(Self { client })
    }

    /// Executes an exact MiniMax image request and persists job/artifact sidecars.
    pub(crate) fn execute(
        &self,
        registry: &ProviderRegistry,
        auth_store: &AuthStore,
        service: &MediaGenerationService,
        request: MinimaxImageGenerationRequest,
    ) -> Result<MinimaxImageGenerationResult> {
        // `request.parameters` are already resolved and keyed by upstream
        // request field; validate them against the adapter's allowlist.
        let selected_parameters = validated_request_fields(&request.parameters)?;

        let discovery_cache = MediaDiscoveryCache::default();
        let (provider, execution) = resolve_image_execution_descriptor(
            registry,
            &request.provider_id,
            &request.model_id,
            &request.adapter,
            &discovery_cache,
        )?;
        let plan = plan_image_generation(request.count, &execution.batch)?;
        if plan.calls.iter().any(|call| call.requested_count != 1) {
            bail!("MiniMax image generation supports only per-image call plans");
        }

        let job_id = Uuid::new_v4().to_string();
        let created_at_ms = now_ms();
        let mut job = MediaJob::new(
            job_id.clone(),
            MediaKind::Image,
            request.provider_id.clone(),
            request.model_id.clone(),
            request.prompt.clone(),
            request.count,
            created_at_ms,
        );
        job.adapter = Some(request.adapter.clone());
        job.parameters = request.parameters.clone();
        service.save_job(&job)?;
        job.transition(MediaJobStatus::Running, now_ms())?;
        service.save_job(&job)?;

        let mut outputs = Vec::new();
        let mut last_error = None;
        for _ in &plan.calls {
            match self.request_image(
                provider,
                auth_store,
                &execution,
                &request,
                selected_parameters.clone(),
            ) {
                Ok(output) => outputs.push(output),
                Err(error) => {
                    last_error = Some(error);
                    break;
                }
            }
        }
        if outputs.is_empty() {
            let error = last_error
                .map(|error| format!("{error:#}"))
                .unwrap_or_else(|| "MiniMax image generation produced no images".to_string());
            job.error = Some(error.clone());
            job.transition(MediaJobStatus::Failed, now_ms())?;
            service.save_job(&job)?;
            bail!(error);
        }

        let mut artifacts = Vec::new();
        for (index, output) in outputs.into_iter().enumerate() {
            let artifact_id = Uuid::new_v4().to_string();
            let filename = "image.png";
            let artifact_path =
                service.write_image_artifact_bytes(&artifact_id, filename, &output.bytes)?;
            let artifact = MediaArtifact {
                id: artifact_id.clone(),
                job_id: job_id.clone(),
                kind: MediaKind::Image,
                path: artifact_path.clone(),
                mime_type: "image/png".to_string(),
                byte_count: output.bytes.len() as u64,
                metadata: artifact_metadata(
                    &request,
                    &artifact_path,
                    &output,
                    index,
                    created_at_ms,
                ),
                preview: None,
                created_at_ms,
            };
            service.save_artifact(&artifact)?;
            job.attach_artifact(artifact_id, now_ms());
            artifacts.push(artifact);
        }
        job.transition(MediaJobStatus::Succeeded, now_ms())?;
        service.save_job(&job)?;

        Ok(MinimaxImageGenerationResult { job, artifacts })
    }

    fn request_image(
        &self,
        provider: &ProviderDescriptor,
        auth_store: &AuthStore,
        execution: &MediaExecutionDescriptor,
        request: &MinimaxImageGenerationRequest,
        parameters: BTreeMap<String, String>,
    ) -> Result<MinimaxOutput> {
        let url = provider_execution_url(provider, execution, "MiniMax image generation")?;
        let secrets = provider_error_secrets(provider, auth_store, CredentialAliasMode::Strict);
        let body =
            MinimaxImageRequest::new(&request.model_id, &request.prompt, parameters).to_body();
        let mut http = self.client.post(url).json(&body);
        for (name, value) in &provider.headers {
            http = http.header(name.as_str(), value.as_str());
        }
        if let Some(token) = bearer_token(provider, auth_store, CredentialAliasMode::Strict)? {
            http = http.bearer_auth(token);
        }
        let response = http
            .send()
            .map_err(|error| anyhow!("{}", redact_secrets(&error.to_string(), &secrets)))
            .context("send MiniMax image generation request")?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| anyhow!("{}", redact_secrets(&error.to_string(), &secrets)))
            .context("read MiniMax image generation response")?;
        if !status.is_success() {
            bail!(
                "MiniMax image generation failed with status {}: {}",
                status.as_u16(),
                redact_secrets(&body, &secrets)
            );
        }
        let value: Value =
            serde_json::from_str(&body).context("parse MiniMax image generation response")?;
        minimax_output_from_response(&self.client, &value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MinimaxOutput {
    bytes: Vec<u8>,
    remote_source_url: Option<String>,
}

fn validated_request_fields(
    parameters: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    for request_field in parameters.keys() {
        if !MINIMAX_ALLOWED_REQUEST_FIELDS.contains(&request_field.as_str()) {
            bail!("MiniMax image request field unsupported: {request_field}");
        }
    }
    Ok(parameters.clone())
}

fn minimax_output_from_response(client: &Client, value: &Value) -> Result<MinimaxOutput> {
    if let Some(base_resp) = value.get("base_resp") {
        let status_code = base_resp
            .get("status_code")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if status_code != 0 {
            let status_msg = base_resp
                .get("status_msg")
                .and_then(Value::as_str)
                .unwrap_or("unknown MiniMax error");
            bail!("MiniMax image generation failed: {status_code} {status_msg}");
        }
    }
    let data = value
        .get("data")
        .context("MiniMax image generation response did not contain data")?;
    if let Some(encoded) = first_string(data.get("image_base64")) {
        let bytes = BASE64_STANDARD
            .decode(encoded.trim())
            .context("decode MiniMax image_base64")?;
        return Ok(MinimaxOutput {
            bytes,
            remote_source_url: None,
        });
    }
    if let Some(url) = first_string(data.get("image_urls")) {
        let bytes = download_image_url(client, url, "MiniMax image")?;
        return Ok(MinimaxOutput {
            bytes,
            remote_source_url: Some(url.to_string()),
        });
    }
    bail!("MiniMax image generation response did not contain an image")
}

fn first_string(value: Option<&Value>) -> Option<&str> {
    match value? {
        Value::String(text) => Some(text),
        Value::Array(items) => items.iter().find_map(Value::as_str),
        _ => None,
    }
}

fn artifact_metadata(
    request: &MinimaxImageGenerationRequest,
    path: &std::path::Path,
    output: &MinimaxOutput,
    index: usize,
    created_at_ms: u64,
) -> Value {
    let mut metadata = json!({
        "providerId": request.provider_id,
        "modelId": request.model_id,
        "adapter": request.adapter,
        "prompt": request.prompt,
        "parameters": request.parameters,
        "index": index,
        "mimeType": "image/png",
        "localPath": path,
        "byteCount": output.bytes.len() as u64,
        "createdAtMs": created_at_ms,
    });
    if let Some(remote_source_url) = &output.remote_source_url {
        metadata["remoteSourceUrl"] = json!(remote_source_url);
    }
    metadata
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::MediaGenerationService;
    use indexmap::IndexMap;
    use puffer_provider_registry::{
        AuthMode, AuthStore, Axis, AxisRole, ControlKind, MediaBatchDescriptor, MediaBatchMode,
        MediaExecutionDescriptor, MediaExecutionKind, MediaKindDescriptor, MediaModelDescriptor,
        MediaOperation, ModelDescriptor, ProviderDescriptor, ProviderMediaDescriptor,
        ProviderRegistry, Variant, Variants, WireType,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use tempfile::tempdir;

    fn registry_with_provider(base_url: String) -> ProviderRegistry {
        registry_with_provider_batch(base_url, MediaBatchDescriptor::default())
    }

    fn registry_with_provider_batch(
        base_url: String,
        batch: MediaBatchDescriptor,
    ) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        registry.register(ProviderDescriptor {
            id: "minimax".to_string(),
            display_name: "MiniMax".to_string(),
            base_url: "https://api.minimax.io/anthropic".to_string(),
            default_api: "anthropic-messages".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            chat_completions_path: None,
            discovery: None,
            media: Some(ProviderMediaDescriptor {
                image: Some(MediaKindDescriptor {
                    discovery: None,
                    execution: Some(MediaExecutionDescriptor {
                        adapter: MediaExecutionKind::MinimaxImage,
                        base_url: Some(base_url),
                        path: "/v1/image_generation".to_string(),
                        batch,
                        prompt_format: Default::default(),
                    }),
                    models: vec![MediaModelDescriptor {
                        id: "image-01".to_string(),
                        display_name: Some("Image 01".to_string()),
                        max_outputs: None,
                        execution: None,
                        operations: vec![MediaOperation::Generate],
                        axes: vec![
                            Axis {
                                id: "aspect_ratio".to_string(),
                                label: "Aspect ratio".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec!["1:1".to_string(), "16:9".to_string()],
                                    default: "1:1".to_string(),
                                },
                                request_field: Some("aspect_ratio".to_string()),
                                wire_type: WireType::String,
                            },
                            Axis {
                                id: "response_format".to_string(),
                                label: "Response format".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec!["url".to_string(), "base64".to_string()],
                                    default: "base64".to_string(),
                                },
                                request_field: Some("response_format".to_string()),
                                wire_type: WireType::String,
                            },
                        ],
                        variants: Variants::Single(Variant {
                            model_id: "image-01".to_string(),
                            base_params: ::std::collections::BTreeMap::new(),
                        }),
                        media_map: None,
                    }],
                }),
                video: None,
            }),
            models: Vec::<ModelDescriptor>::new(),
        });
        registry
    }

    fn auth_store() -> AuthStore {
        let mut auth = AuthStore::default();
        auth.set_api_key("minimax", "sk-minimax");
        auth
    }

    fn request() -> MinimaxImageGenerationRequest {
        MinimaxImageGenerationRequest {
            provider_id: "minimax".to_string(),
            model_id: "image-01".to_string(),
            adapter: "minimax_image".to_string(),
            prompt: "draw a precise icon".to_string(),
            parameters: BTreeMap::from([
                ("aspect_ratio".to_string(), "16:9".to_string()),
                ("response_format".to_string(), "base64".to_string()),
            ]),
            count: 1,
        }
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut buffer = [0_u8; 8192];
        let size = stream.read(&mut buffer).expect("read request");
        String::from_utf8_lossy(&buffer[..size]).to_string()
    }

    #[test]
    fn minimax_image_posts_generation_request_and_decodes_base64() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let request_text = read_http_request(&mut stream);
            let body = json!({
                "data": {"image_base64": ["aW1hZ2UtYnl0ZXM="]},
                "base_resp": {"status_code": 0, "status_msg": "success"}
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
        let registry = registry_with_provider(format!("http://{address}"));
        let service_dir = tempdir().expect("tempdir");

        let result = MinimaxImageAdapter::new()
            .expect("adapter")
            .execute(
                &registry,
                &auth_store(),
                &MediaGenerationService::new(service_dir.path()),
                request(),
            )
            .expect("generation succeeds");

        let request_text = server.join().expect("server");
        assert!(request_text.starts_with("POST /v1/image_generation HTTP/1.1"));
        assert!(request_text.contains("authorization: Bearer sk-minimax"));
        assert!(request_text.contains("\"model\":\"image-01\""));
        assert!(request_text.contains("\"aspect_ratio\":\"16:9\""));
        assert!(request_text.contains("\"response_format\":\"base64\""));
        assert_eq!(
            std::fs::read(&result.artifacts[0].path).unwrap(),
            b"image-bytes"
        );
        assert_eq!(result.artifacts[0].metadata["adapter"], "minimax_image");
    }

    #[test]
    fn minimax_image_failed_later_call_preserves_first_artifact() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for index in 0..2 {
                let (mut stream, _) = listener.accept().expect("request");
                requests.push(read_http_request(&mut stream));
                let body = if index == 0 {
                    json!({
                        "data": {"image_base64": ["aW1hZ2U="]},
                        "base_resp": {"status_code": 0, "status_msg": "success"}
                    })
                    .to_string()
                } else {
                    json!({
                        "data": {},
                        "base_resp": {"status_code": 1001, "status_msg": "failed"}
                    })
                    .to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).expect("response");
            }
            requests
        });
        let registry = registry_with_provider(format!("http://{address}"));
        let service_dir = tempdir().expect("tempdir");
        let mut request = request();
        request.count = 2;

        let result = MinimaxImageAdapter::new()
            .expect("adapter")
            .execute(
                &registry,
                &auth_store(),
                &MediaGenerationService::new(service_dir.path()),
                request,
            )
            .expect("partial generation succeeds");

        assert_eq!(server.join().expect("server").len(), 2);
        assert_eq!(result.job.requested_count, 2);
        assert_eq!(result.job.status, MediaJobStatus::Succeeded);
        assert_eq!(result.job.produced_count(), 1);
        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(std::fs::read(&result.artifacts[0].path).unwrap(), b"image");
        assert!(service_dir.path().join(".puffer/media/images").exists());
    }

    #[test]
    fn minimax_image_rejects_exact_batch_plan_before_creating_job() {
        let registry = registry_with_provider_batch(
            "http://127.0.0.1:9".to_string(),
            MediaBatchDescriptor {
                mode: MediaBatchMode::Exact,
                max_images_per_call: Some(2),
            },
        );
        let service_dir = tempdir().expect("tempdir");
        let mut request = request();
        request.count = 2;

        let error = MinimaxImageAdapter::new()
            .expect("adapter")
            .execute(
                &registry,
                &auth_store(),
                &MediaGenerationService::new(service_dir.path()),
                request,
            )
            .expect_err("MiniMax rejects exact batch plans");

        assert_eq!(
            error.to_string(),
            "MiniMax image generation supports only per-image call plans"
        );
        assert!(!service_dir.path().join(".puffer/media/jobs").exists());
        assert!(!service_dir.path().join(".puffer/media/images").exists());
    }
}
