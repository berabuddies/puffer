use super::{
    execute_tool_call, parse_http_json_response, run_turn_hooks, send_http_request_raw,
    ToolExecutionBackend, ToolInvocation, TurnStreamEvent, APP_VERSION,
};
use crate::permissions::load_runtime_permission_context;
mod support;

pub(super) use self::support::build_codex_openai_request_body;
use self::support::{
    append_default_openai_headers, apply_previous_response_id, compact_openai_input,
    extend_input_with_response_items, is_codex_openai_provider,
    is_openai_structured_output_error, next_openai_input,
    openai_base_url_for_auth, openai_model_supports_reasoning, openai_registry_credential,
    openai_responses_path, openai_stream_read_timeout, prefer_native_structured_output,
    retry_openai_transport, structured_output_endpoint_id, trace_openai_http_request,
    trace_openai_http_response_headers, OPENAI_STRUCTURED_OUTPUT_FAMILY,
};
use super::structured_output_support::{
    openai_chat_completion_tools_for_request, openai_chat_response_format,
    openai_responses_text_config, openai_tool_definitions_for_request, StructuredOutputConfig,
};
use super::system_prompt::render_runtime_system_prompt;
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_provider_openai::{
    build_chat_completions_request, build_json_post_request, extract_chat_completions_text,
    extract_chat_completions_tool_calls, extract_responses_text, extract_responses_tool_calls,
    parse_chat_completions_response, parse_responses_response, refresh_oauth_token, OpenAIAuth,
    OpenAIChatCompletionsRequest, OpenAIChatFunctionCall, OpenAIChatMessage, OpenAIChatToolCall,
    OpenAIRequestConfig, OpenAIResponseToolCall, OpenAIResponsesFunctionCallOutput,
    OpenAIResponsesResponse, OpenAIResponsesToolChoiceMode,
};
use puffer_provider_registry::{AuthStore, ProviderDescriptor, ProviderRegistry, StoredCredential};
use puffer_resources::LoadedResources;
use puffer_tools::ToolRegistry;
use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;
use serde_json::{json, Value};

pub(super) use super::openai_sse::{
    is_event_stream, parse_openai_sse_reader, parse_openai_sse_response,
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

pub(super) fn execute_openai(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
) -> Result<super::TurnExecution> {
    let structured_output = options.structured_output;
    let use_native = prefer_native_structured_output(state, provider, &model_id, structured_output);
    match execute_openai_once(
        state,
        resources,
        providers,
        provider,
        model_id.clone(),
        auth_store,
        input,
        options,
        use_native,
    ) {
        Ok(turn) => Ok(turn),
        Err(error) if use_native && is_openai_structured_output_error(&error) => {
            state.mark_native_structured_output_unsupported(
                OPENAI_STRUCTURED_OUTPUT_FAMILY,
                provider.id.as_str(),
                &model_id,
                structured_output_endpoint_id(provider),
            );
            execute_openai_once(
                state, resources, providers, provider, model_id, auth_store, input, options, false,
            )
        }
        Err(error) => Err(error),
    }
}

fn execute_openai_once(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
    use_native: bool,
) -> Result<super::TurnExecution> {
    let structured_output = options.structured_output;
    let mut execution = resolve_openai_execution_config(state, auth_store, provider)?;
    let registry = ToolRegistry::from_resources(resources);
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let text = openai_responses_text_config(structured_output, use_native);
    let tools = openai_tool_definitions_for_request(
        &registry,
        structured_output,
        use_native,
        Some(&permission_context),
        options.tool_filter,
    )?;
    let system_prompt = render_runtime_system_prompt(
        state,
        resources,
        &model_id,
        &tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<std::collections::BTreeSet<_>>(),
    )?;
    let instructions = openai_request_instructions(state, Some(&system_prompt))?;
    let mut next_input = transcript_to_openai_input(state, input)?;
    let mut invocations = Vec::new();
    let supports_reasoning = openai_model_supports_reasoning(provider, &model_id);
    let mut previous_response_id = None;

    loop {
        let response =
            send_openai_request_with_refresh(auth_store, &mut execution, |request_config| {
                let mut body = build_codex_openai_request_body(
                    state,
                    &model_id,
                    &instructions,
                    next_input.clone(),
                    &tools,
                    supports_reasoning,
                    text.clone(),
                    false,
                );
                apply_previous_response_id(&mut body, previous_response_id.as_deref());
                build_json_post_request(
                    request_config,
                    openai_responses_path(&request_config.base_url),
                    &body,
                )
            })?;

        let parsed = parse_responses_response(&serde_json::to_string(&response)?)?;
        previous_response_id = parsed.id.clone();
        let tool_calls = extract_responses_tool_calls(&parsed)?;
        if tool_calls.is_empty() {
            let assistant_text = parse_openai_assistant_text(&parsed, &response, state)?;
            run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
            return Ok(super::TurnExecution {
                assistant_text,
                tool_invocations: invocations,
            });
        }

        let cwd = state.cwd.clone();
        let tool_results = execute_openai_tool_calls(
            state,
            resources,
            providers,
            auth_store,
            &tool_calls,
            &registry,
            &cwd,
            &execution.request_config,
            &model_id,
            structured_output,
            options.tool_filter,
        )?;
        invocations.extend(tool_results.invocations);
        next_input = next_openai_input(
            previous_response_id.as_deref(),
            next_input,
            continuation_input(&tool_calls, &tool_results.outputs),
        );
        compact_openai_input(&mut next_input, provider, &model_id);
    }
}

pub(super) fn execute_openai_streaming<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
    on_event: &mut F,
) -> Result<super::TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    let structured_output = options.structured_output;
    let use_native = prefer_native_structured_output(state, provider, &model_id, structured_output);
    match execute_openai_streaming_once(
        state,
        resources,
        providers,
        provider,
        model_id.clone(),
        auth_store,
        input,
        options,
        use_native,
        on_event,
    ) {
        Ok(turn) => Ok(turn),
        Err(error) if use_native && is_openai_structured_output_error(&error) => {
            state.mark_native_structured_output_unsupported(
                OPENAI_STRUCTURED_OUTPUT_FAMILY,
                provider.id.as_str(),
                &model_id,
                structured_output_endpoint_id(provider),
            );
            execute_openai_streaming_once(
                state, resources, providers, provider, model_id, auth_store, input, options, false,
                on_event,
            )
        }
        Err(error) => Err(error),
    }
}

fn execute_openai_streaming_once<F>(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
    use_native: bool,
    on_event: &mut F,
) -> Result<super::TurnExecution>
where
    F: FnMut(TurnStreamEvent),
{
    let structured_output = options.structured_output;
    let mut execution = resolve_openai_execution_config(state, auth_store, provider)?;

    let registry = ToolRegistry::from_resources(resources);
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let text = openai_responses_text_config(structured_output, use_native);
    let tools = openai_tool_definitions_for_request(
        &registry,
        structured_output,
        use_native,
        Some(&permission_context),
        options.tool_filter,
    )?;
    let system_prompt = render_runtime_system_prompt(
        state,
        resources,
        &model_id,
        &tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<std::collections::BTreeSet<_>>(),
    )?;
    let instructions = openai_request_instructions(state, Some(&system_prompt))?;
    let mut next_input = transcript_to_openai_input(state, input)?;
    let mut invocations = Vec::new();
    let supports_reasoning = openai_model_supports_reasoning(provider, &model_id);

    loop {
        let response = retry_openai_transport(|| {
            send_openai_request_with_refresh_streaming(
                auth_store,
                &mut execution,
                |request_config| {
                    let body = build_codex_openai_request_body(
                        state,
                        &model_id,
                        &instructions,
                        next_input.clone(),
                        &tools,
                        supports_reasoning,
                        text.clone(),
                        true,
                    );
                    build_json_post_request(
                        request_config,
                        openai_responses_path(&request_config.base_url),
                        &body,
                    )
                },
                on_event,
            )
        })?;

        let parsed = parse_responses_response(&serde_json::to_string(&response)?)?;
        let tool_calls = extract_responses_tool_calls(&parsed)?;
        if tool_calls.is_empty() {
            let assistant_text = parse_openai_assistant_text(&parsed, &response, state)?;
            run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
            return Ok(super::TurnExecution {
                assistant_text,
                tool_invocations: invocations,
            });
        }

        on_event(TurnStreamEvent::ToolCallsRequested(
            tool_calls
                .iter()
                .map(|tool_call| super::ToolCallRequest {
                    tool_id: tool_call.name.clone(),
                    input: serde_json::to_string(&tool_call.arguments).unwrap_or_default(),
                })
                .collect(),
        ));
        let cwd = state.cwd.clone();
        let tool_results = execute_openai_tool_calls(
            state,
            resources,
            providers,
            auth_store,
            &tool_calls,
            &registry,
            &cwd,
            &execution.request_config,
            &model_id,
            structured_output,
            options.tool_filter,
        )?;
        if !tool_results.invocations.is_empty() {
            on_event(TurnStreamEvent::ToolInvocations(
                tool_results.invocations.clone(),
            ));
        }
        invocations.extend(tool_results.invocations);
        next_input = extend_input_with_response_items(
            next_input,
            &response,
            continuation_input(&tool_calls, &tool_results.outputs),
        );
        // Context management between tool iterations (CC/Codex parity).
        // Truncate old items in the input array to stay within budget.
        compact_openai_input(&mut next_input, provider, &model_id);
    }
}

pub(super) fn execute_openai_completions(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
) -> Result<super::TurnExecution> {
    let structured_output = options.structured_output;
    let use_native = prefer_native_structured_output(state, provider, &model_id, structured_output);
    match execute_openai_completions_once(
        state,
        resources,
        providers,
        provider,
        model_id.clone(),
        auth_store,
        input,
        options,
        use_native,
    ) {
        Ok(turn) => Ok(turn),
        Err(error) if use_native && is_openai_structured_output_error(&error) => {
            state.mark_native_structured_output_unsupported(
                OPENAI_STRUCTURED_OUTPUT_FAMILY,
                provider.id.as_str(),
                &model_id,
                structured_output_endpoint_id(provider),
            );
            execute_openai_completions_once(
                state, resources, providers, provider, model_id, auth_store, input, options, false,
            )
        }
        Err(error) => Err(error),
    }
}

fn execute_openai_completions_once(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &mut AuthStore,
    input: &str,
    options: super::TurnRequestOptions<'_>,
    use_native: bool,
) -> Result<super::TurnExecution> {
    let structured_output = options.structured_output;
    let mut execution = resolve_openai_execution_config(state, auth_store, provider)?;
    let registry = ToolRegistry::from_resources(resources);
    let permission_context = load_runtime_permission_context(&state.cwd, resources, state)?;
    let response_format = openai_chat_response_format(structured_output, use_native);
    let tools = openai_chat_completion_tools_for_request(
        &registry,
        structured_output,
        use_native,
        Some(&permission_context),
        options.tool_filter,
    )?;
    let system_prompt = render_runtime_system_prompt(
        state,
        resources,
        &model_id,
        &tools
            .iter()
            .map(|tool| tool.function.name.clone())
            .collect::<std::collections::BTreeSet<_>>(),
    )?;
    let mut messages = transcript_to_openai_chat_messages(state, input, Some(&system_prompt))?;
    let mut invocations = Vec::new();

    loop {
        let response =
            send_openai_request_with_refresh(auth_store, &mut execution, |request_config| {
                build_chat_completions_request(
                    request_config,
                    &OpenAIChatCompletionsRequest {
                        model: model_id.clone(),
                        messages: messages.clone(),
                        tools: tools.clone(),
                        tool_choice: if tools.is_empty() {
                            None
                        } else {
                            Some(OpenAIResponsesToolChoiceMode::Auto)
                        },
                        response_format: response_format.clone(),
                    },
                )
            })?;
        let parsed = parse_chat_completions_response(&serde_json::to_string(&response)?)?;
        let tool_calls = extract_chat_completions_tool_calls(&parsed)?;
        let choice = parsed
            .choices
            .first()
            .ok_or_else(|| anyhow!("OpenAI Chat Completions response did not contain choices"))?;
        if tool_calls.is_empty() {
            let text = extract_chat_completions_text(&parsed);
            let assistant_text = if text.trim().is_empty() {
                parse_openai_text(&response)
                    .or_else(|_| parse_openai_text_fallback(&response, state))?
            } else {
                text
            };
            run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
            return Ok(super::TurnExecution {
                assistant_text,
                tool_invocations: invocations,
            });
        }

        let cwd = state.cwd.clone();
        let tool_results = execute_openai_tool_calls(
            state,
            resources,
            providers,
            auth_store,
            &tool_calls,
            &registry,
            &cwd,
            &execution.request_config,
            &model_id,
            structured_output,
            options.tool_filter,
        )?;
        invocations.extend(tool_results.invocations);
        messages.push(OpenAIChatMessage {
            role: choice
                .message
                .role
                .clone()
                .unwrap_or_else(|| "assistant".to_string()),
            content: choice.message.content.clone(),
            tool_call_id: None,
            tool_calls: tool_calls
                .iter()
                .map(|tool_call| OpenAIChatToolCall {
                    id: tool_call.call_id.clone(),
                    kind: "function".to_string(),
                    function: OpenAIChatFunctionCall {
                        name: tool_call.name.clone(),
                        arguments: serde_json::to_string(&tool_call.arguments)
                            .unwrap_or_else(|_| "{}".to_string()),
                    },
                })
                .collect(),
        });
        for output in tool_results.outputs {
            messages.push(OpenAIChatMessage {
                role: "tool".to_string(),
                content: Some(json!(output.output)),
                tool_call_id: Some(output.call_id),
                tool_calls: Vec::new(),
            });
        }
    }
}

fn continuation_input(
    tool_calls: &[OpenAIResponseToolCall],
    outputs: &[OpenAIResponsesFunctionCallOutput],
) -> Value {
    let mut items = Vec::with_capacity(tool_calls.len() + outputs.len());
    for tool_call in tool_calls {
        items.push(json!({
            "type": "function_call",
            "call_id": tool_call.call_id,
            "name": tool_call.name,
            "arguments": serde_json::to_string(&tool_call.arguments).unwrap_or_else(|_| "{}".to_string()),
        }));
    }
    items.extend(outputs.iter().map(|output| json!(output)));
    Value::Array(items)
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
    let mut outputs = Vec::new();
    let mut invocations = Vec::new();
    for tool_call in tool_calls {
        let (output, success) = match execute_tool_call(
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
                (output, execution.success)
            }
            Err(error) => (format!("Tool execution failed: {error}"), false),
        };
        outputs.push(OpenAIResponsesFunctionCallOutput {
            kind: "function_call_output".to_string(),
            call_id: tool_call.call_id.clone(),
            output: output.clone(),
        });
        invocations.push(ToolInvocation {
            tool_id: tool_call.name.clone(),
            input: serde_json::to_string(&tool_call.arguments)?,
            output,
            success,
        });
    }
    Ok(OpenAIToolResults {
        outputs,
        invocations,
    })
}

fn parse_openai_text(response: &Value) -> Result<String> {
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

pub(super) fn transcript_to_openai_input(state: &AppState, input: &str) -> Result<Value> {
    if state.transcript.is_empty() {
        return Ok(Value::String(input.to_string()));
    }

    let mut items = Vec::new();
    items.extend(
        state
            .transcript
            .iter()
            .enumerate()
            .map(|(index, message)| match message.role {
                crate::MessageRole::User => json!({
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": message.text,
                        }
                    ],
                }),
                crate::MessageRole::Assistant => json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": message.text,
                            "annotations": [],
                        }
                    ],
                    "status": "completed",
                    "id": format!("msg_{index}"),
                }),
                crate::MessageRole::System => json!({
                    "role": "system",
                    "content": message.text,
                }),
            }),
    );
    Ok(Value::Array(items))
}

fn openai_request_instructions(state: &AppState, system_prompt: Option<&str>) -> Result<String> {
    let mut sections = Vec::new();
    if let Some(system_prompt) = system_prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        sections.push(system_prompt.to_string());
    }
    if let Some(plan_mode_context) =
        crate::command_helpers::prompt::plan_mode_context_message(state)?
            .map(|message| message.trim().to_string())
            .filter(|message| !message.is_empty())
    {
        sections.push(plan_mode_context);
    }
    Ok(sections.join("\n\n"))
}

pub(super) fn transcript_to_openai_chat_messages(
    state: &AppState,
    input: &str,
    system_prompt: Option<&str>,
) -> Result<Vec<OpenAIChatMessage>> {
    let plan_mode_context = crate::command_helpers::prompt::plan_mode_context_message(state)?;
    let mut messages = Vec::new();
    if let Some(system_prompt) = system_prompt.filter(|prompt| !prompt.trim().is_empty()) {
        messages.push(OpenAIChatMessage {
            role: "system".to_string(),
            content: Some(json!(system_prompt)),
            tool_call_id: None,
            tool_calls: Vec::new(),
        });
    }
    if let Some(plan_mode_context) = plan_mode_context {
        messages.push(OpenAIChatMessage {
            role: "system".to_string(),
            content: Some(json!(plan_mode_context)),
            tool_call_id: None,
            tool_calls: Vec::new(),
        });
    }
    messages.extend(
        state
            .transcript
            .iter()
            .map(|message| OpenAIChatMessage {
                role: match message.role {
                    crate::MessageRole::User => "user".to_string(),
                    crate::MessageRole::Assistant => "assistant".to_string(),
                    crate::MessageRole::System => "system".to_string(),
                },
                content: Some(json!(message.text)),
                tool_call_id: None,
                tool_calls: Vec::new(),
            })
            .collect::<Vec<_>>(),
    );
    if messages.is_empty() {
        messages.push(OpenAIChatMessage {
            role: "user".to_string(),
            content: Some(json!(input)),
            tool_call_id: None,
            tool_calls: Vec::new(),
        });
    }
    Ok(messages)
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
    append_default_openai_headers(&mut custom_headers, provider.id.as_str());
    let session_id = Some(state.session.id.to_string());
    let originator = OPENAI_CODEX_ORIGINATOR.to_string();
    match auth_store.get(provider.id.as_str()) {
        Some(StoredCredential::ApiKey { key }) => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: provider.base_url.clone(),
                version: APP_VERSION.to_string(),
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
            },
            refresh_token: None,
            codex_style: codex_style_for_provider(provider, false),
        }),
        Some(StoredCredential::OAuth(credential)) => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: openai_base_url_for_auth(provider, true),
                version: APP_VERSION.to_string(),
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
            },
            refresh_token: Some(credential.refresh_token.clone()),
            codex_style: codex_style_for_provider(provider, true),
        }),
        None if provider.auth_modes.is_empty() => Ok(OpenAIExecutionConfig {
            provider_id: provider.id.clone(),
            request_config: OpenAIRequestConfig {
                base_url: provider.base_url.clone(),
                version: APP_VERSION.to_string(),
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

pub(super) fn send_openai_request_with_refresh<F>(
    auth_store: &mut AuthStore,
    execution: &mut OpenAIExecutionConfig,
    build_request: F,
) -> Result<Value>
where
    F: Fn(&OpenAIRequestConfig) -> Result<puffer_provider_openai::BuiltOpenAIRequest>,
{
    retry_openai_transport(|| {
        send_openai_request_with_refresh_once(auth_store, execution, &build_request)
    })
}

fn send_openai_request_with_refresh_once<F>(
    auth_store: &mut AuthStore,
    execution: &mut OpenAIExecutionConfig,
    build_request: &F,
) -> Result<Value>
where
    F: Fn(&OpenAIRequestConfig) -> Result<puffer_provider_openai::BuiltOpenAIRequest>,
{
    let request = build_request(&execution.request_config)?;
    let response = send_http_request_raw(&request.url, &request.headers, &request.body, false)?;
    if response.status != StatusCode::UNAUTHORIZED || execution.refresh_token.is_none() {
        return parse_http_json_response(&request.url, false, response);
    }

    let refresh_token = execution
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing refresh token for OpenAI OAuth retry"))?;
    let refreshed = refresh_oauth_token(&refresh_token)
        .context("failed to refresh OpenAI OAuth credentials after 401")?;
    let stored = openai_registry_credential(refreshed);
    execution.request_config.auth = OpenAIAuth::OAuthBearer(stored.access_token.clone());
    execution.request_config.account_id = stored.account_id.clone();
    execution.refresh_token = Some(stored.refresh_token.clone());
    auth_store.set_oauth(execution.provider_id.clone(), stored);

    let retry = build_request(&execution.request_config)?;
    let retry_response = send_http_request_raw(&retry.url, &retry.headers, &retry.body, false)?;
    parse_http_json_response(&retry.url, false, retry_response)
}

fn send_openai_request_with_refresh_streaming<F, G>(
    auth_store: &mut AuthStore,
    execution: &mut OpenAIExecutionConfig,
    build_request: F,
    on_event: &mut G,
) -> Result<Value>
where
    F: Fn(&OpenAIRequestConfig) -> Result<puffer_provider_openai::BuiltOpenAIRequest>,
    G: FnMut(TurnStreamEvent),
{
    let request = build_request(&execution.request_config)?;
    let response = retry_openai_transport(|| {
        send_openai_request_stream_raw(&request.url, &request.headers, &request.body)
    })?;
    if response.status() != StatusCode::UNAUTHORIZED || execution.refresh_token.is_none() {
        return parse_openai_stream_response(&request.url, response, on_event);
    }

    let refresh_token = execution
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing refresh token for OpenAI OAuth retry"))?;
    let refreshed = refresh_oauth_token(&refresh_token)
        .context("failed to refresh OpenAI OAuth credentials after 401")?;
    let stored = openai_registry_credential(refreshed);
    execution.request_config.auth = OpenAIAuth::OAuthBearer(stored.access_token.clone());
    execution.request_config.account_id = stored.account_id.clone();
    execution.refresh_token = Some(stored.refresh_token.clone());
    auth_store.set_oauth(execution.provider_id.clone(), stored);

    let retry = build_request(&execution.request_config)?;
    let retry_response = retry_openai_transport(|| {
        send_openai_request_stream_raw(&retry.url, &retry.headers, &retry.body)
    })?;
    parse_openai_stream_response(&retry.url, retry_response, on_event)
}

fn send_openai_request_stream_raw(
    url: &str,
    headers: &[(String, String)],
    body: &str,
) -> Result<Response> {
    trace_openai_http_request(url, headers, body);
    let client = Client::builder()
        .timeout(openai_stream_read_timeout())
        .build()?;
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

fn parse_openai_stream_response<G>(url: &str, response: Response, on_event: &mut G) -> Result<Value>
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
        let text = response.text()?;
        bail!("request failed with status {}: {}", status, text);
    }
    if is_event_stream(content_type.as_deref(), "") {
        return parse_openai_sse_reader(std::io::BufReader::new(response), on_event)
            .with_context(|| format!("failed to parse SSE response from {url}"));
    }
    let text = response.text()?;
    serde_json::from_str::<Value>(&text)
        .with_context(|| format!("response from {url} was not valid JSON"))
}
