use puffer_provider_registry::{
    AxisRole, ControlKind, MediaBatchMode, MediaDiscoveryKind, MediaExecutionKind,
    MediaModelDescriptor, MediaOperation, ProviderDescriptor, Variants, WireType,
    CANONICAL_MEDIA_RATIOS,
};
use puffer_resources::ProviderPack;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

const IMAGE_PROVIDER_YAMLS: &[(&str, &str)] = &[
    (
        "byteplus",
        include_str!("../../../resources/providers/byteplus.yaml"),
    ),
    (
        "openai",
        include_str!("../../../resources/providers/openai.yaml"),
    ),
    (
        "zhipu",
        include_str!("../../../resources/providers/zhipu.yaml"),
    ),
    ("xai", include_str!("../../../resources/providers/xai.yaml")),
    (
        "minimax",
        include_str!("../../../resources/providers/minimax.yaml"),
    ),
    (
        "minimax-cn",
        include_str!("../../../resources/providers/minimax-cn.yaml"),
    ),
    (
        "openrouter",
        include_str!("../../../resources/providers/openrouter.yaml"),
    ),
    (
        "vercel-ai-gateway",
        include_str!("../../../resources/providers/vercel-ai-gateway.yaml"),
    ),
    (
        "worldrouter",
        include_str!("../../../resources/providers/worldrouter.yaml"),
    ),
];

const ALL_PROVIDER_YAMLS: &[(&str, &str)] = &[
    (
        "anthropic",
        include_str!("../../../resources/providers/anthropic.yaml"),
    ),
    (
        "byteplus",
        include_str!("../../../resources/providers/byteplus.yaml"),
    ),
    (
        "cerebras",
        include_str!("../../../resources/providers/cerebras.yaml"),
    ),
    (
        "groq",
        include_str!("../../../resources/providers/groq.yaml"),
    ),
    (
        "kimi-coding",
        include_str!("../../../resources/providers/kimi-coding.yaml"),
    ),
    (
        "kimi-openai",
        include_str!("../../../resources/providers/kimi-openai.yaml"),
    ),
    (
        "llama-cpp",
        include_str!("../../../resources/providers/llama-cpp.yaml"),
    ),
    (
        "lmstudio",
        include_str!("../../../resources/providers/lmstudio.yaml"),
    ),
    (
        "minimax",
        include_str!("../../../resources/providers/minimax.yaml"),
    ),
    (
        "minimax-cn",
        include_str!("../../../resources/providers/minimax-cn.yaml"),
    ),
    (
        "ollama",
        include_str!("../../../resources/providers/ollama.yaml"),
    ),
    (
        "openai",
        include_str!("../../../resources/providers/openai.yaml"),
    ),
    (
        "openrouter",
        include_str!("../../../resources/providers/openrouter.yaml"),
    ),
    (
        "qwen35",
        include_str!("../../../resources/providers/qwen35.yaml"),
    ),
    (
        "relaydance",
        include_str!("../../../resources/providers/relaydance.yaml"),
    ),
    (
        "vercel-ai-gateway",
        include_str!("../../../resources/providers/vercel-ai-gateway.yaml"),
    ),
    (
        "vllm",
        include_str!("../../../resources/providers/vllm.yaml"),
    ),
    (
        "worldrouter",
        include_str!("../../../resources/providers/worldrouter.yaml"),
    ),
    ("xai", include_str!("../../../resources/providers/xai.yaml")),
    (
        "zhipu",
        include_str!("../../../resources/providers/zhipu.yaml"),
    ),
];

const SEEDANCE_VIDEO_DURATIONS: &[&str] = &[
    "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15",
];
const SEEDANCE_VIDEO_RATIOS: &[&str] = &["Auto", "16:9", "4:3", "1:1", "3:4", "9:16", "21:9"];
const SEEDANCE_VIDEO_RESOLUTIONS: &[&str] = &["480p", "720p", "1080p"];
const SEEDANCE_FAST_VIDEO_RESOLUTIONS: &[&str] = &["480p", "720p"];
const SEEDANCE_15_VIDEO_DURATIONS: &[&str] = &["4", "5", "6", "7", "8", "9", "10", "11", "12"];
const TASK5_CANONICAL_IMAGE_PROVIDER_IDS: &[&str] =
    &["openai", "minimax", "minimax-cn", "byteplus"];
const RAW_IMAGE_AXIS_IDS: &[&str] = &[
    "size",
    "quality",
    "output_format",
    "response_format",
    "sequential_image_generation",
    "aspect_ratio",
    "resolution",
];

fn provider_descriptor(provider_id: &str, yaml: &str) -> ProviderDescriptor {
    let pack: ProviderPack = serde_yaml::from_str(yaml)
        .unwrap_or_else(|err| panic!("{provider_id}.yaml should parse: {err}"));
    assert_eq!(pack.id, provider_id);
    pack.into_descriptor()
}

fn assert_raw_image_executions_declare_batch_mode(provider_id: &str, yaml: &str) {
    let value: serde_yaml::Value =
        serde_yaml::from_str(yaml).unwrap_or_else(|error| panic!("{provider_id}: {error}"));
    let image = value
        .get("media")
        .and_then(|media| media.get("image"))
        .unwrap_or_else(|| panic!("{provider_id}: missing media.image"));
    let execution = image
        .get("execution")
        .unwrap_or_else(|| panic!("{provider_id}: missing media.image.execution"));
    assert!(
        execution
            .get("batch")
            .and_then(|batch| batch.get("mode"))
            .and_then(serde_yaml::Value::as_str)
            .is_some(),
        "{provider_id}: media.image.execution.batch.mode must be explicit"
    );
    if let Some(models) = image.get("models").and_then(serde_yaml::Value::as_sequence) {
        for model in models {
            if let Some(model_execution) = model.get("execution") {
                let model_id = model
                    .get("id")
                    .and_then(serde_yaml::Value::as_str)
                    .unwrap_or("<missing>");
                assert!(
                    model_execution
                        .get("batch")
                        .and_then(|batch| batch.get("mode"))
                        .and_then(serde_yaml::Value::as_str)
                        .is_some(),
                    "{provider_id}/{model_id}: models[].execution.batch.mode must be explicit"
                );
            }
        }
    }
}

fn assert_select_parameter(
    model: &MediaModelDescriptor,
    name: &str,
    label: &str,
    values: &[&str],
    default: &str,
    request_field: &str,
) {
    assert_select_parameter_with_wire_type(
        model,
        name,
        label,
        values,
        default,
        request_field,
        WireType::String,
    );
}

fn assert_select_parameter_with_wire_type(
    model: &MediaModelDescriptor,
    name: &str,
    label: &str,
    values: &[&str],
    default: &str,
    request_field: &str,
    wire_type: WireType,
) {
    assert_enum_axis(
        model,
        name,
        label,
        values,
        default,
        AxisRole::Param,
        Some(request_field),
        wire_type,
    );
}

fn assert_enum_axis(
    model: &MediaModelDescriptor,
    name: &str,
    label: &str,
    values: &[&str],
    default: &str,
    role: AxisRole,
    request_field: Option<&str>,
    wire_type: WireType,
) {
    let axis = model
        .axes
        .iter()
        .find(|axis| axis.id == name)
        .unwrap_or_else(|| panic!("{} should declare axis {name}", model.id));

    assert_eq!(axis.label, label);
    assert_eq!(axis.role, role);
    assert_eq!(axis.request_field.as_deref(), request_field);
    assert_eq!(axis.wire_type, wire_type);
    match &axis.control {
        ControlKind::Enum {
            values: actual,
            default: actual_default,
        } => {
            assert_eq!(
                actual,
                &values
                    .iter()
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>()
            );
            assert_eq!(actual_default, default);
        }
        other => panic!("{} axis {name} should be enum, got {other:?}", model.id),
    }
}

fn assert_range_parameter(
    model: &MediaModelDescriptor,
    name: &str,
    label: &str,
    min: f64,
    max: f64,
    step: f64,
    default: f64,
    request_field: &str,
) {
    let axis = model
        .axes
        .iter()
        .find(|axis| axis.id == name)
        .unwrap_or_else(|| panic!("{} should declare axis {name}", model.id));

    assert_eq!(axis.label, label);
    assert_eq!(axis.role, AxisRole::Param);
    assert_eq!(axis.request_field.as_deref(), Some(request_field));
    assert_eq!(axis.wire_type, WireType::Number);
    match &axis.control {
        ControlKind::Range {
            min: actual_min,
            max: actual_max,
            step: actual_step,
            default: actual_default,
        } => {
            assert_eq!(
                (*actual_min, *actual_max, *actual_step, *actual_default),
                (min, max, step, default)
            );
        }
        other => panic!("{} axis {name} should be range, got {other:?}", model.id),
    }
}

fn assert_bool_axis(
    model: &MediaModelDescriptor,
    name: &str,
    label: &str,
    default: bool,
    role: AxisRole,
) {
    let axis = model
        .axes
        .iter()
        .find(|axis| axis.id == name)
        .unwrap_or_else(|| panic!("{} should declare axis {name}", model.id));

    assert_eq!(axis.label, label);
    assert_eq!(axis.role, role);
    assert_eq!(axis.request_field, None);
    match &axis.control {
        ControlKind::Bool {
            default: actual_default,
        } => assert_eq!(*actual_default, default),
        other => panic!("{} axis {name} should be bool, got {other:?}", model.id),
    }
}

fn assert_task5_image_model_uses_canonical_axes(provider_id: &str, model: &MediaModelDescriptor) {
    assert!(
        model.max_outputs.is_some_and(|max| max <= 9),
        "{provider_id}/{} should declare max_outputs at or below 9",
        model.id
    );
    let axis_ids = model
        .axes
        .iter()
        .map(|axis| axis.id.as_str())
        .collect::<BTreeSet<_>>();
    for raw_axis in RAW_IMAGE_AXIS_IDS {
        assert!(
            !axis_ids.contains(raw_axis),
            "{provider_id}/{} must not expose raw image provider axis {raw_axis}",
            model.id
        );
    }
    for axis in &model.axes {
        match axis.id.as_str() {
            "mode" => {
                assert_eq!(axis.label, "Mode");
                assert_eq!(axis.role, AxisRole::Param);
                assert_eq!(axis.request_field, None);
            }
            "ratio" => {
                assert_eq!(axis.label, "Ratio");
                assert_eq!(axis.role, AxisRole::Param);
                assert_eq!(axis.request_field, None);
                if let ControlKind::Enum { values, default } = &axis.control {
                    assert!(
                        values
                            .iter()
                            .all(|value| CANONICAL_MEDIA_RATIOS.contains(&value.as_str())),
                        "{provider_id}/{} ratio values must be canonical",
                        model.id
                    );
                    assert!(
                        values.contains(default),
                        "{provider_id}/{} ratio default must be declared",
                        model.id
                    );
                } else {
                    panic!("{provider_id}/{} ratio axis should be enum", model.id);
                }
            }
            other => panic!(
                "{provider_id}/{} should expose only canonical image axes, got {other}",
                model.id
            ),
        }
    }
    assert!(
        axis_ids.contains("ratio"),
        "{provider_id}/{} should expose a canonical ratio axis",
        model.id
    );
    assert!(
        model.media_map.is_some(),
        "{provider_id}/{} should map canonical axes through media_map",
        model.id
    );
}

fn assert_ratio_media_map_value(
    model: &MediaModelDescriptor,
    field: &str,
    ratio: &str,
    value: Option<&str>,
) {
    let ratio_map = model
        .media_map
        .as_ref()
        .and_then(|media_map| media_map.ratio.as_ref())
        .unwrap_or_else(|| panic!("{} should declare media_map.ratio", model.id));
    assert_eq!(ratio_map.field, field);
    assert_eq!(
        ratio_map
            .values
            .get(ratio)
            .and_then(|value| value.as_deref()),
        value,
        "{} should map ratio {ratio}",
        model.id
    );
}

fn single_variant_base_params(model: &MediaModelDescriptor) -> &BTreeMap<String, String> {
    match &model.variants {
        Variants::Single(variant) => &variant.base_params,
        _ => panic!("{} should use a single variant", model.id),
    }
}

#[test]
fn all_provider_yamls_covers_bundled_provider_files() {
    let provider_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/providers");
    let actual = fs::read_dir(&provider_dir)
        .unwrap_or_else(|err| panic!("read {}: {err}", provider_dir.display()))
        .filter_map(|entry| {
            let path = entry.expect("provider dir entry").path();
            (path.extension().and_then(|extension| extension.to_str()) == Some("yaml"))
                .then_some(path)
        })
        .map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_else(|| panic!("provider yaml path must have a UTF-8 stem: {path:?}"))
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    let listed = ALL_PROVIDER_YAMLS
        .iter()
        .map(|(provider_id, _yaml)| (*provider_id).to_string())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        listed, actual,
        "ALL_PROVIDER_YAMLS must include every bundled provider YAML"
    );
}

#[test]
fn bundled_image_executions_declare_explicit_batch_mode() {
    for (provider_id, yaml) in [
        (
            "openai",
            include_str!("../../../resources/providers/openai.yaml"),
        ),
        ("xai", include_str!("../../../resources/providers/xai.yaml")),
        (
            "zhipu",
            include_str!("../../../resources/providers/zhipu.yaml"),
        ),
        (
            "byteplus",
            include_str!("../../../resources/providers/byteplus.yaml"),
        ),
        (
            "minimax",
            include_str!("../../../resources/providers/minimax.yaml"),
        ),
        (
            "minimax-cn",
            include_str!("../../../resources/providers/minimax-cn.yaml"),
        ),
        (
            "openrouter",
            include_str!("../../../resources/providers/openrouter.yaml"),
        ),
        (
            "vercel-ai-gateway",
            include_str!("../../../resources/providers/vercel-ai-gateway.yaml"),
        ),
        (
            "worldrouter",
            include_str!("../../../resources/providers/worldrouter.yaml"),
        ),
    ] {
        assert_raw_image_executions_declare_batch_mode(provider_id, yaml);
    }
}

#[test]
fn bundled_image_provider_descriptors_validate_and_do_not_duplicate_model_ids() {
    for &(provider_id, yaml) in IMAGE_PROVIDER_YAMLS {
        let descriptor = provider_descriptor(provider_id, yaml);
        descriptor
            .validate_media_descriptors()
            .unwrap_or_else(|err| panic!("{provider_id} media descriptor validates: {err}"));
        let Some(image) = descriptor
            .media
            .as_ref()
            .and_then(|media| media.image.as_ref())
        else {
            continue;
        };
        let ids = image
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();
        let unique_ids = ids.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(
            ids.len(),
            unique_ids.len(),
            "{provider_id} image model ids must be unique"
        );
    }
}

#[test]
fn task5_image_providers_expose_only_canonical_product_axes() {
    for &(provider_id, yaml) in IMAGE_PROVIDER_YAMLS {
        if !TASK5_CANONICAL_IMAGE_PROVIDER_IDS.contains(&provider_id) {
            continue;
        }
        let descriptor = provider_descriptor(provider_id, yaml);
        descriptor
            .validate_media_descriptors()
            .unwrap_or_else(|err| panic!("{provider_id} media descriptor validates: {err}"));
        let image = descriptor
            .media
            .as_ref()
            .and_then(|media| media.image.as_ref())
            .unwrap_or_else(|| panic!("{provider_id} should declare image media"));
        for model in &image.models {
            assert_task5_image_model_uses_canonical_axes(provider_id, model);
        }
    }
}

#[test]
fn task5_video_providers_use_canonical_setting_labels() {
    for provider_id in ["byteplus", "relaydance", "worldrouter"] {
        let yaml = ALL_PROVIDER_YAMLS
            .iter()
            .find_map(|(id, yaml)| (*id == provider_id).then_some(*yaml))
            .unwrap_or_else(|| panic!("{provider_id} should be listed"));
        let descriptor = provider_descriptor(provider_id, yaml);
        descriptor
            .validate_media_descriptors()
            .unwrap_or_else(|err| panic!("{provider_id} media descriptor validates: {err}"));
        let video = descriptor
            .media
            .as_ref()
            .and_then(|media| media.video.as_ref())
            .unwrap_or_else(|| panic!("{provider_id} should declare video media"));
        for model in &video.models {
            for axis in &model.axes {
                match axis.id.as_str() {
                    "resolution" | "mode" => assert_eq!(axis.label, "Mode"),
                    "ratio" | "aspect_ratio" => assert_eq!(axis.label, "Ratio"),
                    "duration" | "duration_seconds" => assert_eq!(axis.label, "Duration"),
                    _ => {}
                }
            }
        }
    }
}

#[test]
fn relaydance_declares_executable_video_descriptor() {
    let descriptor = provider_descriptor(
        "relaydance",
        include_str!("../../../resources/providers/relaydance.yaml"),
    );
    descriptor
        .validate_media_descriptors()
        .expect("relaydance media descriptor validates");
    let video = descriptor
        .media
        .as_ref()
        .and_then(|media| media.video.as_ref())
        .expect("relaydance video media descriptor");
    let execution = video
        .execution
        .as_ref()
        .expect("relaydance video execution descriptor");

    assert_eq!(
        video.discovery.as_ref().map(|discovery| discovery.adapter),
        Some(MediaDiscoveryKind::Static)
    );
    assert_eq!(execution.adapter, MediaExecutionKind::RelaydanceVideo);
    assert_eq!(execution.path, "/v1/video/generations");

    let models_by_id = video
        .models
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        models_by_id.keys().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "doubao-seedance-2-0",
            "doubao-seedance-2-0-fast",
            "grok-imagine-video",
            "grok-imagine-video-1.5-preview",
            "happyhorse-1.0-t2v",
            "seedance-1-5-pro",
            "seedance-fast-nsfw",
            "seedance-nsfw",
        ])
    );

    let expected = [
        (
            "doubao-seedance-2-0",
            "Seedance 2.0",
            SEEDANCE_VIDEO_DURATIONS,
            &["720p", "1080p"][..],
            AxisRole::Selector,
            None,
        ),
        (
            "doubao-seedance-2-0-fast",
            "Seedance 2.0 Fast",
            SEEDANCE_VIDEO_DURATIONS,
            SEEDANCE_FAST_VIDEO_RESOLUTIONS,
            AxisRole::Param,
            Some("metadata.resolution"),
        ),
        (
            "seedance-1-5-pro",
            "Seedance 1.5 Pro",
            SEEDANCE_15_VIDEO_DURATIONS,
            SEEDANCE_VIDEO_RESOLUTIONS,
            AxisRole::Param,
            Some("metadata.resolution"),
        ),
        (
            "seedance-nsfw",
            "Seedance NSFW",
            SEEDANCE_VIDEO_DURATIONS,
            &["720p", "1080p"][..],
            AxisRole::Selector,
            None,
        ),
        (
            "seedance-fast-nsfw",
            "Seedance Fast NSFW",
            SEEDANCE_VIDEO_DURATIONS,
            SEEDANCE_FAST_VIDEO_RESOLUTIONS,
            AxisRole::Param,
            Some("metadata.resolution"),
        ),
    ];
    for (model_id, display_name, durations, resolutions, resolution_role, resolution_field) in
        expected
    {
        let model = models_by_id
            .get(model_id)
            .unwrap_or_else(|| panic!("relaydance should include {model_id}"));
        assert_eq!(model.display_name.as_deref(), Some(display_name));
        assert_eq!(model.operations, vec![MediaOperation::Generate]);
        assert_range_parameter(
            model,
            "duration",
            "Duration",
            durations.first().unwrap().parse::<f64>().unwrap(),
            durations.last().unwrap().parse::<f64>().unwrap(),
            1.0,
            5.0,
            "seconds",
        );
        assert_enum_axis(
            model,
            "resolution",
            "Mode",
            resolutions,
            resolutions.last().expect("resolution default"),
            resolution_role,
            resolution_field,
            WireType::String,
        );
        assert_enum_axis(
            model,
            "ratio",
            "Ratio",
            SEEDANCE_VIDEO_RATIOS,
            "16:9",
            AxisRole::Param,
            None,
            WireType::String,
        );
        assert_ratio_media_map_value(model, "metadata.ratio", "Auto", Some("adaptive"));
        if model_id == "seedance-1-5-pro" {
            assert_bool_axis(model, "audio", "Native audio", true, AxisRole::Selector);
        }
    }

    let prompt_only_expected = [
        ("grok-imagine-video", "Grok Imagine Video"),
        (
            "grok-imagine-video-1.5-preview",
            "Grok Imagine Video 1.5 Preview",
        ),
        ("happyhorse-1.0-t2v", "HappyHorse 1.0 Text to Video"),
    ];
    for (model_id, display_name) in prompt_only_expected {
        let model = models_by_id
            .get(model_id)
            .unwrap_or_else(|| panic!("relaydance should include {model_id}"));
        assert_eq!(model.display_name.as_deref(), Some(display_name));
        assert_eq!(model.operations, vec![MediaOperation::Generate]);
        assert!(
            model.axes.is_empty(),
            "{model_id} should stay prompt-only until RelayDance exposes parameter metadata"
        );
    }
}

#[test]
fn byteplus_declares_executable_video_descriptor() {
    let descriptor = provider_descriptor(
        "byteplus",
        include_str!("../../../resources/providers/byteplus.yaml"),
    );
    descriptor
        .validate_media_descriptors()
        .expect("byteplus media descriptor validates");
    let video = descriptor
        .media
        .as_ref()
        .and_then(|media| media.video.as_ref())
        .expect("byteplus video media descriptor");
    let execution = video
        .execution
        .as_ref()
        .expect("byteplus video execution descriptor");

    assert_eq!(
        video.discovery.as_ref().map(|discovery| discovery.adapter),
        Some(MediaDiscoveryKind::Static)
    );
    assert_eq!(execution.adapter, MediaExecutionKind::BytePlusVideo);
    assert_eq!(execution.path, "/contents/generations/tasks");

    let models_by_id = video
        .models
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        models_by_id.keys().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from(["dreamina-seedance-2-0", "dreamina-seedance-2-0-fast",])
    );

    let expected = [
        (
            "dreamina-seedance-2-0",
            "Dreamina Seedance 2.0",
            SEEDANCE_VIDEO_RESOLUTIONS,
        ),
        (
            "dreamina-seedance-2-0-fast",
            "Dreamina Seedance 2.0 Fast",
            SEEDANCE_FAST_VIDEO_RESOLUTIONS,
        ),
    ];
    for (model_id, display_name, resolutions) in expected {
        let model = models_by_id
            .get(model_id)
            .unwrap_or_else(|| panic!("byteplus should include {model_id}"));
        assert_eq!(model.display_name.as_deref(), Some(display_name));
        assert_eq!(model.operations, vec![MediaOperation::Generate]);
        assert_range_parameter(
            model, "duration", "Duration", 4.0, 15.0, 1.0, 5.0, "duration",
        );
        assert_enum_axis(
            model,
            "ratio",
            "Ratio",
            SEEDANCE_VIDEO_RATIOS,
            "Auto",
            AxisRole::Param,
            None,
            WireType::String,
        );
        assert_ratio_media_map_value(model, "ratio", "Auto", Some("adaptive"));
        assert_select_parameter(
            model,
            "resolution",
            "Mode",
            resolutions,
            "720p",
            "resolution",
        );
    }
}

#[test]
fn worldrouter_declares_executable_video_descriptor() {
    let descriptor = provider_descriptor(
        "worldrouter",
        include_str!("../../../resources/providers/worldrouter.yaml"),
    );
    descriptor
        .validate_media_descriptors()
        .expect("worldrouter media descriptor validates");
    let video = descriptor
        .media
        .as_ref()
        .and_then(|media| media.video.as_ref())
        .expect("worldrouter video media descriptor");
    let execution = video
        .execution
        .as_ref()
        .expect("worldrouter video execution descriptor");

    assert_eq!(
        video.discovery.as_ref().map(|discovery| discovery.adapter),
        Some(MediaDiscoveryKind::Static)
    );
    assert_eq!(execution.adapter, MediaExecutionKind::WorldRouterVideo);
    assert_eq!(execution.path, "/api/v3/contents/generations/tasks");
}

#[test]
fn only_executable_video_providers_declare_video_media() {
    let expected = BTreeSet::from(["byteplus", "relaydance", "worldrouter"]);
    for (provider_id, yaml) in ALL_PROVIDER_YAMLS {
        let descriptor = provider_descriptor(provider_id, yaml);
        let has_video = descriptor
            .media
            .as_ref()
            .and_then(|media| media.video.as_ref())
            .is_some();
        assert_eq!(
            has_video,
            expected.contains(provider_id),
            "{provider_id} must not declare media.video until a Puffer video adapter exists"
        );
    }
}

#[test]
fn native_image_providers_do_not_use_gateway_alias_model_ids() {
    let native_provider_ids = BTreeSet::from([
        "byteplus",
        "openai",
        "zhipu",
        "xai",
        "minimax",
        "minimax-cn",
    ]);

    for &(provider_id, yaml) in IMAGE_PROVIDER_YAMLS {
        if !native_provider_ids.contains(provider_id) {
            continue;
        }
        let descriptor = provider_descriptor(provider_id, yaml);
        let image = descriptor
            .media
            .as_ref()
            .and_then(|media| media.image.as_ref())
            .unwrap_or_else(|| panic!("{provider_id} should declare image media"));
        for model in &image.models {
            assert!(
                !model.id.contains('/'),
                "{provider_id} native image model {} must not be a gateway alias",
                model.id
            );
        }
    }
}

#[test]
fn vercel_static_image_models_have_images_json_execution_overrides() {
    let descriptor = provider_descriptor(
        "vercel-ai-gateway",
        include_str!("../../../resources/providers/vercel-ai-gateway.yaml"),
    );
    let image = descriptor
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .expect("vercel image media descriptor");

    assert_eq!(
        image.execution.as_ref().map(|execution| execution.adapter),
        Some(MediaExecutionKind::ChatImageOutput),
        "provider-level Vercel image execution should remain chat image output"
    );

    for model in &image.models {
        let execution = model
            .execution
            .as_ref()
            .unwrap_or_else(|| panic!("{} must override execution", model.id));
        assert_eq!(
            execution.adapter,
            MediaExecutionKind::ImagesJson,
            "{} must execute through Images JSON",
            model.id
        );
        assert_eq!(
            execution.path, "/images/generations",
            "{} must use the Vercel Images JSON path",
            model.id
        );
    }
}

#[test]
fn openrouter_remains_discovery_driven_without_static_image_fallbacks() {
    let descriptor = provider_descriptor(
        "openrouter",
        include_str!("../../../resources/providers/openrouter.yaml"),
    );
    let image = descriptor
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .expect("openrouter image media descriptor");
    assert!(
        image.models.is_empty(),
        "OpenRouter should not add static image fallback models"
    );
}

#[test]
fn zhipu_images_json_uses_per_image_batch_mode() {
    let descriptor = provider_descriptor(
        "zhipu",
        include_str!("../../../resources/providers/zhipu.yaml"),
    );
    let image = descriptor
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .expect("zhipu image media descriptor");
    let execution = image
        .execution
        .as_ref()
        .expect("zhipu image execution descriptor");

    assert_eq!(execution.adapter, MediaExecutionKind::ImagesJson);
    assert_eq!(execution.path, "/images/generations");
    assert_eq!(execution.batch.mode, MediaBatchMode::PerImage);
    assert_eq!(execution.batch.max_images_per_call, None);
}

#[test]
fn openai_catalog_declares_current_image_api_models() {
    let descriptor = provider_descriptor(
        "openai",
        include_str!("../../../resources/providers/openai.yaml"),
    );
    let image = descriptor
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .expect("openai image media descriptor");

    assert_eq!(
        image.execution.as_ref().map(|execution| execution.adapter),
        Some(MediaExecutionKind::ImagesJson)
    );
    assert_eq!(
        image
            .execution
            .as_ref()
            .map(|execution| execution.path.as_str()),
        Some("/v1/images/generations")
    );

    let expected = BTreeMap::from([
        ("chatgpt-image-latest", "ChatGPT Image Latest"),
        ("gpt-image-1", "GPT Image 1"),
        ("gpt-image-1-mini", "GPT Image 1 Mini"),
        ("gpt-image-1.5", "GPT Image 1.5"),
        ("gpt-image-2", "GPT Image 2"),
    ]);
    let models_by_id = image
        .models
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect::<BTreeMap<_, _>>();
    let expected_model_ids = expected.keys().copied().collect::<BTreeSet<_>>();
    let actual_model_ids = models_by_id.keys().copied().collect::<BTreeSet<_>>();

    assert_eq!(
        actual_model_ids, expected_model_ids,
        "OpenAI image catalog should mirror currently callable Image API model ids"
    );
    assert!(
        !actual_model_ids.contains("dall-e-2") && !actual_model_ids.contains("dall-e-3"),
        "OpenAI image catalog should not include DALL-E models removed on 2026-05-12"
    );

    for (model_id, display_name) in &expected {
        let model_id = *model_id;
        let model = models_by_id
            .get(model_id)
            .unwrap_or_else(|| panic!("OpenAI should include {model_id}"));
        assert_eq!(model.display_name.as_deref(), Some(*display_name));
        assert!(
            model.operations.contains(&MediaOperation::Generate),
            "{model_id} should support image generation"
        );
        assert_task5_image_model_uses_canonical_axes("openai", model);
        assert_eq!(
            model.max_outputs,
            Some(9),
            "{model_id} should expose the global image output cap"
        );
        assert!(
            model
                .media_map
                .as_ref()
                .and_then(|media_map| media_map.size.as_ref())
                .is_some(),
            "{model_id} should map Mode + Ratio to the Images API size field"
        );
        if model_id == "gpt-image-2" {
            assert_enum_axis(
                model,
                "mode",
                "Mode",
                &["1K SD", "2K HD"],
                "1K SD",
                AxisRole::Param,
                None,
                WireType::String,
            );
        } else {
            assert_enum_axis(
                model,
                "mode",
                "Mode",
                &["1K SD"],
                "1K SD",
                AxisRole::Param,
                None,
                WireType::String,
            );
        }
    }
}

#[test]
fn worldrouter_catalog_declares_all_documented_image_models() {
    let descriptor = provider_descriptor(
        "worldrouter",
        include_str!("../../../resources/providers/worldrouter.yaml"),
    );
    let image = descriptor
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .expect("worldrouter image media descriptor");

    assert_eq!(
        image.execution.as_ref().map(|execution| execution.adapter),
        Some(MediaExecutionKind::ImagesJson)
    );
    assert_eq!(
        image
            .execution
            .as_ref()
            .map(|execution| execution.path.as_str()),
        Some("/images/generations")
    );

    let expected = BTreeMap::from([
        ("gemini-2.5-flash-image", "Gemini 2.5 Flash Image"),
        ("gemini-3-pro-image-preview", "Gemini 3 Pro Image Preview"),
        (
            "gemini-3.1-flash-image-preview",
            "Gemini 3.1 Flash Image Preview",
        ),
        ("gpt-image-2", "GPT Image 2"),
    ]);
    let models_by_id = image
        .models
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect::<BTreeMap<_, _>>();
    let expected_model_ids = expected.keys().copied().collect::<BTreeSet<_>>();
    let actual_model_ids = models_by_id.keys().copied().collect::<BTreeSet<_>>();

    assert_eq!(
        actual_model_ids, expected_model_ids,
        "WorldRouter image catalog should mirror documented generation model ids"
    );

    for (model_id, display_name) in &expected {
        let model_id = *model_id;
        let model = models_by_id
            .get(model_id)
            .unwrap_or_else(|| panic!("WorldRouter should include {model_id}"));
        assert_eq!(model.display_name.as_deref(), Some(*display_name));
        assert!(
            model.operations.contains(&MediaOperation::Generate),
            "{model_id} should support image generation"
        );
        assert_eq!(
            model.max_outputs,
            Some(9),
            "{model_id} should expose the global image output cap"
        );
        if model_id == "gpt-image-2" {
            assert_eq!(model.execution, None);
            assert_enum_axis(
                model,
                "ratio",
                "Ratio",
                &["1:1", "3:2", "2:3"],
                "1:1",
                AxisRole::Param,
                None,
                WireType::String,
            );
        } else {
            let execution = model
                .execution
                .as_ref()
                .unwrap_or_else(|| panic!("{model_id} should override execution"));
            assert_eq!(
                execution.adapter,
                MediaExecutionKind::GeminiGenerateContent,
                "{model_id} should use Gemini generateContent"
            );
            assert_eq!(
                execution.base_url.as_deref(),
                Some("https://inference-api.worldrouter.ai"),
                "{model_id} should override the OpenAI /v1 base URL"
            );
            assert_eq!(
                execution.path, "/v1beta/models/{model}:generateContent",
                "{model_id} should use the native Gemini route"
            );
        }
    }

    assert_enum_axis(
        models_by_id["gemini-2.5-flash-image"],
        "ratio",
        "Ratio",
        &["1:1"],
        "1:1",
        AxisRole::Param,
        Some("aspectRatio"),
        WireType::String,
    );
    assert!(
        models_by_id["gemini-2.5-flash-image"]
            .axes
            .iter()
            .all(|axis| axis.request_field.as_deref() != Some("imageSize")),
        "gemini-2.5-flash-image must not send imageSize"
    );
    assert_enum_axis(
        models_by_id["gemini-3.1-flash-image-preview"],
        "mode",
        "Mode",
        &["0.5K", "1K", "2K", "4K"],
        "2K",
        AxisRole::Param,
        Some("imageSize"),
        WireType::String,
    );
}

#[test]
fn byteplus_catalog_declares_only_current_native_seedream_models() {
    let descriptor = provider_descriptor(
        "byteplus",
        include_str!("../../../resources/providers/byteplus.yaml"),
    );
    let image = descriptor
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .expect("byteplus image media descriptor");

    assert_eq!(
        image.execution.as_ref().map(|execution| execution.adapter),
        Some(MediaExecutionKind::ImagesJson)
    );
    assert_eq!(
        image
            .execution
            .as_ref()
            .map(|execution| execution.path.as_str()),
        Some("/images/generations")
    );

    let expected = BTreeMap::from([
        ("seedream-5-0-260128", "Seedream 5.0 Lite"),
        ("seedream-4-5-251128", "Seedream 4.5"),
        ("seedream-4-0-250828", "Seedream 4.0"),
    ]);
    let models_by_id = image
        .models
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect::<BTreeMap<_, _>>();
    let expected_model_ids = expected.keys().copied().collect::<BTreeSet<_>>();
    let actual_model_ids = models_by_id.keys().copied().collect::<BTreeSet<_>>();

    assert_eq!(
        actual_model_ids, expected_model_ids,
        "BytePlus image catalog should exactly match the current native Seedream allowlist"
    );

    for (model_id, display_name) in &expected {
        let model_id = *model_id;
        let display_name = *display_name;
        let model = models_by_id
            .get(model_id)
            .unwrap_or_else(|| panic!("BytePlus should include {model_id}"));
        assert_eq!(model.display_name.as_deref(), Some(display_name));
        assert!(
            model.operations.contains(&MediaOperation::Generate),
            "{model_id} should support image generation"
        );
        assert_task5_image_model_uses_canonical_axes("byteplus", model);
        assert_eq!(
            model.max_outputs,
            Some(9),
            "{model_id} should expose the global image output cap"
        );
        assert_enum_axis(
            model,
            "mode",
            "Mode",
            &["2K HD"],
            "2K HD",
            AxisRole::Param,
            None,
            WireType::String,
        );
        if model_id == "seedream-5-0-260128" {
            assert_eq!(
                single_variant_base_params(model)
                    .get("output_format")
                    .map(String::as_str),
                Some("jpeg"),
                "{model_id} should keep output_format hidden in base_params"
            );
        } else {
            assert!(
                single_variant_base_params(model)
                    .get("output_format")
                    .is_none(),
                "{model_id} should not synthesize unsupported output_format"
            );
        }
    }
}
