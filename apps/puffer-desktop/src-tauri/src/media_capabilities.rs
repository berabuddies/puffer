use crate::dtos::{MediaCapabilityAxisDto, MediaCapabilityInfoDto};
use puffer_media::{
    list_exact_media_capabilities_with_cache, ExactMediaDiscoveryCache, MediaCapabilityView,
};
use puffer_provider_registry::{AuthStore, Axis, ProviderRegistry};

/// Lists descriptor-backed exact media capabilities for the desktop backend.
pub(crate) fn list(
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    kind: Option<&str>,
    discovery_cache: &ExactMediaDiscoveryCache,
) -> Vec<MediaCapabilityInfoDto> {
    list_exact_media_capabilities_with_cache(providers, auth_store, kind, discovery_cache)
        .into_iter()
        .map(media_capability_info_dto)
        .collect()
}

fn media_capability_info_dto(capability: MediaCapabilityView) -> MediaCapabilityInfoDto {
    let axes = capability
        .axes
        .into_iter()
        .map(media_capability_axis_dto)
        .collect();

    MediaCapabilityInfoDto {
        provider_id: capability.provider_id,
        provider_display_name: capability.provider_display_name,
        model_id: capability.model_id,
        model_display_name: capability.model_display_name,
        kind: capability.kind,
        operation: capability.operation,
        adapter: capability.adapter,
        axes,
        status: capability.status,
        source: capability.source,
        reason: capability.reason,
        checked_at_ms: capability.checked_at_ms,
    }
}

fn media_capability_axis_dto(axis: Axis) -> MediaCapabilityAxisDto {
    MediaCapabilityAxisDto {
        id: axis.id,
        label: axis.label,
        role: axis.role,
        control: axis.control,
    }
}
