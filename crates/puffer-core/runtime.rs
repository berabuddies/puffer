use crate::permissions::load_runtime_permission_context;
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_provider_registry::{
    AuthStore, OAuthCredential, ProviderDescriptor, ProviderRegistry, StoredCredential,
};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use puffer_transport_anthropic::{
    build_messages_request, get_session_ingress_auth, AnthropicAuth, AnthropicMessage,
    AnthropicModelRequest, AnthropicRequestConfig,
};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::time::Duration;

#[cfg(test)]
mod agent_runtime_tests;
mod agents;
mod anthropic_sse;
pub mod background_tasks;
pub mod claude_tools;
mod context_usage;
mod hook_support;
mod local_mcp_resources;
mod local_tools;
mod openai;
mod openai_sse;
mod openai_ws;
mod permission_prompt;
mod reflection;
mod request_tool_filter;
mod side_question;
mod structured_output_support;
mod system_prompt;
pub mod teammate_loop;
mod tool_executor;

mod debug_context;
pub(crate) use self::context_usage::render_context_usage_summary;
pub(crate) use self::debug_context::render_debug_context;
pub(crate) use self::hook_support::run_turn_hooks;
#[cfg(test)]
use self::openai::{
    build_codex_openai_request_body, execute_openai_tool_calls, openai_tool_definitions,
    parse_openai_sse_response_streaming, resolve_openai_execution_config,
};
use self::openai::{
    execute_openai, execute_openai_completions, is_event_stream, parse_openai_sse_response,
};
pub use self::permission_prompt::{
    with_permission_prompt_handler, PermissionPromptAction, PermissionPromptRequest,
};
pub use self::reflection::{
    CodeJudgeConfig, LlmJudgeConfig, LlmJudgeContextScope, LlmJudgeMode, LlmJudgePromptCacheMode,
    ReflectionConfig, ReflectionLanguage, ReflectionTraceEvent,
};
pub(crate) use self::request_tool_filter::{build_request_tool_filter, RequestToolFilter};
pub use self::structured_output_support::StructuredOutputConfig;
use self::structured_output_support::{
    anthropic_tool_definitions_for_request, validate_structured_output_schema,
};

#[cfg(test)]
use self::structured_output_support::anthropic_tool_definitions;
use self::system_prompt::render_runtime_system_prompt;
use self::tool_executor::{
    execute_tool_call, is_parallel_safe_tool, resolve_tool_permission, PermissionOutcome,
    ToolExecutionBackend,
};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const OPENAI_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const HTTP_RETRY_ATTEMPTS_ENV: &str = "PUFFER_HTTP_RETRY_ATTEMPTS";
const HTTP_RETRY_DELAY_MS_ENV: &str = "PUFFER_HTTP_RETRY_DELAY_MS";

#[derive(Debug, Clone, Default)]
struct TurnRequestOptions<'a> {
    structured_output: Option<&'a StructuredOutputConfig>,
    tool_filter: Option<&'a RequestToolFilter>,
    reflection: Option<ReflectionConfig>,
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

/// Describes one tool call executed during a model turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocation {
    pub call_id: String,
    pub tool_id: String,
    pub input: String,
    pub output: String,
    pub success: bool,
}

/// Describes one tool call requested by the model before execution finishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRequest {
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
    apply_session_reflection_default(state, &mut options);
    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    match resolve_model_api(state, providers, provider, &model_id).as_str() {
        "anthropic-messages" => execute_anthropic(
            state, resources, providers, provider, model_id, auth_store, input, options,
        ),
        "openai-responses" | "azure-openai-responses" | "openai-codex-responses" => execute_openai(
            state, resources, providers, provider, model_id, auth_store, input, options,
        ),
        "openai-completions" => execute_openai_completions(
            state, resources, providers, provider, model_id, auth_store, input, options,
        ),
        other => bail!(
            "provider {} with api {other} is not executable yet",
            provider.id
        ),
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
    apply_session_reflection_default(state, &mut options);
    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    match resolve_model_api(state, providers, provider, &model_id).as_str() {
        "openai-responses" | "azure-openai-responses" | "openai-codex-responses"
            if openai::openai_websocket_enabled() =>
        {
            openai::execute_openai_websocket_streaming(
                state, resources, providers, provider, model_id, auth_store, input, options,
                on_event,
            )
        }
        "openai-responses" | "azure-openai-responses" | "openai-codex-responses" => {
            openai::execute_openai_streaming(
                state, resources, providers, provider, model_id, auth_store, input, options,
                on_event,
            )
        }
        "anthropic-messages" => execute_anthropic_streaming(
            state, resources, providers, auth_store, input, options, on_event,
        ),
        _ => execute_user_prompt_with_options(
            state, resources, providers, auth_store, input, options,
        ),
    }
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
fn execute_anthropic(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: TurnRequestOptions<'_>,
) -> Result<TurnExecution> {
    use openai::conversation::{
        build_system_reminder, compact_conversation_with, inject_post_compact_context,
        items_to_anthropic_messages, transcript_to_items, ConversationItem,
    };

    let structured_output = options.structured_output;
    let auth = anthropic_auth_for_provider(auth_store, provider)?;
    let registry = ToolRegistry::from_resources(resources);
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let plan_mode_context = crate::plan_mode::take_plan_mode_context_message(state, resources)?;

    // Build canonical conversation items (shared with OpenAI path).
    let mut items = transcript_to_items(state, input);
    let mut invocations = Vec::new();
    let mut reflection_traces: Vec<ReflectionTraceEvent> = Vec::new();
    let mut reflection = options
        .reflection
        .map(|config| reflection::ReflectionTracker::new(input, config));

    let request_config = AnthropicRequestConfig {
        base_url: provider.base_url.clone(),
        session_id: state.session.id.to_string(),
        custom_headers: provider.headers.clone(),
        remote_container_id: None,
        remote_session_id: None,
        client_app: None,
        entrypoint: "cli".to_string(),
        user_type: "external".to_string(),
        version: APP_VERSION.to_string(),
        workload: None,
        additional_protection: false,
        cch_enabled: true,
        auth: auth.clone(),
        beta_header: None,
        client_request_id: None,
    };
    // Build request for URL, headers, and attribution prefix (fingerprint uses first user text).
    let request = build_messages_request(
        &request_config,
        &AnthropicModelRequest {
            model: model_id.clone(),
            max_tokens: resolve_max_output_tokens(provider, &model_id),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: input.to_string(),
            }],
        },
    )?;
    let tools = anthropic_tool_definitions_for_request(
        &registry,
        structured_output,
        Some(&permission_context),
        options.tool_filter,
    )?;
    let system_prompt = render_runtime_system_prompt(
        state,
        resources,
        &model_id,
        &tools
            .iter()
            .filter_map(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<std::collections::BTreeSet<_>>(),
    )?;

    // System reminder (currentDate + gitStatus) — now in system blocks, not prepended to messages.
    let git_status = git_status_context();
    let system_reminder = build_system_reminder(&git_status);

    // Anthropic summary function: uses Anthropic Messages API.
    let summary_url = request.url.clone();
    let summary_headers = request.headers.clone();
    let anthropic_summary_fn = |old_context: &str, mid: &str| -> Option<String> {
        anthropic_generate_summary(old_context, mid, &summary_url, &summary_headers)
    };

    // Pre-turn compaction using shared logic.
    let cwd = state.cwd.clone();
    let compacted =
        compact_conversation_with(&mut items, provider, &model_id, None, &anthropic_summary_fn);
    if compacted {
        inject_post_compact_context(&mut items, &cwd);
    }

    // Resolve thinking/reasoning support.
    let model_supports_thinking = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.supports_reasoning)
        .unwrap_or(false);
    let max_output = resolve_max_output_tokens(provider, &model_id);

    loop {
        // Convert items to Anthropic wire format at each iteration.
        let wire_messages = items_to_anthropic_messages(&items);

        let mut body = json!({
            "model": model_id,
            "max_tokens": max_output,
            "messages": wire_messages,
            "system": anthropic_system_blocks(
                &request.attribution_prefix_block,
                Some(system_prompt.as_str()),
                plan_mode_context.as_deref(),
                Some(&system_reminder),
            )
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.clone());
            body["tool_choice"] = json!({"type": "auto"});
        }
        let provider_supports_thinking_api =
            provider.id == "anthropic" || provider.base_url.contains("anthropic.com");
        if model_supports_thinking && provider_supports_thinking_api && state.effort_level != "low"
        {
            let thinking_budget = match state.effort_level.as_str() {
                "high" | "max" => max_output.saturating_sub(1).min(16_384),
                _ => max_output.saturating_sub(1).min(8_192),
            };
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": thinking_budget
            });
        } else {
            body["temperature"] = json!(1);
        }
        if model_supports_thinking && provider_supports_thinking_api {
            body["context_management"] = json!({
                "edits": [{
                    "type": "clear_thinking_20251015",
                    "keep": "all"
                }]
            });
        }
        if state.fast_mode {
            body["speed"] = json!("fast");
        }
        body["metadata"] = json!({
            "user_id": format!(
                "{{\"session_id\":\"{}\",\"device_id\":\"puffer-cli\"}}",
                state.session.id
            )
        });

        let response =
            match send_http_request(&request.url, &request.headers, &body.to_string(), true) {
                Ok(response) => response,
                Err(error) => {
                    let err_msg = error.to_string();
                    // 413 / prompt_too_long recovery: drop oldest items and retry.
                    if (err_msg.contains("413")
                        || err_msg.contains("prompt_too_long")
                        || err_msg.contains("too long"))
                        && items.len() > 3
                    {
                        let drop_count = (items.len() / 3).max(1);
                        items.drain(..drop_count);
                        // Ensure items start with a Message.
                        if !matches!(items.first(), Some(ConversationItem::Message { .. })) {
                            items.insert(
                                0,
                                ConversationItem::user_message(
                                    "[Context truncated to fit within model limits]",
                                ),
                            );
                        }
                        continue;
                    }
                    return Err(error);
                }
            };

        if let Some(tool_results) = execute_anthropic_tool_calls(
            state,
            resources,
            providers,
            auth_store,
            &response,
            &registry,
            &cwd,
            &request_config,
            &model_id,
            structured_output,
            options.tool_filter,
        )? {
            invocations.extend(tool_results.invocations.clone());
            // Append response content as ConversationItems.
            append_anthropic_response_to_items(&mut items, &response, &tool_results);
            if let Some(observation) = reflection.as_mut().and_then(|tracker| {
                tracker.observe_batch_with_judge(
                    &tool_results.invocations,
                    &items,
                    state,
                    resources,
                    providers,
                    auth_store,
                )
            }) {
                reflection_traces.extend(observation.trace_events);
                if let Some(checkpoint) = observation.checkpoint {
                    items.push(ConversationItem::user_message(checkpoint.prompt));
                }
            }
            // Compact between tool iterations using shared logic.
            let compacted = compact_conversation_with(
                &mut items,
                provider,
                &model_id,
                None,
                &anthropic_summary_fn,
            );
            if compacted {
                inject_post_compact_context(&mut items, &cwd);
            }
            continue;
        }

        let assistant_text = parse_anthropic_text(&response)?;
        run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
        return Ok(TurnExecution {
            assistant_text,
            tool_invocations: invocations,
            reflection_traces,
        });
    }
}

/// Streaming variant of execute_anthropic — sends `stream: true` and parses
/// SSE events, emitting TextDelta in real-time.
fn execute_anthropic_streaming<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    options: TurnRequestOptions<'_>,
    on_event: &mut F,
) -> Result<TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    use openai::conversation::{
        build_system_reminder, compact_conversation_with, inject_post_compact_context,
        items_to_anthropic_messages, transcript_to_items, ConversationItem,
    };

    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    let structured_output = options.structured_output;
    let auth = anthropic_auth_for_provider(auth_store, provider)?;
    let registry = ToolRegistry::from_resources(resources);
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let plan_mode_context = crate::plan_mode::take_plan_mode_context_message(state, resources)?;

    // Build canonical conversation items (shared with OpenAI path).
    let mut items = transcript_to_items(state, input);
    let mut invocations = Vec::new();
    let mut reflection_traces: Vec<ReflectionTraceEvent> = Vec::new();
    let mut reflection = options
        .reflection
        .map(|config| reflection::ReflectionTracker::new(input, config));

    let request_config = build_anthropic_request_config(state, provider, &auth);
    let request = build_messages_request(
        &request_config,
        &AnthropicModelRequest {
            model: model_id.clone(),
            max_tokens: resolve_max_output_tokens(provider, &model_id),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: input.to_string(),
            }],
        },
    )?;
    let tools = anthropic_tool_definitions_for_request(
        &registry,
        structured_output,
        Some(&permission_context),
        options.tool_filter,
    )?;
    let system_prompt = render_runtime_system_prompt(
        state,
        resources,
        &model_id,
        &tools
            .iter()
            .filter_map(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<std::collections::BTreeSet<_>>(),
    )?;

    // System reminder (currentDate + gitStatus) — now in system blocks.
    let git_status = git_status_context();
    let system_reminder = build_system_reminder(&git_status);

    // Anthropic summary function.
    let summary_url = request.url.clone();
    let summary_headers = request.headers.clone();
    let anthropic_summary_fn = |old_context: &str, mid: &str| -> Option<String> {
        anthropic_generate_summary(old_context, mid, &summary_url, &summary_headers)
    };

    // Pre-turn compaction.
    let cwd = state.cwd.clone();
    let compacted =
        compact_conversation_with(&mut items, provider, &model_id, None, &anthropic_summary_fn);
    if compacted {
        inject_post_compact_context(&mut items, &cwd);
    }

    let model_supports_thinking = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.supports_reasoning)
        .unwrap_or(false);
    let provider_supports_thinking_api =
        provider.id == "anthropic" || provider.base_url.contains("anthropic.com");
    let max_output = resolve_max_output_tokens(provider, &model_id);

    loop {
        // Drain completed background tasks and inject as user messages.
        let completed =
            claude_tools::workflow::drain_completed_shell_tasks(&state.cwd, &state.session.id);
        if !completed.is_empty() {
            let notice = format!(
                "<system-reminder>\n{}\nUse TaskOutput to retrieve the full output if needed.\n</system-reminder>",
                completed.join("\n")
            );
            items.push(ConversationItem::user_message(&notice));
        }

        // Convert items to Anthropic wire format.
        let wire_messages = items_to_anthropic_messages(&items);

        let mut body = json!({
            "model": model_id,
            "max_tokens": max_output,
            "messages": wire_messages,
            "stream": true,
            "system": anthropic_system_blocks(
                &request.attribution_prefix_block,
                Some(system_prompt.as_str()),
                plan_mode_context.as_deref(),
                Some(&system_reminder),
            )
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.clone());
            body["tool_choice"] = json!({"type": "auto"});
        }
        if model_supports_thinking && provider_supports_thinking_api && state.effort_level != "low"
        {
            let thinking_budget = match state.effort_level.as_str() {
                "high" | "max" => max_output.saturating_sub(1).min(16_384),
                _ => max_output.saturating_sub(1).min(8_192),
            };
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": thinking_budget
            });
        } else {
            body["temperature"] = json!(1);
        }

        // Send streaming request.
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap_or_else(|_| Client::new());
        let mut http_request = client.post(&request.url);
        for (key, value) in &request.headers {
            http_request = http_request.header(key, value);
        }
        http_request = http_request
            .header("content-type", "application/json")
            .header("accept", "text/event-stream");
        let http_response = http_request
            .body(body.to_string())
            .send()
            .with_context(|| format!("failed to send streaming request to {}", request.url))?;

        if !http_response.status().is_success() {
            let status = http_response.status();
            let text = http_response.text().unwrap_or_default();
            bail!("request failed with status {status}: {text}");
        }

        let response = anthropic_sse::parse_anthropic_sse(http_response, on_event)?;

        if let Some(tool_results) = execute_anthropic_tool_calls(
            state,
            resources,
            providers,
            auth_store,
            &response,
            &registry,
            &cwd,
            &request_config,
            &model_id,
            structured_output,
            options.tool_filter,
        )? {
            if !tool_results.invocations.is_empty() {
                on_event(TurnStreamEvent::ToolInvocations(
                    tool_results.invocations.clone(),
                ));
            }
            invocations.extend(tool_results.invocations.clone());
            // Append response content as ConversationItems.
            append_anthropic_response_to_items(&mut items, &response, &tool_results);
            if let Some(observation) = reflection.as_mut().and_then(|tracker| {
                tracker.observe_batch_with_judge(
                    &tool_results.invocations,
                    &items,
                    state,
                    resources,
                    providers,
                    auth_store,
                )
            }) {
                for trace_event in &observation.trace_events {
                    on_event(TurnStreamEvent::ReflectionTrace(trace_event.clone()));
                }
                reflection_traces.extend(observation.trace_events);
                if let Some(checkpoint) = observation.checkpoint {
                    on_event(TurnStreamEvent::ReflectionCheckpoint(
                        checkpoint.summary.clone(),
                    ));
                    items.push(ConversationItem::user_message(checkpoint.prompt));
                }
            }
            // Compact between tool iterations.
            let compacted = compact_conversation_with(
                &mut items,
                provider,
                &model_id,
                None,
                &anthropic_summary_fn,
            );
            if compacted {
                inject_post_compact_context(&mut items, &cwd);
            }
            continue;
        }

        let assistant_text = parse_anthropic_text(&response)?;
        run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
        return Ok(TurnExecution {
            assistant_text,
            tool_invocations: invocations,
            reflection_traces,
        });
    }
}

fn build_anthropic_request_config(
    state: &AppState,
    provider: &ProviderDescriptor,
    auth: &AnthropicAuth,
) -> AnthropicRequestConfig {
    AnthropicRequestConfig {
        base_url: provider.base_url.clone(),
        session_id: state.session.id.to_string(),
        custom_headers: provider.headers.clone(),
        remote_container_id: None,
        remote_session_id: None,
        client_app: None,
        entrypoint: "cli".to_string(),
        user_type: "external".to_string(),
        version: APP_VERSION.to_string(),
        workload: None,
        additional_protection: false,
        cch_enabled: true,
        auth: auth.clone(),
        beta_header: None,
        client_request_id: None,
    }
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
                // Retry on 429 (rate limit) and 5xx (server errors).
                let status = response.status.as_u16();
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

fn anthropic_auth_for_provider(
    auth_store: &AuthStore,
    provider: &ProviderDescriptor,
) -> Result<AnthropicAuth> {
    match auth_store.get(&provider.id) {
        Some(StoredCredential::ApiKey { key }) => Ok(AnthropicAuth::ApiKey(key.clone())),
        Some(StoredCredential::OAuth(OAuthCredential { access_token, .. })) => {
            Ok(AnthropicAuth::OAuthBearer(access_token.clone()))
        }
        None if provider.auth_modes.is_empty() => Ok(AnthropicAuth::None),
        None => get_session_ingress_auth().ok_or_else(|| {
            anyhow!(
                "no credentials configured for provider {}; use `puffer auth set-api-key {}` first",
                provider.id,
                provider.id
            )
        }),
    }
}
fn parse_anthropic_text(response: &Value) -> Result<String> {
    let parts = response
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("anthropic response missing content array"))?
        .iter()
        .filter_map(|item| {
            let item_type = item.get("type").and_then(Value::as_str)?;
            if item_type == "text" {
                item.get("text").and_then(Value::as_str).map(str::to_string)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        bail!("anthropic response did not contain text content");
    }
    Ok(parts.join("\n"))
}
#[cfg(test)]
fn anthropic_tool_schema(handler: &str) -> Value {
    match handler {
        "bash" => json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" }
            },
            "required": ["command"],
        }),
        "read_file" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"],
        }),
        "write_file" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "contents": { "type": "string" }
            },
            "required": ["path", "contents"],
        }),
        "replace_in_file" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old": { "type": "string" },
                "new": { "type": "string" },
                "replace_all": { "type": "boolean" }
            },
            "required": ["path", "old", "new"],
        }),
        "list_dir" => json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": [],
        }),
        "search_text" => json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "path": { "type": "string" }
            },
            "required": ["query"],
        }),
        _ => json!({
            "type": "object",
            "properties": {},
        }),
    }
}
fn execute_anthropic_tool_calls(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    response: &Value,
    registry: &ToolRegistry,
    cwd: &std::path::Path,
    request_config: &AnthropicRequestConfig,
    model_id: &str,
    structured_output: Option<&StructuredOutputConfig>,
    tool_filter: Option<&RequestToolFilter>,
) -> Result<Option<AnthropicToolResults>> {
    let Some(content) = response.get("content").and_then(Value::as_array) else {
        return Ok(None);
    };

    let mut results = Vec::new();
    let mut invocations = Vec::new();
    for item in content {
        if item.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let tool_id = item
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("anthropic tool_use block missing name"))?;
        let tool_use_id = item
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("anthropic tool_use block missing id"))?;
        let input = item
            .get("input")
            .ok_or_else(|| anyhow!("anthropic tool_use block missing input"))?;
        let execution = execute_tool_call(
            state,
            resources,
            providers,
            auth_store,
            registry,
            model_id,
            cwd,
            ToolExecutionBackend::Anthropic {
                request_config,
                structured_output,
            },
            tool_filter,
            tool_id,
            input.clone(),
        )?;
        let raw_output = if execution.output.stderr.is_empty() {
            execution.output.stdout
        } else if execution.output.stdout.is_empty() {
            execution.output.stderr
        } else {
            format!("{}\n{}", execution.output.stdout, execution.output.stderr)
        };
        // Persist oversized tool results to disk, returning a preview message.
        // Falls back to head truncation if disk write fails.
        // (CC limits: 50K chars per tool, 200K chars per message).
        let output_text =
            process_tool_result(&raw_output, MAX_TOOL_RESULT_CHARS, &state.session.id);
        results.push(json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": output_text,
            "is_error": !execution.success,
        }));
        invocations.push(ToolInvocation {
            call_id: tool_use_id.to_string(),
            tool_id: tool_id.to_string(),
            input: serde_json::to_string(input)?,
            output: output_text.clone(),
            success: execution.success,
        });
    }

    if results.is_empty() {
        return Ok(None);
    }

    // Enforce per-message aggregate budget (CC: 200K).
    let mut output_strings: Vec<String> = invocations.iter().map(|i| i.output.clone()).collect();
    enforce_tool_result_budget(&mut output_strings, &state.session.id);
    // Apply budget changes back to results and invocations.
    for (i, new_output) in output_strings.into_iter().enumerate() {
        if new_output != invocations[i].output {
            results[i]["content"] = json!(new_output);
            invocations[i].output = new_output;
        }
    }

    Ok(Some(AnthropicToolResults {
        results: Value::Array(results),
        invocations,
    }))
}

struct AnthropicToolResults {
    results: Value,
    invocations: Vec<ToolInvocation>,
}

/// Converts an Anthropic API response + tool execution results into
/// ConversationItems and appends them to the items vector.
///
/// Extracts from the response:
/// - Text blocks → assistant message
/// - tool_use blocks → FunctionCall items
///
/// From tool_results.invocations:
/// - Each invocation → FunctionCallOutput item
fn append_anthropic_response_to_items(
    items: &mut Vec<openai::conversation::ConversationItem>,
    response: &Value,
    tool_results: &AnthropicToolResults,
) {
    use openai::conversation::{ConversationItem, ToolOutputPayload};

    // Extract assistant text (non-tool-use content blocks) from response.
    if let Some(content) = response.get("content").and_then(Value::as_array) {
        let texts: Vec<&str> = content
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect();
        if !texts.is_empty() {
            items.push(ConversationItem::assistant_message(texts.join("\n")));
        }
    }

    // Extract tool_use blocks from response → FunctionCall items.
    if let Some(content) = response.get("content").and_then(Value::as_array) {
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let call_id = block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let arguments = block
                .get("input")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "{}".to_string());
            items.push(ConversationItem::FunctionCall {
                call_id,
                name,
                arguments,
            });
        }
    }

    // Tool execution results → FunctionCallOutput items.
    for inv in &tool_results.invocations {
        items.push(ConversationItem::FunctionCallOutput {
            call_id: inv.call_id.clone(),
            output: if inv.success {
                ToolOutputPayload::success(inv.output.clone())
            } else {
                ToolOutputPayload::error(inv.output.clone())
            },
        });
    }
}

/// Generates an AI summary via the Anthropic Messages API.
/// Used as the summary function for `compact_conversation_with()`.
fn anthropic_generate_summary(
    old_context: &str,
    model_id: &str,
    url: &str,
    headers: &[(String, String)],
) -> Option<String> {
    let compact_prompt = format!(
        "Summarize this conversation fragment into a compact context block. \
         Preserve file paths, function names, errors, and key decisions verbatim. \
         Structure: 1) Intent 2) Key Concepts 3) Files & Code 4) Errors & Fixes \
         5) Pending Tasks 6) Current State. Be thorough but concise. \
         Do NOT use any tools.\n\n---\n\n{old_context}"
    );

    let body = json!({
        "model": model_id,
        "max_tokens": 16_384,
        "messages": [{"role": "user", "content": compact_prompt}],
    });

    match send_http_request(url, headers, &body.to_string(), true) {
        Ok(response) => parse_anthropic_text(&response).ok(),
        Err(_) => None,
    }
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

fn anthropic_system_blocks(
    attribution_prefix_block: &str,
    system_prompt: Option<&str>,
    plan_mode_context: Option<&str>,
    system_reminder: Option<&str>,
) -> Vec<Value> {
    let mut blocks = vec![json!({
        "type": "text",
        "text": attribution_prefix_block,
    })];
    if let Some(system_prompt) = system_prompt.filter(|prompt| !prompt.trim().is_empty()) {
        blocks.push(json!({
            "type": "text",
            "text": system_prompt,
            "cache_control": { "type": "ephemeral" }
        }));
    }
    if let Some(plan_mode_context) = plan_mode_context {
        blocks.push(json!({
            "type": "text",
            "text": plan_mode_context,
        }));
    }
    if let Some(reminder) = system_reminder.filter(|r| !r.trim().is_empty()) {
        blocks.push(json!({
            "type": "text",
            "text": format!(
                "<system-reminder>\nAs you answer the user's questions, you can use the following context:\n\
                 {reminder}\n\n\
                 IMPORTANT: this context may or may not be relevant to your tasks. \
                 You should not respond to this context unless it is highly relevant to your task.\n\
                 </system-reminder>"
            ),
        }));
    }
    blocks
}

#[cfg(test)]
mod tests;
