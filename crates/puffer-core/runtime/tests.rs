use super::*;
use puffer_config::PufferConfig;
use puffer_provider_openai::OpenAIResponseToolCall;
use puffer_provider_registry::{AuthMode, ProviderDescriptor};
use puffer_resources::{LoadedItem, LoadedResources, SourceInfo, SourceKind, ToolSpec};
use puffer_session_store::SessionMetadata;
use uuid::Uuid;

fn provider() -> ProviderDescriptor {
    ProviderDescriptor {
        id: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        default_api: "anthropic-messages".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: Default::default(),
        discovery: None,
        models: vec![puffer_provider_registry::ModelDescriptor {
            id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            provider: "anthropic".to_string(),
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8192,
            supports_reasoning: true,
        }],
    }
}

fn openai_provider() -> ProviderDescriptor {
    ProviderDescriptor {
        id: "openai".to_string(),
        display_name: "OpenAI".to_string(),
        base_url: "https://api.openai.com".to_string(),
        default_api: "openai-responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: Default::default(),
        discovery: None,
        models: vec![puffer_provider_registry::ModelDescriptor {
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            provider: "openai".to_string(),
            api: "openai-responses".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
        }],
    }
}

fn state() -> AppState {
    AppState::new(
        PufferConfig::default(),
        std::env::current_dir().unwrap(),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            cwd: std::env::current_dir().unwrap(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        },
    )
}

fn anthropic_request_config() -> AnthropicRequestConfig {
    AnthropicRequestConfig {
        base_url: "https://api.anthropic.com".to_string(),
        session_id: "session-test".to_string(),
        custom_headers: Default::default(),
        remote_container_id: None,
        remote_session_id: None,
        client_app: None,
        entrypoint: "cli".to_string(),
        user_type: "external".to_string(),
        version: "0.1.0".to_string(),
        workload: None,
        additional_protection: false,
        cch_enabled: true,
        auth: AnthropicAuth::ApiKey("sk-ant".to_string()),
        beta_header: None,
        client_request_id: None,
    }
}

fn openai_request_config() -> OpenAIRequestConfig {
    OpenAIRequestConfig {
        base_url: "https://api.openai.com".to_string(),
        version: "0.1.0".to_string(),
        auth: OpenAIAuth::ApiKey("sk-openai".to_string()),
    }
}

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

fn loaded_tool(id: &str, description: &str, handler: &str) -> LoadedItem<ToolSpec> {
    LoadedItem {
        value: ToolSpec {
            id: id.to_string(),
            name: id.to_string(),
            description: description.to_string(),
            handler: handler.to_string(),
            handler_args: Vec::new(),
            approval_policy: None,
            sandbox_policy: None,
            shared_lib: None,
            enabled_if: None,
            input_schema: None,
            metadata: Default::default(),
            display: Default::default(),
        },
        source_info: SourceInfo {
            path: format!("{id}.yaml").into(),
            kind: SourceKind::Builtin,
        },
    }
}

#[test]
fn anthropic_tool_schema_lists_expected_fields() {
    let schema = anthropic_tool_schema("write_file");
    let required = schema.get("required").and_then(Value::as_array).unwrap();
    assert!(required.iter().any(|value| value == "path"));
    assert!(required.iter().any(|value| value == "contents"));
}

#[test]
fn resolve_selection_uses_first_provider_model_when_unset() {
    let mut registry = ProviderRegistry::new();
    registry.register(provider());
    let state = state();
    let (provider, model_id) = resolve_provider_and_model(&state, &registry).unwrap();
    assert_eq!(provider.id, "anthropic");
    assert_eq!(model_id, "claude-sonnet-4-5");
}

#[test]
fn resolve_model_api_supports_custom_provider_families() {
    let mut descriptor = provider();
    descriptor.id = "custom-openai".to_string();
    descriptor.display_name = "Custom OpenAI".to_string();
    descriptor.base_url = "https://example.invalid".to_string();
    descriptor.default_api = "openai-responses".to_string();
    descriptor.models[0].provider = "custom-openai".to_string();
    descriptor.models[0].api = "openai-responses".to_string();
    let mut registry = ProviderRegistry::new();
    registry.register(descriptor);
    let mut state = state();
    state.current_provider = Some("custom-openai".to_string());
    let (provider, model_id) = resolve_provider_and_model(&state, &registry).unwrap();
    assert_eq!(
        resolve_model_api(&state, &registry, provider, &model_id),
        "openai-responses"
    );
}

#[test]
fn execute_user_prompt_accepts_openai_family_aliases() {
    let mut descriptor = provider();
    descriptor.id = "azure-openai".to_string();
    descriptor.display_name = "Azure OpenAI".to_string();
    descriptor.base_url = "https://example.invalid".to_string();
    descriptor.default_api = "azure-openai-responses".to_string();
    descriptor.models[0].provider = "azure-openai".to_string();
    descriptor.models[0].api = "azure-openai-responses".to_string();
    let mut registry = ProviderRegistry::new();
    registry.register(descriptor);
    let mut azure_state = state();
    azure_state.current_provider = Some("azure-openai".to_string());
    azure_state.current_model = Some("azure-openai/claude-sonnet-4-5".to_string());
    let mut auth = AuthStore::default();
    auth.set_api_key("azure-openai", "sk-test");
    let error = execute_user_prompt(
        &azure_state,
        &LoadedResources::default(),
        &registry,
        &auth,
        "hello",
    )
    .unwrap_err();
    assert!(!error
        .to_string()
        .contains("provider azure-openai with api azure-openai-responses is not executable yet"));

    let mut descriptor = provider();
    descriptor.id = "openrouter".to_string();
    descriptor.display_name = "OpenRouter".to_string();
    descriptor.base_url = "https://example.invalid".to_string();
    descriptor.default_api = "openai-completions".to_string();
    descriptor.models[0].provider = "openrouter".to_string();
    descriptor.models[0].api = "openai-completions".to_string();
    let mut registry = ProviderRegistry::new();
    registry.register(descriptor);
    let mut openrouter_state = state();
    openrouter_state.current_provider = Some("openrouter".to_string());
    openrouter_state.current_model = Some("openrouter/claude-sonnet-4-5".to_string());
    let mut auth = AuthStore::default();
    auth.set_api_key("openrouter", "sk-test");
    let error = execute_user_prompt(
        &openrouter_state,
        &LoadedResources::default(),
        &registry,
        &auth,
        "hello",
    )
    .unwrap_err();
    assert!(!error
        .to_string()
        .contains("provider openrouter with api openai-completions is not executable yet"));

    let mut descriptor = provider();
    descriptor.id = "mistral".to_string();
    descriptor.display_name = "Mistral".to_string();
    descriptor.base_url = "https://example.invalid".to_string();
    descriptor.default_api = "mistral-conversations".to_string();
    descriptor.models[0].provider = "mistral".to_string();
    descriptor.models[0].api = "mistral-conversations".to_string();
    let mut registry = ProviderRegistry::new();
    registry.register(descriptor);
    let mut mistral_state = state();
    mistral_state.current_provider = Some("mistral".to_string());
    mistral_state.current_model = Some("mistral/claude-sonnet-4-5".to_string());
    let mut auth = AuthStore::default();
    auth.set_api_key("mistral", "sk-test");
    let error = execute_user_prompt(
        &mistral_state,
        &LoadedResources::default(),
        &registry,
        &auth,
        "hello",
    )
    .unwrap_err();
    assert!(!error
        .to_string()
        .contains("provider mistral with api mistral-conversations is not executable yet"));
}

#[test]
fn execute_user_prompt_allows_no_auth_providers() {
    let mut descriptor = provider();
    descriptor.id = "ollama".to_string();
    descriptor.display_name = "Ollama".to_string();
    descriptor.base_url = "http://127.0.0.1:11434".to_string();
    descriptor.default_api = "openai-completions".to_string();
    descriptor.auth_modes.clear();
    descriptor.models[0].provider = "ollama".to_string();
    descriptor.models[0].api = "openai-completions".to_string();
    let mut registry = ProviderRegistry::new();
    registry.register(descriptor);
    let mut state = state();
    state.current_provider = Some("ollama".to_string());
    state.current_model = Some("ollama/claude-sonnet-4-5".to_string());
    let error = execute_user_prompt(
        &state,
        &LoadedResources::default(),
        &registry,
        &AuthStore::default(),
        "hello",
    )
    .unwrap_err();
    assert!(!error
        .to_string()
        .contains("no credentials configured for provider ollama"));
}

#[test]
fn execute_anthropic_tool_calls_runs_registered_tools() {
    let resources = LoadedResources {
        tools: vec![loaded_tool("bash", "Run shell", "bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let response = json!({
        "content": [
            {
                "type": "tool_use",
                "id": "toolu_1",
                "name": "bash",
                "input": {
                    "command": "printf hi"
                }
            }
        ]
    });
    let result = execute_anthropic_tool_calls(
        &resources,
        &response,
        &registry,
        std::env::current_dir().unwrap().as_path(),
        &anthropic_request_config(),
        "claude-sonnet-4-5",
    )
    .unwrap();
    assert!(result.is_some());
}

#[test]
fn openai_tool_definitions_use_registry_schema() {
    let resources = LoadedResources {
        tools: vec![loaded_tool("search_text", "Search", "search_text")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let provider = openai_provider();
    let tools = openai_tool_definitions(&registry, &provider, "gpt-5");
    assert_eq!(tools[0].name, "search_text");
    assert_eq!(tools[0].kind, "function");
    assert_eq!(tools[0].parameters["type"], "object");
}

#[test]
fn tool_definitions_filter_disabled_tools() {
    let mut denied = loaded_tool("deny_bash", "Denied", "bash");
    denied.value.approval_policy = Some("never".to_string());
    let mut gated = loaded_tool("off_bash", "Off", "bash");
    gated.value.enabled_if = Some("false".to_string());
    let resources = LoadedResources {
        tools: vec![loaded_tool("bash", "Run shell", "bash"), denied, gated],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let openai_provider_descriptor = openai_provider();
    let openai_tools = openai_tool_definitions(&registry, &openai_provider_descriptor, "gpt-5");
    assert_eq!(openai_tools.len(), 1);
    assert_eq!(openai_tools[0].name, "bash");
    let anthropic_provider = provider();
    let anthropic_tools =
        anthropic_tool_definitions(&registry, &anthropic_provider, "claude-sonnet-4-5");
    assert_eq!(anthropic_tools.len(), 1);
    assert_eq!(anthropic_tools[0]["name"], "bash");
}

#[test]
fn execute_openai_tool_calls_serializes_outputs() {
    let resources = LoadedResources {
        tools: vec![loaded_tool("bash", "Run shell", "bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_1".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_1".to_string(),
        name: "bash".to_string(),
        arguments: json!({ "command": "printf hi" }),
    }];
    let result = execute_openai_tool_calls(
        &resources,
        &tool_calls,
        &registry,
        std::env::current_dir().unwrap().as_path(),
        &openai_request_config(),
        "gpt-5",
    )
    .unwrap();
    assert_eq!(result.outputs[0].kind, "function_call_output");
    assert_eq!(result.outputs[0].call_id, "call_1");
    assert!(result.outputs[0].output.contains("hi"));
    assert_eq!(result.invocations[0].tool_id, "bash");
}

#[test]
fn execute_openai_tool_calls_blocks_policy_disabled_tools() {
    let mut denied = loaded_tool("bash", "Run shell", "bash");
    denied.value.approval_policy = Some("never".to_string());
    let resources = LoadedResources {
        tools: vec![denied],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_1".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_1".to_string(),
        name: "bash".to_string(),
        arguments: json!({ "command": "printf hi" }),
    }];
    let result = execute_openai_tool_calls(
        &resources,
        &tool_calls,
        &registry,
        std::env::current_dir().unwrap().as_path(),
        &openai_request_config(),
        "gpt-5",
    )
    .unwrap();
    assert_eq!(result.outputs[0].kind, "function_call_output");
    assert!(result.outputs[0].output.contains("disabled by policy"));
    assert!(!result.outputs[0].output.contains("hi"));
    assert!(!result.invocations[0].success);
}

#[test]
fn execute_anthropic_tool_calls_blocks_policy_disabled_tools() {
    let mut denied = loaded_tool("bash", "Run shell", "bash");
    denied.value.approval_policy = Some("never".to_string());
    let resources = LoadedResources {
        tools: vec![denied],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let response = json!({
        "content": [
            {
                "type": "tool_use",
                "id": "toolu_1",
                "name": "bash",
                "input": {
                    "command": "printf hi"
                }
            }
        ]
    });
    let result = execute_anthropic_tool_calls(
        &resources,
        &response,
        &registry,
        std::env::current_dir().unwrap().as_path(),
        &anthropic_request_config(),
        "claude-sonnet-4-5",
    )
    .unwrap()
    .unwrap();
    let content = result.results.as_array().unwrap();
    assert_eq!(content[0]["is_error"], Value::Bool(true));
    assert!(content[0]["content"]
        .as_str()
        .unwrap_or_default()
        .contains("disabled by policy"));
    assert!(!result.invocations[0].success);
}

#[test]
fn tool_hooks_run_for_completed_tool_calls() {
    let temp = tempfile::tempdir().unwrap();
    let hook_output = temp.path().join("hook.txt");
    let resources = LoadedResources {
        hooks: vec![LoadedItem {
            value: puffer_resources::HookSpec {
                id: "tool-end".to_string(),
                event: "tool_end".to_string(),
                command: format!("printf \"$PUFFER_TOOL_ID\" > {}", hook_output.display()),
            },
            source_info: SourceInfo {
                path: "hook.yaml".into(),
                kind: SourceKind::Builtin,
            },
        }],
        tools: vec![loaded_tool("bash", "Run shell", "bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let response = json!({
        "content": [
            {
                "type": "tool_use",
                "id": "toolu_1",
                "name": "bash",
                "input": {
                    "command": "printf hi"
                }
            }
        ]
    });
    let _ = execute_anthropic_tool_calls(
        &resources,
        &response,
        &registry,
        temp.path(),
        &anthropic_request_config(),
        "claude-sonnet-4-5",
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(hook_output).unwrap(), "bash");
}

#[test]
fn transcript_to_anthropic_messages_replays_all_roles() {
    let mut state = state();
    state.push_message(crate::MessageRole::User, "hello");
    state.push_message(crate::MessageRole::Assistant, "hi");
    state.push_message(crate::MessageRole::System, "note");

    let messages = transcript_to_anthropic_messages(&state, "fallback");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[2]["content"], "[system]\nnote");
}

#[test]
fn transcript_to_openai_input_replays_transcript_items() {
    let mut state = state();
    state.push_message(crate::MessageRole::User, "hello");
    state.push_message(crate::MessageRole::Assistant, "hi");

    let input = transcript_to_openai_input(&state, "fallback");
    let items = input.as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["role"], "user");
    assert_eq!(items[1]["type"], "message");
    assert_eq!(items[1]["role"], "assistant");
}

#[test]
fn transcript_to_openai_chat_messages_replays_transcript_items() {
    let mut state = state();
    state.push_message(crate::MessageRole::User, "hello");
    state.push_message(crate::MessageRole::Assistant, "hi");

    let messages = transcript_to_openai_chat_messages(&state, "fallback");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[0].content, Some(json!("hello")));
}

#[test]
fn transcript_to_openai_chat_messages_preserves_system_role() {
    let mut state = state();
    state.push_message(crate::MessageRole::System, "rules");
    state.push_message(crate::MessageRole::User, "hello");
    state.push_message(crate::MessageRole::Assistant, "hi");

    let messages = transcript_to_openai_chat_messages(&state, "fallback");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].role, "user");
    assert_eq!(messages[2].role, "assistant");
}
