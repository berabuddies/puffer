use crate::auth::AuthMode;
use crate::CANONICAL_MEDIA_RATIOS;
use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_MEDIA_OUTPUTS: u8 = 9;
const RESERVED_MODE_AXIS_ID: &str = "mode";
const RESERVED_RATIO_AXIS_ID: &str = "ratio";

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

/// Describes provider-scoped media capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProviderMediaDescriptor {
    #[serde(default)]
    pub image: Option<MediaKindDescriptor>,
    #[serde(default)]
    pub video: Option<MediaKindDescriptor>,
}

/// Describes media-generation capability metadata for one provider/kind.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MediaKindDescriptor {
    #[serde(default)]
    pub discovery: Option<MediaDiscoveryDescriptor>,
    #[serde(default)]
    pub execution: Option<MediaExecutionDescriptor>,
    #[serde(default)]
    pub models: Vec<MediaModelDescriptor>,
}

/// Describes how image model candidates may be discovered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaDiscoveryDescriptor {
    #[serde(default = "default_media_discovery_kind")]
    pub adapter: MediaDiscoveryKind,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub query: BTreeMap<String, String>,
}

/// Describes the API-shape adapter used for image execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MediaExecutionDescriptor {
    pub adapter: MediaExecutionKind,
    #[serde(default)]
    pub base_url: Option<String>,
    pub path: String,
    /// Describes how requested image counts are split into provider calls.
    #[serde(default)]
    pub batch: MediaBatchDescriptor,
    /// Controls how the prompt is serialized in video generation requests.
    #[serde(default)]
    pub prompt_format: VideoPromptFormat,
}

/// Describes how an image execution endpoint handles multi-image requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MediaBatchDescriptor {
    #[serde(default = "default_media_batch_mode")]
    pub mode: MediaBatchMode,
    #[serde(default)]
    pub max_images_per_call: Option<u8>,
}

impl Default for MediaBatchDescriptor {
    fn default() -> Self {
        Self {
            mode: MediaBatchMode::PerImage,
            max_images_per_call: None,
        }
    }
}

/// Describes supported image batch execution policies.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaBatchMode {
    PerImage,
    Exact,
}

/// Controls how the text prompt is serialized in the video generation request body.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VideoPromptFormat {
    /// Sends `"prompt": "<text>"` with `"n": 1` — default for Relaydance and most providers.
    #[default]
    Prompt,
    /// Sends `"content": [{"type": "text", "text": "<text>"}]` — required by WorldRouter.
    ContentArray,
}

/// Describes one logical media model: its user-facing axes and the concrete
/// upstream variant(s) those axes resolve to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaModelDescriptor {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub max_outputs: Option<u8>,
    /// Adapter source, inherently shared by this model's variants.
    #[serde(default)]
    pub execution: Option<MediaExecutionDescriptor>,
    #[serde(default)]
    pub operations: Vec<MediaOperation>,
    #[serde(default)]
    pub axes: Vec<crate::Axis>,
    #[serde(default)]
    pub media_map: Option<crate::MediaMap>,
    pub variants: crate::Variants,
}

/// Describes currently implemented media discovery adapters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaDiscoveryKind {
    Static,
    TrustedImageOutput,
}

/// Describes currently implemented media execution adapters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaExecutionKind {
    ImagesJson,
    GeminiGenerateContent,
    ChatImageOutput,
    MinimaxImage,
    ReplicateVideo,
    RelaydanceVideo,
    #[serde(rename = "byteplus_video")]
    BytePlusVideo,
    #[serde(rename = "worldrouter_video")]
    WorldRouterVideo,
}

/// Describes image media operations.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaOperation {
    Generate,
}

/// Describes one model provider and the models it exposes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    pub media: Option<ProviderMediaDescriptor>,
    #[serde(default)]
    pub models: Vec<ModelDescriptor>,
}

impl ProviderDescriptor {
    /// Validates declared provider media descriptors without requiring execution availability.
    pub fn validate_media_descriptors(&self) -> Result<()> {
        let Some(media) = &self.media else {
            return Ok(());
        };

        let mut errors = Vec::new();
        if let Some(image) = &media.image {
            validate_media_kind_descriptor("media.image", image, &mut errors);
        }
        if let Some(video) = &media.video {
            validate_media_kind_descriptor("media.video", video, &mut errors);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            bail!("{}", errors.join("; "))
        }
    }
}

fn validate_media_kind_descriptor(
    location: &str,
    descriptor: &MediaKindDescriptor,
    errors: &mut Vec<String>,
) {
    if let Some(execution) = &descriptor.execution {
        execution.validate(&format!("{location}.execution"), errors);
    }
    for (index, model) in descriptor.models.iter().enumerate() {
        let model_location = format!("{location}.models[{index}]");
        model.validate(&model_location, errors);
    }
}

impl MediaExecutionDescriptor {
    fn validate(&self, location: &str, errors: &mut Vec<String>) {
        if self
            .base_url
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            errors.push(format!("{location}.base_url must not be empty"));
        }
        if self.path.trim().is_empty() {
            errors.push(format!("{location}.path must not be empty"));
        }
        match self.batch.mode {
            MediaBatchMode::PerImage => {
                if self.batch.max_images_per_call.is_some() {
                    errors.push(format!(
                        "{location}.batch.max_images_per_call is only valid when batch.mode is exact"
                    ));
                }
            }
            MediaBatchMode::Exact => match self.batch.max_images_per_call {
                Some(limit) if limit >= 2 => {}
                _ => errors.push(format!(
                    "{location}.batch.max_images_per_call must be at least 2 when batch.mode is exact"
                )),
            },
        }
    }
}

impl MediaModelDescriptor {
    fn validate(&self, location: &str, errors: &mut Vec<String>) {
        let id = self.id.trim();
        if id.is_empty() {
            errors.push(format!("{location}.id must not be empty"));
        } else if id.eq_ignore_ascii_case("auto") {
            errors.push(format!("{location}.id must be a concrete image model id"));
        } else if has_wildcard_or_regex_marker(id) {
            errors.push(format!(
                "{location}.id must not contain wildcard or regex markers"
            ));
        }

        if self.operations.is_empty() {
            errors.push(format!(
                "{location}.operations must include at least one operation"
            ));
        }

        if self
            .max_outputs
            .is_some_and(|max| max == 0 || max > MAX_MEDIA_OUTPUTS)
        {
            errors.push(format!(
                "{location}.max_outputs must be between 1 and {MAX_MEDIA_OUTPUTS}"
            ));
        }

        if let Some(execution) = &self.execution {
            execution.validate(&format!("{location}.execution"), errors);
        }

        if let Err(error) = validate_one_media_model(self) {
            errors.push(format!("{location}: {error}"));
        }
    }
}

/// Enforces the typed-axis/variant invariants for a single logical model:
/// at most one selector axis, param axes carry a `request_field`, ranges are
/// well-formed, and every selector value maps to exactly one variant.
fn validate_one_media_model(model: &MediaModelDescriptor) -> Result<()> {
    use crate::{AxisRole, ControlKind, Variants};
    let selectors: Vec<&crate::Axis> = model
        .axes
        .iter()
        .filter(|a| a.role == AxisRole::Selector)
        .collect();
    if selectors.len() > 1 {
        bail!("media model {} has more than one selector axis", model.id);
    }
    for axis in &model.axes {
        if axis.role == AxisRole::Param
            && axis.request_field.is_none()
            && !canonical_axis_is_covered_by_media_map(axis, model.media_map.as_ref())
        {
            bail!(
                "media model {} param axis {} needs a request_field",
                model.id,
                axis.id
            );
        }
        if axis.id == RESERVED_RATIO_AXIS_ID {
            validate_ratio_axis_values(&model.id, axis)?;
        }
        if let ControlKind::Range {
            min,
            max,
            step,
            default,
        } = &axis.control
        {
            if !(max > min && *step > 0.0) || default < min || default > max {
                bail!(
                    "media model {} axis {} has an invalid range",
                    model.id,
                    axis.id
                );
            }
        }
    }
    validate_media_map(&model.id, &model.axes, model.media_map.as_ref())?;
    if let Variants::BySelector { selector, map } = &model.variants {
        let axis = selectors
            .first()
            .filter(|a| &a.id == selector)
            .with_context(|| {
                format!(
                    "media model {} selector {selector} has no matching selector axis",
                    model.id
                )
            })?;
        match &axis.control {
            ControlKind::Enum { values, .. } => {
                for v in values {
                    if !map.contains_key(v) {
                        bail!("media model {} selector value {v} has no variant", model.id);
                    }
                }
                for k in map.keys() {
                    if !values.contains(k) {
                        bail!(
                            "media model {} variant key {k} is not a selector value",
                            model.id
                        );
                    }
                }
            }
            ControlKind::Bool { .. } => {
                for k in map.keys() {
                    if k != "true" && k != "false" {
                        bail!(
                            "media model {} bool selector variant key {k} must be true/false",
                            model.id
                        );
                    }
                }
                for expected in ["true", "false"] {
                    if !map.contains_key(expected) {
                        bail!(
                            "media model {} bool selector is missing the {expected} variant",
                            model.id
                        );
                    }
                }
            }
            ControlKind::Range { .. } => {
                bail!(
                    "media model {} selector axis {selector} must be enum or bool, not range",
                    model.id
                );
            }
        }
    } else if !selectors.is_empty() {
        bail!(
            "media model {} has a selector axis but a Single variant",
            model.id
        );
    }
    Ok(())
}

fn canonical_axis_is_covered_by_media_map(
    axis: &crate::Axis,
    media_map: Option<&crate::MediaMap>,
) -> bool {
    let Some(media_map) = media_map else {
        return false;
    };
    match axis.id.as_str() {
        RESERVED_MODE_AXIS_ID => media_map.size.is_some(),
        RESERVED_RATIO_AXIS_ID => media_map.ratio.is_some() || media_map.size.is_some(),
        _ => false,
    }
}

fn validate_ratio_axis_values(model_id: &str, axis: &crate::Axis) -> Result<()> {
    let crate::ControlKind::Enum { values, .. } = &axis.control else {
        return Ok(());
    };
    for value in values {
        validate_canonical_ratio(model_id, value)?;
    }
    Ok(())
}

fn validate_media_map(
    model_id: &str,
    axes: &[crate::Axis],
    media_map: Option<&crate::MediaMap>,
) -> Result<()> {
    let Some(media_map) = media_map else {
        return Ok(());
    };
    if let Some(ratio_map) = &media_map.ratio {
        let ratio_values =
            require_enum_axis_values(model_id, axes, RESERVED_RATIO_AXIS_ID, "media_map.ratio")?;
        validate_media_map_field(model_id, "ratio", &ratio_map.field)?;
        if ratio_map.values.is_empty() {
            bail!("media model {model_id} media_map.ratio needs at least one value");
        }
        for ratio in ratio_map.values.keys() {
            validate_canonical_ratio(model_id, ratio)?;
            validate_axis_contains_value(model_id, "ratio", ratio, ratio_values)?;
        }
    }
    if let Some(size_map) = &media_map.size {
        let mode_values =
            require_enum_axis_values(model_id, axes, RESERVED_MODE_AXIS_ID, "media_map.size")?;
        let ratio_values =
            require_enum_axis_values(model_id, axes, RESERVED_RATIO_AXIS_ID, "media_map.size")?;
        validate_media_map_field(model_id, "size", &size_map.field)?;
        if size_map.values.is_empty() {
            bail!("media model {model_id} media_map.size needs at least one mode");
        }
        for (mode, ratios) in &size_map.values {
            if mode.trim().is_empty() {
                bail!("media model {model_id} media_map.size mode must not be empty");
            }
            validate_axis_contains_value(model_id, "mode", mode, mode_values)?;
            if ratios.is_empty() {
                bail!("media model {model_id} media_map.size mode {mode} needs ratios");
            }
            for ratio in ratios.keys() {
                validate_canonical_ratio(model_id, ratio)?;
                validate_axis_contains_value(model_id, "ratio", ratio, ratio_values)?;
            }
        }
        if size_map.common_ratios().is_empty() {
            bail!(
                "media model {model_id} media_map.size needs at least one common ratio across modes"
            );
        }
    }
    Ok(())
}

fn require_enum_axis_values<'a>(
    model_id: &str,
    axes: &'a [crate::Axis],
    axis_id: &str,
    map_name: &str,
) -> Result<&'a Vec<String>> {
    enum_axis_values(axes, axis_id).ok_or_else(|| {
        anyhow::anyhow!("media model {model_id} {map_name} requires an enum {axis_id} axis")
    })
}

fn validate_media_map_field(model_id: &str, map_name: &str, field: &str) -> Result<()> {
    if field.trim().is_empty() {
        bail!("media model {model_id} media_map.{map_name}.field must not be empty");
    }
    Ok(())
}

fn enum_axis_values<'a>(axes: &'a [crate::Axis], axis_id: &str) -> Option<&'a Vec<String>> {
    axes.iter()
        .find(|axis| axis.id == axis_id)
        .and_then(|axis| match &axis.control {
            crate::ControlKind::Enum { values, .. } => Some(values),
            _ => None,
        })
}

fn validate_axis_contains_value(
    model_id: &str,
    axis_id: &str,
    value: &str,
    axis_values: &[String],
) -> Result<()> {
    if !axis_values.iter().any(|axis_value| axis_value == value) {
        bail!("media model {model_id} media_map {axis_id} value {value} is not declared");
    }
    Ok(())
}

fn validate_canonical_ratio(model_id: &str, ratio: &str) -> Result<()> {
    if !CANONICAL_MEDIA_RATIOS.contains(&ratio) {
        bail!("media model {model_id} ratio {ratio} is not a canonical ratio");
    }
    Ok(())
}

fn has_wildcard_or_regex_marker(value: &str) -> bool {
    value.chars().any(|ch| {
        matches!(
            ch,
            '*' | '?' | '[' | ']' | '(' | ')' | '{' | '}' | '|' | '^' | '$' | '\\'
        )
    })
}

/// Stores one provider plus its provenance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

fn default_media_discovery_kind() -> MediaDiscoveryKind {
    MediaDiscoveryKind::Static
}

fn default_media_batch_mode() -> MediaBatchMode {
    MediaBatchMode::PerImage
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
