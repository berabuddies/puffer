use super::capabilities::MediaKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Stores durable metadata for a generated media file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaArtifact {
    pub(crate) id: String,
    pub(crate) job_id: String,
    pub(crate) kind: MediaKind,
    pub(crate) path: PathBuf,
    pub(crate) mime_type: String,
    pub(crate) byte_count: u64,
    pub(crate) metadata: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) preview: Option<MediaArtifactPreview>,
    pub(crate) created_at_ms: u64,
}

/// Stores the typed preview metadata for a media artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum MediaArtifactPreview {
    #[serde(rename = "poster")]
    Poster(MediaPosterPreview),
}

impl MediaArtifactPreview {
    /// Builds metadata for an available poster preview.
    pub(crate) fn available_poster(path: impl Into<PathBuf>, byte_count: u64) -> Self {
        Self::Poster(MediaPosterPreview {
            state: MediaArtifactPreviewState::Available,
            path: Some(path.into()),
            mime_type: Some("image/jpeg".to_string()),
            byte_count: Some(byte_count),
            reason: None,
        })
    }

    /// Builds metadata for a missing poster preview.
    pub(crate) fn missing_poster(reason: impl Into<String>) -> Self {
        Self::Poster(MediaPosterPreview {
            state: MediaArtifactPreviewState::Missing,
            path: None,
            mime_type: None,
            byte_count: None,
            reason: Some(reason.into()),
        })
    }

    /// Returns the poster preview details.
    pub(crate) fn poster(&self) -> &MediaPosterPreview {
        match self {
            Self::Poster(preview) => preview,
        }
    }
}

/// Describes whether an artifact poster preview can be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum MediaArtifactPreviewState {
    Available,
    Missing,
}

/// Stores single-purpose poster image preview details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaPosterPreview {
    pub(crate) state: MediaArtifactPreviewState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) byte_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}
