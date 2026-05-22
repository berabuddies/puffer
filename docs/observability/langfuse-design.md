# Puffer Observability — Langfuse Integration Design

## Goal

Surface every Puffer turn (LLM call + tool batch + reasoning + compaction
+ sub-agent) as a structured trace tree in Langfuse so engineers can
inspect cost, latency, content, and failure modes per session.

## Survey of related agents (Phase 1)

| Dim | Codex | pi-mono | OpenCode | Warp |
|-----|-------|---------|----------|------|
| Purpose | Product analytics | Almost none | Ops/trace | Product analytics + crash |
| Protocol | JSON-RPC (codex BE) | env vars + headers | OTLP (logs+traces) | Rudderstack track + Sentry |
| Endpoint | OpenAI Codex BE | Upstream LLM | `OTEL_EXPORTER_OTLP_ENDPOINT` | Rudderstack data plane |
| Granularity | High (reducer state machine) | None | Auto + manual spans | High (typed enum) |
| Cross-process ctx | thread_id + turn_id | None | AsyncLocalStorage | user_id + anon_id |
| Token usage | `TurnTokenUsageFact` | None | AI SDK embedded | Event payload |
| Tool tracing | `SkillInvocation` + `HookRunFact` | None | `Tool.execute` span | Event |
| Compaction | `CodexCompactionEvent` | None | — | — |
| Sub-agent | `subagent_source` + `parent_thread_id` | None | Span auto-nesting | — |
| UGC isolation | None explicit | — | — | `contains_ugc` flag |
| Gating | `analytics_enabled` | env var | env var | Channel + FeatureFlag |
| Langfuse-friendly | No (private) | No | **Yes (OTLP)** | No (Rudderstack) |

**Conclusion:** OpenCode's OTLP path is the closest to what Puffer
needs. Codex's reducer + typed-fact pattern is worth borrowing for the
internal API. Pi-mono and Warp's product-analytics models are
not a fit (we want ops/trace, not user-behavior tracking).

## Goal trace tree (per user prompt)

```
session_id = puffer-session-<uuid>
└── trace_id = turn-<n>-<uuid>          ← 1 trace per user prompt
    name = "agent_loop"
    input = user prompt
    output = assistant_text
    metadata = { model, provider, cwd, sandbox_mode, effort_level }
    │
    ├── observation: provider_call_1     ← Langfuse "generation"
    │   gen_ai.system = openai
    │   gen_ai.request.model = gpt-5.5
    │   gen_ai.usage.input_tokens
    │   gen_ai.usage.output_tokens
    │   gen_ai.usage.reasoning_tokens
    │   metadata.ttfb_ms / ttfc_ms
    │
    ├── observation: tool_call_1 (Read sample.txt)  ← Langfuse "span"
    │   input  = { path: "..." }
    │   output = first N bytes of file
    │   metadata = { call_id, success, terminate, duration_ms }
    │
    ├── observation: tool_call_2 (Read other.txt) — sibling (parallel)
    │
    ├── observation: provider_call_2 (final-text turn)
    │
    └── observation: compaction (if triggered)
        input  = old_context (hashed; or full when include_content=true)
        output = summary
        metadata = { phase: 2, triggered_at_input_tokens: N }
```

## Protocol & dependencies

- **OTLP/HTTP** (not gRPC) — Langfuse is HTTP-only.
- Langfuse OTLP endpoint: `<base>/api/public/otel/v1/traces` (and `/v1/logs`
  if we add logs later). Auth = HTTP Basic with `<public_key>:<secret_key>`
  base64-encoded.
- Crates to add: `opentelemetry` (~0.31), `opentelemetry_sdk`,
  `opentelemetry-otlp` (with `http-json` feature), `tracing-opentelemetry`
  (bridge to existing `tracing::span!` calls), `base64`.

## Crate layout

```
crates/puffer-observability/
├── Cargo.toml
└── src/
    ├── lib.rs           ← public API: ObservabilityHandle, init, shutdown
    ├── tracer.rs        ← OTel SDK setup, BatchSpanProcessor, runtime
    ├── attributes.rs    ← gen_ai.* + langfuse.* attribute helpers
    ├── redaction.rs     ← content redaction policy
    └── tests/
        └── trace_shape.rs  ← mock OTLP server, validate span tree
```

## Configuration surface

User-facing config (all opt-in; default OFF):

```toml
# ~/.puffer/config.toml
[observability]
enabled       = false                    # opt-in
endpoint      = "http://localhost:3000"
public_key    = "pk-lf-..."
secret_key    = "sk-lf-..."
service_name  = "puffer"
sample_rate   = 1.0
include_content = true                   # set false to redact prompt/output
```

OR standard OTel env vars (also recognized):

```
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:3000/api/public/otel
OTEL_EXPORTER_OTLP_HEADERS=Authorization=Basic <base64(pk:sk)>
PUFFER_OBSERVABILITY=1
```

If both are set, env vars win (matches OTel convention).

## Code-level mounting points

| Existing code path | OTel hook |
|---|---|
| `execute_user_prompt_streaming_*` entry | start root span `agent_loop`, attach session_id |
| `agent_loop::run_streaming_loop` per iteration | child span `turn-<N>` |
| `TurnSession::one_turn_streaming` | child span `provider_call`; emit token usage from `Usage` event |
| `execute_tool_batch` per tool | child span `tool.<id>`; record input + output |
| `compact_conversation_with` | child span `compaction` |
| `TurnStreamEvent::Usage` | write usage to enclosing `provider_call` span |
| `CancelToken.cancel()` | mark span status = aborted |

The mounting is done by passing an `&Option<ObservabilityHandle>`
through `LoopInputs` (similar to `cancel: Option<&CancelToken>`).
When `None`, every hook becomes a noop in seconds.

## Attribute schema

Use **OpenTelemetry GenAI Semantic Conventions** (`gen_ai.*`):
- `gen_ai.system` ∈ `openai`, `anthropic`, `kimi`, `xai`, ...
- `gen_ai.request.model`
- `gen_ai.request.temperature` / `top_p` / `max_tokens`
- `gen_ai.usage.input_tokens`
- `gen_ai.usage.output_tokens`
- `gen_ai.usage.cache_read_input_tokens`
- `gen_ai.response.finish_reasons` = `["stop"]` etc.

Langfuse-specific extensions (`langfuse.*`):
- `langfuse.session.id`
- `langfuse.observation.type` = `generation` | `span` | `event`
- `langfuse.user.id` (when puffer learns user id from auth)

Puffer-specific (`puffer.*`):
- `puffer.cwd`
- `puffer.session.id`
- `puffer.provider.id`
- `puffer.api`
- `puffer.tool.call_id`
- `puffer.tool.parallel`
- `puffer.compaction.phase`
- `puffer.cancel.observed`

## Lifecycle & threading

- BatchSpanProcessor uses a dedicated single-thread tokio runtime
  isolated from the main blocking runtime (puffer is reqwest::blocking).
- On process exit: `force_flush()` + `shutdown()` so buffered spans
  reach Langfuse before exit. Wire into existing
  `puffer_core::shutdown_runtime_services()`.
- On cancel: still flush; mark in-flight span as `aborted`.

## Redaction

Tool outputs may include sensitive bytes (`Read /etc/passwd`,
secrets in env vars, etc.). Default policy:
- Always send: tool name, success, duration_ms.
- Conditionally send (when `include_content = true`): full input + output.
- Hard cap on per-attribute size (4 KiB) to keep spans bounded.
- Tool ids on a denylist (`AuthSetApiKey`, `Email`, `Telegram`, legacy
  `EmailConfigure`) always
  redact regardless of flag.

## Performance budget

- Disabled: 0 syscalls, 0 allocations.
- Enabled: ≤ 1 ms overhead per span (OTel SDK is well-tuned).
- Network: BatchSpanProcessor with 5-second flush interval, max 512
  spans per batch. Langfuse rate-limits at thousands per minute.

## Test plan

1. **Unit:** `puffer-observability::tracer` builds a TracerProvider with
   a mock OTLP receiver; emit one span; assert receiver got the
   expected attribute set.
2. **Integration:** mock `mockito` HTTP server pretending to be
   Langfuse OTLP; run a fake agent_loop turn; assert trace tree shape
   (1 root + N expected children).
3. **Live:** `docker compose up langfuse` (Postgres + Langfuse server),
   run `hanbbq/gpt-5.5` multi-turn flow, manually inspect Langfuse UI
   for the resulting trace.
4. **Cancel:** ESC during a turn; verify the span is closed with
   status `aborted` and partial attributes are visible.

## Migration & backwards compatibility

- New crate; no existing crate signature changes.
- `LoopInputs` gains an `observability: Option<&ObservabilityHandle>`
  field — additive (other call sites pass `None`).
- All existing tests run with observability `None` → unaffected.

## Out of scope (follow-ups)

- Logs export via OTLP `/v1/logs` (current focus is traces only).
- Metrics export via OTLP `/v1/metrics` (per-tool histograms etc.).
- Multi-tenant: per-`session_id` Langfuse project routing.
- LLM-judge / reflection traces piped as nested spans (we already
  emit `ReflectionTrace` events; just need to attach them).
- A `puffer trace export <session-id>` CLI to push historical sessions.
