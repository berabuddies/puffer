use crate::AppState;
use anyhow::{anyhow, bail, Result};
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_tools::{ToolExecutionResult, ToolRegistry};
#[cfg(test)]
#[allow(unused_imports)]
use puffer_transport_anthropic::{AnthropicAuth, AnthropicRequestConfig};
#[cfg(test)]
#[allow(unused_imports)]
use serde_json::json;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

mod agent_loop;
#[cfg(test)]
mod agent_runtime_tests;
mod agent_support;
mod agents;
mod anthropic;
mod anthropic_sse;
mod auto_approval_review;
pub mod background_tasks;
mod blocking_loop;
mod browser_auto_review;
pub mod claude_tools;
mod context_usage;
pub mod errors;
mod filesystem_access;
pub mod goals;
mod hook_support;
mod http_support;
pub(crate) mod internal_tool_permissions;
pub(crate) mod lambda_gate;
pub(crate) mod lambda_skill_activation;
pub(crate) mod lambda_tool;
mod local_tools;
pub mod mcp_discovery;
mod microcompact;
mod openai;
mod openai_sse;
mod openai_ws;
pub(crate) mod overflow;
mod permission_prompt;
mod plan_events;
pub(crate) mod process_store;
mod provider_adapter;
pub mod quota;
mod reflection;
mod request_tool_filter;
pub mod resource_watcher;
pub(crate) mod secrets;
mod side_question;
mod structured_output_support;
mod system_prompt;
pub mod teammate_loop;
mod tool_batch;
mod tool_executor;
mod tool_results;

mod debug_context;

/// Installs the process-wide subscription manager so workflow tools
/// can dispatch into it. See `claude_tools::workflow::subscription_globals`.
pub fn install_subscription_manager(
    manager: std::sync::Arc<puffer_subscriptions::SubscriptionManager>,
) -> Result<()> {
    claude_tools::workflow::subscription_globals::install(manager)
}

/// Returns the installed subscription manager, or an error.
pub fn subscription_manager() -> Result<std::sync::Arc<puffer_subscriptions::SubscriptionManager>> {
    claude_tools::workflow::subscription_globals::manager()
}

/// Installs a process-wide [`puffer_observability::ObservabilityHandle`]
/// so the streaming entrypoints can pick it up without every caller
/// plumbing it explicitly. Called once at app boot. Idempotent.
pub fn install_observability(handle: puffer_observability::ObservabilityHandle) {
    let mut slot = OBSERVABILITY_HANDLE
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    *slot = Some(handle);
}

/// Returns a clone of the installed observability handle, if any.
pub fn observability_handle() -> Option<puffer_observability::ObservabilityHandle> {
    OBSERVABILITY_HANDLE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

static OBSERVABILITY_HANDLE: std::sync::Mutex<Option<puffer_observability::ObservabilityHandle>> =
    std::sync::Mutex::new(None);
static ACTIVE_RUNTIME_WORK: AtomicUsize = AtomicUsize::new(0);

/// Returns true while Puffer is actively executing a prompt turn or spawned agent turn.
pub fn runtime_work_active() -> bool {
    ACTIVE_RUNTIME_WORK.load(Ordering::SeqCst) > 0
}

struct RuntimeWorkGuard;

impl RuntimeWorkGuard {
    fn enter() -> Self {
        ACTIVE_RUNTIME_WORK.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for RuntimeWorkGuard {
    fn drop(&mut self) {
        ACTIVE_RUNTIME_WORK.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
use self::anthropic::{anthropic_tool_schema, execute_anthropic_tool_calls};
pub use self::browser_auto_review::{
    browser_auto_review_runtime_result_from_json, BrowserAutoReviewActionSet,
    BrowserAutoReviewRawAction, BrowserAutoReviewRequest, BrowserAutoReviewRuntimeResult,
    BrowserAutoReviewSessionTargeting, BrowserAutoReviewSource,
    BrowserAutoReviewSuggestedGrantScope, BrowserAutoReviewTargetClass, BrowserAutoReviewUrlSource,
};
pub(crate) use self::context_usage::render_context_usage_summary;
pub(crate) use self::debug_context::render_debug_context;
pub(crate) use self::hook_support::run_turn_hooks;
#[cfg(test)]
pub(crate) use self::http_support::{
    http_5xx_backoff_with_jitter, http_5xx_base_delay, http_5xx_max_attempts, http_retry_config,
    is_retryable_http_error, parse_retry_after_headers, retry_delay, send_http_request_raw,
    HttpRetryConfig, RawHttpResponse, HTTP_RETRY_ATTEMPTS_ENV, HTTP_RETRY_DELAY_MS_ENV,
};
pub(crate) use self::http_support::{
    parse_http_json_response, retry_on_5xx, send_http_request, send_http_request_raw_with_proxy,
    send_http_request_with_proxy,
};
#[cfg(test)]
use self::openai::parse_openai_sse_response;
#[cfg(test)]
use self::openai::{
    build_codex_openai_request_body, execute_openai_tool_calls, openai_tool_definitions,
    parse_openai_sse_response_streaming, resolve_openai_execution_config,
};
pub use self::permission_prompt::{
    with_permission_prompt_handler, with_user_question_prompt_handler,
    BrowserPermissionPromptActionSet, BrowserPermissionPromptPayload,
    BrowserPermissionPromptSource, BrowserPermissionPromptTargetClass, PermissionPromptAction,
    PermissionPromptRequest, PermissionPromptReviewPayload, UserQuestionPromptRequest,
    UserQuestionPromptResponse,
};
pub use self::reflection::{
    CodeJudgeConfig, LlmJudgeConfig, LlmJudgeContextScope, LlmJudgeMode, LlmJudgePromptCacheMode,
    ReflectionConfig, ReflectionLanguage, ReflectionTraceEvent,
};
pub(crate) use self::request_tool_filter::{build_request_tool_filter, RequestToolFilter};
use self::structured_output_support::validate_structured_output_schema;
pub use self::structured_output_support::StructuredOutputConfig;

#[cfg(test)]
use self::structured_output_support::anthropic_tool_definitions;
use self::tool_executor::{
    execute_tool_call, is_parallel_safe_tool, resolve_tool_permission, PermissionOutcome,
    ToolExecutionBackend,
};
#[cfg(test)]
pub(crate) use self::tool_results::{build_persisted_output_message, PREVIEW_SIZE_CHARS};
pub(crate) use self::tool_results::{
    enforce_tool_result_budget, git_status_context, process_tool_result, resolve_max_output_tokens,
    MAX_TOOL_RESULT_CHARS,
};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const OPENAI_CODEX_COMPAT_VERSION: &str = "0.125.0";
const OPENAI_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const SUPPRESS_TOOLS_FOR_SIMPLE_TURNS_ENV: &str = "PUFFER_SUPPRESS_TOOLS_FOR_SIMPLE_TURNS";
const LOCAL_REPLY_FOR_SIMPLE_TURNS_ENV: &str = "PUFFER_LOCAL_REPLY_FOR_SIMPLE_TURNS";

#[derive(Clone, Default)]
struct TurnRequestOptions<'a> {
    structured_output: Option<&'a StructuredOutputConfig>,
    tool_filter: Option<&'a RequestToolFilter>,
    reflection: Option<ReflectionConfig>,
    /// Cooperative cancellation token. When set, the agent loop checks
    /// it at every turn boundary. See [`CancelToken`].
    cancel: Option<&'a CancelToken>,
    /// Hard cap on inner-loop iterations (provider round-trips). When
    /// `Some(N)`, the agent loop bails after N iterations regardless
    /// of whether the model is still requesting tool calls. Mirrors
    /// Claude Code's `runForkedAgent.maxTurns` (cc-src
    /// `utils/forkedAgent.ts:103`; v2.1.131 `compact` fork uses
    /// `maxTurns: 1`). `None` keeps the existing unbounded behavior
    /// — the loop exits only when the model stops requesting tools.
    /// Used by [`subagent::execute_subagent`] to bound grader-style
    /// sub-agents that would otherwise loop indefinitely on a soft
    /// prompt-side budget.
    max_turns: Option<u32>,
    /// Optional observability handle. When set, the agent loop wraps
    /// each turn / provider call / tool batch in OTel spans and pushes
    /// them to the configured OTLP endpoint (typically Langfuse).
    /// Owned (Arc-backed clone) so we can fall back to the
    /// process-wide handle without lifetime gymnastics.
    observability: Option<puffer_observability::ObservabilityHandle>,
    lightweight_context: bool,
}

/// Cooperative cancellation handle threaded through the agent loop and
/// provider sessions. Mirrors pi-mono's `signal: AbortSignal` pattern
/// (`pi-mono/packages/ai/src/types.ts:70`). The token is `Clone` and
/// cheap (one `Arc<AtomicBool>`); callers can hold a handle and call
/// [`CancelToken::cancel`] to flip it from another thread.
///
/// Cancellation is **cooperative**: the agent loop checks the token at
/// turn boundaries (between tool batches, before each provider call,
/// before each tool invocation). It is not strictly mid-stream — a
/// long single SSE turn will not abort until that turn finishes — but
/// it covers the common ESC-to-cancel UX. Mid-stream cancel is a
/// follow-up that requires threading the token into every SSE parser.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    inner: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl CancelToken {
    /// Creates a fresh, uncancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true once any handle has called [`cancel`].
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Flips the token. Idempotent.
    pub fn cancel(&self) {
        self.inner.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Returns `Err(anyhow!("cancelled"))` when cancelled, otherwise
    /// `Ok(())`. Convenience for the `?` operator at boundaries.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            anyhow::bail!("cancelled");
        }
        Ok(())
    }
}

/// Describes one tool call executed during a model turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocation {
    pub call_id: String,
    pub tool_id: String,
    pub input: String,
    pub output: String,
    pub success: bool,
    /// Structured tool metadata retained for traceability and session replay.
    pub metadata: Value,
    /// When true, the agent_loop stops after the current batch finishes
    /// (and emits this invocation's tool result back to the model). Mirrors
    /// pi-mono's `AgentToolResult.terminate` (`pi-mono/packages/agent/src/types.ts:301`).
    /// The driver only honors termination when **every** invocation in the
    /// batch sets this to true — single-tool early-stop is fine, but in a
    /// parallel batch the model expects all results back. Set by tools
    /// that emit `{ "terminate": true }` in their `ToolOutput.metadata`.
    pub terminate: bool,
}

impl ToolInvocation {
    /// Returns true when the invocation was synthesized from a provider stream
    /// item before the provider response reached a terminal event.
    pub fn is_provider_stream_invocation(&self) -> bool {
        self.metadata
            .pointer("/puffer/provider_stream_invocation")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }
}

/// Describes one tool call requested by the model before execution finishes.
/// The `call_id` matches the paired `ToolInvocation` when execution
/// completes — callers can use it to upgrade a pending UI card in place
/// rather than rendering a fresh one on result arrival.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub tool_id: String,
    pub input: String,
}
/// Stores the visible result of one executed model turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnExecution {
    pub assistant_text: String,
    pub tool_invocations: Vec<ToolInvocation>,
    /// Reflection trace events observed during the turn. Populated by every
    /// execute path (streaming or not) so callers can round-trip them to
    /// `SessionStore::append_trace_event` (or any other sidecar) without
    /// branching on transport. Streaming consumers still receive the same
    /// events incrementally via `TurnStreamEvent::ReflectionTrace` and may
    /// treat this field as a replay-friendly fallback.
    pub reflection_traces: Vec<ReflectionTraceEvent>,
}

/// Per-turn token usage report emitted after the provider response completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnUsageReport {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

/// Identifies which retry loop (or one-shot fallback) emitted a
/// `TurnStreamEvent::RetryAttempt`. The inner transport loop and the outer
/// stream-restart loop run independent counters with different maxima, so
/// consumers need the source to render interleaved attempts coherently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryAttemptKind {
    /// Inner connection-level retry (`retry_openai_transport`).
    Transport,
    /// Outer restart of a stream that died before a terminal event.
    Stream,
    /// One-shot resend without the reasoning include selector.
    ReasoningFallback,
    /// WebSocket transport retry / one-shot fallback to SSE.
    WsFallback,
}

impl RetryAttemptKind {
    /// Returns the stable event label for this retry-attempt source.
    pub fn as_str(&self) -> &'static str {
        match self {
            RetryAttemptKind::Transport => "transport",
            RetryAttemptKind::Stream => "stream",
            RetryAttemptKind::ReasoningFallback => "reasoning-fallback",
            RetryAttemptKind::WsFallback => "ws-fallback",
        }
    }
}

/// Describes one incremental event emitted while a model turn is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnStreamEvent {
    /// A chunk of the model's internal reasoning / thinking output.
    ThinkingDelta(String),
    TextDelta(String),
    ToolCallsRequested(Vec<ToolCallRequest>),
    ToolInvocations(Vec<ToolInvocation>),
    /// The active plan file changed during plan mode.
    PlanUpdated {
        file_path: String,
        content: Option<String>,
    },
    /// The model exited plan mode and returned the completed plan.
    PlanCompleted {
        file_path: String,
        content: Option<String>,
    },
    ReflectionTrace(ReflectionTraceEvent),
    ReflectionCheckpoint(String),
    /// A transport-level or stream-level retry is about to be attempted.
    ///
    /// Consumers that render live deltas should discard in-flight assistant
    /// text, thinking, and tool-call previews from the failed attempt before
    /// rendering the next attempt.
    RetryAttempt {
        attempt: usize,
        max_attempts: usize,
        error: String,
        kind: RetryAttemptKind,
    },
    /// Per-turn token usage (including cache hit data).
    Usage(TurnUsageReport),
}

/// Executes one user prompt against the currently selected provider and model.
pub fn execute_user_prompt(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
) -> Result<TurnExecution> {
    execute_user_prompt_with_options(
        state,
        resources,
        providers,
        auth_store,
        input,
        TurnRequestOptions::default(),
    )
}

/// Executes one user prompt with all tools suppressed for advisory side-turns.
pub fn execute_user_prompt_without_tools(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
) -> Result<TurnExecution> {
    execute_user_prompt_with_options(
        state,
        resources,
        providers,
        auth_store,
        input,
        TurnRequestOptions {
            structured_output: None,
            tool_filter: Some(RequestToolFilter::empty_static()),
            reflection: None,
            cancel: None,
            max_turns: None,
            observability: None,
            lightweight_context: false,
        },
    )
}

/// Executes one streamed user prompt with all tools suppressed.
pub fn execute_user_prompt_streaming_without_tools<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    mut on_event: F,
) -> Result<TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    execute_user_prompt_streaming_with_options(
        state,
        resources,
        providers,
        auth_store,
        input,
        TurnRequestOptions {
            tool_filter: Some(RequestToolFilter::empty_static()),
            ..TurnRequestOptions::default()
        },
        &mut on_event,
    )
}

/// Executes one user prompt with a request-scoped tool filter.
pub(crate) fn execute_user_prompt_with_tool_filter(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    tool_filter: Option<&RequestToolFilter>,
) -> Result<TurnExecution> {
    execute_user_prompt_with_options(
        state,
        resources,
        providers,
        auth_store,
        input,
        TurnRequestOptions {
            structured_output: None,
            tool_filter,
            reflection: None,
            cancel: None,
            max_turns: None,
            observability: None,
            lightweight_context: false,
        },
    )
}

/// Executes a Claude-style side question without mutating the main session transcript state.
pub fn execute_side_question(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    question: &str,
) -> Result<TurnExecution> {
    side_question::execute_side_question(state, resources, providers, auth_store, question)
}

/// Executes one concrete tool action from a workflow without involving a model turn.
pub fn execute_tool_action_once(
    state: &mut AppState,
    resources: &LoadedResources,
    cwd: &std::path::Path,
    tool_id: &str,
    input: Value,
) -> Result<ToolExecutionResult> {
    let registry = ToolRegistry::from_resources(resources);
    let definition = registry
        .definition(tool_id)
        .cloned()
        .ok_or_else(|| anyhow!("unknown workflow action tool `{tool_id}`"))?;
    let filesystem_policy =
        filesystem_access::runtime_filesystem_policy(cwd, resources, state, None)?;
    claude_tools::execute_tool(
        state,
        resources,
        &registry,
        &definition,
        cwd,
        &filesystem_policy,
        input,
        claude_tools::ProviderToolContext::None,
    )
}

/// Executes the runtime-backed `Agent` tool from local command handlers.
pub fn execute_agent_tool_once(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    cwd: &std::path::Path,
    input: Value,
) -> Result<String> {
    agents::execute_agent_tool(state, resources, providers, auth_store, cwd, input)
}

/// Shuts down long-lived runtime services such as cached LSP sessions.
pub fn shutdown_runtime_services() -> Result<()> {
    // Shut down any active in-process teammates.
    {
        let registry = teammate_loop::teammate_registry().lock().unwrap();
        for (agent_id, tx) in registry.iter() {
            let _ = tx.send(teammate_loop::TeammateMessage::Shutdown {
                request_id: format!("session-exit-{agent_id}"),
            });
        }
    }
    // Brief grace period for teammates to exit.
    std::thread::sleep(std::time::Duration::from_millis(500));
    // Clear the registry.
    teammate_loop::teammate_registry().lock().unwrap().clear();
    // Wait for detached background-agent threads (spawned by the
    // `Agent` tool with `run_in_background=true`) to reach a terminal
    // status. Without this, a `puffer benchmark-run` that fires
    // background subagents would exit immediately after the parent
    // response, killing the spawned threads mid-LLM-call and losing
    // their OTel spans before they could flush. Bounded so a stuck
    // upstream can't block CLI exit indefinitely; tasks still active
    // at the deadline are abandoned (their threads are killed by
    // process exit but the wait at least gives short subagent runs a
    // chance to land in Langfuse).
    let still_active =
        background_tasks::task_manager().wait_for_drain(std::time::Duration::from_secs(30));
    if still_active > 0 {
        eprintln!(
            "shutdown: {still_active} background agent(s) still running after 30s — \
             their traces may be incomplete in the observability backend"
        );
    }
    // Flush any buffered observability spans before exit. Codex review
    // note: bounded by `ObservabilityConfig::shutdown_timeout` so a
    // hung Langfuse can't block CLI exit.
    if let Some(handle) = observability_handle() {
        handle.shutdown();
    }
    claude_tools::workflow::lsp::shutdown_lsp_services()
}

/// Executes one user prompt with a request-scoped structured output contract.
pub fn execute_user_prompt_with_structured_output(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    structured_output: &StructuredOutputConfig,
) -> Result<TurnExecution> {
    validate_structured_output_schema(structured_output)?;
    execute_user_prompt_with_options(
        state,
        resources,
        providers,
        auth_store,
        input,
        TurnRequestOptions {
            structured_output: Some(structured_output),
            tool_filter: None,
            reflection: None,
            cancel: None,
            max_turns: None,
            observability: None,
            lightweight_context: false,
        },
    )
}

fn execute_user_prompt_with_options(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    mut options: TurnRequestOptions<'_>,
) -> Result<TurnExecution> {
    let _runtime_work = RuntimeWorkGuard::enter();
    apply_session_reflection_default(state, &mut options);
    // Inject the process-wide observability handle so the
    // non-streaming path (used by `execute_user_prompt`, in turn
    // called by spawned agents / teammates / reflection judge / side
    // questions) gets the same agent_loop / turn / provider_call span
    // scaffolding as the streaming path. Review v3 #3 flagged the
    // missing handle injection here.
    if options.observability.is_none() {
        options.observability = observability_handle();
    }
    // Side-turns (memory review/flush, reflection sub-agent, etc.) pass a
    // tool_filter — skip the post-turn memory hook for those, otherwise we
    // recurse forever.
    let is_side_turn = options.tool_filter.is_some();
    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    let api = resolve_model_api(state, providers, provider, &model_id);
    let Some(adapter) = provider_adapter::adapter_for_api(&api) else {
        bail!(
            "provider {} with api {api} is not executable yet",
            provider.id
        );
    };
    let saved_lambda_gate = state.lambda_gate.clone();
    let saved_pending_lambda_host_call = state.pending_lambda_host_call.clone();
    let result = adapter.execute_turn(
        state, resources, providers, provider, model_id, auth_store, input, options,
    );
    state.lambda_gate = saved_lambda_gate;
    state.pending_lambda_host_call = saved_pending_lambda_host_call;
    if !is_side_turn && result.is_ok() {
        maybe_run_project_memory_review(state, resources, providers, auth_store);
    }
    result
}

/// After a main-turn completes, fire the side-turn memory review if the
/// turn-counter crossed the configured nudge interval. Lives in runtime —
/// not in any single front-end — so daemon/CLI/streaming/blocking paths all
/// behave the same. The side-turn itself sets `tool_filter`, so the recursion
/// guard in `execute_user_prompt_with_options` prevents loops.
fn maybe_run_project_memory_review(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
) {
    if crate::memory::project_memory_turn_completed(state) {
        crate::memory::spawn_project_memory_review(state, resources, providers, auth_store);
    }
}

/// If the caller did not pass an explicit reflection policy, inherit the
/// session-scoped one configured via `/reflect`.
fn apply_session_reflection_default(state: &AppState, options: &mut TurnRequestOptions<'_>) {
    if options.reflection.is_none() {
        if let Some(config) = state.reflection_config.as_ref() {
            options.reflection = Some(config.clone());
        }
    }
}

/// Executes one user prompt and emits incremental stream events when the provider supports them.
pub fn execute_user_prompt_streaming<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    mut on_event: F,
) -> Result<TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    execute_user_prompt_streaming_with_options(
        state,
        resources,
        providers,
        auth_store,
        input,
        TurnRequestOptions::default(),
        &mut on_event,
    )
}

/// Executes one user prompt with streaming events and interactive permission handling.
pub fn execute_user_prompt_streaming_with_permissions<F, P>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    structured_output: Option<&StructuredOutputConfig>,
    mut on_event: F,
    on_permission: P,
) -> Result<TurnExecution>
where
    F: FnMut(TurnStreamEvent),
    P: FnMut(PermissionPromptRequest) -> PermissionPromptAction + 'static,
{
    with_permission_prompt_handler(on_permission, || {
        execute_user_prompt_streaming_with_options(
            state,
            resources,
            providers,
            auth_store,
            input,
            TurnRequestOptions {
                structured_output,
                tool_filter: None,
                reflection: None,
                cancel: None,
                max_turns: None,
                observability: None,
                lightweight_context: false,
            },
            &mut on_event,
        )
    })
}

/// Streaming + interactive permissions + cancellation. Same shape as
/// `execute_user_prompt_streaming_with_permissions` plus a
/// [`CancelToken`] reference. The TUI uses this so ESC during a turn
/// flips the token and the agent loop returns
/// `Err("cancelled")` at the next boundary instead of the worker
/// thread silently running to completion against a dropped channel.
pub fn execute_user_prompt_streaming_with_permissions_and_cancel<F, P>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    structured_output: Option<&StructuredOutputConfig>,
    cancel: &CancelToken,
    mut on_event: F,
    on_permission: P,
) -> Result<TurnExecution>
where
    F: FnMut(TurnStreamEvent),
    P: FnMut(PermissionPromptRequest) -> PermissionPromptAction + 'static,
{
    with_permission_prompt_handler(on_permission, || {
        execute_user_prompt_streaming_with_options(
            state,
            resources,
            providers,
            auth_store,
            input,
            TurnRequestOptions {
                structured_output,
                tool_filter: None,
                reflection: None,
                cancel: Some(cancel),
                max_turns: None,
                observability: None,
                lightweight_context: false,
            },
            &mut on_event,
        )
    })
}

/// Streaming + interactive permissions + cancellation, scoped by a prompt's
/// declarative `allowed_tools` metadata.
pub fn execute_user_prompt_streaming_with_prompt_tools_and_cancel<F, P>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    prompt_tool_scope: Option<&str>,
    structured_output: Option<&StructuredOutputConfig>,
    cancel: &CancelToken,
    mut on_event: F,
    on_permission: P,
) -> Result<TurnExecution>
where
    F: FnMut(TurnStreamEvent),
    P: FnMut(PermissionPromptRequest) -> PermissionPromptAction + 'static,
{
    let tool_filter = prompt_tool_scope
        .and_then(|prompt_id| puffer_resources::prompt_by_id(resources, prompt_id))
        .map(|prompt| build_request_tool_filter(&prompt.value.allowed_tools))
        .transpose()?
        .flatten();
    with_permission_prompt_handler(on_permission, || {
        execute_user_prompt_streaming_with_options(
            state,
            resources,
            providers,
            auth_store,
            input,
            TurnRequestOptions {
                structured_output,
                tool_filter: tool_filter.as_ref(),
                reflection: None,
                cancel: Some(cancel),
                max_turns: None,
                observability: None,
                lightweight_context: false,
            },
            &mut on_event,
        )
    })
}

/// Executes one user prompt with a request-scoped structured output contract and streaming events.
pub fn execute_user_prompt_streaming_with_structured_output<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    structured_output: &StructuredOutputConfig,
    mut on_event: F,
) -> Result<TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    validate_structured_output_schema(structured_output)?;
    execute_user_prompt_streaming_with_options(
        state,
        resources,
        providers,
        auth_store,
        input,
        TurnRequestOptions {
            structured_output: Some(structured_output),
            tool_filter: None,
            reflection: None,
            cancel: None,
            max_turns: None,
            observability: None,
            lightweight_context: false,
        },
        &mut on_event,
    )
}

/// Executes one user prompt with streaming events and a caller-supplied
/// cancellation token. The token is checked at every turn boundary
/// (between provider calls and between tool batches). When tripped, the
/// loop returns `Err(anyhow!("cancelled"))`. Mirrors pi-mono's
/// `signal: AbortSignal` parameter on `agentLoop`
/// (`pi-mono/packages/agent/src/agent-loop.ts:34`).
pub fn execute_user_prompt_streaming_with_cancel<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    cancel: &CancelToken,
    mut on_event: F,
) -> Result<TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    execute_user_prompt_streaming_with_options(
        state,
        resources,
        providers,
        auth_store,
        input,
        TurnRequestOptions {
            structured_output: None,
            tool_filter: None,
            reflection: None,
            cancel: Some(cancel),
            max_turns: None,
            observability: None,
            lightweight_context: false,
        },
        &mut on_event,
    )
}

/// Executes one user prompt with streaming events and a reflection policy.
pub fn execute_user_prompt_streaming_with_reflection<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    reflection: ReflectionConfig,
    mut on_event: F,
) -> Result<TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    execute_user_prompt_streaming_with_options(
        state,
        resources,
        providers,
        auth_store,
        input,
        TurnRequestOptions {
            structured_output: None,
            tool_filter: None,
            reflection: Some(reflection),
            cancel: None,
            max_turns: None,
            observability: None,
            lightweight_context: false,
        },
        &mut on_event,
    )
}

fn execute_user_prompt_streaming_with_options<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    mut options: TurnRequestOptions<'_>,
    on_event: &mut F,
) -> Result<TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    let _runtime_work = RuntimeWorkGuard::enter();
    apply_session_reflection_default(state, &mut options);
    // Inject the process-wide observability handle when callers
    // didn't supply one. This means `execute_user_prompt_streaming(...)`
    // (the most common entry) automatically gets traced once
    // `install_observability(...)` was called at boot, without
    // every caller having to plumb the handle through.
    //
    // `installed_handle` is a stack local kept alive for the whole
    // function so the borrow is sound. `options.observability` is
    // `Option<&'a Handle>`; we shrink the implicit lifetime to the
    // local binding here.
    if options.observability.is_none() {
        options.observability = observability_handle();
    }
    let is_side_turn = options.tool_filter.is_some();
    let suppress_tools = suppress_tools_for_simple_turn(input);
    if suppress_tools && local_reply_for_simple_turns_enabled() {
        // simple-turn shortcut: no provider call, no memory review.
        let assistant_text = local_simple_turn_reply(input);
        on_event(TurnStreamEvent::TextDelta(assistant_text.to_string()));
        return Ok(TurnExecution {
            assistant_text: assistant_text.to_string(),
            tool_invocations: Vec::new(),
            reflection_traces: Vec::new(),
        });
    }
    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    let api = resolve_model_api(state, providers, provider, &model_id);
    let Some(adapter) = provider_adapter::adapter_for_api(&api) else {
        // Adapter unknown → fall through to non-streaming dispatch which
        // emits the canonical "not executable yet" error message.
        // The delegate fires its own post-turn memory hook; nothing to do here.
        if suppress_tools {
            options.tool_filter = Some(RequestToolFilter::empty_static());
            options.lightweight_context = true;
        }
        return execute_user_prompt_with_options(
            state, resources, providers, auth_store, input, options,
        );
    };
    if suppress_tools {
        options.tool_filter = Some(RequestToolFilter::empty_static());
        options.lightweight_context = true;
    }
    let saved_lambda_gate = state.lambda_gate.clone();
    let saved_pending_lambda_host_call = state.pending_lambda_host_call.clone();
    let result = adapter.execute_turn_streaming(
        state, resources, providers, provider, model_id, auth_store, input, options, on_event,
    );
    state.lambda_gate = saved_lambda_gate;
    state.pending_lambda_host_call = saved_pending_lambda_host_call;
    if !is_side_turn && result.is_ok() {
        maybe_run_project_memory_review(state, resources, providers, auth_store);
    }
    result
}

fn suppress_tools_for_simple_turn(input: &str) -> bool {
    if !std::env::var(SUPPRESS_TOOLS_FOR_SIMPLE_TURNS_ENV)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
    {
        return false;
    }
    matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "hi" | "hello" | "hey"
    )
}

fn local_reply_for_simple_turns_enabled() -> bool {
    std::env::var(LOCAL_REPLY_FOR_SIMPLE_TURNS_ENV)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn local_simple_turn_reply(input: &str) -> &'static str {
    match input.trim().to_ascii_lowercase().as_str() {
        "hey" => "Hey.",
        _ => "Hi.",
    }
}
/// Resolves the active provider descriptor and model id for a runtime turn.
pub(super) fn resolve_provider_and_model<'a>(
    state: &AppState,
    providers: &'a ProviderRegistry,
) -> Result<(&'a ProviderDescriptor, String)> {
    if providers.providers().next().is_none() {
        return Err(anyhow!("no providers are registered"));
    }

    if let Some(selected) = &state.current_model {
        if let Some(model) = providers.resolve_model(selected) {
            let provider = providers
                .provider(&model.provider)
                .ok_or_else(|| anyhow!("provider {} not found", model.provider))?;
            return Ok((provider, model.id.clone()));
        }
        if let Some(provider_id) = &state.current_provider {
            if let Some(provider) = providers.provider(provider_id) {
                if let Some(model) = provider.models.iter().find(|model| model.id == *selected) {
                    return Ok((provider, model.id.clone()));
                }
            }
        }
        if let Some((provider, model)) = providers.providers().find_map(|provider| {
            provider
                .models
                .iter()
                .find(|model| model.id == *selected)
                .map(|model| (provider, model))
        }) {
            return Ok((provider, model.id.clone()));
        }
    }

    if let Some(provider_id) = &state.current_provider {
        let provider = providers
            .provider(provider_id)
            .ok_or_else(|| anyhow!("provider {provider_id} not found"))?;
        let model_id = provider
            .models
            .first()
            .map(|model| model.id.clone())
            .ok_or_else(|| anyhow!("provider {provider_id} has no configured models"))?;
        return Ok((provider, model_id));
    }
    let provider = providers
        .providers()
        .next()
        .expect("checked for an empty provider registry above");
    let model_id = provider
        .models
        .first()
        .map(|model| model.id.clone())
        .ok_or_else(|| anyhow!("provider {} has no configured models", provider.id))?;
    Ok((provider, model_id))
}

fn resolve_model_api(
    state: &AppState,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: &str,
) -> String {
    state
        .current_model
        .as_ref()
        .and_then(|selected| {
            providers
                .resolve_model(selected)
                .map(|model| model.api.clone())
        })
        .or_else(|| {
            provider
                .models
                .iter()
                .find(|model| model.id == model_id)
                .map(|model| model.api.clone())
        })
        .unwrap_or_else(|| provider.default_api.clone())
}

#[cfg(test)]
mod tests;
