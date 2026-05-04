//! Attribute helpers — typed wrappers around the `gen_ai.*` /
//! `langfuse.*` / `puffer.*` namespaces so callers can't accidentally
//! drift attribute names.
//!
//! See `docs/observability/langfuse-design.md` § Attribute schema.

use opentelemetry::KeyValue;

/// `gen_ai.system` — the LLM vendor family. Keep in sync with the
/// values Langfuse / Phoenix / Arize-Phoenix recognize.
#[derive(Debug, Clone, Copy)]
pub enum GenAiSystem {
    OpenAi,
    Anthropic,
    Kimi,
    Xai,
    Other,
}

impl GenAiSystem {
    /// Stable lowercase identifier used as the `gen_ai.system`
    /// attribute on provider call spans. Stable across renames so
    /// dashboards don't drift.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Kimi => "kimi",
            Self::Xai => "xai",
            Self::Other => "other",
        }
    }

    /// Map a puffer provider id (`hanbbq`, `kimi-coding`, ...) into a
    /// system family. Unknown providers fall back to `Other`.
    pub fn from_provider(provider_id: &str) -> Self {
        let lower = provider_id.to_ascii_lowercase();
        if lower.contains("anthropic") || lower.contains("kimi-coding") {
            Self::Anthropic
        } else if lower.contains("kimi") {
            Self::Kimi
        } else if lower.contains("xai") || lower.contains("grok") {
            Self::Xai
        } else if lower.contains("openai") || lower.contains("gpt") || lower.contains("hanbbq") {
            Self::OpenAi
        } else {
            Self::Other
        }
    }
}

/// Discriminator used by Langfuse to render an observation as a
/// generation (LLM call) vs span (everything else).
#[derive(Debug, Clone, Copy)]
pub enum ObservationKind {
    AgentLoop,
    Turn,
    ProviderCall, // -> Langfuse generation
    Tool,         // -> Langfuse span
    Compaction,   // -> Langfuse span
    Reflection,   // -> Langfuse span
}

impl ObservationKind {
    /// Span name (used by OTel + Langfuse UI).
    pub fn span_name(&self) -> &'static str {
        match self {
            Self::AgentLoop => "agent_loop",
            Self::Turn => "turn",
            Self::ProviderCall => "provider_call",
            Self::Tool => "tool",
            // Outer wrapper spans (decision logic). Always emitted so
            // a viewer can see *that* a check ran and what it decided.
            // The actual LLM round-trip — when it fires — is a CHILD
            // `subagent.<kind>` GENERATION span underneath.
            Self::Compaction => "compaction",
            Self::Reflection => "reflection",
        }
    }

    /// Langfuse-specific attribute that picks generation vs span.
    pub fn langfuse_observation_type(&self) -> &'static str {
        match self {
            Self::ProviderCall => "generation",
            _ => "span",
        }
    }
}

// ---------------------------------------------------------------------
// Puffer-specific attribute keys (kept as typed `&'static str` so
// callers reference them via constants, not strings).
// ---------------------------------------------------------------------

pub const PUFFER_SESSION_ID: &str = "puffer.session.id";
pub const PUFFER_CWD: &str = "puffer.cwd";
pub const PUFFER_PROVIDER_ID: &str = "puffer.provider.id";
pub const PUFFER_API: &str = "puffer.api";
pub const PUFFER_TOOL_CALL_ID: &str = "puffer.tool.call_id";
pub const PUFFER_TOOL_PARALLEL: &str = "puffer.tool.parallel";

pub const LANGFUSE_OBSERVATION_TYPE: &str = "langfuse.observation.type";
pub const LANGFUSE_OBSERVATION_INPUT: &str = "langfuse.observation.input";
pub const LANGFUSE_OBSERVATION_OUTPUT: &str = "langfuse.observation.output";
/// Severity per Langfuse: DEBUG / DEFAULT / WARNING / ERROR. Used on
/// observations to surface error/warn rows in the Tracing UI.
pub const LANGFUSE_OBSERVATION_LEVEL: &str = "langfuse.observation.level";
pub const LANGFUSE_OBSERVATION_STATUS_MESSAGE: &str = "langfuse.observation.status_message";
pub const LANGFUSE_SESSION_ID: &str = "langfuse.session.id";
pub const LANGFUSE_TRACE_INPUT: &str = "langfuse.trace.input";
pub const LANGFUSE_TRACE_OUTPUT: &str = "langfuse.trace.output";
pub const LANGFUSE_TRACE_TAGS: &str = "langfuse.trace.tags";
pub const LANGFUSE_USER_ID: &str = "langfuse.user.id";
/// Per-trace deployment environment (e.g. "dev", "prod"). Maps to
/// the Langfuse Environment column in the Tracing list.
pub const LANGFUSE_ENVIRONMENT: &str = "langfuse.environment";

/// gen_ai request parameters per OTel GenAI semconv. Surface model
/// invocation settings on the provider_call generation observation.
pub const GEN_AI_REQUEST_MAX_TOKENS: &str = "gen_ai.request.max_tokens";
pub const GEN_AI_REQUEST_TEMPERATURE: &str = "gen_ai.request.temperature";
pub const GEN_AI_REQUEST_TOP_P: &str = "gen_ai.request.top_p";
pub const GEN_AI_RESPONSE_MODEL: &str = "gen_ai.response.model";

// gen_ai.* shortcuts
pub const GEN_AI_SYSTEM: &str = "gen_ai.system";
pub const GEN_AI_REQUEST_MODEL: &str = "gen_ai.request.model";
pub const GEN_AI_USAGE_INPUT: &str = "gen_ai.usage.input_tokens";
pub const GEN_AI_USAGE_OUTPUT: &str = "gen_ai.usage.output_tokens";
pub const GEN_AI_USAGE_CACHE_READ: &str = "gen_ai.usage.cache_read_input_tokens";

/// Builder for attribute lists. Skips empty/None values so spans don't
/// accumulate noise like `puffer.tool.call_id = ""`. Returns
/// `Vec<KeyValue>` ready to drop into `with_attributes`.
#[derive(Default)]
pub struct AttributeBag {
    inner: Vec<KeyValue>,
}

impl AttributeBag {
    /// New empty bag.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a string attribute, skipping empty values to avoid
    /// noise on the span.
    pub fn str(mut self, key: &'static str, value: impl Into<String>) -> Self {
        let v = value.into();
        if !v.is_empty() {
            self.inner.push(KeyValue::new(key, v));
        }
        self
    }

    /// Push a string attribute when present; no-op when `None`.
    pub fn opt_str(self, key: &'static str, value: Option<impl Into<String>>) -> Self {
        match value {
            Some(v) => self.str(key, v),
            None => self,
        }
    }

    /// Push an integer attribute.
    pub fn int(mut self, key: &'static str, value: i64) -> Self {
        self.inner.push(KeyValue::new(key, value));
        self
    }

    /// Push an integer attribute when present; no-op when `None`.
    pub fn opt_int(self, key: &'static str, value: Option<i64>) -> Self {
        match value {
            Some(v) => self.int(key, v),
            None => self,
        }
    }

    /// Push a boolean attribute.
    pub fn bool(mut self, key: &'static str, value: bool) -> Self {
        self.inner.push(KeyValue::new(key, value));
        self
    }

    /// Push an arbitrary pre-built `KeyValue` (for array-typed
    /// attributes the typed setters above do not cover).
    pub fn kv(mut self, kv: KeyValue) -> Self {
        self.inner.push(kv);
        self
    }

    /// Finalize the bag into a `Vec<KeyValue>` ready for span build.
    pub fn build(self) -> Vec<KeyValue> {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_provider_classifies_known_relays() {
        assert!(matches!(
            GenAiSystem::from_provider("kimi-coding"),
            GenAiSystem::Anthropic
        ));
        assert!(matches!(
            GenAiSystem::from_provider("kimi-openai"),
            GenAiSystem::Kimi
        ));
        assert!(matches!(
            GenAiSystem::from_provider("hanbbq"),
            GenAiSystem::OpenAi
        ));
        assert!(matches!(
            GenAiSystem::from_provider("anthropic"),
            GenAiSystem::Anthropic
        ));
        assert!(matches!(
            GenAiSystem::from_provider("grok-relay"),
            GenAiSystem::Xai
        ));
        assert!(matches!(
            GenAiSystem::from_provider("unknown-thing"),
            GenAiSystem::Other
        ));
    }

    #[test]
    fn observation_kind_emits_correct_langfuse_type() {
        assert_eq!(
            ObservationKind::ProviderCall.langfuse_observation_type(),
            "generation"
        );
        assert_eq!(ObservationKind::Tool.langfuse_observation_type(), "span");
    }

    #[test]
    fn attribute_bag_skips_empty_strings() {
        let kvs = AttributeBag::new()
            .str(PUFFER_SESSION_ID, "abc")
            .str(PUFFER_CWD, "")
            .opt_str(PUFFER_PROVIDER_ID, Some("hanbbq"))
            .opt_str(PUFFER_API, None::<String>)
            .build();
        assert_eq!(kvs.len(), 2);
        let keys: Vec<&str> = kvs.iter().map(|kv| kv.key.as_str()).collect();
        assert!(keys.contains(&PUFFER_SESSION_ID));
        assert!(keys.contains(&PUFFER_PROVIDER_ID));
    }
}
