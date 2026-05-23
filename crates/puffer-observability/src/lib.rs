//! Puffer observability — OTLP/HTTP trace exporter, primarily targeted at
//! Langfuse but compatible with any OTLP collector (Honeycomb, Tempo,
//! Datadog, Jaeger).
//!
//! See `docs/observability/langfuse-design.md` for the design rationale.
//!
//! # Lifecycle
//!
//! 1. App boot calls [`init_from_config`] (or [`init_from_env`]). On a
//!    valid config, it constructs an [`ObservabilityHandle`] backed by an
//!    OTel `TracerProvider` + OTLP/HTTP exporter; on missing/invalid
//!    config it returns `None` (everything downstream becomes a no-op).
//! 2. The handle is threaded through `LoopInputs` and provider sessions
//!    as `Option<&ObservabilityHandle>` (mirrors `CancelToken`).
//! 3. App shutdown calls [`ObservabilityHandle::shutdown`] to flush
//!    buffered spans before exit.
//!
//! # Threading
//!
//! Puffer's main runtime is `reqwest::blocking`; we do **not** want to
//! force the rest of the workspace into async just to push spans. We
//! solve this by owning a single dedicated multi-threaded tokio runtime
//! inside the handle and using the OTel SDK's `BatchSpanProcessor` with
//! `Tokio` as the spawning runtime. All OTel I/O happens off the main
//! thread; span construction and attribute writes are sync.

use anyhow::{Context, Result};
use base64::Engine;
use opentelemetry::{global, global::BoxedTracer, KeyValue};
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    trace::{RandomIdGenerator, Sampler, SimpleSpanProcessor, TracerProvider},
    Resource,
};
use std::sync::Arc;
use std::time::Duration;

mod attributes;
mod hooks;
mod redaction;
#[cfg(test)]
mod tests;

pub use attributes::{
    AttributeBag, GenAiSystem, ObservationKind, GEN_AI_REQUEST_MAX_TOKENS, GEN_AI_REQUEST_MODEL,
    GEN_AI_REQUEST_TEMPERATURE, GEN_AI_REQUEST_TOP_P, GEN_AI_RESPONSE_MODEL, GEN_AI_SYSTEM,
    LANGFUSE_ENVIRONMENT, LANGFUSE_OBSERVATION_INPUT, LANGFUSE_OBSERVATION_LEVEL,
    LANGFUSE_OBSERVATION_OUTPUT, LANGFUSE_OBSERVATION_STATUS_MESSAGE, LANGFUSE_TRACE_INPUT,
    LANGFUSE_TRACE_OUTPUT, LANGFUSE_TRACE_TAGS, LANGFUSE_USER_ID, PUFFER_API, PUFFER_CWD,
    PUFFER_PROVIDER_ID, PUFFER_SESSION_ID, PUFFER_TOOL_CALL_ID, PUFFER_TOOL_PARALLEL,
};
pub use hooks::{
    start_agent_loop_span, start_compaction_span, start_provider_span, start_reflection_span,
    start_subagent_generation_span, start_subagent_generation_span_at, start_tool_span,
    start_turn_span, SpanGuard,
};
pub use opentelemetry::Context as OtelContext;
pub use redaction::{redact, ContentKind, RedactionPolicy};

/// User-facing configuration for the observability pipeline.
///
/// **Codex review note (high-severity):** the default content-capture
/// posture is conservative. `include_*` flags are split so operators
/// can opt into prompts only, tool I/O only, or both. Coding agents
/// regularly ingest source files, env vars, and diffs — sending
/// those to a third-party trace store by default would be a security
/// regression.
#[derive(Debug, Clone)]
pub struct ObservabilityConfig {
    /// OTLP endpoint *base*. The exporter will POST to
    /// `<endpoint>/v1/traces` (matching the OpenTelemetry spec).
    /// For Langfuse, set this to `<langfuse-host>/api/public/otel`.
    pub endpoint: String,
    /// Static resource attributes attached to every span.
    pub service_name: String,
    /// Build/release tag forwarded as `langfuse.version` so dashboards
    /// can correlate traces with a specific puffer build.
    pub release: Option<String>,
    /// Deployment environment (e.g. "dev", "staging", "prod").
    /// Forwarded as `langfuse.environment` on every span and surfaces
    /// in Langfuse's Environment column / filter. Discovered from
    /// `PUFFER_OBSERVABILITY_ENVIRONMENT` or `OTEL_DEPLOYMENT_ENVIRONMENT`.
    pub environment: Option<String>,
    /// Free-form tags forwarded as `langfuse.tags`.
    pub tags: Vec<String>,
    /// Per-trace user identity stamped on every root `agent_loop`
    /// span as `langfuse.user.id`. Populates Langfuse's Users tab so
    /// trace cost / activity can be sliced by operator. Discovered
    /// from env in this order: `PUFFER_OBSERVABILITY_USER_ID` →
    /// `LANGFUSE_USER_ID` → `USER`/`USERNAME` (fallback).
    pub user_id: Option<String>,
    /// HTTP headers for the exporter. For Langfuse, supply
    /// `Authorization: Basic <base64(public_key:secret_key)>` plus
    /// `x-langfuse-ingestion-version=4` (added automatically by
    /// [`Self::for_langfuse`]).
    pub headers: Vec<(String, String)>,
    /// Trace sampler — 1.0 = always on; 0.0 = drop everything;
    /// values between 0 and 1 emit `ParentBased(TraceIdRatio)`.
    pub sample_rate: f64,
    /// Send the user prompt + injected system reminders. Default:
    /// false. Risk: leaks any text the user typed.
    pub include_prompts: bool,
    /// Send the assistant's text + thinking deltas. Default: false.
    /// Risk: leaks model reasoning that may quote prompts/tool I/O.
    pub include_outputs: bool,
    /// Send tool input arguments + outputs. Default: false. Risk:
    /// extreme — file contents, env vars, diffs, secret tokens.
    pub include_tool_io: bool,
    /// Provider/tool ids whose I/O is *always* redacted regardless
    /// of the include flags. The default list covers the obvious
    /// secrets-in-the-name tools.
    pub always_redact_tool_ids: Vec<String>,
    /// Hard ceiling on per-span flush time at process exit.
    pub shutdown_timeout: Duration,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            service_name: "puffer".to_string(),
            release: option_env!("CARGO_PKG_VERSION").map(str::to_string),
            tags: Vec::new(),
            environment: None,
            user_id: None,
            headers: Vec::new(),
            sample_rate: 1.0,
            include_prompts: false,
            include_outputs: false,
            include_tool_io: false,
            always_redact_tool_ids: vec![
                "AuthSetApiKey".into(),
                "Email".into(),
                "EmailConfigure".into(),
                "Login".into(),
                "Logout".into(),
                "Telegram".into(),
                "TelegramLoginSubmitCode".into(),
                "TelegramLoginSubmitPassword".into(),
            ],
            shutdown_timeout: Duration::from_secs(5),
        }
    }
}

impl ObservabilityConfig {
    /// Convenience constructor for Langfuse: takes the base URL plus
    /// the public/secret key pair; emits the matching OTLP endpoint,
    /// Basic auth header, and Langfuse ingestion-version pin.
    pub fn for_langfuse(
        host: impl Into<String>,
        public_key: impl AsRef<str>,
        secret_key: impl AsRef<str>,
    ) -> Self {
        let host = host.into();
        let host = host.trim_end_matches('/').to_string();
        let endpoint = format!("{host}/api/public/otel");
        let raw = format!("{}:{}", public_key.as_ref(), secret_key.as_ref());
        let basic = base64::engine::general_purpose::STANDARD.encode(raw);
        Self {
            endpoint,
            headers: vec![
                ("Authorization".to_string(), format!("Basic {basic}")),
                // Langfuse OTel ingestion: pin the version the SDK
                // shape was written against. Without this, future
                // server-side changes can silently route to a
                // different parser.
                ("x-langfuse-ingestion-version".to_string(), "4".to_string()),
            ],
            ..Self::default()
        }
    }
}

/// Owns the OTel `TracerProvider`, its background tokio runtime, and
/// the redaction policy. Cloneable; clones share state via `Arc`.
#[derive(Clone)]
pub struct ObservabilityHandle {
    inner: Arc<HandleInner>,
}

struct HandleInner {
    provider: TracerProvider,
    redaction: RedactionPolicy,
    shutdown_timeout: Duration,
    /// Per-trace tags surfaced on the root `agent_loop` span as
    /// `langfuse.trace.tags` so Langfuse renders them in its tag
    /// filter / per-trace tag chips.
    tags: Vec<String>,
    /// Per-trace user id surfaced on the root `agent_loop` span as
    /// `langfuse.user.id` so Langfuse's Users tab populates and
    /// per-user cost / activity slicing works.
    user_id: Option<String>,
    /// Deployment environment surfaced on the root `agent_loop` span
    /// as `langfuse.environment` so Langfuse's Environment filter
    /// works against e.g. "dev" / "prod".
    environment: Option<String>,
}

impl ObservabilityHandle {
    /// Initialize the OTLP/HTTP exporter from an [`ObservabilityConfig`].
    /// The returned handle's drop runs the equivalent of `shutdown` —
    /// callers that want a deterministic flush should call
    /// [`ObservabilityHandle::shutdown`] explicitly before process exit.
    pub fn init(config: ObservabilityConfig) -> Result<Self> {
        let exporter_headers: std::collections::HashMap<String, String> =
            config.headers.iter().cloned().collect();

        // Use http/protobuf (the OTLP spec default) for Langfuse
        // compatibility. opentelemetry-otlp 0.27 typed builder.
        let exporter = SpanExporter::builder()
            .with_http()
            .with_endpoint(format!(
                "{}/v1/traces",
                config.endpoint.trim_end_matches('/')
            ))
            .with_protocol(Protocol::HttpBinary)
            .with_headers(exporter_headers)
            .with_timeout(Duration::from_secs(10))
            .build()
            .context("building OTLP HTTP span exporter")?;

        // SimpleSpanProcessor: each span ends with a synchronous
        // POST. We pick this over BatchSpanProcessor to avoid pulling
        // a tokio runtime into puffer's blocking process. At CLI
        // agent scale (≤ 100 spans/turn) the per-span latency is
        // dominated by the upstream LLM, so a 10ms OTLP POST is
        // negligible. If we later need batching we can swap back.
        let processor = SimpleSpanProcessor::new(Box::new(exporter));

        let mut resource_kvs = vec![
            KeyValue::new("service.name", config.service_name.clone()),
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            KeyValue::new("telemetry.sdk.name", "puffer-observability"),
        ];
        if let Some(release) = config.release.clone() {
            // Codex review note: Langfuse uses `langfuse.version` and
            // `langfuse.release` for build correlation. We forward
            // both from the same source to keep the dashboard
            // selectors useful regardless of which one is configured.
            resource_kvs.push(KeyValue::new("langfuse.version", release.clone()));
            resource_kvs.push(KeyValue::new("langfuse.release", release));
        }
        if let Some(env) = config.environment.clone() {
            // Langfuse's trace-level Environment column resolves from
            // resource attributes; per-observation `langfuse.environment`
            // (set by the root span helper) handles the observation level.
            resource_kvs.push(KeyValue::new("langfuse.environment", env.clone()));
            resource_kvs.push(KeyValue::new("deployment.environment", env));
        }
        // Tags travel with each trace via `langfuse.trace.tags` on the
        // root span, NOT the resource — Langfuse's tag filter reads
        // span-level. See `start_agent_loop_span`.
        let resource = Resource::new(resource_kvs);

        let sampler = if config.sample_rate >= 1.0 {
            Sampler::AlwaysOn
        } else if config.sample_rate <= 0.0 {
            Sampler::AlwaysOff
        } else {
            Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(config.sample_rate)))
        };

        let provider = TracerProvider::builder()
            .with_span_processor(processor)
            .with_resource(resource)
            .with_sampler(sampler)
            .with_id_generator(RandomIdGenerator::default())
            .build();

        // Register globally so callers can reach the tracer via the
        // global API even if they don't have a handle (e.g. background
        // workers spawned from inside the agent loop).
        global::set_tracer_provider(provider.clone());

        let redaction = RedactionPolicy::from_config(
            config.include_prompts,
            config.include_outputs,
            config.include_tool_io,
            config.always_redact_tool_ids,
        );
        let shutdown_timeout = config.shutdown_timeout;
        let tags = config.tags;
        let user_id = config.user_id;
        let environment = config.environment;

        Ok(Self {
            inner: Arc::new(HandleInner {
                provider,
                redaction,
                shutdown_timeout,
                tags,
                user_id,
                environment,
            }),
        })
    }

    /// Per-trace tags pulled from `ObservabilityConfig::tags`. Used by
    /// `start_agent_loop_span` to set `langfuse.trace.tags` on the root
    /// span so Langfuse renders them in its tag filter.
    pub fn tags(&self) -> &[String] {
        &self.inner.tags
    }

    /// Per-trace user id pulled from `ObservabilityConfig::user_id`.
    /// Used by `start_agent_loop_span` to set `langfuse.user.id` on
    /// the root span so Langfuse's Users tab populates.
    pub fn user_id(&self) -> Option<&str> {
        self.inner.user_id.as_deref()
    }

    /// Per-trace deployment environment for `langfuse.environment`.
    pub fn environment(&self) -> Option<&str> {
        self.inner.environment.as_deref()
    }

    /// Build an [`ObservabilityConfig`] from environment variables that
    /// follow the OTel spec (with Puffer-specific extensions), then
    /// call [`init`]. Returns `Ok(None)` when no endpoint is
    /// configured (the canonical "off" state).
    ///
    /// Recognized vars:
    /// - `OTEL_EXPORTER_OTLP_ENDPOINT` (required, no default)
    /// - `OTEL_EXPORTER_OTLP_HEADERS=k1=v1,k2=v2` (optional)
    /// - `OTEL_SERVICE_NAME` (defaults to "puffer")
    /// - `PUFFER_OBSERVABILITY_SAMPLE_RATE` (defaults to 1.0)
    /// - `PUFFER_OBSERVABILITY_INCLUDE_PROMPTS` (defaults to "0")
    /// - `PUFFER_OBSERVABILITY_INCLUDE_OUTPUTS` (defaults to "0")
    /// - `PUFFER_OBSERVABILITY_INCLUDE_TOOL_IO` (defaults to "0")
    /// - `PUFFER_OBSERVABILITY_INCLUDE_CONTENT` legacy "all-on" toggle
    ///   (when "1", flips all three include_* flags to true).
    /// - `LANGFUSE_PUBLIC_KEY` / `LANGFUSE_SECRET_KEY` (optional —
    ///   when both are set, an `Authorization: Basic <base64>` header
    ///   and the Langfuse ingestion-version pin are added on top of
    ///   `OTEL_EXPORTER_OTLP_HEADERS`. This keeps the secret out of
    ///   workspace TOML, per Codex review.)
    pub fn try_init_from_env() -> Result<Option<Self>> {
        let endpoint = match std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
            Ok(v) if !v.is_empty() => v,
            _ => return Ok(None),
        };
        let mut headers = std::env::var("OTEL_EXPORTER_OTLP_HEADERS")
            .ok()
            .map(parse_headers)
            .unwrap_or_default();
        if let (Ok(pk), Ok(sk)) = (
            std::env::var("LANGFUSE_PUBLIC_KEY"),
            std::env::var("LANGFUSE_SECRET_KEY"),
        ) {
            if !pk.is_empty() && !sk.is_empty() {
                let basic = base64::engine::general_purpose::STANDARD.encode(format!("{pk}:{sk}"));
                upsert_header(&mut headers, "Authorization", format!("Basic {basic}"));
                upsert_header(
                    &mut headers,
                    "x-langfuse-ingestion-version",
                    "4".to_string(),
                );
            }
        }
        let service_name =
            std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "puffer".to_string());
        let sample_rate: f64 = std::env::var("PUFFER_OBSERVABILITY_SAMPLE_RATE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);
        let legacy_all = std::env::var("PUFFER_OBSERVABILITY_INCLUDE_CONTENT")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        let bool_env = |name: &str| -> bool {
            std::env::var(name)
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                .unwrap_or(false)
        };
        let include_prompts = legacy_all || bool_env("PUFFER_OBSERVABILITY_INCLUDE_PROMPTS");
        let include_outputs = legacy_all || bool_env("PUFFER_OBSERVABILITY_INCLUDE_OUTPUTS");
        let include_tool_io = legacy_all || bool_env("PUFFER_OBSERVABILITY_INCLUDE_TOOL_IO");
        let release = std::env::var("PUFFER_OBSERVABILITY_RELEASE").ok();
        let environment = std::env::var("PUFFER_OBSERVABILITY_ENVIRONMENT")
            .ok()
            .or_else(|| std::env::var("OTEL_DEPLOYMENT_ENVIRONMENT").ok())
            .filter(|s| !s.is_empty());
        let tags = std::env::var("PUFFER_OBSERVABILITY_TAGS")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        // User id discovery — explicit puffer override beats Langfuse-
        // standard env beats the OS user. Skipped entirely when none
        // is set so Langfuse's Users tab remains empty rather than
        // populated with a synthetic anonymous bucket.
        let user_id = std::env::var("PUFFER_OBSERVABILITY_USER_ID")
            .ok()
            .or_else(|| std::env::var("LANGFUSE_USER_ID").ok())
            .or_else(|| std::env::var("USER").ok())
            .or_else(|| std::env::var("USERNAME").ok())
            .filter(|s| !s.is_empty());
        let config = ObservabilityConfig {
            endpoint,
            service_name,
            headers,
            sample_rate,
            include_prompts,
            include_outputs,
            include_tool_io,
            release,
            environment,
            tags,
            user_id,
            ..ObservabilityConfig::default()
        };
        Ok(Some(Self::init(config)?))
    }

    /// Borrow the SDK tracer for direct span construction. Most callers
    /// should go through the higher-level helpers in
    /// [`agent_loop_hooks`].
    pub fn tracer(&self) -> BoxedTracer {
        global::tracer("puffer")
    }

    /// Returns the active redaction policy.
    pub fn redaction(&self) -> &RedactionPolicy {
        &self.inner.redaction
    }

    /// Force-flush buffered spans then shut down the provider. Should
    /// be called at process exit (e.g. from
    /// `puffer_core::shutdown_runtime_services`) so spans aren't
    /// lost when the process tears down.
    ///
    /// Codex review note: bounded by [`ObservabilityConfig::shutdown_timeout`]
    /// so a stuck Langfuse server can't hang the CLI on `Ctrl+D`.
    pub fn shutdown(self) {
        let timeout = self.inner.shutdown_timeout;
        let provider = self.inner.provider.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = provider.force_flush();
            let _ = tx.send(());
        });
        // Best-effort flush within the configured budget. If the
        // network's lost we'd rather drop a few spans than hang the
        // user.
        let _ = rx.recv_timeout(timeout);
        drop(self);
    }
}

fn parse_headers(spec: String) -> Vec<(String, String)> {
    spec.split(',')
        .filter_map(|entry| {
            let (k, v) = entry.split_once('=')?;
            let k = k.trim();
            let v = v.trim();
            if k.is_empty() {
                None
            } else {
                Some((k.to_string(), v.to_string()))
            }
        })
        .collect()
}

/// Inserts or replaces `name` in a header list (case-insensitive
/// match). Existing duplicates of the same name are removed first.
fn upsert_header(headers: &mut Vec<(String, String)>, name: &str, value: String) {
    headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    headers.push((name.to_string(), value));
}

#[cfg(test)]
pub(crate) fn parse_headers_for_tests(spec: &str) -> Vec<(String, String)> {
    parse_headers(spec.to_string())
}
