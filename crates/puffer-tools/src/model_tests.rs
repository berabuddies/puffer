use crate::{
    builtin_tool_definition, builtin_tool_definitions, ToolInput, ToolInputSchema, ToolKind,
    ToolPropertySchema, ToolSchemaType,
};
use serde_json::Value;
use std::collections::BTreeMap;

#[test]
fn builtin_definitions_cover_all_supported_tools() {
    let definitions = builtin_tool_definitions();
    let ids = definitions
        .iter()
        .map(|definition| definition.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            "bash",
            "read_file",
            "write_file",
            "replace_in_file",
            "move_path",
            "remove_path",
            "list_dir",
            "search_text"
        ]
    );
}

#[test]
fn write_file_schema_is_exported_for_models() {
    let definition = builtin_tool_definition(ToolKind::WriteFile);
    let schema = definition.input_schema.as_json_schema();
    let required = schema.get("required").and_then(Value::as_array).unwrap();
    assert_eq!(required.len(), 2);
    assert!(required
        .iter()
        .any(|value| value.as_str() == Some("contents")));
    assert_eq!(
        schema["properties"]["path"]["type"].as_str(),
        Some("string")
    );
    assert_eq!(schema["additionalProperties"].as_bool(), Some(false));
}

#[test]
fn schema_export_supports_boolean_integer_and_array_types() {
    let schema = ToolInputSchema {
        properties: BTreeMap::from([
            (
                "flag".to_string(),
                ToolPropertySchema {
                    value_type: ToolSchemaType::Boolean,
                    description: "Boolean flag".to_string(),
                    required: false,
                },
            ),
            (
                "count".to_string(),
                ToolPropertySchema {
                    value_type: ToolSchemaType::Integer,
                    description: "Count".to_string(),
                    required: false,
                },
            ),
            (
                "paths".to_string(),
                ToolPropertySchema {
                    value_type: ToolSchemaType::Array,
                    description: "Paths".to_string(),
                    required: false,
                },
            ),
        ]),
        raw_json_schema: None,
    }
    .as_json_schema();
    assert_eq!(
        schema["properties"]["flag"]["type"].as_str(),
        Some("boolean")
    );
    assert_eq!(
        schema["properties"]["count"]["type"].as_str(),
        Some("integer")
    );
    assert_eq!(
        schema["properties"]["paths"]["type"].as_str(),
        Some("array")
    );
}

#[test]
fn raw_json_schema_round_trips_verbatim() {
    let schema = ToolInputSchema {
        properties: BTreeMap::new(),
        raw_json_schema: Some(
            serde_json::json!({
                "type": "object",
                "properties": {
                    "timeout": {
                        "type": "number",
                        "description": "Timeout in milliseconds"
                    },
                    "value": {
                        "oneOf": [
                            { "type": "string" },
                            { "type": "boolean" },
                            { "type": "number" }
                        ]
                    }
                },
                "required": ["timeout"],
                "additionalProperties": false
            })
            .to_string(),
        ),
    }
    .as_json_schema();
    assert_eq!(
        schema["properties"]["timeout"]["type"].as_str(),
        Some("number")
    );
    assert_eq!(
        schema["properties"]["value"]["oneOf"]
            .as_array()
            .map(Vec::len),
        Some(3)
    );
}

#[test]
fn boolean_fields_in_builtin_schemas_use_boolean_type() {
    let replace_schema = builtin_tool_definition(ToolKind::ReplaceInFile).input_schema;
    let remove_schema = builtin_tool_definition(ToolKind::RemovePath).input_schema;
    let bash_schema = builtin_tool_definition(ToolKind::Bash).input_schema;
    assert_eq!(
        replace_schema.properties["replace_all"].value_type,
        ToolSchemaType::Boolean
    );
    assert_eq!(
        remove_schema.properties["recursive"].value_type,
        ToolSchemaType::Boolean
    );
    assert_eq!(
        bash_schema.properties["run_in_background"].value_type,
        ToolSchemaType::Boolean
    );
    assert_eq!(
        bash_schema
            .properties
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["command", "run_in_background", "timeout"]
    );
    assert_eq!(
        bash_schema.properties["timeout"].value_type,
        ToolSchemaType::Integer
    );
}

#[test]
fn anthropic_tool_payload_uses_definition_schema() {
    let definition = builtin_tool_definition(ToolKind::Bash);
    let payload = definition.as_anthropic_tool();
    assert_eq!(payload["name"].as_str(), Some("bash"));
    assert_eq!(
        payload["input_schema"]["properties"]["command"]["type"].as_str(),
        Some("string")
    );
}

#[test]
fn serde_tool_input_uses_internal_tool_tag() {
    let input: ToolInput = serde_json::from_value(serde_json::json!({
        "tool": "write_file",
        "path": "notes/todo.txt",
        "contents": "ship it",
    }))
    .unwrap();
    assert_eq!(
        input,
        ToolInput::WriteFile {
            path: "notes/todo.txt".into(),
            contents: "ship it".to_string(),
        }
    );
}

#[test]
fn serde_read_input_accepts_offset_and_limit() {
    let input: ToolInput = serde_json::from_value(serde_json::json!({
        "tool": "read_file",
        "path": "notes/todo.txt",
        "offset": 10,
        "limit": 25,
    }))
    .unwrap();
    assert_eq!(
        input,
        ToolInput::ReadFile {
            path: "notes/todo.txt".into(),
            offset: Some(10),
            limit: Some(25),
        }
    );
}

#[test]
fn serde_bash_input_accepts_timeout_and_background_fields() {
    let input: ToolInput = serde_json::from_value(serde_json::json!({
        "tool": "bash",
        "command": "printf hi",
        "timeout": 2500,
        "run_in_background": false,
    }))
    .unwrap();
    assert_eq!(
        input,
        ToolInput::Bash {
            command: "printf hi".to_string(),
            timeout: Some(2500),
            run_in_background: false,
        }
    );
}

#[test]
fn tool_input_reports_kind() {
    assert_eq!(
        ToolInput::Bash {
            command: "printf hi".to_string(),
            timeout: None,
            run_in_background: false,
        }
        .kind(),
        ToolKind::Bash
    );
    assert_eq!(
        ToolInput::SearchText {
            query: "needle".to_string(),
            path: None,
        }
        .kind(),
        ToolKind::SearchText
    );
    assert_eq!(
        ToolInput::ReplaceInFile {
            path: "note.txt".into(),
            old: "a".to_string(),
            new: "b".to_string(),
            replace_all: false,
        }
        .kind(),
        ToolKind::ReplaceInFile
    );
    assert_eq!(
        ToolInput::MovePath {
            from: "a".into(),
            to: "b".into(),
        }
        .kind(),
        ToolKind::MovePath
    );
}
