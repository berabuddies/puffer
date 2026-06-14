use super::artifacts::MediaArtifactPreview;
use super::video_poster::{
    extract_video_poster, VideoPosterExtraction, VideoPosterExtractionRequest, VideoPosterOptions,
};
use super::{MediaArtifact, MediaGenerationService, MediaJob, MediaJobStatus, MediaKind};
use crate::{media_failure_error, MediaFailureContext};
use anyhow::{anyhow, Context, Result};
use serde_json::json;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

const VIDEO_MIME_TYPE: &str = "video/mp4";

/// Shared bounded polling configuration for async video adapters.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VideoPollingConfig {
    pub(crate) max_attempts: usize,
    pub(crate) delay: Duration,
}

impl Default for VideoPollingConfig {
    fn default() -> Self {
        Self {
            max_attempts: 200,
            delay: Duration::from_millis(3_000),
        }
    }
}

/// Describes a completed provider video task ready for local artifact storage.
pub(crate) struct CompletedVideoTask<'a> {
    pub(crate) provider_id: &'a str,
    pub(crate) task_id: &'a str,
    pub(crate) remote_status: &'a str,
    pub(crate) video_url: Option<&'a str>,
    pub(crate) filename_prefix: &'static str,
    pub(crate) missing_url_message: &'static str,
}

/// Builds a task poll URL by appending the provider task id to the submit URL.
pub(crate) fn video_poll_url(submit_url: &str, job: &MediaJob) -> Result<String> {
    let id = job
        .provider_job_id
        .as_ref()
        .context("video job is missing a task id")?;
    let mut url = reqwest::Url::parse(submit_url).context("video submit URL must be absolute")?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow!("video submit URL cannot be a base"))?;
        segments.push(id);
    }
    Ok(url.to_string())
}

/// Maps a provider/gateway task-status string onto a media job status.
///
/// Covers the New API gateway lifecycle (`NOT_START`/`SUBMITTED`/`QUEUED`/
/// `IN_PROGRESS`/`SUCCESS`/`FAILURE`) and ModelArk native vocabulary. This is
/// intentionally infallible: any unrecognized status (including `UNKNOWN` and
/// future strings) maps to a non-terminal `Running` so polling keeps waiting
/// instead of hard-failing the job.
pub(crate) fn map_video_task_status(raw: &str) -> MediaJobStatus {
    match raw.trim().to_ascii_lowercase().as_str() {
        "not_start" | "submitted" | "queued" | "pending" => MediaJobStatus::Queued,
        "in_progress" | "running" | "processing" => MediaJobStatus::Running,
        "success" | "succeeded" | "completed" => MediaJobStatus::Succeeded,
        "failure" | "failed" | "error" | "expired" => MediaJobStatus::Failed,
        "cancelled" | "canceled" => MediaJobStatus::Canceled,
        _ => MediaJobStatus::Running,
    }
}

/// Builds provider request context for video adapter failures.
pub(crate) fn video_request_failure_context(
    provider_id: &str,
    adapter_id: &'static str,
    model_id: &str,
    phase: &str,
) -> MediaFailureContext {
    MediaFailureContext::new("video", provider_id.to_string())
        .adapter(adapter_id)
        .model(model_id.to_string())
        .phase(phase)
}

/// Builds provider job context for video adapter failures.
pub(crate) fn video_job_failure_context(
    provider_id: &str,
    adapter_id: &'static str,
    job: &MediaJob,
    phase: &str,
) -> MediaFailureContext {
    let mut context = video_request_failure_context(provider_id, adapter_id, &job.model_id, phase);
    if let Some(provider_job_id) = &job.provider_job_id {
        context = context.provider_job_id(provider_job_id.clone());
    }
    if let Some(remote_status) = &job.remote_status {
        context = context.remote_status(remote_status.clone());
    }
    context
}

/// Wraps a request-scoped video adapter failure with structured diagnostics.
pub(crate) fn wrap_video_request_error(
    provider_id: &str,
    adapter_id: &'static str,
    model_id: &str,
    phase: &'static str,
    error: anyhow::Error,
) -> anyhow::Error {
    media_failure_error(
        video_request_failure_context(provider_id, adapter_id, model_id, phase),
        error.context(format!(
            "provider={provider_id} adapter={adapter_id} phase={phase}"
        )),
    )
}

/// Records a transient poll error on the job without changing its status, then
/// persists it. The polling loop sees a non-terminal job and retries within its
/// attempt budget, so a single network/parse hiccup never aborts the whole job.
pub(crate) fn record_transient_poll_error(
    service: &MediaGenerationService,
    mut job: MediaJob,
    error: anyhow::Error,
    now_ms: u64,
) -> Result<MediaJob> {
    job.error = Some(format!("{error:#}"));
    job.updated_at_ms = now_ms;
    service.save_job(&job)?;
    Ok(job)
}

/// Polls a video job until it reaches a terminal state or exhausts attempts.
pub(crate) fn poll_video_until_terminal(
    mut job: MediaJob,
    config: VideoPollingConfig,
    mut sleep: impl FnMut(Duration),
    mut now_ms: impl FnMut() -> u64,
    mut poll_once: impl FnMut(MediaJob, u64) -> Result<MediaJob>,
) -> Result<MediaJob> {
    for attempt in 0..config.max_attempts {
        job = poll_once(job, now_ms())?;
        if job.status.is_terminal() {
            return Ok(job);
        }
        if attempt + 1 < config.max_attempts {
            sleep(config.delay);
        }
    }
    anyhow::bail!(
        "video job `{}` did not reach a terminal status after {} polls",
        job.id,
        config.max_attempts
    )
}

/// Marks a media job failed, stores its diagnostic, and persists it.
pub(crate) fn persist_failed_video_job(
    service: &MediaGenerationService,
    mut job: MediaJob,
    error: impl Into<String>,
    now_ms: u64,
) -> Result<MediaJob> {
    job.error = Some(error.into());
    job.transition(MediaJobStatus::Failed, now_ms)?;
    service.save_job(&job)?;
    Ok(job)
}

/// Downloads and persists the MP4 artifact for a succeeded video job.
pub(crate) fn complete_video_job(
    service: &MediaGenerationService,
    job: MediaJob,
    task: CompletedVideoTask<'_>,
    now_ms: u64,
    download_bytes: impl FnOnce(&str) -> Result<Vec<u8>>,
) -> Result<MediaJob> {
    complete_video_job_with_poster_extractor(
        service,
        job,
        task,
        now_ms,
        download_bytes,
        |video_path, poster_path| {
            extract_video_poster(VideoPosterExtractionRequest::new(
                video_path,
                poster_path,
                VideoPosterOptions::default(),
            ))
        },
    )
}

/// Downloads and persists an MP4 artifact using an injected poster extractor.
pub(crate) fn complete_video_job_with_poster_extractor(
    service: &MediaGenerationService,
    mut job: MediaJob,
    task: CompletedVideoTask<'_>,
    now_ms: u64,
    download_bytes: impl FnOnce(&str) -> Result<Vec<u8>>,
    extract_poster: impl FnOnce(&Path, &Path) -> VideoPosterExtraction,
) -> Result<MediaJob> {
    if !job.artifact_ids.is_empty() {
        job.transition(MediaJobStatus::Succeeded, now_ms)?;
        service.save_job(&job)?;
        return Ok(job);
    }
    let url = task.video_url.context(task.missing_url_message)?;
    let bytes = match download_bytes(url) {
        Ok(bytes) => bytes,
        Err(error) => {
            persist_failed_video_job(service, job, format!("{error:#}"), now_ms)?;
            return Err(error);
        }
    };
    let artifact_id = Uuid::new_v4().to_string();
    let path = service.write_video_artifact_bytes(
        &artifact_id,
        &format!("{}-{artifact_id}.mp4", task.filename_prefix),
        &bytes,
    )?;
    let poster_path = service.poster_artifact_file_path(&artifact_id)?;
    let preview = match extract_poster(&path, &poster_path) {
        VideoPosterExtraction::Available { byte_count } => {
            MediaArtifactPreview::available_poster(poster_path, byte_count)
        }
        VideoPosterExtraction::Missing { reason } => MediaArtifactPreview::missing_poster(reason),
    };
    let artifact = MediaArtifact {
        id: artifact_id.clone(),
        job_id: job.id.clone(),
        kind: MediaKind::Video,
        path,
        mime_type: VIDEO_MIME_TYPE.to_string(),
        byte_count: bytes.len() as u64,
        metadata: json!({
            "provider": task.provider_id,
            "taskId": task.task_id,
            "remoteStatus": task.remote_status,
        }),
        preview: Some(preview),
        created_at_ms: now_ms,
    };
    service.save_artifact(&artifact)?;
    job.attach_artifact(artifact_id, now_ms);
    job.error = None;
    job.transition(MediaJobStatus::Succeeded, now_ms)?;
    service.save_job(&job)?;
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::super::artifacts::{MediaArtifactPreview, MediaArtifactPreviewState};
    use super::super::video_poster::VideoPosterExtraction;
    use super::*;

    fn running_video_job() -> MediaJob {
        let mut job = MediaJob::new(
            "job-video-1",
            MediaKind::Video,
            "provider-1",
            "model-1",
            "animate",
            1,
            1,
        );
        job.transition(MediaJobStatus::Running, 2).unwrap();
        job
    }

    #[test]
    fn map_video_task_status_covers_new_api_lifecycle() {
        use super::map_video_task_status;
        assert_eq!(map_video_task_status("NOT_START"), MediaJobStatus::Queued);
        assert_eq!(map_video_task_status("submitted"), MediaJobStatus::Queued);
        assert_eq!(
            map_video_task_status("IN_PROGRESS"),
            MediaJobStatus::Running
        );
        assert_eq!(map_video_task_status("SUCCESS"), MediaJobStatus::Succeeded);
        assert_eq!(map_video_task_status("FAILURE"), MediaJobStatus::Failed);
        assert_eq!(map_video_task_status("canceled"), MediaJobStatus::Canceled);
        // Unknown status -> non-terminal (keep polling), never errors.
        assert_eq!(map_video_task_status("unknown"), MediaJobStatus::Running);
        assert_eq!(
            map_video_task_status("totally-new-status"),
            MediaJobStatus::Running
        );
    }

    fn completed_task<'a>() -> CompletedVideoTask<'a> {
        CompletedVideoTask {
            provider_id: "provider-1",
            task_id: "task-1",
            remote_status: "succeeded",
            video_url: Some("https://media.test/generated.mp4"),
            filename_prefix: "provider-video",
            missing_url_message: "missing video URL",
        }
    }

    #[test]
    fn complete_video_job_records_available_poster_preview() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());

        let completed = complete_video_job_with_poster_extractor(
            &service,
            running_video_job(),
            completed_task(),
            3,
            |_| Ok(b"mp4-bytes".to_vec()),
            |video_path, poster_path| {
                assert!(video_path.exists());
                assert_eq!(
                    poster_path.file_name().and_then(|name| name.to_str()),
                    Some("poster.jpg")
                );
                std::fs::create_dir_all(poster_path.parent().unwrap()).unwrap();
                std::fs::write(poster_path, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
                VideoPosterExtraction::Available { byte_count: 4 }
            },
        )
        .unwrap();

        let artifact = service.load_artifact(&completed.artifact_ids[0]).unwrap();
        let preview = artifact
            .preview
            .as_ref()
            .map(MediaArtifactPreview::poster)
            .expect("poster preview");
        assert_eq!(preview.state, MediaArtifactPreviewState::Available);
        assert_eq!(preview.mime_type.as_deref(), Some("image/jpeg"));
        assert_eq!(preview.byte_count, Some(4));
        let expected_poster_path = service.poster_artifact_file_path(&artifact.id).unwrap();
        assert_eq!(
            preview.path.as_deref(),
            Some(expected_poster_path.as_path())
        );
    }

    #[test]
    fn complete_video_job_keeps_video_success_when_poster_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());

        let completed = complete_video_job_with_poster_extractor(
            &service,
            running_video_job(),
            completed_task(),
            3,
            |_| Ok(b"mp4-bytes".to_vec()),
            |_video_path, _poster_path| VideoPosterExtraction::Missing {
                reason: "ffmpeg timed out after 10s".to_string(),
            },
        )
        .unwrap();

        assert_eq!(completed.status, MediaJobStatus::Succeeded);
        let artifact = service.load_artifact(&completed.artifact_ids[0]).unwrap();
        let preview = artifact
            .preview
            .as_ref()
            .map(MediaArtifactPreview::poster)
            .expect("poster preview");
        assert_eq!(preview.state, MediaArtifactPreviewState::Missing);
        assert_eq!(
            preview.reason.as_deref(),
            Some("ffmpeg timed out after 10s")
        );
        assert!(preview.path.is_none());
    }
}
