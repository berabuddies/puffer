use puffer_provider_registry::{
    AuthMode, ModelDescriptor, ModelDiscoveryConfig, ProviderDescriptor, ProviderMediaDescriptor,
    ProviderSource, ProviderSourceKind,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Identifies which layer produced a resource.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Builtin,
    User,
    Workspace,
}

/// Captures provenance for a resource file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceInfo {
    pub path: PathBuf,
    pub kind: SourceKind,
}

impl SourceInfo {
    /// Converts resource provenance into provider-registry provenance.
    pub fn as_provider_source(&self) -> ProviderSource {
        ProviderSource {
            kind: match self.kind {
                SourceKind::Builtin => ProviderSourceKind::ResourcePack,
                SourceKind::User => ProviderSourceKind::UserConfig,
                SourceKind::Workspace => ProviderSourceKind::WorkspaceConfig,
            },
            path: Some(self.path.display().to_string()),
        }
    }
}

/// Wraps a loaded resource with its source metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoadedItem<T> {
    pub value: T,
    pub source_info: SourceInfo,
}

/// Declares a YAML-editable tool specification.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolSpec {
    pub id: String,
    pub name: String,
    pub description: String,
    pub handler: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub handler_args: Vec<String>,
    #[serde(default)]
    pub approval_policy: Option<String>,
    #[serde(default)]
    pub sandbox_policy: Option<String>,
    #[serde(default)]
    pub shared_lib: Option<String>,
    #[serde(default)]
    pub enabled_if: Option<String>,
    #[serde(default)]
    pub input_schema: Option<Value>,
    #[serde(default)]
    pub metadata: ToolMetadataSpec,
    #[serde(default)]
    pub display: ToolDisplaySpec,
}

/// Declares a YAML-editable prompt template.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptTemplate {
    pub id: String,
    pub description: String,
    pub template: String,
    #[serde(default)]
    pub variables: Vec<PromptVariableSpec>,
    #[serde(default, alias = "allowed-tools", alias = "allowedTools")]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub provider_override: Option<String>,
    #[serde(default)]
    pub model_override: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub chained_from: Vec<String>,
    #[serde(default, alias = "forProvider", alias = "for-provider")]
    pub for_provider: Option<String>,
    #[serde(default, alias = "forModel", alias = "for-model")]
    pub for_model: Option<String>,
}

/// Declares a YAML-editable subagent definition.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSpec {
    pub id: String,
    pub description: String,
    pub prompt: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default, alias = "disallowedTools")]
    pub disallowed_tools: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default, alias = "permissionMode")]
    pub permission_mode: Option<String>,
    #[serde(default, alias = "maxTurns")]
    pub max_turns: Option<u32>,
    #[serde(default, alias = "initialPrompt")]
    pub initial_prompt: Option<String>,
    #[serde(default)]
    pub background: bool,
    #[serde(default)]
    pub memory: Option<AgentMemoryScope>,
    #[serde(default, alias = "requiredMcpServers")]
    pub required_mcp_servers: Vec<String>,
    #[serde(default, alias = "mcpServers")]
    pub mcp_servers: Vec<AgentMcpServerSpec>,
    #[serde(default)]
    pub hooks: Vec<HookSpec>,
    #[serde(default)]
    pub isolation: Option<String>,
}

/// Declares the supported persistent memory scopes for one agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMemoryScope {
    User,
    Project,
    Local,
}

/// Declares one MCP server attachment for an agent definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum AgentMcpServerSpec {
    Reference(String),
    Inline(std::collections::BTreeMap<String, McpServerSpec>),
}

impl PromptTemplate {
    /// Renders the prompt template using the provided variables and prompt defaults.
    pub fn render(&self, variables: &std::collections::BTreeMap<String, String>) -> String {
        let mut rendered = self.template.clone();
        for variable in &self.variables {
            let key = format!("${}", variable.name);
            let value = variables
                .get(&variable.name)
                .cloned()
                .or_else(|| variable.default.clone())
                .unwrap_or_default();
            rendered = rendered.replace(&key, &value);
        }
        for (name, value) in variables {
            rendered = rendered.replace(&format!("${name}"), value);
        }
        rendered
    }
}

/// Declares one variable accepted by a prompt template.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptVariableSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
}

/// Carries optional runtime metadata overrides for a declarative tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolMetadataSpec {
    #[serde(default)]
    pub may_spawn_processes: bool,
    #[serde(default)]
    pub may_read_files: bool,
    #[serde(default)]
    pub may_write_files: bool,
}

/// Carries optional display hints for a declarative tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolDisplaySpec {
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub show_in_status: bool,
}

/// Declares a YAML-editable hook specification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookSpec {
    pub id: String,
    pub event: String,
    pub command: String,
}

/// Declares a skill resource loaded from `SKILL.md`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillSpec {
    pub name: String,
    pub description: String,
    pub content: String,
    #[serde(default, alias = "allowed-tools", alias = "allowedTools")]
    pub allowed_tools: Vec<String>,
    #[serde(default, alias = "argument-hint", alias = "argumentHint")]
    pub argument_hint: Option<String>,
    #[serde(default, alias = "arguments", alias = "argumentNames")]
    pub argument_names: Vec<String>,
    #[serde(
        default = "default_user_invocable",
        alias = "user-invocable",
        alias = "userInvocable"
    )]
    pub user_invocable: bool,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub disable_model_invocation: bool,
    #[serde(default, alias = "requires-action", alias = "requiresAction")]
    pub requires_action: bool,
    #[serde(default)]
    pub verification: Option<SkillVerificationSpec>,
}

impl Default for SkillSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            content: String::new(),
            allowed_tools: Vec::new(),
            argument_hint: None,
            argument_names: Vec::new(),
            user_invocable: true,
            model: None,
            effort: None,
            context: None,
            disable_model_invocation: false,
            requires_action: false,
            verification: None,
        }
    }
}

fn default_user_invocable() -> bool {
    true
}

/// Carries source-level verification metadata for a loaded skill.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillVerificationSpec {
    pub system: String,
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub generated_path: Option<String>,
    #[serde(default)]
    pub host_catalogue_path: Option<String>,
    #[serde(default)]
    pub compiler_path: Option<String>,
    #[serde(default)]
    pub host_tool_bindings: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub require_approval: bool,
    #[serde(default)]
    pub tools: Option<usize>,
    #[serde(default)]
    pub actions: Option<usize>,
}

/// Declares a plugin command entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCommandSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// Declares an MCP server manifest or plugin MCP reference.
///
/// `transport` selects the wire protocol used to reach the server:
///
/// * `stdio` (default) — `target` is a shell-words command line, executed
///   as a subprocess; the runner pipes JSON-RPC over stdin/stdout. `env`
///   supplies explicit child environment values. `inherit_env` defaults to
///   true for backwards compatibility; set it to false when a verified
///   skill requires a safe baseline environment plus only explicit env.
///   `timeout` and `connect_timeout` are optional second-based limits.
/// * `http` (alias `streamable-http`) — `target` is the absolute URL of an
///   rmcp streamable-HTTP endpoint. Optional `headers` are sent on every
///   request; values support `${VAR}` / `$VAR` env-var expansion so users
///   can put `Authorization: "Bearer ${GITHUB_TOKEN}"` in YAML without
///   committing the token itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerSpec {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    pub transport: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub description: String,
    /// Extra environment variables supplied to stdio MCP subprocesses.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    /// Whether stdio MCP subprocesses inherit the full Puffer process
    /// environment. Defaults to true to preserve existing user manifests.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub inherit_env: bool,
    /// Per-tool-call timeout in seconds. Defaults to 120 when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// Initial connection and discovery timeout in seconds. Defaults to 60
    /// when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout: Option<u64>,
    /// Extra HTTP headers attached to every request when
    /// `transport == "http"` / `"streamable-http"`. Ignored for other
    /// transports. Values support `${VAR}` env-var expansion.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
    /// OAuth 2.0 configuration for HTTP MCP servers (pass 1.5e).
    ///
    /// When set, the runner will discover OAuth metadata from the server,
    /// dynamically register a client (RFC 7591), drive the
    /// authorization-code-with-PKCE flow on demand, persist tokens under
    /// `<config>/puffer/mcp-tokens/`, and refresh on expiry. Ignored for
    /// non-HTTP transports.
    ///
    /// Three accepted YAML shapes:
    ///
    /// ```yaml
    /// oauth: true
    /// # or
    /// oauth: { scope: "read write", client_name: "puffer" }
    /// # or
    /// oauth: { enabled: true }
    /// ```
    ///
    /// `false` / `null` / absent all mean "no OAuth".
    #[serde(default)]
    pub oauth: Option<McpOAuthSpec>,
}

/// Per-server OAuth 2.0 client configuration.
///
/// All fields are optional so the bare `oauth: true` shape works (rmcp
/// negotiates everything from the server's metadata document and dynamic
/// client registration response).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum McpOAuthSpec {
    /// `oauth: true` / `oauth: false` shorthand.
    Bool(bool),
    /// `oauth: { scope: ..., client_name: ... }` long form.
    Detailed(McpOAuthDetail),
}

/// Long-form OAuth configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpOAuthDetail {
    /// Explicit enable flag (defaults to `true` when the detailed form is
    /// used at all — opting out is `oauth: false`).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Space- or array-separated OAuth scopes to request. Empty means
    /// "use the auth server's default".
    #[serde(default)]
    pub scope: String,
    /// Display name registered with the auth server during dynamic client
    /// registration.
    #[serde(default)]
    pub client_name: String,
}

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

impl McpOAuthSpec {
    /// True when the spec opts the server into OAuth.
    pub fn enabled(&self) -> bool {
        match self {
            McpOAuthSpec::Bool(b) => *b,
            McpOAuthSpec::Detailed(d) => d.enabled,
        }
    }

    /// Scopes to request, parsed from the long-form `scope` string. The
    /// short-form `oauth: true` returns an empty list — rmcp falls back
    /// to the auth server's default scope set in that case.
    pub fn scopes(&self) -> Vec<String> {
        match self {
            McpOAuthSpec::Bool(_) => Vec::new(),
            McpOAuthSpec::Detailed(d) => {
                d.scope.split_whitespace().map(|s| s.to_string()).collect()
            }
        }
    }

    /// Display name to register with the auth server during DCR.
    pub fn client_name(&self) -> Option<String> {
        match self {
            McpOAuthSpec::Bool(_) => None,
            McpOAuthSpec::Detailed(d) => {
                if d.client_name.trim().is_empty() {
                    None
                } else {
                    Some(d.client_name.clone())
                }
            }
        }
    }
}

/// Declares a declarative LSP server integration entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LspServerSpec {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    pub command: String,
    #[serde(default)]
    pub install_hint: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub extension_to_language: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub workspace_folder: Option<String>,
}

/// Declares a declarative plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginSpec {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub commands: Vec<PluginCommandSpec>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub agents: Vec<AgentSpec>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerSpec>,
    #[serde(default)]
    pub lsp_servers: Vec<LspServerSpec>,
}

/// Declares a mascot resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MascotSpec {
    pub id: String,
    pub display_name: String,
    pub introduction: String,
}

/// Declares an IDE integration manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdeSpec {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
}

/// Declares a provider pack loaded from YAML.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderPack {
    pub id: String,
    pub display_name: String,
    pub base_url: String,
    pub default_api: String,
    #[serde(default)]
    pub auth_modes: Vec<AuthMode>,
    #[serde(default)]
    pub headers: indexmap::IndexMap<String, String>,
    #[serde(default)]
    pub query_params: indexmap::IndexMap<String, String>,
    /// Optional override for `/v1/chat/completions`. Used by relays
    /// whose `base_url` already includes a versioned prefix (Zhipu's
    /// `/api/paas/v4`, MiniMax's `/v1` already-suffixed proxies, etc.)
    /// so we don't double up to `/v4/v1/chat/completions`.
    #[serde(default)]
    pub chat_completions_path: Option<String>,
    #[serde(default)]
    pub discovery: Option<ModelDiscoveryConfig>,
    #[serde(default)]
    pub media: Option<ProviderMediaDescriptor>,
    #[serde(default)]
    pub models: Vec<ModelDescriptor>,
}

impl ProviderPack {
    /// Converts the provider pack into a registry descriptor.
    pub fn into_descriptor(self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id,
            display_name: self.display_name,
            base_url: self.base_url,
            default_api: self.default_api,
            auth_modes: self.auth_modes,
            headers: self.headers,
            query_params: self.query_params,
            chat_completions_path: self.chat_completions_path,
            discovery: self.discovery,
            media: self.media,
            models: self.models,
        }
    }
}

/// Holds all loaded resources across bundled, user, and workspace layers.
#[derive(Debug, Clone, Default)]
pub struct LoadedResources {
    pub providers: Vec<LoadedItem<ProviderPack>>,
    pub tools: Vec<LoadedItem<ToolSpec>>,
    pub internal_tools: Vec<LoadedItem<ToolSpec>>,
    pub agents: Vec<LoadedItem<AgentSpec>>,
    pub prompts: Vec<LoadedItem<PromptTemplate>>,
    pub hooks: Vec<LoadedItem<HookSpec>>,
    pub skills: Vec<LoadedItem<SkillSpec>>,
    pub mascots: Vec<LoadedItem<MascotSpec>>,
    pub plugins: Vec<LoadedItem<PluginSpec>>,
    pub mcp_servers: Vec<LoadedItem<McpServerSpec>>,
    pub ides: Vec<LoadedItem<IdeSpec>>,
    pub diagnostics: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_provider_registry::{ControlKind, MediaModelDescriptor, WireType};

    fn axis<'a>(
        model: &'a MediaModelDescriptor,
        axis_id: &str,
    ) -> &'a puffer_provider_registry::Axis {
        model
            .axes
            .iter()
            .find(|axis| axis.id == axis_id)
            .unwrap_or_else(|| panic!("{} should declare axis {axis_id}", model.id))
    }

    fn enum_axis_values<'a>(model: &'a MediaModelDescriptor, axis_id: &str) -> &'a [String] {
        match &axis(model, axis_id).control {
            ControlKind::Enum { values, .. } => values,
            other => panic!("{} axis {axis_id} should be enum, got {other:?}", model.id),
        }
    }

    /// Confirms the bundled `zhipu.yaml` parses as a `ProviderPack`
    /// and that `chat_completions_path` flows from yaml through to
    /// `ProviderDescriptor` so the runtime sees the override. Without
    /// this end-to-end wiring the relay 404s on
    /// `…/api/paas/v4/v1/chat/completions`.
    #[test]
    fn zhipu_yaml_parses_with_chat_completions_path_override() {
        let yaml = include_str!("../../../resources/providers/zhipu.yaml");
        let pack: ProviderPack = serde_yaml::from_str(yaml).expect("zhipu.yaml parses");
        assert_eq!(pack.id, "zhipu");
        assert_eq!(pack.base_url, "https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(
            pack.chat_completions_path.as_deref(),
            Some("/chat/completions")
        );
        let descriptor = pack.into_descriptor();
        assert_eq!(
            descriptor.chat_completions_path.as_deref(),
            Some("/chat/completions"),
            "chat_completions_path must flow through into_descriptor"
        );
    }

    /// Confirms the bundled OpenAI fallback list does not default the desktop
    /// UI to an older model before runtime discovery can refresh availability.
    #[test]
    fn openai_yaml_prefers_current_public_responses_models() {
        let yaml = include_str!("../../../resources/providers/openai.yaml");
        let pack: ProviderPack = serde_yaml::from_str(yaml).expect("openai.yaml parses");
        assert_eq!(pack.id, "openai");
        assert_eq!(
            pack.discovery
                .as_ref()
                .expect("discovery")
                .max_output_tokens,
            128_000
        );
        let first = pack.models.first().expect("at least one fallback model");
        assert_eq!(first.id, "gpt-5.5");
        assert_eq!(first.max_output_tokens, 128_000);
        assert!(first.context_window >= 1_000_000);
        assert!(
            pack.models.iter().any(|model| model.id == "gpt-5.4-nano"),
            "fallback list should include the current nano model"
        );
    }

    /// Confirms OpenAI declares only concrete executable Image API models for
    /// the shared exact media resolver.
    #[test]
    fn openai_yaml_declares_exact_gpt_image_descriptor() {
        let yaml = include_str!("../../../resources/providers/openai.yaml");
        let pack: ProviderPack = serde_yaml::from_str(yaml).expect("openai.yaml parses");
        let descriptor = pack.into_descriptor();

        descriptor
            .validate_media_descriptors()
            .expect("media descriptor validates");
        let image = descriptor
            .media
            .as_ref()
            .and_then(|media| media.image.as_ref())
            .expect("image media descriptor");
        assert_eq!(
            image
                .execution
                .as_ref()
                .map(|execution| execution.path.as_str()),
            Some("/v1/images/generations")
        );
        let gpt_image_1 = image
            .models
            .iter()
            .find(|model| model.id == "gpt-image-1")
            .expect("OpenAI should include GPT Image 1");
        assert!(gpt_image_1
            .operations
            .contains(&puffer_provider_registry::MediaOperation::Generate));
        assert_eq!(gpt_image_1.max_outputs, Some(9));
        assert!(enum_axis_values(gpt_image_1, "mode").contains(&"1K SD".to_string()));
        assert!(enum_axis_values(gpt_image_1, "ratio").contains(&"2:3".to_string()));
        let gpt_image_1_size_map = gpt_image_1
            .media_map
            .as_ref()
            .and_then(|media_map| media_map.size.as_ref())
            .expect("gpt-image-1 should map canonical selections to size");
        assert_eq!(gpt_image_1_size_map.field, "size");
        assert_eq!(
            gpt_image_1_size_map.values["1K SD"]["1:1"].as_deref(),
            Some("1024x1024")
        );
        match &gpt_image_1.variants {
            puffer_provider_registry::Variants::Single(variant) => {
                assert_eq!(
                    variant.base_params.get("quality").map(String::as_str),
                    Some("auto")
                );
                assert_eq!(
                    variant.base_params.get("output_format").map(String::as_str),
                    Some("png")
                );
            }
            other => panic!("gpt-image-1 should use one variant, got {other:?}"),
        }
        let gpt_image_2 = image
            .models
            .iter()
            .find(|model| model.id == "gpt-image-2")
            .expect("OpenAI should include the current GPT Image 2 model");
        let gpt_image_2_size_map = gpt_image_2
            .media_map
            .as_ref()
            .and_then(|media_map| media_map.size.as_ref())
            .expect("gpt-image-2 should map canonical selections to size");
        assert_eq!(
            gpt_image_2_size_map.values["2K HD"]["1:1"].as_deref(),
            Some("2048x2048")
        );
        assert!(!image.models.iter().any(|model| model.id == "auto"));
    }

    /// Confirms provider image model lists include all official static models
    /// that fit the currently implemented media execution adapters.
    #[test]
    fn bundled_static_image_model_lists_cover_official_provider_models() {
        let cases = [
            (
                "xai",
                include_str!("../../../resources/providers/xai.yaml"),
                &["grok-imagine-image-quality", "grok-imagine-image"][..],
            ),
            (
                "minimax-cn",
                include_str!("../../../resources/providers/minimax-cn.yaml"),
                &["image-01", "image-01-live"][..],
            ),
        ];

        for (provider_id, yaml, expected_models) in cases {
            let pack: ProviderPack = serde_yaml::from_str(yaml).expect("provider yaml parses");
            assert_eq!(pack.id, provider_id);
            let descriptor = pack.into_descriptor();
            let image = descriptor
                .media
                .as_ref()
                .and_then(|media| media.image.as_ref())
                .expect("image media descriptor");
            for expected_model in expected_models {
                assert!(
                    image.models.iter().any(|model| model.id == *expected_model),
                    "{provider_id} should include {expected_model}"
                );
            }
        }
    }

    /// Confirms cloud providers with documented synchronous JSON image APIs
    /// declare exact executable descriptors.
    #[test]
    fn bundled_cloud_image_providers_declare_executable_descriptors() {
        let providers = [
            (
                "byteplus",
                include_str!("../../../resources/providers/byteplus.yaml"),
                "seedream-5-0-260128",
                puffer_provider_registry::MediaExecutionKind::ImagesJson,
            ),
            (
                "zhipu",
                include_str!("../../../resources/providers/zhipu.yaml"),
                "glm-image",
                puffer_provider_registry::MediaExecutionKind::ImagesJson,
            ),
            (
                "xai",
                include_str!("../../../resources/providers/xai.yaml"),
                "grok-imagine-image-quality",
                puffer_provider_registry::MediaExecutionKind::ImagesJson,
            ),
            (
                "vercel-ai-gateway",
                include_str!("../../../resources/providers/vercel-ai-gateway.yaml"),
                "openai/gpt-image-2",
                puffer_provider_registry::MediaExecutionKind::ImagesJson,
            ),
            (
                "minimax",
                include_str!("../../../resources/providers/minimax.yaml"),
                "image-01",
                puffer_provider_registry::MediaExecutionKind::MinimaxImage,
            ),
            (
                "minimax-cn",
                include_str!("../../../resources/providers/minimax-cn.yaml"),
                "image-01",
                puffer_provider_registry::MediaExecutionKind::MinimaxImage,
            ),
        ];

        for (provider_id, yaml, expected_model, expected_adapter) in providers {
            let pack: ProviderPack = serde_yaml::from_str(yaml).expect("provider yaml parses");
            assert_eq!(pack.id, provider_id);
            let descriptor = pack.into_descriptor();
            descriptor
                .validate_media_descriptors()
                .expect("media descriptor validates");
            let image = descriptor
                .media
                .as_ref()
                .and_then(|media| media.image.as_ref())
                .expect("image media descriptor");
            let model = image
                .models
                .iter()
                .find(|model| model.id == expected_model)
                .unwrap_or_else(|| panic!("{provider_id} should include {expected_model}"));
            let effective_adapter = model
                .execution
                .as_ref()
                .or(image.execution.as_ref())
                .map(|execution| execution.adapter);
            assert_eq!(effective_adapter, Some(expected_adapter));
            assert!(!image.models.iter().any(|model| model.id == "auto"));
        }
    }

    /// Guards the BytePlus video duration contract end-to-end: the bundled
    /// descriptor must encode `duration` as a JSON number for both Seedance 2.0
    /// models. The fast model rejects a string `duration` with `InvalidParameter`,
    /// so a dropped `wire_type` (defaulting to `String`) silently breaks
    /// submission. See resources/providers/byteplus.yaml.
    #[test]
    fn bundled_byteplus_video_duration_is_numeric() {
        let pack: ProviderPack =
            serde_yaml::from_str(include_str!("../../../resources/providers/byteplus.yaml"))
                .expect("byteplus.yaml parses");
        let descriptor = pack.into_descriptor();
        let video = descriptor
            .media
            .as_ref()
            .and_then(|media| media.video.as_ref())
            .expect("byteplus video media descriptor");

        for model_id in ["dreamina-seedance-2-0", "dreamina-seedance-2-0-fast"] {
            let model = video
                .models
                .iter()
                .find(|model| model.id == model_id)
                .unwrap_or_else(|| panic!("byteplus video should include {model_id}"));
            let duration = axis(model, "duration");
            assert_eq!(duration.request_field.as_deref(), Some("duration"));
            assert_eq!(
                duration.wire_type,
                WireType::Number,
                "{model_id} duration must serialize as a JSON number, not a string",
            );
        }
    }

    /// Confirms trusted router/gateway media discovery is only declared with
    /// executable chat image-output adapters.
    #[test]
    fn bundled_router_gateway_image_discovery_uses_chat_output_adapter() {
        let providers = [
            (
                "openrouter",
                include_str!("../../../resources/providers/openrouter.yaml"),
            ),
            (
                "vercel-ai-gateway",
                include_str!("../../../resources/providers/vercel-ai-gateway.yaml"),
            ),
        ];

        for (provider_id, yaml) in providers {
            let pack: ProviderPack = serde_yaml::from_str(yaml).expect("provider yaml parses");
            assert_eq!(pack.id, provider_id);
            let descriptor = pack.into_descriptor();
            descriptor
                .validate_media_descriptors()
                .expect("media descriptor validates");
            let image = descriptor
                .media
                .as_ref()
                .and_then(|media| media.image.as_ref())
                .expect("image media descriptor");
            assert_eq!(
                image.discovery.as_ref().map(|discovery| discovery.adapter),
                Some(puffer_provider_registry::MediaDiscoveryKind::TrustedImageOutput)
            );
            assert_eq!(
                image.execution.as_ref().map(|execution| execution.adapter),
                Some(puffer_provider_registry::MediaExecutionKind::ChatImageOutput)
            );
        }
    }

    /// Confirms Vercel's static image-only models remain executable through
    /// Images JSON while provider-level discovery uses chat image output.
    #[test]
    fn vercel_static_image_models_override_discovery_execution_adapter() {
        let yaml = include_str!("../../../resources/providers/vercel-ai-gateway.yaml");
        let pack: ProviderPack = serde_yaml::from_str(yaml).expect("vercel yaml parses");
        let descriptor = pack.into_descriptor();
        descriptor
            .validate_media_descriptors()
            .expect("media descriptor validates");
        let image = descriptor
            .media
            .as_ref()
            .and_then(|media| media.image.as_ref())
            .expect("image media descriptor");
        let model = image
            .models
            .iter()
            .find(|model| model.id == "openai/gpt-image-2")
            .expect("vercel static image model");

        assert_eq!(
            model.execution.as_ref().map(|execution| execution.adapter),
            Some(puffer_provider_registry::MediaExecutionKind::ImagesJson)
        );
    }

    /// Confirms Vercel's official image-only model list is mirrored as a
    /// static Images JSON fallback when discovery is unavailable.
    #[test]
    fn vercel_static_image_models_cover_official_gateway_image_type() {
        let yaml = include_str!("../../../resources/providers/vercel-ai-gateway.yaml");
        let pack: ProviderPack = serde_yaml::from_str(yaml).expect("vercel yaml parses");
        let descriptor = pack.into_descriptor();
        let image = descriptor
            .media
            .as_ref()
            .and_then(|media| media.image.as_ref())
            .expect("image media descriptor");
        let expected_models = [
            "bfl/flux-2-flex",
            "bfl/flux-2-klein-4b",
            "bfl/flux-2-klein-9b",
            "bfl/flux-2-max",
            "bfl/flux-2-pro",
            "bfl/flux-kontext-max",
            "bfl/flux-kontext-pro",
            "bfl/flux-pro-1.0-fill",
            "bfl/flux-pro-1.1",
            "bfl/flux-pro-1.1-ultra",
            "bytedance/seedream-4.0",
            "bytedance/seedream-4.5",
            "bytedance/seedream-5.0-lite",
            "google/imagen-4.0-fast-generate-001",
            "google/imagen-4.0-generate-001",
            "google/imagen-4.0-ultra-generate-001",
            "openai/gpt-image-1",
            "openai/gpt-image-1-mini",
            "openai/gpt-image-1.5",
            "openai/gpt-image-2",
            "prodia/flux-fast-schnell",
            "recraft/recraft-v2",
            "recraft/recraft-v3",
            "recraft/recraft-v4",
            "recraft/recraft-v4-pro",
            "recraft/recraft-v4.1",
            "recraft/recraft-v4.1-pro",
            "recraft/recraft-v4.1-utility",
            "recraft/recraft-v4.1-utility-pro",
            "xai/grok-imagine-image",
        ];
        let expected_model_ids: std::collections::BTreeSet<_> =
            expected_models.iter().copied().collect();
        let actual_model_ids: Vec<_> = image.models.iter().map(|model| model.id.as_str()).collect();
        let unique_actual_model_ids: std::collections::BTreeSet<_> =
            actual_model_ids.iter().copied().collect();

        assert_eq!(
            actual_model_ids.len(),
            unique_actual_model_ids.len(),
            "vercel static image fallback models must not contain duplicates"
        );
        assert_eq!(
            unique_actual_model_ids, expected_model_ids,
            "vercel static image fallback must exactly mirror official image-type models"
        );

        for expected_model in expected_models {
            let model = image
                .models
                .iter()
                .find(|model| model.id == expected_model)
                .unwrap_or_else(|| panic!("vercel should include {expected_model}"));
            assert_eq!(
                model.execution.as_ref().map(|execution| execution.adapter),
                Some(puffer_provider_registry::MediaExecutionKind::ImagesJson)
            );
        }
    }

    /// Confirms the bundled provider catalog includes WorldRouter so desktop
    /// provider selection does not depend on workspace-local resource files.
    #[test]
    fn worldrouter_yaml_parses_as_builtin_agent_provider() {
        let yaml = include_str!("../../../resources/providers/worldrouter.yaml");
        let pack: ProviderPack = serde_yaml::from_str(yaml).expect("worldrouter.yaml parses");
        assert_eq!(pack.id, "worldrouter");
        assert_eq!(pack.base_url, "https://inference-api.worldrouter.ai/v1");
        assert_eq!(pack.default_api, "openai-completions");
        assert_eq!(
            pack.chat_completions_path.as_deref(),
            Some("/chat/completions")
        );
        assert!(pack
            .auth_modes
            .iter()
            .any(|mode| matches!(mode, puffer_provider_registry::AuthMode::ApiKey)));
        let descriptor = pack.into_descriptor();
        assert!(
            descriptor.models.iter().any(|model| model.id == "auto"),
            "WorldRouter should expose the auto routing fallback model"
        );
        let image = descriptor
            .media
            .as_ref()
            .and_then(|media| media.image.as_ref())
            .expect("WorldRouter should expose exact image generation models");
        let ids = image
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            ids,
            std::collections::BTreeSet::from([
                "gemini-2.5-flash-image",
                "gemini-3-pro-image-preview",
                "gemini-3.1-flash-image-preview",
                "gpt-image-2",
            ])
        );
    }
}
