use crate::media::byteplus_video::{
    byteplus_video_request_from_parameters, BytePlusVideoAdapter, BytePlusVideoPollingConfig,
    BYTEPLUS_VIDEO_ADAPTER,
};
use crate::media::http_support::{
    bearer_token, provider_error_secrets, provider_execution_url, redact_secrets,
    CredentialAliasMode,
};
use crate::media::relaydance_video::{
    relaydance_video_request_from_parameters, RelaydanceVideoAdapter, RelaydanceVideoPollingConfig,
    RELAYDANCE_VIDEO_ADAPTER,
};
use crate::media::replicate_video::{
    ReplicatePollingConfig, ReplicateVideoAdapter, ReplicateVideoRequest,
};
use crate::media::resolver::{
    resolve_media_request, resolve_video_execution_descriptor, ResolvedMediaRequest,
};
use crate::media::worldrouter_video::{
    WorldRouterVideoAdapter, WorldRouterVideoPollingConfig, WorldRouterVideoRequest,
    WORLDROUTER_VIDEO_ADAPTER,
};
use crate::media::{MediaGenerationService, MediaJob, MediaKind};
use crate::runtime::{
    exact_media_generation_result, load_media_job_artifacts, now_ms, parse_media_operation,
    ExactMediaDiscoveryCache, ExactMediaGenerationRequest, ExactMediaGenerationResult,
};
use anyhow::{anyhow, bail, Context, Result};
use puffer_provider_registry::{AuthStore, ProviderRegistry, VideoPromptFormat};
use std::collections::BTreeMap;
use std::path::Path;

/// Executes an exact text-to-video request through the selected video adapter.
///
/// `request.model_id` is the logical model id and `request.parameters` are the
/// user's axis selections (keyed by axis id); both are resolved into a concrete
/// upstream model id, adapter, and request-field-keyed parameters before
/// dispatch.
pub(super) fn generate_exact_video_from_media_request(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    workspace_root: &Path,
    request: ExactMediaGenerationRequest,
    discovery_cache: &ExactMediaDiscoveryCache,
) -> Result<ExactMediaGenerationResult> {
    reclaim_orphaned_video_jobs(registry, auth_store, workspace_root);
    parse_media_operation(&request.operation)?;
    validate_video_count(request.count.unwrap_or(1))?;
    let resolved = resolve_media_request(
        registry,
        auth_store,
        &request.provider_id,
        &request.model_id,
        MediaKind::Video,
        &request.parameters,
        &discovery_cache.inner,
    )?;
    match resolved.adapter.as_str() {
        "replicate_video" => {
            generate_replicate_video(registry, auth_store, workspace_root, &request, &resolved)
        }
        "relaydance_video" => {
            generate_relaydance_video(registry, auth_store, workspace_root, &request, &resolved)
        }
        "byteplus_video" => {
            generate_byteplus_video(registry, auth_store, workspace_root, &request, &resolved)
        }
        "worldrouter_video" => {
            generate_worldrouter_video(registry, auth_store, workspace_root, &request, &resolved)
        }
        adapter => bail!("video media adapter unavailable for {adapter}"),
    }
}

fn generate_replicate_video(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    workspace_root: &Path,
    request: &ExactMediaGenerationRequest,
    resolved: &ResolvedMediaRequest,
) -> Result<ExactMediaGenerationResult> {
    reject_unsupported_video_image_references(&request.provider_id, &request.image_references)?;
    let provider = registry.provider(&request.provider_id).with_context(|| {
        format!(
            "selected video model unavailable: {}/{} via {}",
            request.provider_id, resolved.model_id, resolved.adapter
        )
    })?;
    let api_key = bearer_token(provider, auth_store, CredentialAliasMode::Strict)?
        .context("Replicate API token is required")?;
    let service = MediaGenerationService::new(workspace_root);
    let adapter = ReplicateVideoAdapter::new(api_key)?;
    let video_request = replicate_video_request_from_parameters(
        resolved.model_id.clone(),
        request.prompt.clone(),
        resolved.parameters.clone(),
    )?;
    let job = adapter.submit(&service, video_request, now_ms())?;
    let job = adapter.poll_until_terminal(&service, job, ReplicatePollingConfig::default())?;
    finish_exact_video_job(&service, job)
}

/// Resolves provider + execution + token and constructs a Relaydance video
/// adapter, returning it alongside the secrets to redact from error strings.
/// Shared by generation and orphan reclaim.
fn build_relaydance_adapter(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    provider_id: &str,
    model_id: &str,
    adapter: &str,
) -> Result<(RelaydanceVideoAdapter, VideoPromptFormat, Vec<String>)> {
    let (provider, execution) =
        resolve_video_execution_descriptor(registry, provider_id, model_id, adapter)?;
    let api_key = bearer_token(provider, auth_store, CredentialAliasMode::Strict)?
        .context("Relaydance API key is required")?;
    let secrets = provider_error_secrets(provider, auth_store, CredentialAliasMode::Strict);
    let submit_url = provider_execution_url(provider, &execution, "video task")?;
    let prompt_format = execution.prompt_format;
    let built =
        RelaydanceVideoAdapter::new(api_key, submit_url.to_string(), provider_id.to_string())?;
    Ok((built, prompt_format, secrets))
}

/// Resolves provider + execution + token and constructs a BytePlus video
/// adapter, returning it alongside the secrets to redact from error strings.
/// Shared by generation and orphan reclaim.
fn build_byteplus_adapter(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    provider_id: &str,
    model_id: &str,
    adapter: &str,
) -> Result<(BytePlusVideoAdapter, Vec<String>)> {
    let (provider, execution) =
        resolve_video_execution_descriptor(registry, provider_id, model_id, adapter)?;
    let api_key = bearer_token(provider, auth_store, CredentialAliasMode::Strict)?
        .context("BytePlus API key is required")?;
    let secrets = provider_error_secrets(provider, auth_store, CredentialAliasMode::Strict);
    let submit_url = provider_execution_url(provider, &execution, "video task")?;
    let built =
        BytePlusVideoAdapter::new(api_key, submit_url.to_string(), provider_id.to_string())?;
    Ok((built, secrets))
}

/// Resolves provider + execution + token and constructs a WorldRouter video
/// adapter, returning it alongside the secrets to redact from error strings.
/// Shared by generation and orphan reclaim.
fn build_worldrouter_adapter(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    provider_id: &str,
    model_id: &str,
    adapter: &str,
) -> Result<(WorldRouterVideoAdapter, Vec<String>)> {
    let (provider, execution) =
        resolve_video_execution_descriptor(registry, provider_id, model_id, adapter)?;
    let api_key = bearer_token(provider, auth_store, CredentialAliasMode::Strict)?
        .context("WorldRouter API key is required")?;
    let secrets = provider_error_secrets(provider, auth_store, CredentialAliasMode::Strict);
    let submit_url = provider_execution_url(provider, &execution, "video task")?;
    let built =
        WorldRouterVideoAdapter::new(api_key, submit_url.to_string(), provider_id.to_string())?;
    Ok((built, secrets))
}

/// Best-effort: poll each non-terminal async video job once so orphaned tasks
/// (e.g. the app died mid-render) get reclaimed on the next generation. Never
/// propagates errors — a missing token or offline provider must not block the
/// user's new request. Replicate jobs are excluded: they finish synchronously
/// and leave no orphan to reclaim.
fn reclaim_orphaned_video_jobs(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    workspace_root: &Path,
) {
    let service = MediaGenerationService::new(workspace_root);
    let Ok(jobs) = service.list_video_jobs() else {
        return;
    };
    for job in jobs {
        if job.status.is_terminal() || job.provider_job_id.is_none() {
            continue;
        }
        match job.adapter.as_deref() {
            Some(RELAYDANCE_VIDEO_ADAPTER) => {
                if let Ok((adapter, _format, _secrets)) = build_relaydance_adapter(
                    registry,
                    auth_store,
                    &job.provider_id,
                    &job.model_id,
                    RELAYDANCE_VIDEO_ADAPTER,
                ) {
                    let _ = adapter.poll(&service, job, now_ms());
                }
            }
            Some(BYTEPLUS_VIDEO_ADAPTER) => {
                if let Ok((adapter, _secrets)) = build_byteplus_adapter(
                    registry,
                    auth_store,
                    &job.provider_id,
                    &job.model_id,
                    BYTEPLUS_VIDEO_ADAPTER,
                ) {
                    let _ = adapter.poll(&service, job, now_ms());
                }
            }
            Some(WORLDROUTER_VIDEO_ADAPTER) => {
                if let Ok((adapter, _secrets)) = build_worldrouter_adapter(
                    registry,
                    auth_store,
                    &job.provider_id,
                    &job.model_id,
                    WORLDROUTER_VIDEO_ADAPTER,
                ) {
                    let _ = adapter.poll(&service, job, now_ms());
                }
            }
            _ => {}
        }
    }
}

fn generate_relaydance_video(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    workspace_root: &Path,
    request: &ExactMediaGenerationRequest,
    resolved: &ResolvedMediaRequest,
) -> Result<ExactMediaGenerationResult> {
    reject_unsupported_video_image_references(&request.provider_id, &request.image_references)?;
    let service = MediaGenerationService::new(workspace_root);
    let (adapter, prompt_format, secrets) = build_relaydance_adapter(
        registry,
        auth_store,
        &request.provider_id,
        &resolved.model_id,
        &resolved.adapter,
    )?;
    let video_request = relaydance_video_request_from_parameters(
        resolved.model_id.clone(),
        request.prompt.clone(),
        &resolved.parameters,
        &resolved.parameter_wire_types,
        prompt_format,
    )?;
    let job = adapter
        .submit(
            &service,
            video_request,
            resolved.parameters.clone(),
            now_ms(),
        )
        .map_err(|error| redact_media_error(error, &secrets))?;
    let job = adapter
        .poll_until_terminal(
            &service,
            job,
            RelaydanceVideoPollingConfig::default(),
            std::thread::sleep,
            now_ms,
        )
        .map_err(|error| redact_media_error(error, &secrets))?;
    finish_exact_video_job(&service, job)
}

fn generate_byteplus_video(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    workspace_root: &Path,
    request: &ExactMediaGenerationRequest,
    resolved: &ResolvedMediaRequest,
) -> Result<ExactMediaGenerationResult> {
    let service = MediaGenerationService::new(workspace_root);
    let (adapter, secrets) = build_byteplus_adapter(
        registry,
        auth_store,
        &request.provider_id,
        &resolved.model_id,
        &resolved.adapter,
    )?;
    let video_request = byteplus_video_request_from_parameters(
        resolved.model_id.clone(),
        request.prompt.clone(),
        request.image_references.clone(),
        &resolved.parameters,
    )?;
    let job = adapter
        .submit(
            &service,
            video_request,
            resolved.parameters.clone(),
            now_ms(),
        )
        .map_err(|error| redact_media_error(error, &secrets))?;
    let job = adapter
        .poll_until_terminal(
            &service,
            job,
            BytePlusVideoPollingConfig::default(),
            std::thread::sleep,
            now_ms,
        )
        .map_err(|error| redact_media_error(error, &secrets))?;
    finish_exact_video_job(&service, job)
}

fn generate_worldrouter_video(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    workspace_root: &Path,
    request: &ExactMediaGenerationRequest,
    resolved: &ResolvedMediaRequest,
) -> Result<ExactMediaGenerationResult> {
    let service = MediaGenerationService::new(workspace_root);
    let (adapter, secrets) = build_worldrouter_adapter(
        registry,
        auth_store,
        &request.provider_id,
        &resolved.model_id,
        &resolved.adapter,
    )?;
    let video_request = WorldRouterVideoRequest {
        model: resolved.model_id.clone(),
        prompt: request.prompt.clone(),
        image_references: request.image_references.clone(),
        params: resolved.parameters.clone(),
    };
    let job = adapter
        .submit(
            &service,
            video_request,
            resolved.parameters.clone(),
            now_ms(),
        )
        .map_err(|error| redact_media_error(error, &secrets))?;
    let job = adapter
        .poll_until_terminal(
            &service,
            job,
            WorldRouterVideoPollingConfig::default(),
            std::thread::sleep,
            now_ms,
        )
        .map_err(|error| redact_media_error(error, &secrets))?;
    finish_exact_video_job(&service, job)
}

fn finish_exact_video_job(
    service: &MediaGenerationService,
    job: MediaJob,
) -> Result<ExactMediaGenerationResult> {
    let artifacts = load_media_job_artifacts(service, &job)?;
    Ok(exact_media_generation_result(job, artifacts))
}

fn reject_unsupported_video_image_references(
    provider_id: &str,
    image_references: &[String],
) -> Result<()> {
    if image_references.is_empty() {
        return Ok(());
    }
    bail!("provider {provider_id} does not support video image references")
}

fn redact_media_error(error: anyhow::Error, secrets: &[String]) -> anyhow::Error {
    if let Some(diagnostic) = crate::media_failure_diagnostic(&error) {
        let diagnostic = diagnostic.redact(secrets);
        return anyhow::Error::new(crate::MediaFailureError::new(diagnostic));
    }
    anyhow!("{}", redact_secrets(&format!("{error:#}"), secrets))
}

fn replicate_video_request_from_parameters(
    model_id: String,
    prompt: String,
    parameters: BTreeMap<String, String>,
) -> Result<ReplicateVideoRequest> {
    let aspect_ratio = parameters
        .get("aspect_ratio")
        .cloned()
        .context("video generation parameter unsupported: aspect_ratio=")?;
    let duration_seconds = parameters
        .get("duration")
        .context("video generation parameter unsupported: duration=")?
        .parse::<u32>()
        .context("video generation parameter unsupported: duration")?;
    Ok(ReplicateVideoRequest {
        model: model_id,
        prompt,
        aspect_ratio,
        duration_seconds,
    })
}

fn validate_video_count(count: u8) -> Result<u8> {
    if count == 1 {
        Ok(count)
    } else {
        bail!("video generation count must be 1")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::byteplus_video::tests_support::scripted as byteplus_scripted;
    use crate::media::relaydance_video::tests_support::scripted as relaydance_scripted;
    use crate::media::MediaJobStatus;
    use serde_json::json;

    #[test]
    fn poll_once_reclaims_completed_relaydance_job() {
        let dir = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(dir.path());

        // Pre-seed a video job frozen in queued, carrying a provider_job_id.
        let mut job = MediaJob::new(
            "frozen-1",
            MediaKind::Video,
            "relaydance",
            "seedance-nsfw",
            "p",
            1,
            1,
        );
        job.adapter = Some(RELAYDANCE_VIDEO_ADAPTER.to_string());
        job.provider_job_id = Some("task_done".to_string());
        service.save_job(&job).unwrap();

        // Inject a gateway response that already reports SUCCESS.
        let adapter = RelaydanceVideoAdapter::with_transport(
            "token",
            "https://relaydance.com/v1/video/generations",
            "relaydance",
            relaydance_scripted(
                json!({}),
                vec![json!({ "data": { "id": "task_done", "status": "SUCCESS",
                    "result_url": "https://cdn.example.com/v.mp4" } })],
            ),
        );
        let loaded = service.load_job("frozen-1").unwrap();
        let reclaimed = adapter.poll(&service, loaded, 2).expect("reclaim poll");
        assert_eq!(reclaimed.status, MediaJobStatus::Succeeded);
        assert_eq!(reclaimed.artifact_ids.len(), 1);
    }

    #[test]
    fn poll_once_reclaims_completed_byteplus_job() {
        let dir = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(dir.path());

        // Pre-seed a byteplus video job frozen in queued, carrying a task id.
        let mut job = MediaJob::new(
            "frozen-bp",
            MediaKind::Video,
            "byteplus",
            "dreamina-seedance-2-0-260128",
            "p",
            1,
            1,
        );
        job.adapter = Some(BYTEPLUS_VIDEO_ADAPTER.to_string());
        job.provider_job_id = Some("task_done".to_string());
        service.save_job(&job).unwrap();

        // Inject a ModelArk response that already reports success.
        let adapter = BytePlusVideoAdapter::with_transport(
            "token",
            "https://ark.ap-southeast.bytepluses.com/api/v3/contents/generations/tasks",
            "byteplus",
            byteplus_scripted(
                json!({}),
                vec![json!({ "id": "task_done", "status": "succeeded",
                    "content": { "video_url": "https://cdn.example.com/v.mp4" } })],
            ),
        );
        let loaded = service.load_job("frozen-bp").unwrap();
        let reclaimed = adapter.poll(&service, loaded, 2).expect("reclaim poll");
        assert_eq!(reclaimed.status, MediaJobStatus::Succeeded);
        assert_eq!(reclaimed.artifact_ids.len(), 1);
    }

    #[test]
    fn relaydance_rejects_video_image_references() {
        let workspace = tempfile::tempdir().unwrap();
        let request = ExactMediaGenerationRequest {
            kind: "video".to_string(),
            provider_id: "relaydance".to_string(),
            model_id: "doubao-seedance-2-0-720p".to_string(),
            operation: "generate".to_string(),
            prompt: "animate image 1".to_string(),
            image_references: vec!["https://example.com/person.png".to_string()],
            parameters: BTreeMap::new(),
            count: Some(1),
        };

        let error = generate_relaydance_video(
            &ProviderRegistry::new(),
            &AuthStore::default(),
            workspace.path(),
            &request,
            &ResolvedMediaRequest {
                provider_id: "relaydance".to_string(),
                model_id: "doubao-seedance-2-0-720p".to_string(),
                adapter: RELAYDANCE_VIDEO_ADAPTER.to_string(),
                parameters: BTreeMap::new(),
                parameter_wire_types: BTreeMap::new(),
                count: 1,
            },
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("relaydance"));
        assert!(error.contains("image references"));
    }

    #[test]
    fn replicate_rejects_video_image_references() {
        let workspace = tempfile::tempdir().unwrap();
        let request = ExactMediaGenerationRequest {
            kind: "video".to_string(),
            provider_id: "replicate".to_string(),
            model_id: "owner/model-version".to_string(),
            operation: "generate".to_string(),
            prompt: "animate image 1".to_string(),
            image_references: vec!["https://example.com/person.png".to_string()],
            parameters: BTreeMap::new(),
            count: Some(1),
        };

        let error = generate_replicate_video(
            &ProviderRegistry::new(),
            &AuthStore::default(),
            workspace.path(),
            &request,
            &ResolvedMediaRequest {
                provider_id: "replicate".to_string(),
                model_id: "owner/model-version".to_string(),
                adapter: "replicate_video".to_string(),
                parameters: BTreeMap::new(),
                parameter_wire_types: BTreeMap::new(),
                count: 1,
            },
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("replicate"));
        assert!(error.contains("image references"));
    }
}
