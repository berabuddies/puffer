use super::capabilities::{MediaCapability, MediaKind};
use anyhow::{bail, Context, Result};
use puffer_provider_registry::{
    canonical_provider_id, AuthStore, Axis, AxisRole, ControlKind, MediaExecutionDescriptor,
    MediaExecutionKind, MediaMap, MediaModelDescriptor, MediaOperation, ProviderDescriptor,
    ProviderRegistry, Variants, WireType,
};
use std::collections::{BTreeMap, BTreeSet};

const CAPABILITY_STATUS_AVAILABLE: &str = "available";
const CAPABILITY_STATUS_UNAVAILABLE: &str = "unavailable";
const CAPABILITY_REASON_ADAPTER_UNAVAILABLE: &str = "adapter_unavailable";
const CAPABILITY_REASON_MISSING_AUTH: &str = "missing_auth";
const CAPABILITY_SOURCE_STATIC: &str = "static";
const MODE_AXIS_ID: &str = "mode";
const RATIO_AXIS_ID: &str = "ratio";
const OUTPUT_AXIS_ID: &str = "output";

/// Carries cached dynamic media discovery records into capability resolution.
#[derive(Debug, Clone, Default)]
pub(crate) struct MediaDiscoveryCache {
    pub(crate) image_models: Vec<CachedImageMediaModel>,
}

/// Carries one cached exact image model discovered for a provider.
#[derive(Debug, Clone)]
pub(crate) struct CachedImageMediaModel {
    pub(crate) provider_id: String,
    pub(crate) model: MediaModelDescriptor,
    pub(crate) source: String,
}

/// Describes a concrete upstream media request resolved from a logical model and
/// the user's axis selections: the upstream `model_id` to call, the execution
/// `adapter`, provider request `parameters`, each parameter's JSON wire type,
/// and the runtime output `count`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedMediaRequest {
    pub(crate) provider_id: String,
    pub(crate) model_id: String,
    pub(crate) adapter: String,
    pub(crate) parameters: BTreeMap<String, String>,
    pub(crate) parameter_wire_types: BTreeMap<String, WireType>,
    pub(crate) count: u8,
}

/// Resolves selectable exact media capabilities from provider descriptors.
pub(crate) fn resolve_media_capabilities(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    kind: MediaKind,
    operation: MediaOperation,
    checked_at_ms: u64,
    discovery_cache: &MediaDiscoveryCache,
) -> Vec<MediaCapability> {
    match kind {
        MediaKind::Image => resolve_image_capabilities(
            registry,
            auth_store,
            operation,
            checked_at_ms,
            discovery_cache,
        ),
        MediaKind::Video => {
            resolve_video_capabilities(registry, auth_store, operation, checked_at_ms)
        }
    }
}

/// Resolves a logical media selection (provider + logical model + axis
/// selections) into a concrete upstream request. Validates each provided
/// selection against its axis control, resolves the variant from any selector
/// axis, and merges the chosen variant's `base_params` with each param axis's
/// `request_field` value (defaulting unset axes from their control).
pub(crate) fn resolve_media_request(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    provider_id: &str,
    logical_model_id: &str,
    kind: MediaKind,
    selections: &BTreeMap<String, String>,
    discovery_cache: &MediaDiscoveryCache,
) -> Result<ResolvedMediaRequest> {
    let cap = resolve_media_capabilities(
        registry,
        auth_store,
        kind,
        MediaOperation::Generate,
        0,
        discovery_cache,
    )
    .into_iter()
    .find(|c| c.provider_id == provider_id && c.model_id == logical_model_id)
    .with_context(|| format!("unknown media model {provider_id}/{logical_model_id}"))?;

    if cap.status != CAPABILITY_STATUS_AVAILABLE {
        bail!(
            "selected {} model unavailable: {provider_id}/{logical_model_id}",
            media_kind_error_name(kind)
        );
    }

    // 1. Validate each provided selection against its axis control.
    for axis in &cap.axes {
        let Some(value) = selections.get(&axis.id) else {
            continue;
        };
        match &axis.control {
            ControlKind::Enum { values, .. } if !values.contains(value) => {
                bail!("axis {} value {value} is not allowed", axis.id)
            }
            ControlKind::Range { min, max, step, .. } => {
                let n: f64 = value
                    .parse()
                    .with_context(|| format!("axis {} must be numeric", axis.id))?;
                // Snap to the nearest grid point and compare, so non-unit steps
                // (e.g. 0.1) aren't rejected by float division error.
                let nearest = min + ((n - min) / step).round() * step;
                if n < *min || n > *max || (nearest - n).abs() > 1e-9 {
                    bail!("axis {} value {value} is out of range", axis.id);
                }
            }
            ControlKind::Bool { .. } if value != "true" && value != "false" => {
                bail!("axis {} must be true/false", axis.id)
            }
            _ => {}
        }
    }

    // 2. Resolve the variant.
    let variant = match &cap.variants {
        Variants::Single(v) => v.clone(),
        Variants::BySelector { selector, map } => {
            let chosen = selections
                .get(selector)
                .cloned()
                .unwrap_or_else(|| default_for_axis(&cap.axes, selector));
            map.get(&chosen).cloned().with_context(|| {
                format!("unsupported {selector} value {chosen} for {logical_model_id}")
            })?
        }
    };

    // 3. Merge base_params with ordinary Param-axis selections.
    let mut parameters = variant.base_params.clone();
    let mut parameter_wire_types = parameters
        .keys()
        .map(|field| (field.clone(), WireType::String))
        .collect::<BTreeMap<_, _>>();
    for axis in cap.axes.iter().filter(|a| {
        a.role == AxisRole::Param
            && !runtime_only_axis(&a.id)
            && !mapped_reserved_axis(&a.id, cap.media_map.as_ref())
    }) {
        if let Some(field) = &axis.request_field {
            let v = selections
                .get(&axis.id)
                .cloned()
                .unwrap_or_else(|| default_for_axis(&cap.axes, &axis.id));
            parameters.insert(field.clone(), v);
            parameter_wire_types.insert(field.clone(), axis.wire_type);
        }
    }
    apply_ratio_media_map(
        &mut parameters,
        &mut parameter_wire_types,
        cap.media_map.as_ref(),
        &cap.axes,
        selections,
    )?;
    apply_size_media_map(
        &mut parameters,
        &mut parameter_wire_types,
        cap.media_map.as_ref(),
        &cap.axes,
        selections,
    )?;
    let count = resolved_count(kind, &cap.axes, selections, cap.max_outputs)?;

    Ok(ResolvedMediaRequest {
        provider_id: cap.provider_id,
        model_id: variant.model_id,
        adapter: cap.adapter,
        parameters,
        parameter_wire_types,
        count,
    })
}

fn runtime_only_axis(axis_id: &str) -> bool {
    axis_id == OUTPUT_AXIS_ID
}

fn mapped_reserved_axis(axis_id: &str, media_map: Option<&MediaMap>) -> bool {
    let Some(media_map) = media_map else {
        return false;
    };
    match axis_id {
        MODE_AXIS_ID => media_map.size.is_some(),
        RATIO_AXIS_ID => media_map.ratio.is_some() || media_map.size.is_some(),
        _ => false,
    }
}

fn apply_ratio_media_map(
    parameters: &mut BTreeMap<String, String>,
    parameter_wire_types: &mut BTreeMap<String, WireType>,
    media_map: Option<&MediaMap>,
    axes: &[Axis],
    selections: &BTreeMap<String, String>,
) -> Result<()> {
    let Some(ratio_map) = media_map.and_then(|map| map.ratio.as_ref()) else {
        return Ok(());
    };
    let ratio = selected_axis_value(axes, selections, RATIO_AXIS_ID)
        .context("media_map.ratio requires a ratio axis")?;
    let mapped = ratio_map
        .values
        .get(&ratio)
        .with_context(|| format!("ratio {ratio} is not mapped"))?;
    if let Some(value) = mapped {
        parameters.insert(ratio_map.field.clone(), value.clone());
        parameter_wire_types.insert(ratio_map.field.clone(), WireType::String);
    }
    Ok(())
}

fn apply_size_media_map(
    parameters: &mut BTreeMap<String, String>,
    parameter_wire_types: &mut BTreeMap<String, WireType>,
    media_map: Option<&MediaMap>,
    axes: &[Axis],
    selections: &BTreeMap<String, String>,
) -> Result<()> {
    let Some(size_map) = media_map.and_then(|map| map.size.as_ref()) else {
        return Ok(());
    };
    let mode = selected_axis_value(axes, selections, MODE_AXIS_ID)
        .context("media_map.size requires a mode axis")?;
    let ratio = selected_axis_value(axes, selections, RATIO_AXIS_ID)
        .context("media_map.size requires a ratio axis")?;
    let ratios = size_map
        .values
        .get(&mode)
        .with_context(|| format!("mode {mode} is not mapped"))?;
    let mapped = ratios
        .get(&ratio)
        .with_context(|| format!("mode {mode} ratio {ratio} is not mapped"))?;
    if let Some(value) = mapped {
        parameters.insert(size_map.field.clone(), value.clone());
        parameter_wire_types.insert(size_map.field.clone(), WireType::String);
    }
    Ok(())
}

fn resolved_count(
    kind: MediaKind,
    axes: &[Axis],
    selections: &BTreeMap<String, String>,
    max_outputs: Option<u8>,
) -> Result<u8> {
    if kind == MediaKind::Video {
        return Ok(1);
    }
    let value =
        selected_axis_value(axes, selections, OUTPUT_AXIS_ID).unwrap_or_else(|| "1".to_string());
    let count = value.parse::<u8>().context("axis output must be numeric")?;
    let max_outputs = max_outputs.unwrap_or(1).min(9);
    if count == 0 || count > max_outputs {
        bail!("axis output value {value} is out of range");
    }
    Ok(count)
}

fn selected_axis_value(
    axes: &[Axis],
    selections: &BTreeMap<String, String>,
    axis_id: &str,
) -> Option<String> {
    axes.iter().find(|axis| axis.id == axis_id).map(|_| {
        selections
            .get(axis_id)
            .cloned()
            .unwrap_or_else(|| default_for_axis(axes, axis_id))
    })
}

fn default_for_axis(axes: &[Axis], axis_id: &str) -> String {
    axes.iter()
        .find(|a| a.id == axis_id)
        .map(|a| match &a.control {
            ControlKind::Enum { default, .. } => default.clone(),
            ControlKind::Range { default, .. } => format!("{default}"),
            ControlKind::Bool { default } => default.to_string(),
        })
        .unwrap_or_default()
}

/// Resolves the provider and execution descriptor for an exact image selection.
pub(crate) fn resolve_image_execution_descriptor<'a>(
    registry: &'a ProviderRegistry,
    provider_id: &str,
    model_id: &str,
    adapter: &str,
    discovery_cache: &MediaDiscoveryCache,
) -> Result<(&'a ProviderDescriptor, MediaExecutionDescriptor)> {
    let unavailable =
        || format!("selected image model unavailable: {provider_id}/{model_id} via {adapter}");
    let provider = registry.provider(provider_id).with_context(unavailable)?;
    let image = provider
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .with_context(unavailable)?;
    let model = image
        .models
        .iter()
        .find(|model| model.id == model_id)
        .or_else(|| {
            discovery_cache
                .image_models
                .iter()
                .filter(|cached| cached.provider_id == provider.id)
                .map(|cached| &cached.model)
                .find(|model| model.id == model_id)
        });
    let execution = model
        .and_then(|model| model.execution.as_ref())
        .or(image.execution.as_ref())
        .cloned()
        .with_context(unavailable)?;
    if !execution_adapter_is_available_for_kind(MediaKind::Image, execution.adapter)
        || adapter_id(execution.adapter) != adapter
    {
        bail!("image media adapter unavailable for {adapter}");
    }

    Ok((provider, execution))
}

/// Resolves the provider and execution descriptor for an exact video selection.
///
/// The `model_id` may be a concrete variant id that is not itself a declared
/// logical model (selector variants share their logical model's provider-level
/// `video.execution`), so the lookup falls back to the provider execution.
pub(crate) fn resolve_video_execution_descriptor<'a>(
    registry: &'a ProviderRegistry,
    provider_id: &str,
    model_id: &str,
    adapter: &str,
) -> Result<(&'a ProviderDescriptor, MediaExecutionDescriptor)> {
    let unavailable =
        || format!("selected video model unavailable: {provider_id}/{model_id} via {adapter}");
    let provider = registry.provider(provider_id).with_context(unavailable)?;
    let video = provider
        .media
        .as_ref()
        .and_then(|media| media.video.as_ref())
        .with_context(unavailable)?;
    let execution = video
        .models
        .iter()
        .find(|model| model.id == model_id)
        .and_then(|model| model.execution.as_ref())
        .or(video.execution.as_ref())
        .cloned()
        .with_context(unavailable)?;
    if !execution_adapter_is_available_for_kind(MediaKind::Video, execution.adapter)
        || adapter_id(execution.adapter) != adapter
    {
        bail!("video media adapter unavailable for {adapter}");
    }

    Ok((provider, execution))
}

fn resolve_image_capabilities(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    operation: MediaOperation,
    checked_at_ms: u64,
    discovery_cache: &MediaDiscoveryCache,
) -> Vec<MediaCapability> {
    let mut capabilities = Vec::new();
    for provider in registry.providers() {
        if !provider_is_connected(provider, auth_store) {
            continue;
        }
        let Some(image) = provider
            .media
            .as_ref()
            .and_then(|media| media.image.as_ref())
        else {
            continue;
        };
        let mut emitted_model_ids = std::collections::HashSet::new();
        let static_models = image
            .models
            .iter()
            .map(|model| (model, CAPABILITY_SOURCE_STATIC));
        let discovered_models = discovery_cache.image_models.iter().filter_map(|cached| {
            (cached.provider_id == provider.id).then_some((&cached.model, cached.source.as_str()))
        });
        for (model, source) in static_models.chain(discovered_models) {
            if !emitted_model_ids.insert(model.id.clone()) {
                continue;
            }
            if !media_model_is_available(model, operation) {
                continue;
            }
            let Some(execution) = image_execution(image.execution.as_ref(), model) else {
                continue;
            };
            if !execution_adapter_is_available_for_kind(MediaKind::Image, execution.adapter) {
                continue;
            }
            capabilities.push(MediaCapability {
                provider_id: provider.id.clone(),
                provider_display_name: provider.display_name.clone(),
                model_id: model.id.clone(),
                model_display_name: model
                    .display_name
                    .clone()
                    .unwrap_or_else(|| model.id.clone()),
                kind: MediaKind::Image,
                operation: operation_wire_name(operation).to_string(),
                adapter: adapter_id(execution.adapter).to_string(),
                axes: normalized_image_axes(model),
                variants: model.variants.clone(),
                max_outputs: model.max_outputs,
                media_map: model.media_map.clone(),
                status: CAPABILITY_STATUS_AVAILABLE.to_string(),
                source: source.to_string(),
                reason: None,
                checked_at_ms,
            });
        }
    }
    capabilities
}

fn resolve_video_capabilities(
    registry: &ProviderRegistry,
    auth_store: &AuthStore,
    operation: MediaOperation,
    checked_at_ms: u64,
) -> Vec<MediaCapability> {
    let mut capabilities = Vec::new();
    for provider in registry.providers() {
        let Some(video) = provider
            .media
            .as_ref()
            .and_then(|media| media.video.as_ref())
        else {
            continue;
        };
        let provider_connected = provider_is_connected(provider, auth_store);
        for model in &video.models {
            if !media_model_is_available(model, operation) {
                continue;
            }
            let Some(execution) = image_execution(video.execution.as_ref(), model) else {
                continue;
            };
            let adapter_available =
                execution_adapter_is_available_for_kind(MediaKind::Video, execution.adapter);
            let (status, reason) = video_capability_state(provider_connected, adapter_available);
            capabilities.push(MediaCapability {
                provider_id: provider.id.clone(),
                provider_display_name: provider.display_name.clone(),
                model_id: model.id.clone(),
                model_display_name: model
                    .display_name
                    .clone()
                    .unwrap_or_else(|| model.id.clone()),
                kind: MediaKind::Video,
                operation: operation_wire_name(operation).to_string(),
                adapter: adapter_id(execution.adapter).to_string(),
                axes: normalized_video_axes(model),
                variants: model.variants.clone(),
                max_outputs: model.max_outputs,
                media_map: model.media_map.clone(),
                status: status.to_string(),
                source: CAPABILITY_SOURCE_STATIC.to_string(),
                reason: reason.map(str::to_string),
                checked_at_ms,
            });
        }
    }
    capabilities
}

fn normalized_image_axes(model: &MediaModelDescriptor) -> Vec<Axis> {
    let ratio_values = exact_ratio_values_from_media_map(model.media_map.as_ref());
    let mut axes = model
        .axes
        .iter()
        .filter(|axis| axis.id != OUTPUT_AXIS_ID)
        .filter_map(|axis| normalize_image_axis(axis, ratio_values.as_ref()))
        .collect::<Vec<_>>();
    axes.push(output_axis(model.max_outputs));
    axes
}

fn normalize_image_axis(axis: &Axis, ratio_values: Option<&BTreeSet<String>>) -> Option<Axis> {
    if axis.id != RATIO_AXIS_ID {
        return Some(axis.clone());
    }
    let Some(ratio_values) = ratio_values else {
        return Some(axis.clone());
    };
    let ControlKind::Enum { values, default } = &axis.control else {
        return Some(axis.clone());
    };
    let filtered = values
        .iter()
        .filter(|value| ratio_values.contains(*value))
        .cloned()
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        return None;
    }
    let default = if filtered.contains(default) {
        default.clone()
    } else {
        filtered[0].clone()
    };
    let mut axis = axis.clone();
    axis.control = ControlKind::Enum {
        values: filtered,
        default,
    };
    Some(axis)
}

fn exact_ratio_values_from_media_map(media_map: Option<&MediaMap>) -> Option<BTreeSet<String>> {
    let media_map = media_map?;
    let mut values = None;
    if let Some(ratio_map) = &media_map.ratio {
        values = Some(ratio_map.values.keys().cloned().collect::<BTreeSet<_>>());
    }
    if let Some(size_map) = &media_map.size {
        let size_values = size_map.common_ratios();
        values = Some(match values {
            Some(mut existing) => {
                existing.retain(|ratio| size_values.contains(ratio));
                existing
            }
            None => size_values,
        });
    }
    values
}

fn output_axis(max_outputs: Option<u8>) -> Axis {
    let max = max_outputs.unwrap_or(1).clamp(1, 9) as f64;
    Axis {
        id: OUTPUT_AXIS_ID.to_string(),
        label: "Output".to_string(),
        role: AxisRole::Param,
        control: ControlKind::Range {
            min: 1.0,
            max,
            step: 1.0,
            default: 1.0,
        },
        request_field: None,
        wire_type: WireType::Number,
    }
}

fn normalized_video_axes(model: &MediaModelDescriptor) -> Vec<Axis> {
    model
        .axes
        .iter()
        .cloned()
        .map(normalize_video_axis_label)
        .collect()
}

fn normalize_video_axis_label(mut axis: Axis) -> Axis {
    axis.label = match axis.id.as_str() {
        MODE_AXIS_ID | "resolution" => "Mode".to_string(),
        RATIO_AXIS_ID | "aspect_ratio" => "Ratio".to_string(),
        "duration" | "duration_seconds" => "Duration".to_string(),
        _ => axis.label,
    };
    axis
}

fn video_capability_state(
    provider_connected: bool,
    adapter_available: bool,
) -> (&'static str, Option<&'static str>) {
    if !adapter_available {
        (
            CAPABILITY_STATUS_UNAVAILABLE,
            Some(CAPABILITY_REASON_ADAPTER_UNAVAILABLE),
        )
    } else if !provider_connected {
        (
            CAPABILITY_STATUS_UNAVAILABLE,
            Some(CAPABILITY_REASON_MISSING_AUTH),
        )
    } else {
        (CAPABILITY_STATUS_AVAILABLE, None)
    }
}

fn image_execution<'a>(
    provider_execution: Option<&'a MediaExecutionDescriptor>,
    model: &'a MediaModelDescriptor,
) -> Option<&'a MediaExecutionDescriptor> {
    model.execution.as_ref().or(provider_execution)
}

fn provider_is_connected(provider: &ProviderDescriptor, auth_store: &AuthStore) -> bool {
    if provider.auth_modes.is_empty() {
        return true;
    }
    if auth_store.has_auth(&provider.id) {
        return true;
    }
    let canonical = canonical_provider_id(&provider.id);
    if canonical != provider.id && auth_store.has_auth(&canonical) {
        return true;
    }
    provider.id == "openai" && auth_store.has_auth("codex")
}

fn execution_adapter_is_available_for_kind(kind: MediaKind, adapter: MediaExecutionKind) -> bool {
    matches!(
        (kind, adapter),
        (MediaKind::Image, MediaExecutionKind::ImagesJson)
            | (MediaKind::Image, MediaExecutionKind::GeminiGenerateContent)
            | (MediaKind::Image, MediaExecutionKind::ChatImageOutput)
            | (MediaKind::Image, MediaExecutionKind::MinimaxImage)
            | (MediaKind::Video, MediaExecutionKind::ReplicateVideo)
            | (MediaKind::Video, MediaExecutionKind::RelaydanceVideo)
            | (MediaKind::Video, MediaExecutionKind::BytePlusVideo)
            | (MediaKind::Video, MediaExecutionKind::WorldRouterVideo)
    )
}

fn media_model_is_available(model: &MediaModelDescriptor, operation: MediaOperation) -> bool {
    let id = model.id.trim();
    !id.is_empty()
        && !id.eq_ignore_ascii_case("auto")
        && !has_wildcard_or_regex_marker(id)
        && model.operations.contains(&operation)
}

fn operation_wire_name(operation: MediaOperation) -> &'static str {
    match operation {
        MediaOperation::Generate => "generate",
    }
}

fn media_kind_error_name(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "image",
        MediaKind::Video => "video",
    }
}

pub(crate) fn adapter_id(adapter: MediaExecutionKind) -> &'static str {
    match adapter {
        MediaExecutionKind::ImagesJson => "images_json",
        MediaExecutionKind::GeminiGenerateContent => "gemini_generate_content",
        MediaExecutionKind::ChatImageOutput => "chat_image_output",
        MediaExecutionKind::MinimaxImage => "minimax_image",
        MediaExecutionKind::ReplicateVideo => "replicate_video",
        MediaExecutionKind::RelaydanceVideo => "relaydance_video",
        MediaExecutionKind::BytePlusVideo => "byteplus_video",
        MediaExecutionKind::WorldRouterVideo => "worldrouter_video",
    }
}

fn has_wildcard_or_regex_marker(value: &str) -> bool {
    value.chars().any(|ch| {
        matches!(
            ch,
            '*' | '?' | '[' | ']' | '(' | ')' | '{' | '}' | '|' | '^' | '$' | '\\'
        )
    })
}

#[cfg(test)]
#[path = "resolver_tests.rs"]
mod tests;
