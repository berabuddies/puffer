//! Single source of truth for every environment variable Puffer reads.
//!
//! Modeled on Claude Code's `managedEnvConstants.ts` pattern: one file
//! enumerating every env var the binary consults, with a description and
//! a citation pointing back to the read site. Future PRs use this for
//! documentation generation, validation, and a `--show-env` debug
//! command.
//!
//! ## Adding a new env var
//!
//! 1. Implement the `std::env::var(...)` read at the call site.
//! 2. Add an entry in [`ALL_ENV_VARS`] under the appropriate section
//!    comment, sorted alphabetically inside that section. Include the
//!    `crate::file:line` of every place it is read.
//! 3. The unit tests in this module enforce uniqueness and a non-empty
//!    description so drift surfaces in CI rather than at debug time.
//!
//! ## What this file is NOT
//!
//! - Not a runtime gate. Reads still happen at their original call site.
//!   This is documentation + future tooling input, not an interception
//!   layer. (CC's `managedEnvConstants.ts` doubles as a security gate
//!   for managed-settings; we don't yet have managed settings, so this
//!   file is descriptive only.)
//! - Not a list of every env var Puffer **exports** to subprocesses.
//!   Hooks, external tools, subscribers, and the PTY inherit ambient
//!   env and additionally see `PUFFER_TOOL_*`, `PUFFER_HOOK_*`,
//!   `PUFFER_TURN_*`, `PUFFER_SKILL_*`, `PUFFER_EDITOR_TARGET`. Those
//!   are documented at the export site (e.g.
//!   `crates/puffer-core/runtime/hook_support.rs`) and not enumerated
//!   here unless Puffer itself also reads them back.

/// One row of the env var registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvVar {
    /// The exact env var name as read at the call site.
    pub name: &'static str,
    /// One-line human-readable purpose. Should describe behavior, not
    /// internals.
    pub description: &'static str,
    /// Default value baked into code (`None` when the variable is purely
    /// optional / off when unset).
    pub default: Option<&'static str>,
    /// `crate::file:line` citations for every read site. Multi-element
    /// when the same var is read from several places.
    pub source_files: &'static [&'static str],
}

/// Every env var Puffer reads. Sections are organized by purpose; entries
/// inside each section are sorted alphabetically.
///
/// **Coverage policy**: this list aims to be exhaustive for first-party
/// reads (i.e. `std::env::var` / `std::env::var_os` / `env::var` calls
/// in `crates/`). It deliberately excludes:
///
/// - Tests-only reads (`#[cfg(test)]`, `crates/*/tests/`, `examples/`).
/// - Process-bulk inheritance (e.g. `daemon_pty.rs` forwarding all of
///   `std::env::vars_os()` into the user's shell).
/// - Build-time `env!` macro reads (e.g. `CARGO_MANIFEST_DIR`,
///   `CARGO_PKG_VERSION`) — those are compile-time and don't need a
///   runtime registry.
/// - Dynamically-named reads from user config (e.g.
///   `RemoteRunnerConfig::auth_token_env`,
///   `provider.env_http_headers`, `PUFFER_LSP_COMMAND_<NAME>` per-LSP
///   override) — those are user-supplied names, not fixed identifiers.
pub const ALL_ENV_VARS: &[EnvVar] = &[
    // -----------------------------------------------------------------
    // Provider routing & auth
    // -----------------------------------------------------------------
    EnvVar {
        name: "ANTHROPIC_BASE_URL",
        description:
            "Currently unused as a direct env read; documented here for parity with CC and as a \
             reservation. Anthropic transport reads the base URL from the provider registry, not \
             this env var.",
        default: None,
        source_files: &[],
    },
    EnvVar {
        name: "API_TIMEOUT_MS",
        description:
            "Stainless-style HTTP timeout (milliseconds) reported in the Anthropic stainless \
             header. Clamped via `/1000` for the header value; does not actually gate the \
             reqwest timeout.",
        default: Some("600000"),
        source_files: &["puffer-transport-anthropic/src/request.rs:290"],
    },
    EnvVar {
        name: "CLAUDE_CODE_ORGANIZATION_UUID",
        description:
            "Active Claude organization UUID for session-ingress auth (managed-agent runtime). \
             Paired with `CLAUDE_CODE_SESSION_ACCESS_TOKEN`.",
        default: None,
        source_files: &["puffer-transport-anthropic/src/auth.rs:54,371"],
    },
    EnvVar {
        name: "CLAUDE_CODE_SESSION_ACCESS_TOKEN",
        description:
            "Inline session-ingress access token used by the Anthropic transport when running \
             under Claude Code's managed-agent host.",
        default: None,
        source_files: &["puffer-transport-anthropic/src/auth.rs:41,366"],
    },
    EnvVar {
        name: "CLAUDE_CODE_WEBSOCKET_AUTH_FILE_DESCRIPTOR",
        description:
            "File-descriptor number containing a session-ingress access token. Read when the \
             token is passed via `fd:` rather than env or file.",
        default: None,
        source_files: &["puffer-transport-anthropic/src/auth.rs:44,381"],
    },
    EnvVar {
        name: "CLAUDE_SESSION_INGRESS_TOKEN_FILE",
        description: "Path to a file holding a session-ingress access token. Falls back to \
             `DEFAULT_SESSION_ACCESS_TOKEN_PATH` when unset.",
        default: Some("/home/claude/.claude/remote/.session_ingress_token"),
        source_files: &["puffer-transport-anthropic/src/auth.rs:47,395"],
    },
    EnvVar {
        name: "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
        description: "Overrides the `originator` header / user-agent prefix sent on OpenAI-codex \
             requests. Defaults to `codex_cli_rs`.",
        default: Some("codex_cli_rs"),
        source_files: &["puffer-provider-openai/src/codex.rs:4,8"],
    },
    EnvVar {
        name: "CODEX_REFRESH_TOKEN_URL_OVERRIDE",
        description:
            "Overrides the OpenAI-codex OAuth refresh-token URL. Used to point at a staging or \
             test endpoint without code changes.",
        default: None,
        source_files: &["puffer-provider-openai/src/auth.rs:20,214"],
    },
    EnvVar {
        name: "OPENAI_API_KEY",
        description: "OpenAI API key. In benchmark mode, takes priority over imported codex \
             credentials and is written into the in-memory auth store.",
        default: None,
        source_files: &["puffer-cli/src/benchmark_run.rs:487"],
    },
    EnvVar {
        name: "OPENAI_BASE_URL",
        description:
            "Custom base URL for OpenAI / proxy / private endpoint. In benchmark mode, applied \
             to the provider registry as an override before model registration.",
        default: None,
        source_files: &["puffer-cli/src/benchmark_run.rs:117"],
    },
    EnvVar {
        name: "OPENAI_ORGANIZATION",
        description:
            "Forwarded as the `OpenAI-Organization` request header for OpenAI / Responses API \
             calls when no explicit header is configured.",
        default: None,
        source_files: &["puffer-core/runtime/openai/support.rs:290,346-355"],
    },
    EnvVar {
        name: "OPENAI_PROJECT",
        description:
            "Forwarded as the `OpenAI-Project` request header for OpenAI / Responses API calls \
             when no explicit header is configured.",
        default: None,
        source_files: &["puffer-core/runtime/openai/support.rs:291,346-355"],
    },
    // -----------------------------------------------------------------
    // HTTP retry & transport
    // -----------------------------------------------------------------
    EnvVar {
        name: "PUFFER_HTTP_RETRY_ATTEMPTS",
        description:
            "Number of HTTP retry attempts for the generic transport retry wrapper. Clamped to \
             [0, 10]. Used by Anthropic + generic provider paths.",
        default: Some("3"),
        source_files: &["puffer-core/runtime.rs:138,855-859"],
    },
    EnvVar {
        name: "PUFFER_HTTP_RETRY_DELAY_MS",
        description: "Linear-backoff base delay (ms) between HTTP retries. Clamped to [0, 30000]. \
             Effective delay = base * attempt index.",
        default: Some("1000"),
        source_files: &["puffer-core/runtime.rs:139,860-862"],
    },
    EnvVar {
        name: "PUFFER_HTTP_TRACE_PATH",
        description:
            "When set to a writable path, every HTTP request and response (Anthropic + OpenAI \
             SSE + retries) is appended to this file. Authorization headers are redacted.",
        default: None,
        source_files: &[
            "puffer-core/runtime.rs:907,939,953",
            "puffer-core/runtime/anthropic.rs:662",
            "puffer-core/runtime/openai/support.rs:371,404",
        ],
    },
    EnvVar {
        name: "PUFFER_OPENAI_HTTP_MAX_ATTEMPTS",
        description: "Max retry attempts for the OpenAI Responses transport (separate from \
             `PUFFER_HTTP_RETRY_ATTEMPTS`). Clamped to [1, 5].",
        default: Some("3"),
        source_files: &["puffer-core/runtime/openai/support.rs:122-128"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_HTTP_RETRY_DELAY_MS",
        description:
            "Delay (ms) between OpenAI Responses transport retries. Clamped to [0, 10000].",
        default: Some("1000"),
        source_files: &["puffer-core/runtime/openai/support.rs:131-136"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_SSE_TRACE_PATH",
        description:
            "When set, OpenAI SSE event stream is appended to this file (one line per parsed \
             event). Distinct from PUFFER_HTTP_TRACE_PATH which captures whole \
             request/response.",
        default: None,
        source_files: &["puffer-core/runtime/openai_sse.rs:136"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_STREAM_READ_TIMEOUT_MS",
        description: "Per-read SSE idle timeout (ms) for the OpenAI Responses stream. Clamped to \
             [1000, 300000].",
        default: Some("180000"),
        source_files: &["puffer-core/runtime/openai/support.rs:140-144"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_WEBSOCKET",
        description:
            "When truthy, switches the OpenAI Responses transport to a persistent WebSocket \
             connection (with SSE fallback on connection failure). Truthy = `1` or `true` \
             case-insensitive.",
        default: None,
        source_files: &["puffer-core/runtime/openai/websocket.rs:36"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_WS_MAX_ATTEMPTS",
        description:
            "Max reconnect attempts for the OpenAI WebSocket transport. Clamped to [1, 5].",
        default: Some("3"),
        source_files: &["puffer-core/runtime/openai/websocket.rs:590"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_WS_RETRY_DELAY_MS",
        description: "Delay (ms) between WebSocket reconnect attempts. Clamped to [0, 10000].",
        default: Some("1000"),
        source_files: &["puffer-core/runtime/openai/websocket.rs:599"],
    },
    // -----------------------------------------------------------------
    // OpenAI/Codex behavior toggles
    // -----------------------------------------------------------------
    EnvVar {
        name: "PUFFER_OPENAI_DISABLE_CODEX_STYLE",
        description:
            "When `1`, force-disables codex-style request shaping for OpenAI providers even \
             when the provider would otherwise be detected as codex.",
        default: None,
        source_files: &["puffer-core/runtime/openai.rs:908"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_DISABLE_PREVIOUS_RESPONSE_ID",
        description: "Legacy alias for `PUFFER_OPENAI_DISABLE_RESPONSE_THREADING`. Disables \
             `previous_response_id` chaining on the OpenAI Responses API.",
        default: None,
        source_files: &["puffer-core/runtime/openai/support.rs:204-208"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_DISABLE_REASONING",
        description:
            "When truthy, drops the `reasoning` block from OpenAI Responses requests. Used to \
             work around providers that reject reasoning configs. Truthy = `1`/`true`.",
        default: None,
        source_files: &["puffer-core/runtime/openai/support.rs:453-458"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_DISABLE_RESPONSE_THREADING",
        description:
            "When truthy, disables the `previous_response_id` threading optimization (forces \
             every turn to send full input). Useful when a proxy strips response IDs.",
        default: None,
        source_files: &["puffer-core/runtime/openai/support.rs:204-208"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_ENABLE_CUSTOM_RESPONSE_THREADING",
        description: "When truthy, force-enables response threading for providers that aren't \
             auto-detected as supporting it. Overrides URL-based detection.",
        default: None,
        source_files: &["puffer-core/runtime/openai/support.rs:209-211"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_NATIVE_WEB_SEARCH",
        description:
            "When `0` / `false`, suppresses native OpenAI web_search tool injection (forcing \
             the puffer-side WebSearch fallback). Any other value (or unset) leaves it \
             enabled when the provider supports it.",
        default: None,
        source_files: &["puffer-core/runtime/structured_output_support.rs:160-167"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_STORE_RESPONSES",
        description:
            "When `1` or `true` (case-insensitive), sets `store: true` on OpenAI Responses \
             requests. Default is `false` to keep server-side state opt-in.",
        default: None,
        source_files: &["puffer-core/runtime/openai/support.rs:33-35"],
    },
    EnvVar {
        name: "PUFFER_OPENAI_WEB_SEARCH_MODE",
        description:
            "Selects native OpenAI web_search access mode. `cached` → external_web_access=false; \
             anything else (or unset) → external_web_access=true (live web).",
        default: Some("live"),
        source_files: &["puffer-core/runtime/structured_output_support.rs:179"],
    },
    // -----------------------------------------------------------------
    // System prompts & reflection / judge
    // -----------------------------------------------------------------
    EnvVar {
        name: "PUFFER_BENCHMARK_LLM_JUDGE_EFFORT",
        description:
            "Effort level for the benchmark reflection LLM judge (`low`, `medium`, `high`, \
             ...). Defaults to `low`.",
        default: Some("low"),
        source_files: &["puffer-cli/src/benchmark_reflection.rs:16"],
    },
    EnvVar {
        name: "PUFFER_BENCHMARK_LLM_JUDGE_MODEL",
        description:
            "Model selector (provider/model) for the benchmark reflection LLM judge. Defaults \
             to the same model the agent is running.",
        default: None,
        source_files: &["puffer-cli/src/benchmark_reflection.rs:9"],
    },
    EnvVar {
        name: "PUFFER_SYSTEM_PROMPT_1",
        description:
            "When set non-empty, this string is inserted as the very first system message of \
             every conversation (managed-host system prompt slot).",
        default: None,
        source_files: &["puffer-core/runtime/openai/conversation.rs:482"],
    },
    // -----------------------------------------------------------------
    // Microcompact (PR #75 in-place context pruning)
    // -----------------------------------------------------------------
    EnvVar {
        name: "PUFFER_MICROCOMPACT",
        description:
            "Enables in-place tool_result content clearing (microcompact pass). Truthy = `1` \
             or `true` case-insensitive. Off by default to match Claude Code's GrowthBook \
             default.",
        default: None,
        source_files: &["puffer-core/runtime/microcompact.rs:97"],
    },
    EnvVar {
        name: "PUFFER_MICROCOMPACT_GAP_MINUTES",
        description: "Time-gap trigger (minutes) for the microcompact pass.",
        default: Some("60"),
        source_files: &["puffer-core/runtime/microcompact.rs:109"],
    },
    EnvVar {
        name: "PUFFER_MICROCOMPACT_KEEP_RECENT",
        description:
            "Number of recent tool_result entries to leave un-cleared during a microcompact \
             pass.",
        default: Some("5"),
        source_files: &["puffer-core/runtime/microcompact.rs:104"],
    },
    EnvVar {
        name: "PUFFER_MICROCOMPACT_TOKEN_THRESHOLD",
        description:
            "When set, also triggers microcompact once context exceeds this many tokens (in \
             addition to the time-gap trigger).",
        default: None,
        source_files: &["puffer-core/runtime/microcompact.rs:114"],
    },
    // -----------------------------------------------------------------
    // Observability (OTel + Langfuse)
    // -----------------------------------------------------------------
    EnvVar {
        name: "LANGFUSE_PUBLIC_KEY",
        description:
            "Langfuse public key. When paired with `LANGFUSE_SECRET_KEY`, an `Authorization: \
             Basic <base64>` header is added to OTLP exports.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:344"],
    },
    EnvVar {
        name: "LANGFUSE_SECRET_KEY",
        description: "Langfuse secret key. See `LANGFUSE_PUBLIC_KEY`.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:345"],
    },
    EnvVar {
        name: "LANGFUSE_USER_ID",
        description: "Fallback user-id for the Langfuse Users tab when \
             `PUFFER_OBSERVABILITY_USER_ID` is unset.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:395"],
    },
    EnvVar {
        name: "OTEL_DEPLOYMENT_ENVIRONMENT",
        description: "Standard OTel deployment environment label. Used as fallback when \
             `PUFFER_OBSERVABILITY_ENVIRONMENT` is unset.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:377"],
    },
    EnvVar {
        name: "OTEL_EXPORTER_OTLP_ENDPOINT",
        description:
            "OTLP endpoint for tracing/metrics export. When unset (or empty), observability \
             initialization is skipped entirely.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:335"],
    },
    EnvVar {
        name: "OTEL_EXPORTER_OTLP_HEADERS",
        description: "Comma-separated `key=value` pairs forwarded as headers on OTLP exports.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:339"],
    },
    EnvVar {
        name: "OTEL_SERVICE_NAME",
        description: "Service name attribute on exported OTel resources. Defaults to `puffer`.",
        default: Some("puffer"),
        source_files: &["puffer-observability/src/lib.rs:358"],
    },
    EnvVar {
        name: "PUFFER_OBSERVABILITY_ENVIRONMENT",
        description: "Deployment environment label (overrides `OTEL_DEPLOYMENT_ENVIRONMENT`).",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:375"],
    },
    EnvVar {
        name: "PUFFER_OBSERVABILITY_INCLUDE_CONTENT",
        description: "Legacy all-on toggle: when truthy, sets all three include_* flags to true. \
             Truthy = `1`, `true`, or `yes`.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:363"],
    },
    EnvVar {
        name: "PUFFER_OBSERVABILITY_INCLUDE_OUTPUTS",
        description: "When truthy, includes model output text in OTel spans (off by default for \
             privacy). Truthy = `1`, `true`, or `yes`.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:372"],
    },
    EnvVar {
        name: "PUFFER_OBSERVABILITY_INCLUDE_PROMPTS",
        description:
            "When truthy, includes prompt content in OTel spans. Truthy = `1`, `true`, or \
             `yes`.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:371"],
    },
    EnvVar {
        name: "PUFFER_OBSERVABILITY_INCLUDE_TOOL_IO",
        description: "When truthy, includes tool input/output bodies in OTel spans. Truthy = `1`, \
             `true`, or `yes`.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:373"],
    },
    EnvVar {
        name: "PUFFER_OBSERVABILITY_RELEASE",
        description: "Release / version label attached to OTel resources for grouping in Langfuse.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:374"],
    },
    EnvVar {
        name: "PUFFER_OBSERVABILITY_SAMPLE_RATE",
        description:
            "Float in [0.0, 1.0] sampled into the OTel tracer. Defaults to 1.0 (sample all).",
        default: Some("1.0"),
        source_files: &["puffer-observability/src/lib.rs:359"],
    },
    EnvVar {
        name: "PUFFER_OBSERVABILITY_TAGS",
        description:
            "Comma-separated tag list attached to spans (after trim, empty entries dropped).",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:379"],
    },
    EnvVar {
        name: "PUFFER_OBSERVABILITY_USER_ID",
        description:
            "Explicit user-id for OTel/Langfuse spans. Highest-priority slot in the chain \
             `PUFFER_OBSERVABILITY_USER_ID` -> `LANGFUSE_USER_ID` -> `USER` -> `USERNAME`.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:393"],
    },
    // -----------------------------------------------------------------
    // Heartbeat (puffer-cli daemon health pings)
    // -----------------------------------------------------------------
    EnvVar {
        name: "PUFFER_HEARTBEAT_ENABLED",
        description:
            "When truthy, the daemon spawns a worker thread that pings `PUFFER_HEARTBEAT_URL` \
             on an interval.",
        default: None,
        source_files: &["puffer-cli/src/heartbeat.rs:8"],
    },
    EnvVar {
        name: "PUFFER_HEARTBEAT_INTERVAL_SECONDS",
        description: "Heartbeat ping interval (seconds). Floor is 5s; default 60s.",
        default: Some("60"),
        source_files: &["puffer-cli/src/heartbeat.rs:10"],
    },
    EnvVar {
        name: "PUFFER_HEARTBEAT_URL",
        description: "URL the heartbeat worker POSTs to.",
        default: None,
        source_files: &["puffer-cli/src/heartbeat.rs:9"],
    },
    // -----------------------------------------------------------------
    // Paths, config, & resources
    // -----------------------------------------------------------------
    EnvVar {
        name: "PUFFER_BIN",
        description: "Path to the `puffer` binary itself. Used by the subscriber supervisor when \
             spawning skill processes that re-invoke `puffer ...` as their command.",
        default: None,
        source_files: &["puffer-subscriber-runtime/src/supervisor.rs:279"],
    },
    EnvVar {
        name: "PUFFER_BUILTIN_RESOURCES_DIR",
        description:
            "Override path to the built-in resources directory (skills, mascot, etc.). When \
             set, the daemon-browser client also avoids the auto-resolution path.",
        default: None,
        source_files: &[
            "puffer-config/src/lib.rs:12",
            "puffer-cli/src/daemon_browser/client.rs:147,221",
        ],
    },
    EnvVar {
        name: "PUFFER_CHROME",
        description: "Override path to the Chrome / Chromium executable used by the managed-agent \
             browser runtime. When unset, falls back to platform-specific candidate paths.",
        default: None,
        source_files: &["puffer-cli/src/daemon_browser/chrome.rs:122"],
    },
    EnvVar {
        name: "PUFFER_DISCOVERY_CACHE_PATH",
        description: "Override path for the model-discovery cache JSON. Defaults to \
             `~/.puffer/model_discovery_cache.json`.",
        default: Some("~/.puffer/model_discovery_cache.json"),
        source_files: &["puffer-provider-registry/src/registry.rs:364"],
    },
    EnvVar {
        name: "PUFFER_HOME",
        description: "Override for the user-config root (analogous to `CLAUDE_HOME`). Defaults to \
             `$HOME/.puffer`. Read in several places to keep test isolation possible.",
        default: Some("$HOME/.puffer"),
        source_files: &[
            "puffer-config/src/lib.rs:152",
            "puffer-provider-registry/src/import.rs:56",
            "puffer-provider-registry/src/secure_oauth.rs:303",
        ],
    },
    EnvVar {
        name: "PUFFER_REMOTE_SESSION_URL",
        description:
            "Explicit remote session URL override. Highest-priority slot before consulting \
             `state.remote_name`-based resolution.",
        default: None,
        source_files: &["puffer-core/command_helpers/session.rs:678"],
    },
    EnvVar {
        name: "PUFFER_RESOURCES_DIR",
        description: "Override path to the bundled-resources root used by `puffer subscriptions`. \
             Defaults to `./resources` relative to cwd.",
        default: Some("./resources"),
        source_files: &["puffer-cli/src/subscriptions.rs:266"],
    },
    // -----------------------------------------------------------------
    // Subscribers / skills
    // -----------------------------------------------------------------
    EnvVar {
        name: "PUFFER_SKILL_STATE_DIR",
        description: "Persistent state dir for a running subscriber/skill process. Set by the \
             supervisor; read inside subscriber binaries (email, telegram-user).",
        default: None,
        source_files: &[
            "puffer-subscriber-email/src/run.rs:38",
            "puffer-subscriber-telegram-user/src/state.rs:37",
        ],
    },
    EnvVar {
        name: "PUFFER_SKILL_TOPIC",
        description: "Event-bus topic for the running subscriber. Set by the supervisor; read by \
             subscriber binaries when emitting envelopes.",
        default: None,
        source_files: &[
            "puffer-subscriber-email/src/run.rs:42",
            "puffer-subscriber-telegram-user/src/state.rs:42",
        ],
    },
    EnvVar {
        name: "PUFFER_TELEGRAM_API_HASH",
        description: "Telegram MTProto api_hash for the user-mode subscriber.",
        default: None,
        source_files: &["puffer-subscriber-telegram-user/src/state.rs:128"],
    },
    EnvVar {
        name: "PUFFER_TELEGRAM_API_ID",
        description: "Telegram MTProto api_id for the user-mode subscriber. Parsed as integer.",
        default: None,
        source_files: &["puffer-subscriber-telegram-user/src/state.rs:125"],
    },
    // -----------------------------------------------------------------
    // Tooling auth / runner
    // -----------------------------------------------------------------
    EnvVar {
        name: "PUFFER_NO_BROWSER",
        description:
            "When truthy, suppresses the built-in Browser tool registration. Truthy values \
             include `1`, `true`, `yes`, `on`. Set automatically by the local daemon when no \
             browser runtime is available.",
        default: None,
        source_files: &[
            "puffer-resources/src/loader.rs:152",
            "puffer-tools/src/registry.rs:269",
            "puffer-cli/src/daemon.rs:142,144",
        ],
    },
    EnvVar {
        name: "PUFFER_RUNNER_TOKEN",
        description:
            "Bearer token expected by the standalone `puffer-tool-runner` binary on its gRPC \
             auth interceptor. Mutually exclusive with `--token` flag.",
        default: None,
        source_files: &["puffer-tool-runner/src/main.rs:36"],
    },
    EnvVar {
        name: "PUFFER_STAINLESS_RUNTIME_VERSION",
        description: "Stainless `runtime` version reported in the Anthropic stainless header.",
        default: Some("v24.3.0"),
        source_files: &["puffer-transport-anthropic/src/request.rs:286"],
    },
    // -----------------------------------------------------------------
    // OAuth storage / test escape hatches
    // -----------------------------------------------------------------
    EnvVar {
        name: "PUFFER_TEST_PLAINTEXT_OAUTH_STORAGE",
        description:
            "When set (any value), forces puffer-provider-registry to use the plaintext-disk \
             OAuth backend instead of the macOS keychain. Used by tests on macOS hosts.",
        default: None,
        source_files: &["puffer-provider-registry/src/secure_oauth.rs:12,159"],
    },
    // -----------------------------------------------------------------
    // Benchmark
    // -----------------------------------------------------------------
    EnvVar {
        name: "PUFFER_BENCHMARK_WORKING_DIRS",
        description:
            "Colon-separated list of pre-created working directories the benchmark runner \
             reuses (avoids tempdir churn under high-throughput suites).",
        default: None,
        source_files: &["puffer-cli/src/benchmark_run.rs:26,775,818"],
    },
    // -----------------------------------------------------------------
    // Generic Unix / shell / editor / terminal — reads only
    // -----------------------------------------------------------------
    EnvVar {
        name: "ALACRITTY_LOG",
        description:
            "Presence-only check used by terminal-detection to identify the Alacritty terminal \
             emulator.",
        default: None,
        source_files: &["puffer-core/command_helpers/terminal_setup.rs:410"],
    },
    EnvVar {
        name: "COLORTERM",
        description: "Read by the TUI to decide whether the terminal supports truecolor (24-bit) \
             output. Matched against `truecolor` / `24bit`.",
        default: None,
        source_files: &["puffer-tui/src/render/top_panel.rs:345"],
    },
    EnvVar {
        name: "COMSPEC",
        description: "Windows-only fallback for the daemon PTY when `$SHELL` is unset.",
        default: None,
        source_files: &["puffer-cli/src/daemon_pty.rs:517"],
    },
    EnvVar {
        name: "CURSOR_TRACE_ID",
        description: "Presence-only check used by terminal-setup to detect the Cursor terminal.",
        default: None,
        source_files: &["puffer-core/command_helpers/terminal_setup.rs:377"],
    },
    EnvVar {
        name: "EDITOR",
        description:
            "Standard fallback editor command (after `VISUAL`). Used by `puffer doctor` and \
             the prompt-editor flow.",
        default: None,
        source_files: &[
            "puffer-core/command_helpers/doctor.rs:363",
            "puffer-core/command_helpers/prompt.rs:507",
            "puffer-core/command_helpers/common.rs:732",
        ],
    },
    EnvVar {
        name: "GHOSTTY_RESOURCES_DIR",
        description: "Presence-only check used by codex User-Agent generation to identify Ghostty.",
        default: None,
        source_files: &["puffer-provider-openai/src/codex.rs:57"],
    },
    EnvVar {
        name: "HOME",
        description: "User home directory. Read in many path-resolution sites (config, sessions, \
             tilde-expansion, OAuth keychain hashing). Linux/macOS-canonical.",
        default: None,
        source_files: &[
            "puffer-cli/src/daemon_files.rs:268",
            "puffer-cli/src/benchmark_run.rs:174",
            "puffer-config/src/lib.rs:154",
            "puffer-core/workspace_paths.rs:229,235",
            "puffer-core/permissions.rs:472,477",
            "puffer-core/runtime/request_tool_filter.rs:206,209",
            "puffer-core/runtime/claude_tools/workflow/email_configure.rs:95",
            "puffer-core/runtime/claude_tools/workflow/subscriber_install.rs:63",
            "puffer-core/runtime/claude_tools/workflow/subscriber_list.rs:71",
            "puffer-core/runtime/claude_tools/workflow/subscriber_scaffold.rs:99",
            "puffer-core/runtime/claude_tools/workflow/subscription_create.rs:88",
            "puffer-core/runtime/claude_tools/workflow/telegram_login.rs:236",
            "puffer-subscriber-telegram-user/src/import.rs:90,103",
            "puffer-provider-registry/src/import.rs:58",
            "puffer-subscriptions/src/action.rs:278",
        ],
    },
    EnvVar {
        name: "ITERM_PROFILE",
        description: "Presence-only check used by codex User-Agent generation to identify iTerm2.",
        default: None,
        source_files: &["puffer-provider-openai/src/codex.rs:54"],
    },
    EnvVar {
        name: "ITERM_SESSION_ID",
        description: "Presence-only check used by codex User-Agent generation to identify iTerm2.",
        default: None,
        source_files: &["puffer-provider-openai/src/codex.rs:54"],
    },
    EnvVar {
        name: "KITTY_PID",
        description: "Presence-only check used by codex User-Agent generation to identify Kitty.",
        default: None,
        source_files: &["puffer-provider-openai/src/codex.rs:51"],
    },
    EnvVar {
        name: "LOCALAPPDATA",
        description: "Windows: candidate base path when looking for Chrome / Chromium installs \
             and the default Telegram Desktop tdata directory.",
        default: None,
        source_files: &[
            "puffer-cli/src/daemon_browser/chrome.rs:160",
            "puffer-subscriber-telegram-user/src/import.rs:113",
        ],
    },
    EnvVar {
        name: "PATH",
        description:
            "Read for: locating `command` executables (LSP live, chrome candidates), and the \
             VSCode-remote-SSH heuristic in terminal-setup.",
        default: None,
        source_files: &[
            "puffer-cli/src/daemon_browser/chrome.rs:233",
            "puffer-core/runtime/claude_tools/workflow/lsp_live.rs:709",
            "puffer-core/command_helpers/terminal_setup.rs:471",
        ],
    },
    EnvVar {
        name: "PROGRAMFILES",
        description: "Windows: candidate base path when looking for Chrome / Chromium installs.",
        default: None,
        source_files: &["puffer-cli/src/daemon_browser/chrome.rs:160"],
    },
    EnvVar {
        name: "PROGRAMFILES(X86)",
        description:
            "Windows: candidate base path when looking for 32-bit Chrome / Chromium installs.",
        default: None,
        source_files: &["puffer-cli/src/daemon_browser/chrome.rs:160"],
    },
    EnvVar {
        name: "SHELL",
        description: "User's preferred shell. Used by: bash builtin tool detection, system-prompt \
             shell-info line, daemon PTY shell resolution.",
        default: None,
        source_files: &[
            "puffer-tools/src/builtins.rs:17",
            "puffer-cli/src/daemon_pty.rs:510",
            "puffer-core/runtime/system_prompt.rs:412",
        ],
    },
    EnvVar {
        name: "STY",
        description: "Presence-only check used by terminal-setup to identify GNU screen.",
        default: None,
        source_files: &["puffer-core/command_helpers/terminal_setup.rs:416"],
    },
    EnvVar {
        name: "TERM",
        description:
            "Terminal type. Read by the TUI for truecolor detection and by terminal-setup for \
             ghostty/kitty/alacritty identification.",
        default: None,
        source_files: &[
            "puffer-tui/src/render/top_panel.rs:348",
            "puffer-core/command_helpers/terminal_setup.rs:389,392,420",
            "puffer-provider-openai/src/codex.rs:60",
        ],
    },
    EnvVar {
        name: "TERM_PROGRAM",
        description:
            "Terminal-emulator identifier (Apple_Terminal, vscode, cursor, ...). Used for \
             terminal-setup branch logic and the codex User-Agent.",
        default: None,
        source_files: &[
            "puffer-core/command_helpers/terminal_setup.rs:400",
            "puffer-provider-openai/src/codex.rs:41",
        ],
    },
    EnvVar {
        name: "TERM_PROGRAM_VERSION",
        description: "Companion to `TERM_PROGRAM` — used in the codex User-Agent token \
             (`<program>/<version>`).",
        default: None,
        source_files: &["puffer-provider-openai/src/codex.rs:42"],
    },
    EnvVar {
        name: "TMUX",
        description: "Presence-only check used by terminal-setup to identify tmux.",
        default: None,
        source_files: &["puffer-core/command_helpers/terminal_setup.rs:413"],
    },
    EnvVar {
        name: "USER",
        description:
            "Unix user name. Used by: secure-OAuth keychain salt, observability user-id chain.",
        default: Some("puffer-code-user"),
        source_files: &[
            "puffer-provider-registry/src/secure_oauth.rs:294",
            "puffer-observability/src/lib.rs:396",
        ],
    },
    EnvVar {
        name: "USERNAME",
        description: "Windows-canonical user name. Used as the last-fallback user-id for \
             observability spans.",
        default: None,
        source_files: &["puffer-observability/src/lib.rs:397"],
    },
    EnvVar {
        name: "VISUAL",
        description:
            "Preferred visual editor (overrides `EDITOR`). Used by `puffer doctor` and the \
             prompt-editor flow.",
        default: None,
        source_files: &[
            "puffer-core/command_helpers/doctor.rs:364",
            "puffer-core/command_helpers/prompt.rs:505",
            "puffer-core/command_helpers/common.rs:727",
        ],
    },
    EnvVar {
        name: "VSCODE_GIT_ASKPASS_MAIN",
        description: "Read by terminal-setup to disambiguate VSCode-flavored terminals (Cursor, \
             Windsurf, vscode-server) by looking at the askpass binary path.",
        default: None,
        source_files: &["puffer-core/command_helpers/terminal_setup.rs:381,470"],
    },
    EnvVar {
        name: "WEZTERM_VERSION",
        description:
            "Presence-only / value used by codex User-Agent generation to identify WezTerm \
             and emit `wezterm/<version>`.",
        default: None,
        source_files: &["puffer-provider-openai/src/codex.rs:45"],
    },
    EnvVar {
        name: "WT_SESSION",
        description: "Presence-only check used by codex User-Agent generation to identify Windows \
             Terminal.",
        default: None,
        source_files: &["puffer-provider-openai/src/codex.rs:48"],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_env_vars_have_unique_names() {
        let mut seen: HashSet<&'static str> = HashSet::new();
        for entry in ALL_ENV_VARS {
            assert!(
                seen.insert(entry.name),
                "duplicate env var name in registry: {}",
                entry.name
            );
        }
    }

    #[test]
    fn all_env_vars_have_descriptions() {
        for entry in ALL_ENV_VARS {
            assert!(
                !entry.description.trim().is_empty(),
                "empty description for env var: {}",
                entry.name
            );
            // Names are uppercase identifiers (allow digits, underscores,
            // and parentheses for `PROGRAMFILES(X86)`).
            assert!(
                entry.name.chars().all(|c| c.is_ascii_uppercase()
                    || c.is_ascii_digit()
                    || c == '_'
                    || c == '('
                    || c == ')'),
                "non-uppercase env var name: {}",
                entry.name
            );
        }
    }
}
