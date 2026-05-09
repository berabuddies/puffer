use crate::auth::AuthMode;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Describes the response format used by a provider's model discovery endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelDiscoveryFormat {
    OpenAiModels,
    AnthropicModels,
    OllamaModels,
}

/// Configures runtime discovery for provider-reported models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelDiscoveryConfig {
    pub path: String,
    pub response: ModelDiscoveryFormat,
    pub api: String,
    pub context_window: u32,
    pub max_output_tokens: u32,
    #[serde(default)]
    pub supports_reasoning: bool,
    #[serde(default = "default_items_field")]
    pub items_field: String,
    #[serde(default = "default_id_field")]
    pub id_field: String,
    #[serde(default)]
    pub display_name_field: Option<String>,
    #[serde(default)]
    pub headers: IndexMap<String, String>,
}

/// Describes the origin of a registered provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSourceKind {
    Builtin,
    ResourcePack,
    UserConfig,
    WorkspaceConfig,
}

/// Carries source provenance for a registered provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderSource {
    pub kind: ProviderSourceKind,
    #[serde(default)]
    pub path: Option<String>,
}

/// Describes one provider model exposed to the rest of the application.
///
/// Mirrors pi-mono's `Model<TApi>` shape (see
/// `pi-mono/packages/ai/src/types.ts:426-451`) — `id`, `display_name`,
/// `api`, `context_window`, `max_output_tokens`, `supports_reasoning`,
/// `input` modalities, optional `cost` and `compat`. The fields beyond
/// the original puffer set are all `Option<…>` / serde-default so
/// existing yamls keep parsing without churn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    pub api: String,
    pub context_window: u32,
    pub max_output_tokens: u32,
    #[serde(default)]
    pub supports_reasoning: bool,
    /// Modalities the model accepts as input. Defaults to text-only when
    /// unset — parity with pi-mono's
    /// `input: ("text" | "image")[]` (see `types.ts:433`). Used by the
    /// runtime to decide whether to downgrade image content blocks to
    /// placeholder text before sending to a text-only model.
    #[serde(default = "default_input_modalities")]
    pub input: Vec<Modality>,
    /// Per-million-token pricing. Used by the cost tracker — when set,
    /// `command_summary` reports an actual USD figure instead of the
    /// historical `unavailable` placeholder. Mirrors pi-mono's
    /// `cost: { input, output, cacheRead, cacheWrite }` (see
    /// `types.ts:434-439`).
    #[serde(default)]
    pub cost: Option<ModelCost>,
    /// Optional declarative API-shape compat overrides. When `None` (the
    /// common case), runtime helpers in `puffer-core` auto-detect each
    /// flag from `base_url` / `provider.id` to preserve historical
    /// behavior. Set explicitly for third-party endpoints whose URL
    /// shape doesn't match the heuristic — e.g. an OpenRouter relay
    /// that exposes a `/chat/completions` style API but lives at a
    /// non-`api.openai.com` URL, or a self-hosted Anthropic-compatible
    /// proxy that exposes a `thinking` block. Inspired by pi-mono's
    /// `Model<TApi>.compat?` field.
    #[serde(default)]
    pub compat: Option<ModelCompat>,
}

/// Input modality a model accepts. Defaults to `[Text]` when omitted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Text,
    Image,
}

fn default_input_modalities() -> Vec<Modality> {
    vec![Modality::Text]
}

/// Per-million-token pricing in USD. All four fields are positive
/// floats; zero means "free for this stream."
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ModelCost {
    /// USD per million input (prompt) tokens.
    pub input: f64,
    /// USD per million output (completion) tokens.
    pub output: f64,
    /// USD per million cache-read tokens (Anthropic-style cache hit).
    #[serde(default)]
    pub cache_read: f64,
    /// USD per million cache-write tokens (Anthropic-style ephemeral
    /// or 1-hour cache write).
    #[serde(default)]
    pub cache_write: f64,
}

// ModelCost holds floats so we accept default `Eq` impl loss; provide
// a manual one so the surrounding `ModelDescriptor` keeps `PartialEq`
// clean when callers compare.
impl Eq for ModelCost {}

impl ModelCost {
    /// Computes the total USD cost for the given token mix.
    pub fn total(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
    ) -> f64 {
        let scale = 1.0 / 1_000_000.0;
        (self.input * input_tokens as f64
            + self.output * output_tokens as f64
            + self.cache_read * cache_read_tokens as f64
            + self.cache_write * cache_write_tokens as f64)
            * scale
    }
}

/// API-discriminated declarative compat override. The `api` tag mirrors
/// `ModelDescriptor::api` — runtime helpers should pick the matching
/// variant or fall back to URL-based auto-detection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "api")]
pub enum ModelCompat {
    #[serde(rename = "openai-responses")]
    OpenAiResponses(OpenAiResponsesCompat),
    #[serde(rename = "openai-completions")]
    OpenAiCompletions(OpenAiCompletionsCompat),
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages(AnthropicMessagesCompat),
}

/// Compat flags for OpenAI Responses-shaped providers (the canonical
/// public OpenAI Responses API plus its codex / azure variants).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAiResponsesCompat {
    /// Whether the provider supports server-side response threading via
    /// `previous_response_id`. Auto-detected from
    /// `provider.id == "openai" && base_url.contains("api.openai.com")`
    /// or `base_url.contains("/api/codex")` when not set.
    #[serde(default)]
    pub supports_response_threading: Option<bool>,
    /// HTTP path for the Responses endpoint. Auto-detected:
    /// `/responses` for `/backend-api` or `/api/codex` URLs;
    /// `/v1/responses` everywhere else.
    #[serde(default)]
    pub responses_path: Option<ResponsesPath>,
    /// Whether to inject the codex-compat `version: <const>` header.
    /// Auto-detected from `provider.id == "openai"`.
    #[serde(default)]
    pub send_codex_version_header: Option<bool>,
    /// Whether the provider is "codex-style" (uses the Codex pseudo-API
    /// shape rather than the canonical Responses shape). Auto-detected
    /// from `default_api == "openai-codex-responses"` or
    /// base_url containing `/backend-api` or `/api/codex`.
    #[serde(default)]
    pub codex_style: Option<bool>,
    /// Override base_url under OAuth credentials. When set, OAuth flows
    /// route to this URL instead of `provider.base_url`. The default
    /// `https://chatgpt.com/backend-api/codex` rewrite still applies for
    /// `provider.id == "openai"` when this is `None`.
    #[serde(default)]
    pub oauth_base_url: Option<String>,
}

/// Compat flags for OpenAI Chat-Completions-shaped providers (the
/// canonical OpenAI Chat Completions API plus its many "compatible"
/// relays — groq, cerebras, openrouter, deepseek, zai, qwen, kimi).
///
/// Mirrors pi-mono's `OpenAICompletionsCompat`
/// (`pi-mono/packages/ai/src/types.ts:277`). Each flag is `Option<…>`
/// so existing model yamls keep parsing; runtime helpers fall back to
/// URL-based auto-detection when unset.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAiCompletionsCompat {
    /// Whether replayed assistant messages must include an empty
    /// `reasoning_content` field when reasoning is enabled. DeepSeek V4
    /// rejects multi-turn requests without it. Pi-mono auto-enables this
    /// for any model whose id contains `deepseek-v4` and runs through
    /// `openai-completions`.
    #[serde(default)]
    pub requires_reasoning_content_on_assistant_messages: Option<bool>,
    /// Wire format for reasoning/thinking parameters. Defaults to
    /// `Openai` (top-level `reasoning_effort: "<level>"`). Provider
    /// variants:
    /// - `Openrouter`: `reasoning: { effort: "<level>" }`
    /// - `Deepseek`: `thinking: { type: "enabled" }` plus
    ///   `reasoning_effort`
    /// - `Zai`: top-level `enable_thinking: true`
    /// - `Qwen`: top-level `enable_thinking: true`
    /// - `QwenChatTemplate`: `chat_template_kwargs.enable_thinking: true`
    #[serde(default)]
    pub thinking_format: Option<ThinkingFormat>,
    /// Optional mapping from puffer effort levels (`minimal` / `low` /
    /// `medium` / `high` / `xhigh`) to the vendor-specific string sent
    /// in `reasoning_effort`. Useful for relays that use non-standard
    /// names (e.g. DeepSeek V4 maps `xhigh` → `max`). Mirrors pi-mono's
    /// `reasoningEffortMap`.
    #[serde(default)]
    pub reasoning_effort_map: Option<IndexMap<String, String>>,
    /// Whether the provider supports `reasoning_effort` (or its
    /// thinking-format equivalent) at all. Defaults to true when the
    /// model has `supports_reasoning: true`. Set to `Some(false)` for
    /// reasoning-capable relays whose API does not accept the field.
    #[serde(default)]
    pub supports_reasoning_effort: Option<bool>,
}

/// Wire format for the reasoning/thinking parameter on OpenAI
/// Chat-Completions-shaped requests. Default = `Openai`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingFormat {
    /// `reasoning_effort: "<level>"` at the top level (canonical
    /// OpenAI shape). Default.
    Openai,
    /// `reasoning: { effort: "<level>" }` (OpenRouter).
    Openrouter,
    /// `thinking: { type: "enabled" }` plus `reasoning_effort`
    /// (DeepSeek V4).
    Deepseek,
    /// Top-level `enable_thinking: true` (Z.ai).
    Zai,
    /// Top-level `enable_thinking: true` (Qwen direct API).
    Qwen,
    /// `chat_template_kwargs: { enable_thinking: true }` (Qwen via
    /// vLLM / chat template).
    QwenChatTemplate,
}

/// Compat flags for Anthropic Messages-shaped providers (canonical
/// `api.anthropic.com` plus its self-hosted relays).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnthropicMessagesCompat {
    /// Whether the provider accepts the Anthropic `thinking` block.
    /// Auto-detected from `provider.id == "anthropic"` or
    /// `base_url.contains("anthropic.com")`.
    #[serde(default)]
    pub supports_thinking_api: Option<bool>,
}

/// Wire path for the OpenAI Responses API.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponsesPath {
    /// `/v1/responses` — the canonical public OpenAI endpoint.
    V1Responses,
    /// `/responses` — codex / backend-api style endpoint.
    Responses,
}

impl ModelCompat {
    /// Returns the OpenAI Responses compat block when this variant matches.
    pub fn as_openai_responses(&self) -> Option<&OpenAiResponsesCompat> {
        match self {
            ModelCompat::OpenAiResponses(c) => Some(c),
            _ => None,
        }
    }

    /// Returns the OpenAI Chat Completions compat block when this variant matches.
    pub fn as_openai_completions(&self) -> Option<&OpenAiCompletionsCompat> {
        match self {
            ModelCompat::OpenAiCompletions(c) => Some(c),
            _ => None,
        }
    }

    /// Returns the Anthropic Messages compat block when this variant matches.
    pub fn as_anthropic_messages(&self) -> Option<&AnthropicMessagesCompat> {
        match self {
            ModelCompat::AnthropicMessages(c) => Some(c),
            _ => None,
        }
    }
}

/// Describes one model provider and the models it exposes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: String,
    pub display_name: String,
    pub base_url: String,
    pub default_api: String,
    #[serde(default)]
    pub auth_modes: Vec<AuthMode>,
    #[serde(default)]
    pub headers: IndexMap<String, String>,
    #[serde(default)]
    pub query_params: IndexMap<String, String>,
    /// Override for the OpenAI Chat Completions endpoint path. Set
    /// when the provider's `base_url` already encodes a versioned
    /// prefix (e.g. Zhipu: `https://open.bigmodel.cn/api/paas/v4` +
    /// `chat_completions_path: "/chat/completions"`). When omitted,
    /// the OpenAI provider crate falls back to `/v1/chat/completions`.
    ///
    /// Note: the analogous override for the Responses API lives in the
    /// per-model `OpenAiResponsesCompat::responses_path` enum, since
    /// today only `/v1/responses` and `/responses` are observed in the
    /// wild. If a third variant ever appears, we can promote that field
    /// to a free-form string here too.
    #[serde(default)]
    pub chat_completions_path: Option<String>,
    #[serde(default)]
    pub discovery: Option<ModelDiscoveryConfig>,
    #[serde(default)]
    pub models: Vec<ModelDescriptor>,
}

/// Stores one provider plus its provenance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisteredProvider {
    pub descriptor: ProviderDescriptor,
    pub source: ProviderSource,
}

fn default_items_field() -> String {
    "data".to_string()
}

fn default_id_field() -> String {
    "id".to_string()
}
