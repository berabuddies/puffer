use crate::internal_tools::{
    generated_media_internal_bash_output, GeneratedMediaInternalCommandKind,
};
use crate::media::{
    artifacts::MediaArtifactPreviewState, MediaArtifact, MediaGenerationService, MediaKind,
};
use serde::{Deserialize, Serialize};
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};

const REMOTE_SOURCE_URL_METADATA_KEY: &str = "remoteSourceUrl";

/// Describes the preview-read result for a generated media artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum GeneratedMediaPreviewResult {
    Available {
        #[serde(rename = "mimeType")]
        mime_type: String,
        bytes: Vec<u8>,
    },
    Missing,
    Unsupported,
}

/// Describes a trusted generated video file that can be served through a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedVideoAccessMetadata {
    pub path: PathBuf,
    pub mime_type: String,
    pub byte_count: u64,
    pub remote_source_url: Option<String>,
}

/// Describes generated video access metadata lookup state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratedVideoAccessMetadataResult {
    Available(GeneratedVideoAccessMetadata),
    Missing,
    Unsupported,
}

/// Describes the generated media kind carried by a timeline attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedMediaTimelineAttachmentKind {
    Image,
    Video,
}

impl GeneratedMediaTimelineAttachmentKind {
    /// Returns the wire value used by desktop chat attachment DTOs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
        }
    }
}

/// Describes one generated media attachment synthesized from a tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedMediaTimelineAttachment {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub byte_count: u64,
    pub extension: String,
    pub kind: GeneratedMediaTimelineAttachmentKind,
    pub state: String,
    pub job_id: String,
    pub artifact_id: String,
    pub index: usize,
    pub local_path: Option<String>,
    pub remote_source_url: Option<String>,
}

/// Carries generated image attachment metadata without image bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedMediaAttachmentMetadata {
    pub artifact_id: String,
    pub mime_type: String,
    pub byte_count: u64,
    pub state: String,
    pub local_path: Option<String>,
    pub remote_source_url: Option<String>,
}

/// Loads generated image metadata by artifact id.
pub fn generated_media_attachment_metadata(
    workspace_root: impl AsRef<Path>,
    artifact_id: &str,
) -> Option<GeneratedMediaAttachmentMetadata> {
    let service = MediaGenerationService::new(workspace_root.as_ref());
    let artifact = service.load_artifact(artifact_id).ok()?;
    generated_media_attachment_metadata_from_artifact(workspace_root.as_ref(), artifact)
}

/// Loads generated image metadata or falls back to missing metadata from a tool result.
pub fn generated_media_attachment_metadata_with_fallback(
    workspace_root: impl AsRef<Path>,
    artifact_id: &str,
    fallback_mime_type: &str,
    fallback_byte_count: u64,
) -> Option<GeneratedMediaAttachmentMetadata> {
    let artifact_id = artifact_id.trim();
    if !valid_generated_media_artifact_id(artifact_id) {
        return None;
    }
    let service = MediaGenerationService::new(workspace_root.as_ref());
    match service.load_artifact(artifact_id) {
        Ok(artifact) => {
            generated_media_attachment_metadata_from_artifact(workspace_root.as_ref(), artifact)
        }
        Err(_) => {
            if !generated_media_artifact_sidecar_missing(workspace_root.as_ref(), artifact_id) {
                return None;
            }
            let mime_type = canonical_sidecar_image_mime_type(Some(fallback_mime_type))?;
            Some(GeneratedMediaAttachmentMetadata {
                artifact_id: artifact_id.to_string(),
                mime_type: mime_type.to_string(),
                byte_count: fallback_byte_count,
                state: "missing".to_string(),
                local_path: None,
                remote_source_url: None,
            })
        }
    }
}

/// Reads generated artifact preview bytes by artifact id.
pub fn read_generated_media_preview_by_artifact(
    workspace_root: impl AsRef<Path>,
    artifact_id: &str,
) -> GeneratedMediaPreviewResult {
    let workspace_root = workspace_root.as_ref();
    let service = MediaGenerationService::new(workspace_root);
    let artifact = match service.load_artifact(artifact_id) {
        Ok(artifact) => artifact,
        Err(_) => return GeneratedMediaPreviewResult::Missing,
    };
    match artifact.kind {
        MediaKind::Image => {
            let image_root = generated_media_image_root(workspace_root);
            read_generated_media_preview_from_root_with_mime(
                &image_root,
                &artifact.path,
                Some(&artifact.mime_type),
            )
        }
        MediaKind::Video => read_generated_video_poster_preview(workspace_root, &artifact),
    }
}

/// Resolves trusted generated video metadata by artifact id.
pub fn generated_video_access_metadata_by_artifact(
    workspace_root: impl AsRef<Path>,
    artifact_id: &str,
) -> GeneratedVideoAccessMetadataResult {
    let workspace_root = workspace_root.as_ref();
    let service = MediaGenerationService::new(workspace_root);
    let artifact = match service.load_artifact(artifact_id) {
        Ok(artifact) => artifact,
        Err(_) => return GeneratedVideoAccessMetadataResult::Missing,
    };
    if artifact.kind != MediaKind::Video {
        return GeneratedVideoAccessMetadataResult::Unsupported;
    }
    let Some(mime_type) = canonical_generated_video_mime_type(&artifact.mime_type) else {
        return GeneratedVideoAccessMetadataResult::Unsupported;
    };
    let canonical_path = match canonical_generated_video_artifact_path(workspace_root, &artifact) {
        Ok(path) => path,
        Err(GeneratedMediaPathError::Missing) => {
            return GeneratedVideoAccessMetadataResult::Missing;
        }
        Err(GeneratedMediaPathError::Unsupported) => {
            return GeneratedVideoAccessMetadataResult::Unsupported;
        }
    };
    let byte_count = std::fs::metadata(&canonical_path)
        .map(|metadata| metadata.len())
        .unwrap_or(artifact.byte_count);
    let remote_source_url = media_artifact_remote_source_url(&artifact);
    GeneratedVideoAccessMetadataResult::Available(GeneratedVideoAccessMetadata {
        path: canonical_path,
        mime_type: mime_type.to_string(),
        byte_count,
        remote_source_url,
    })
}

/// Extracts generated media timeline attachments from a trusted Bash tool event.
pub fn generated_media_timeline_attachments(
    workspace_root: impl AsRef<Path>,
    tool_id: &str,
    input: &str,
    output: &str,
) -> Vec<GeneratedMediaTimelineAttachment> {
    let workspace_root = workspace_root.as_ref();
    let Some((kind, value)) = generated_media_internal_bash_output(tool_id, input, output) else {
        return Vec::new();
    };
    let Some(job_id) = generated_media_timeline_job_id(&value) else {
        return Vec::new();
    };
    match kind {
        GeneratedMediaInternalCommandKind::Image => generated_media_timeline_artifacts(&value)
            .filter_map(|artifact| {
                generated_media_timeline_image_attachment(workspace_root, job_id, artifact)
            })
            .collect(),
        GeneratedMediaInternalCommandKind::Video if is_generated_video_timeline_output(&value) => {
            generated_media_timeline_artifacts(&value)
                .filter_map(|artifact| {
                    generated_media_timeline_video_attachment(workspace_root, job_id, artifact)
                })
                .collect()
        }
        GeneratedMediaInternalCommandKind::Video => Vec::new(),
    }
}

/// Returns the remote source URL stored on a generated media artifact, if present.
pub(crate) fn media_artifact_remote_source_url(artifact: &MediaArtifact) -> Option<String> {
    artifact
        .metadata
        .get(REMOTE_SOURCE_URL_METADATA_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn generated_media_artifact_sidecar_missing(workspace_root: &Path, artifact_id: &str) -> bool {
    let sidecar_path = workspace_root
        .join(".puffer")
        .join("media")
        .join("artifact-sidecars")
        .join(format!("{artifact_id}.json"));
    matches!(
        std::fs::symlink_metadata(sidecar_path),
        Err(error) if error.kind() == ErrorKind::NotFound
    )
}

fn generated_media_attachment_metadata_from_artifact(
    workspace_root: &Path,
    artifact: MediaArtifact,
) -> Option<GeneratedMediaAttachmentMetadata> {
    if artifact.kind != MediaKind::Image {
        return None;
    }
    let image_root = generated_media_image_root(workspace_root);
    let canonical_path = match canonical_generated_media_image_path(&image_root, &artifact.path) {
        Ok(path) => Some(path),
        Err(GeneratedMediaPathError::Missing) => None,
        Err(GeneratedMediaPathError::Unsupported) => return None,
    };
    let state = if canonical_path.is_some() {
        "available"
    } else {
        "missing"
    };
    let local_path = canonical_path
        .as_ref()
        .map(|path| path.display().to_string());
    let remote_source_url = media_artifact_remote_source_url(&artifact);
    let mime_type = canonical_path
        .as_ref()
        .and_then(|path| canonical_generated_image_mime_type(path, Some(&artifact.mime_type)))
        .or_else(|| {
            canonical_sidecar_image_mime_type(Some(&artifact.mime_type)).map(str::to_string)
        })
        .or_else(|| generated_image_mime_type(&artifact.path).map(str::to_string))
        .unwrap_or_else(|| artifact.mime_type.clone());
    Some(GeneratedMediaAttachmentMetadata {
        artifact_id: artifact.id,
        mime_type,
        byte_count: artifact.byte_count,
        state: state.to_string(),
        local_path,
        remote_source_url,
    })
}

fn generated_media_timeline_job_id(value: &serde_json::Value) -> Option<&str> {
    let job_id = value
        .get("jobId")
        .and_then(serde_json::Value::as_str)?
        .trim();
    (!job_id.is_empty()).then_some(job_id)
}

fn generated_media_timeline_artifacts(
    value: &serde_json::Value,
) -> impl Iterator<Item = &serde_json::Value> {
    value
        .get("artifacts")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
}

fn is_generated_video_timeline_output(value: &serde_json::Value) -> bool {
    value.get("kind").and_then(serde_json::Value::as_str) == Some("video")
}

fn generated_media_timeline_image_attachment(
    workspace_root: &Path,
    job_id: &str,
    artifact: &serde_json::Value,
) -> Option<GeneratedMediaTimelineAttachment> {
    let artifact_id = generated_media_artifact_id(artifact)?;
    let index = generated_media_artifact_index(artifact);
    let fallback_mime_type = artifact
        .get("mimeType")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("image/png");
    let fallback_size = generated_media_artifact_size(artifact);
    let metadata = generated_media_attachment_metadata_with_fallback(
        workspace_root,
        artifact_id,
        fallback_mime_type,
        fallback_size,
    )?;
    let extension = generated_media_timeline_image_extension(&metadata.mime_type).to_string();
    Some(GeneratedMediaTimelineAttachment {
        id: format!("generated-image:{artifact_id}"),
        name: "Generated image".to_string(),
        mime_type: metadata.mime_type,
        byte_count: metadata.byte_count,
        extension,
        kind: GeneratedMediaTimelineAttachmentKind::Image,
        state: metadata.state,
        job_id: job_id.to_string(),
        artifact_id: artifact_id.to_string(),
        index,
        local_path: metadata.local_path,
        remote_source_url: metadata.remote_source_url,
    })
}

fn generated_media_timeline_video_attachment(
    workspace_root: &Path,
    job_id: &str,
    artifact: &serde_json::Value,
) -> Option<GeneratedMediaTimelineAttachment> {
    let artifact_id = generated_media_artifact_id(artifact)?;
    let output_mime_type = artifact
        .get("mimeType")
        .and_then(serde_json::Value::as_str)
        .and_then(canonical_generated_video_mime_type)?;
    let index = generated_media_artifact_index(artifact);
    match generated_video_access_metadata_by_artifact(workspace_root, artifact_id) {
        GeneratedVideoAccessMetadataResult::Available(metadata) => {
            let extension = generated_media_timeline_video_extension(&metadata.mime_type);
            Some(GeneratedMediaTimelineAttachment {
                id: format!("generated-video:{artifact_id}"),
                name: "Generated video".to_string(),
                mime_type: metadata.mime_type,
                byte_count: metadata.byte_count,
                extension: extension.to_string(),
                kind: GeneratedMediaTimelineAttachmentKind::Video,
                state: "available".to_string(),
                job_id: job_id.to_string(),
                artifact_id: artifact_id.to_string(),
                index,
                local_path: Some(metadata.path.display().to_string()),
                remote_source_url: metadata.remote_source_url,
            })
        }
        GeneratedVideoAccessMetadataResult::Missing => {
            let extension = generated_media_timeline_video_extension(output_mime_type);
            Some(GeneratedMediaTimelineAttachment {
                id: format!("generated-video:{artifact_id}"),
                name: "Generated video".to_string(),
                mime_type: output_mime_type.to_string(),
                byte_count: generated_media_artifact_size(artifact),
                extension: extension.to_string(),
                kind: GeneratedMediaTimelineAttachmentKind::Video,
                state: "missing".to_string(),
                job_id: job_id.to_string(),
                artifact_id: artifact_id.to_string(),
                index,
                local_path: None,
                remote_source_url: None,
            })
        }
        GeneratedVideoAccessMetadataResult::Unsupported => None,
    }
}

fn generated_media_artifact_id(artifact: &serde_json::Value) -> Option<&str> {
    let artifact_id = artifact.get("artifactId")?.as_str()?.trim();
    valid_generated_media_artifact_id(artifact_id).then_some(artifact_id)
}

fn generated_media_artifact_index(artifact: &serde_json::Value) -> usize {
    artifact
        .get("index")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize
}

fn generated_media_artifact_size(artifact: &serde_json::Value) -> u64 {
    artifact
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

fn generated_media_timeline_image_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "image/png" => "PNG",
        "image/jpeg" => "JPEG",
        "image/webp" => "WEBP",
        _ => "IMAGE",
    }
}

fn generated_media_timeline_video_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "video/mp4" => "MP4",
        "video/webm" => "WEBM",
        _ => "VIDEO",
    }
}

fn valid_generated_media_artifact_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn read_generated_video_poster_preview(
    workspace_root: &Path,
    artifact: &MediaArtifact,
) -> GeneratedMediaPreviewResult {
    let Some(preview) = artifact.preview.as_ref() else {
        return GeneratedMediaPreviewResult::Missing;
    };
    let poster = preview.poster();
    if poster.state != MediaArtifactPreviewState::Available {
        return GeneratedMediaPreviewResult::Missing;
    }
    let Some(path) = poster.path.as_ref() else {
        return GeneratedMediaPreviewResult::Missing;
    };
    let Some(expected_mime_type) = poster
        .mime_type
        .as_deref()
        .and_then(|mime_type| canonical_sidecar_image_mime_type(Some(mime_type)))
    else {
        return GeneratedMediaPreviewResult::Unsupported;
    };
    if expected_mime_type != "image/jpeg" {
        return GeneratedMediaPreviewResult::Unsupported;
    }
    let artifact_root = generated_media_artifact_root(workspace_root, &artifact.id);
    let canonical_path = match canonical_generated_media_artifact_path(&artifact_root, path) {
        Ok(path) => path,
        Err(GeneratedMediaPathError::Missing) => return GeneratedMediaPreviewResult::Missing,
        Err(GeneratedMediaPathError::Unsupported) => {
            return GeneratedMediaPreviewResult::Unsupported;
        }
    };
    let bytes = match std::fs::read(&canonical_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return GeneratedMediaPreviewResult::Missing;
        }
        Err(_) => return GeneratedMediaPreviewResult::Missing,
    };
    let Some(mime_type) = sniff_generated_image_mime_type(&bytes) else {
        return GeneratedMediaPreviewResult::Unsupported;
    };
    if mime_type != expected_mime_type {
        return GeneratedMediaPreviewResult::Unsupported;
    }
    GeneratedMediaPreviewResult::Available {
        mime_type: mime_type.to_string(),
        bytes,
    }
}

fn read_generated_media_preview_from_root_with_mime(
    image_root: &Path,
    path: &Path,
    sidecar_mime_type: Option<&str>,
) -> GeneratedMediaPreviewResult {
    let canonical_path = match canonical_generated_media_image_path(image_root, path) {
        Ok(path) => path,
        Err(GeneratedMediaPathError::Missing) => return GeneratedMediaPreviewResult::Missing,
        Err(GeneratedMediaPathError::Unsupported) => {
            return GeneratedMediaPreviewResult::Unsupported;
        }
    };
    let bytes = match std::fs::read(&canonical_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return GeneratedMediaPreviewResult::Missing;
        }
        Err(_) => return GeneratedMediaPreviewResult::Missing,
    };
    let Some(mime_type) = sniff_generated_image_mime_type(&bytes)
        .or_else(|| canonical_sidecar_image_mime_type(sidecar_mime_type))
        .or_else(|| generated_image_mime_type(&canonical_path))
    else {
        return GeneratedMediaPreviewResult::Unsupported;
    };
    GeneratedMediaPreviewResult::Available {
        mime_type: mime_type.to_string(),
        bytes,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeneratedMediaPathError {
    Missing,
    Unsupported,
}

fn canonical_generated_media_image_path(
    image_root: &Path,
    path: &Path,
) -> std::result::Result<PathBuf, GeneratedMediaPathError> {
    let canonical_path = match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return if missing_generated_media_path_is_under_root(image_root, path) {
                Err(GeneratedMediaPathError::Missing)
            } else {
                Err(GeneratedMediaPathError::Unsupported)
            };
        }
        Err(_) => return Err(GeneratedMediaPathError::Missing),
    };
    let canonical_root =
        std::fs::canonicalize(image_root).map_err(|_| GeneratedMediaPathError::Unsupported)?;
    if !canonical_path.starts_with(canonical_root) {
        return Err(GeneratedMediaPathError::Unsupported);
    }
    let metadata = match std::fs::metadata(&canonical_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(GeneratedMediaPathError::Missing);
        }
        Err(_) => return Err(GeneratedMediaPathError::Missing),
    };
    if !metadata.is_file() {
        return Err(GeneratedMediaPathError::Unsupported);
    }
    Ok(canonical_path)
}

fn generated_media_image_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".puffer").join("media").join("images")
}

fn generated_media_video_root(workspace_root: &Path, artifact_id: &str) -> PathBuf {
    workspace_root
        .join(".puffer")
        .join("media")
        .join("videos")
        .join(artifact_id)
}

fn generated_media_artifact_root(workspace_root: &Path, artifact_id: &str) -> PathBuf {
    workspace_root
        .join(".puffer")
        .join("media")
        .join("artifacts")
        .join(artifact_id)
}

fn canonical_generated_video_artifact_path(
    workspace_root: &Path,
    artifact: &MediaArtifact,
) -> std::result::Result<PathBuf, GeneratedMediaPathError> {
    let roots = [
        generated_media_video_root(workspace_root, &artifact.id),
        generated_media_artifact_root(workspace_root, &artifact.id),
    ];
    let mut missing = false;
    for root in roots {
        match canonical_generated_media_artifact_path(&root, &artifact.path) {
            Ok(path) => return Ok(path),
            Err(GeneratedMediaPathError::Missing) => missing = true,
            Err(GeneratedMediaPathError::Unsupported) => {}
        }
    }
    if missing {
        Err(GeneratedMediaPathError::Missing)
    } else {
        Err(GeneratedMediaPathError::Unsupported)
    }
}

fn canonical_generated_media_artifact_path(
    artifact_root: &Path,
    path: &Path,
) -> std::result::Result<PathBuf, GeneratedMediaPathError> {
    canonical_generated_media_image_path(artifact_root, path)
}

fn canonical_generated_video_mime_type(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "video/mp4" => Some("video/mp4"),
        "video/webm" => Some("video/webm"),
        _ => None,
    }
}

fn missing_generated_media_path_is_under_root(image_root: &Path, path: &Path) -> bool {
    if let (Ok(canonical_root), Some(parent)) = (std::fs::canonicalize(image_root), path.parent()) {
        if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
            return canonical_parent.starts_with(canonical_root);
        }
    }
    lexical_path_starts_with(path, image_root)
}

fn lexical_path_starts_with(path: &Path, root: &Path) -> bool {
    lexical_normalize_path(path).starts_with(lexical_normalize_path(root))
}

fn lexical_normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn canonical_sidecar_image_mime_type(value: Option<&str>) -> Option<&'static str> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

fn canonical_generated_image_mime_type(
    path: &Path,
    sidecar_mime_type: Option<&str>,
) -> Option<String> {
    let bytes = generated_image_magic_bytes(path)?;
    sniff_generated_image_mime_type(&bytes)
        .or_else(|| canonical_sidecar_image_mime_type(sidecar_mime_type))
        .or_else(|| generated_image_mime_type(path))
        .map(str::to_string)
}

fn generated_image_magic_bytes(path: &Path) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = [0_u8; 12];
    let count = file.read(&mut buffer).ok()?;
    Some(buffer[..count].to_vec())
}

fn sniff_generated_image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

fn generated_image_mime_type(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}
