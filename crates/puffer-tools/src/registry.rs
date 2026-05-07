use crate::agent_prompt::render_agent_tool_description;
use crate::config_prompt::render_config_tool_description;
use crate::external::{
    builtin_handler_name, execute_runtime, runtime_from_definition, ToolRuntime,
};
use crate::mcp_qualify::{qualify_tools, McpToolKey};
use crate::{
    builtin_tool_definition_by_handler, ToolDefinition, ToolDisplayHints, ToolExecutionResult,
    ToolInput, ToolInputSchema, ToolKind, ToolMetadata, ToolPolicyHints, ToolPropertySchema,
    ToolSchemaType,
};
use anyhow::{anyhow, Result};
use puffer_resources::{plugin_mcp_servers, LoadedResources, ToolSpec};
use std::collections::BTreeMap;
use std::path::Path;
use time::{Month, OffsetDateTime};

/// One MCP-discovered tool ready for `ToolRegistry::register_mcp_tools`.
///
/// Mirrors the subset of `puffer_runner_api::McpTool` the registry needs,
/// kept independent so `puffer-tools` doesn't have to depend on the runner
/// API crate.
#[derive(Debug, Clone)]
pub struct McpToolEntry {
    pub server: String,
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Option<serde_json::Value>,
}

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
    aliases: BTreeMap<String, String>,
    has_mcp_resource_servers: bool,
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
        let mut registry = Self::from_definitions(
            resources
                .tools
                .iter()
                .filter_map(|item| definition_from_spec(&item.value)),
        );
        registry.has_mcp_resource_servers =
            !resources.mcp_servers.is_empty() || !plugin_mcp_servers(resources).is_empty();
        if let Some(tool) = registry.tools.get_mut("Agent") {
            tool.spec.description = render_agent_tool_description(resources);
        }
        if let Some(tool) = registry.tools.get_mut("Config") {
            tool.spec.description = render_config_tool_description(resources);
        }
        registry
    }

    /// Registers one MCP-discovered tool per `(server, name, schema)` row.
    ///
    /// Each entry is added under its qualified `mcp__<server>__<tool>` id
    /// with handler `runtime:mcp_call` and `handler_args = [server, tool]`,
    /// so the runtime executor can dispatch back to the runner without
    /// having to undo sanitization. Collisions are resolved via the same
    /// hash-suffix strategy codex uses (see `mcp_qualify::qualify_tools`).
    pub fn register_mcp_tools(
        &mut self,
        rows: impl IntoIterator<Item = McpToolEntry>,
    ) -> Result<()> {
        let entries: Vec<McpToolEntry> = rows.into_iter().collect();
        let qualified = qualify_tools(
            entries
                .iter()
                .map(|entry| McpToolKey {
                    server: entry.server.clone(),
                    tool: entry.name.clone(),
                })
                .collect(),
        );
        for (entry, qualified) in entries.into_iter().zip(qualified.into_iter()) {
            let input_schema = parse_mcp_input_schema(entry.input_schema.as_ref());
            let definition = ToolDefinition {
                id: qualified.qualified_name.clone(),
                name: qualified.qualified_name.clone(),
                description: entry.description.unwrap_or_default(),
                handler: "runtime:mcp_call".to_string(),
                aliases: Vec::new(),
                handler_args: vec![entry.server.clone(), entry.name.clone()],
                kind: ToolKind::Custom,
                input_schema,
                metadata: ToolMetadata::default(),
                policy: ToolPolicyHints {
                    approval_policy: Some("auto".to_string()),
                    sandbox_policy: Some("read-only".to_string()),
                },
                shared_lib: None,
                enabled_if: None,
                display: ToolDisplayHints::default(),
            };
            self.register(definition)?;
        }
        Ok(())
    }

    /// Registers one declarative tool definition when it maps to a supported runtime.
    pub fn register(&mut self, definition: ToolDefinition) -> Result<()> {
        let runtime = runtime_from_definition(&definition)?;
        let canonical_id = definition.id.clone();
        for alias in &definition.aliases {
            self.aliases.insert(alias.clone(), canonical_id.clone());
        }
        self.tools.insert(
            canonical_id,
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

    /// Returns whether the loaded resources include MCP servers that can expose resources.
    pub fn has_mcp_resource_servers(&self) -> bool {
        self.has_mcp_resource_servers
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
        self.tools.get(tool_id).or_else(|| {
            self.aliases
                .get(tool_id)
                .and_then(|canonical_id| self.tools.get(canonical_id))
        })
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
    if puffer_builtin_browser_disabled() && spec.handler == "runtime:browser" {
        return None;
    }
    if spec
        .approval_policy
        .as_deref()
        .is_some_and(policy_value_disables_tool)
        || spec
            .enabled_if
            .as_deref()
            .is_some_and(enabled_if_value_disables_tool)
    {
        return None;
    }

    let mut definition = builtin_tool_definition_by_handler(&builtin_handler_name(&spec.handler))
        .unwrap_or_else(|| ToolDefinition {
            id: spec.id.clone(),
            name: spec.name.clone(),
            description: spec.description.clone(),
            handler: spec.handler.clone(),
            aliases: spec.aliases.clone(),
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
    definition.description = render_tool_description(&spec.description);
    definition.handler = spec.handler.clone();
    definition.aliases = spec.aliases.clone();
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

fn puffer_builtin_browser_disabled() -> bool {
    std::env::var("PUFFER_NO_BROWSER")
        .ok()
        .is_some_and(|value| enabled_if_value_disables_tool(&value) || value.trim() == "1")
}

fn parse_mcp_input_schema(schema: Option<&serde_json::Value>) -> ToolInputSchema {
    match schema {
        Some(value) => parse_input_schema(value).unwrap_or_else(|_| ToolInputSchema {
            properties: BTreeMap::new(),
            raw_json_schema: serde_json::to_string(value).ok(),
        }),
        None => ToolInputSchema::default(),
    }
}

fn parse_input_schema(input_schema: &serde_json::Value) -> Result<ToolInputSchema> {
    let raw_json_schema = serde_json::to_string(input_schema)?;
    let properties = input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object);
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
    for (name, property) in properties.into_iter().flatten() {
        let value_type = match property.get("type").and_then(serde_json::Value::as_str) {
            Some("string") => ToolSchemaType::String,
            Some("object") => ToolSchemaType::Object,
            Some("boolean") => ToolSchemaType::Boolean,
            Some("number") => ToolSchemaType::Number,
            Some("integer") => ToolSchemaType::Integer,
            Some("array") => ToolSchemaType::Array,
            Some(other) => return Err(anyhow!("unsupported input schema type {other}")),
            None => infer_schema_type(property),
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
    Ok(ToolInputSchema {
        properties: parsed,
        raw_json_schema: Some(raw_json_schema),
    })
}

fn infer_schema_type(property: &serde_json::Value) -> ToolSchemaType {
    if property.get("properties").is_some() || property.get("additionalProperties").is_some() {
        return ToolSchemaType::Object;
    }
    if property.get("items").is_some() {
        return ToolSchemaType::Array;
    }
    property
        .get("oneOf")
        .and_then(serde_json::Value::as_array)
        .or_else(|| property.get("anyOf").and_then(serde_json::Value::as_array))
        .into_iter()
        .flatten()
        .find_map(
            |schema| match schema.get("type").and_then(serde_json::Value::as_str) {
                Some("string") => Some(ToolSchemaType::String),
                Some("object") => Some(ToolSchemaType::Object),
                Some("boolean") => Some(ToolSchemaType::Boolean),
                Some("number") => Some(ToolSchemaType::Number),
                Some("integer") => Some(ToolSchemaType::Integer),
                Some("array") => Some(ToolSchemaType::Array),
                _ => None,
            },
        )
        .unwrap_or(ToolSchemaType::String)
}

fn render_tool_description(description: &str) -> String {
    description
        .replace("{{CURRENT_MONTH_YEAR}}", &current_month_year())
        .lines()
        .map(|line| if line.trim().is_empty() { "" } else { line })
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

fn current_month_year() -> String {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    format!("{} {}", month_name(now.month()), now.year())
}

fn month_name(month: Month) -> &'static str {
    match month {
        Month::January => "January",
        Month::February => "February",
        Month::March => "March",
        Month::April => "April",
        Month::May => "May",
        Month::June => "June",
        Month::July => "July",
        Month::August => "August",
        Month::September => "September",
        Month::October => "October",
        Month::November => "November",
        Month::December => "December",
    }
}

fn policy_value_disables_tool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "disabled" | "deny"
    )
}

fn enabled_if_value_disables_tool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "disabled" | "deny" | "false" | "never" | "off"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_resources::{LoadedItem, SourceInfo, SourceKind};
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn workspace_builtin_tool_resources() -> LoadedResources {
        let tools_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/tools");
        let mut tool_paths = fs::read_dir(&tools_dir)
            .expect("read builtin tool dir")
            .map(|entry| entry.expect("directory entry").path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("yaml"))
            .collect::<Vec<_>>();
        tool_paths.sort();

        let tools = tool_paths
            .into_iter()
            .map(|path| {
                let spec = serde_yaml::from_str::<ToolSpec>(
                    &fs::read_to_string(&path).expect("read builtin tool resource"),
                )
                .expect("parse builtin tool resource");
                LoadedItem {
                    value: spec,
                    source_info: SourceInfo {
                        path,
                        kind: SourceKind::Builtin,
                    },
                }
            })
            .collect();

        LoadedResources {
            tools,
            ..LoadedResources::default()
        }
    }

    fn bash_tool_spec() -> ToolSpec {
        ToolSpec {
            id: "bash".to_string(),
            name: "bash".to_string(),
            description: "Run shell".to_string(),
            handler: "bash".to_string(),
            aliases: Vec::new(),
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

    fn browser_tool_spec() -> ToolSpec {
        ToolSpec {
            id: "Browser".to_string(),
            name: "Browser".to_string(),
            description: "Control browser".to_string(),
            handler: "runtime:browser".to_string(),
            aliases: vec!["browser".to_string()],
            handler_args: Vec::new(),
            approval_policy: Some("auto".to_string()),
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
    fn registry_builds_runtime_sleep_tool_from_resources() {
        let description = "Wait for a specified duration. The user can interrupt the sleep at any time.\n\nUse this when the user tells you to sleep or rest, when you have nothing to do, or when you're waiting for something.\n\nYou may receive <tick> prompts — these are periodic check-ins. Look for useful work to do before sleeping.\n\nYou can call this concurrently with other tools — it won't interfere with them.\n\nPrefer this over `Bash(sleep ...)` — it doesn't hold a shell process.\n\nEach wake-up costs an API call, but the prompt cache expires after 5 minutes of inactivity — balance accordingly.";
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: ToolSpec {
                    id: "Sleep".to_string(),
                    name: "Sleep".to_string(),
                    description: description.to_string(),
                    handler: "runtime:sleep".to_string(),
                    aliases: Vec::new(),
                    handler_args: Vec::new(),
                    approval_policy: Some("never".to_string()),
                    sandbox_policy: Some("read-only".to_string()),
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "duration_ms": {
                                "type": "integer",
                                "description": "Duration to wait in milliseconds."
                            }
                        },
                        "required": ["duration_ms"],
                        "additionalProperties": false
                    })),
                    metadata: Default::default(),
                    display: Default::default(),
                },
                source_info: SourceInfo {
                    path: PathBuf::from("sleep.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let registry = ToolRegistry::from_resources(&resources);
        let definition = registry.definition("Sleep").expect("sleep tool definition");
        assert_eq!(definition.description, description);
        assert_eq!(definition.handler, "runtime:sleep");
        assert_eq!(
            definition.input_schema.as_json_schema()["required"],
            serde_json::json!(["duration_ms"])
        );
    }

    #[test]
    fn puffer_no_browser_hides_builtin_browser_tool() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("PUFFER_NO_BROWSER", "1");
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: browser_tool_spec(),
                source_info: SourceInfo {
                    path: PathBuf::from("browser.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let registry = ToolRegistry::from_resources(&resources);
        std::env::remove_var("PUFFER_NO_BROWSER");

        assert!(registry.definition("Browser").is_none());
        assert!(registry.definition("browser").is_none());
    }

    #[test]
    fn puffer_browser_tool_registers_when_not_disabled() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("PUFFER_NO_BROWSER");
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: browser_tool_spec(),
                source_info: SourceInfo {
                    path: PathBuf::from("browser.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let registry = ToolRegistry::from_resources(&resources);

        assert!(registry.definition("Browser").is_some());
        assert!(registry.definition("browser").is_some());
    }

    #[test]
    fn workspace_builtin_tool_resources_are_registerable() {
        let resources = workspace_builtin_tool_resources();
        let registry = ToolRegistry::from_resources(&resources);
        let expected = resources
            .tools
            .iter()
            .filter_map(|item| definition_from_spec(&item.value))
            .map(|definition| definition.id)
            .collect::<BTreeSet<_>>();
        let registered = registry
            .definitions()
            .map(|definition| definition.id.clone())
            .collect::<BTreeSet<_>>();
        let missing = expected
            .difference(&registered)
            .cloned()
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "builtin resources produced unsupported tool registrations: {missing:?}"
        );
        assert!(registry.definition("Sleep").is_some());
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
                aliases: Vec::new(),
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
                    aliases: Vec::new(),
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
    fn registry_schema_override_supports_boolean_number_integer_and_array_types() {
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
                            "timeout": {
                                "type": "number",
                                "description": "Fractional timeout"
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
            definition.input_schema.properties["timeout"].value_type,
            crate::ToolSchemaType::Number
        );
        assert_eq!(
            definition.input_schema.properties["count"].value_type,
            crate::ToolSchemaType::Integer
        );
        assert_eq!(
            definition.input_schema.properties["paths"].value_type,
            crate::ToolSchemaType::Array
        );
        assert_eq!(
            definition.input_schema.as_json_schema()["properties"]["timeout"]["type"].as_str(),
            Some("number")
        );
    }

    #[test]
    fn registry_schema_override_preserves_nested_json_schema() {
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: ToolSpec {
                    handler: "builtin:bash".to_string(),
                    input_schema: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "message": {
                                "description": "Structured message content",
                                "anyOf": [
                                    { "type": "string" },
                                    {
                                        "type": "object",
                                        "properties": {
                                            "type": {
                                                "type": "string",
                                                "enum": ["shutdown_request"]
                                            }
                                        },
                                        "required": ["type"]
                                    }
                                ]
                            }
                        },
                        "required": ["message"],
                        "additionalProperties": false
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
        let schema = definition.input_schema.as_json_schema();
        assert_eq!(schema["required"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            schema["properties"]["message"]["anyOf"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn registry_schema_override_allows_object_schemas_without_properties() {
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: ToolSpec {
                    handler: "builtin:bash".to_string(),
                    input_schema: Some(serde_json::json!({
                        "type": "object",
                        "additionalProperties": true,
                        "description": "Dynamic payload"
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
        let schema = definition.input_schema.as_json_schema();
        assert_eq!(schema["type"].as_str(), Some("object"));
        assert_eq!(schema["additionalProperties"].as_bool(), Some(true));
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

    #[test]
    fn registry_resolves_tool_aliases_to_the_canonical_definition() {
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: ToolSpec {
                    id: "SendUserMessage".to_string(),
                    name: "SendUserMessage".to_string(),
                    description: "Send a message to the user".to_string(),
                    handler: "runtime:workflow:send_user_message".to_string(),
                    aliases: vec!["Brief".to_string()],
                    handler_args: Vec::new(),
                    approval_policy: Some("auto".to_string()),
                    sandbox_policy: Some("read-only".to_string()),
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "message": { "type": "string" },
                            "status": { "type": "string" }
                        },
                        "required": ["message", "status"],
                        "additionalProperties": false
                    })),
                    metadata: Default::default(),
                    display: Default::default(),
                },
                source_info: SourceInfo {
                    path: PathBuf::from("send_user_message.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let registry = ToolRegistry::from_resources(&resources);
        let definition = registry
            .definition("Brief")
            .expect("Brief alias should resolve");

        assert_eq!(definition.id, "SendUserMessage");
        assert_eq!(definition.aliases, vec!["Brief".to_string()]);
    }

    #[test]
    fn register_mcp_tools_exposes_qualified_ids_with_handler_args() {
        let mut registry = ToolRegistry::default();
        registry
            .register_mcp_tools(vec![
                McpToolEntry {
                    server: "playwright".to_string(),
                    name: "browser_navigate".to_string(),
                    description: Some("Open a URL".to_string()),
                    input_schema: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "url": { "type": "string", "description": "Target URL" }
                        },
                        "required": ["url"]
                    })),
                },
                McpToolEntry {
                    server: "playwright".to_string(),
                    name: "browser_close".to_string(),
                    description: None,
                    input_schema: None,
                },
            ])
            .unwrap();
        let definition = registry
            .definition("mcp__playwright__browser_navigate")
            .expect("qualified definition registered");
        assert_eq!(definition.handler, "runtime:mcp_call");
        assert_eq!(
            definition.handler_args,
            vec!["playwright".to_string(), "browser_navigate".to_string()]
        );
        assert_eq!(definition.description, "Open a URL");
        assert_eq!(
            definition.input_schema.properties["url"].value_type,
            crate::ToolSchemaType::String
        );

        let close = registry
            .definition("mcp__playwright__browser_close")
            .expect("second qualified definition registered");
        assert_eq!(close.description, "");
        assert!(close.input_schema.properties.is_empty());
    }

    #[test]
    fn register_mcp_tools_handles_collisions_with_hash_suffix() {
        let mut registry = ToolRegistry::default();
        registry
            .register_mcp_tools(vec![
                McpToolEntry {
                    server: "a".to_string(),
                    name: "x".to_string(),
                    description: None,
                    input_schema: None,
                },
                McpToolEntry {
                    server: "a".to_string(),
                    name: "x".to_string(),
                    description: None,
                    input_schema: None,
                },
            ])
            .unwrap();
        let qualified_ids: Vec<String> = registry
            .definitions()
            .map(|definition| definition.id.clone())
            .collect();
        assert!(qualified_ids.iter().any(|id| id == "mcp__a__x"));
        assert!(qualified_ids.iter().any(|id| id.starts_with("mcp__a__x__")));
    }

    #[test]
    fn registry_interpolates_current_month_year_placeholders_in_descriptions() {
        let resources = LoadedResources {
            tools: vec![LoadedItem {
                value: ToolSpec {
                    id: "WebSearch".to_string(),
                    name: "WebSearch".to_string(),
                    description: "Month: {{CURRENT_MONTH_YEAR}}".to_string(),
                    handler: "runtime:workflow:web_search".to_string(),
                    aliases: Vec::new(),
                    handler_args: Vec::new(),
                    approval_policy: Some("auto".to_string()),
                    sandbox_policy: Some("read-only".to_string()),
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" }
                        },
                        "required": ["query"],
                        "additionalProperties": false
                    })),
                    metadata: Default::default(),
                    display: Default::default(),
                },
                source_info: SourceInfo {
                    path: PathBuf::from("web_search.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };
        let registry = ToolRegistry::from_resources(&resources);
        let definition = registry.definition("WebSearch").expect("tool definition");
        assert!(definition.description.starts_with("Month: "));
        assert!(!definition.description.contains("{{CURRENT_MONTH_YEAR}}"));
    }
}
