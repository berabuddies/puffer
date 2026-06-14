use super::*;
use crate::{ControlKind, WireType};

fn provider_with_media_yaml(media_yaml: &str) -> String {
    format!(
        r#"
id: test-provider
display_name: Test Provider
base_url: https://api.test-provider.example
default_api: openai-responses
auth_modes:
  - api_key
models: []
{media_yaml}
"#
    )
}

fn provider_with_basic_image_execution(execution_yaml: &str) -> String {
    let media_yaml = format!(
        r#"
media:
  image:
    execution:
{execution_yaml}
    models:
      - id: gpt-image-1
        operations:
          - generate
        variants: {{ model_id: gpt-image-1 }}
"#
    );
    provider_with_media_yaml(&media_yaml)
}

#[test]
fn existing_provider_yaml_parses_without_media() {
    let yaml = include_str!("../../../resources/providers/anthropic.yaml");
    let provider: ProviderDescriptor = serde_yaml::from_str(yaml).expect("anthropic yaml parses");

    assert_eq!(provider.id, "anthropic");
    assert!(provider.media.is_none());
    provider
        .validate_media_descriptors()
        .expect("missing media is valid");
}

#[test]
fn media_model_descriptor_carries_axes_and_variants() {
    let yaml = r#"
id: seedance-1-5-pro
display_name: Seedance 1.5 Pro
operations: [generate]
axes:
  - { id: audio, label: Native audio, role: selector, control: !bool { default: true } }
variants:
  selector: audio
  map:
    "true": { model_id: seedance-1-5-pro-with-audio }
    "false": { model_id: seedance-1-5-pro-no-audio }
"#;
    let model: MediaModelDescriptor = serde_yaml::from_str(yaml).expect("parse");
    assert_eq!(model.id, "seedance-1-5-pro");
    assert_eq!(model.axes.len(), 1);
}

#[test]
fn validate_rejects_selector_value_without_variant_key() {
    // audio axis carries true/false but map only has "true" → invalid.
    let yaml = r#"
id: bad
axes:
  - { id: audio, label: Audio, role: selector, control: !bool { default: true } }
variants:
  selector: audio
  map:
    "true": { model_id: x-with-audio }
"#;
    let model: MediaModelDescriptor = serde_yaml::from_str(yaml).expect("parse");
    assert!(validate_one_media_model(&model).is_err());
}

#[test]
fn valid_image_media_descriptor_parses_and_validates() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    discovery:
      adapter: static
    execution:
      adapter: images_json
      base_url: https://api.test-provider.example
      path: /v1/images/generations
      batch:
        mode: exact
        max_images_per_call: 4
    models:
      - id: gpt-image-1
        display_name: GPT Image 1
        operations:
          - generate
        axes:
          - { id: size, label: Size, role: param, control: !enum { values: ["1024x1024","1536x1024"], default: "1024x1024" }, request_field: size }
          - { id: quality, label: Quality, role: param, control: !enum { values: ["auto","high"], default: "auto" }, request_field: quality }
          - { id: output_format, label: Output format, role: param, control: !enum { values: ["png","jpeg"], default: "png" }, request_field: output_format }
        variants: { model_id: gpt-image-1 }
"#,
    );

    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    provider
        .validate_media_descriptors()
        .expect("media descriptor validates");
    let image = provider
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .expect("image media");
    assert_eq!(
        image.execution.as_ref().map(|execution| execution.adapter),
        Some(MediaExecutionKind::ImagesJson)
    );
    assert_eq!(
        image
            .execution
            .as_ref()
            .and_then(|execution| execution.base_url.as_deref()),
        Some("https://api.test-provider.example")
    );
    let execution = image.execution.as_ref().expect("image execution");
    assert_eq!(execution.batch.mode, MediaBatchMode::Exact);
    assert_eq!(execution.batch.max_images_per_call, Some(4));
    assert_eq!(image.models[0].operations, vec![MediaOperation::Generate]);
    assert_eq!(image.models[0].axes[0].id, "size");
}

#[test]
fn image_media_descriptor_accepts_canonical_media_map() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: images_json
      path: /v1/images/generations
    models:
      - id: gpt-image-1
        display_name: GPT Image 1
        operations: [generate]
        max_outputs: 4
        axes:
          - { id: mode, label: Mode, role: param, control: !enum { values: ["1K SD","2K HD"], default: "1K SD" } }
          - { id: ratio, label: Ratio, role: param, control: !enum { values: ["Auto","1:1","16:9"], default: "Auto" } }
        media_map:
          size:
            field: size
            values:
              "1K SD":
                Auto: null
                "1:1": "1024x1024"
                "16:9": "1536x864"
              "2K HD":
                "1:1": "2048x2048"
                "16:9": "2048x1152"
        variants: { model_id: gpt-image-1 }
"#,
    );

    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    provider
        .validate_media_descriptors()
        .expect("canonical media map validates");
    let model = &provider
        .media
        .as_ref()
        .unwrap()
        .image
        .as_ref()
        .unwrap()
        .models[0];
    assert_eq!(model.max_outputs, Some(4));
    let size_map = model.media_map.as_ref().unwrap().size.as_ref().unwrap();
    assert_eq!(size_map.field, "size");
    assert_eq!(
        size_map.values["1K SD"]["1:1"].as_deref(),
        Some("1024x1024")
    );
    assert_eq!(size_map.values["1K SD"]["Auto"], None);
}

#[test]
fn validate_rejects_image_max_outputs_above_global_cap() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: images_json
      path: /v1/images/generations
    models:
      - id: gpt-image-1
        operations: [generate]
        max_outputs: 10
        axes:
          - { id: size, label: Size, role: param, control: !enum { values: ["1024x1024"], default: "1024x1024" }, request_field: size }
        variants: { model_id: gpt-image-1 }
"#,
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    let error = provider
        .validate_media_descriptors()
        .expect_err("max_outputs above nine is invalid");

    assert!(error.to_string().contains("max_outputs"), "{error}");
}

#[test]
fn validate_rejects_noncanonical_ratio_axis_values() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: images_json
      path: /v1/images/generations
    models:
      - id: gpt-image-1
        operations: [generate]
        axes:
          - { id: ratio, label: Ratio, role: param, control: !enum { values: ["1:1","5:4"], default: "1:1" }, request_field: aspect_ratio }
        variants: { model_id: gpt-image-1 }
"#,
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    let error = provider
        .validate_media_descriptors()
        .expect_err("ratio values must be canonical");

    assert!(error.to_string().contains("canonical ratio"), "{error}");
}

#[test]
fn validate_rejects_unmapped_mode_or_ratio_without_request_field() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: images_json
      path: /v1/images/generations
    models:
      - id: gpt-image-1
        operations: [generate]
        axes:
          - { id: mode, label: Mode, role: param, control: !enum { values: ["1K SD"], default: "1K SD" } }
          - { id: ratio, label: Ratio, role: param, control: !enum { values: ["1:1"], default: "1:1" } }
        variants: { model_id: gpt-image-1 }
"#,
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    let error = provider
        .validate_media_descriptors()
        .expect_err("unmapped canonical axes still need request fields");

    assert!(error.to_string().contains("request_field"), "{error}");
}

#[test]
fn validate_rejects_media_map_ratio_keys_outside_canonical_list() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: minimax_image
      path: /v1/image_generation
    models:
      - id: image-01
        operations: [generate]
        axes:
          - { id: ratio, label: Ratio, role: param, control: !enum { values: ["1:1"], default: "1:1" } }
        media_map:
          ratio:
            field: aspect_ratio
            values:
              "1:1": "1:1"
              "5:4": "5:4"
        variants: { model_id: image-01 }
"#,
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    let error = provider
        .validate_media_descriptors()
        .expect_err("media_map ratio keys must be canonical");

    assert!(error.to_string().contains("canonical ratio"), "{error}");
}

#[test]
fn validate_rejects_ratio_media_map_without_ratio_axis() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: minimax_image
      path: /v1/image_generation
    models:
      - id: image-01
        operations: [generate]
        axes:
          - { id: prompt_style, label: Style, role: param, control: !enum { values: ["default"], default: "default" }, request_field: style }
        media_map:
          ratio:
            field: aspect_ratio
            values:
              "1:1": "1:1"
        variants: { model_id: image-01 }
"#,
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    let error = provider
        .validate_media_descriptors()
        .expect_err("ratio map requires a ratio axis");

    assert!(error.to_string().contains("ratio axis"), "{error}");
}

#[test]
fn validate_rejects_size_media_map_without_mode_or_ratio_axes() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: images_json
      path: /v1/images/generations
    models:
      - id: gpt-image-1
        operations: [generate]
        axes:
          - { id: output_format, label: Output format, role: param, control: !enum { values: ["png"], default: "png" }, request_field: output_format }
        media_map:
          size:
            field: size
            values:
              "1K SD":
                "1:1": "1024x1024"
        variants: { model_id: gpt-image-1 }
"#,
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    let error = provider
        .validate_media_descriptors()
        .expect_err("size map requires mode and ratio axes");

    assert!(
        error.to_string().contains("mode axis") || error.to_string().contains("ratio axis"),
        "{error}"
    );
}

#[test]
fn validate_rejects_size_media_map_without_common_ratios() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: images_json
      path: /v1/images/generations
    models:
      - id: gpt-image-1
        operations: [generate]
        axes:
          - { id: mode, label: Mode, role: param, control: !enum { values: ["1K SD", "2K HD"], default: "1K SD" } }
          - { id: ratio, label: Ratio, role: param, control: !enum { values: ["1:1", "16:9"], default: "1:1" } }
        media_map:
          size:
            field: size
            values:
              "1K SD":
                "1:1": "1024x1024"
              "2K HD":
                "16:9": "2048x1152"
        variants: { model_id: gpt-image-1 }
"#,
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    let error = provider
        .validate_media_descriptors()
        .expect_err("independent mode and ratio axes need at least one common ratio");

    assert!(error.to_string().contains("common ratio"), "{error}");
}

#[test]
fn provider_media_descriptor_accepts_video_models() {
    let yaml = r#"
id: replicate
display_name: Replicate
base_url: https://api.replicate.com
default_api: openai-responses
auth_modes: [api_key]
media:
  video:
    execution:
      adapter: replicate_video
      path: /v1/predictions
    models:
      - id: owner/model-version
        display_name: Video Model
        operations: [generate]
        axes:
          - { id: aspect_ratio, label: Aspect ratio, role: param, control: !enum { values: ["16:9","9:16"], default: "16:9" }, request_field: aspect_ratio }
          - { id: duration, label: Duration, role: param, control: !enum { values: ["5","8"], default: "5" }, request_field: duration }
        variants: { model_id: owner/model-version }
"#;

    let provider: ProviderDescriptor = serde_yaml::from_str(yaml).expect("parse provider");
    provider
        .validate_media_descriptors()
        .expect("valid media descriptor");
    assert!(provider.media.unwrap().video.is_some());
}

#[test]
fn axis_wire_type_defaults_to_string() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  video:
    execution:
      adapter: replicate_video
      path: /v1/predictions
    models:
      - id: owner/model-version
        operations: [generate]
        axes:
          - { id: duration_seconds, label: Duration, role: param, control: !enum { values: ["5","8"], default: "5" }, request_field: duration }
        variants: { model_id: owner/model-version }
"#,
    );

    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("parse provider");
    let axis = &provider
        .media
        .as_ref()
        .and_then(|media| media.video.as_ref())
        .expect("video media")
        .models[0]
        .axes[0];

    assert_eq!(axis.wire_type, WireType::String);
    provider
        .validate_media_descriptors()
        .expect("default wire type validates");
}

#[test]
fn axis_wire_type_parses_number() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  video:
    execution:
      adapter: byteplus_video
      path: /contents/generations/tasks
    models:
      - id: dreamina-seedance-2-0-260128
        operations: [generate]
        axes:
          - { id: duration_seconds, label: Duration, role: param, control: !range { min: 4.0, max: 5.0, step: 1.0, default: 5.0 }, request_field: duration, wire_type: number }
        variants: { model_id: dreamina-seedance-2-0-260128 }
"#,
    );

    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("parse provider");
    let axis = &provider
        .media
        .as_ref()
        .and_then(|media| media.video.as_ref())
        .expect("video media")
        .models[0]
        .axes[0];

    assert_eq!(axis.wire_type, WireType::Number);
    provider
        .validate_media_descriptors()
        .expect("number wire type validates");
}

#[test]
fn validate_rejects_param_axis_without_request_field() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  video:
    execution:
      adapter: replicate_video
      path: /v1/predictions
    models:
      - id: owner/model-version
        operations: [generate]
        axes:
          - { id: duration, label: Duration, role: param, control: !enum { values: ["5"], default: "5" } }
        variants: { model_id: owner/model-version }
"#,
    );

    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("parse provider");
    let error = provider
        .validate_media_descriptors()
        .unwrap_err()
        .to_string();
    assert!(error.contains("request_field"), "{error}");
}

#[test]
fn missing_image_execution_batch_defaults_to_per_image() {
    let yaml = provider_with_basic_image_execution(
        "      adapter: images_json\n      path: /v1/images/generations",
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");
    let execution = provider
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .and_then(|image| image.execution.as_ref())
        .expect("image execution");

    assert_eq!(execution.batch.mode, MediaBatchMode::PerImage);
    assert_eq!(execution.batch.max_images_per_call, None);
    provider
        .validate_media_descriptors()
        .expect("default per-image batch validates");
}

#[test]
fn per_image_batch_rejects_max_images_per_call() {
    let yaml = provider_with_basic_image_execution(
        "      adapter: images_json\n      path: /v1/images/generations\n      batch:\n        mode: per_image\n        max_images_per_call: 1",
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    let error = provider
        .validate_media_descriptors()
        .expect_err("per-image mode must not carry an exact batch limit");

    assert!(
        error
            .to_string()
            .contains("media.image.execution.batch.max_images_per_call"),
        "{error}"
    );
}

#[test]
fn exact_batch_requires_at_least_two_images_per_call() {
    let yaml = provider_with_basic_image_execution(
        "      adapter: images_json\n      path: /v1/images/generations\n      batch:\n        mode: exact\n        max_images_per_call: 1",
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    let error = provider
        .validate_media_descriptors()
        .expect_err("exact mode needs a real batch size");

    assert!(
        error
            .to_string()
            .contains("media.image.execution.batch.max_images_per_call"),
        "{error}"
    );
}

#[test]
fn auto_image_model_is_rejected_by_validation() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: images_json
      path: /v1/images/generations
    models:
      - id: auto
        operations:
          - generate
        variants: { model_id: auto }
"#,
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    let error = provider
        .validate_media_descriptors()
        .expect_err("auto model is invalid");

    assert!(
        error.to_string().contains("media.image.models[0].id"),
        "{error}"
    );
}

#[test]
fn missing_image_execution_parses_and_validates_for_later_availability_skip() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    models:
      - id: gpt-image-1
        operations:
          - generate
        variants: { model_id: gpt-image-1 }
"#,
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    assert!(provider
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .and_then(|image| image.execution.as_ref())
        .is_none());
    provider
        .validate_media_descriptors()
        .expect("missing execution only affects availability");
}

#[test]
fn empty_image_execution_path_is_rejected_by_validation() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: images_json
      path: ""
    models:
      - id: gpt-image-1
        operations:
          - generate
        variants: { model_id: gpt-image-1 }
"#,
    );
    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    let error = provider
        .validate_media_descriptors()
        .expect_err("empty execution path is invalid");

    assert!(error.to_string().contains("execution.path"), "{error}");
}

#[test]
fn old_top_level_image_batch_limit_is_rejected() {
    let yaml = provider_with_basic_image_execution(
        "      adapter: images_json\n      path: /v1/images/generations\n      max_images_per_call: 4",
    );
    let error = serde_yaml::from_str::<ProviderDescriptor>(&yaml)
        .expect_err("old top-level batch limit should be rejected");

    assert!(error.to_string().contains("max_images_per_call"), "{error}");
}

#[test]
fn image_media_descriptor_uses_param_axes() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: images_json
      path: /v1/images/generations
    models:
      - id: gpt-image-1
        display_name: GPT Image 1
        operations:
          - generate
        axes:
          - { id: size, label: Size, role: param, control: !enum { values: ["1024x1024","1536x1024"], default: "1024x1024" }, request_field: size }
          - { id: output_format, label: Output format, role: param, control: !enum { values: ["png","jpeg"], default: "png" }, request_field: output_format }
        variants: { model_id: gpt-image-1 }
"#,
    );

    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    provider
        .validate_media_descriptors()
        .expect("media descriptor validates");
    let image = provider
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .expect("image media");
    assert_eq!(
        image.execution.as_ref().map(|execution| execution.adapter),
        Some(MediaExecutionKind::ImagesJson)
    );
    let axis = &image.models[0].axes[0];
    assert_eq!(axis.id, "size");
    assert!(matches!(&axis.control, ControlKind::Enum { default, .. } if default == "1024x1024"));
    assert_eq!(axis.request_field.as_deref(), Some("size"));
}

#[test]
fn image_model_can_override_provider_execution_adapter() {
    let yaml = provider_with_media_yaml(
        r#"
media:
  image:
    execution:
      adapter: chat_image_output
      path: /chat/completions
    models:
      - id: image-only-model
        operations:
          - generate
        execution:
          adapter: images_json
          path: /images/generations
        variants: { model_id: image-only-model }
"#,
    );

    let provider: ProviderDescriptor = serde_yaml::from_str(&yaml).expect("provider parses");

    provider
        .validate_media_descriptors()
        .expect("media descriptor validates");
    let model = provider
        .media
        .as_ref()
        .and_then(|media| media.image.as_ref())
        .and_then(|image| image.models.first())
        .expect("image model");
    assert_eq!(
        model.execution.as_ref().map(|execution| execution.adapter),
        Some(MediaExecutionKind::ImagesJson)
    );
}

#[test]
fn media_execution_kind_parses_relaydance_video() {
    let kind: MediaExecutionKind = serde_yaml::from_str("relaydance_video").expect("parse");
    assert_eq!(kind, MediaExecutionKind::RelaydanceVideo);
}

#[test]
fn media_execution_kind_parses_byteplus_video() {
    let kind: MediaExecutionKind = serde_yaml::from_str("byteplus_video").expect("parse");
    assert_eq!(kind, MediaExecutionKind::BytePlusVideo);
}

#[test]
fn media_execution_kind_parses_worldrouter_video() {
    let kind: MediaExecutionKind = serde_yaml::from_str("worldrouter_video").expect("parse");
    assert_eq!(kind, MediaExecutionKind::WorldRouterVideo);
}

#[test]
fn media_execution_kind_parses_gemini_generate_content() {
    let kind: MediaExecutionKind = serde_yaml::from_str("gemini_generate_content").expect("parse");
    assert_eq!(kind, MediaExecutionKind::GeminiGenerateContent);
}

#[test]
fn media_execution_kind_rejects_openai_video() {
    let error = serde_yaml::from_str::<MediaExecutionKind>("openai_video").unwrap_err();
    assert!(error.to_string().contains("unknown variant"));
}
