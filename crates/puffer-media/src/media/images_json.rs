use super::artifacts::MediaArtifact;
use super::http_support::{
    bearer_token, download_image_url, provider_error_secrets, provider_execution_url,
    redact_secrets, CredentialAliasMode,
};
use super::jobs::{MediaJob, MediaJobStatus};
use super::planner::{plan_image_generation, ImageGenerationPlan};
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

const DEFAULT_IMAGE_REQUEST_TIMEOUT_MS: u64 = 300_000;
const IMAGES_JSON_ALLOWED_REQUEST_FIELDS: &[&str] = &[
    "model",
    "prompt",
    "size",
    "quality",
    "output_format",
    "response_format",
    "aspect_ratio",
    "resolution",
    "sequential_image_generation",
];

/// Request shape for OpenAI image generation after media settings resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImagesJsonRequest {
    model: String,
    prompt: String,
    parameters: BTreeMap<String, String>,
    count: u8,
}

impl ImagesJsonRequest {
    fn new(
        model: impl Into<String>,
        prompt: impl Into<String>,
        parameters: BTreeMap<String, String>,
        count: u8,
    ) -> Self {
        Self {
            model: model.into(),
            prompt: prompt.into(),
            parameters,
            count,
        }
    }

    fn to_body(&self) -> Value {
        let mut body = Map::new();
        body.insert("model".to_string(), json!(self.model));
        body.insert("prompt".to_string(), json!(self.prompt));
        for (name, value) in &self.parameters {
            if name == "n" {
                continue;
            }
            body.insert(name.clone(), json!(value));
        }
        if self.count > 1 {
            body.insert("n".to_string(), json!(self.count));
        }
        Value::Object(body)
    }
}

/// Carries an exact OpenAI Images-compatible generation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImagesJsonGenerationRequest {
    pub(crate) provider_id: String,
    pub(crate) model_id: String,
    pub(crate) adapter: String,
    pub(crate) prompt: String,
    pub(crate) parameters: BTreeMap<String, String>,
    pub(crate) count: u8,
}

/// Carries persisted media records created by the OpenAI Images adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImagesJsonGenerationResult {
    pub(crate) job: MediaJob,
    pub(crate) artifacts: Vec<MediaArtifact>,
}

/// Executes descriptor-driven OpenAI Images-compatible generation.
#[derive(Debug, Clone)]
pub(crate) struct ImagesJsonAdapter {
    client: Client,
}

impl ImagesJsonAdapter {
    /// Creates an adapter with a default blocking HTTP client.
    pub(crate) fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_millis(DEFAULT_IMAGE_REQUEST_TIMEOUT_MS))
            .build()
            .context("build image generation HTTP client")?;
        Ok(Self { client })
    }

    /// Executes an exact image generation request and persists job/artifact sidecars.
    pub(crate) fn execute(
        &self,
        registry: &ProviderRegistry,
        auth_store: &AuthStore,
        service: &MediaGenerationService,
        request: ImagesJsonGenerationRequest,
    ) -> Result<ImagesJsonGenerationResult> {
        let discovery_cache = MediaDiscoveryCache::default();
        let (provider, execution) = resolve_image_execution_descriptor(
            registry,
            &request.provider_id,
            &request.model_id,
            &request.adapter,
            &discovery_cache,
        )?;
        let plan = plan_image_generation(request.count, &execution.batch)?;
        // `request.parameters` are already resolved and keyed by upstream
        // request field; validate them against the adapter's allowlist.
        let request_parameters = validated_request_fields(&request.parameters)?;

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

        let outputs = match self.request_images(
            provider,
            auth_store,
            &request,
            request_parameters.clone(),
            &execution,
            &plan,
        ) {
            Ok(output) => output,
            Err(error) => {
                job.error = Some(format!("{error:#}"));
                job.transition(MediaJobStatus::Failed, now_ms())?;
                service.save_job(&job)?;
                return Err(error);
            }
        };

        let mut artifacts = Vec::new();
        for (index, output) in outputs.into_iter().enumerate() {
            let artifact_id = Uuid::new_v4().to_string();
            let output_format = resolved_output_format(&request_parameters, &output.bytes);
            let filename = format!("image.{}", extension_for_output_format(&output_format));
            let artifact_path =
                service.write_image_artifact_bytes(&artifact_id, &filename, &output.bytes)?;
            let artifact = MediaArtifact {
                id: artifact_id.clone(),
                job_id: job_id.clone(),
                kind: MediaKind::Image,
                path: artifact_path.clone(),
                mime_type: mime_type_for_output_format(&output_format).to_string(),
                byte_count: output.bytes.len() as u64,
                metadata: artifact_metadata(
                    &request,
                    &request_parameters,
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
        if artifacts.is_empty() {
            job.error = Some("image generation produced no images".to_string());
            job.transition(MediaJobStatus::Failed, now_ms())?;
            service.save_job(&job)?;
            bail!("image generation produced no images");
        }
        job.transition(MediaJobStatus::Succeeded, now_ms())?;
        service.save_job(&job)?;

        Ok(ImagesJsonGenerationResult { job, artifacts })
    }

    fn request_images(
        &self,
        provider: &ProviderDescriptor,
        auth_store: &AuthStore,
        request: &ImagesJsonGenerationRequest,
        parameters: BTreeMap<String, String>,
        execution: &MediaExecutionDescriptor,
        plan: &ImageGenerationPlan,
    ) -> Result<Vec<ImageOutput>> {
        let url = provider_execution_url(provider, execution, "image generation")?;
        let secrets =
            provider_error_secrets(provider, auth_store, CredentialAliasMode::OpenAiCodexAlias);
        let token = bearer_token(provider, auth_store, CredentialAliasMode::OpenAiCodexAlias)?;
        let mut outputs = Vec::new();
        for call in &plan.calls {
            let call_result = (|| -> Result<Vec<ImageOutput>> {
                let body = ImagesJsonRequest::new(
                    &request.model_id,
                    &request.prompt,
                    parameters.clone(),
                    call.requested_count,
                )
                .to_body();
                let mut http = self.client.post(url.clone()).json(&body);
                for (name, value) in &provider.headers {
                    http = http.header(name.as_str(), value.as_str());
                }
                if let Some(token) = &token {
                    http = http.bearer_auth(token);
                }
                let response = http
                    .send()
                    .map_err(|error| anyhow!("{}", redact_secrets(&error.to_string(), &secrets)))
                    .context("send image generation request")?;
                let status = response.status();
                let body = response
                    .text()
                    .map_err(|error| anyhow!("{}", redact_secrets(&error.to_string(), &secrets)))
                    .context("read image generation response")?;
                if !status.is_success() {
                    bail!(
                        "image generation failed with status {}: {}",
                        status.as_u16(),
                        redact_secrets(&body, &secrets)
                    );
                }
                let value: Value =
                    serde_json::from_str(&body).context("parse image generation response")?;
                image_outputs_from_response(&self.client, &value, call.requested_count)
            })();

            match call_result {
                Ok(mut call_outputs) => {
                    let short_response = call_outputs.len() < call.requested_count as usize;
                    outputs.append(&mut call_outputs);
                    if short_response {
                        break;
                    }
                }
                Err(error) => {
                    if outputs.is_empty() {
                        return Err(error);
                    }
                    break;
                }
            }
        }
        Ok(outputs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageOutput {
    bytes: Vec<u8>,
    revised_prompt: Option<String>,
    remote_source_url: Option<String>,
}

fn validated_request_fields(
    parameters: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    for request_field in parameters.keys() {
        if !IMAGES_JSON_ALLOWED_REQUEST_FIELDS.contains(&request_field.as_str()) {
            bail!("image generation request field unsupported: {request_field}");
        }
    }
    Ok(parameters.clone())
}

fn image_outputs_from_response(
    client: &Client,
    value: &Value,
    count: u8,
) -> Result<Vec<ImageOutput>> {
    let Some(items) = value.get("data").and_then(Value::as_array) else {
        bail!("image generation response did not contain an image");
    };
    let requested_count = count as usize;
    let mut outputs = Vec::new();
    for item in items.iter().take(requested_count) {
        outputs.push(image_output_from_item(client, item)?);
    }
    Ok(outputs)
}

fn image_output_from_item(client: &Client, item: &Value) -> Result<ImageOutput> {
    let revised_prompt = item
        .get("revised_prompt")
        .or_else(|| item.get("revisedPrompt"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    if let Some(encoded) = item.get("b64_json").and_then(Value::as_str) {
        let bytes = BASE64_STANDARD
            .decode(encoded.trim())
            .context("decode image b64_json")?;
        return Ok(ImageOutput {
            bytes,
            revised_prompt,
            remote_source_url: None,
        });
    }
    if let Some(url) = item.get("url").and_then(Value::as_str) {
        let bytes = download_image_url(client, url, "image response")?;
        return Ok(ImageOutput {
            bytes,
            revised_prompt,
            remote_source_url: Some(url.to_string()),
        });
    }
    bail!("image generation response did not contain an image")
}

fn artifact_metadata(
    request: &ImagesJsonGenerationRequest,
    parameters: &BTreeMap<String, String>,
    path: &std::path::Path,
    output: &ImageOutput,
    index: usize,
    created_at_ms: u64,
) -> Value {
    let output_format = resolved_output_format(parameters, &output.bytes);
    let mut metadata = json!({
        "providerId": request.provider_id,
        "modelId": request.model_id,
        "adapter": request.adapter,
        "prompt": request.prompt,
        "parameters": parameters,
        "index": index,
        "mimeType": mime_type_for_output_format(&output_format),
        "localPath": path,
        "byteCount": output.bytes.len() as u64,
        "createdAtMs": created_at_ms,
    });
    if let Some(revised_prompt) = &output.revised_prompt {
        metadata["revisedPrompt"] = json!(revised_prompt);
    }
    if let Some(remote_source_url) = &output.remote_source_url {
        metadata["remoteSourceUrl"] = json!(remote_source_url);
    }
    metadata
}

/// Determines the artifact output format from the response bytes, falling back to
/// the declared `output_format` parameter (then PNG) when the bytes carry no
/// recognized image signature. Sniffing the bytes keeps the saved extension and
/// MIME truthful even when the model omits an `output_format` parameter and the
/// provider returns a different format than requested.
fn resolved_output_format(parameters: &BTreeMap<String, String>, bytes: &[u8]) -> String {
    detect_image_format(bytes)
        .map(str::to_string)
        .unwrap_or_else(|| output_format_for_parameters(parameters))
}

/// Recognizes common image formats from their leading magic bytes.
fn detect_image_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpeg")
    } else if bytes.len() >= 12 && bytes[0..4] == *b"RIFF" && bytes[8..12] == *b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn output_format_for_parameters(parameters: &BTreeMap<String, String>) -> String {
    parameters
        .get("output_format")
        .cloned()
        .unwrap_or_else(|| "png".to_string())
}

fn mime_type_for_output_format(format: &str) -> &'static str {
    match format.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

fn extension_for_output_format(format: &str) -> &'static str {
    match format.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "jpeg",
        "webp" => "webp",
        _ => "png",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
#[path = "images_json_tests.rs"]
mod tests;
