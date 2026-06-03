use super::{
    execute_tool_call, is_parallel_safe_tool, parse_http_json_response, resolve_tool_permission,
    PermissionOutcome, ToolExecutionBackend, ToolInvocation, TurnStreamEvent, APP_VERSION,
    OPENAI_CODEX_COMPAT_VERSION,
};
mod adapters;
mod completions_session;
pub(crate) mod conversation;
mod legacy_streaming;
mod responses_session;
mod support;
mod websocket;
mod websocket_state;

pub(crate) use self::adapters::{OpenAICompletionsAdapter, OpenAIResponsesAdapter};
pub(super) use self::support::build_codex_openai_request_body;
use self::support::{
    append_default_openai_headers, is_codex_openai_provider, openai_base_url_for_auth,
    openai_registry_credential, openai_stream_read_timeout, retry_openai_transport,
    trace_openai_http_request, trace_openai_http_response_headers,
};
#[cfg(test)]
pub(super) use self::websocket_state::reset_openai_websocket_http_fallbacks;
use super::structured_output_support::StructuredOutputConfig;
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_config::ProxyConfig;
use puffer_provider_openai::{
    extract_responses_text, extract_responses_tool_calls, refresh_oauth_token,
    refresh_oauth_token_with_client, OpenAIAuth, OpenAIRequestConfig, OpenAIResponseToolCall,
    OpenAIResponsesFunctionCallOutput, OpenAIResponsesResponse,
};
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry, StoredCredential};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;
use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufRead, Read};

pub(super) use super::openai_sse::{is_event_stream, parse_openai_sse_response};
use super::openai_sse::{
    is_openai_sse_api_error, openai_response_incomplete_error, parse_openai_sse_reader_typed,
    OpenAISseResult,
};

#[cfg(test)]
pub(super) use super::openai_sse::parse_openai_sse_response_streaming;

#[cfg(test)]
pub(super) use super::structured_output_support::openai_tool_definitions;

const OPENAI_CODEX_ORIGINATOR: &str = "codex_cli_rs";

#[derive(Debug, Clone)]
pub(super) struct OpenAIExecutionConfig {
    pub(super) provider_id: String,
    pub(super) request_config: OpenAIRequestConfig,
    pub(super) refresh_token: Option<String>,
    pub(super) codex_style: bool,
}

pub(super) struct OpenAIToolResults {
    pub(super) outputs: Vec<OpenAIResponsesFunctionCallOutput>,
    pub(super) invocations: Vec<ToolInvocation>,
}

fn openai_request_version(provider: &ProviderDescriptor, oauth: bool) -> String {
    if is_codex_openai_provider(provider) || (oauth && provider.id == "openai") {
        OPENAI_CODEX_COMPAT_VERSION.to_string()
    } else {
        APP_VERSION.to_string()
    }
}

pub(super) fn execute_openai_tool_calls(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    tool_calls: &[OpenAIResponseToolCall],
    registry: &ToolRegistry,
    cwd: &std::path::Path,
    request_config: &OpenAIRequestConfig,
    model_id: &str,
    structured_output: Option<&StructuredOutputConfig>,
    tool_filter: Option<&super::RequestToolFilter>,
) -> Result<OpenAIToolResults> {
    if state.lambda_gate.is_some() || tool_calls.iter().any(|tc| tc.name == "Skill") {
        return execute_openai_tool_calls_serial(
            state,
            resources,
            providers,
            auth_store,
            tool_calls,
            registry,
            cwd,
            request_config,
            model_id,
            structured_output,
            tool_filter,
        );
    }
    // Count how many parallel-safe tools we have.
    let parallel_count = tool_calls
        .iter()
        .filter(|tc| is_parallel_safe_tool(&tc.name))
        .count();

    // If 0-1 tool calls or nothing to parallelize, use the serial fast-path.
    if tool_calls.len() <= 1 || parallel_count <= 1 {
        return execute_openai_tool_calls_serial(
            state,
            resources,
            providers,
            auth_store,
            tool_calls,
            registry,
            cwd,
            request_config,
            model_id,
            structured_output,
            tool_filter,
        );
    }

    // ---------- Phase 1: Pre-resolve permissions for all tools (serial) ----------
    // Permission prompts require user interaction and &mut state (for AllowSession),
    // so they must be resolved before we enter the parallel phase.
    let mut permissions: Vec<PermissionOutcome> = Vec::with_capacity(tool_calls.len());
    for tc in tool_calls {
        permissions.push(resolve_tool_permission(
            state,
            resources,
            providers,
            auth_store,
            registry,
            cwd,
            &tc.name,
            &tc.arguments,
            tool_filter,
        )?);
    }

    // ---------- Phase 2: Execute tools ----------
    // Clone immutable data needed by parallel tools.
    let provider_context = super::claude_tools::ProviderToolContext::OpenAI {
        request_config,
        model_id,
        proxy: &state.config.network.proxy,
        structured_output,
    };
    // Cloned before `thread::scope` so each worker can route through the
    // active `ToolRunner` (e.g. `RemoteToolRunner`) without touching `state`.
    let runner = state.tool_runner.clone();

    // Pre-allocate results array; each slot filled by either parallel or serial exec.
    let mut results: Vec<Option<(String, bool, Value)>> = vec![None; tool_calls.len()];

    // Execute parallel-safe permitted tools concurrently.
    std::thread::scope(|s| {
        let mut handles: Vec<(
            usize,
            std::thread::ScopedJoinHandle<'_, (String, bool, Value)>,
        )> = Vec::new();
        for (i, tc) in tool_calls.iter().enumerate() {
            // Skip denied tools and non-parallel tools.
            if !is_parallel_safe_tool(&tc.name) {
                continue;
            }
            if let PermissionOutcome::Denied(ref denied) = permissions[i] {
                results[i] = Some((
                    denied.output.stdout.clone(),
                    denied.success,
                    denied.output.metadata.clone(),
                ));
                continue;
            }
            let filesystem_policy = match &permissions[i] {
                PermissionOutcome::Allowed(policy) => policy.clone(),
                PermissionOutcome::Denied(_) => unreachable!(),
            };
            let definition = match registry.definition(&tc.name) {
                Some(d) => d.clone(),
                None => {
                    results[i] = Some((format!("unknown tool {}", tc.name), false, Value::Null));
                    continue;
                }
            };
            let args = match super::secrets::expand_secret_placeholders(state, &tc.arguments) {
                Ok(args) => args,
                Err(error) => {
                    results[i] = Some((
                        super::secrets::redact_known_secrets(
                            state,
                            &format!("Tool execution failed: {error}"),
                        ),
                        false,
                        Value::Null,
                    ));
                    continue;
                }
            };
            let pc = &provider_context;
            let sid = &state.session.id;
            let runner_clone = runner.clone();
            handles.push((
                i,
                s.spawn(move || {
                    match super::claude_tools::execute_parallel_tool(
                        &definition,
                        cwd,
                        &filesystem_policy.workspace_roots,
                        &filesystem_policy,
                        sid,
                        args,
                        resources,
                        registry,
                        pc,
                        &runner_clone,
                    ) {
                        Ok(exec) => {
                            let output = if exec.output.stderr.is_empty() {
                                exec.output.stdout
                            } else if exec.output.stdout.is_empty() {
                                exec.output.stderr
                            } else {
                                format!("{}\n{}", exec.output.stdout, exec.output.stderr)
                            };
                            (output, exec.success, exec.output.metadata)
                        }
                        Err(error) => (
                            format!("Tool execution failed: {error}"),
                            false,
                            Value::Null,
                        ),
                    }
                }),
            ));
        }
        for (i, handle) in handles {
            results[i] =
                Some(handle.join().unwrap_or_else(|_| {
                    ("Tool execution panicked".to_string(), false, Value::Null)
                }));
        }
    });

    // Execute serial tools (those that need &mut state).
    for (i, tc) in tool_calls.iter().enumerate() {
        if results[i].is_some() {
            continue; // Already executed in parallel or denied.
        }
        if let PermissionOutcome::Denied(ref denied) = permissions[i] {
            results[i] = Some((
                denied.output.stdout.clone(),
                denied.success,
                denied.output.metadata.clone(),
            ));
            continue;
        }
        // Serial execution with full &mut state access.
        let (output, success, metadata) = match execute_tool_call(
            state,
            resources,
            providers,
            auth_store,
            registry,
            model_id,
            cwd,
            ToolExecutionBackend::OpenAi {
                request_config,
                structured_output,
            },
            tool_filter,
            &tc.name,
            tc.arguments.clone(),
        ) {
            Ok(exec) => {
                let output = if exec.output.stderr.is_empty() {
                    exec.output.stdout
                } else if exec.output.stdout.is_empty() {
                    exec.output.stderr
                } else {
                    format!("{}\n{}", exec.output.stdout, exec.output.stderr)
                };
                (output, exec.success, exec.output.metadata)
            }
            Err(error) => (
                format!("Tool execution failed: {error}"),
                false,
                Value::Null,
            ),
        };
        results[i] = Some((output, success, metadata));
    }

    // ---------- Phase 3: Assemble outputs in original order ----------
    let session_id = &state.session.id;
    let mut outputs = Vec::with_capacity(tool_calls.len());
    let mut invocations = Vec::with_capacity(tool_calls.len());
    for (i, tc) in tool_calls.iter().enumerate() {
        let (raw_output, success, metadata) = results[i]
            .take()
            .unwrap_or_else(|| ("Tool was not executed".to_string(), false, Value::Null));
        let raw_output = super::secrets::redact_known_secrets(state, &raw_output);
        let metadata = super::secrets::redact_json_value(state, &metadata);
        let output =
            super::process_tool_result(&raw_output, super::MAX_TOOL_RESULT_CHARS, session_id);
        outputs.push(OpenAIResponsesFunctionCallOutput {
            kind: "function_call_output".to_string(),
            call_id: tc.call_id.clone(),
            output: output.clone(),
        });
        invocations.push(ToolInvocation {
            call_id: tc.call_id.clone(),
            tool_id: tc.name.clone(),
            input: serde_json::to_string(&tc.arguments)?,
            output,
            success,
            metadata,
            terminate: false,
        });
    }

    // Enforce per-message aggregate budget (CC: 200K).
    let mut output_strings: Vec<String> = outputs.iter().map(|o| o.output.clone()).collect();
    super::enforce_tool_result_budget(&mut output_strings, session_id);
    for (i, new_output) in output_strings.into_iter().enumerate() {
        if new_output != outputs[i].output {
            invocations[i].output = new_output.clone();
            outputs[i].output = new_output;
        }
    }

    Ok(OpenAIToolResults {
        outputs,
        invocations,
    })
}

/// Serial fallback for single tool calls or when no parallelism is beneficial.
fn execute_openai_tool_calls_serial(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    tool_calls: &[OpenAIResponseToolCall],
    registry: &ToolRegistry,
    cwd: &std::path::Path,
    request_config: &OpenAIRequestConfig,
    model_id: &str,
    structured_output: Option<&StructuredOutputConfig>,
    tool_filter: Option<&super::RequestToolFilter>,
) -> Result<OpenAIToolResults> {
    let mut outputs = Vec::new();
    let mut invocations = Vec::new();
    for tool_call in tool_calls {
        let (output, success, metadata) = match execute_tool_call(
            state,
            resources,
            providers,
            auth_store,
            registry,
            model_id,
            cwd,
            ToolExecutionBackend::OpenAi {
                request_config,
                structured_output,
            },
            tool_filter,
            &tool_call.name,
            tool_call.arguments.clone(),
        ) {
            Ok(execution) => {
                let output = if execution.output.stderr.is_empty() {
                    execution.output.stdout
                } else if execution.output.stdout.is_empty() {
                    execution.output.stderr
                } else {
                    format!("{}\n{}", execution.output.stdout, execution.output.stderr)
                };
                (output, execution.success, execution.output.metadata)
            }
            Err(error) => (
                format!("Tool execution failed: {error}"),
                false,
                Value::Null,
            ),
        };
        let output = super::secrets::redact_known_secrets(state, &output);
        let metadata = super::secrets::redact_json_value(state, &metadata);
        let output =
            super::process_tool_result(&output, super::MAX_TOOL_RESULT_CHARS, &state.session.id);
        outputs.push(OpenAIResponsesFunctionCallOutput {
            kind: "function_call_output".to_string(),
            call_id: tool_call.call_id.clone(),
            output: output.clone(),
        });
        invocations.push(ToolInvocation {
            call_id: tool_call.call_id.clone(),
            tool_id: tool_call.name.clone(),
            input: serde_json::to_string(&tool_call.arguments)?,
            output,
            success,
            metadata,
            terminate: false,
        });
    }

    // Enforce per-message aggregate budget (CC: 200K).
    let mut output_strings: Vec<String> = outputs.iter().map(|o| o.output.clone()).collect();
    super::enforce_tool_result_budget(&mut output_strings, &state.session.id);
    for (i, new_output) in output_strings.into_iter().enumerate() {
        if new_output != outputs[i].output {
            invocations[i].output = new_output.clone();
            outputs[i].output = new_output;
        }
    }

    Ok(OpenAIToolResults {
        outputs,
        invocations,
    })
}

pub(super) fn parse_openai_text(response: &Value) -> Result<String> {
    if let Some(text) = response.get("output_text").and_then(Value::as_str) {
        return Ok(text.to_string());
    }

    let mut parts = Vec::new();
    if let Some(items) = response.get("output").and_then(Value::as_array) {
        for item in items {
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for block in content {
                    let block_type = block
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if matches!(block_type, "output_text" | "text") {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            parts.push(text.to_string());
                        }
                    }
                }
            }
        }
    }
    if parts.is_empty() {
        bail!("openai response did not contain output text");
    }
    Ok(parts.join("\n"))
}

pub(super) fn openai_request_instructions(
    state: &mut AppState,
    resources: &LoadedResources,
    system_prompt: Option<&str>,
) -> Result<String> {
    let mut sections = Vec::new();
    if let Some(system_prompt) = system_prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        sections.push(system_prompt.to_string());
    }
    if let Some(plan_mode_context) =
        crate::plan_mode::take_plan_mode_context_message(state, resources)?
            .map(|message| message.trim().to_string())
            .filter(|message| !message.is_empty())
    {
        sections.push(plan_mode_context);
    }
    // Dynamic context (date, git status, CLAUDE.md) is now injected as a
    // context user message in the `input` array, not here.  This keeps
    // `instructions` static and cacheable (matching Codex's design where
    // `instructions` = pure developer instructions, and contextual data
    // lives in `input` items).
    Ok(sections.join("\n\n"))
}

/// Builds the dynamic context message injected into the `input` array.
///
/// This follows CC/Codex's pattern of separating static instructions
/// (in `instructions`) from dynamic context (in `input` messages).
/// The `<system-reminder>` XML tag helps the model distinguish
/// system-injected context from user-authored messages.
pub(super) fn build_context_reminder_message(state: &AppState) -> String {
    let reminder = self::conversation::build_system_reminder(state, &super::git_status_context());
    format!(
        "<system-reminder>\n{}\n\n      IMPORTANT: this context may or may not be relevant to your tasks. You should not respond to this context unless it is highly relevant to your task.\n</system-reminder>",
        reminder
    )
}

pub(super) fn parse_openai_assistant_text(
    parsed: &OpenAIResponsesResponse,
    response: &Value,
    state: &AppState,
) -> Result<String> {
    let text = extract_responses_text(parsed);
    if text.trim().is_empty() {
        parse_openai_text(response).or_else(|_| parse_openai_text_fallback(response, state))
    } else {
        Ok(text)
    }
}

pub(super) fn parse_openai_text_fallback(response: &Value, state: &AppState) -> Result<String> {
    if let Some(text) = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        return Ok(text);
    }
    let output_kinds = response
        .get("output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("type").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    bail!(
        "provider {} returned an unsupported response shape for session {} (output types: {})",
        state.current_provider.as_deref().unwrap_or("unknown"),
        state.session.id,
        if output_kinds.is_empty() {
            "<none>"
        } else {
            output_kinds.as_str()
        }
    )
}

pub(super) fn resolve_openai_execution_config(
    state: &AppState,
    auth_store: &AuthStore,
    provider: &ProviderDescriptor,
) -> Result<OpenAIExecutionConfig> {
    let mut custom_headers = provider
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    // The execution config is built once per session (no model_id in
    // scope); compat-driven version-header gating consults the
    // descriptor in `support::append_default_openai_headers` only when
    // a model is supplied. Pass `None` here — auto-detect handles the
    // canonical providers (`provider.id == "openai"`).
    append_default_openai_headers(&mut custom_headers, provider.id.as_str(), None);
    let session_id = Some(state.session.id.to_string());
    let originator = OPENAI_CODEX_ORIGINATOR.to_string();
    match auth_store.get(provider.id.as_str()) {
        Some(StoredCredential::ApiKey { key }) => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: provider.base_url.clone(),
                version: openai_request_version(provider, false),
                auth: OpenAIAuth::ApiKey(key.clone()),
                originator,
                session_id,
                account_id: None,
                custom_headers,
                query_params: provider
                    .query_params
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                chat_completions_path: provider.chat_completions_path.clone(),
                responses_path: None,
            },
            refresh_token: None,
            codex_style: codex_style_for_provider(provider, false),
        }),
        Some(StoredCredential::OAuth(credential)) => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: openai_base_url_for_auth(provider, true),
                version: openai_request_version(provider, true),
                auth: OpenAIAuth::OAuthBearer(credential.access_token.clone()),
                originator,
                session_id,
                account_id: credential.account_id.clone(),
                custom_headers,
                query_params: provider
                    .query_params
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                chat_completions_path: provider.chat_completions_path.clone(),
                responses_path: None,
            },
            refresh_token: Some(credential.refresh_token.clone()),
            codex_style: codex_style_for_provider(provider, true),
        }),
        None if provider.auth_modes.is_empty() => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: provider.base_url.clone(),
                version: openai_request_version(provider, false),
                auth: OpenAIAuth::None,
                originator,
                session_id,
                account_id: None,
                custom_headers,
                query_params: provider
                    .query_params
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                chat_completions_path: provider.chat_completions_path.clone(),
                responses_path: None,
            },
            refresh_token: None,
            codex_style: codex_style_for_provider(provider, false),
        }),
        None => bail!(
            "no credentials configured for provider {}; use `puffer auth set-api-key {}` first",
            provider.id,
            provider.id
        ),
    }
}

fn codex_style_for_provider(provider: &ProviderDescriptor, oauth: bool) -> bool {
    let requested = if oauth && provider.id == "openai" {
        true
    } else {
        is_codex_openai_provider(provider)
    };
    requested
        && std::env::var("PUFFER_OPENAI_DISABLE_CODEX_STYLE")
            .ok()
            .as_deref()
            != Some("1")
}

/// Sends a blocking OpenAI request and refreshes OAuth credentials once after a 401.
pub(super) fn send_openai_request_with_refresh<F>(
    auth_store: &mut AuthStore,
    execution: &mut OpenAIExecutionConfig,
    proxy: &ProxyConfig,
    build_request: F,
) -> Result<Value>
where
    F: Fn(&OpenAIRequestConfig) -> Result<puffer_provider_openai::BuiltOpenAIRequest>,
{
    retry_openai_transport(
        || send_openai_request_with_refresh_once(auth_store, execution, proxy, &build_request),
        |_, _, _| {},
    )
}

fn send_openai_request_with_refresh_once<F>(
    auth_store: &mut AuthStore,
    execution: &mut OpenAIExecutionConfig,
    proxy: &ProxyConfig,
    build_request: &F,
) -> Result<Value>
where
    F: Fn(&OpenAIRequestConfig) -> Result<puffer_provider_openai::BuiltOpenAIRequest>,
{
    let request = build_request(&execution.request_config)?;
    let response = super::send_http_request_raw_with_proxy(
        &request.url,
        &request.headers,
        &request.body,
        false,
        proxy,
    )?;
    if response.status != StatusCode::UNAUTHORIZED || execution.refresh_token.is_none() {
        return parse_http_json_response(&request.url, false, response);
    }

    let refresh_token = execution
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing refresh token for OpenAI OAuth retry"))?;
    let refreshed = match crate::network::blocking_client_for_url(
        proxy,
        crate::network::HttpPurpose::OAuth,
        puffer_provider_openai::OPENAI_TOKEN_URL,
        std::time::Duration::from_secs(60),
    ) {
        Ok(client) => refresh_oauth_token_with_client(&client, &refresh_token),
        Err(_) => refresh_oauth_token(&refresh_token),
    }
    .context("failed to refresh OpenAI OAuth credentials after 401")?;
    let stored = openai_registry_credential(refreshed);
    execution.request_config.auth = OpenAIAuth::OAuthBearer(stored.access_token.clone());
    execution.request_config.account_id = stored.account_id.clone();
    execution.refresh_token = Some(stored.refresh_token.clone());
    auth_store.set_oauth(execution.provider_id.clone(), stored);

    let retry = build_request(&execution.request_config)?;
    let retry_response = super::send_http_request_raw_with_proxy(
        &retry.url,
        &retry.headers,
        &retry.body,
        false,
        proxy,
    )?;
    parse_http_json_response(&retry.url, false, retry_response)
}

/// Sends a streaming OpenAI request with OAuth refresh and transport-level retries.
pub(super) fn send_openai_request_with_refresh_streaming<F, G>(
    auth_store: &mut AuthStore,
    execution: &mut OpenAIExecutionConfig,
    proxy: &ProxyConfig,
    build_request: F,
    on_event: &mut G,
) -> Result<OpenAISseResult>
where
    F: Fn(&OpenAIRequestConfig) -> Result<puffer_provider_openai::BuiltOpenAIRequest>,
    G: FnMut(TurnStreamEvent),
{
    let request = build_request(&execution.request_config)?;
    // Layered retry: inner = connection-level (`retry_openai_transport`)
    // surfaces `RetryAttempt` events on `on_event`; outer =
    // HTTP 5xx response status (`runtime::retry_on_5xx`) traces
    // via `tracing::warn!` to avoid the closure borrow conflict
    // (both branches would otherwise want `&mut on_event`).
    // CC's SDK retries on >=500 the same way (`shouldRetry` in
    // claude-2.1.133 bundle).
    let response = super::retry_on_5xx(
        || {
            retry_openai_transport(
                || {
                    send_openai_request_stream_raw(
                        &request.url,
                        &request.headers,
                        &request.body,
                        proxy,
                    )
                },
                |attempt, max, error| {
                    on_event(TurnStreamEvent::RetryAttempt {
                        attempt,
                        max_attempts: max,
                        error: error.to_string(),
                    });
                },
            )
        },
        |attempt, max, status| {
            tracing::warn!(
                target: "puffer::runtime::openai",
                "5xx retry: attempt {attempt}/{max}, HTTP {}, sleeping before retry",
                status.as_u16()
            );
        },
    )?;
    if response.status() != StatusCode::UNAUTHORIZED || execution.refresh_token.is_none() {
        return parse_openai_stream_response(&request.url, response, on_event);
    }

    let refresh_token = execution
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing refresh token for OpenAI OAuth retry"))?;
    let refreshed = match crate::network::blocking_client_for_url(
        proxy,
        crate::network::HttpPurpose::OAuth,
        puffer_provider_openai::OPENAI_TOKEN_URL,
        std::time::Duration::from_secs(60),
    ) {
        Ok(client) => refresh_oauth_token_with_client(&client, &refresh_token),
        Err(_) => refresh_oauth_token(&refresh_token),
    }
    .context("failed to refresh OpenAI OAuth credentials after 401")?;
    let stored = openai_registry_credential(refreshed);
    execution.request_config.auth = OpenAIAuth::OAuthBearer(stored.access_token.clone());
    execution.request_config.account_id = stored.account_id.clone();
    execution.refresh_token = Some(stored.refresh_token.clone());
    auth_store.set_oauth(execution.provider_id.clone(), stored);

    let retry = build_request(&execution.request_config)?;
    let retry_response = super::retry_on_5xx(
        || {
            retry_openai_transport(
                || send_openai_request_stream_raw(&retry.url, &retry.headers, &retry.body, proxy),
                |attempt, max, error| {
                    on_event(TurnStreamEvent::RetryAttempt {
                        attempt,
                        max_attempts: max,
                        error: error.to_string(),
                    });
                },
            )
        },
        |attempt, max, status| {
            tracing::warn!(
                target: "puffer::runtime::openai",
                "5xx retry (post-401-refresh): attempt {attempt}/{max}, HTTP {}",
                status.as_u16()
            );
        },
    )?;
    parse_openai_stream_response(&retry.url, retry_response, on_event)
}

fn send_openai_request_stream_raw(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    proxy: &ProxyConfig,
) -> Result<Response> {
    trace_openai_http_request(url, headers, body);
    let client = crate::network::blocking_client_for_url(
        proxy,
        crate::network::HttpPurpose::Model,
        url,
        openai_stream_read_timeout(),
    )
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
    let response = request
        .body(body.to_string())
        .send()
        .with_context(|| format!("request to {url} failed"))?;
    trace_openai_http_response_headers(
        url,
        response.status().as_u16(),
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value: &reqwest::header::HeaderValue| value.to_str().ok()),
    );
    Ok(response)
}

fn parse_openai_stream_response<G>(
    url: &str,
    response: Response,
    on_event: &mut G,
) -> Result<OpenAISseResult>
where
    G: FnMut(TurnStreamEvent),
{
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    if !status.is_success() {
        // Use lossy decode (`unwrap_or_default`) instead of `?` so a
        // non-UTF8 / partially-truncated body still reaches the
        // classifier — for the quota-error path losing a byte or two
        // is preferable to surfacing a UTF-8 error and missing the
        // 429/403 promotion entirely. Mirrors anthropic.rs:266.
        let text = response.text().unwrap_or_default();
        // Promote 429 / 403-access-terminated to a typed `QuotaError`
        // so the benchmark CLI can exit with a distinct code instead
        // of letting the orchestration layer burn its retry budget on
        // a quota window. See `runtime::quota` for design notes.
        if let Some(quota) = super::quota::classify_response("openai", status.as_u16(), &text) {
            return Err(anyhow::Error::new(quota));
        }
        bail!("request failed with status {}: {}", status, text);
    }
    let mut reader = std::io::BufReader::new(response);
    let looks_like_sse = if is_event_stream(content_type.as_deref(), "") {
        true
    } else {
        let prefix = reader.fill_buf()?;
        let prefix = std::str::from_utf8(prefix).unwrap_or_default();
        is_event_stream(content_type.as_deref(), prefix)
    };
    if looks_like_sse {
        return match parse_openai_sse_reader_typed(reader, on_event) {
            Ok(result) => Ok(result),
            Err(error) if is_openai_sse_api_error(&error) => Err(error),
            Err(error) => {
                Err(error).with_context(|| format!("failed to parse SSE response from {url}"))
            }
        };
    }
    // Non-SSE fallback: parse JSON directly into typed struct — one parse, no roundtrip.
    let mut text = String::new();
    reader.read_to_string(&mut text)?;
    let raw: Value = serde_json::from_str(&text)
        .with_context(|| format!("response from {url} was not valid JSON"))?;
    if let Some(error) = openai_response_incomplete_error(&raw) {
        return Err(error);
    }
    let response_id = raw.get("id").and_then(Value::as_str).map(str::to_string);
    let input_tokens = raw
        .pointer("/usage/input_tokens")
        .and_then(Value::as_u64)
        .map(|v| v as usize);
    let parsed: OpenAIResponsesResponse = serde_json::from_value(raw.clone())
        .with_context(|| format!("response from {url} was not a valid Responses payload"))?;
    let assistant_text = extract_responses_text(&parsed);
    let tool_calls = extract_responses_tool_calls(&parsed)?;
    let reasoning_items: Vec<Value> = raw
        .pointer("/output")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    Ok(OpenAISseResult {
        response_id,
        input_tokens,
        output_tokens: None,
        cached_tokens: None,
        assistant_text,
        tool_calls,
        emitted_tool_call_ids: HashSet::new(),
        reasoning_items,
        raw_response: raw,
    })
}
