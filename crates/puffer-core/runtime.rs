use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_provider_openai::{
    build_chat_completions_request, build_responses_request, build_tool_responses_request,
    extract_chat_completions_text, extract_chat_completions_tool_calls, extract_responses_text,
    extract_responses_tool_calls, parse_chat_completions_response, parse_responses_response,
    OpenAIAuth, OpenAIChatCompletionsRequest, OpenAIChatFunctionCall, OpenAIChatMessage,
    OpenAIChatToolCall, OpenAIRequestConfig, OpenAIResponsesFunctionCallOutput,
    OpenAIResponsesRequest, OpenAIResponsesToolChoice, OpenAIResponsesToolChoiceMode,
    OpenAIResponsesToolRequest,
};
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
use serde_json::{json, Value};

mod hook_support;
mod local_tools;
mod mistral;
mod tool_support;
mod web_search;

use hook_support::{run_tool_hooks, run_turn_hooks};
use local_tools::{execute_runtime_local_tool, is_runtime_local_tool};
use tool_support::{
    anthropic_tool_definitions, enforce_tool_permission, is_provider_web_search_tool,
    merge_tool_output, openai_chat_completion_tools, openai_tool_definitions,
};
use web_search::{execute_anthropic_web_search, execute_openai_web_search};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Describes one tool call executed during a model turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocation {
    pub tool_id: String,
    pub input: String,
    pub output: String,
    pub success: bool,
}
/// Stores the visible result of one executed model turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnExecution {
    pub assistant_text: String,
    pub tool_invocations: Vec<ToolInvocation>,
}
/// Executes one user prompt against the currently selected provider and model.
pub fn execute_user_prompt(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    input: &str,
) -> Result<TurnExecution> {
    let (provider, model_id) = resolve_provider_and_model(state, providers)?;
    match resolve_model_api(state, providers, provider, &model_id).as_str() {
        "anthropic-messages" => {
            execute_anthropic(state, resources, provider, model_id, auth_store, input)
        }
        "openai-responses" | "azure-openai-responses" | "openai-codex-responses" => {
            execute_openai(state, resources, provider, model_id, auth_store, input)
        }
        "openai-completions" => {
            execute_openai_completions(state, resources, provider, model_id, auth_store, input)
        }
        "mistral-conversations" => {
            mistral::execute_turn(state, resources, provider, model_id, auth_store, input)
        }
        other => bail!(
            "provider {} with api {other} is not executable yet",
            provider.id
        ),
    }
}
fn resolve_provider_and_model<'a>(
    state: &AppState,
    providers: &'a ProviderRegistry,
) -> Result<(&'a ProviderDescriptor, String)> {
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
        .ok_or_else(|| anyhow!("no providers are registered"))?;
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
    state: &AppState,
    resources: &LoadedResources,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &AuthStore,
    input: &str,
) -> Result<TurnExecution> {
    let auth = anthropic_auth_for_provider(auth_store, provider)?;
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
    let registry = ToolRegistry::from_resources(resources);
    let mut messages = transcript_to_anthropic_messages(state, input);
    let mut invocations = Vec::new();
    let request = build_messages_request(
        &request_config,
        &AnthropicModelRequest {
            model: model_id.clone(),
            max_tokens: 1024,
            messages: transcript_to_anthropic_request_messages(state, input),
        },
    )?;

    for _ in 0..8 {
        let mut body = json!({
            "model": model_id,
            "max_tokens": 1024,
            "messages": messages,
            "system": [
                {
                    "type": "text",
                    "text": request.attribution_prefix_block.clone(),
                }
            ]
        });

        let tools = anthropic_tool_definitions(&registry, provider, &model_id);
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
        }

        let response = send_http_request(&request.url, &request.headers, &body.to_string(), true)?;
        if let Some(tool_results) = execute_anthropic_tool_calls(
            resources,
            &response,
            &registry,
            &state.cwd,
            &request_config,
            &model_id,
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
fn execute_openai(
    state: &AppState,
    resources: &LoadedResources,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &AuthStore,
    input: &str,
) -> Result<TurnExecution> {
    let auth = openai_auth_for_provider(auth_store, provider)?;
    let request_config = OpenAIRequestConfig {
        base_url: provider.base_url.clone(),
        version: APP_VERSION.to_string(),
        auth,
    };
    let registry = ToolRegistry::from_resources(resources);
    let tools = openai_tool_definitions(&registry, provider, &model_id);
    let mut previous_response_id = None;
    let mut next_input = transcript_to_openai_input(state, input);
    let mut invocations = Vec::new();

    for _ in 0..8 {
        let response = if tools.is_empty()
            && previous_response_id.is_none()
            && matches!(next_input, Value::String(_))
        {
            let request = build_responses_request(
                &request_config,
                &OpenAIResponsesRequest {
                    model: model_id.clone(),
                    input: next_input.as_str().unwrap_or_default().to_string(),
                },
            )?;
            send_http_request(&request.url, &request.headers, &request.body, false)?
        } else {
            let request = build_tool_responses_request(
                &request_config,
                &OpenAIResponsesToolRequest {
                    model: model_id.clone(),
                    input: next_input.clone(),
                    tools: tools.clone(),
                    include: Vec::new(),
                    tool_choice: if tools.is_empty() {
                        None
                    } else {
                        Some(OpenAIResponsesToolChoice::Mode(
                            OpenAIResponsesToolChoiceMode::Auto,
                        ))
                    },
                    previous_response_id: previous_response_id.clone(),
                },
            )?;
            send_http_request(&request.url, &request.headers, &request.body, false)?
        };

        let parsed = parse_responses_response(&serde_json::to_string(&response)?)?;
        let tool_calls = extract_responses_tool_calls(&parsed)?;
        if tool_calls.is_empty() {
            let assistant_text = parse_openai_assistant_text(&parsed, &response, state)?;
            run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
            return Ok(TurnExecution {
                assistant_text,
                tool_invocations: invocations,
            });
        }

        let response_id = parsed
            .id
            .clone()
            .ok_or_else(|| anyhow!("OpenAI response missing id for tool continuation"))?;
        let tool_results = execute_openai_tool_calls(
            resources,
            &tool_calls,
            &registry,
            &state.cwd,
            &request_config,
            &model_id,
        )?;
        invocations.extend(tool_results.invocations);
        previous_response_id = Some(response_id);
        next_input = json!(tool_results.outputs);
    }

    bail!("openai tool loop exceeded iteration limit")
}

fn execute_openai_completions(
    state: &AppState,
    resources: &LoadedResources,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &AuthStore,
    input: &str,
) -> Result<TurnExecution> {
    let auth = openai_auth_for_provider(auth_store, provider)?;
    let request_config = OpenAIRequestConfig {
        base_url: provider.base_url.clone(),
        version: APP_VERSION.to_string(),
        auth,
    };
    let registry = ToolRegistry::from_resources(resources);
    let tools = openai_chat_completion_tools(&registry, provider, &model_id);
    let mut messages = transcript_to_openai_chat_messages(state, input);
    let mut invocations = Vec::new();

    for _ in 0..8 {
        let request = build_chat_completions_request(
            &request_config,
            &OpenAIChatCompletionsRequest {
                model: model_id.clone(),
                messages: messages.clone(),
                tools: tools.clone(),
                tool_choice: if tools.is_empty() {
                    None
                } else {
                    Some(OpenAIResponsesToolChoiceMode::Auto)
                },
            },
        )?;
        let response = send_http_request(&request.url, &request.headers, &request.body, false)?;
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
            return Ok(TurnExecution {
                assistant_text,
                tool_invocations: invocations,
            });
        }

        let tool_results = execute_openai_tool_calls(
            resources,
            &tool_calls,
            &registry,
            &state.cwd,
            &request_config,
            &model_id,
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

    bail!("openai chat completions tool loop exceeded iteration limit")
}
fn send_http_request(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    anthropic: bool,
) -> Result<Value> {
    let client = Client::new();
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
    let text = response.text()?;
    if !status.is_success() {
        bail!("request failed with status {}: {}", status, text);
    }
    let json = serde_json::from_str::<Value>(&text)
        .with_context(|| format!("response from {url} was not valid JSON"))?;
    Ok(json)
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
fn openai_auth_for_provider(
    auth_store: &AuthStore,
    provider: &ProviderDescriptor,
) -> Result<OpenAIAuth> {
    match auth_store.get(&provider.id) {
        Some(StoredCredential::ApiKey { key }) => Ok(OpenAIAuth::ApiKey(key.clone())),
        Some(StoredCredential::OAuth(OAuthCredential { access_token, .. })) => {
            Ok(OpenAIAuth::OAuthBearer(access_token.clone()))
        }
        None if provider.auth_modes.is_empty() => Ok(OpenAIAuth::None),
        None => bail!(
            "no credentials configured for provider {}; use `puffer auth set-api-key {}` first",
            provider.id,
            provider.id
        ),
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
fn execute_anthropic_tool_calls(
    resources: &LoadedResources,
    response: &Value,
    registry: &ToolRegistry,
    cwd: &std::path::Path,
    request_config: &AnthropicRequestConfig,
    model_id: &str,
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
        if let Err(error) = enforce_tool_permission(registry, tool_id) {
            let output_text = error.to_string();
            run_tool_hooks(
                resources,
                cwd,
                "tool_end",
                tool_id,
                input,
                false,
                "",
                &output_text,
            );
            results.push(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": output_text,
                "is_error": true,
            }));
            invocations.push(ToolInvocation {
                tool_id: tool_id.to_string(),
                input: serde_json::to_string(input)?,
                output: output_text,
                success: false,
            });
            continue;
        }
        let definition = registry
            .definition(tool_id)
            .ok_or_else(|| anyhow!("unknown tool {tool_id}"))?;
        if is_runtime_local_tool(definition) {
            let output_text =
                execute_runtime_local_tool(resources, registry, definition, cwd, input.clone())?;
            run_tool_hooks(
                resources,
                cwd,
                "tool_end",
                tool_id,
                input,
                true,
                &output_text,
                "",
            );
            results.push(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": output_text,
                "is_error": false,
            }));
            invocations.push(ToolInvocation {
                tool_id: tool_id.to_string(),
                input: serde_json::to_string(input)?,
                output: output_text,
                success: true,
            });
            continue;
        }
        if registry
            .definition(tool_id)
            .is_some_and(is_provider_web_search_tool)
        {
            let output_text =
                execute_anthropic_web_search(request_config, model_id, input.clone())?;
            run_tool_hooks(
                resources,
                cwd,
                "tool_end",
                tool_id,
                input,
                true,
                &output_text,
                "",
            );
            results.push(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": output_text,
                "is_error": false,
            }));
            invocations.push(ToolInvocation {
                tool_id: tool_id.to_string(),
                input: serde_json::to_string(input)?,
                output: output_text,
                success: true,
            });
            continue;
        }
        let execution = registry.execute_json(tool_id, cwd, input.clone())?;
        run_tool_hooks(
            resources,
            cwd,
            "tool_end",
            tool_id,
            input,
            execution.success,
            &execution.output.stdout,
            &execution.output.stderr,
        );
        let output_text = merge_tool_output(execution.output.stdout, execution.output.stderr);
        results.push(json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": output_text,
            "is_error": !execution.success,
        }));
        invocations.push(ToolInvocation {
            tool_id: tool_id.to_string(),
            input: serde_json::to_string(input)?,
            output: output_text,
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

struct OpenAIToolResults {
    outputs: Vec<OpenAIResponsesFunctionCallOutput>,
    invocations: Vec<ToolInvocation>,
}

fn execute_openai_tool_calls(
    resources: &LoadedResources,
    tool_calls: &[puffer_provider_openai::OpenAIResponseToolCall],
    registry: &ToolRegistry,
    cwd: &std::path::Path,
    request_config: &OpenAIRequestConfig,
    model_id: &str,
) -> Result<OpenAIToolResults> {
    let mut outputs = Vec::new();
    let mut invocations = Vec::new();
    for tool_call in tool_calls {
        if let Err(error) = enforce_tool_permission(registry, &tool_call.name) {
            let output = error.to_string();
            run_tool_hooks(
                resources,
                cwd,
                "tool_end",
                &tool_call.name,
                &tool_call.arguments,
                false,
                "",
                &output,
            );
            outputs.push(OpenAIResponsesFunctionCallOutput {
                kind: "function_call_output".to_string(),
                call_id: tool_call.call_id.clone(),
                output: output.clone(),
            });
            invocations.push(ToolInvocation {
                tool_id: tool_call.name.clone(),
                input: serde_json::to_string(&tool_call.arguments)?,
                output,
                success: false,
            });
            continue;
        }
        let definition = registry
            .definition(&tool_call.name)
            .ok_or_else(|| anyhow!("unknown tool {}", tool_call.name))?;
        if is_runtime_local_tool(definition) {
            let output = execute_runtime_local_tool(
                resources,
                registry,
                definition,
                cwd,
                tool_call.arguments.clone(),
            )?;
            run_tool_hooks(
                resources,
                cwd,
                "tool_end",
                &tool_call.name,
                &tool_call.arguments,
                true,
                &output,
                "",
            );
            outputs.push(OpenAIResponsesFunctionCallOutput {
                kind: "function_call_output".to_string(),
                call_id: tool_call.call_id.clone(),
                output: output.clone(),
            });
            invocations.push(ToolInvocation {
                tool_id: tool_call.name.clone(),
                input: serde_json::to_string(&tool_call.arguments)?,
                output,
                success: true,
            });
            continue;
        }
        if registry
            .definition(&tool_call.name)
            .is_some_and(is_provider_web_search_tool)
        {
            let output =
                execute_openai_web_search(request_config, model_id, tool_call.arguments.clone())?;
            run_tool_hooks(
                resources,
                cwd,
                "tool_end",
                &tool_call.name,
                &tool_call.arguments,
                true,
                &output,
                "",
            );
            outputs.push(OpenAIResponsesFunctionCallOutput {
                kind: "function_call_output".to_string(),
                call_id: tool_call.call_id.clone(),
                output: output.clone(),
            });
            invocations.push(ToolInvocation {
                tool_id: tool_call.name.clone(),
                input: serde_json::to_string(&tool_call.arguments)?,
                output,
                success: true,
            });
            continue;
        }
        let execution = registry.execute_json(&tool_call.name, cwd, tool_call.arguments.clone())?;
        run_tool_hooks(
            resources,
            cwd,
            "tool_end",
            &tool_call.name,
            &tool_call.arguments,
            execution.success,
            &execution.output.stdout,
            &execution.output.stderr,
        );
        let output = merge_tool_output(execution.output.stdout, execution.output.stderr);
        outputs.push(OpenAIResponsesFunctionCallOutput {
            kind: "function_call_output".to_string(),
            call_id: tool_call.call_id.clone(),
            output: output.clone(),
        });
        invocations.push(ToolInvocation {
            tool_id: tool_call.name.clone(),
            input: serde_json::to_string(&tool_call.arguments)?,
            output,
            success: execution.success,
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
fn transcript_to_openai_input(state: &AppState, input: &str) -> Value {
    if state.transcript.is_empty() {
        return Value::String(input.to_string());
    }

    Value::Array(
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
            })
            .collect(),
    )
}

fn transcript_to_openai_chat_messages(state: &AppState, input: &str) -> Vec<OpenAIChatMessage> {
    let mut messages = state
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
        .collect::<Vec<_>>();
    if messages.is_empty() {
        messages.push(OpenAIChatMessage {
            role: "user".to_string(),
            content: Some(json!(input)),
            tool_call_id: None,
            tool_calls: Vec::new(),
        });
    }
    messages
}

fn parse_openai_assistant_text(
    parsed: &puffer_provider_openai::OpenAIResponsesResponse,
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
fn parse_openai_text_fallback(response: &Value, state: &AppState) -> Result<String> {
    if let Some(text) = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        return Ok(text);
    }
    bail!(
        "provider {} returned an unsupported response shape for session {}",
        state.current_provider.as_deref().unwrap_or("unknown"),
        state.session.id
    )
}
#[cfg(test)]
mod tests;
