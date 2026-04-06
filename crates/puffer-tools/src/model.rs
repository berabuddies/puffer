use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
/// Describes the primitive type for a tool schema node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSchemaType {
    Object,
    String,
    Boolean,
    Integer,
    Array,
}

impl ToolSchemaType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::String => "string",
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Array => "array",
        }
    }
}

/// Defines one named property inside a tool input schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPropertySchema {
    pub value_type: ToolSchemaType,
    pub description: String,
    pub required: bool,
}

/// Describes the top-level structured input expected by a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolInputSchema {
    pub properties: BTreeMap<String, ToolPropertySchema>,
}

impl ToolInputSchema {
    /// Converts the typed schema into a JSON Schema object for model APIs.
    pub fn as_json_schema(&self) -> Value {
        let properties = self
            .properties
            .iter()
            .map(|(name, property)| {
                (
                    name.clone(),
                    json!({
                        "type": property.value_type.as_str(),
                        "description": property.description,
                    }),
                )
            })
            .collect::<serde_json::Map<String, Value>>();
        let required = self
            .properties
            .iter()
            .filter_map(|(name, property)| property.required.then(|| name.clone()))
            .collect::<Vec<_>>();
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }
}

/// Captures practical runtime metadata for a local workspace tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolMetadata {
    pub may_spawn_processes: bool,
    pub may_read_files: bool,
    pub may_write_files: bool,
}

/// Carries optional approval and sandbox hints for a tool definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolPolicyHints {
    pub approval_policy: Option<String>,
    pub sandbox_policy: Option<String>,
}

/// Carries optional presentation hints for a tool definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolDisplayHints {
    pub group: Option<String>,
    pub title: Option<String>,
    pub show_in_status: bool,
}

/// Identifies one built-in local tool implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Custom,
    Bash,
    ReadFile,
    WriteFile,
    ReplaceInFile,
    MovePath,
    RemovePath,
    ListDir,
    SearchText,
}

impl ToolKind {
    /// Returns the built-in tool kind for a handler id when it is supported.
    pub fn from_handler(handler: &str) -> Option<Self> {
        match handler {
            "bash" => Some(Self::Bash),
            "read_file" => Some(Self::ReadFile),
            "write_file" => Some(Self::WriteFile),
            "replace_in_file" => Some(Self::ReplaceInFile),
            "move_path" => Some(Self::MovePath),
            "remove_path" => Some(Self::RemovePath),
            "list_dir" => Some(Self::ListDir),
            "search_text" => Some(Self::SearchText),
            _ => None,
        }
    }

    /// Returns the canonical built-in handler id for the tool kind.
    pub fn handler(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Bash => "bash",
            Self::ReadFile => "read_file",
            Self::WriteFile => "write_file",
            Self::ReplaceInFile => "replace_in_file",
            Self::MovePath => "move_path",
            Self::RemovePath => "remove_path",
            Self::ListDir => "list_dir",
            Self::SearchText => "search_text",
        }
    }

    /// Returns the canonical built-in tool definition for this tool kind.
    pub fn definition(self) -> ToolDefinition {
        builtin_tool_definition(self)
    }
}

/// Full typed tool definition used by the registry and model adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub handler: String,
    pub handler_args: Vec<String>,
    pub kind: ToolKind,
    pub input_schema: ToolInputSchema,
    pub metadata: ToolMetadata,
    pub policy: ToolPolicyHints,
    pub shared_lib: Option<String>,
    pub enabled_if: Option<String>,
    pub display: ToolDisplayHints,
}

impl ToolDefinition {
    /// Converts the tool definition into an Anthropic-compatible tool payload.
    pub fn as_anthropic_tool(&self) -> Value {
        json!({
            "name": self.id,
            "description": self.description,
            "input_schema": self.input_schema.as_json_schema(),
        })
    }
}

/// Typed input for the built-in bash tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BashToolInput {
    pub command: String,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub run_in_background: bool,
    #[serde(default, rename = "dangerouslyDisableSandbox")]
    pub dangerously_disable_sandbox: bool,
}

/// Typed input for the built-in read-file tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadFileToolInput {
    pub path: PathBuf,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Typed input for the built-in write-file tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteFileToolInput {
    pub path: PathBuf,
    pub contents: String,
}

/// Typed input for the built-in replace-in-file tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceInFileToolInput {
    pub path: PathBuf,
    pub old: String,
    pub new: String,
    #[serde(default)]
    pub replace_all: bool,
}

/// Typed input for the built-in move-path tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovePathToolInput {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Typed input for the built-in remove-path tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovePathToolInput {
    pub path: PathBuf,
    #[serde(default)]
    pub recursive: bool,
}

/// Typed input for the built-in directory-listing tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListDirToolInput {
    #[serde(default)]
    pub path: Option<PathBuf>,
}

/// Typed input for the built-in workspace text-search tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchTextToolInput {
    pub query: String,
    #[serde(default)]
    pub path: Option<PathBuf>,
}

/// Tagged tool input accepted by the tool CLI and model tool loops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolInput {
    Bash {
        command: String,
        #[serde(default)]
        timeout: Option<u64>,
        #[serde(default)]
        run_in_background: bool,
        #[serde(default, rename = "dangerouslyDisableSandbox")]
        dangerously_disable_sandbox: bool,
    },
    ReadFile {
        path: PathBuf,
        #[serde(default)]
        offset: Option<usize>,
        #[serde(default)]
        limit: Option<usize>,
    },
    WriteFile {
        path: PathBuf,
        contents: String,
    },
    ReplaceInFile {
        path: PathBuf,
        old: String,
        new: String,
        replace_all: bool,
    },
    MovePath {
        from: PathBuf,
        to: PathBuf,
    },
    RemovePath {
        path: PathBuf,
        recursive: bool,
    },
    ListDir {
        path: Option<PathBuf>,
    },
    SearchText {
        query: String,
        path: Option<PathBuf>,
    },
}

impl ToolInput {
    /// Returns the built-in tool kind represented by this input payload.
    pub fn kind(&self) -> ToolKind {
        match self {
            Self::Bash { .. } => ToolKind::Bash,
            Self::ReadFile { .. } => ToolKind::ReadFile,
            Self::WriteFile { .. } => ToolKind::WriteFile,
            Self::ReplaceInFile { .. } => ToolKind::ReplaceInFile,
            Self::MovePath { .. } => ToolKind::MovePath,
            Self::RemovePath { .. } => ToolKind::RemovePath,
            Self::ListDir { .. } => ToolKind::ListDir,
            Self::SearchText { .. } => ToolKind::SearchText,
        }
    }

    /// Converts the enum into the matching kind plus strongly typed payload.
    pub fn into_kind_payload(self) -> (ToolKind, TypedToolInput) {
        match self {
            Self::Bash {
                command,
                timeout,
                run_in_background,
                dangerously_disable_sandbox,
            } => (
                ToolKind::Bash,
                TypedToolInput::Bash(BashToolInput {
                    command,
                    timeout,
                    run_in_background,
                    dangerously_disable_sandbox,
                }),
            ),
            Self::ReadFile {
                path,
                offset,
                limit,
            } => (
                ToolKind::ReadFile,
                TypedToolInput::ReadFile(ReadFileToolInput {
                    path,
                    offset,
                    limit,
                }),
            ),
            Self::WriteFile { path, contents } => (
                ToolKind::WriteFile,
                TypedToolInput::WriteFile(WriteFileToolInput { path, contents }),
            ),
            Self::ReplaceInFile {
                path,
                old,
                new,
                replace_all,
            } => (
                ToolKind::ReplaceInFile,
                TypedToolInput::ReplaceInFile(ReplaceInFileToolInput {
                    path,
                    old,
                    new,
                    replace_all,
                }),
            ),
            Self::MovePath { from, to } => (
                ToolKind::MovePath,
                TypedToolInput::MovePath(MovePathToolInput { from, to }),
            ),
            Self::RemovePath { path, recursive } => (
                ToolKind::RemovePath,
                TypedToolInput::RemovePath(RemovePathToolInput { path, recursive }),
            ),
            Self::ListDir { path } => (
                ToolKind::ListDir,
                TypedToolInput::ListDir(ListDirToolInput { path }),
            ),
            Self::SearchText { query, path } => (
                ToolKind::SearchText,
                TypedToolInput::SearchText(SearchTextToolInput { query, path }),
            ),
        }
    }
}

/// Strongly typed tool payloads used by local executors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedToolInput {
    Bash(BashToolInput),
    ReadFile(ReadFileToolInput),
    WriteFile(WriteFileToolInput),
    ReplaceInFile(ReplaceInFileToolInput),
    MovePath(MovePathToolInput),
    RemovePath(RemovePathToolInput),
    ListDir(ListDirToolInput),
    SearchText(SearchTextToolInput),
}

/// Captures normalized stdout, stderr, and metadata from one tool run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub stdout: String,
    pub stderr: String,
    pub metadata: Value,
}

/// Stores the normalized result of one local tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecutionResult {
    pub tool_id: String,
    pub success: bool,
    pub output: ToolOutput,
}

/// Returns all built-in tool definitions in stable id order.
pub fn builtin_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        builtin_tool_definition(ToolKind::Bash),
        builtin_tool_definition(ToolKind::ReadFile),
        builtin_tool_definition(ToolKind::WriteFile),
        builtin_tool_definition(ToolKind::ReplaceInFile),
        builtin_tool_definition(ToolKind::MovePath),
        builtin_tool_definition(ToolKind::RemovePath),
        builtin_tool_definition(ToolKind::ListDir),
        builtin_tool_definition(ToolKind::SearchText),
    ]
}

/// Returns the built-in tool definition for the given tool kind.
pub fn builtin_tool_definition(kind: ToolKind) -> ToolDefinition {
    match kind {
        ToolKind::Custom => ToolDefinition {
            id: "custom".to_string(),
            name: "custom".to_string(),
            description: "Run a custom declarative tool.".to_string(),
            handler: kind.handler().to_string(),
            handler_args: Vec::new(),
            kind,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        },
        ToolKind::Bash => ToolDefinition {
            id: "bash".to_string(),
            name: "bash".to_string(),
            description: "Run a shell command in the current workspace.".to_string(),
            handler: kind.handler().to_string(),
            handler_args: Vec::new(),
            kind,
            input_schema: ToolInputSchema {
                properties: BTreeMap::from([
                    (
                        "command".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "Shell command to execute.".to_string(),
                            required: true,
                        },
                    ),
                    (
                        "timeout".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::Integer,
                            description: "Optional timeout in milliseconds.".to_string(),
                            required: false,
                        },
                    ),
                    (
                        "run_in_background".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::Boolean,
                            description: "When true, request background execution.".to_string(),
                            required: false,
                        },
                    ),
                    (
                        "dangerouslyDisableSandbox".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::Boolean,
                            description: "When true, request unsandboxed execution.".to_string(),
                            required: false,
                        },
                    ),
                ]),
            },
            metadata: ToolMetadata {
                may_spawn_processes: true,
                may_read_files: true,
                may_write_files: true,
            },
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        },
        ToolKind::ReadFile => ToolDefinition {
            id: "read_file".to_string(),
            name: "read_file".to_string(),
            description: "Read one file from the current workspace.".to_string(),
            handler: kind.handler().to_string(),
            handler_args: Vec::new(),
            kind,
            input_schema: ToolInputSchema {
                properties: BTreeMap::from([
                    (
                        "path".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "Path to the file to read.".to_string(),
                            required: true,
                        },
                    ),
                    (
                        "offset".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::Integer,
                            description: "Optional 0-indexed line offset.".to_string(),
                            required: false,
                        },
                    ),
                    (
                        "limit".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::Integer,
                            description: "Optional maximum number of lines to return.".to_string(),
                            required: false,
                        },
                    ),
                ]),
            },
            metadata: ToolMetadata {
                may_spawn_processes: false,
                may_read_files: true,
                may_write_files: false,
            },
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        },
        ToolKind::WriteFile => ToolDefinition {
            id: "write_file".to_string(),
            name: "write_file".to_string(),
            description: "Write one file in the current workspace.".to_string(),
            handler: kind.handler().to_string(),
            handler_args: Vec::new(),
            kind,
            input_schema: ToolInputSchema {
                properties: BTreeMap::from([
                    (
                        "path".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "Path to the file to write.".to_string(),
                            required: true,
                        },
                    ),
                    (
                        "contents".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "New file contents.".to_string(),
                            required: true,
                        },
                    ),
                ]),
            },
            metadata: ToolMetadata {
                may_spawn_processes: false,
                may_read_files: false,
                may_write_files: true,
            },
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        },
        ToolKind::ReplaceInFile => ToolDefinition {
            id: "replace_in_file".to_string(),
            name: "replace_in_file".to_string(),
            description: "Replace text inside one workspace file.".to_string(),
            handler: kind.handler().to_string(),
            handler_args: Vec::new(),
            kind,
            input_schema: ToolInputSchema {
                properties: BTreeMap::from([
                    (
                        "path".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "Path to the file to update.".to_string(),
                            required: true,
                        },
                    ),
                    (
                        "old".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "Exact text to replace.".to_string(),
                            required: true,
                        },
                    ),
                    (
                        "new".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "Replacement text.".to_string(),
                            required: true,
                        },
                    ),
                    (
                        "replace_all".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::Boolean,
                            description: "When true, replace every occurrence.".to_string(),
                            required: false,
                        },
                    ),
                ]),
            },
            metadata: ToolMetadata {
                may_spawn_processes: false,
                may_read_files: true,
                may_write_files: true,
            },
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        },
        ToolKind::MovePath => ToolDefinition {
            id: "move_path".to_string(),
            name: "move_path".to_string(),
            description: "Move or rename one workspace file or directory.".to_string(),
            handler: kind.handler().to_string(),
            handler_args: Vec::new(),
            kind,
            input_schema: ToolInputSchema {
                properties: BTreeMap::from([
                    (
                        "from".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "Existing source path.".to_string(),
                            required: true,
                        },
                    ),
                    (
                        "to".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "Destination path.".to_string(),
                            required: true,
                        },
                    ),
                ]),
            },
            metadata: ToolMetadata {
                may_spawn_processes: false,
                may_read_files: false,
                may_write_files: true,
            },
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        },
        ToolKind::RemovePath => ToolDefinition {
            id: "remove_path".to_string(),
            name: "remove_path".to_string(),
            description: "Remove one workspace file or directory.".to_string(),
            handler: kind.handler().to_string(),
            handler_args: Vec::new(),
            kind,
            input_schema: ToolInputSchema {
                properties: BTreeMap::from([
                    (
                        "path".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "Path to remove.".to_string(),
                            required: true,
                        },
                    ),
                    (
                        "recursive".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::Boolean,
                            description: "Set to true when removing directories recursively."
                                .to_string(),
                            required: false,
                        },
                    ),
                ]),
            },
            metadata: ToolMetadata {
                may_spawn_processes: false,
                may_read_files: false,
                may_write_files: true,
            },
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        },
        ToolKind::ListDir => ToolDefinition {
            id: "list_dir".to_string(),
            name: "list_dir".to_string(),
            description: "List files and directories from the current workspace.".to_string(),
            handler: kind.handler().to_string(),
            handler_args: Vec::new(),
            kind,
            input_schema: ToolInputSchema {
                properties: BTreeMap::from([(
                    "path".to_string(),
                    ToolPropertySchema {
                        value_type: ToolSchemaType::String,
                        description: "Optional directory path to inspect.".to_string(),
                        required: false,
                    },
                )]),
            },
            metadata: ToolMetadata {
                may_spawn_processes: false,
                may_read_files: true,
                may_write_files: false,
            },
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        },
        ToolKind::SearchText => ToolDefinition {
            id: "search_text".to_string(),
            name: "search_text".to_string(),
            description: "Search for text in workspace files with ripgrep-style semantics."
                .to_string(),
            handler: kind.handler().to_string(),
            handler_args: Vec::new(),
            kind,
            input_schema: ToolInputSchema {
                properties: BTreeMap::from([
                    (
                        "query".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "Text or regex pattern to search for.".to_string(),
                            required: true,
                        },
                    ),
                    (
                        "path".to_string(),
                        ToolPropertySchema {
                            value_type: ToolSchemaType::String,
                            description: "Optional path scope for the search.".to_string(),
                            required: false,
                        },
                    ),
                ]),
            },
            metadata: ToolMetadata {
                may_spawn_processes: true,
                may_read_files: true,
                may_write_files: false,
            },
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        },
    }
}

/// Returns the built-in tool definition for the given handler id, if available.
pub fn builtin_tool_definition_by_handler(handler: &str) -> Option<ToolDefinition> {
    match handler {
        "bash" => Some(builtin_tool_definition(ToolKind::Bash)),
        "read_file" => Some(builtin_tool_definition(ToolKind::ReadFile)),
        "write_file" => Some(builtin_tool_definition(ToolKind::WriteFile)),
        "replace_in_file" => Some(builtin_tool_definition(ToolKind::ReplaceInFile)),
        "move_path" => Some(builtin_tool_definition(ToolKind::MovePath)),
        "remove_path" => Some(builtin_tool_definition(ToolKind::RemovePath)),
        "list_dir" => Some(builtin_tool_definition(ToolKind::ListDir)),
        "search_text" => Some(builtin_tool_definition(ToolKind::SearchText)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            bash_schema.properties["dangerouslyDisableSandbox"].value_type,
            ToolSchemaType::Boolean
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
    fn serde_bash_input_accepts_timeout_and_sandbox_fields() {
        let input: ToolInput = serde_json::from_value(serde_json::json!({
            "tool": "bash",
            "command": "printf hi",
            "timeout": 2500,
            "run_in_background": false,
            "dangerouslyDisableSandbox": false,
        }))
        .unwrap();
        assert_eq!(
            input,
            ToolInput::Bash {
                command: "printf hi".to_string(),
                timeout: Some(2500),
                run_in_background: false,
                dangerously_disable_sandbox: false,
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
                dangerously_disable_sandbox: false,
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
}
