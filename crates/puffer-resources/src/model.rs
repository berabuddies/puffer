use puffer_provider_registry::{
    AuthMode, ModelDescriptor, ModelDiscoveryConfig, ProviderDescriptor, ProviderSource,
    ProviderSourceKind,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
        }
    }
}

fn default_user_invocable() -> bool {
    true
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
///   as a subprocess; the runner pipes JSON-RPC over stdin/stdout.
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
    /// Extra HTTP headers attached to every request when
    /// `transport == "http"` / `"streamable-http"`. Ignored for other
    /// transports. Values support `${VAR}` env-var expansion.
    #[serde(default)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
            models: self.models,
        }
    }
}

/// Holds all loaded resources across bundled, user, and workspace layers.
#[derive(Debug, Clone, Default)]
pub struct LoadedResources {
    pub providers: Vec<LoadedItem<ProviderPack>>,
    pub tools: Vec<LoadedItem<ToolSpec>>,
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
        assert_eq!(pack.discovery.as_ref().expect("discovery").max_output_tokens, 128_000);
        let first = pack.models.first().expect("at least one fallback model");
        assert_eq!(first.id, "gpt-5.5");
        assert_eq!(first.max_output_tokens, 128_000);
        assert!(first.context_window >= 1_000_000);
        assert!(
            pack.models.iter().any(|model| model.id == "gpt-5.4-nano"),
            "fallback list should include the current nano model"
        );
    }
}
