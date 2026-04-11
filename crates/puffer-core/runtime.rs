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
pub mod claude_tools;
pub mod teammate_loop;
mod context_usage;
mod hook_support;
mod local_mcp_resources;
mod local_tools;
mod anthropic_sse;
mod openai;
mod openai_sse;
mod permission_prompt;
mod request_tool_filter;
mod side_question;
mod structured_output_support;
mod system_prompt;
mod tool_executor;

pub(crate) use self::context_usage::render_context_usage_summary;
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
pub(crate) use self::request_tool_filter::{build_request_tool_filter, RequestToolFilter};
pub use self::structured_output_support::StructuredOutputConfig;
use self::structured_output_support::{
    anthropic_tool_definitions_for_request, validate_structured_output_schema,
};

#[cfg(test)]
use self::structured_output_support::anthropic_tool_definitions;
use self::system_prompt::render_runtime_system_prompt;
use self::tool_executor::{execute_tool_call, ToolExecutionBackend};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const OPENAI_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const HTTP_RETRY_ATTEMPTS_ENV: &str = "PUFFER_HTTP_RETRY_ATTEMPTS";
const HTTP_RETRY_DELAY_MS_ENV: &str = "PUFFER_HTTP_RETRY_DELAY_MS";

#[derive(Debug, Clone, Copy, Default)]
struct TurnRequestOptions<'a> {
    structured_output: Option<&'a StructuredOutputConfig>,
    tool_filter: Option<&'a RequestToolFilter>,
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
}

/// Describes one incremental event emitted while a model turn is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnStreamEvent {
    TextDelta(String),
    ToolCallsRequested(Vec<ToolCallRequest>),
    ToolInvocations(Vec<ToolInvocation>),
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
        },
    )
}

fn execute_user_prompt_with_options(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    input: &str,
    options: TurnRequestOptions<'_>,
) -> Result<TurnExecution> {
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
    options: TurnRequestOptions<'_>,
    on_event: &mut F,
) -> Result<TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    match resolve_model_api(state, providers, provider, &model_id).as_str() {
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
    let structured_output = options.structured_output;
    let auth = anthropic_auth_for_provider(auth_store, provider)?;
    let registry = ToolRegistry::from_resources(resources);
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let mut messages = transcript_to_anthropic_messages(state, input);
    let mut invocations = Vec::new();
    let plan_mode_context = crate::command_helpers::prompt::plan_mode_context_message(state)?;
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
    let request = build_messages_request(
        &request_config,
        &AnthropicModelRequest {
            model: model_id.clone(),
            max_tokens: resolve_max_output_tokens(provider, &model_id),
            messages: transcript_to_anthropic_request_messages(state, input),
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

    // Prepend system-reminder context (CC's prependUserContext).
    // Injected as the first user message so the model sees current date/context.
    prepend_system_reminder(&mut messages);

    // History snipping: truncate old tool outputs in messages to save context.
    // CC does this via applyToolResultBudget / applyHistorySnip.
    snip_old_tool_outputs(&mut messages);

    // Auto-compact before turn: generate summary if over threshold.
    let context_window = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.context_window as u32)
        .unwrap_or(200_000);
    let auto_compact_threshold = context_window.saturating_mul(80) / 100;
    auto_compact_messages(
        &mut messages,
        auto_compact_threshold,
        &request.url,
        &request.headers,
        &model_id,
        resolve_max_output_tokens(provider, &model_id),
    );

    // Resolve thinking/reasoning support from model capabilities + effort level.
    let model_supports_thinking = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.supports_reasoning)
        .unwrap_or(false);
    let max_output = resolve_max_output_tokens(provider, &model_id);

    for _ in 0..8 {
        let mut body = json!({
            "model": model_id,
            "max_tokens": max_output,
            "messages": messages,
            "system": anthropic_system_blocks(
                &request.attribution_prefix_block,
                Some(system_prompt.as_str()),
                plan_mode_context.as_deref(),
            )
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.clone());
            body["tool_choice"] = json!({"type": "auto"});
        }
        // Add thinking/reasoning when the model supports it, effort is not "low",
        // and the provider actually supports the Anthropic thinking API format.
        let provider_supports_thinking_api = provider.id == "anthropic"
            || provider.base_url.contains("anthropic.com");
        if model_supports_thinking && provider_supports_thinking_api && state.effort_level != "low" {
            let thinking_budget = match state.effort_level.as_str() {
                "high" | "max" => max_output.saturating_sub(1).min(16_384),
                _ => max_output.saturating_sub(1).min(8_192), // medium default
            };
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": thinking_budget
            });
        } else {
            // Temperature is only sent when thinking is disabled (CC behavior).
            body["temperature"] = json!(1);
        }
        // Context management: clear old thinking blocks server-side (CC parity).
        if model_supports_thinking && provider_supports_thinking_api {
            body["context_management"] = json!({
                "edits": [{
                    "type": "clear_thinking_20251015",
                    "keep": "all"
                }]
            });
        }
        // Fast mode: send speed='fast' when the user has toggled /fast on.
        if state.fast_mode {
            body["speed"] = json!("fast");
        }
        // Metadata for request attribution (matches CC's metadata.user_id).
        body["metadata"] = json!({
            "user_id": format!(
                "{{\"session_id\":\"{}\",\"device_id\":\"puffer-cli\"}}",
                state.session.id
            )
        });

        let response = match send_http_request(&request.url, &request.headers, &body.to_string(), true) {
            Ok(response) => response,
            Err(error) => {
                let err_msg = error.to_string();
                // 413 / prompt_too_long recovery: drop oldest messages and retry.
                if err_msg.contains("413")
                    || err_msg.contains("prompt_too_long")
                    || err_msg.contains("too long")
                {
                    if messages.len() > 3 {
                        let drop_count = (messages.len() / 3).max(1);
                        messages.drain(..drop_count);
                        // Ensure first message is user role for valid alternation.
                        if messages
                            .first()
                            .and_then(|m| m["role"].as_str())
                            == Some("user")
                        {
                            if let Some(first) = messages.first_mut() {
                                let existing = first["content"].as_str().unwrap_or("").to_string();
                                first["content"] = json!(format!(
                                    "[Context truncated]\n\n{existing}"
                                ));
                            }
                        } else {
                            messages.insert(
                                0,
                                json!({
                                    "role": "user",
                                    "content": "[Context truncated to fit within model limits]"
                                }),
                            );
                        }
                        continue;
                    }
                }
                return Err(error);
            }
        };
        let cwd = state.cwd.clone();
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
            invocations.extend(tool_results.invocations);
            messages.push(json!({
                "role": "assistant",
                "content": response
                    .get("content")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new())),
            }));
            messages.push(json!({
                "role": "user",
                "content": tool_results.results,
            }));
            // Context management between tool iterations (CC parity).
            snip_old_tool_outputs(&mut messages);
            auto_compact_messages(
                &mut messages,
                auto_compact_threshold,
                &request.url,
                &request.headers,
                &model_id,
                max_output,
            );
            continue;
        }

        let assistant_text = parse_anthropic_text(&response)?;
        run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
        return Ok(TurnExecution {
            assistant_text,
            tool_invocations: invocations,
        });
    }

    bail!("anthropic tool loop exceeded iteration limit")
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
    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    let structured_output = options.structured_output;
    let auth = anthropic_auth_for_provider(auth_store, provider)?;
    let registry = ToolRegistry::from_resources(resources);
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let mut messages = transcript_to_anthropic_messages(state, input);
    let mut invocations = Vec::new();
    let plan_mode_context = crate::command_helpers::prompt::plan_mode_context_message(state)?;

    let request_config = build_anthropic_request_config(state, provider, &auth);
    let request = build_messages_request(
        &request_config,
        &AnthropicModelRequest {
            model: model_id.clone(),
            max_tokens: resolve_max_output_tokens(provider, &model_id),
            messages: transcript_to_anthropic_request_messages(state, input),
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

    prepend_system_reminder(&mut messages);
    snip_old_tool_outputs(&mut messages);

    let model_supports_thinking = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.supports_reasoning)
        .unwrap_or(false);
    let provider_supports_thinking_api =
        provider.id == "anthropic" || provider.base_url.contains("anthropic.com");
    let max_output = resolve_max_output_tokens(provider, &model_id);

    for _ in 0..8 {
        let mut body = json!({
            "model": model_id,
            "max_tokens": max_output,
            "messages": messages,
            "stream": true,
            "system": anthropic_system_blocks(
                &request.attribution_prefix_block,
                Some(system_prompt.as_str()),
                plan_mode_context.as_deref(),
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

        // Send streaming request — use raw reqwest response for true SSE streaming.
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

        // Parse SSE stream — reqwest::blocking::Response implements Read,
        // so the parser reads events as they arrive from the network.
        let response =
            anthropic_sse::parse_anthropic_sse(http_response, on_event)?;

        let cwd = state.cwd.clone();
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
            invocations.extend(tool_results.invocations);
            messages.push(json!({
                "role": "assistant",
                "content": response
                    .get("content")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new())),
            }));
            messages.push(json!({
                "role": "user",
                "content": tool_results.results,
            }));
            // Context management between tool iterations (CC parity).
            snip_old_tool_outputs(&mut messages);
            let ctx_threshold = provider
                .models.iter().find(|m| m.id == model_id)
                .map(|m| m.context_window as u32).unwrap_or(200_000)
                .saturating_mul(80) / 100;
            auto_compact_messages(
                &mut messages,
                ctx_threshold,
                &request.url,
                &request.headers,
                &model_id,
                max_output,
            );
            continue;
        }

        let assistant_text = parse_anthropic_text(&response)?;
        run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
        return Ok(TurnExecution {
            assistant_text,
            tool_invocations: invocations,
        });
    }

    bail!("anthropic streaming tool loop exceeded iteration limit")
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
                if attempt < total_attempts
                    && (status == 429 || (500..=599).contains(&status))
                {
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
        // Truncate oversized tool results to prevent context overflow
        // (CC limits: 50K chars per tool, 200K chars per message).
        let output_text = truncate_tool_result(&raw_output, MAX_TOOL_RESULT_CHARS);
        results.push(json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": output_text,
            "is_error": !execution.success,
        }));
        invocations.push(ToolInvocation {
            tool_id: tool_id.to_string(),
            input: serde_json::to_string(input)?,
            output: output_text.clone(),
            success: execution.success,
        });
    }

    if results.is_empty() {
        Ok(None)
    } else {
        Ok(Some(AnthropicToolResults {
            results: Value::Array(results),
            invocations,
        }))
    }
}

struct AnthropicToolResults {
    results: Value,
    invocations: Vec<ToolInvocation>,
}
/// Trims older messages from the front when the estimated token count exceeds
/// the threshold, keeping the most recent messages to stay within budget.
/// This matches CC's auto-compact behavior (triggered at ~80% context usage).
/// Maximum characters per individual tool result (matches CC's DEFAULT_MAX_RESULT_SIZE_CHARS).
const MAX_TOOL_RESULT_CHARS: usize = 50_000;

/// Prepends a system-reminder user message with current date and context.
/// Matches CC's `prependUserContext()` which injects `<system-reminder>` tags.
fn prepend_system_reminder(messages: &mut Vec<Value>) {
    let now = time::OffsetDateTime::now_utc();
    let date_str = format!("{}-{:02}-{:02}", now.year(), now.month() as u8, now.day());
    let git_status = git_status_context();
    let mut sections = format!(
        "# currentDate\nToday's date is {date_str}."
    );
    if !git_status.is_empty() {
        sections.push_str(&format!("\n# gitStatus\n{git_status}"));
    }
    let reminder = format!(
        "<system-reminder>\nAs you answer the user's questions, you can use the following context:\n\
         {sections}\n\n\
         IMPORTANT: this context may or may not be relevant to your tasks. \
         You should not respond to this context unless it is highly relevant to your task.\n\
         </system-reminder>"
    );
    // Merge into first user message if possible to avoid breaking alternation.
    if let Some(first) = messages.first_mut() {
        if first["role"].as_str() == Some("user") {
            let existing = first["content"].as_str().unwrap_or("").to_string();
            first["content"] = json!(format!("{reminder}\n{existing}"));
            return;
        }
    }
    messages.insert(0, json!({"role": "user", "content": reminder}));
}

/// Returns a short git status summary for system-reminder injection (CC parity).
fn git_status_context() -> String {
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

/// Number of recent messages whose tool outputs are preserved in full.
const SNIP_KEEP_RECENT: usize = 6;
/// Maximum chars for a snipped tool output (older messages).
const SNIP_MAX_CHARS: usize = 500;

/// Truncates tool outputs in older messages to free context space.
/// Keeps the most recent SNIP_KEEP_RECENT messages intact.
/// This matches CC's history snipping / tool result budget.
fn snip_old_tool_outputs(messages: &mut [Value]) {
    let total = messages.len();
    if total <= SNIP_KEEP_RECENT {
        return;
    }
    let cutoff = total - SNIP_KEEP_RECENT;
    for msg in &mut messages[..cutoff] {
        let role = msg["role"].as_str().unwrap_or("");
        if role != "user" {
            continue;
        }
        // Check if this is a system-tagged tool output message.
        let content = msg["content"].as_str().unwrap_or("");
        if !content.starts_with("[system]\nTool ") {
            continue;
        }
        if content.chars().count() <= SNIP_MAX_CHARS {
            continue;
        }
        let snipped: String = content.chars().take(SNIP_MAX_CHARS).collect();
        msg["content"] = json!(format!("{snipped}\n[...output snipped...]"));
    }
}

fn truncate_tool_result(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}\n\n[Output truncated — {max_chars} char limit reached]")
}

/// Estimates token count for a message array (~4 chars per token).
fn estimate_message_tokens(messages: &[Value]) -> u32 {
    messages
        .iter()
        .map(|m| {
            let text = m["content"].as_str().unwrap_or("");
            (text.chars().count() as u32 + 3) / 4
        })
        .sum()
}

/// Auto-compact: if token estimate exceeds threshold, generate an AI summary
/// of old messages and replace them. Falls back to simple drop on API failure.
///
/// CC calls this before every API request in the query loop. Codex does the
/// same at pre-sampling and post-sampling points.
/// Maximum auto-compact cycles per turn to prevent infinite loops.
const MAX_AUTO_COMPACT_CYCLES: u32 = 10;

fn auto_compact_messages(
    messages: &mut Vec<Value>,
    threshold_tokens: u32,
    url: &str,
    headers: &[(String, String)],
    model_id: &str,
    max_output: u32,
) {
    // Circuit breaker: track consecutive compactions.
    // Resets when messages are under threshold (no compact needed).
    thread_local! {
        static COMPACT_COUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    if estimate_message_tokens(messages) <= threshold_tokens || messages.len() <= 2 {
        COMPACT_COUNT.with(|c| c.set(0)); // Under threshold → reset counter.
        return;
    }

    let count = COMPACT_COUNT.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    if count >= MAX_AUTO_COMPACT_CYCLES {
        return; // Exhausted — stop to prevent infinite loops.
    }

    // Build a compact prompt from the messages we're about to drop.
    // Keep the most recent 4 messages intact, summarize the rest.
    let keep_count = 4.min(messages.len());
    let to_summarize = &messages[..messages.len() - keep_count];
    if to_summarize.is_empty() {
        return;
    }

    // Build summary request: ask the model to summarize the old messages.
    let mut summary_content = String::new();
    for msg in to_summarize {
        let role = msg["role"].as_str().unwrap_or("?");
        let text = msg["content"].as_str().unwrap_or("");
        let preview: String = text.chars().take(500).collect();
        summary_content.push_str(&format!("[{role}]: {preview}\n\n"));
    }

    let compact_prompt = format!(
        "Summarize this conversation fragment into a compact context block. \
         Preserve file paths, function names, errors, and key decisions verbatim. \
         Structure: 1) Intent 2) Key Concepts 3) Files & Code 4) Errors & Fixes \
         5) Pending Tasks 6) Current State. Be thorough but concise.\n\n---\n\n{summary_content}"
    );

    let body = json!({
        "model": model_id,
        "max_tokens": max_output.min(4096),
        "messages": [
            {"role": "user", "content": compact_prompt}
        ],
    });

    // Try to generate summary via API. On failure, fall back to simple drop.
    let summary = match send_http_request(url, headers, &body.to_string(), true) {
        Ok(response) => parse_anthropic_text(&response).ok(),
        Err(_) => None,
    };

    // Replace old messages with summary.
    let kept: Vec<Value> = messages.split_off(messages.len() - keep_count);
    messages.clear();

    let summary_text = summary.unwrap_or_else(|| {
        "[Earlier conversation automatically compacted to fit context window]".to_string()
    });

    messages.push(json!({
        "role": "user",
        "content": format!(
            "[Conversation compacted — prior context summarized below]\n\n{summary_text}"
        )
    }));
    // Need an assistant ack to maintain alternation before the kept messages.
    messages.push(json!({
        "role": "assistant",
        "content": "Understood. I have the summarized context and will continue from here."
    }));
    messages.extend(kept);
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

fn transcript_to_anthropic_messages(state: &AppState, input: &str) -> Vec<Value> {
    let mut messages = state
        .transcript
        .iter()
        .map(|message| match message.role {
            crate::MessageRole::User => json!({
                "role": "user",
                "content": message.text,
            }),
            crate::MessageRole::Assistant => json!({
                "role": "assistant",
                "content": message.text,
            }),
            crate::MessageRole::System => json!({
                "role": "user",
                "content": format!("[system]\n{}", message.text),
            }),
        })
        .collect::<Vec<_>>();
    if messages.is_empty() {
        messages.push(json!({
            "role": "user",
            "content": input,
        }));
    }
    messages
}
fn transcript_to_anthropic_request_messages(
    state: &AppState,
    input: &str,
) -> Vec<AnthropicMessage> {
    let mut messages = state
        .transcript
        .iter()
        .map(|message| AnthropicMessage {
            role: match message.role {
                crate::MessageRole::Assistant => "assistant".to_string(),
                crate::MessageRole::User | crate::MessageRole::System => "user".to_string(),
            },
            content: match message.role {
                crate::MessageRole::System => format!("[system]\n{}", message.text),
                _ => message.text.clone(),
            },
        })
        .collect::<Vec<_>>();
    if messages.is_empty() {
        messages.push(AnthropicMessage {
            role: "user".to_string(),
            content: input.to_string(),
        });
    }
    messages
}
fn anthropic_system_blocks(
    attribution_prefix_block: &str,
    system_prompt: Option<&str>,
    plan_mode_context: Option<&str>,
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
    blocks
}

#[cfg(test)]
mod tests;
