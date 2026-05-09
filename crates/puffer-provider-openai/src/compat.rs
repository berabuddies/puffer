//! Vendor-specific compatibility quirks for OpenAI-compatible providers.
//!
//! Different "OpenAI-compatible" Chat Completions endpoints differ in subtle
//! and incompatible ways: which field name carries the token cap, how
//! reasoning/thinking is requested, whether tool definitions accept `strict`,
//! and how prompt-cache control is wired up. Rather than scatter
//! `if base_url.contains("...")` checks through `request.rs`, we centralize
//! the per-vendor decisions in [`OpenAICompat`].
//!
//! This module is a Rust port of pi-mono's `OpenAICompletionsCompat` interface
//! and its `detectCompat` helper. References:
//! - struct shape — `pi-mono/packages/ai/src/types.ts:282-339`
//! - auto-detect logic — `pi-mono/packages/ai/src/providers/openai-completions.ts:1034-1088`
//!
//! # Status
//!
//! This is **foundation only** — the struct is defined, auto-detected, and
//! tested, but [`crate::build_chat_completions_request`] does not yet consume
//! it. Follow-up PRs will thread an `OpenAICompat` through
//! `OpenAIRequestConfig` so vendor-specific code paths can be removed.

/// Which JSON field a provider expects for the response-token cap.
///
/// OpenAI's newer chat-completions models require `max_completion_tokens`;
/// almost every Chinese OpenAI-compatible relay (Moonshot Kimi, Chutes,
/// Cloudflare AI Gateway, etc.) still requires the legacy `max_tokens`.
///
/// See pi-mono `openai-completions.ts:1057` & `:1068`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxTokensField {
    /// Legacy `max_tokens` field (most non-OpenAI relays).
    MaxTokens,
    /// New OpenAI `max_completion_tokens` field (default for `api.openai.com`).
    MaxCompletionTokens,
}

/// How a provider expects reasoning / thinking to be requested.
///
/// pi-mono ships six dialects on the wire (see `types.ts:301-302`); we mirror
/// them here plus a `None` sentinel for vendors that don't expose any
/// reasoning controls at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingFormat {
    /// OpenAI: top-level `reasoning_effort: "low" | "medium" | "high"`.
    OpenAI,
    /// OpenRouter: nested `reasoning: { effort: "..." }`.
    OpenRouter,
    /// DeepSeek: `thinking: { type: "enabled" }` plus `reasoning_effort`.
    DeepSeek,
    /// z.ai / Zhipu BigModel: top-level `enable_thinking: bool`.
    Zai,
    /// Qwen via vLLM/SGLang chat template: `chat_template_kwargs.enable_thinking`.
    QwenChatTemplate,
    /// Provider has no reasoning controls (or we couldn't auto-detect).
    None,
}

/// How prompt-cache markers are encoded in the request payload.
///
/// pi-mono only ships `Anthropic`-style cache_control today (used by
/// OpenRouter when relaying to Claude models — `openai-completions.ts:1061`).
/// We additionally model `OpenAI`'s native `prompt_cache_retention` knob and
/// a `None` for providers that don't surface cache control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheControlFormat {
    /// Anthropic-style `cache_control: { type: "ephemeral" }` markers.
    Anthropic,
    /// OpenAI-native `prompt_cache_retention` field.
    OpenAI,
    /// No prompt-cache controls supported.
    None,
}

/// Vendor-specific quirks for an OpenAI-compatible Chat Completions endpoint.
///
/// Captured fields are the subset of pi-mono's `OpenAICompletionsCompat` that
/// the puffer request builder will need first. Additional fields (e.g.
/// `requires_tool_result_name`, `supports_store`, `send_session_affinity_headers`)
/// can be added incrementally as call sites adopt this struct.
///
/// Defaults to OpenAI-strict behavior — see [`OpenAICompat::default`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenAICompat {
    /// Which field name carries the token cap.
    pub max_tokens_field: MaxTokensField,
    /// How reasoning is requested on the wire.
    pub thinking_format: ThinkingFormat,
    /// Whether thinking blocks must be inlined as `<thinking>` text instead
    /// of being sent through the structured reasoning channel.
    /// (pi-mono `types.ts:297-298` — used by some relays that strip
    /// reasoning blobs but echo plain text.)
    pub requires_thinking_as_text: bool,
    /// How prompt-cache markers are written into the body.
    pub cache_control_format: CacheControlFormat,
    /// Whether the provider accepts the `reasoning_effort` knob at all.
    /// (pi-mono `openai-completions.ts:1066` — Grok, z.ai, Moonshot, and
    /// Cloudflare AI Gateway all reject it today.)
    pub supports_reasoning_effort: bool,
    /// Whether the provider honors `tools[].function.strict: true`.
    /// (pi-mono `:1083` — Moonshot and Cloudflare AI Gateway do not.)
    pub supports_strict_mode: bool,
    /// Whether the provider supports `parallel_tool_calls: false` to force
    /// serial tool execution. Most OpenAI-compatible relays accept it; some
    /// Chinese providers ignore or 400 on the field.
    pub supports_parallel_tool_calls: bool,
}

impl Default for OpenAICompat {
    /// Strict OpenAI defaults (`api.openai.com` / GPT-5-class models).
    ///
    /// Mirrors the "everything supported, nothing weird" branch of pi-mono's
    /// `detectCompat` (`openai-completions.ts:1063-1087`).
    fn default() -> Self {
        Self {
            max_tokens_field: MaxTokensField::MaxCompletionTokens,
            thinking_format: ThinkingFormat::OpenAI,
            requires_thinking_as_text: false,
            cache_control_format: CacheControlFormat::OpenAI,
            supports_reasoning_effort: true,
            supports_strict_mode: true,
            supports_parallel_tool_calls: true,
        }
    }
}

impl OpenAICompat {
    /// Auto-detect compat settings from a provider id and base URL.
    ///
    /// `provider_id` takes precedence over URL pattern matching when set —
    /// matching pi-mono's `detectCompat` precedence (`:1035-1041`).
    ///
    /// Recognized signals (port of `openai-completions.ts:1038-1086`):
    ///
    /// | Signal | Mapping |
    /// | --- | --- |
    /// | `openai.com` | strict OpenAI defaults |
    /// | `bigmodel.cn` / `api.z.ai` / provider="zai" | [`ThinkingFormat::Zai`], no `reasoning_effort` |
    /// | `deepseek.com` / provider="deepseek" | [`ThinkingFormat::DeepSeek`] |
    /// | `openrouter.ai` / provider="openrouter" | [`ThinkingFormat::OpenRouter`] |
    /// | `moonshot.cn` / `kimi` / provider="moonshotai" | [`ThinkingFormat::DeepSeek`], `max_tokens`, no strict mode |
    /// | `bigai.cn` / `qwen` | [`ThinkingFormat::QwenChatTemplate`] |
    /// | `minimax.io` / `minimax.cn` | OpenAI defaults |
    /// | unknown | OpenAI defaults |
    pub fn auto_detect(provider_id: &str, base_url: &str) -> Self {
        let provider = provider_id.to_ascii_lowercase();
        let url = base_url.to_ascii_lowercase();

        // Strict OpenAI — early return so later overrides don't trigger on
        // e.g. a custom OpenAI proxy hostname.
        if url.contains("openai.com") && !url.contains("openrouter.ai") {
            return Self::default();
        }

        // z.ai / Zhipu BigModel — pi-mono `:1038`, `:1066`, `:1075`.
        let is_zai = provider == "zai"
            || provider == "zhipu"
            || url.contains("api.z.ai")
            || url.contains("bigmodel.cn");
        if is_zai {
            return Self {
                max_tokens_field: MaxTokensField::MaxTokens,
                thinking_format: ThinkingFormat::Zai,
                requires_thinking_as_text: false,
                cache_control_format: CacheControlFormat::None,
                supports_reasoning_effort: false,
                supports_strict_mode: true,
                supports_parallel_tool_calls: true,
            };
        }

        // Moonshot Kimi — pi-mono `:1039`, `:1057`, `:1066`, `:1083`.
        let is_moonshot = provider == "moonshotai"
            || provider == "moonshotai-cn"
            || provider == "kimi"
            || url.contains("api.moonshot.")
            || url.contains("moonshot.cn")
            || url.contains("kimi.com");
        if is_moonshot {
            return Self {
                max_tokens_field: MaxTokensField::MaxTokens,
                thinking_format: ThinkingFormat::DeepSeek,
                requires_thinking_as_text: false,
                cache_control_format: CacheControlFormat::None,
                supports_reasoning_effort: false,
                supports_strict_mode: false,
                supports_parallel_tool_calls: true,
            };
        }

        // DeepSeek — pi-mono `:1060`, `:1072-1074`.
        let is_deepseek = provider == "deepseek" || url.contains("deepseek.com");
        if is_deepseek {
            return Self {
                max_tokens_field: MaxTokensField::MaxCompletionTokens,
                thinking_format: ThinkingFormat::DeepSeek,
                requires_thinking_as_text: false,
                cache_control_format: CacheControlFormat::None,
                supports_reasoning_effort: true,
                supports_strict_mode: true,
                supports_parallel_tool_calls: true,
            };
        }

        // OpenRouter — pi-mono `:1077-1079`. Cache control is Anthropic-style
        // only when relaying to `anthropic/*` models; we conservatively pick
        // Anthropic here and let callers override per-model.
        let is_openrouter = provider == "openrouter" || url.contains("openrouter.ai");
        if is_openrouter {
            return Self {
                max_tokens_field: MaxTokensField::MaxCompletionTokens,
                thinking_format: ThinkingFormat::OpenRouter,
                requires_thinking_as_text: false,
                cache_control_format: CacheControlFormat::Anthropic,
                supports_reasoning_effort: true,
                supports_strict_mode: true,
                supports_parallel_tool_calls: true,
            };
        }

        // Qwen / bigai.cn — pi-mono `types.ts:301-302` ("qwen-chat-template").
        // The vLLM/SGLang chat-template path uses `chat_template_kwargs`.
        let is_qwen = provider == "qwen"
            || provider.starts_with("qwen-")
            || url.contains("bigai.cn")
            || url.contains("dashscope.")
            || url.contains("qwen.");
        if is_qwen {
            return Self {
                max_tokens_field: MaxTokensField::MaxTokens,
                thinking_format: ThinkingFormat::QwenChatTemplate,
                requires_thinking_as_text: false,
                cache_control_format: CacheControlFormat::None,
                supports_reasoning_effort: false,
                supports_strict_mode: true,
                supports_parallel_tool_calls: true,
            };
        }

        // MiniMax — listed for completeness; behaves like OpenAI defaults
        // but uses `max_tokens` (legacy field).
        let is_minimax = provider == "minimax"
            || url.contains("minimax.io")
            || url.contains("minimax.cn")
            || url.contains("minimaxi.");
        if is_minimax {
            return Self {
                max_tokens_field: MaxTokensField::MaxTokens,
                ..Self::default()
            };
        }

        // Unknown vendor — assume OpenAI-strict defaults.
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_returns_openai_compatible_struct() {
        let c = OpenAICompat::default();
        assert_eq!(c.max_tokens_field, MaxTokensField::MaxCompletionTokens);
        assert_eq!(c.thinking_format, ThinkingFormat::OpenAI);
        assert!(!c.requires_thinking_as_text);
        assert_eq!(c.cache_control_format, CacheControlFormat::OpenAI);
        assert!(c.supports_reasoning_effort);
        assert!(c.supports_strict_mode);
        assert!(c.supports_parallel_tool_calls);
    }

    #[test]
    fn auto_detect_openai_returns_openai_defaults() {
        let c = OpenAICompat::auto_detect("openai", "https://api.openai.com/v1");
        assert_eq!(c, OpenAICompat::default());
    }

    #[test]
    fn auto_detect_zhipu_returns_zai_thinking_format() {
        // provider=zai
        let c = OpenAICompat::auto_detect("zai", "https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(c.thinking_format, ThinkingFormat::Zai);
        assert_eq!(c.max_tokens_field, MaxTokensField::MaxTokens);
        assert!(!c.supports_reasoning_effort);

        // bare bigmodel.cn URL (no provider hint) should still match
        let c2 = OpenAICompat::auto_detect("", "https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(c2.thinking_format, ThinkingFormat::Zai);

        // api.z.ai alias
        let c3 = OpenAICompat::auto_detect("", "https://api.z.ai/api/paas/v4");
        assert_eq!(c3.thinking_format, ThinkingFormat::Zai);
    }

    #[test]
    fn auto_detect_openrouter_returns_openrouter_format() {
        let c = OpenAICompat::auto_detect("openrouter", "https://openrouter.ai/api/v1");
        assert_eq!(c.thinking_format, ThinkingFormat::OpenRouter);
        assert_eq!(c.max_tokens_field, MaxTokensField::MaxCompletionTokens);
        assert_eq!(c.cache_control_format, CacheControlFormat::Anthropic);
    }

    #[test]
    fn auto_detect_deepseek_returns_deepseek_format() {
        let c = OpenAICompat::auto_detect("deepseek", "https://api.deepseek.com/v1");
        assert_eq!(c.thinking_format, ThinkingFormat::DeepSeek);
        assert!(c.supports_reasoning_effort);
    }

    #[test]
    fn auto_detect_moonshot_returns_deepseek_hint_and_max_tokens() {
        let c = OpenAICompat::auto_detect("moonshotai", "https://api.moonshot.cn/v1");
        assert_eq!(c.thinking_format, ThinkingFormat::DeepSeek);
        assert_eq!(c.max_tokens_field, MaxTokensField::MaxTokens);
        assert!(!c.supports_reasoning_effort);
        assert!(!c.supports_strict_mode);

        // kimi.com alias
        let c2 = OpenAICompat::auto_detect("kimi", "https://api.kimi.com/v1");
        assert_eq!(c2.thinking_format, ThinkingFormat::DeepSeek);
        assert!(!c2.supports_strict_mode);
    }

    #[test]
    fn auto_detect_qwen_returns_chat_template_format() {
        let c = OpenAICompat::auto_detect("qwen", "https://dashscope.aliyuncs.com/v1");
        assert_eq!(c.thinking_format, ThinkingFormat::QwenChatTemplate);
        assert_eq!(c.max_tokens_field, MaxTokensField::MaxTokens);

        let c2 = OpenAICompat::auto_detect("", "https://api.bigai.cn/v1");
        assert_eq!(c2.thinking_format, ThinkingFormat::QwenChatTemplate);
    }

    #[test]
    fn auto_detect_minimax_uses_max_tokens_legacy_field() {
        let c = OpenAICompat::auto_detect("minimax", "https://api.minimax.io/v1");
        assert_eq!(c.max_tokens_field, MaxTokensField::MaxTokens);
        assert_eq!(c.thinking_format, ThinkingFormat::OpenAI);

        let c2 = OpenAICompat::auto_detect("", "https://api.minimax.cn/v1");
        assert_eq!(c2.max_tokens_field, MaxTokensField::MaxTokens);
    }

    #[test]
    fn unknown_base_url_falls_back_to_default() {
        let c = OpenAICompat::auto_detect("some-new-vendor", "https://api.example.test/v1");
        assert_eq!(c, OpenAICompat::default());
    }

    #[test]
    fn provider_id_is_case_insensitive() {
        let c = OpenAICompat::auto_detect("OpenRouter", "https://OPENROUTER.AI/api/v1");
        assert_eq!(c.thinking_format, ThinkingFormat::OpenRouter);
    }
}
