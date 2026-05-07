//! Helper builders that wrap [`opentelemetry::trace::Tracer`] for the
//! mounting points in `puffer_core::runtime`. The goal is to keep the
//! complexity of OTel APIs out of the agent loop — all callers do is
//!
//! ```ignore
//! let span = obs::start_provider_span(&handle, "openai", "gpt-5.5");
//! // ... do work ...
//! span.set_token_usage(input, output, cache_read);
//! drop(span); // span ends synchronously, posts to OTLP
//! ```
//!
//! The wrappers intentionally take `Option<&ObservabilityHandle>` so
//! every call site is `if let Some(h) = handle { ... }`-free; when
//! disabled, the helpers return [`SpanGuard::Disabled`] which is a
//! zero-cost no-op.

use crate::attributes::{
    AttributeBag, GenAiSystem, ObservationKind, GEN_AI_REQUEST_MODEL, GEN_AI_SYSTEM,
    GEN_AI_USAGE_CACHE_READ, GEN_AI_USAGE_INPUT, GEN_AI_USAGE_OUTPUT, LANGFUSE_OBSERVATION_TYPE,
    LANGFUSE_SESSION_ID, PUFFER_API, PUFFER_CWD, PUFFER_PROVIDER_ID, PUFFER_SESSION_ID,
    PUFFER_TOOL_CALL_ID, PUFFER_TOOL_PARALLEL,
};
use crate::redaction::ContentKind;
use crate::ObservabilityHandle;
use opentelemetry::trace::{Span as OtelSpan, SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{Context as OtelContext, KeyValue};

/// Span-like guard that records observability data when active and
/// silently no-ops when the handle is missing.
pub enum SpanGuard {
    Disabled,
    Active {
        span: opentelemetry::global::BoxedSpan,
        ctx: OtelContext,
        handle: ObservabilityHandle,
    },
}

impl SpanGuard {
    /// Internal helper: build an active guard from a started span +
    /// the OtelContext that owns it.
    fn active(
        span: opentelemetry::global::BoxedSpan,
        parent: OtelContext,
        handle: ObservabilityHandle,
    ) -> Self {
        // Push the new span onto the context so child spans started
        // via `start_*_span(handle, ...)` see it as the parent.
        let ctx = parent.with_span(span_clone(&span));
        Self::Active { span, ctx, handle }
    }

    /// Returns the OTel `Context` carrying this span. Used by the
    /// agent loop to propagate span context into `thread::scope`
    /// branches when running parallel tools — Codex review noted that
    /// without explicit context attach, parallel tool spans become
    /// orphans.
    pub fn context(&self) -> Option<&OtelContext> {
        match self {
            Self::Active { ctx, .. } => Some(ctx),
            Self::Disabled => None,
        }
    }

    /// Add an arbitrary string attribute (already redacted by caller).
    pub fn set_str(&mut self, key: &'static str, value: impl Into<String>) {
        if let Self::Active { span, .. } = self {
            span.set_attribute(KeyValue::new(key, value.into()));
        }
    }

    /// Add a numeric (f64) attribute. Use this for ratios / percentages
    /// so backends like Langfuse can aggregate / filter numerically
    /// instead of treating them as opaque strings.
    pub fn set_f64(&mut self, key: &'static str, value: f64) {
        if let Self::Active { span, .. } = self {
            span.set_attribute(KeyValue::new(key, value));
        }
    }

    /// Records a redacted prompt/output blob on the span. Falls
    /// through to no-op when disabled or when the handle's redaction
    /// policy returns the redacted form for this kind.
    pub fn set_content(&mut self, key: &'static str, kind: ContentKind, value: &str) {
        if let Self::Active { span, handle, .. } = self {
            let redacted = handle.redaction().apply(kind, value);
            span.set_attribute(KeyValue::new(key, redacted));
        }
    }

    /// Records token usage on a provider_call span. When any of the
    /// counts is `None`, the corresponding attribute is omitted.
    pub fn set_token_usage(
        &mut self,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cache_read_tokens: Option<u64>,
    ) {
        if let Self::Active { span, .. } = self {
            if let Some(v) = input_tokens {
                span.set_attribute(KeyValue::new(GEN_AI_USAGE_INPUT, v as i64));
            }
            if let Some(v) = output_tokens {
                span.set_attribute(KeyValue::new(GEN_AI_USAGE_OUTPUT, v as i64));
            }
            if let Some(v) = cache_read_tokens {
                span.set_attribute(KeyValue::new(GEN_AI_USAGE_CACHE_READ, v as i64));
            }
        }
    }

    /// Marks the span as cancelled.
    pub fn mark_cancelled(&mut self) {
        if let Self::Active { span, .. } = self {
            span.set_status(Status::error("cancelled"));
            span.set_attribute(KeyValue::new("puffer.cancel.observed", true));
        }
    }

    /// Marks the span as failed with the given message.
    pub fn mark_error(&mut self, message: impl Into<String>) {
        if let Self::Active { span, .. } = self {
            span.set_status(Status::error(message.into()));
        }
    }

    /// Force-end the span before drop. Drop also ends; this is for
    /// when callers want explicit ordering.
    pub fn end(self) {
        drop(self);
    }
}

/// `BoxedSpan` is `!Clone`, but for the `with_span` call we need a
/// reference-style reuse. Drop the cloning intent — instead, propagate
/// just the SpanContext, which is `Copy`-via-`Clone`. The "context"
/// observation is good enough: child spans only need the parent's
/// SpanContext (trace_id + span_id), not its full attribute payload.
fn span_clone(span: &opentelemetry::global::BoxedSpan) -> SpanContextHolder {
    SpanContextHolder {
        ctx: span.span_context().clone(),
    }
}

/// Lightweight `Span` impl that just holds the parent's SpanContext
/// for child-span resolution. Doesn't actually own a span — never
/// records, never ends. Used purely so OtelContext can resolve
/// "what's the active span context" when starting children.
struct SpanContextHolder {
    ctx: opentelemetry::trace::SpanContext,
}

impl OtelSpan for SpanContextHolder {
    fn add_event_with_timestamp<T>(&mut self, _: T, _: std::time::SystemTime, _: Vec<KeyValue>)
    where
        T: Into<std::borrow::Cow<'static, str>>,
    {
    }
    fn span_context(&self) -> &opentelemetry::trace::SpanContext {
        &self.ctx
    }
    fn is_recording(&self) -> bool {
        false
    }
    fn set_attribute(&mut self, _: KeyValue) {}
    fn set_status(&mut self, _: Status) {}
    fn update_name<T>(&mut self, _: T)
    where
        T: Into<std::borrow::Cow<'static, str>>,
    {
    }
    fn add_link(&mut self, _: opentelemetry::trace::SpanContext, _: Vec<KeyValue>) {}
    fn end_with_timestamp(&mut self, _: std::time::SystemTime) {}
}

// ---------------------------------------------------------------------
// High-level helpers — one per mounting point in the agent loop.
// ---------------------------------------------------------------------

/// Start the root `agent_loop` span for a user prompt. Returns a
/// guard that callers should keep alive for the duration of the
/// prompt. When `handle` is `None`, returns [`SpanGuard::Disabled`].
pub fn start_agent_loop_span(
    handle: Option<&ObservabilityHandle>,
    session_id: &str,
    cwd: &str,
) -> SpanGuard {
    let Some(handle) = handle else {
        return SpanGuard::Disabled;
    };
    let tracer = handle.tracer();
    let parent = OtelContext::current();
    let mut builder = tracer
        .span_builder(ObservationKind::AgentLoop.span_name())
        .with_kind(SpanKind::Internal);
    let mut bag = AttributeBag::new()
        .str(PUFFER_SESSION_ID, session_id)
        .str(LANGFUSE_SESSION_ID, session_id)
        .str(PUFFER_CWD, cwd)
        .str(
            LANGFUSE_OBSERVATION_TYPE,
            ObservationKind::AgentLoop.langfuse_observation_type(),
        );
    let tags = handle.tags();
    if !tags.is_empty() {
        // Langfuse renders trace tag chips from a string-array
        // attribute on the root span. The OTel spec allows array-typed
        // attributes; we go through `KeyValue::new` directly because
        // `AttributeBag::str` is single-string only.
        let kv = opentelemetry::KeyValue::new(
            crate::attributes::LANGFUSE_TRACE_TAGS,
            opentelemetry::Value::Array(opentelemetry::Array::String(
                tags.iter().map(|t| t.clone().into()).collect(),
            )),
        );
        bag = bag.kv(kv);
    }
    if let Some(uid) = handle.user_id() {
        bag = bag.str(crate::attributes::LANGFUSE_USER_ID, uid);
    }
    if let Some(env) = handle.environment() {
        bag = bag.str(crate::attributes::LANGFUSE_ENVIRONMENT, env);
    }
    builder.attributes = Some(bag.build());
    let span = tracer.build_with_context(builder, &parent);
    SpanGuard::active(span, parent, handle.clone())
}

/// Start a `turn` child span. Pass the parent guard's context so the
/// span nests correctly.
pub fn start_turn_span(
    handle: Option<&ObservabilityHandle>,
    parent: Option<&OtelContext>,
    turn_index: u32,
) -> SpanGuard {
    let Some(handle) = handle else {
        return SpanGuard::Disabled;
    };
    let tracer = handle.tracer();
    let parent_ctx = parent.cloned().unwrap_or_else(OtelContext::current);
    let mut builder = tracer
        .span_builder(ObservationKind::Turn.span_name())
        .with_kind(SpanKind::Internal);
    builder.attributes = Some(
        AttributeBag::new()
            .int("puffer.turn.index", turn_index as i64)
            .str(
                LANGFUSE_OBSERVATION_TYPE,
                ObservationKind::Turn.langfuse_observation_type(),
            )
            .build(),
    );
    let span = tracer.build_with_context(builder, &parent_ctx);
    SpanGuard::active(span, parent_ctx, handle.clone())
}

/// Start a `provider_call` child span (Langfuse "generation").
pub fn start_provider_span(
    handle: Option<&ObservabilityHandle>,
    parent: Option<&OtelContext>,
    provider_id: &str,
    api: &str,
    model: &str,
) -> SpanGuard {
    let Some(handle) = handle else {
        return SpanGuard::Disabled;
    };
    let tracer = handle.tracer();
    let parent_ctx = parent.cloned().unwrap_or_else(OtelContext::current);
    let system = GenAiSystem::from_provider(provider_id);
    let mut builder = tracer
        .span_builder(ObservationKind::ProviderCall.span_name())
        .with_kind(SpanKind::Client);
    builder.attributes = Some(
        AttributeBag::new()
            .str(PUFFER_PROVIDER_ID, provider_id)
            .str(PUFFER_API, api)
            .str(GEN_AI_SYSTEM, system.as_str())
            .str(GEN_AI_REQUEST_MODEL, model)
            .str(
                LANGFUSE_OBSERVATION_TYPE,
                ObservationKind::ProviderCall.langfuse_observation_type(),
            )
            .build(),
    );
    let span = tracer.build_with_context(builder, &parent_ctx);
    SpanGuard::active(span, parent_ctx, handle.clone())
}

/// Start a `tool` child span. `parallel = true` indicates the tool
/// is being run inside `thread::scope`; the caller MUST pass the
/// parent context explicitly because OTel's thread-local context
/// does NOT cross scoped-thread boundaries.
pub fn start_tool_span(
    handle: Option<&ObservabilityHandle>,
    parent: Option<&OtelContext>,
    tool_id: &str,
    call_id: &str,
    parallel: bool,
) -> SpanGuard {
    let Some(handle) = handle else {
        return SpanGuard::Disabled;
    };
    let tracer = handle.tracer();
    let parent_ctx = parent.cloned().unwrap_or_else(OtelContext::current);
    let mut builder = tracer
        .span_builder(format!("{}.{}", ObservationKind::Tool.span_name(), tool_id))
        .with_kind(SpanKind::Internal);
    builder.attributes = Some(
        AttributeBag::new()
            .str("puffer.tool.id", tool_id)
            .str(PUFFER_TOOL_CALL_ID, call_id)
            .bool(PUFFER_TOOL_PARALLEL, parallel)
            .str(
                LANGFUSE_OBSERVATION_TYPE,
                ObservationKind::Tool.langfuse_observation_type(),
            )
            .build(),
    );
    let span = tracer.build_with_context(builder, &parent_ctx);
    SpanGuard::active(span, parent_ctx, handle.clone())
}

/// Start a `subagent.reflection_judge` child span. Wraps the per-batch
/// reflection observation that may or may not fire an LLM judge call.
/// Callers populate stage-by-stage attributes
/// (`puffer.reflection.assessment.*`, `puffer.reflection.code_judge.*`,
/// `puffer.reflection.llm_judge.*`, `puffer.reflection.final.*`) so a
/// viewer can tell exactly which path the reflection pipeline took.
pub fn start_reflection_span(
    handle: Option<&ObservabilityHandle>,
    parent: Option<&OtelContext>,
) -> SpanGuard {
    let Some(handle) = handle else {
        return SpanGuard::Disabled;
    };
    let tracer = handle.tracer();
    let parent_ctx = parent.cloned().unwrap_or_else(OtelContext::current);
    let mut builder = tracer
        .span_builder(ObservationKind::Reflection.span_name())
        .with_kind(SpanKind::Internal);
    builder.attributes = Some(
        AttributeBag::new()
            .str(
                LANGFUSE_OBSERVATION_TYPE,
                ObservationKind::Reflection.langfuse_observation_type(),
            )
            .build(),
    );
    let span = tracer.build_with_context(builder, &parent_ctx);
    SpanGuard::active(span, parent_ctx, handle.clone())
}

/// Start a `subagent.<kind>` GENERATION span. Use this *inside* a
/// reflection / compaction wrapper when an LLM round-trip actually
/// fires, so the inner generation observation carries
/// `langfuse.observation.type=generation` (with token usage) while the
/// outer wrapper stays a plain SPAN. `kind` is also surfaced as
/// `puffer.subagent.kind` for filtering.
///
/// Mental model:
/// - Outer wrapper SPAN: control/decision logic — always present.
/// - Inner subagent GENERATION: actual LLM call — only present when
///   the wrapper decided to make the call. So the absence of an inner
///   subagent span means "no LLM round-trip happened here".
pub fn start_subagent_generation_span(
    handle: Option<&ObservabilityHandle>,
    parent: Option<&OtelContext>,
    kind: &'static str,
) -> SpanGuard {
    start_subagent_generation_span_at(handle, parent, kind, None)
}

/// Variant that backdates the span's start time. Use when the wrapped
/// LLM call has already completed by the time you can confirm it
/// fired (e.g. reflection judge events arrive after `run_llm_judge`
/// returns) — pass the `SystemTime` captured before the call and
/// the span will reflect the actual latency rather than the post-hoc
/// emission point.
pub fn start_subagent_generation_span_at(
    handle: Option<&ObservabilityHandle>,
    parent: Option<&OtelContext>,
    kind: &'static str,
    start_time: Option<std::time::SystemTime>,
) -> SpanGuard {
    let Some(handle) = handle else {
        return SpanGuard::Disabled;
    };
    let tracer = handle.tracer();
    let parent_ctx = parent.cloned().unwrap_or_else(OtelContext::current);
    let mut builder = tracer
        .span_builder(format!("subagent.{kind}"))
        .with_kind(SpanKind::Internal);
    if let Some(start) = start_time {
        builder = builder.with_start_time(start);
    }
    builder.attributes = Some(
        AttributeBag::new()
            .str(LANGFUSE_OBSERVATION_TYPE, "generation")
            .str("puffer.subagent.kind", kind)
            .build(),
    );
    let span = tracer.build_with_context(builder, &parent_ctx);
    SpanGuard::active(span, parent_ctx, handle.clone())
}

/// Start a `compaction` child span.
pub fn start_compaction_span(
    handle: Option<&ObservabilityHandle>,
    parent: Option<&OtelContext>,
    phase: u8,
) -> SpanGuard {
    let Some(handle) = handle else {
        return SpanGuard::Disabled;
    };
    let tracer = handle.tracer();
    let parent_ctx = parent.cloned().unwrap_or_else(OtelContext::current);
    let mut builder = tracer
        .span_builder(ObservationKind::Compaction.span_name())
        .with_kind(SpanKind::Internal);
    builder.attributes = Some(
        AttributeBag::new()
            .int("puffer.compaction.phase", phase as i64)
            .str(
                LANGFUSE_OBSERVATION_TYPE,
                ObservationKind::Compaction.langfuse_observation_type(),
            )
            .build(),
    );
    let span = tracer.build_with_context(builder, &parent_ctx);
    SpanGuard::active(span, parent_ctx, handle.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// When the handle is None every helper returns `Disabled` — no
    /// allocations, no global state mutation. Critical for the
    /// "observability disabled" zero-cost contract.
    #[test]
    fn disabled_helpers_short_circuit() {
        let span = start_agent_loop_span(None, "session", "/tmp");
        assert!(matches!(span, SpanGuard::Disabled));
        let span = start_provider_span(None, None, "openai", "openai-responses", "gpt-5.5");
        assert!(matches!(span, SpanGuard::Disabled));
        let span = start_tool_span(None, None, "Read", "call-1", false);
        assert!(matches!(span, SpanGuard::Disabled));
        let span = start_compaction_span(None, None, 2);
        assert!(matches!(span, SpanGuard::Disabled));
        let span = start_turn_span(None, None, 0);
        assert!(matches!(span, SpanGuard::Disabled));
    }

    #[test]
    fn disabled_set_methods_are_inert() {
        let mut span = start_provider_span(None, None, "openai", "x", "y");
        span.set_str("k", "v");
        span.set_content("k", ContentKind::Prompt, "secret");
        span.set_token_usage(Some(1), Some(2), Some(3));
        span.mark_cancelled();
        span.mark_error("nope");
        span.end();
    }
}
