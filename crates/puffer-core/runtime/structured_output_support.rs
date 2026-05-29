use crate::permissions::RuntimePermissionContext;
use anyhow::{anyhow, bail, Context, Result};
use puffer_provider_openai::{
    OpenAIChatCompletionTool, OpenAIChatCompletionToolFunction, OpenAIChatResponseFormat,
    OpenAIChatResponseJsonSchema, OpenAIResponsesTextConfig, OpenAIResponsesTextFormat,
    OpenAIResponsesTool,
};
use puffer_tools::{
    ToolDefinition, ToolDisplayHints, ToolInputSchema, ToolKind, ToolMetadata, ToolPolicyHints,
    ToolRegistry,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use time::{format_description, OffsetDateTime};

const STRUCTURED_OUTPUT_TOOL_ID: &str = "StructuredOutput";
const LIST_MCP_RESOURCES_TOOL_ID: &str = "ListMcpResourcesTool";
const READ_MCP_RESOURCE_TOOL_ID: &str = "ReadMcpResourceTool";
const WEB_SEARCH_TOOL_ID: &str = "WebSearch";
const WEB_SEARCH_CURRENT_YEAR_LINE: &str =
    "- You MUST use the current year when searching for recent information, documentation, or current events.";
const WEB_SEARCH_CURRENT_MONTH_PREFIX: &str = "- The current month is ";

/// Describes one request-scoped structured output contract for a model turn.
#[derive(Debug, Clone, PartialEq)]
pub struct StructuredOutputConfig {
    pub name: String,
    pub description: Option<String>,
    pub schema: Value,
}

impl StructuredOutputConfig {
    /// Creates a request-scoped structured output contract from a JSON Schema.
    pub fn new(name: impl Into<String>, schema: Value) -> Self {
        Self {
            name: name.into(),
            description: None,
            schema,
        }
    }

    /// Adds an optional human-readable description to the output contract.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

pub(super) fn validate_structured_output_schema(config: &StructuredOutputConfig) -> Result<()> {
    let name = config.name.trim();
    if name.is_empty() {
        bail!("StructuredOutput requires a non-empty schema name");
    }
    let schema = &config.schema;
    if !schema.is_object() {
        bail!("StructuredOutput schema must be a JSON object");
    }
    let _ = jsonschema::validator_for(schema)
        .map_err(|error| anyhow!("invalid StructuredOutput schema: {error}"))?;
    Ok(())
}

pub(super) fn validate_structured_output_payload(
    config: &StructuredOutputConfig,
    input: &Value,
) -> Result<()> {
    validate_structured_output_schema(config)?;
    let validator = jsonschema::validator_for(&config.schema)
        .map_err(|error| anyhow!("invalid StructuredOutput schema: {error}"))?;
    let errors = validator
        .iter_errors(input)
        .take(5)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        return Ok(());
    }
    bail!(
        "Output does not match required schema: {}",
        errors.join(", ")
    );
}

pub(super) fn anthropic_tool_definitions(
    registry: &ToolRegistry,
    structured_output: Option<&StructuredOutputConfig>,
) -> Result<Vec<Value>> {
    anthropic_tool_definitions_for_request(registry, structured_output, None)
}

pub(super) fn anthropic_tool_definitions_for_request(
    registry: &ToolRegistry,
    structured_output: Option<&StructuredOutputConfig>,
    permission_context: Option<&RuntimePermissionContext>,
) -> Result<Vec<Value>> {
    let definitions =
        anthropic_tool_definitions_with_structured_output(registry, structured_output)?;
    Ok(definitions
        .into_iter()
        .filter(|definition| tool_visible_to_model(permission_context, definition))
        .map(|definition| {
            json!({
                "name": definition.id,
                "description": rendered_tool_description(&definition),
                "input_schema": definition.input_schema.as_json_schema(),
            })
        })
        .collect())
}

pub(super) fn openai_tool_definitions(
    registry: &ToolRegistry,
    structured_output: Option<&StructuredOutputConfig>,
    use_native: bool,
) -> Result<Vec<OpenAIResponsesTool>> {
    openai_tool_definitions_for_request(registry, structured_output, use_native, None)
}

pub(super) fn openai_tool_definitions_for_request(
    registry: &ToolRegistry,
    structured_output: Option<&StructuredOutputConfig>,
    use_native: bool,
    permission_context: Option<&RuntimePermissionContext>,
) -> Result<Vec<OpenAIResponsesTool>> {
    let definitions =
        openai_tool_definitions_with_structured_output(registry, structured_output, use_native)?;
    Ok(definitions
        .into_iter()
        .filter(|definition| tool_visible_to_model(permission_context, definition))
        .map(|definition| {
            if is_native_openai_web_search(&definition) {
                native_openai_web_search_tool()
            } else {
                OpenAIResponsesTool {
                    kind: "function".to_string(),
                    name: definition.id.clone(),
                    description: rendered_openai_tool_description(&definition),
                    strict: false,
                    parameters: openai_tool_parameters_schema(
                        definition.input_schema.as_json_schema(),
                    ),
                    filters: None,
                    user_location: None,
                    external_web_access: None,
                }
            }
        })
        .collect())
}

fn is_native_openai_web_search(definition: &ToolDefinition) -> bool {
    if definition.id != WEB_SEARCH_TOOL_ID {
        return false;
    }
    if std::env::var("PUFFER_OPENAI_NATIVE_WEB_SEARCH")
        .ok()
        .is_some_and(|value| value == "0" || value.eq_ignore_ascii_case("false"))
    {
        return false;
    }
    definition.handler == "provider:web_search"
}

/// Builds the native OpenAI Responses `web_search` tool entry.
///
/// Mirrors codex's `create_web_search_tool` shape: `external_web_access`
/// must be set to a boolean (`true` for live, `false` for cached). The
/// bare `{"type": "web_search"}` form documented by OpenAI is not actually
/// accepted by the Responses API — codex carries a long-standing TODO
/// about this in `tools/src/tool_spec.rs`. Default to live access so
/// gpt-5/4-mini and friends actually invoke the tool; flip to cached via
/// `PUFFER_OPENAI_WEB_SEARCH_MODE=cached`.
fn native_openai_web_search_tool() -> OpenAIResponsesTool {
    let external_web_access = match std::env::var("PUFFER_OPENAI_WEB_SEARCH_MODE")
        .ok()
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("cached") => Some(false),
        _ => Some(true),
    };
    OpenAIResponsesTool {
        kind: "web_search".to_string(),
        name: String::new(),
        description: String::new(),
        strict: false,
        parameters: Value::Null,
        filters: None,
        user_location: None,
        external_web_access,
    }
}

pub(super) fn openai_chat_completion_tools(
    registry: &ToolRegistry,
    structured_output: Option<&StructuredOutputConfig>,
    use_native: bool,
) -> Result<Vec<OpenAIChatCompletionTool>> {
    openai_chat_completion_tools_for_request(registry, structured_output, use_native, None)
}

pub(super) fn openai_chat_completion_tools_for_request(
    registry: &ToolRegistry,
    structured_output: Option<&StructuredOutputConfig>,
    use_native: bool,
    permission_context: Option<&RuntimePermissionContext>,
) -> Result<Vec<OpenAIChatCompletionTool>> {
    let definitions =
        openai_tool_definitions_with_structured_output(registry, structured_output, use_native)?;
    Ok(definitions
        .into_iter()
        .filter(|definition| tool_visible_to_model(permission_context, definition))
        .map(|definition| OpenAIChatCompletionTool {
            kind: "function".to_string(),
            function: OpenAIChatCompletionToolFunction {
                name: definition.id.clone(),
                description: rendered_openai_tool_description(&definition),
                parameters: openai_tool_parameters_schema(definition.input_schema.as_json_schema()),
                strict: false,
            },
        })
        .collect())
}

pub(super) fn openai_responses_text_config(
    structured_output: Option<&StructuredOutputConfig>,
    use_native: bool,
) -> Option<OpenAIResponsesTextConfig> {
    if !use_native {
        return None;
    }
    let config = structured_output?;
    Some(OpenAIResponsesTextConfig {
        format: Some(OpenAIResponsesTextFormat {
            kind: "json_schema".to_string(),
            name: config.name.clone(),
            description: config.description.clone(),
            schema: openai_compatible_schema(config.schema.clone()),
            strict: true,
        }),
        verbosity: None,
    })
}

pub(super) fn openai_chat_response_format(
    structured_output: Option<&StructuredOutputConfig>,
    use_native: bool,
) -> Option<OpenAIChatResponseFormat> {
    if !use_native {
        return None;
    }
    let config = structured_output?;
    Some(OpenAIChatResponseFormat {
        kind: "json_schema".to_string(),
        json_schema: OpenAIChatResponseJsonSchema {
            name: config.name.clone(),
            description: config.description.clone(),
            schema: openai_compatible_schema(config.schema.clone()),
            strict: true,
        },
    })
}

fn anthropic_tool_definitions_with_structured_output(
    registry: &ToolRegistry,
    structured_output: Option<&StructuredOutputConfig>,
) -> Result<Vec<ToolDefinition>> {
    let mut definitions = registry
        .definitions()
        .filter(|definition| include_base_model_tool(registry, definition))
        .cloned()
        .collect::<Vec<_>>();
    if let Some(config) = structured_output {
        definitions.push(requested_structured_output_definition(registry, config)?);
    }
    Ok(definitions)
}

fn openai_tool_definitions_with_structured_output(
    registry: &ToolRegistry,
    structured_output: Option<&StructuredOutputConfig>,
    use_native: bool,
) -> Result<Vec<ToolDefinition>> {
    let mut definitions = registry
        .definitions()
        .filter(|definition| include_base_model_tool(registry, definition))
        .cloned()
        .collect::<Vec<_>>();
    if !use_native {
        if let Some(config) = structured_output {
            definitions.push(requested_structured_output_definition(registry, config)?);
        }
    }
    Ok(definitions)
}

fn include_base_model_tool(registry: &ToolRegistry, definition: &ToolDefinition) -> bool {
    if definition.id == STRUCTURED_OUTPUT_TOOL_ID {
        return false;
    }
    if matches!(
        definition.id.as_str(),
        LIST_MCP_RESOURCES_TOOL_ID | READ_MCP_RESOURCE_TOOL_ID
    ) {
        return registry.has_mcp_resource_servers();
    }
    true
}

fn tool_visible_to_model(
    permission_context: Option<&RuntimePermissionContext>,
    definition: &ToolDefinition,
) -> bool {
    permission_context
        .map(|context| context.tool_visible_to_model(definition))
        .unwrap_or(true)
}

fn requested_structured_output_definition(
    registry: &ToolRegistry,
    config: &StructuredOutputConfig,
) -> Result<ToolDefinition> {
    validate_structured_output_schema(config)?;
    let mut definition = registry
        .definition(STRUCTURED_OUTPUT_TOOL_ID)
        .cloned()
        .unwrap_or_else(|| synthetic_structured_output_definition(config));
    definition.input_schema = ToolInputSchema {
        properties: BTreeMap::new(),
        raw_json_schema: Some(
            serde_json::to_string(&config.schema).context("failed to serialize schema")?,
        ),
    };
    Ok(definition)
}

pub(super) fn requested_structured_output_definition_for_request(
    registry: &ToolRegistry,
    structured_output: Option<&StructuredOutputConfig>,
) -> Result<Option<ToolDefinition>> {
    structured_output
        .map(|config| requested_structured_output_definition(registry, config))
        .transpose()
}

fn synthetic_structured_output_definition(config: &StructuredOutputConfig) -> ToolDefinition {
    ToolDefinition {
        id: STRUCTURED_OUTPUT_TOOL_ID.to_string(),
        name: STRUCTURED_OUTPUT_TOOL_ID.to_string(),
        description: config
            .description
            .clone()
            .unwrap_or_else(|| "Return structured output in the requested format".to_string()),
        handler: "runtime:workflow:structured_output".to_string(),
        aliases: Vec::new(),
        handler_args: Vec::new(),
        kind: ToolKind::Custom,
        input_schema: ToolInputSchema::default(),
        metadata: ToolMetadata::default(),
        policy: ToolPolicyHints::default(),
        shared_lib: None,
        enabled_if: None,
        display: ToolDisplayHints::default(),
    }
}

fn rendered_tool_description(definition: &ToolDefinition) -> String {
    if definition.id == WEB_SEARCH_TOOL_ID {
        return render_web_search_description(&definition.description);
    }
    definition.description.clone()
}

fn rendered_openai_tool_description(definition: &ToolDefinition) -> String {
    match definition.id.as_str() {
        "Agent" => "Delegate an independent subtask to a subagent. Use for large or parallelizable work, not simple file reads or searches.".to_string(),
        "AskUserQuestion" => "Ask the user a short clarification or decision question when blocked by ambiguity.".to_string(),
        "TodoWrite" => "Update the task list with pending, in_progress, and completed items. Keep at most one item in progress.".to_string(),
        "Read" => "Read a file. Prefer reading the whole file unless it is large; use offset or limit for partial reads.".to_string(),
        "Glob" => "Find files by path pattern. Prefer this over shelling out to find or ls for discovery.".to_string(),
        "Grep" => "Search file contents with ripgrep-style patterns. Prefer this over running grep or rg in Bash.".to_string(),
        "Edit" => "Make an exact text edit in an existing file. Read the file first when needed.".to_string(),
        "Write" => "Write a file, creating parent directories if needed.".to_string(),
        "Bash" => "Run a shell command when no dedicated tool is a better fit.".to_string(),
        "TaskOutput" => "Read the saved output of a background task by id.".to_string(),
        "WebFetch" => "Fetch and summarize content from a specific URL.".to_string(),
        "WebSearch" => render_compact_web_search_description(),
        _ => compact_tool_description(definition),
    }
}

fn compact_tool_description(definition: &ToolDefinition) -> String {
    let trimmed = definition.description.trim();
    if trimmed.is_empty() {
        return definition.name.clone();
    }
    let first_paragraph = trimmed.split("\n\n").next().unwrap_or(trimmed).trim();
    if first_paragraph.len() <= 220 {
        first_paragraph.to_string()
    } else {
        let mut shortened = first_paragraph.chars().take(217).collect::<String>();
        shortened.push_str("...");
        shortened
    }
}

fn render_compact_web_search_description() -> String {
    format!(
        "Search the web for current or external information. The current month is {} and you must use this year when searching for recent information.",
        current_month_year()
    )
}

fn render_web_search_description(description: &str) -> String {
    let current_month_year = current_month_year();
    if let Some(existing_line) = description.lines().find(|line| {
        line.trim_start()
            .starts_with(WEB_SEARCH_CURRENT_MONTH_PREFIX)
    }) {
        let indent = existing_line
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .collect::<String>();
        let specific_line = format!(
            "{indent}- The current month is {current_month_year}. You MUST use this year when searching for recent information, documentation, or current events."
        );
        return description.replacen(existing_line, &specific_line, 1);
    }
    let specific_line = format!(
        "- The current month is {current_month_year}. You MUST use this year when searching for recent information, documentation, or current events."
    );
    if description.contains(WEB_SEARCH_CURRENT_YEAR_LINE) {
        return description.replace(WEB_SEARCH_CURRENT_YEAR_LINE, &specific_line);
    }
    format!("{description}\n{specific_line}")
}

fn current_month_year() -> String {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    let format =
        format_description::parse("[month repr:long] [year]").expect("valid month/year format");
    now.format(&format)
        .unwrap_or_else(|_| "the current month".to_string())
}

fn openai_compatible_schema(schema: Value) -> Value {
    match schema {
        Value::Object(mut object) => {
            if object.get("type").and_then(Value::as_str) == Some("array") {
                let items = object
                    .remove("items")
                    .map(openai_compatible_schema)
                    .unwrap_or_else(|| json!({ "type": "string" }));
                object.insert("items".to_string(), items);
            }

            if let Some(Value::Object(properties)) = object.get_mut("properties") {
                for property in properties.values_mut() {
                    let normalized = openai_compatible_schema(property.take());
                    *property = normalized;
                }
            }

            if let Some(items) = object.get_mut("items") {
                let normalized = openai_compatible_schema(items.take());
                *items = normalized;
            }

            Value::Object(object)
        }
        value => value,
    }
}

fn openai_tool_parameters_schema(schema: Value) -> Value {
    const DISALLOWED_ROOT_KEYS: [&str; 5] = ["oneOf", "anyOf", "allOf", "enum", "not"];

    let normalized = openai_compatible_schema(schema);
    let Value::Object(mut object) = normalized else {
        return json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        });
    };

    object.insert("type".to_string(), json!("object"));
    object
        .entry("properties".to_string())
        .or_insert_with(|| json!({}));
    for key in DISALLOWED_ROOT_KEYS {
        object.remove(key);
    }
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_tools::ToolRegistry;

    #[test]
    fn openai_tool_definitions_use_compact_agent_description() {
        let registry = ToolRegistry::from_definitions(vec![ToolDefinition {
            id: "Agent".to_string(),
            name: "Agent".to_string(),
            description:
                "Very long agent description that should not be forwarded verbatim to OpenAI."
                    .to_string(),
            handler: "runtime:agent".to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        }]);

        let definitions = openai_tool_definitions(&registry, None, false).unwrap();

        assert_eq!(definitions.len(), 1);
        assert_eq!(
            definitions[0].description,
            "Delegate an independent subtask to a subagent. Use for large or parallelizable work, not simple file reads or searches."
        );
    }

    #[test]
    fn openai_tool_descriptions_keep_current_month_in_web_search_description() {
        let description = rendered_openai_tool_description(&ToolDefinition {
            id: WEB_SEARCH_TOOL_ID.to_string(),
            name: WEB_SEARCH_TOOL_ID.to_string(),
            description: "Search the web.".to_string(),
            handler: "runtime:web_search".to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        });

        assert!(description.contains("The current month is"));
        assert!(description.contains("recent information"));
    }

    fn structured_output_registry() -> ToolRegistry {
        ToolRegistry::from_definitions(vec![ToolDefinition {
            id: STRUCTURED_OUTPUT_TOOL_ID.to_string(),
            name: STRUCTURED_OUTPUT_TOOL_ID.to_string(),
            description: "Structured output helper".to_string(),
            handler: "runtime:workflow:structured_output".to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        }])
    }

    #[test]
    fn payload_validation_rejects_mismatched_instances() {
        let config = StructuredOutputConfig::new(
            "shape",
            json!({
                "type": "object",
                "properties": {
                    "answer": { "type": "string" }
                },
                "required": ["answer"],
                "additionalProperties": false
            }),
        );
        let error = validate_structured_output_payload(&config, &json!({"answer": 42}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Output does not match required schema"));
    }

    #[test]
    fn openai_fallback_includes_requested_structured_output_tool() {
        let registry = structured_output_registry();
        let config = StructuredOutputConfig::new(
            "shape",
            json!({
                "type": "object",
                "properties": {
                    "answer": { "type": "string" }
                },
                "required": ["answer"]
            }),
        );
        let tools = openai_tool_definitions(&registry, Some(&config), false).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, STRUCTURED_OUTPUT_TOOL_ID);
        assert_eq!(tools[0].parameters["required"], json!(["answer"]));
    }

    #[test]
    fn openai_tool_parameters_strip_root_schema_combinators() {
        let registry = ToolRegistry::from_definitions(vec![ToolDefinition {
            id: "WorkflowCreate".to_string(),
            name: "WorkflowCreate".to_string(),
            description: "Create workflow".to_string(),
            handler: "runtime:workflow:workflow_create".to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema {
                properties: BTreeMap::new(),
                raw_json_schema: Some(
                    serde_json::to_string(&json!({
                        "type": "object",
                        "properties": {
                            "action": {
                                "type": "object",
                                "anyOf": [
                                    { "required": ["prompt"] },
                                    { "required": ["command"] }
                                ]
                            },
                            "yaml_action": { "type": "string" }
                        },
                        "required": ["slug"],
                        "anyOf": [
                            { "required": ["action"] },
                            { "required": ["yaml_action"] }
                        ],
                        "additionalProperties": false
                    }))
                    .unwrap(),
                ),
            },
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        }]);

        let responses_tools = openai_tool_definitions(&registry, None, false).unwrap();
        let chat_tools = openai_chat_completion_tools(&registry, None, false).unwrap();
        let parameters = &responses_tools[0].parameters;

        assert_eq!(parameters["type"], "object");
        assert!(parameters.get("anyOf").is_none());
        assert_eq!(
            parameters["properties"]["action"]["anyOf"][0]["required"],
            json!(["prompt"])
        );
        assert_eq!(chat_tools[0].function.parameters, *parameters);
    }

    #[test]
    fn openai_native_path_skips_structured_output_tool() {
        let registry = structured_output_registry();
        let config = StructuredOutputConfig::new("shape", json!({ "type": "object" }));
        let tools = openai_tool_definitions(&registry, Some(&config), true).unwrap();
        assert!(tools.is_empty());
    }

    fn web_search_tool_definition(handler: &str) -> ToolDefinition {
        ToolDefinition {
            id: WEB_SEARCH_TOOL_ID.to_string(),
            name: WEB_SEARCH_TOOL_ID.to_string(),
            description: "Search the web.".to_string(),
            handler: handler.to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        }
    }

    #[test]
    fn provider_web_search_handler_emits_native_responses_tool() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prev_native = std::env::var_os("PUFFER_OPENAI_NATIVE_WEB_SEARCH");
        let prev_mode = std::env::var_os("PUFFER_OPENAI_WEB_SEARCH_MODE");
        std::env::remove_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH");
        std::env::remove_var("PUFFER_OPENAI_WEB_SEARCH_MODE");
        let registry =
            ToolRegistry::from_definitions(vec![web_search_tool_definition("provider:web_search")]);
        let tools = openai_tool_definitions(&registry, None, false).unwrap();
        if let Some(value) = prev_native {
            std::env::set_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH", value);
        }
        if let Some(value) = prev_mode {
            std::env::set_var("PUFFER_OPENAI_WEB_SEARCH_MODE", value);
        }
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].kind, "web_search");
        assert!(tools[0].name.is_empty());
        assert!(tools[0].description.is_empty());
        assert!(tools[0].parameters.is_null());
        let serialized = serde_json::to_value(&tools[0]).unwrap();
        assert_eq!(
            serialized,
            json!({ "type": "web_search", "external_web_access": true })
        );
    }

    #[test]
    fn web_search_mode_env_switches_external_access() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prev_native = std::env::var_os("PUFFER_OPENAI_NATIVE_WEB_SEARCH");
        let prev_mode = std::env::var_os("PUFFER_OPENAI_WEB_SEARCH_MODE");
        std::env::remove_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH");
        std::env::set_var("PUFFER_OPENAI_WEB_SEARCH_MODE", "cached");
        let registry =
            ToolRegistry::from_definitions(vec![web_search_tool_definition("provider:web_search")]);
        let tools = openai_tool_definitions(&registry, None, false).unwrap();
        if let Some(value) = prev_native {
            std::env::set_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH", value);
        }
        match prev_mode {
            Some(value) => std::env::set_var("PUFFER_OPENAI_WEB_SEARCH_MODE", value),
            None => std::env::remove_var("PUFFER_OPENAI_WEB_SEARCH_MODE"),
        }
        let serialized = serde_json::to_value(&tools[0]).unwrap();
        assert_eq!(
            serialized,
            json!({ "type": "web_search", "external_web_access": false })
        );
    }

    #[test]
    fn non_provider_web_search_handler_stays_function_tool() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prev = std::env::var_os("PUFFER_OPENAI_NATIVE_WEB_SEARCH");
        std::env::remove_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH");
        let registry = ToolRegistry::from_definitions(vec![web_search_tool_definition(
            "runtime:workflow:web_search",
        )]);
        let tools = openai_tool_definitions(&registry, None, false).unwrap();
        if let Some(value) = prev {
            std::env::set_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH", value);
        }
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].kind, "function");
        assert_eq!(tools[0].name, WEB_SEARCH_TOOL_ID);
    }

    #[test]
    fn env_opt_out_disables_native_web_search() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let registry =
            ToolRegistry::from_definitions(vec![web_search_tool_definition("provider:web_search")]);
        let prev = std::env::var_os("PUFFER_OPENAI_NATIVE_WEB_SEARCH");
        std::env::set_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH", "0");
        let tools = openai_tool_definitions(&registry, None, false).unwrap();
        match prev {
            Some(value) => std::env::set_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH", value),
            None => std::env::remove_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH"),
        }
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].kind, "function");
        assert_eq!(tools[0].name, WEB_SEARCH_TOOL_ID);
    }

    #[test]
    fn openai_native_responses_format_is_emitted_when_enabled() {
        let config = StructuredOutputConfig::new("shape", json!({ "type": "object" }));
        let text = openai_responses_text_config(Some(&config), true).unwrap();
        let format = text
            .format
            .expect("format must be emitted for structured output");
        assert_eq!(format.kind, "json_schema");
        assert_eq!(format.name, "shape");
        assert!(format.strict);
    }

    #[test]
    fn openai_native_responses_format_is_disabled_when_requested() {
        let config = StructuredOutputConfig::new("shape", json!({ "type": "object" }));
        assert!(openai_responses_text_config(Some(&config), false).is_none());
    }
}
