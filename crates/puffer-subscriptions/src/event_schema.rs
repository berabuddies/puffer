//! Event schema metadata for monitor rule builders.

use crate::{FilterSpec, TaggedFilterSpec};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Describes the schema used to build monitor rules for one event source.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct EventSchema {
    /// Schema version. Version 1 is the only supported version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Optional event-source id for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_source: Option<String>,
    /// Payload fields that were used by the subscriber to construct Event.text.
    #[serde(default)]
    pub text_fields: Vec<EventTextField>,
    /// Payload-relative fields that can be used for field filters.
    #[serde(default)]
    pub fields: Vec<EventField>,
    /// Resolved source path. Filled by the loader, not by resource JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<PathBuf>,
}

/// Documents one payload field that contributed to Event.text.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct EventTextField {
    /// Payload-relative path, for example `message.subject`.
    pub path: String,
    /// Human-readable label for UI hints.
    pub label: String,
}

/// Describes one payload field that can be filtered.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct EventField {
    /// Payload-relative path, for example `message.subject`.
    pub path: String,
    /// Human-readable label.
    pub label: String,
    /// Field value type.
    #[serde(rename = "type")]
    pub field_type: EventFieldType,
    /// Operators allowed for this field.
    #[serde(default)]
    pub operators: Vec<EventOperator>,
    /// Optional enum/display values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<EventFieldValue>,
}

/// Supported event field value types.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventFieldType {
    /// UTF-8 string.
    String,
    /// JSON boolean.
    Boolean,
    /// JSON number.
    Number,
    /// String-like enum value.
    Enum,
    /// Presence/nonnull check.
    Exists,
}

/// Supported rule operators.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventOperator {
    /// Exact equality after the existing jq-like evaluator stringifies values.
    Equals,
    /// Literal substring test for string fields.
    Contains,
    /// User-provided regex for string fields.
    Matches,
    /// Presence check.
    Exists,
}

/// One enum/display value for a schema field.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct EventFieldValue {
    /// JSON scalar value to send to the daemon.
    pub value: Value,
    /// Human-readable label.
    pub label: String,
}

/// Field-rule intent accepted by the monitor rule RPC.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct EventFieldRule {
    /// Payload field path or schema field id.
    pub field: String,
    /// Operator requested by the UI.
    pub operator: EventOperator,
    /// Optional JSON scalar value.
    #[serde(default)]
    pub value: Option<Value>,
}

/// Loads `event_schema.json` from a subscriber manifest directory.
pub fn load_event_schema_from_dir(dir: &Path) -> Result<Option<EventSchema>> {
    let path = dir.join("event_schema.json");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut schema: EventSchema =
        serde_json::from_slice(&bytes).with_context(|| format!("invalid {}", path.display()))?;
    validate_event_schema(&schema)?;
    schema.source_path = Some(path);
    Ok(Some(schema))
}

/// Validates one event schema before it is exposed to callers.
pub fn validate_event_schema(schema: &EventSchema) -> Result<()> {
    if schema.version != 1 {
        anyhow::bail!("event schema version must be 1");
    }
    let mut seen = BTreeSet::new();
    for field in &schema.fields {
        validate_schema_path(&field.path)?;
        if !seen.insert(field.path.clone()) {
            anyhow::bail!("duplicate event schema field `{}`", field.path);
        }
        validate_operator_set(field)?;
    }
    for text_field in &schema.text_fields {
        validate_schema_path(&text_field.path)?;
    }
    Ok(())
}

/// Compiles one schema-backed field rule into a Puffer filter.
pub fn compile_event_field_rule(schema: &EventSchema, rule: &EventFieldRule) -> Result<FilterSpec> {
    let field = schema
        .fields
        .iter()
        .find(|field| field.path == rule.field)
        .with_context(|| format!("event field `{}` is not declared", rule.field))?;
    if !field.operators.contains(&rule.operator) {
        anyhow::bail!(
            "operator `{:?}` is not allowed for `{}`",
            rule.operator,
            field.path
        );
    }
    Ok(FilterSpec::Tagged(TaggedFilterSpec::Jq {
        expression: compile_jq_expression(field, rule)?,
    }))
}

fn default_version() -> u32 {
    1
}

fn validate_schema_path(path: &str) -> Result<()> {
    let path = path.trim();
    if path.is_empty() || path.starts_with('.') || path.starts_with('$') {
        anyhow::bail!("event schema path `{path}` must be payload-relative");
    }
    for part in path.split('.') {
        if part.is_empty()
            || !part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            anyhow::bail!("event schema path `{path}` contains an invalid segment");
        }
    }
    Ok(())
}

fn validate_operator_set(field: &EventField) -> Result<()> {
    if field.operators.is_empty() {
        anyhow::bail!("event schema field `{}` must declare operators", field.path);
    }
    for operator in &field.operators {
        if !operator_allowed(field.field_type, *operator) {
            anyhow::bail!(
                "operator `{:?}` is not allowed for {:?} field `{}`",
                operator,
                field.field_type,
                field.path
            );
        }
    }
    if field.field_type == EventFieldType::Enum && field.values.is_empty() {
        anyhow::bail!("enum field `{}` must declare values", field.path);
    }
    Ok(())
}

fn operator_allowed(field_type: EventFieldType, operator: EventOperator) -> bool {
    match field_type {
        EventFieldType::String => matches!(
            operator,
            EventOperator::Contains | EventOperator::Equals | EventOperator::Matches
        ),
        EventFieldType::Boolean | EventFieldType::Number | EventFieldType::Enum => {
            operator == EventOperator::Equals
        }
        EventFieldType::Exists => operator == EventOperator::Exists,
    }
}

fn compile_jq_expression(field: &EventField, rule: &EventFieldRule) -> Result<String> {
    match rule.operator {
        EventOperator::Exists => Ok(format!(".{} | test(\".+\")", field.path)),
        EventOperator::Equals => {
            let value = rule_value_string(rule)?;
            Ok(format!(".{} == {}", field.path, json_string(&value)?))
        }
        EventOperator::Contains => {
            let value = regex::escape(&rule_value_string(rule)?);
            Ok(format!(".{} | test({})", field.path, json_string(&value)?))
        }
        EventOperator::Matches => {
            let pattern = rule_value_string(rule)?;
            regex::Regex::new(&pattern)
                .with_context(|| format!("invalid regex for `{}`", field.path))?;
            Ok(format!(
                ".{} | test({})",
                field.path,
                json_string(&pattern)?
            ))
        }
    }
}

fn rule_value_string(rule: &EventFieldRule) -> Result<String> {
    let value = rule
        .value
        .as_ref()
        .context("event field rule value required")?;
    Ok(match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        other => anyhow::bail!("event field rule value must be a JSON scalar, got {other}"),
    })
}

fn json_string(value: &str) -> Result<String> {
    serde_json::to_string(value).context("failed to encode jq string literal")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TaggedFilterSpec, filter_matches};
    use serde_json::json;

    fn schema() -> EventSchema {
        EventSchema {
            version: 1,
            event_source: Some("gmail-browser".to_string()),
            text_fields: vec![EventTextField {
                path: "message.snippet".to_string(),
                label: "Snippet".to_string(),
            }],
            fields: vec![
                EventField {
                    path: "message.subject".to_string(),
                    label: "Subject".to_string(),
                    field_type: EventFieldType::String,
                    operators: vec![
                        EventOperator::Contains,
                        EventOperator::Equals,
                        EventOperator::Matches,
                    ],
                    values: Vec::new(),
                },
                EventField {
                    path: "message.has_attachment".to_string(),
                    label: "Has attachment".to_string(),
                    field_type: EventFieldType::Boolean,
                    operators: vec![EventOperator::Equals],
                    values: vec![
                        EventFieldValue {
                            value: json!(true),
                            label: "Yes".to_string(),
                        },
                        EventFieldValue {
                            value: json!(false),
                            label: "No".to_string(),
                        },
                    ],
                },
                EventField {
                    path: "media".to_string(),
                    label: "Has media".to_string(),
                    field_type: EventFieldType::Exists,
                    operators: vec![EventOperator::Exists],
                    values: Vec::new(),
                },
            ],
            source_path: None,
        }
    }

    #[test]
    fn validates_payload_relative_nested_paths_and_unique_fields() {
        assert!(validate_event_schema(&schema()).is_ok());

        let mut duplicate = schema();
        duplicate.fields.push(duplicate.fields[0].clone());
        assert!(
            validate_event_schema(&duplicate)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
    }

    #[test]
    fn rejects_unsafe_paths_versions_and_operator_sets() {
        let mut bad_version = schema();
        bad_version.version = 2;
        assert!(
            validate_event_schema(&bad_version)
                .unwrap_err()
                .to_string()
                .contains("version")
        );

        let mut bad_path = schema();
        bad_path.fields[0].path = ".payload.subject".to_string();
        assert!(
            validate_event_schema(&bad_path)
                .unwrap_err()
                .to_string()
                .contains("path")
        );

        let mut bad_operator = schema();
        bad_operator.fields[2].operators = vec![EventOperator::Contains];
        assert!(
            validate_event_schema(&bad_operator)
                .unwrap_err()
                .to_string()
                .contains("operator")
        );
    }

    #[test]
    fn compiles_schema_fields_to_payload_rooted_jq_filters() {
        let filter = compile_event_field_rule(
            &schema(),
            &EventFieldRule {
                field: "message.subject".to_string(),
                operator: EventOperator::Contains,
                value: Some(json!("invoice.")),
            },
        )
        .unwrap();

        assert!(matches!(
            &filter,
            FilterSpec::Tagged(TaggedFilterSpec::Jq { expression })
                if expression == ".message.subject | test(\"invoice\\\\.\")"
        ));
        assert!(filter_matches(
            Some(&filter),
            "",
            &json!({"message": {"subject": "June invoice."}})
        ));
        assert!(!filter_matches(
            Some(&filter),
            "",
            &json!({"payload": {"message": {"subject": "June invoice."}}})
        ));

        let exists = compile_event_field_rule(
            &schema(),
            &EventFieldRule {
                field: "media".to_string(),
                operator: EventOperator::Exists,
                value: None,
            },
        )
        .unwrap();
        assert!(filter_matches(
            Some(&exists),
            "",
            &json!({"media": "photo"})
        ));
        assert!(!filter_matches(Some(&exists), "", &json!({})));
    }

    #[test]
    fn bundled_subscriber_event_schemas_are_valid() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root");
        for slug in ["telegram-user", "gmail-browser", "email", "gcal-browser"] {
            let schema =
                load_event_schema_from_dir(&root.join("resources").join("subscribers").join(slug))
                    .unwrap()
                    .unwrap_or_else(|| panic!("missing bundled event schema for {slug}"));
            assert_eq!(schema.version, 1);
            assert!(
                !schema.fields.is_empty(),
                "bundled event schema for {slug} has no fields"
            );
        }
    }
}
