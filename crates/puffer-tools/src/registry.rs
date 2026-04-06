use crate::external::{
    builtin_handler_name, execute_runtime, runtime_from_definition, ToolRuntime,
};
use crate::{
    builtin_tool_definition_by_handler, ToolDefinition, ToolDisplayHints, ToolExecutionResult,
    ToolInput, ToolInputSchema, ToolKind, ToolMetadata, ToolPolicyHints, ToolPropertySchema,
    ToolSchemaType,
};
use anyhow::{anyhow, Result};
use puffer_resources::{LoadedResources, ToolSpec};
use std::collections::BTreeMap;
use std::path::Path;

/// One registered tool with its declarative metadata and runtime kind.
pub struct RegisteredTool {
    pub spec: ToolDefinition,
    runtime: ToolRuntime,
}

impl RegisteredTool {
    /// Returns the model-facing definition for the registered tool.
    pub fn definition(&self) -> &ToolDefinition {
        &self.spec
    }
}

/// Registry of executable built-in tools derived from loaded resources.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, RegisteredTool>,
}

impl ToolRegistry {
    /// Builds a tool registry from declarative tool definitions.
    pub fn from_definitions(definitions: impl IntoIterator<Item = ToolDefinition>) -> Self {
        let mut registry = Self::default();
        for definition in definitions {
            let _ = registry.register(definition);
        }
        registry
    }

    /// Builds a tool registry from loaded declarative resources.
    pub fn from_resources(resources: &LoadedResources) -> Self {
        Self::from_definitions(
            resources
                .tools
                .iter()
                .filter_map(|item| definition_from_spec(&item.value)),
        )
    }

    /// Registers one declarative tool definition when it maps to a supported runtime.
    pub fn register(&mut self, definition: ToolDefinition) -> Result<()> {
        let runtime = runtime_from_definition(&definition)?;
        self.tools.insert(
            definition.id.clone(),
            RegisteredTool {
                spec: definition,
                runtime,
            },
        );
        Ok(())
    }

    /// Returns the number of executable tools in the registry.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Returns true when the registry has no executable tools.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Returns all registered tools in stable id order.
    pub fn tools(&self) -> impl Iterator<Item = &RegisteredTool> {
        self.tools.values()
    }

    /// Returns all model-facing tool definitions in stable id order.
    pub fn definitions(&self) -> impl Iterator<Item = &ToolDefinition> {
        self.tools.values().map(RegisteredTool::definition)
    }

    /// Looks up a registered tool by id.
    pub fn tool(&self, tool_id: &str) -> Option<&RegisteredTool> {
        self.tools.get(tool_id)
    }

    /// Looks up a model-facing tool definition by id.
    pub fn definition(&self, tool_id: &str) -> Option<&ToolDefinition> {
        self.tool(tool_id).map(RegisteredTool::definition)
    }

    /// Executes a registered tool by id with typed input.
    pub fn execute(
        &self,
        tool_id: &str,
        cwd: &Path,
        input: ToolInput,
    ) -> Result<ToolExecutionResult> {
        let payload = serde_json::to_value(input)?;
        self.execute_json(tool_id, cwd, payload)
    }

    /// Executes a registered tool by id using a raw JSON input payload.
    pub fn execute_json(
        &self,
        tool_id: &str,
        cwd: &Path,
        input: serde_json::Value,
    ) -> Result<ToolExecutionResult> {
        let tool = self
            .tool(tool_id)
            .ok_or_else(|| anyhow!("unknown tool {tool_id}"))?;
        execute_runtime(&tool.runtime, &tool.spec, cwd, input)
    }
}

fn definition_from_spec(spec: &ToolSpec) -> Option<ToolDefinition> {
    let mut definition = builtin_tool_definition_by_handler(&builtin_handler_name(&spec.handler))
        .unwrap_or_else(|| ToolDefinition {
            id: spec.id.clone(),
            name: spec.name.clone(),
            description: spec.description.clone(),
            handler: spec.handler.clone(),
            handler_args: spec.handler_args.clone(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: spec.shared_lib.clone(),
            enabled_if: spec.enabled_if.clone(),
            display: ToolDisplayHints::default(),
        });
    definition.id = spec.id.clone();
    definition.name = spec.name.clone();
    definition.description = spec.description.clone();
    definition.handler = spec.handler.clone();
    definition.handler_args = spec.handler_args.clone();
    if let Some(input_schema) = &spec.input_schema {
        if let Ok(parsed) = parse_input_schema(input_schema) {
            definition.input_schema = parsed;
        }
    }
    definition.metadata.may_spawn_processes |= spec.metadata.may_spawn_processes;
    definition.metadata.may_read_files |= spec.metadata.may_read_files;
    definition.metadata.may_write_files |= spec.metadata.may_write_files;
    definition.policy = ToolPolicyHints {
        approval_policy: spec.approval_policy.clone(),
        sandbox_policy: spec.sandbox_policy.clone(),
    };
    definition.shared_lib = spec.shared_lib.clone();
    definition.enabled_if = spec.enabled_if.clone();
    definition.display = ToolDisplayHints {
        group: spec.display.group.clone(),
        title: spec.display.title.clone(),
        show_in_status: spec.display.show_in_status,
    };
    Some(definition)
}

fn parse_input_schema(input_schema: &serde_json::Value) -> Result<ToolInputSchema> {
    let properties = input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!("input_schema is missing properties"))?;
    let required = input_schema
        .get("required")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut parsed = BTreeMap::new();
    for (name, property) in properties {
        let value_type = match property.get("type").and_then(serde_json::Value::as_str) {
            Some("string") | None => ToolSchemaType::String,
            Some("object") => ToolSchemaType::Object,
            Some("boolean") => ToolSchemaType::Boolean,
            Some("integer") => ToolSchemaType::Integer,
            Some("array") => ToolSchemaType::Array,
            Some(other) => return Err(anyhow!("unsupported input schema type {other}")),
        };
        let description = property
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        parsed.insert(
            name.clone(),
            ToolPropertySchema {
                value_type,
                description,
                required: required.iter().any(|required_name| required_name == name),
            },
        );
    }
    Ok(ToolInputSchema { properties: parsed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_resources::{LoadedItem, SourceInfo, SourceKind};
    use std::path::PathBuf;

    fn bash_tool_spec() -> ToolSpec {
        ToolSpec {
            id: "bash".to_string(),
            name: "bash".to_string(),
            description: "Run shell".to_string(),
            handler: "bash".to_string(),
            handler_args: Vec::new(),
            approval_policy: Some("on-request".to_string()),
            sandbox_policy: Some("workspace-write".to_string()),
            shared_lib: None,
            enabled_if: None,
            input_schema: None,
            metadata: Default::default(),
            display: Default::default(),
        }
    }

    #[test]
    fn registry_builds_from_resources() {
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: bash_tool_spec(),
                source_info: SourceInfo {
                    path: PathBuf::from("bash.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let registry = ToolRegistry::from_resources(&resources);
        assert!(registry.tool("bash").is_some());
    }

    #[test]
    fn registry_builds_from_definitions() {
        let registry = ToolRegistry::from_definitions([ToolKind::ReadFile.definition()]);
        assert_eq!(registry.len(), 1);
        assert!(registry.tool("read_file").is_some());
    }

    #[test]
    fn register_rejects_unknown_handlers() {
        let mut registry = ToolRegistry::default();
        let error = registry
            .register(ToolDefinition {
                id: "custom".to_string(),
                name: "custom".to_string(),
                description: "Custom".to_string(),
                handler: "custom_handler".to_string(),
                handler_args: Vec::new(),
                kind: ToolKind::Custom,
                input_schema: ToolInputSchema::default(),
                metadata: ToolMetadata::default(),
                policy: ToolPolicyHints::default(),
                shared_lib: None,
                enabled_if: None,
                display: ToolDisplayHints::default(),
            })
            .unwrap_err();
        assert!(error.to_string().contains("unsupported tool handler"));
    }

    #[test]
    fn registry_registers_external_exec_handlers() {
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: ToolSpec {
                    id: "echo_json".to_string(),
                    name: "echo_json".to_string(),
                    description: "Echo JSON".to_string(),
                    handler: "exec:python3".to_string(),
                    handler_args: vec![
                        "-c".to_string(),
                        "import json,sys; data=json.load(sys.stdin); print(json.dumps({'received': data}))"
                            .to_string(),
                    ],
                    approval_policy: None,
                    sandbox_policy: None,
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: None,
                    metadata: Default::default(),
                    display: Default::default(),
                },
                source_info: SourceInfo {
                    path: PathBuf::from("custom.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let registry = ToolRegistry::from_resources(&resources);
        let temp = tempfile::tempdir().unwrap();
        let result = registry
            .execute_json(
                "echo_json",
                temp.path(),
                serde_json::json!({ "path": "demo.txt" }),
            )
            .unwrap();
        assert!(result.success);
        assert!(result.output.stdout.contains("demo.txt"));
        assert_eq!(
            result.output.metadata["received"]["path"],
            serde_json::Value::String("demo.txt".to_string())
        );
    }

    #[test]
    fn resource_registry_carries_model_metadata_and_policy_hints() {
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: bash_tool_spec(),
                source_info: SourceInfo {
                    path: PathBuf::from("bash.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let registry = ToolRegistry::from_resources(&resources);
        let definition = registry.definition("bash").unwrap();
        assert_eq!(definition.description, "Run shell");
        assert_eq!(
            definition.policy.approval_policy.as_deref(),
            Some("on-request")
        );
        assert_eq!(
            definition.input_schema.properties["command"].value_type,
            crate::ToolSchemaType::String
        );
        assert!(definition.metadata.may_spawn_processes);
    }

    #[test]
    fn registry_accepts_builtin_prefix_and_schema_override() {
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: ToolSpec {
                    handler: "builtin:bash".to_string(),
                    input_schema: Some(serde_json::json!({
                        "properties": {
                            "command": {
                                "type": "string",
                                "description": "Command override"
                            }
                        },
                        "required": ["command"]
                    })),
                    ..bash_tool_spec()
                },
                source_info: SourceInfo {
                    path: PathBuf::from("bash.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let registry = ToolRegistry::from_resources(&resources);
        let definition = registry.definition("bash").expect("tool definition");
        assert_eq!(definition.handler, "builtin:bash");
        assert_eq!(
            definition.input_schema.properties["command"].description,
            "Command override"
        );
    }

    #[test]
    fn registry_schema_override_supports_boolean_integer_and_array_types() {
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: ToolSpec {
                    handler: "builtin:bash".to_string(),
                    input_schema: Some(serde_json::json!({
                        "properties": {
                            "flag": {
                                "type": "boolean",
                                "description": "Boolean flag"
                            },
                            "count": {
                                "type": "integer",
                                "description": "Integer count"
                            },
                            "paths": {
                                "type": "array",
                                "description": "Path list"
                            }
                        }
                    })),
                    ..bash_tool_spec()
                },
                source_info: SourceInfo {
                    path: PathBuf::from("bash.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let registry = ToolRegistry::from_resources(&resources);
        let definition = registry.definition("bash").expect("tool definition");
        assert_eq!(
            definition.input_schema.properties["flag"].value_type,
            crate::ToolSchemaType::Boolean
        );
        assert_eq!(
            definition.input_schema.properties["count"].value_type,
            crate::ToolSchemaType::Integer
        );
        assert_eq!(
            definition.input_schema.properties["paths"].value_type,
            crate::ToolSchemaType::Array
        );
    }

    #[test]
    fn execute_json_parses_input_for_registered_tool() {
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: bash_tool_spec(),
                source_info: SourceInfo {
                    path: PathBuf::from("bash.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let registry = ToolRegistry::from_resources(&resources);
        let temp = tempfile::tempdir().unwrap();
        let result = registry
            .execute_json(
                "bash",
                temp.path(),
                serde_json::json!({ "command": "printf hi" }),
            )
            .unwrap();
        assert_eq!(result.output.stdout, "hi");
    }
}
