use puffer_provider_registry::{Axis, MediaMap, Variants};
use serde::{Deserialize, Serialize};

/// Identifies the broad media asset type a capability or job handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum MediaKind {
    Image,
    Video,
}

/// Describes one provider/logical-model media generation capability.
///
/// Carries the typed user-facing `axes` plus the `variants` table that maps a
/// selector axis's value (or a single upstream model) to a concrete upstream
/// `model_id` + `base_params`. `MediaCapability` derives only `PartialEq`
/// because `Axis` → `ControlKind::Range` holds `f64`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaCapability {
    pub(crate) provider_id: String,
    pub(crate) provider_display_name: String,
    pub(crate) model_id: String,
    pub(crate) model_display_name: String,
    pub(crate) kind: MediaKind,
    pub(crate) operation: String,
    pub(crate) adapter: String,
    pub(crate) axes: Vec<Axis>,
    pub(crate) variants: Variants,
    pub(crate) max_outputs: Option<u8>,
    pub(crate) media_map: Option<MediaMap>,
    pub(crate) status: String,
    pub(crate) source: String,
    pub(crate) reason: Option<String>,
    pub(crate) checked_at_ms: u64,
}
