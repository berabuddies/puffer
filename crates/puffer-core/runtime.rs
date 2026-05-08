use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry};
use puffer_resources::LoadedResources;
#[cfg(test)]
#[allow(unused_imports)]
use puffer_tools::ToolRegistry;
#[cfg(test)]
#[allow(unused_imports)]
use puffer_transport_anthropic::{AnthropicAuth, AnthropicRequestConfig};
use reqwest::blocking::Client;
use reqwest::StatusCode;
#[cfg(test)]
#[allow(unused_imports)]
use serde_json::json;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

mod agent_loop;
#[cfg(test)]
mod agent_runtime_tests;
mod agents;
mod anthropic;
mod anthropic_sse;
pub mod background_tasks;
mod blocking_loop;
pub mod claude_tools;
mod context_usage;
pub mod goals;
mod hook_support;
mod local_tools;
pub mod mcp_discovery;
mod microcompact;
mod openai;
mod openai_sse;
mod openai_ws;
mod permission_prompt;
mod provider_adapter;
pub mod quota;
mod reflection;
mod request_tool_filter;
mod side_question;
mod structured_output_support;
mod system_prompt;
pub mod teammate_loop;
mod tool_batch;
mod tool_executor;

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
pub(crate) use self::context_usage::render_context_usage_summary;
pub(crate) use self::debug_context::render_debug_context;
pub(crate) use self::hook_support::run_turn_hooks;
#[cfg(test)]
use self::openai::{
    build_codex_openai_request_body, execute_openai_tool_calls, openai_tool_definitions,
    parse_openai_sse_response_streaming, resolve_openai_execution_config,
};
use self::openai::{is_event_stream, parse_openai_sse_response};
pub use self::permission_prompt::{
    with_permission_prompt_handler, with_user_question_prompt_handler, PermissionPromptAction,
    PermissionPromptRequest, UserQuestionPromptRequest, UserQuestionPromptResponse,
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

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const OPENAI_CODEX_COMPAT_VERSION: &str = "0.125.0";
const OPENAI_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const HTTP_RETRY_ATTEMPTS_ENV: &str = "PUFFER_HTTP_RETRY_ATTEMPTS";
const HTTP_RETRY_DELAY_MS_ENV: &str = "PUFFER_HTTP_RETRY_DELAY_MS";

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
}

#[derive(Debug)]
struct RawHttpResponse {
    status: StatusCode,
    content_type: Option<String>,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HttpRetryConfig {
    retries: usize,
    delay_ms: u64,
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
    /// When true, the agent_loop stops after the current batch finishes
    /// (and emits this invocation's tool result back to the model). Mirrors
    /// pi-mono's `AgentToolResult.terminate` (`pi-mono/packages/agent/src/types.ts:301`).
    /// The driver only honors termination when **every** invocation in the
    /// batch sets this to true — single-tool early-stop is fine, but in a
    /// parallel batch the model expects all results back. Set by tools
    /// that emit `{ "terminate": true }` in their `ToolOutput.metadata`.
    pub terminate: bool,
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

/// Describes one incremental event emitted while a model turn is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnStreamEvent {
    /// A chunk of the model's internal reasoning / thinking output.
    ThinkingDelta(String),
    TextDelta(String),
    ToolCallsRequested(Vec<ToolCallRequest>),
    ToolInvocations(Vec<ToolInvocation>),
    ReflectionTrace(ReflectionTraceEvent),
    ReflectionCheckpoint(String),
    /// A transport-level retry is about to be attempted.
    RetryAttempt {
        attempt: usize,
        max_attempts: usize,
        error: String,
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
    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    let api = resolve_model_api(state, providers, provider, &model_id);
    let Some(adapter) = provider_adapter::adapter_for_api(&api) else {
        bail!(
            "provider {} with api {api} is not executable yet",
            provider.id
        );
    };
    adapter.execute_turn(
        state, resources, providers, provider, model_id, auth_store, input, options,
    )
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
    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    let api = resolve_model_api(state, providers, provider, &model_id);
    let Some(adapter) = provider_adapter::adapter_for_api(&api) else {
        // Adapter unknown → fall through to non-streaming dispatch which
        // emits the canonical "not executable yet" error message.
        return execute_user_prompt_with_options(
            state, resources, providers, auth_store, input, options,
        );
    };
    adapter.execute_turn_streaming(
        state, resources, providers, provider, model_id, auth_store, input, options, on_event,
    )
}
fn resolve_provider_and_model<'a>(
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

fn send_http_request(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    anthropic: bool,
) -> Result<Value> {
    let response = send_http_request_raw(url, headers, body, anthropic)?;
    parse_http_json_response(url, anthropic, response)
}

fn send_http_request_raw(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    anthropic: bool,
) -> Result<RawHttpResponse> {
    trace_http_exchange("request", url, headers, body);
    let retry_config = http_retry_config();
    let total_attempts = retry_config.retries.saturating_add(1);
    for attempt in 1..=total_attempts {
        match send_http_request_raw_once(url, headers, body, anthropic) {
            Ok(response) => {
                trace_http_response(url, response.status.as_u16(), &response.text);
                // Retry on 429 (rate limit) and 5xx (server errors) — UNLESS
                // the response classifies as a `QuotaError`. Retrying a 429
                // 4 times with linear backoff burns the budget the typed
                // quota path is supposed to protect: by the time the
                // classifier sees the error, the orchestrator has already
                // wasted ~10s of cooldown on a window that needs minutes
                // (or hours for `access_terminated`). Bail immediately so
                // the provider adapter can promote the error and the
                // benchmark CLI can exit with `QUOTA_EXIT_CODE`.
                let status = response.status.as_u16();
                let provider = if anthropic { "anthropic" } else { "openai" };
                if quota::classify_response(provider, status, &response.text).is_some() {
                    return Ok(response);
                }
                if attempt < total_attempts && (status == 429 || (500..=599).contains(&status)) {
                    let delay = retry_delay(retry_config, attempt);
                    if !delay.is_zero() {
                        std::thread::sleep(delay);
                    }
                    continue;
                }
                return Ok(response);
            }
            Err(error) if attempt < total_attempts && is_retryable_http_error(&error) => {
                trace_http_retry(url, attempt, &error);
                let delay = retry_delay(retry_config, attempt);
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("http retry loop exited without returning")
}

fn send_http_request_raw_once(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    anthropic: bool,
) -> Result<RawHttpResponse> {
    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| Client::new());
    let mut request = client.post(url);
    for (key, value) in headers {
        request = request.header(key, value);
    }
    if !headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("content-type"))
    {
        request = request.header("content-type", "application/json");
    }
    if anthropic
        && !headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("anthropic-version"))
    {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let response = request
        .body(body.to_string())
        .send()
        .with_context(|| format!("request to {url} failed"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let text = response
        .text()
        .with_context(|| format!("failed to read response body from {url}"))?;
    Ok(RawHttpResponse {
        status,
        content_type,
        text,
    })
}

fn http_retry_config() -> HttpRetryConfig {
    HttpRetryConfig {
        retries: parsed_env_usize(HTTP_RETRY_ATTEMPTS_ENV)
            .unwrap_or(3)
            .min(10),
        delay_ms: parsed_env_u64(HTTP_RETRY_DELAY_MS_ENV)
            .unwrap_or(1_000)
            .min(30_000),
    }
}

fn parsed_env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn parsed_env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn retry_delay(config: HttpRetryConfig, attempt: usize) -> Duration {
    if config.delay_ms == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(config.delay_ms.saturating_mul(attempt as u64))
}

fn is_retryable_http_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|value| value.is_timeout() || value.is_connect())
            || cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(is_retryable_io_error)
    })
}

fn is_retryable_io_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
    )
}

fn trace_http_exchange(kind: &str, url: &str, headers: &[(String, String)], body: &str) {
    let Ok(path) = std::env::var("PUFFER_HTTP_TRACE_PATH") else {
        return;
    };
    let rendered_headers = headers
        .iter()
        .map(|(key, value)| {
            if key.eq_ignore_ascii_case("authorization") {
                format!("{key}: <redacted>")
            } else {
                format!("{key}: {value}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(
                file,
                "--- {} {} ---\n{}\n\n{}\n",
                kind.to_ascii_uppercase(),
                url,
                rendered_headers,
                body
            )
        });
}

fn trace_http_response(url: &str, status: u16, body: &str) {
    let Ok(path) = std::env::var("PUFFER_HTTP_TRACE_PATH") else {
        return;
    };
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(file, "--- RESPONSE {} {} ---\n{}\n", status, url, body)
        });
}

fn trace_http_retry(url: &str, attempt: usize, error: &anyhow::Error) {
    let Ok(path) = std::env::var("PUFFER_HTTP_TRACE_PATH") else {
        return;
    };
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write as _;
            writeln!(file, "--- RETRY {} {} ---\n{}\n", attempt, url, error)
        });
}

fn parse_http_json_response(
    url: &str,
    anthropic: bool,
    response: RawHttpResponse,
) -> Result<Value> {
    if !response.status.is_success() {
        // Promote 429 / 403-access-terminated to a typed `QuotaError`
        // before falling back to the generic `bail!`. Without this, the
        // entire Anthropic blocking path (the only caller that goes
        // through `send_http_request` → `parse_http_json_response`)
        // bypasses quota classification: the SSE / streaming paths in
        // `runtime/anthropic.rs:275` and `runtime/openai.rs:1064`
        // already do this, but the blocking path used by
        // `one_turn_blocking` would otherwise lose the typed error and
        // the benchmark CLI would never see `QUOTA_EXIT_CODE`.
        let provider = if anthropic { "anthropic" } else { "openai" };
        if let Some(quota) =
            quota::classify_response(provider, response.status.as_u16(), &response.text)
        {
            return Err(anyhow::Error::new(quota));
        }
        bail!(
            "request failed with status {}: {}",
            response.status,
            response.text
        );
    }
    if !anthropic && is_event_stream(response.content_type.as_deref(), &response.text) {
        return parse_openai_sse_response(&response.text)
            .with_context(|| format!("failed to parse SSE response from {url}"));
    }
    serde_json::from_str::<Value>(&response.text)
        .with_context(|| format!("response from {url} was not valid JSON"))
}

/// Trims older messages from the front when the estimated token count exceeds
/// the threshold, keeping the most recent messages to stay within budget.
/// This matches CC/Codex auto-compact behavior (triggered at ~90% of effective context).
/// Maximum characters per individual tool result (matches CC's DEFAULT_MAX_RESULT_SIZE_CHARS).
pub(super) const MAX_TOOL_RESULT_CHARS: usize = 50_000;

/// Maximum aggregate characters for all tool results in a single turn.
/// CC: MAX_TOOL_RESULTS_PER_MESSAGE_CHARS = 200_000.
pub(super) const MAX_TOOL_RESULTS_PER_MESSAGE_CHARS: usize = 200_000;

/// Preview size for persisted tool outputs (matches CC's PREVIEW_SIZE_BYTES).
const PREVIEW_SIZE_CHARS: usize = 2_000;

const PERSISTED_OUTPUT_TAG: &str = "<persisted-output>";
const PERSISTED_OUTPUT_CLOSING_TAG: &str = "</persisted-output>";

/// Returns a short git status summary for system-reminder injection (CC parity).
pub(super) fn git_status_context() -> String {
    let branch = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if branch.is_empty() {
        return String::new();
    }
    let status = std::process::Command::new("git")
        .args(["status", "--short", "--no-ahead-behind"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let log = std::process::Command::new("git")
        .args(["log", "--oneline", "-3", "--no-decorate"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let mut result = format!("Current branch: {branch}");
    if !status.is_empty() {
        result.push_str(&format!("\nStatus:\n{status}"));
    }
    if !log.is_empty() {
        result.push_str(&format!("\nRecent commits:\n{log}"));
    }
    result
}

/// Process a tool result: if oversized, persist to disk and return a preview
/// message (CC pattern). Falls back to head truncation if persistence fails.
pub(super) fn process_tool_result(text: &str, max_chars: usize, session_id: &uuid::Uuid) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    // Try to persist to disk and return a preview (CC pattern).
    if let Some(message) = persist_and_preview(text, session_id) {
        return message;
    }
    // Fallback: head truncation.
    truncate_tool_result(text, max_chars)
}

pub(super) fn truncate_tool_result(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    let head_len = max_chars / 2;
    let tail_len = max_chars - head_len;
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[chars.len() - tail_len..].iter().collect();
    let omitted = chars.len() - max_chars;
    format!("{head}\n\n[…{omitted} chars truncated…]\n\n{tail}")
}

/// Persist text to a temp file and return a `<persisted-output>` preview message.
fn persist_and_preview(text: &str, session_id: &uuid::Uuid) -> Option<String> {
    let dir = std::env::temp_dir()
        .join(format!("puffer-{session_id}"))
        .join("tool-results");
    std::fs::create_dir_all(&dir).ok()?;
    let filename = format!("{}.txt", uuid::Uuid::new_v4());
    let filepath = dir.join(&filename);
    std::fs::write(&filepath, text).ok()?;
    Some(build_persisted_output_message(
        &filepath.to_string_lossy(),
        text,
    ))
}

fn build_persisted_output_message(filepath: &str, text: &str) -> String {
    let (preview, has_more) = generate_preview(text, PREVIEW_SIZE_CHARS);
    let size_str = format_byte_size(text.len());
    let preview_size_str = format_byte_size(PREVIEW_SIZE_CHARS);
    format!(
        "{PERSISTED_OUTPUT_TAG}\n\
         Output too large ({size_str}). Full output saved to: {filepath}\n\n\
         Preview (first {preview_size_str}):\n\
         {preview}{}\
         {PERSISTED_OUTPUT_CLOSING_TAG}",
        if has_more { "\n...\n" } else { "\n" },
    )
}

/// Generate a preview of text, trying to cut at a newline boundary.
fn generate_preview(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let truncated: String = text.chars().take(max_chars).collect();
    // Cut at last newline within the back half, to avoid mid-line breaks.
    let cut = truncated
        .rfind('\n')
        .filter(|&pos| pos > truncated.len() / 2)
        .unwrap_or(truncated.len());
    (truncated[..cut].to_string(), true)
}

fn format_byte_size(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} bytes")
    }
}

/// Enforce per-message aggregate budget on tool result outputs.
/// When total output exceeds MAX_TOOL_RESULTS_PER_MESSAGE_CHARS, persist the
/// largest results to disk and replace with previews (CC pattern).
pub(super) fn enforce_tool_result_budget(outputs: &mut [String], session_id: &uuid::Uuid) {
    let total: usize = outputs.iter().map(|o| o.len()).sum();
    if total <= MAX_TOOL_RESULTS_PER_MESSAGE_CHARS {
        return;
    }
    // Sort indices by size (largest first), persist until under budget.
    let mut indices: Vec<usize> = (0..outputs.len()).collect();
    indices.sort_by(|&a, &b| outputs[b].len().cmp(&outputs[a].len()));
    let mut remaining = total;
    for idx in indices {
        if remaining <= MAX_TOOL_RESULTS_PER_MESSAGE_CHARS {
            break;
        }
        let output = &outputs[idx];
        if output.contains(PERSISTED_OUTPUT_TAG) {
            continue; // Already persisted in per-tool step.
        }
        if let Some(msg) = persist_and_preview(output, session_id) {
            remaining = remaining.saturating_sub(output.len()) + msg.len();
            outputs[idx] = msg;
        }
    }
}

/// Resolves the max output tokens for the given model, falling back to a
/// sensible default when the provider catalog doesn't specify one.
fn resolve_max_output_tokens(provider: &ProviderDescriptor, model_id: &str) -> u32 {
    provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.max_output_tokens)
        .filter(|&v| v > 0)
        .unwrap_or(16_384)
}

#[cfg(test)]
mod tests;
