use super::*;
use crate::media::MediaKind;
use puffer_provider_registry::{
    AuthStore, ControlKind, MediaModelDescriptor, MediaOperation, ProviderDescriptor,
    ProviderRegistry, Variant, Variants, WireType,
};

const RELAYDANCE_YAML: &str = r#"
id: relaydance
display_name: Relaydance
base_url: https://relaydance.com
default_api: openai-completions
auth_modes: [api_key]
media:
  video:
    discovery: { adapter: static }
    execution: { adapter: relaydance_video, path: /v1/video/generations }
    models:
      - id: seedance-1-5-pro
        display_name: Seedance 1.5 Pro
        operations: [generate]
        axes:
          - { id: resolution, label: Mode, role: param, control: !enum { values: ["480p", "720p", "1080p"], default: "1080p" }, request_field: metadata.resolution }
          - { id: duration, label: Length, role: param, control: !range { min: 4.0, max: 12.0, step: 1.0, default: 5.0 }, request_field: seconds, wire_type: number }
          - { id: ratio, label: Video ratio, role: param, control: !enum { values: ["16:9", "9:16"], default: "16:9" }, request_field: metadata.ratio }
          - { id: audio, label: Native audio, role: selector, control: !bool { default: true } }
        variants:
          selector: audio
          map:
            "true": { model_id: seedance-1-5-pro-with-audio }
            "false": { model_id: seedance-1-5-pro-no-audio }
"#;

const TIERED_VIDEO_YAML: &str = r#"
id: tiered-video
display_name: Tiered Video
base_url: https://example.invalid
default_api: openai-completions
auth_modes: [api_key]
media:
  video:
    discovery: { adapter: static }
    execution: { adapter: replicate_video, path: /v1/videos/generations }
    models:
      - id: tiered-2-1
        display_name: Tiered 2.1
        operations: [generate]
        axes:
          - { id: tier, label: Quality, role: selector, control: !enum { values: ["std", "pro"], default: "std" } }
          - { id: duration, label: Length, role: param, control: !enum { values: ["5", "10"], default: "5" }, request_field: duration, wire_type: number }
        variants:
          selector: tier
          map:
            "std": { model_id: tiered-2-1-std, base_params: { resolution: "720p" } }
            "pro": { model_id: tiered-2-1-pro, base_params: { resolution: "1080p" } }
"#;

const OPENAI_IMAGE_YAML: &str = r#"
id: openai
display_name: OpenAI
base_url: https://api.openai.com
default_api: openai-responses
auth_modes: [api_key]
media:
  image:
    discovery: { adapter: static }
    execution: { adapter: images_json, path: /v1/images/generations }
    models:
      - id: gpt-image-1
        display_name: GPT Image 1
        operations: [generate]
        axes:
          - { id: size, label: Size, role: param, control: !enum { values: ["1024x1024", "1536x1024"], default: "1024x1024" }, request_field: size }
          - { id: quality, label: Quality, role: param, control: !enum { values: ["auto", "high"], default: "auto" }, request_field: quality }
        variants: { model_id: gpt-image-1 }
"#;

const CANONICAL_IMAGE_YAML: &str = r#"
id: openai
display_name: OpenAI
base_url: https://api.openai.com
default_api: openai-responses
auth_modes: [api_key]
media:
  image:
    discovery: { adapter: static }
    execution: { adapter: images_json, path: /v1/images/generations }
    models:
      - id: gpt-image-1
        display_name: GPT Image 1
        operations: [generate]
        max_outputs: 4
        axes:
          - { id: mode, label: Mode, role: param, control: !enum { values: ["1K SD", "2K HD"], default: "1K SD" } }
          - { id: ratio, label: Ratio, role: param, control: !enum { values: ["Auto", "1:1", "16:9", "21:9"], default: "Auto" } }
        media_map:
          size:
            field: size
            values:
              "1K SD":
                Auto: null
                "1:1": "1024x1024"
                "16:9": "1536x864"
              "2K HD":
                Auto: null
                "1:1": "2048x2048"
        variants: { model_id: gpt-image-1 }
"#;

const CANONICAL_RATIO_IMAGE_YAML: &str = r#"
id: minimax
display_name: MiniMax
base_url: https://api.minimax.io
default_api: anthropic-messages
auth_modes: [api_key]
media:
  image:
    execution: { adapter: minimax_image, path: /v1/image_generation }
    models:
      - id: image-01
        display_name: Image 01
        operations: [generate]
        max_outputs: 4
        axes:
          - { id: ratio, label: Ratio, role: param, control: !enum { values: ["Auto", "1:1", "16:9"], default: "Auto" } }
        media_map:
          ratio:
            field: aspect_ratio
            values:
              Auto: null
              "1:1": "1:1"
              "16:9": "16:9"
        variants:
          model_id: image-01
          base_params:
            response_format: base64
"#;

const DISCOVERED_IMAGE_PROVIDER_YAML: &str = r#"
id: openrouter
display_name: OpenRouter
base_url: https://openrouter.ai/api
default_api: openai-completions
auth_modes: [api_key]
media:
  image:
    execution: { adapter: chat_image_output, path: /chat/completions }
    models: []
"#;

fn registry_from_yaml(yamls: &[&str]) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    let providers: Vec<ProviderDescriptor> = yamls
        .iter()
        .map(|yaml| serde_yaml::from_str::<ProviderDescriptor>(yaml).expect("provider parses"))
        .collect();
    registry.register_many(providers);
    registry
}

fn auth_for(provider_ids: &[&str]) -> AuthStore {
    let mut auth = AuthStore::default();
    for id in provider_ids {
        auth.set_api_key(*id, "sk-test");
    }
    auth
}

fn test_video_registry() -> (ProviderRegistry, AuthStore) {
    (
        registry_from_yaml(&[RELAYDANCE_YAML, TIERED_VIDEO_YAML]),
        auth_for(&["relaydance", "tiered-video"]),
    )
}

fn test_image_registry() -> (ProviderRegistry, AuthStore) {
    (
        registry_from_yaml(&[OPENAI_IMAGE_YAML]),
        auth_for(&["openai"]),
    )
}

fn btree(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn resolved_video_capability_carries_axes_from_descriptor() {
    let (registry, auth) = test_video_registry();
    let caps = resolve_media_capabilities(
        &registry,
        &auth,
        MediaKind::Video,
        MediaOperation::Generate,
        0,
        &MediaDiscoveryCache::default(),
    );
    let pro = caps
        .iter()
        .find(|c| c.model_id == "seedance-1-5-pro")
        .expect("logical");
    assert!(pro.axes.iter().any(|a| a.id == "audio"));
    assert!(!pro.adapter.is_empty());
}

#[test]
fn connected_exact_image_capability_carries_axes() {
    let (registry, auth) = test_image_registry();
    let caps = resolve_media_capabilities(
        &registry,
        &auth,
        MediaKind::Image,
        MediaOperation::Generate,
        42,
        &MediaDiscoveryCache::default(),
    );
    assert_eq!(caps.len(), 1);
    assert_eq!(caps[0].provider_id, "openai");
    assert_eq!(caps[0].model_id, "gpt-image-1");
    assert_eq!(caps[0].adapter, "images_json");
    assert_eq!(caps[0].status, "available");
    assert!(caps[0].axes.iter().any(|a| a.id == "size"));
}

#[test]
fn image_capability_synthesizes_output_and_filters_ratio_values() {
    let registry = registry_from_yaml(&[CANONICAL_IMAGE_YAML]);
    let auth = auth_for(&["openai"]);
    let caps = resolve_media_capabilities(
        &registry,
        &auth,
        MediaKind::Image,
        MediaOperation::Generate,
        42,
        &MediaDiscoveryCache::default(),
    );

    assert_eq!(caps.len(), 1);
    let mode = caps[0]
        .axes
        .iter()
        .find(|axis| axis.id == "mode")
        .expect("mode axis");
    assert!(matches!(
        &mode.control,
        ControlKind::Enum { values, default }
            if values == &vec!["1K SD".to_string(), "2K HD".to_string()] && default == "1K SD"
    ));
    let ratio = caps[0]
        .axes
        .iter()
        .find(|axis| axis.id == "ratio")
        .expect("ratio axis");
    assert!(matches!(
        &ratio.control,
        ControlKind::Enum { values, default }
            if values == &vec!["Auto".to_string(), "1:1".to_string()]
                && default == "Auto"
    ));
    let output = caps[0]
        .axes
        .iter()
        .find(|axis| axis.id == "output")
        .expect("output axis");
    assert_eq!(output.label, "Output");
    assert!(output.request_field.is_none());
    assert!(matches!(
        output.control,
        ControlKind::Range { min, max, step, default }
            if min == 1.0 && max == 4.0 && step == 1.0 && default == 1.0
    ));
}

#[test]
fn video_capability_normalizes_core_axis_labels() {
    let (registry, auth) = test_video_registry();
    let caps = resolve_media_capabilities(
        &registry,
        &auth,
        MediaKind::Video,
        MediaOperation::Generate,
        0,
        &MediaDiscoveryCache::default(),
    );
    let pro = caps
        .iter()
        .find(|c| c.model_id == "seedance-1-5-pro")
        .expect("logical");

    assert_eq!(
        pro.axes
            .iter()
            .find(|axis| axis.id == "resolution")
            .map(|axis| axis.label.as_str()),
        Some("Mode")
    );
    assert_eq!(
        pro.axes
            .iter()
            .find(|axis| axis.id == "duration")
            .map(|axis| axis.label.as_str()),
        Some("Duration")
    );
    assert_eq!(
        pro.axes
            .iter()
            .find(|axis| axis.id == "ratio")
            .map(|axis| axis.label.as_str()),
        Some("Ratio")
    );
}

#[test]
fn discovered_image_output_capability_does_not_infer_mode_or_ratio_axes() {
    let registry = registry_from_yaml(&[DISCOVERED_IMAGE_PROVIDER_YAML]);
    let auth = auth_for(&["openrouter"]);
    let discovery_cache = MediaDiscoveryCache {
        image_models: vec![CachedImageMediaModel {
            provider_id: "openrouter".to_string(),
            model: MediaModelDescriptor {
                id: "openrouter/image-chat".to_string(),
                display_name: Some("Image Chat".to_string()),
                max_outputs: None,
                execution: None,
                operations: vec![MediaOperation::Generate],
                axes: Vec::new(),
                media_map: None,
                variants: Variants::Single(Variant {
                    model_id: "openrouter/image-chat".to_string(),
                    base_params: BTreeMap::new(),
                }),
            },
            source: "provider_discovery".to_string(),
        }],
    };

    let caps = resolve_media_capabilities(
        &registry,
        &auth,
        MediaKind::Image,
        MediaOperation::Generate,
        42,
        &discovery_cache,
    );

    assert_eq!(caps.len(), 1);
    let axis_ids = caps[0]
        .axes
        .iter()
        .map(|axis| axis.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(axis_ids, vec!["output"]);
}

#[test]
fn resolves_audio_selector_to_concrete_model() {
    let (registry, auth) = test_video_registry();
    let r = resolve_media_request(
        &registry,
        &auth,
        "relaydance",
        "seedance-1-5-pro",
        MediaKind::Video,
        &btree(&[
            ("audio", "false"),
            ("resolution", "720p"),
            ("duration", "6"),
        ]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap();
    assert_eq!(r.model_id, "seedance-1-5-pro-no-audio");
    assert_eq!(r.adapter, "relaydance_video");
    assert_eq!(r.parameters["metadata.resolution"], "720p");
    assert_eq!(r.parameters["seconds"], "6");
    assert_eq!(r.parameter_wire_types["seconds"], WireType::Number);
}

#[test]
fn resolves_defaults_for_unset_axes() {
    let (registry, auth) = test_video_registry();
    let r = resolve_media_request(
        &registry,
        &auth,
        "relaydance",
        "seedance-1-5-pro",
        MediaKind::Video,
        &btree(&[]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap();
    // audio default true -> with-audio variant
    assert_eq!(r.model_id, "seedance-1-5-pro-with-audio");
    assert_eq!(r.parameters["metadata.resolution"], "1080p");
    assert_eq!(r.parameters["seconds"], "5");
    assert_eq!(r.parameters["metadata.ratio"], "16:9");
}

#[test]
fn resolves_selector_base_params() {
    let (registry, auth) = test_video_registry();
    let r = resolve_media_request(
        &registry,
        &auth,
        "tiered-video",
        "tiered-2-1",
        MediaKind::Video,
        &btree(&[("tier", "pro"), ("duration", "10")]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap();
    assert_eq!(r.model_id, "tiered-2-1-pro");
    assert_eq!(r.parameters["resolution"], "1080p");
    assert_eq!(r.parameters["duration"], "10");
    assert_eq!(r.parameter_wire_types["duration"], WireType::Number);
}

#[test]
fn resolves_image_request_to_request_field_params() {
    let (registry, auth) = test_image_registry();
    let r = resolve_media_request(
        &registry,
        &auth,
        "openai",
        "gpt-image-1",
        MediaKind::Image,
        &btree(&[("size", "1536x1024")]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap();
    assert_eq!(r.model_id, "gpt-image-1");
    assert_eq!(r.adapter, "images_json");
    assert_eq!(r.parameters["size"], "1536x1024");
    assert_eq!(r.parameters["quality"], "auto");
}

#[test]
fn resolves_size_media_map_and_output_count() {
    let registry = registry_from_yaml(&[CANONICAL_IMAGE_YAML]);
    let auth = auth_for(&["openai"]);
    let r = resolve_media_request(
        &registry,
        &auth,
        "openai",
        "gpt-image-1",
        MediaKind::Image,
        &btree(&[
            ("mode", "2K HD"),
            ("ratio", "1:1"),
            ("output", "3"),
            ("output_format", "png"),
        ]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap();

    assert_eq!(r.model_id, "gpt-image-1");
    assert_eq!(r.parameters["size"], "2048x2048");
    assert!(!r.parameters.contains_key("mode"));
    assert!(!r.parameters.contains_key("ratio"));
    assert!(!r.parameters.contains_key("output"));
    assert!(!r.parameters.contains_key("output_format"));
    assert_eq!(r.count, 3);
}

#[test]
fn resolves_ratio_media_map_and_omits_auto_null_field() {
    let registry = registry_from_yaml(&[CANONICAL_RATIO_IMAGE_YAML]);
    let auth = auth_for(&["minimax"]);
    let mapped = resolve_media_request(
        &registry,
        &auth,
        "minimax",
        "image-01",
        MediaKind::Image,
        &btree(&[("ratio", "16:9")]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap();
    let auto = resolve_media_request(
        &registry,
        &auth,
        "minimax",
        "image-01",
        MediaKind::Image,
        &btree(&[("ratio", "Auto")]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap();

    assert_eq!(mapped.parameters["aspect_ratio"], "16:9");
    assert_eq!(mapped.parameters["response_format"], "base64");
    assert!(!auto.parameters.contains_key("aspect_ratio"));
    assert_eq!(auto.parameters["response_format"], "base64");
}

#[test]
fn rejects_invalid_canonical_ratio_mode_and_output_before_dispatch() {
    let registry = registry_from_yaml(&[CANONICAL_IMAGE_YAML]);
    let auth = auth_for(&["openai"]);

    let bad_ratio = resolve_media_request(
        &registry,
        &auth,
        "openai",
        "gpt-image-1",
        MediaKind::Image,
        &btree(&[("ratio", "16:9")]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap_err();
    let bad_mode = resolve_media_request(
        &registry,
        &auth,
        "openai",
        "gpt-image-1",
        MediaKind::Image,
        &btree(&[("mode", "4K UHD")]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap_err();
    let bad_output = resolve_media_request(
        &registry,
        &auth,
        "openai",
        "gpt-image-1",
        MediaKind::Image,
        &btree(&[("output", "5")]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap_err();

    assert!(bad_ratio.to_string().contains("ratio"), "{bad_ratio}");
    assert!(bad_mode.to_string().contains("mode"), "{bad_mode}");
    assert!(bad_output.to_string().contains("output"), "{bad_output}");
}

#[test]
fn rejects_out_of_range_duration() {
    let (registry, auth) = test_video_registry();
    let err = resolve_media_request(
        &registry,
        &auth,
        "relaydance",
        "seedance-1-5-pro",
        MediaKind::Video,
        &btree(&[("audio", "true"), ("duration", "99")]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("duration"), "{err}");
}

#[test]
fn rejects_unknown_selector_value() {
    let (registry, auth) = test_video_registry();
    let err = resolve_media_request(
        &registry,
        &auth,
        "tiered-video",
        "tiered-2-1",
        MediaKind::Video,
        &btree(&[("tier", "ultra")]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("tier"), "{err}");
}

#[test]
fn rejects_unknown_logical_model() {
    let (registry, auth) = test_video_registry();
    let err = resolve_media_request(
        &registry,
        &auth,
        "relaydance",
        "no-such-model",
        MediaKind::Video,
        &btree(&[]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown media model"), "{err}");
}

#[test]
fn unauthenticated_video_capability_is_missing_auth() {
    let registry = registry_from_yaml(&[RELAYDANCE_YAML]);
    let caps = resolve_media_capabilities(
        &registry,
        &AuthStore::default(),
        MediaKind::Video,
        MediaOperation::Generate,
        0,
        &MediaDiscoveryCache::default(),
    );
    let pro = caps
        .iter()
        .find(|c| c.model_id == "seedance-1-5-pro")
        .expect("pro");
    assert_eq!(pro.status, "unavailable");
    assert_eq!(pro.reason.as_deref(), Some("missing_auth"));
}

#[test]
fn resolve_request_rejects_unavailable_capability() {
    let registry = registry_from_yaml(&[RELAYDANCE_YAML]);
    let err = resolve_media_request(
        &registry,
        &AuthStore::default(),
        "relaydance",
        "seedance-1-5-pro",
        MediaKind::Video,
        &btree(&[]),
        &MediaDiscoveryCache::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("unavailable"), "{err}");
}

#[test]
fn video_execution_descriptor_resolves_for_variant_id() {
    let (registry, _auth) = test_video_registry();
    let (provider, execution) = resolve_video_execution_descriptor(
        &registry,
        "relaydance",
        "seedance-1-5-pro-no-audio",
        "relaydance_video",
    )
    .expect("variant id resolves to provider execution");
    assert_eq!(provider.id, "relaydance");
    assert_eq!(execution.path, "/v1/video/generations");
}

#[test]
fn image_execution_descriptor_resolves() {
    let (registry, _auth) = test_image_registry();
    let (provider, execution) = resolve_image_execution_descriptor(
        &registry,
        "openai",
        "gpt-image-1",
        "images_json",
        &MediaDiscoveryCache::default(),
    )
    .expect("image execution resolves");
    assert_eq!(provider.id, "openai");
    assert_eq!(execution.path, "/v1/images/generations");
}
