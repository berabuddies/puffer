use super::*;
use puffer_provider_mistral::{
    build_chat_request, extract_chat_text, extract_chat_tool_calls, parse_chat_response,
    MistralAuth, MistralChatMessage, MistralChatRequest, MistralRequestConfig, MistralTool,
    MistralToolChoice, MistralToolChoiceMode, MistralToolFunctionDefinition,
};

pub(super) fn execute_turn(
    state: &AppState,
    resources: &LoadedResources,
    provider: &ProviderDescriptor,
    model_id: String,
    auth_store: &AuthStore,
    input: &str,
) -> Result<TurnExecution> {
    let auth = mistral_auth_for_provider(auth_store, provider)?;
    let request_config = MistralRequestConfig {
        base_url: provider.base_url.clone(),
        version: APP_VERSION.to_string(),
        auth,
        custom_headers: provider.headers.clone(),
        session_id: Some(state.session.id.to_string()),
    };
    let registry = ToolRegistry::from_resources(resources);
    let tools = mistral_tool_definitions(&registry);
    let mut messages = transcript_to_mistral_messages(state, input);
    let mut invocations = Vec::new();

    for _ in 0..8 {
        let request = build_chat_request(
            &request_config,
            &MistralChatRequest {
                model: model_id.clone(),
                messages: messages.clone(),
                tools: tools.clone(),
                tool_choice: if tools.is_empty() {
                    None
                } else {
                    Some(MistralToolChoice::Mode(MistralToolChoiceMode::Auto))
                },
                max_tokens: Some(1024),
                stream: false,
            },
        )?;
        let response = send_http_request(&request.url, &request.headers, &request.body, false)?;
        let parsed = parse_chat_response(&serde_json::to_string(&response)?)?;
        let tool_calls = extract_chat_tool_calls(&parsed)?;
        let choice = parsed
            .choices
            .first()
            .ok_or_else(|| anyhow!("Mistral chat response did not contain choices"))?;
        if tool_calls.is_empty() {
            let assistant_text = extract_chat_text(&parsed);
            if assistant_text.trim().is_empty() {
                bail!("mistral response did not contain text content");
            }
            run_turn_hooks(resources, &state.cwd, &assistant_text, invocations.len());
            return Ok(TurnExecution {
                assistant_text,
                tool_invocations: invocations,
            });
        }

        let openai_calls = tool_calls
            .iter()
            .map(|tool_call| puffer_provider_openai::OpenAIResponseToolCall {
                item_id: None,
                status: None,
                call_id: tool_call.call_id.clone(),
                name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
            })
            .collect::<Vec<_>>();
        let tool_results = execute_openai_tool_calls(
            resources,
            &openai_calls,
            &registry,
            &state.cwd,
            &OpenAIRequestConfig {
                base_url: provider.base_url.clone(),
                version: APP_VERSION.to_string(),
                auth: OpenAIAuth::None,
            },
            &model_id,
        )?;
        invocations.extend(tool_results.invocations);
        messages.push(MistralChatMessage {
            role: choice
                .message
                .role
                .clone()
                .unwrap_or_else(|| "assistant".to_string()),
            content: choice.message.content.clone(),
            tool_call_id: None,
            name: None,
            tool_calls: choice.message.tool_calls.clone(),
        });
        for (output, tool_call) in tool_results.outputs.into_iter().zip(tool_calls.iter()) {
            messages.push(MistralChatMessage {
                role: "tool".to_string(),
                content: Some(json!(output.output)),
                tool_call_id: Some(output.call_id),
                name: Some(tool_call.name.clone()),
                tool_calls: Vec::new(),
            });
        }
    }

    bail!("mistral tool loop exceeded iteration limit")
}

fn mistral_auth_for_provider(
    auth_store: &AuthStore,
    provider: &ProviderDescriptor,
) -> Result<MistralAuth> {
    match auth_store.get(&provider.id) {
        Some(StoredCredential::ApiKey { key }) => Ok(MistralAuth::ApiKey(key.clone())),
        Some(StoredCredential::OAuth(OAuthCredential { access_token, .. })) => {
            Ok(MistralAuth::ApiKey(access_token.clone()))
        }
        None if provider.auth_modes.is_empty() => Ok(MistralAuth::None),
        None => bail!(
            "no credentials configured for provider {}; use `puffer auth set-api-key {}` first",
            provider.id,
            provider.id
        ),
    }
}

fn mistral_tool_definitions(registry: &ToolRegistry) -> Vec<MistralTool> {
    registry
        .definitions()
        .map(|definition| MistralTool {
            kind: "function".to_string(),
            function: MistralToolFunctionDefinition {
                name: definition.id.clone(),
                description: definition.description.clone(),
                parameters: definition.input_schema.as_json_schema(),
                strict: false,
            },
        })
        .collect()
}

fn transcript_to_mistral_messages(state: &AppState, input: &str) -> Vec<MistralChatMessage> {
    let mut messages = state
        .transcript
        .iter()
        .map(|message| MistralChatMessage {
            role: match message.role {
                crate::MessageRole::User => "user".to_string(),
                crate::MessageRole::Assistant => "assistant".to_string(),
                crate::MessageRole::System => "system".to_string(),
            },
            content: Some(json!(message.text)),
            tool_call_id: None,
            name: None,
            tool_calls: Vec::new(),
        })
        .collect::<Vec<_>>();
    if messages.is_empty() {
        messages.push(MistralChatMessage {
            role: "user".to_string(),
            content: Some(json!(input)),
            tool_call_id: None,
            name: None,
            tool_calls: Vec::new(),
        });
    }
    messages
}
