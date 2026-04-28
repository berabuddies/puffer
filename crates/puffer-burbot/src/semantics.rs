use crate::contract::{
    ActionContract, IntentExtractorSpec, RiskLevel, SemanticIntentSpec, SideEffectClass,
};
use crate::graph::ActionRef;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct NormalizedIntent {
    pub(crate) intent: String,
    #[serde(default)]
    pub(crate) slots: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IntentSource {
    Direct,
    Extracted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionIntentMatch {
    pub(crate) normalized: NormalizedIntent,
    pub(crate) side_effect_class: SideEffectClass,
    pub(crate) source: IntentSource,
}

/// Returns direct and extracted semantic intents for an action payload.
pub(crate) fn action_intents(action: &ActionContract, payload: &Value) -> Vec<ActionIntentMatch> {
    let mut intents = Vec::new();
    for spec in &action.semantic_intents {
        if let Some(intent) = direct_intent(action, spec, payload) {
            intents.push(intent);
        }
    }
    for spec in &action.intent_extractors {
        if let Some(intent) = extracted_intent(spec, payload) {
            intents.push(intent);
        }
    }
    intents
}

/// Builds an action payload from a normalized intent when the contract declares a mapping.
pub(crate) fn payload_for_intent(
    action: &ActionContract,
    intent: &NormalizedIntent,
) -> Option<Value> {
    for spec in &action.semantic_intents {
        if normalize_token(&spec.intent) != intent.intent {
            continue;
        }
        let mut object = serde_json::Map::new();
        for (arg, value) in &spec.defaults {
            object.insert(arg.clone(), value.clone());
        }
        for (slot, arg) in &spec.slots {
            let value = intent.slots.get(&normalize_token(slot))?;
            object.insert(arg.clone(), json!(value));
        }
        for (slot, arg) in &spec.optional_slots {
            if let Some(value) = intent.slots.get(&normalize_token(slot)) {
                object.insert(arg.clone(), json!(value));
            }
        }
        return Some(Value::Object(object));
    }
    None
}

/// Returns semantic intents whose declared side effects are read-only.
pub(crate) fn safe_observation_intents(
    action: &ActionContract,
    payload: &Value,
) -> Vec<ActionIntentMatch> {
    action_intents(action, payload)
        .into_iter()
        .filter(|intent| read_only_side_effect(&intent.side_effect_class))
        .collect()
}

/// Returns true when a contract or payload-proven intent is a read-only observation.
pub(crate) fn safe_read_observation(action: &ActionContract, payload: &Value) -> bool {
    read_only_side_effect(&action.side_effect_class)
        || action_intents(action, payload)
            .iter()
            .any(|intent| read_only_side_effect(&intent.side_effect_class))
}

/// Returns true for side-effect classes that do not mutate state.
pub(crate) fn read_only_side_effect(side_effect_class: &SideEffectClass) -> bool {
    matches!(
        side_effect_class,
        SideEffectClass::PureObservation
            | SideEffectClass::LocalRead
            | SideEffectClass::ExternalRead
    )
}

/// Produces a stable preference tuple for choosing among equivalent intents.
pub(crate) fn intent_preference(
    _action_ref: &ActionRef,
    action: &ActionContract,
    intent: &ActionIntentMatch,
    node_id: crate::ids::NodeId,
) -> (u8, u8, u64) {
    let source_rank = match intent.source {
        IntentSource::Direct => 0,
        IntentSource::Extracted => 1,
    };
    (source_rank, risk_rank(&action.risk_level), node_id.0)
}

/// Converts arbitrary semantic text into a stable egg-compatible symbol.
pub(crate) fn semantic_symbol(value: &str) -> String {
    let mut symbol = String::new();
    let mut last_was_separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            symbol.push(character.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            symbol.push('_');
            last_was_separator = true;
        }
    }
    let trimmed = symbol.trim_matches('_');
    if trimmed.is_empty() {
        "intent".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Splits a simple shell command and rejects syntax with control or redirection effects.
pub(crate) fn split_simple_command(command: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in command.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            character if character.is_ascii_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '|' | '&' | ';' | '>' | '<' | '`' | '$' => return None,
            character => current.push(character),
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Some(tokens)
}

fn direct_intent(
    action: &ActionContract,
    spec: &SemanticIntentSpec,
    payload: &Value,
) -> Option<ActionIntentMatch> {
    let mut slots = BTreeMap::new();
    for (slot, arg) in &spec.slots {
        let slot_name = normalize_token(slot);
        let value = payload.get(arg)?.as_str()?;
        slots.insert(slot_name.clone(), normalize_slot_value(&slot_name, value));
    }
    for (slot, arg) in &spec.optional_slots {
        if let Some(value) = payload.get(arg).and_then(Value::as_str) {
            let slot_name = normalize_token(slot);
            slots.insert(slot_name.clone(), normalize_slot_value(&slot_name, value));
        }
    }
    Some(ActionIntentMatch {
        normalized: NormalizedIntent {
            intent: normalize_token(&spec.intent),
            slots,
        },
        side_effect_class: spec
            .side_effect_class
            .clone()
            .unwrap_or_else(|| action.side_effect_class.clone()),
        source: IntentSource::Direct,
    })
}

fn extracted_intent(spec: &IntentExtractorSpec, payload: &Value) -> Option<ActionIntentMatch> {
    match normalize_token(&spec.parser).as_str() {
        "regex" => extracted_regex_intent(spec, payload),
        "simple_shell" => extracted_token_intent(spec, payload),
        _ => None,
    }
}

fn extracted_regex_intent(
    spec: &IntentExtractorSpec,
    payload: &Value,
) -> Option<ActionIntentMatch> {
    let text = payload.get(&spec.source_arg)?.as_str()?;
    let pattern = spec.pattern.as_ref()?;
    let regex = regex::Regex::new(pattern).ok()?;
    let captures = regex.captures(text)?;
    let mut slots = BTreeMap::new();
    for (slot, group) in &spec.slot_groups {
        let value = captures.name(group)?.as_str();
        let slot_name = normalize_token(slot);
        slots.insert(slot_name.clone(), normalize_slot_value(&slot_name, value));
    }
    for (slot, group) in &spec.optional_slot_groups {
        if let Some(value) = captures.name(group) {
            let slot_name = normalize_token(slot);
            slots.insert(
                slot_name.clone(),
                normalize_slot_value(&slot_name, value.as_str()),
            );
        }
    }
    Some(extracted_match(spec, slots))
}

fn extracted_token_intent(
    spec: &IntentExtractorSpec,
    payload: &Value,
) -> Option<ActionIntentMatch> {
    let command = payload.get(&spec.source_arg)?.as_str()?.trim();
    let tokens = split_simple_command(command)?;
    let (command_name, args) = tokens.split_first()?;
    if command_name != &spec.command {
        return None;
    }
    for literal in &spec.literals {
        if args.get(literal.position).map(String::as_str) != Some(literal.value.as_str()) {
            return None;
        }
    }
    let mut slots = BTreeMap::new();
    for slot in &spec.slots {
        let slot_name = normalize_token(&slot.name);
        let value = args.get(slot.position)?;
        slots.insert(slot_name.clone(), normalize_slot_value(&slot_name, value));
    }
    for slot in &spec.optional_slots {
        if let Some(value) = args.get(slot.position) {
            let slot_name = normalize_token(&slot.name);
            slots.insert(slot_name.clone(), normalize_slot_value(&slot_name, value));
        }
    }
    Some(extracted_match(spec, slots))
}

fn extracted_match(
    spec: &IntentExtractorSpec,
    slots: BTreeMap<String, String>,
) -> ActionIntentMatch {
    ActionIntentMatch {
        normalized: NormalizedIntent {
            intent: normalize_token(&spec.intent),
            slots,
        },
        side_effect_class: spec
            .side_effect_class
            .clone()
            .unwrap_or(SideEffectClass::PureObservation),
        source: IntentSource::Extracted,
    }
}

fn risk_rank(risk: &RiskLevel) -> u8 {
    match risk {
        RiskLevel::Low => 0,
        RiskLevel::Medium => 1,
        RiskLevel::High => 2,
        RiskLevel::Critical => 3,
    }
}

fn normalize_token(value: &str) -> String {
    semantic_symbol(value)
}

fn normalize_slot_value(slot: &str, value: &str) -> String {
    if path_like_slot(slot) {
        normalize_path(value)
    } else {
        value.trim().to_string()
    }
}

fn path_like_slot(slot: &str) -> bool {
    slot == "path"
        || slot.ends_with("_path")
        || slot == "dir"
        || slot.ends_with("_dir")
        || slot.contains("directory")
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    let without_trailing_slashes = trim_trailing_path_separators(trimmed);
    normalize_read_path(without_trailing_slashes)
}

fn trim_trailing_path_separators(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() && path.starts_with('/') {
        "/"
    } else {
        trimmed
    }
}

fn normalize_read_path(path: &str) -> String {
    if path.is_empty() || path.contains("..") {
        return path.to_string();
    }

    let absolute = path.starts_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            other => parts.push(other),
        }
    }

    let joined = parts.join("/");
    match (absolute, joined.is_empty()) {
        (true, true) => "/".to_string(),
        (true, false) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ApprovalSpec, Idempotency, Reversibility, VerificationSpec};

    fn action() -> ActionContract {
        ActionContract {
            name: "Read".to_string(),
            description: "read".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
            side_effect_class: SideEffectClass::LocalRead,
            reversibility: Reversibility::Reversible,
            idempotency: Idempotency::Idempotent,
            risk_level: RiskLevel::Low,
            preconditions: Vec::new(),
            postconditions: Vec::new(),
            verification: VerificationSpec {
                methods: Vec::new(),
                templates: Vec::new(),
                required_before_completion: false,
                confidence: 0.5,
            },
            approval: ApprovalSpec {
                required: false,
                reason: None,
            },
            failure_modes: Vec::new(),
            forbidden_uses: Vec::new(),
            argument_safety: Vec::new(),
            semantic_intents: vec![SemanticIntentSpec {
                intent: "read_file".to_string(),
                slots: [("path".to_string(), "file_path".to_string())].into(),
                optional_slots: BTreeMap::new(),
                defaults: BTreeMap::new(),
                side_effect_class: None,
            }],
            intent_extractors: Vec::new(),
            repair_rules: Vec::new(),
            cost_estimate: None,
            latency_estimate: None,
        }
    }

    #[test]
    fn direct_intent_maps_payload_slots() {
        let intents = action_intents(&action(), &json!({"file_path": "src/lib.rs"}));

        assert_eq!(intents[0].normalized.intent, "read_file");
        assert_eq!(intents[0].normalized.slots["path"], "src/lib.rs");
    }

    #[test]
    fn path_slots_get_conservative_read_normalization() {
        let intents = action_intents(&action(), &json!({"file_path": "./src//lib.rs/"}));

        assert_eq!(intents[0].normalized.slots["path"], "src/lib.rs");
    }

    #[test]
    fn path_normalization_preserves_parent_segments() {
        let intents = action_intents(&action(), &json!({"file_path": "src/../lib.rs"}));

        assert_eq!(intents[0].normalized.slots["path"], "src/../lib.rs");
    }
}
