use super::artifacts::MediaArtifact;
use super::http_support::{
    bearer_token, provider_error_secrets, provider_execution_url, redact_secrets,
    CredentialAliasMode,
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

const DEFAULT_GEMINI_REQUEST_TIMEOUT_MS: u64 = 300_000;
const GEMINI_ALLOWED_REQUEST_FIELDS: &[&str] = &["aspectRatio", "imageSize"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerateContentRequest {
    prompt: String,
    parameters: BTreeMap<String, String>,
}

impl GeminiGenerateContentRequest {
    fn new(prompt: impl Into<String>, parameters: BTreeMap<String, String>) -> Self {
        Self {
            prompt: prompt.into(),
            parameters,
        }
    }

    fn to_body(&self) -> Value {
        let mut image_config = Map::new();
        for (name, value) in &self.parameters {
            image_config.insert(name.clone(), json!(value));
        }

        let mut generation_config = Map::new();
        generation_config.insert("responseModalities".to_string(), json!(["TEXT", "IMAGE"]));
        if !image_config.is_empty() {
            generation_config.insert("imageConfig".to_string(), Value::Object(image_config));
        }

        json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": self.prompt }]
            }],
            "generationConfig": Value::Object(generation_config)
        })
    }
}

/// Carries an exact Gemini `generateContent` image generation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeminiGenerateContentGenerationRequest {
    pub(crate) provider_id: String,
    pub(crate) model_id: String,
    pub(crate) adapter: String,
    pub(crate) prompt: String,
    pub(crate) parameters: BTreeMap<String, String>,
    pub(crate) count: u8,
}

/// Carries persisted media records created by the Gemini `generateContent` adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeminiGenerateContentGenerationResult {
    pub(crate) job: MediaJob,
    pub(crate) artifacts: Vec<MediaArtifact>,
}

/// Executes descriptor-driven Gemini native `generateContent` image generation.
#[derive(Debug, Clone)]
pub(crate) struct GeminiGenerateContentAdapter {
    client: Client,
}

impl GeminiGenerateContentAdapter {
    /// Creates an adapter with a default blocking HTTP client.
    pub(crate) fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_millis(DEFAULT_GEMINI_REQUEST_TIMEOUT_MS))
            .build()
            .context("build Gemini image generation HTTP client")?;
        Ok(Self { client })
    }

    /// Executes an exact Gemini image request and persists job/artifact sidecars.
    pub(crate) fn execute(
        &self,
        registry: &ProviderRegistry,
        auth_store: &AuthStore,
        service: &MediaGenerationService,
        request: GeminiGenerateContentGenerationRequest,
    ) -> Result<GeminiGenerateContentGenerationResult> {
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
            bail!("Gemini generateContent image generation supports only per-image call plans");
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
                .unwrap_or_else(|| "Gemini image generation produced no images".to_string());
            job.error = Some(error.clone());
            job.transition(MediaJobStatus::Failed, now_ms())?;
            service.save_job(&job)?;
            bail!(error);
        }

        let mut artifacts = Vec::new();
        for (index, output) in outputs.into_iter().enumerate() {
            let artifact_id = Uuid::new_v4().to_string();
            let extension = extension_for_output(&output);
            let filename = format!("image.{extension}");
            let artifact_path =
                service.write_image_artifact_bytes(&artifact_id, &filename, &output.bytes)?;
            let mime_type = output_mime_type(&output).to_string();
            let artifact = MediaArtifact {
                id: artifact_id.clone(),
                job_id: job_id.clone(),
                kind: MediaKind::Image,
                path: artifact_path.clone(),
                mime_type: mime_type.clone(),
                byte_count: output.bytes.len() as u64,
                metadata: artifact_metadata(
                    &request,
                    &artifact_path,
                    &output,
                    &mime_type,
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

        Ok(GeminiGenerateContentGenerationResult { job, artifacts })
    }

    fn request_image(
        &self,
        provider: &ProviderDescriptor,
        auth_store: &AuthStore,
        execution: &MediaExecutionDescriptor,
        request: &GeminiGenerateContentGenerationRequest,
        parameters: BTreeMap<String, String>,
    ) -> Result<GeminiOutput> {
        let url = gemini_execution_url(provider, execution, &request.model_id)?;
        let secrets = provider_error_secrets(provider, auth_store, CredentialAliasMode::Strict);
        let body = GeminiGenerateContentRequest::new(&request.prompt, parameters).to_body();
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
            .context("send Gemini image generation request")?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| anyhow!("{}", redact_secrets(&error.to_string(), &secrets)))
            .context("read Gemini image generation response")?;
        if !status.is_success() {
            bail!(
                "Gemini image generation failed with status {}: {}",
                status.as_u16(),
                redact_secrets(&body, &secrets)
            );
        }
        let value: Value =
            serde_json::from_str(&body).context("parse Gemini image generation response")?;
        image_output_from_response(&value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GeminiOutput {
    bytes: Vec<u8>,
    mime_type: Option<String>,
    response_id: Option<String>,
    model_version: Option<String>,
}

fn validated_request_fields(
    parameters: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    for request_field in parameters.keys() {
        if !GEMINI_ALLOWED_REQUEST_FIELDS.contains(&request_field.as_str()) {
            bail!("Gemini image generation request field unsupported: {request_field}");
        }
    }
    Ok(parameters.clone())
}

fn gemini_execution_url(
    provider: &ProviderDescriptor,
    execution: &MediaExecutionDescriptor,
    model_id: &str,
) -> Result<reqwest::Url> {
    let mut execution = execution.clone();
    execution.path = execution
        .path
        .replace("{model}", model_id)
        .replace("%7Bmodel%7D", model_id);
    provider_execution_url(provider, &execution, "Gemini image generation")
}

fn image_output_from_response(value: &Value) -> Result<GeminiOutput> {
    let response_id = value
        .get("responseId")
        .or_else(|| value.get("response_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let model_version = value
        .get("modelVersion")
        .or_else(|| value.get("model_version"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    for part in candidate_parts(value) {
        let Some(inline_data) = part.get("inlineData").or_else(|| part.get("inline_data")) else {
            continue;
        };
        let Some(encoded) = inline_data.get("data").and_then(Value::as_str) else {
            continue;
        };
        let bytes = BASE64_STANDARD
            .decode(encoded.trim())
            .context("decode Gemini inlineData image")?;
        let mime_type = inline_data
            .get("mimeType")
            .or_else(|| inline_data.get("mime_type"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        return Ok(GeminiOutput {
            bytes,
            mime_type,
            response_id,
            model_version,
        });
    }
    bail!("Gemini image generation response did not contain inline image data")
}

fn candidate_parts(value: &Value) -> Vec<&Value> {
    value
        .get("candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|candidate| candidate.get("content"))
        .filter_map(|content| content.get("parts"))
        .filter_map(Value::as_array)
        .flatten()
        .collect()
}

fn artifact_metadata(
    request: &GeminiGenerateContentGenerationRequest,
    path: &std::path::Path,
    output: &GeminiOutput,
    mime_type: &str,
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
        "mimeType": mime_type,
        "localPath": path,
        "byteCount": output.bytes.len() as u64,
        "createdAtMs": created_at_ms,
    });
    if let Some(response_id) = &output.response_id {
        metadata["responseId"] = json!(response_id);
    }
    if let Some(model_version) = &output.model_version {
        metadata["modelVersion"] = json!(model_version);
    }
    metadata
}

fn output_mime_type(output: &GeminiOutput) -> &'static str {
    match output.mime_type.as_deref().map(str::trim) {
        Some("image/jpeg") | Some("image/jpg") => "image/jpeg",
        Some("image/webp") => "image/webp",
        Some("image/png") => "image/png",
        _ => detect_image_mime_type(&output.bytes).unwrap_or("image/png"),
    }
}

fn extension_for_output(output: &GeminiOutput) -> &'static str {
    match output_mime_type(output) {
        "image/jpeg" => "jpeg",
        "image/webp" => "webp",
        _ => "png",
    }
}

fn detect_image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && bytes[0..4] == *b"RIFF" && bytes[8..12] == *b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
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
        MediaExecutionKind, MediaKindDescriptor, MediaModelDescriptor, MediaOperation,
        ModelDescriptor, ProviderMediaDescriptor, Variant, Variants, WireType,
    };
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use tempfile::tempdir;

    fn registry(base_url: String) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        let native_base_url = base_url.trim_end_matches('/').to_string();
        let openai_base_url = format!("{native_base_url}/v1");
        registry.register(ProviderDescriptor {
            id: "worldrouter".to_string(),
            display_name: "WorldRouter".to_string(),
            base_url: openai_base_url,
            default_api: "openai-completions".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: IndexMap::from([("x-provider-header".to_string(), "present".to_string())]),
            query_params: IndexMap::new(),
            chat_completions_path: None,
            discovery: None,
            media: Some(ProviderMediaDescriptor {
                image: Some(MediaKindDescriptor {
                    discovery: None,
                    execution: None,
                    models: vec![MediaModelDescriptor {
                        id: "gemini-3.1-flash-image-preview".to_string(),
                        display_name: Some("Gemini 3.1 Flash Image Preview".to_string()),
                        max_outputs: Some(3),
                        execution: Some(MediaExecutionDescriptor {
                            adapter: MediaExecutionKind::GeminiGenerateContent,
                            base_url: Some(native_base_url),
                            path: "/v1beta/models/{model}:generateContent".to_string(),
                            batch: MediaBatchDescriptor {
                                mode: MediaBatchMode::PerImage,
                                max_images_per_call: None,
                            },
                            prompt_format: Default::default(),
                        }),
                        operations: vec![MediaOperation::Generate],
                        axes: vec![
                            param_axis("ratio", "Ratio", &["1:1", "16:9"], "1:1", "aspectRatio"),
                            param_axis("mode", "Mode", &["1K", "2K"], "2K", "imageSize"),
                        ],
                        media_map: None,
                        variants: Variants::Single(Variant {
                            model_id: "gemini-3.1-flash-image-preview".to_string(),
                            base_params: BTreeMap::new(),
                        }),
                    }],
                }),
                video: None,
            }),
            models: Vec::<ModelDescriptor>::new(),
        });
        registry
    }

    fn param_axis(
        id: &str,
        label: &str,
        values: &[&str],
        default: &str,
        request_field: &str,
    ) -> Axis {
        Axis {
            id: id.to_string(),
            label: label.to_string(),
            role: AxisRole::Param,
            control: ControlKind::Enum {
                values: values.iter().map(|v| v.to_string()).collect(),
                default: default.to_string(),
            },
            request_field: Some(request_field.to_string()),
            wire_type: WireType::String,
        }
    }

    fn auth_store() -> AuthStore {
        let mut auth = AuthStore::default();
        auth.set_api_key("worldrouter", "sk-worldrouter");
        auth
    }

    fn request() -> GeminiGenerateContentGenerationRequest {
        GeminiGenerateContentGenerationRequest {
            provider_id: "worldrouter".to_string(),
            model_id: "gemini-3.1-flash-image-preview".to_string(),
            adapter: "gemini_generate_content".to_string(),
            prompt: "draw a precise red star".to_string(),
            parameters: BTreeMap::from([
                ("aspectRatio".to_string(), "1:1".to_string()),
                ("imageSize".to_string(), "2K".to_string()),
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
    fn gemini_generate_content_posts_request_and_decodes_inline_image() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let request_text = read_http_request(&mut stream);
            let body = json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "inlineData": {
                                "mimeType": "image/png",
                                "data": "aW1hZ2UtYnl0ZXM="
                            }
                        }]
                    }
                }],
                "modelVersion": "gemini-3.1-flash-image-preview",
                "responseId": "response-test"
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
        let registry = registry(format!("http://{address}"));
        let service_dir = tempdir().expect("tempdir");

        let result = GeminiGenerateContentAdapter::new()
            .expect("adapter")
            .execute(
                &registry,
                &auth_store(),
                &MediaGenerationService::new(service_dir.path()),
                request(),
            )
            .expect("generation succeeds");

        let request_text = server.join().expect("server");
        assert!(request_text
            .starts_with("POST /v1beta/models/gemini-3.1-flash-image-preview:generateContent "));
        assert!(request_text.contains("authorization: Bearer sk-worldrouter"));
        assert!(request_text.contains("x-provider-header: present"));
        assert!(request_text.contains("\"responseModalities\":[\"TEXT\",\"IMAGE\"]"));
        assert!(request_text.contains("\"aspectRatio\":\"1:1\""));
        assert!(request_text.contains("\"imageSize\":\"2K\""));
        assert_eq!(
            std::fs::read(&result.artifacts[0].path).unwrap(),
            b"image-bytes"
        );
        assert_eq!(result.artifacts[0].mime_type, "image/png");
        assert_eq!(
            result.artifacts[0].metadata["adapter"],
            "gemini_generate_content"
        );
        assert_eq!(result.artifacts[0].metadata["responseId"], "response-test");
    }

    #[test]
    fn gemini_generate_content_rejects_unsupported_request_fields() {
        let registry = registry("http://127.0.0.1:9".to_string());
        let service_dir = tempdir().expect("tempdir");
        let mut request = request();
        request
            .parameters
            .insert("quality".to_string(), "high".to_string());

        let error = GeminiGenerateContentAdapter::new()
            .expect("adapter")
            .execute(
                &registry,
                &auth_store(),
                &MediaGenerationService::new(service_dir.path()),
                request,
            )
            .expect_err("unsupported parameter is rejected");

        assert_eq!(
            error.to_string(),
            "Gemini image generation request field unsupported: quality"
        );
        assert!(!service_dir.path().join(".puffer/media/jobs").exists());
    }
}
