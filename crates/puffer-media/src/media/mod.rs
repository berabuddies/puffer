pub(crate) mod artifacts;
pub(crate) mod byteplus_video;
pub(crate) mod capabilities;
pub(crate) mod chat_image_output;
pub(crate) mod discovery;
pub(crate) mod gemini_generate_content;
pub(crate) mod http_support;
pub(crate) mod images_json;
pub(crate) mod jobs;
pub(crate) mod minimax_image;
pub(crate) mod planner;
pub(crate) mod relaydance_video;
pub(crate) mod replicate_video;
pub(crate) mod resolver;
pub(crate) mod video_jobs;
pub(crate) mod video_poster;
pub(crate) mod worldrouter_video;

pub(crate) use artifacts::MediaArtifact;
pub(crate) use capabilities::MediaKind;
pub(crate) use jobs::{MediaJob, MediaJobStatus};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Provides workspace-local media job and artifact persistence helpers.
#[derive(Debug, Clone)]
pub(crate) struct MediaGenerationService {
    workspace_root: PathBuf,
}

impl MediaGenerationService {
    /// Creates a media persistence service rooted in the workspace.
    pub(crate) fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    /// Resolves a safe artifact file path below the service artifact directory.
    pub(crate) fn artifact_file_path(&self, artifact_id: &str, filename: &str) -> Result<PathBuf> {
        validate_simple_id(artifact_id, "artifact id")?;
        validate_artifact_filename(filename)?;
        Ok(self.artifacts_dir().join(artifact_id).join(filename))
    }

    /// Resolves the fixed generated-video poster path below the artifact directory.
    pub(crate) fn poster_artifact_file_path(&self, artifact_id: &str) -> Result<PathBuf> {
        self.artifact_file_path(artifact_id, "poster.jpg")
    }

    /// Resolves a safe generated-image file path below the service image directory.
    pub(crate) fn image_artifact_file_path(
        &self,
        artifact_id: &str,
        filename: &str,
    ) -> Result<PathBuf> {
        validate_simple_id(artifact_id, "artifact id")?;
        validate_artifact_filename(filename)?;
        Ok(self.images_dir().join(artifact_id).join(filename))
    }

    /// Resolves a safe generated-video file path below the service video directory.
    pub(crate) fn video_artifact_file_path(
        &self,
        artifact_id: &str,
        filename: &str,
    ) -> Result<PathBuf> {
        validate_simple_id(artifact_id, "artifact id")?;
        validate_artifact_filename(filename)?;
        Ok(self.videos_dir().join(artifact_id).join(filename))
    }

    /// Writes generated artifact bytes to a safe artifact path.
    #[cfg(test)]
    pub(crate) fn write_artifact_bytes(
        &self,
        artifact_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<PathBuf> {
        let path = self.artifact_file_path(artifact_id, filename)?;
        write_media_bytes(&path, bytes)?;
        Ok(path)
    }

    /// Writes generated image bytes to a safe image artifact path.
    pub(crate) fn write_image_artifact_bytes(
        &self,
        artifact_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<PathBuf> {
        let path = self.image_artifact_file_path(artifact_id, filename)?;
        write_media_bytes(&path, bytes)?;
        Ok(path)
    }

    /// Writes generated video bytes to a safe video artifact path.
    pub(crate) fn write_video_artifact_bytes(
        &self,
        artifact_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<PathBuf> {
        let path = self.video_artifact_file_path(artifact_id, filename)?;
        write_media_bytes(&path, bytes)?;
        Ok(path)
    }

    /// Persists a media job JSON sidecar.
    pub(crate) fn save_job(&self, job: &MediaJob) -> Result<()> {
        validate_simple_id(&job.id, "job id")?;
        write_json_sidecar(&self.job_sidecar_path(&job.id)?, job)
    }

    /// Loads a media job JSON sidecar by id.
    #[cfg(test)]
    pub(crate) fn load_job(&self, job_id: &str) -> Result<MediaJob> {
        read_json_sidecar(&self.job_sidecar_path(job_id)?)
    }

    /// Lists all video media jobs, skipping unreadable or corrupt sidecars.
    pub(crate) fn list_video_jobs(&self) -> Result<Vec<MediaJob>> {
        let dir = self.jobs_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => {
                return Err(err).with_context(|| format!("read media jobs dir {}", dir.display()))
            }
        };
        let mut jobs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Ok(job) = read_json_sidecar::<MediaJob>(&path) {
                if job.kind == MediaKind::Video {
                    jobs.push(job);
                }
            }
        }
        Ok(jobs)
    }

    /// Persists a media artifact JSON sidecar.
    pub(crate) fn save_artifact(&self, artifact: &MediaArtifact) -> Result<()> {
        validate_simple_id(&artifact.id, "artifact id")?;
        write_json_sidecar(&self.artifact_sidecar_path(&artifact.id)?, artifact)
    }

    /// Loads a media artifact JSON sidecar by id.
    pub(crate) fn load_artifact(&self, artifact_id: &str) -> Result<MediaArtifact> {
        read_json_sidecar(&self.artifact_sidecar_path(artifact_id)?)
    }

    fn media_dir(&self) -> PathBuf {
        self.workspace_root.join(".puffer").join("media")
    }

    fn jobs_dir(&self) -> PathBuf {
        self.media_dir().join("jobs")
    }

    fn artifacts_dir(&self) -> PathBuf {
        self.media_dir().join("artifacts")
    }

    fn images_dir(&self) -> PathBuf {
        self.media_dir().join("images")
    }

    fn videos_dir(&self) -> PathBuf {
        self.media_dir().join("videos")
    }

    fn artifact_sidecars_dir(&self) -> PathBuf {
        self.media_dir().join("artifact-sidecars")
    }

    fn job_sidecar_path(&self, job_id: &str) -> Result<PathBuf> {
        validate_simple_id(job_id, "job id")?;
        Ok(self.jobs_dir().join(format!("{job_id}.json")))
    }

    fn artifact_sidecar_path(&self, artifact_id: &str) -> Result<PathBuf> {
        validate_simple_id(artifact_id, "artifact id")?;
        Ok(self
            .artifact_sidecars_dir()
            .join(format!("{artifact_id}.json")))
    }
}

fn write_media_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create media output directory {}", parent.display()))?;
    }
    fs::write(path, bytes).with_context(|| format!("write media bytes {}", path.display()))?;
    Ok(())
}

fn write_json_sidecar<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create media sidecar directory {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("write media sidecar {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("commit media sidecar {}", path.display()))?;
    Ok(())
}

fn read_json_sidecar<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let text = fs::read_to_string(path)
        .with_context(|| format!("read media sidecar {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse media sidecar {}", path.display()))
}

fn validate_simple_id(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("{field} must be a simple identifier");
    }
    Ok(())
}

fn validate_artifact_filename(filename: &str) -> Result<()> {
    let mut components = Path::new(filename).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None)
            if !name.is_empty() && name.to_string_lossy() != "." =>
        {
            Ok(())
        }
        _ => bail!("artifact filename must be a single safe filename"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn list_video_jobs_returns_video_jobs_and_skips_others() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());

        let mut video = MediaJob::new(
            "v1",
            MediaKind::Video,
            "relaydance",
            "seedance-nsfw",
            "p",
            1,
            1,
        );
        video.provider_job_id = Some("task_x".to_string());
        service.save_job(&video).unwrap();

        let image = MediaJob::new("i1", MediaKind::Image, "zhipu", "glm-image", "p", 1, 1);
        service.save_job(&image).unwrap();

        // A corrupt sidecar must be skipped.
        std::fs::write(service.jobs_dir().join("broken.json"), b"{ not json").unwrap();

        let jobs = service.list_video_jobs().expect("list");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "v1");
    }

    #[test]
    fn media_job_status_terminal_states_are_explicit() {
        assert!(!MediaJobStatus::Queued.is_terminal());
        assert!(!MediaJobStatus::Running.is_terminal());
        assert!(MediaJobStatus::Succeeded.is_terminal());
        assert!(MediaJobStatus::Failed.is_terminal());
        assert!(MediaJobStatus::Canceled.is_terminal());
    }

    #[test]
    fn media_job_rejects_transition_out_of_terminal_state() {
        let mut job = MediaJob::new(
            "job-1",
            MediaKind::Image,
            "openai",
            "gpt-image-1",
            "draw a ship",
            1,
            10,
        );

        job.transition(MediaJobStatus::Running, 11).unwrap();
        job.transition(MediaJobStatus::Succeeded, 12).unwrap();

        let error = job.transition(MediaJobStatus::Running, 13).unwrap_err();
        assert!(error.to_string().contains("terminal media job"));
    }

    #[test]
    fn media_artifact_paths_must_stay_under_artifact_directory() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());

        let path = service
            .artifact_file_path("artifact-1", "image.png")
            .unwrap();
        assert!(path.starts_with(temp.path().join(".puffer/media/artifacts")));

        let error = service
            .artifact_file_path("artifact-1", "../escape.png")
            .unwrap_err();
        assert!(error.to_string().contains("artifact filename"));
    }

    #[test]
    fn media_image_paths_must_stay_under_image_directory() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());

        let path = service
            .image_artifact_file_path("artifact-1", "image.png")
            .unwrap();
        assert!(path.starts_with(temp.path().join(".puffer/media/images")));

        let error = service
            .image_artifact_file_path("artifact-1", "../escape.png")
            .unwrap_err();
        assert!(error.to_string().contains("artifact filename"));
    }

    #[test]
    fn media_video_paths_must_stay_under_video_directory() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());

        let path = service
            .video_artifact_file_path("artifact-1", "video.mp4")
            .unwrap();
        assert!(path.starts_with(temp.path().join(".puffer/media/videos")));

        let error = service
            .video_artifact_file_path("artifact-1", "../escape.mp4")
            .unwrap_err();
        assert!(error.to_string().contains("artifact filename"));
    }

    #[test]
    fn media_poster_path_uses_fixed_safe_filename() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());

        let path = service
            .poster_artifact_file_path("artifact-video-1")
            .unwrap();

        assert_eq!(
            path,
            temp.path()
                .join(".puffer/media/artifacts/artifact-video-1/poster.jpg")
        );
    }

    #[test]
    fn media_jobs_and_artifacts_roundtrip_through_json_sidecars() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());
        let mut job = MediaJob::new(
            "job-1",
            MediaKind::Image,
            "openai",
            "gpt-image-1",
            "draw a ship",
            1,
            10,
        );
        job.transition(MediaJobStatus::Running, 11).unwrap();
        service.save_job(&job).unwrap();

        let loaded_job = service.load_job("job-1").unwrap();
        assert_eq!(loaded_job.status, MediaJobStatus::Running);
        assert_eq!(loaded_job.prompt, "draw a ship");

        let artifact_path = service
            .write_artifact_bytes("artifact-1", "ship.png", b"png-bytes")
            .unwrap();
        let artifact = MediaArtifact {
            id: "artifact-1".to_string(),
            job_id: job.id.clone(),
            kind: MediaKind::Image,
            path: artifact_path.clone(),
            mime_type: "image/png".to_string(),
            byte_count: 9,
            metadata: json!({"size": "1024x1024"}),
            preview: None,
            created_at_ms: 12,
        };
        service.save_artifact(&artifact).unwrap();

        let loaded_artifact = service.load_artifact("artifact-1").unwrap();
        assert_eq!(loaded_artifact.path, artifact_path);
        assert_eq!(std::fs::read(artifact_path).unwrap(), b"png-bytes");
    }

    #[test]
    fn media_artifacts_roundtrip_available_poster_preview() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());
        let video_path = service
            .write_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
            .unwrap();
        let poster_path = service
            .write_artifact_bytes("artifact-video-1", "poster.jpg", &[0xff, 0xd8, 0xff, 0xd9])
            .unwrap();
        let artifact = MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: MediaKind::Video,
            path: video_path,
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: json!({}),
            preview: Some(super::artifacts::MediaArtifactPreview::available_poster(
                poster_path.clone(),
                4,
            )),
            created_at_ms: 12,
        };
        service.save_artifact(&artifact).unwrap();

        let loaded_artifact = service.load_artifact("artifact-video-1").unwrap();

        assert_eq!(loaded_artifact.preview, artifact.preview);
        let preview = loaded_artifact
            .preview
            .as_ref()
            .map(super::artifacts::MediaArtifactPreview::poster)
            .expect("poster preview");
        assert_eq!(
            preview.state,
            super::artifacts::MediaArtifactPreviewState::Available
        );
        assert_eq!(preview.path.as_deref(), Some(poster_path.as_path()));
        assert_eq!(preview.mime_type.as_deref(), Some("image/jpeg"));
        assert_eq!(preview.byte_count, Some(4));
    }

    #[test]
    fn media_artifacts_roundtrip_missing_poster_preview() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());
        let video_path = service
            .write_artifact_bytes("artifact-video-1", "generated.mp4", b"mp4-bytes")
            .unwrap();
        let artifact = MediaArtifact {
            id: "artifact-video-1".to_string(),
            job_id: "job-video-1".to_string(),
            kind: MediaKind::Video,
            path: video_path,
            mime_type: "video/mp4".to_string(),
            byte_count: 9,
            metadata: json!({}),
            preview: Some(super::artifacts::MediaArtifactPreview::missing_poster(
                "ffmpeg exited with status 1",
            )),
            created_at_ms: 12,
        };
        service.save_artifact(&artifact).unwrap();

        let loaded_artifact = service.load_artifact("artifact-video-1").unwrap();

        let preview = loaded_artifact
            .preview
            .as_ref()
            .map(super::artifacts::MediaArtifactPreview::poster)
            .expect("poster preview");
        assert_eq!(
            preview.state,
            super::artifacts::MediaArtifactPreviewState::Missing
        );
        assert_eq!(
            preview.reason.as_deref(),
            Some("ffmpeg exited with status 1")
        );
        assert_eq!(preview.path, None);
    }

    #[test]
    fn media_artifacts_load_without_preview_metadata() {
        let artifact: MediaArtifact = serde_json::from_value(json!({
            "id": "artifact-video-1",
            "jobId": "job-video-1",
            "kind": "video",
            "path": "/tmp/generated.mp4",
            "mimeType": "video/mp4",
            "byteCount": 9,
            "metadata": {},
            "createdAtMs": 12
        }))
        .unwrap();

        assert_eq!(artifact.preview, None);
    }
}
