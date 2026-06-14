use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Canonical product-level ratio labels accepted by media descriptors.
pub const CANONICAL_MEDIA_RATIOS: &[&str] = &[
    "Auto", "9:16", "2:3", "3:4", "1:1", "4:3", "3:2", "16:9", "21:9",
];

/// One user-facing control kind. Exactly three; no generic engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    Enum {
        values: Vec<String>,
        default: String,
    },
    Range {
        min: f64,
        max: f64,
        step: f64,
        default: f64,
    },
    Bool {
        default: bool,
    },
}

/// Whether an axis maps to a request parameter or selects an upstream model id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisRole {
    Param,
    Selector,
}

impl AxisRole {
    /// Returns the wire string (matches the `snake_case` serde representation).
    pub fn as_str(self) -> &'static str {
        match self {
            AxisRole::Param => "param",
            AxisRole::Selector => "selector",
        }
    }
}

/// How a Param axis value is encoded into provider JSON. No Bool: no param axis
/// is boolean today (audio is a selector, carrying no wire value).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireType {
    #[default]
    String,
    Number,
}

impl WireType {
    /// Returns the wire string (matches the `snake_case` serde representation).
    pub fn as_str(self) -> &'static str {
        match self {
            WireType::String => "string",
            WireType::Number => "number",
        }
    }
}

/// One user-facing dimension of a logical model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Axis {
    pub id: String,
    pub label: String,
    pub role: AxisRole,
    pub control: ControlKind,
    #[serde(default)]
    pub request_field: Option<String>,
    #[serde(default)]
    pub wire_type: WireType,
}

/// Static provider mapping for reserved media axes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaMap {
    #[serde(default)]
    pub ratio: Option<MediaRatioMap>,
    #[serde(default)]
    pub size: Option<MediaSizeMap>,
}

/// One-dimensional ratio-to-provider-field mapping.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaRatioMap {
    pub field: String,
    #[serde(default)]
    pub values: BTreeMap<String, Option<String>>,
}

/// Two-dimensional mode-and-ratio-to-provider-size mapping.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaSizeMap {
    pub field: String,
    #[serde(default)]
    pub values: BTreeMap<String, BTreeMap<String, Option<String>>>,
}

impl MediaSizeMap {
    /// Returns ratio keys supported by every declared mode in this size map.
    pub fn common_ratios(&self) -> BTreeSet<String> {
        let mut modes = self.values.values();
        let Some(first) = modes.next() else {
            return BTreeSet::new();
        };
        let mut common = first.keys().cloned().collect::<BTreeSet<_>>();
        for ratios in modes {
            common.retain(|ratio| ratios.contains_key(ratio));
        }
        common
    }
}

/// One concrete upstream model variant + the params it implies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub model_id: String,
    #[serde(default)]
    pub base_params: BTreeMap<String, String>,
}

/// At most ONE selector axis per logical model. `Single` = no selector; the
/// untagged enum reads a `{ model_id, .. }` object as `Single` and a
/// `{ selector, map }` object as `BySelector`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Variants {
    Single(Variant),
    BySelector {
        selector: String,
        map: BTreeMap<String, Variant>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Wrap {
        axes: Vec<Axis>,
        variants: Variants,
    }

    #[test]
    fn deserializes_by_selector_logical_model() {
        let yaml = r#"
axes:
  - { id: audio, label: Native audio, role: selector, control: !bool { default: true } }
  - { id: duration, label: Length, role: param, control: !range { min: 4.0, max: 12.0, step: 1.0, default: 5.0 }, request_field: seconds, wire_type: number }
variants:
  selector: audio
  map:
    "true": { model_id: pro-with-audio }
    "false": { model_id: pro-no-audio }
"#;
        let w: Wrap = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(w.axes[0].role, AxisRole::Selector);
        match w.variants {
            Variants::BySelector { selector, map } => {
                assert_eq!(selector, "audio");
                assert_eq!(map["false"].model_id, "pro-no-audio");
            }
            _ => panic!("expected BySelector"),
        }
    }

    #[test]
    fn deserializes_single_variant() {
        let yaml = r#"
axes: []
variants: { model_id: dreamina-seedance-2-0-260128 }
"#;
        let w: Wrap = serde_yaml::from_str(yaml).expect("parse");
        assert!(
            matches!(w.variants, Variants::Single(v) if v.model_id == "dreamina-seedance-2-0-260128")
        );
    }
}
