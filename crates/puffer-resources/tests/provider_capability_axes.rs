use puffer_provider_registry::{AxisRole, ControlKind, Variants, WireType};
use std::collections::BTreeSet;

fn provider(file: &str) -> puffer_provider_registry::ProviderDescriptor {
    // `cargo test` sets CWD to the package root; the provider YAMLs live at the
    // repo root, so resolve relative to CARGO_MANIFEST_DIR (crates/puffer-resources).
    let path = format!(
        "{}/../../resources/providers/{file}.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    let yaml = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {file}"));
    serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("parse {file}: {e}"))
}

#[test]
fn standalone_kling_provider_yaml_is_removed() {
    let path = format!(
        "{}/../../resources/providers/kling.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    assert!(
        !std::path::Path::new(&path).exists(),
        "standalone kling provider must not be bundled"
    );
}

#[test]
fn byteplus_seedance_declares_param_axes_and_single_variant() {
    let p = provider("byteplus");
    let video = p.media.as_ref().unwrap().video.as_ref().unwrap();
    let m = video
        .models
        .iter()
        .find(|m| m.id == "dreamina-seedance-2-0")
        .expect("model");
    let res = m
        .axes
        .iter()
        .find(|a| a.id == "resolution")
        .expect("resolution");
    assert_eq!(res.role, AxisRole::Param);
    assert!(
        matches!(&res.control, ControlKind::Enum { values, .. } if values.contains(&"1080p".to_string()))
    );
    let duration = m
        .axes
        .iter()
        .find(|a| a.id == "duration")
        .expect("duration");
    assert_eq!(duration.label, "Duration");
    let ratio = m.axes.iter().find(|a| a.id == "ratio").expect("ratio");
    assert_eq!(ratio.label, "Ratio");
    assert_eq!(ratio.request_field, None);
    assert!(
        matches!(&ratio.control, ControlKind::Enum { values, default } if default == "Auto" && values.contains(&"Auto".to_string()) && !values.contains(&"adaptive".to_string()))
    );
    let ratio_map = m
        .media_map
        .as_ref()
        .and_then(|media_map| media_map.ratio.as_ref())
        .expect("ratio map");
    assert_eq!(ratio_map.field, "ratio");
    assert_eq!(
        ratio_map
            .values
            .get("Auto")
            .and_then(|value| value.as_deref()),
        Some("adaptive")
    );
    assert!(matches!(m.variants, Variants::Single(_)));
}

#[test]
fn relaydance_folds_resolution_and_audio_into_logical_models() {
    let p = provider("relaydance");
    let video = p.media.as_ref().unwrap().video.as_ref().unwrap();

    let doubao = video
        .models
        .iter()
        .find(|m| m.id == "doubao-seedance-2-0")
        .expect("doubao");
    let res = doubao
        .axes
        .iter()
        .find(|a| a.id == "resolution")
        .expect("resolution");
    assert_eq!(res.role, AxisRole::Selector);
    let duration = doubao
        .axes
        .iter()
        .find(|a| a.id == "duration")
        .expect("duration");
    assert_eq!(duration.label, "Duration");
    let ratio = doubao.axes.iter().find(|a| a.id == "ratio").expect("ratio");
    assert_eq!(ratio.label, "Ratio");
    assert_eq!(ratio.request_field, None);
    assert!(
        matches!(&ratio.control, ControlKind::Enum { values, .. } if values.contains(&"Auto".to_string()) && !values.contains(&"adaptive".to_string()))
    );
    let ratio_map = doubao
        .media_map
        .as_ref()
        .and_then(|media_map| media_map.ratio.as_ref())
        .expect("ratio map");
    assert_eq!(ratio_map.field, "metadata.ratio");
    assert_eq!(
        ratio_map
            .values
            .get("Auto")
            .and_then(|value| value.as_deref()),
        Some("adaptive")
    );
    match &doubao.variants {
        Variants::BySelector { selector, map } => {
            assert_eq!(selector, "resolution");
            assert_eq!(map["720p"].model_id, "doubao-seedance-2-0-720p");
            assert_eq!(map["1080p"].model_id, "doubao-seedance-2-0-1080p");
        }
        _ => panic!("expected BySelector"),
    }

    let pro = video
        .models
        .iter()
        .find(|m| m.id == "seedance-1-5-pro")
        .expect("pro");
    let audio = pro.axes.iter().find(|a| a.id == "audio").expect("audio");
    assert!(matches!(audio.control, ControlKind::Bool { default: true }));
    match &pro.variants {
        Variants::BySelector { selector, map } => {
            assert_eq!(selector, "audio");
            assert_eq!(map["true"].model_id, "seedance-1-5-pro-with-audio");
            assert_eq!(map["false"].model_id, "seedance-1-5-pro-no-audio");
        }
        _ => panic!("expected BySelector"),
    }
}

#[test]
fn all_providers_parse_after_axis_migration() {
    for file in [
        "openai",
        "zhipu",
        "xai",
        "minimax",
        "minimax-cn",
        "vercel-ai-gateway",
        "worldrouter",
        "openrouter",
        "byteplus",
        "relaydance",
    ] {
        let _ = provider(file); // panics on parse failure
    }
}

#[test]
fn worldrouter_declares_current_video_models() {
    let p = provider("worldrouter");
    let video = p.media.as_ref().unwrap().video.as_ref().unwrap();
    assert_eq!(
        video.execution.as_ref().map(|execution| execution.adapter),
        Some(puffer_provider_registry::MediaExecutionKind::WorldRouterVideo)
    );
    assert_eq!(
        video
            .execution
            .as_ref()
            .map(|execution| execution.prompt_format),
        Some(puffer_provider_registry::VideoPromptFormat::ContentArray)
    );

    let ids = video
        .models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids, BTreeSet::from(["seedance-2.0", "seedance-2.0-fast"]));

    let seedance = video
        .models
        .iter()
        .find(|model| model.id == "seedance-2.0")
        .expect("seedance-2.0");
    assert!(
        seedance
            .axes
            .iter()
            .all(|axis| axis.id != "ratio" && axis.request_field.as_deref() != Some("ratio")),
        "WorldRouter Seedance docs do not expose a ratio request parameter"
    );
    let resolution = seedance
        .axes
        .iter()
        .find(|axis| axis.id == "resolution")
        .expect("resolution");
    assert_eq!(resolution.role, AxisRole::Selector);
    assert!(
        matches!(&resolution.control, ControlKind::Enum { values, default } if default == "1080p" && values == &vec!["720p".to_string(), "1080p".to_string()])
    );
    match &seedance.variants {
        Variants::BySelector { selector, map } => {
            assert_eq!(selector, "resolution");
            assert_eq!(map["720p"].base_params["resolution"], "720p");
            assert_eq!(map["1080p"].base_params["resolution"], "1080p");
        }
        _ => panic!("expected BySelector"),
    }

    let duration = seedance
        .axes
        .iter()
        .find(|axis| axis.id == "duration")
        .expect("duration");
    assert_eq!(duration.request_field.as_deref(), Some("duration"));
    assert_eq!(duration.wire_type, WireType::Number);
    assert!(
        matches!(duration.control, ControlKind::Range { min, max, step, default } if min == 5.0 && max == 10.0 && step == 1.0 && default == 5.0)
    );

    let fast = video
        .models
        .iter()
        .find(|model| model.id == "seedance-2.0-fast")
        .expect("seedance-2.0-fast");
    let mode = fast
        .axes
        .iter()
        .find(|axis| axis.id == "resolution")
        .expect("resolution");
    assert_eq!(mode.label, "Mode");
    assert_eq!(mode.role, AxisRole::Param);
    assert_eq!(mode.request_field.as_deref(), Some("resolution"));
    assert!(
        matches!(&mode.control, ControlKind::Enum { values, default } if default == "720p" && values == &vec!["480p".to_string(), "720p".to_string()])
    );
}
