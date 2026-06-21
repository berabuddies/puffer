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

/// Loads `event_schema.json` from a schema metadata directory.
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
        EventOperator::Exists => Ok(format!(".{} | exists", field.path)),
        EventOperator::Equals => {
            let value = rule_value_scalar(rule)?;
            Ok(format!(".{} == {}", field.path, json_literal(value)?))
        }
        EventOperator::Contains => {
            let value = format!("(?i:{})", regex::escape(&rule_value_string(rule)?));
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

fn rule_value_scalar(rule: &EventFieldRule) -> Result<&Value> {
    let value = rule
        .value
        .as_ref()
        .context("event field rule value required")?;
    match value {
        Value::String(_) | Value::Bool(_) | Value::Number(_) => Ok(value),
        other => anyhow::bail!("event field rule value must be a JSON scalar, got {other}"),
    }
}

fn json_string(value: &str) -> Result<String> {
    serde_json::to_string(value).context("failed to encode jq string literal")
}

fn json_literal(value: &Value) -> Result<String> {
    serde_json::to_string(value).context("failed to encode jq value literal")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{filter_matches, TaggedFilterSpec};
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
        assert!(validate_event_schema(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
    }

    #[test]
    fn rejects_unsafe_paths_versions_and_operator_sets() {
        let mut bad_version = schema();
        bad_version.version = 2;
        assert!(validate_event_schema(&bad_version)
            .unwrap_err()
            .to_string()
            .contains("version"));

        let mut bad_path = schema();
        bad_path.fields[0].path = ".payload.subject".to_string();
        assert!(validate_event_schema(&bad_path)
            .unwrap_err()
            .to_string()
            .contains("path"));

        let mut bad_operator = schema();
        bad_operator.fields[2].operators = vec![EventOperator::Contains];
        assert!(validate_event_schema(&bad_operator)
            .unwrap_err()
            .to_string()
            .contains("operator"));
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
                if expression == ".message.subject | test(\"(?i:invoice\\\\.)\")"
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

    #[test]
    fn bundled_command_connector_event_schemas_are_valid() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root");
        for slug in ["telegram-bot", "lark-login", "lark-bot"] {
            let schema =
                load_event_schema_from_dir(&root.join("resources").join("connectors").join(slug))
                    .unwrap()
                    .unwrap_or_else(|| panic!("missing bundled event schema for {slug}"));
            assert_eq!(schema.version, 1);
            assert!(
                !schema.fields.is_empty(),
                "bundled event schema for {slug} has no fields"
            );
        }
    }

    fn bundled_schema(slug: &str) -> EventSchema {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root");
        load_event_schema_from_dir(&root.join("resources").join("subscribers").join(slug))
            .unwrap()
            .unwrap_or_else(|| panic!("missing bundled event schema for {slug}"))
    }

    fn bundled_connector_schema(slug: &str) -> EventSchema {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root");
        load_event_schema_from_dir(&root.join("resources").join("connectors").join(slug))
            .unwrap()
            .unwrap_or_else(|| panic!("missing bundled event schema for {slug}"))
    }

    fn all_bundled_monitor_schemas() -> Vec<(&'static str, EventSchema)> {
        vec![
            ("telegram-user", bundled_schema("telegram-user")),
            ("gmail-browser", bundled_schema("gmail-browser")),
            ("gcal-browser", bundled_schema("gcal-browser")),
            ("email", bundled_schema("email")),
            ("telegram-bot", bundled_connector_schema("telegram-bot")),
            ("lark-login", bundled_connector_schema("lark-login")),
            ("lark-bot", bundled_connector_schema("lark-bot")),
        ]
    }

    fn field_filter(
        schema: &EventSchema,
        field: &str,
        operator: EventOperator,
        value: Option<Value>,
    ) -> FilterSpec {
        compile_event_field_rule(
            schema,
            &EventFieldRule {
                field: field.to_string(),
                operator,
                value,
            },
        )
        .unwrap()
    }

    fn payload_with_path_for_test(path: &str, value: Value) -> Value {
        let mut current = value;
        for part in path.split('.').rev() {
            let mut map = serde_json::Map::new();
            map.insert(part.to_string(), current);
            current = Value::Object(map);
        }
        current
    }

    #[test]
    fn bundled_contains_field_rules_match_case_insensitively() {
        for (schema_slug, schema) in all_bundled_monitor_schemas() {
            let mut checked = schema.text_fields.len();
            for field in schema
                .fields
                .iter()
                .filter(|field| field.operators.contains(&EventOperator::Contains))
            {
                checked += 1;
                let filter = field_filter(
                    &schema,
                    &field.path,
                    EventOperator::Contains,
                    Some(json!("MiXeD")),
                );
                assert!(
                    filter_matches(
                        Some(&filter),
                        "",
                        &payload_with_path_for_test(&field.path, json!("prefix mixed suffix")),
                    ),
                    "{schema_slug} {} contains rule should ignore case",
                    field.path
                );
                assert!(
                    !filter_matches(
                        Some(&filter),
                        "",
                        &payload_with_path_for_test(&field.path, json!("prefix plain suffix")),
                    ),
                    "{schema_slug} {} contains rule should still require the literal value",
                    field.path
                );
            }
            assert!(checked > 0, "{schema_slug} should expose contains fields");
        }
    }

    #[test]
    fn bundled_connector_field_matrix_matches_representative_payloads() {
        let telegram = bundled_schema("telegram-user");
        let gmail = bundled_schema("gmail-browser");
        let email = bundled_schema("email");
        let gcal = bundled_schema("gcal-browser");

        let chat_kind = field_filter(
            &telegram,
            "chat_kind",
            EventOperator::Equals,
            Some(json!("group")),
        );
        assert!(filter_matches(
            Some(&chat_kind),
            "",
            &json!({"chat_kind": "group"})
        ));
        assert!(!filter_matches(
            Some(&chat_kind),
            "",
            &json!({"chat_kind": "user"})
        ));

        let sender_name = field_filter(
            &telegram,
            "sender_name",
            EventOperator::Contains,
            Some(json!("John")),
        );
        assert!(filter_matches(
            Some(&sender_name),
            "",
            &json!({"sender_name": "smith john"})
        ));

        let group_channel_name = field_filter(
            &telegram,
            "group_channel_name",
            EventOperator::Contains,
            Some(json!("Puffer")),
        );
        assert!(filter_matches(
            Some(&group_channel_name),
            "",
            &json!({"chat_kind": "group", "group_channel_name": "Puffer Internal"})
        ));
        assert!(!filter_matches(
            Some(&group_channel_name),
            "",
            &json!({"chat_kind": "user", "chat_title": "Puffer Friend"})
        ));

        let has_media = field_filter(&telegram, "media", EventOperator::Exists, None);
        assert!(filter_matches(
            Some(&has_media),
            "",
            &json!({"media": {"kind": "photo"}})
        ));
        for empty_payload in [
            json!({}),
            json!({"media": null}),
            json!({"media": ""}),
            json!({"media": []}),
        ] {
            assert!(
                !filter_matches(Some(&has_media), "", &empty_payload),
                "empty media payload should not satisfy exists: {empty_payload}"
            );
        }

        let gmail_subject = field_filter(
            &gmail,
            "message.subject",
            EventOperator::Contains,
            Some(json!("invoice")),
        );
        assert!(filter_matches(
            Some(&gmail_subject),
            "",
            &json!({"message": {"subject": "invoice due"}})
        ));

        let gmail_unread = field_filter(
            &gmail,
            "message.unread",
            EventOperator::Equals,
            Some(json!(true)),
        );
        assert!(filter_matches(
            Some(&gmail_unread),
            "",
            &json!({"message": {"unread": true}})
        ));
        assert!(!filter_matches(
            Some(&gmail_unread),
            "",
            &json!({"message": {"unread": "true"}})
        ));

        let gmail_has_attachment = field_filter(
            &gmail,
            "message.hasAttachment",
            EventOperator::Equals,
            Some(json!(true)),
        );
        assert!(filter_matches(
            Some(&gmail_has_attachment),
            "",
            &json!({"message": {"hasAttachment": true}})
        ));
        assert!(!filter_matches(
            Some(&gmail_has_attachment),
            "",
            &json!({"message": {"hasAttachment": false}})
        ));

        let email_has_attachment = field_filter(
            &email,
            "has_attachment",
            EventOperator::Equals,
            Some(json!(true)),
        );
        assert!(filter_matches(
            Some(&email_has_attachment),
            "",
            &json!({"has_attachment": true})
        ));
        assert!(!filter_matches(
            Some(&email_has_attachment),
            "",
            &json!({"has_attachment": "true"})
        ));

        let event_title = field_filter(
            &gcal,
            "event.title",
            EventOperator::Contains,
            Some(json!("invoice")),
        );
        assert!(filter_matches(
            Some(&event_title),
            "",
            &json!({"event": {"title": "invoice review"}})
        ));
    }

    #[test]
    fn field_rules_reject_unknown_fields_and_escape_contains_literals() {
        let telegram = bundled_schema("telegram-user");
        let gmail = bundled_schema("gmail-browser");

        let error = compile_event_field_rule(
            &telegram,
            &EventFieldRule {
                field: "message.subject".to_string(),
                operator: EventOperator::Contains,
                value: Some(json!("invoice")),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("not declared"));

        let literal = field_filter(
            &gmail,
            "message.subject",
            EventOperator::Contains,
            Some(json!("a.+?()")),
        );
        assert!(filter_matches(
            Some(&literal),
            "",
            &json!({"message": {"subject": "literal a.+?() value"}})
        ));
        assert!(!filter_matches(
            Some(&literal),
            "",
            &json!({"message": {"subject": "literal axxx value"}})
        ));

        let regex = field_filter(
            &gmail,
            "message.subject",
            EventOperator::Matches,
            Some(json!("invoice|receipt")),
        );
        assert!(filter_matches(
            Some(&regex),
            "",
            &json!({"message": {"subject": "receipt attached"}})
        ));
        assert!(!filter_matches(
            Some(&regex),
            "",
            &json!({"message": {"subject": "status update"}})
        ));
    }

    #[test]
    fn bundled_command_connector_field_matrix_matches_representative_payloads() {
        let telegram_bot = bundled_connector_schema("telegram-bot");
        let lark_login = bundled_connector_schema("lark-login");
        let lark_bot = bundled_connector_schema("lark-bot");

        assert_eq!(
            telegram_bot
                .fields
                .iter()
                .map(|field| field.path.as_str())
                .collect::<Vec<_>>(),
            vec!["is_group", "bot_mentioned"]
        );

        let group_chat = field_filter(
            &telegram_bot,
            "is_group",
            EventOperator::Equals,
            Some(json!(true)),
        );
        assert!(filter_matches(
            Some(&group_chat),
            "",
            &json!({"is_group": true})
        ));
        assert!(!filter_matches(
            Some(&group_chat),
            "",
            &json!({"is_group": "true"})
        ));

        let bot_not_mentioned = field_filter(
            &telegram_bot,
            "bot_mentioned",
            EventOperator::Equals,
            Some(json!(false)),
        );
        assert!(filter_matches(
            Some(&bot_not_mentioned),
            "",
            &json!({"bot_mentioned": false})
        ));

        let lark_message_type = field_filter(
            &lark_login,
            "message_type",
            EventOperator::Equals,
            Some(json!("text")),
        );
        assert!(filter_matches(
            Some(&lark_message_type),
            "",
            &json!({"message_type": "text"})
        ));
        assert!(!filter_matches(
            Some(&lark_message_type),
            "",
            &json!({"message_type": "image"})
        ));

        let lark_chat_type = field_filter(
            &lark_bot,
            "chat_type",
            EventOperator::Equals,
            Some(json!("group")),
        );
        assert!(filter_matches(
            Some(&lark_chat_type),
            "",
            &json!({"chat_type": "group"})
        ));
        assert!(!filter_matches(
            Some(&lark_chat_type),
            "",
            &json!({"chat_type": "p2p"})
        ));
    }

    #[test]
    fn telegram_enum_labels_map_to_wire_values() {
        let telegram = bundled_schema("telegram-user");
        let field = telegram
            .fields
            .iter()
            .find(|field| field.path == "chat_kind")
            .expect("chat kind field");
        let values = field
            .values
            .iter()
            .map(|value| {
                (
                    value.label.as_str(),
                    value.value.as_str().expect("string enum value"),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(values.get("Group"), Some(&"group"));
        assert_eq!(values.get("Channel"), Some(&"channel"));
        assert_eq!(values.get("Direct message"), Some(&"user"));
    }
}
