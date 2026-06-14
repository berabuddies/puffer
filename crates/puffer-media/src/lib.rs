//! Pure media generation runtime for Puffer Code.

mod artifacts;
mod diagnostics;
mod internal_tools;
pub(crate) mod media;
mod runtime;
mod video;

pub use diagnostics::{
    media_failure_diagnostic, media_failure_error, MediaFailureContext, MediaFailureDiagnostic,
    MediaFailureError, ProviderHttpError,
};
pub use media::planner::validate_image_generation_count;
pub use runtime::{
    discover_exact_media_capabilities, generate_exact_image_with_cache,
    generate_exact_media_with_cache, generated_media_attachment_metadata,
    generated_media_attachment_metadata_with_fallback, generated_media_internal_bash_output,
    generated_media_internal_command_kind, generated_media_timeline_attachments,
    generated_video_access_metadata_by_artifact, list_exact_media_capabilities_with_cache,
    read_generated_media_preview_by_artifact, resolved_exact_image_parameters_with_cache,
    ExactGeneratedArtifact, ExactImageGenerationRequest, ExactImageGenerationResult,
    ExactMediaDiscoveryCache, ExactMediaGenerationRequest, ExactMediaGenerationResult,
    GeneratedMediaAttachmentMetadata, GeneratedMediaInternalCommandKind,
    GeneratedMediaPreviewResult, GeneratedMediaTimelineAttachment,
    GeneratedMediaTimelineAttachmentKind, GeneratedVideoAccessMetadata,
    GeneratedVideoAccessMetadataResult, MediaCapabilityView, MEDIA_DISCOVERY_TTL_MS,
};
