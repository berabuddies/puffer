use crate::media::chat_image_output::{ChatImageOutputAdapter, ChatImageOutputGenerationRequest};
use crate::media::discovery::TrustedImageDiscoveryClient;
use crate::media::gemini_generate_content::{
    GeminiGenerateContentAdapter, GeminiGenerateContentGenerationRequest,
};
use crate::media::images_json::{ImagesJsonAdapter, ImagesJsonGenerationRequest};
use crate::media::minimax_image::{MinimaxImageAdapter, MinimaxImageGenerationRequest};
use crate::media::planner::validate_image_generation_count;
use crate::media::resolver::{
    resolve_media_capabilities, resolve_media_request, MediaDiscoveryCache,
};
use crate::media::{MediaArtifact, MediaGenerationService, MediaJob, MediaJobStatus, MediaKind};
use crate::{MediaFailureContext, MediaFailureDiagnostic};
use anyhow::{bail, Context, Result};
use puffer_provider_registry::{AuthStore, Axis, MediaOperation, ProviderRegistry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::artifacts::media_artifact_remote_source_url;
pub use crate::artifacts::{
    generated_media_attachment_metadata, generated_media_attachment_metadata_with_fallback,
    generated_media_timeline_attachments, generated_video_access_metadata_by_artifact,
    read_generated_media_preview_by_artifact, GeneratedMediaAttachmentMetadata,
    GeneratedMediaPreviewResult, GeneratedMediaTimelineAttachment,
    GeneratedMediaTimelineAttachmentKind, GeneratedVideoAccessMetadata,
    GeneratedVideoAccessMetadataResult,
};
pub use crate::internal_tools::{
    generated_media_internal_bash_output, generated_media_internal_command_kind,
    GeneratedMediaInternalCommandKind,
};
use crate::video::generate_exact_video_from_media_request;

/// Default TTL for trusted media discovery results.
pub const MEDIA_DISCOVERY_TTL_MS: u64 = 5 * 60 * 1_000;

/// Describes one exact media capability suitable for client display.
///
/// Carries the typed user-facing `axes`; derives only `PartialEq` because
/// `Axis` → `ControlKind::Range` holds `f64`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCapabilityView {
    pub provider_id: String,
    pub provider_display_name: String,
    pub model_id: String,
    pub model_display_name: String,
    pub kind: String,
    pub operation: String,
    pub adapter: String,
    pub axes: Vec<Axis>,
    pub status: String,
    pub source: String,
    pub reason: Option<String>,
    pub checked_at_ms: u64,
}

/// Carries an exact image generation request from UI or tool configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactImageGenerationRequest {
    pub provider_id: String,
    /// Logical model id; resolved to a concrete upstream model at use time.
    pub model_id: String,
    pub prompt: String,
    /// Per-axis selections; resolved to request parameters at use time.
    pub parameters: BTreeMap<String, String>,
    pub count: Option<u8>,
}

/// Carries an exact media generation request from UI or tool configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactMediaGenerationRequest {
    pub kind: String,
    pub provider_id: String,
    /// Logical model id; resolved to a concrete upstream model at use time.
    pub model_id: String,
    pub operation: String,
    pub prompt: String,
    pub image_references: Vec<String>,
    /// Per-axis selections; resolved to request parameters at use time.
    pub parameters: BTreeMap<String, String>,
    pub count: Option<u8>,
}

/// Carries one persisted generated image artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactGeneratedArtifact {
    pub artifact_id: String,
    pub index: usize,
    pub path: PathBuf,
    pub mime_type: String,
    pub byte_count: u64,
    pub remote_source_url: Option<String>,
}

/// Carries the persisted job and artifacts produced by exact image generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactImageGenerationResult {
    pub job_id: String,
    pub requested_count: u8,
    pub artifacts: Vec<ExactGeneratedArtifact>,
    pub provider_id: String,
    pub model_id: String,
    pub status: String,
}

/// Carries the persisted job and artifacts produced by exact media generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactMediaGenerationResult {
    pub job_id: String,
    pub requested_count: u8,
    pub artifacts: Vec<ExactGeneratedArtifact>,
    pub kind: String,
    pub provider_id: String,
    pub model_id: String,
    pub status: String,
    pub provider_job_id: Option<String>,
    pub remote_status: Option<String>,
    pub error: Option<String>,
    pub diagnostic: Option<MediaFailureDiagnostic>,
}

/// Carries trusted media discovery results used by capability resolution.
#[derive(Debug, Clone)]
pub struct ExactMediaDiscoveryCache {
    pub(crate) inner: MediaDiscoveryCache,
    cached_at_ms: u64,
}

impl ExactMediaDiscoveryCache {
    /// Creates an empty media discovery cache.
    pub fn empty() -> Self {
        Self {
            inner: MediaDiscoveryCache::default(),
            cached_at_ms: 0,
        }
    }

    /// Returns the time at which this cache was refreshed.
    pub fn cached_at_ms(&self) -> u64 {
        self.cached_at_ms
    }

    /// Returns whether this cache is fresh at the given timestamp.
    pub fn is_fresh_at(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.cached_at_ms) <= MEDIA_DISCOVERY_TTL_MS
    }

    #[cfg(test)]
    pub(crate) fn from_inner_for_test(inner: MediaDiscoveryCache, cached_at_ms: u64) -> Self {
        Self {
            inner,
            cached_at_ms,
        }
    }
}

/// Lists exact media capabilities using static descriptors and trusted discovery cache entries.
pub fn list_exact_media_capabilities_with_cache(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    kind_filter: Option<&str>,
    discovery_cache: &ExactMediaDiscoveryCache,
) -> Vec<MediaCapabilityView> {
    let checked_at_ms = now_ms();
    let mut capabilities = Vec::new();
    if kind_filter_matches(kind_filter, "image") {
        capabilities.extend(resolve_media_capabilities(
            registry,
            auth_store,
            MediaKind::Image,
            MediaOperation::Generate,
            checked_at_ms,
            &discovery_cache.inner,
        ));
    }
    if kind_filter_matches(kind_filter, "video") {
        capabilities.extend(resolve_media_capabilities(
            registry,
            auth_store,
            MediaKind::Video,
            MediaOperation::Generate,
            checked_at_ms,
            &discovery_cache.inner,
        ));
    }
    capabilities
        .into_iter()
        .map(MediaCapabilityView::from)
        .collect()
}

/// Refreshes trusted media discovery cache entries for connected providers.
pub fn discover_exact_media_capabilities(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
) -> ExactMediaDiscoveryCache {
    let inner = TrustedImageDiscoveryClient::new().discover(registry, auth_store);
    ExactMediaDiscoveryCache {
        inner,
        cached_at_ms: now_ms(),
    }
}

/// Generates one exact image using static descriptors plus trusted discovery cache entries.
pub fn generate_exact_image_with_cache(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    workspace_root: &Path,
    request: ExactImageGenerationRequest,
    discovery_cache: &ExactMediaDiscoveryCache,
) -> Result<ExactImageGenerationResult> {
    // `request.model_id` is the logical model id and `request.parameters` are
    // the user's axis selections; resolve them into the concrete upstream model
    // id, adapter, and request-field-keyed parameters before dispatch.
    let parameters = image_parameters_with_count_override(request.parameters, request.count)?;
    let resolved = resolve_media_request(
        registry,
        auth_store,
        &request.provider_id,
        &request.model_id,
        MediaKind::Image,
        &parameters,
        &discovery_cache.inner,
    )?;
    validate_image_generation_count(resolved.count)?;
    let service = MediaGenerationService::new(workspace_root);
    match resolved.adapter.as_str() {
        "images_json" => {
            let result = ImagesJsonAdapter::new()?.execute(
                registry,
                auth_store,
                &service,
                ImagesJsonGenerationRequest {
                    provider_id: resolved.provider_id,
                    model_id: resolved.model_id,
                    adapter: resolved.adapter,
                    prompt: request.prompt,
                    parameters: resolved.parameters,
                    count: resolved.count,
                },
            )?;
            Ok(exact_generation_result(result.job, result.artifacts))
        }
        "minimax_image" => {
            let result = MinimaxImageAdapter::new()?.execute(
                registry,
                auth_store,
                &service,
                MinimaxImageGenerationRequest {
                    provider_id: resolved.provider_id,
                    model_id: resolved.model_id,
                    adapter: resolved.adapter,
                    prompt: request.prompt,
                    parameters: resolved.parameters,
                    count: resolved.count,
                },
            )?;
            Ok(exact_generation_result(result.job, result.artifacts))
        }
        "gemini_generate_content" => {
            let result = GeminiGenerateContentAdapter::new()?.execute(
                registry,
                auth_store,
                &service,
                GeminiGenerateContentGenerationRequest {
                    provider_id: resolved.provider_id,
                    model_id: resolved.model_id,
                    adapter: resolved.adapter,
                    prompt: request.prompt,
                    parameters: resolved.parameters,
                    count: resolved.count,
                },
            )?;
            Ok(exact_generation_result(result.job, result.artifacts))
        }
        "chat_image_output" => {
            let result = ChatImageOutputAdapter::new()?.execute_with_discovery_cache(
                registry,
                auth_store,
                &service,
                ChatImageOutputGenerationRequest {
                    provider_id: resolved.provider_id,
                    model_id: resolved.model_id,
                    adapter: resolved.adapter,
                    prompt: request.prompt,
                    parameters: resolved.parameters,
                    count: resolved.count,
                },
                &discovery_cache.inner,
            )?;
            Ok(exact_generation_result(result.job, result.artifacts))
        }
        adapter => bail!("image media adapter unavailable for {adapter}"),
    }
}

/// Generates exact media using static descriptors plus trusted discovery cache entries.
pub fn generate_exact_media_with_cache(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    workspace_root: &Path,
    request: ExactMediaGenerationRequest,
    discovery_cache: &ExactMediaDiscoveryCache,
) -> Result<ExactMediaGenerationResult> {
    match request.kind.trim() {
        "image" => generate_exact_image_from_media_request(
            registry,
            auth_store,
            workspace_root,
            request,
            discovery_cache,
        ),
        "video" => generate_exact_video_from_media_request(
            registry,
            auth_store,
            workspace_root,
            request,
            discovery_cache,
        ),
        kind => bail!("unsupported media kind `{kind}`"),
    }
}

fn generate_exact_image_from_media_request(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    workspace_root: &Path,
    request: ExactMediaGenerationRequest,
    discovery_cache: &ExactMediaDiscoveryCache,
) -> Result<ExactMediaGenerationResult> {
    parse_media_operation(&request.operation)?;
    let result = generate_exact_image_with_cache(
        registry,
        auth_store,
        workspace_root,
        ExactImageGenerationRequest {
            provider_id: request.provider_id,
            model_id: request.model_id,
            prompt: request.prompt,
            parameters: request.parameters,
            count: request.count,
        },
        discovery_cache,
    )?;
    Ok(ExactMediaGenerationResult {
        job_id: result.job_id,
        requested_count: result.requested_count,
        artifacts: result.artifacts,
        kind: "image".to_string(),
        provider_id: result.provider_id,
        model_id: result.model_id,
        status: result.status,
        provider_job_id: None,
        remote_status: None,
        error: None,
        diagnostic: None,
    })
}

fn image_parameters_with_count_override(
    mut parameters: BTreeMap<String, String>,
    count: Option<u8>,
) -> Result<BTreeMap<String, String>> {
    if let Some(count) = count {
        validate_image_generation_count(count)?;
        parameters.insert("output".to_string(), count.to_string());
    }
    Ok(parameters)
}

pub(crate) fn parse_media_operation(operation: &str) -> Result<MediaOperation> {
    match operation.trim() {
        "generate" => Ok(MediaOperation::Generate),
        operation => bail!("unsupported media operation `{operation}`"),
    }
}

/// Resolves a logical image selection into concrete upstream request
/// parameters (keyed by upstream request field, defaults applied).
pub fn resolved_exact_image_parameters_with_cache(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    selection: &ExactImageGenerationRequest,
    discovery_cache: &ExactMediaDiscoveryCache,
) -> Result<BTreeMap<String, String>> {
    let parameters =
        image_parameters_with_count_override(selection.parameters.clone(), selection.count)?;
    let resolved = resolve_media_request(
        registry,
        auth_store,
        &selection.provider_id,
        &selection.model_id,
        MediaKind::Image,
        &parameters,
        &discovery_cache.inner,
    )?;
    Ok(resolved.parameters)
}

fn exact_generation_result(
    job: MediaJob,
    artifacts: Vec<MediaArtifact>,
) -> ExactImageGenerationResult {
    let artifacts = exact_generated_artifacts(artifacts);
    ExactImageGenerationResult {
        job_id: job.id,
        requested_count: job.requested_count,
        artifacts,
        provider_id: job.provider_id,
        model_id: job.model_id,
        status: media_job_status_name(job.status).to_string(),
    }
}

pub(crate) fn exact_media_generation_result(
    job: MediaJob,
    artifacts: Vec<MediaArtifact>,
) -> ExactMediaGenerationResult {
    let artifacts = exact_generated_artifacts(artifacts);
    let diagnostic = if job.status == MediaJobStatus::Failed {
        job.error.as_ref().map(|error| {
            let mut context =
                MediaFailureContext::new(media_kind_name(job.kind), job.provider_id.clone())
                    .model(job.model_id.clone())
                    .phase("poll");
            if let Some(adapter) = &job.adapter {
                context = context.adapter(adapter.clone());
            }
            if let Some(provider_job_id) = &job.provider_job_id {
                context = context.provider_job_id(provider_job_id.clone());
            }
            if let Some(remote_status) = &job.remote_status {
                context = context.remote_status(remote_status.clone());
            }
            MediaFailureDiagnostic::from_message(context, error.clone())
        })
    } else {
        None
    };
    ExactMediaGenerationResult {
        job_id: job.id,
        requested_count: job.requested_count,
        artifacts,
        kind: media_kind_name(job.kind).to_string(),
        provider_id: job.provider_id,
        model_id: job.model_id,
        status: media_job_status_name(job.status).to_string(),
        provider_job_id: job.provider_job_id,
        remote_status: job.remote_status,
        error: job.error,
        diagnostic,
    }
}

fn exact_generated_artifacts(artifacts: Vec<MediaArtifact>) -> Vec<ExactGeneratedArtifact> {
    artifacts
        .into_iter()
        .enumerate()
        .map(|(index, artifact)| {
            let remote_source_url = media_artifact_remote_source_url(&artifact);
            ExactGeneratedArtifact {
                artifact_id: artifact.id,
                index,
                path: artifact.path,
                mime_type: artifact.mime_type,
                byte_count: artifact.byte_count,
                remote_source_url,
            }
        })
        .collect()
}

pub(crate) fn load_media_job_artifacts(
    service: &MediaGenerationService,
    job: &MediaJob,
) -> Result<Vec<MediaArtifact>> {
    job.artifact_ids
        .iter()
        .map(|artifact_id| {
            service
                .load_artifact(artifact_id)
                .with_context(|| format!("load generated media artifact {artifact_id}"))
        })
        .collect()
}

impl From<crate::media::capabilities::MediaCapability> for MediaCapabilityView {
    fn from(capability: crate::media::capabilities::MediaCapability) -> Self {
        Self {
            provider_id: capability.provider_id,
            provider_display_name: capability.provider_display_name,
            model_id: capability.model_id,
            model_display_name: capability.model_display_name,
            kind: media_kind_name(capability.kind).to_string(),
            operation: capability.operation,
            adapter: capability.adapter,
            // `variants` (upstream model ids + base params) stays server-side:
            // the UI renders from each axis's `control`, not the variant table.
            axes: capability.axes,
            status: capability.status,
            source: capability.source,
            reason: capability.reason,
            checked_at_ms: capability.checked_at_ms,
        }
    }
}

fn kind_filter_matches(kind_filter: Option<&str>, kind: &str) -> bool {
    kind_filter
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none_or(|value| value == kind)
}

fn media_kind_name(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "image",
        MediaKind::Video => "video",
    }
}

fn media_job_status_name(status: MediaJobStatus) -> &'static str {
    match status {
        MediaJobStatus::Queued => "queued",
        MediaJobStatus::Running => "running",
        MediaJobStatus::Succeeded => "succeeded",
        MediaJobStatus::Failed => "failed",
        MediaJobStatus::Canceled => "canceled",
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "generated_preview_tests.rs"]
mod generated_preview_tests;

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
