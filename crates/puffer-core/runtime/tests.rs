use super::*;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_provider_openai::{
    OpenAIAuth, OpenAIRequestConfig, OpenAIResponseToolCall, OpenAIResponsesTextConfig,
    OpenAIResponsesTextFormat, OpenAIResponsesTool,
};
use puffer_provider_registry::{AuthMode, OAuthCredential, ProviderDescriptor, StoredCredential};
use puffer_resources::{
    load_resources, AgentSpec, LoadedItem, LoadedResources, SourceInfo, SourceKind, ToolSpec,
};
use puffer_session_store::SessionMetadata;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

fn provider() -> ProviderDescriptor {
    ProviderDescriptor {
        id: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        default_api: "anthropic-messages".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        media: None,
        models: vec![puffer_provider_registry::ModelDescriptor {
            id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            provider: "anthropic".to_string(),
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8192,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    }
}

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn simple_turn_tool_suppression_is_env_gated() {
    let _guard = env_lock();
    std::env::remove_var(SUPPRESS_TOOLS_FOR_SIMPLE_TURNS_ENV);
    assert!(!suppress_tools_for_simple_turn("hi"));

    std::env::set_var(SUPPRESS_TOOLS_FOR_SIMPLE_TURNS_ENV, "1");
    assert!(suppress_tools_for_simple_turn("hi"));
    assert!(suppress_tools_for_simple_turn(" Hello "));
    assert!(!suppress_tools_for_simple_turn("please read a file"));
    std::env::remove_var(SUPPRESS_TOOLS_FOR_SIMPLE_TURNS_ENV);
}

#[test]
fn local_simple_turn_reply_requires_both_env_flags() {
    let _guard = env_lock();
    std::env::remove_var(SUPPRESS_TOOLS_FOR_SIMPLE_TURNS_ENV);
    std::env::remove_var(LOCAL_REPLY_FOR_SIMPLE_TURNS_ENV);
    assert!(!suppress_tools_for_simple_turn("hi"));
    assert!(!local_reply_for_simple_turns_enabled());

    std::env::set_var(SUPPRESS_TOOLS_FOR_SIMPLE_TURNS_ENV, "1");
    assert!(suppress_tools_for_simple_turn("hi"));
    assert!(!local_reply_for_simple_turns_enabled());

    std::env::set_var(LOCAL_REPLY_FOR_SIMPLE_TURNS_ENV, "1");
    assert!(local_reply_for_simple_turns_enabled());
    assert_eq!(local_simple_turn_reply("hi"), "Hi.");
    assert_eq!(local_simple_turn_reply(" hey "), "Hey.");

    std::env::remove_var(SUPPRESS_TOOLS_FOR_SIMPLE_TURNS_ENV);
    std::env::remove_var(LOCAL_REPLY_FOR_SIMPLE_TURNS_ENV);
}

pub(super) fn state() -> AppState {
    AppState::new(
        PufferConfig::default(),
        std::env::current_dir().unwrap(),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            generated_title: None,
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

pub(super) fn plan_mode_state() -> AppState {
    let cwd = std::env::temp_dir().join(format!("puffer-runtime-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&cwd).unwrap();
    let paths = ConfigPaths::discover(&cwd);
    ensure_workspace_dirs(&paths).unwrap();
    let session = SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        generated_title: None,
        cwd: cwd.clone(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    let mut state = AppState::new(PufferConfig::default(), cwd.clone(), session.clone());
    state.plan_mode = true;
    let plan_dir = paths.workspace_config_dir.join("plans");
    std::fs::create_dir_all(&plan_dir).unwrap();
    std::fs::write(
        plan_dir.join(format!("{}.md", session.id)),
        "# Current Plan\n\n1. Inspect the slash-command flow.\n",
    )
    .unwrap();
    state
}

fn loaded_tool(id: &str, description: &str, handler: &str) -> LoadedItem<ToolSpec> {
    LoadedItem {
        value: ToolSpec {
            id: id.to_string(),
            name: id.to_string(),
            description: description.to_string(),
            handler: handler.to_string(),
            aliases: Vec::new(),
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

/// Scripted HTTP server for runtime loop tests.
fn spawn_server<F>(
    content_type: &'static str,
    expected_requests: usize,
    response_body: F,
) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>)
where
    F: Fn(usize) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut handled = 0_usize;
        while handled < expected_requests && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    let mut buffer = vec![0_u8; 65_536];
                    let bytes = stream.read(&mut buffer).unwrap();
                    let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
                    request_log.lock().unwrap().push(request);
                    let body = response_body(handled);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                    handled += 1;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("listener accept failed: {error}"),
            }
        }
    });
    (format!("http://{address}"), requests, server)
}

fn loaded_agent(
    id: &str,
    description: &str,
    prompt: &str,
    tools: &[&str],
) -> LoadedItem<AgentSpec> {
    LoadedItem {
        value: AgentSpec {
            id: id.to_string(),
            description: description.to_string(),
            prompt: prompt.to_string(),
            tools: tools.iter().map(|tool| tool.to_string()).collect(),
            model: None,
            ..AgentSpec::default()
        },
        source_info: SourceInfo {
            path: format!("{id}.yaml").into(),
            kind: SourceKind::Builtin,
        },
    }
}

fn openai_provider(base_url: String) -> ProviderDescriptor {
    ProviderDescriptor {
        id: "openai".to_string(),
        display_name: "OpenAI".to_string(),
        base_url,
        default_api: "openai-responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        media: None,
        models: vec![puffer_provider_registry::ModelDescriptor {
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            provider: "openai".to_string(),
            api: "openai-responses".to_string(),
            context_window: 272_000,
            max_output_tokens: 16_384,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    }
}

fn test_anthropic_request_config() -> AnthropicRequestConfig {
    AnthropicRequestConfig {
        base_url: "https://api.anthropic.com".to_string(),
        session_id: "session-test".to_string(),
        custom_headers: Default::default(),
        remote_container_id: None,
        remote_session_id: None,
        client_app: None,
        entrypoint: "cli".to_string(),
        user_type: "external".to_string(),
        version: APP_VERSION.to_string(),
        workload: None,
        additional_protection: false,
        cch_enabled: true,
        auth: AnthropicAuth::ApiKey("sk-ant".to_string()),
        beta_header: None,
        client_request_id: None,
    }
}

fn test_openai_request_config() -> OpenAIRequestConfig {
    OpenAIRequestConfig {
        base_url: "https://api.openai.com".to_string(),
        version: APP_VERSION.to_string(),
        auth: OpenAIAuth::ApiKey("sk-openai".to_string()),
        originator: "codex_cli_rs".to_string(),
        session_id: Some("session-test".to_string()),
        account_id: None,
        custom_headers: Vec::new(),
        query_params: Vec::new(),
        chat_completions_path: None,
        responses_path: None,
    }
}

fn fake_jwt(payload: Value) -> String {
    format!("header.{}.sig", URL_SAFE_NO_PAD.encode(payload.to_string()))
}

pub(super) fn refresh_env_lock() -> &'static Mutex<()> {
    crate::test_locks::env_lock()
}

fn bundled_resources() -> LoadedResources {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf();
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths {
        workspace_root: temp.path().join("workspace"),
        workspace_config_dir: temp.path().join("workspace/.puffer"),
        user_config_dir: temp.path().join("user"),
        builtin_resources_dir: root.join("resources"),
    };
    ensure_workspace_dirs(&paths).unwrap();
    let runner = crate::runner_adapter::LocalToolRunner::new();
    load_resources(&paths, &runner).unwrap()
}

#[test]
fn anthropic_tool_schema_lists_expected_fields() {
    let schema = anthropic_tool_schema("write_file");
    let required = schema.get("required").and_then(Value::as_array).unwrap();
    assert!(required.iter().any(|value| value == "path"));
    assert!(required.iter().any(|value| value == "contents"));
}

#[test]
fn runtime_system_prompt_mentions_ignore_memory_policy() {
    let prompt = super::system_prompt::render_runtime_system_prompt(
        &state(),
        &bundled_resources(),
        "gpt-5",
        &std::collections::BTreeSet::new(),
    )
    .unwrap();

    assert!(prompt.contains("ignore or not use memory"));
    assert!(prompt.contains("loaded memory files were empty"));
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
fn resolve_selection_uses_provider_scoped_default_model_id() {
    let mut registry = ProviderRegistry::new();
    let mut descriptor = provider();
    descriptor.id = "openai".to_string();
    descriptor.display_name = "OpenAI".to_string();
    descriptor.default_api = "openai-responses".to_string();
    descriptor.models = vec![
        puffer_provider_registry::ModelDescriptor {
            id: "gpt-5.1".to_string(),
            display_name: "gpt-5.1".to_string(),
            provider: "openai".to_string(),
            api: "openai-responses".to_string(),
            context_window: 272_000,
            max_output_tokens: 16_384,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        },
        puffer_provider_registry::ModelDescriptor {
            id: "gpt-5.4".to_string(),
            display_name: "gpt-5.4".to_string(),
            provider: "openai".to_string(),
            api: "openai-responses".to_string(),
            context_window: 272_000,
            max_output_tokens: 16_384,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        },
    ];
    registry.register(descriptor);
    let mut state = state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("gpt-5.4".to_string());
    let (provider, model_id) = resolve_provider_and_model(&state, &registry).unwrap();
    assert_eq!(provider.id, "openai");
    assert_eq!(model_id, "gpt-5.4");
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
        &mut azure_state,
        &LoadedResources::default(),
        &registry,
        &mut auth,
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
        &mut openrouter_state,
        &LoadedResources::default(),
        &registry,
        &mut auth,
        "hello",
    )
    .unwrap_err();
    assert!(!error
        .to_string()
        .contains("provider openrouter with api openai-completions is not executable yet"));
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
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut AuthStore::default(),
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
    let mut providers = ProviderRegistry::new();
    let provider = provider();
    providers.register(provider.clone());
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
    let mut state = state();
    let request_config = test_anthropic_request_config();
    let result = execute_anthropic_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &response,
        &registry,
        std::env::current_dir().unwrap().as_path(),
        &request_config,
        "claude-sonnet-4-5",
        None,
        None,
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
    let tools = openai_tool_definitions(&registry, None, false).unwrap();
    assert_eq!(tools[0].name, "search_text");
    assert_eq!(tools[0].kind, "function");
    assert_eq!(tools[0].parameters["type"], "object");
}

#[test]
fn openai_tool_definitions_fill_missing_array_items() {
    // The production WebSearch resource uses the `provider:web_search`
    // handler, which now serializes as a native server-side tool with no
    // function-shaped parameters. Force the function-form path so the
    // schema-normalization assertions still apply.
    let _guard = crate::test_locks::env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prev = std::env::var_os("PUFFER_OPENAI_NATIVE_WEB_SEARCH");
    std::env::set_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH", "0");
    let mut web_search = loaded_tool("WebSearch", "Search the web", "provider:web_search");
    web_search.value.input_schema = Some(json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query to execute."
            },
            "allowed_domains": {
                "type": "array",
                "description": "Optional allowlist of domains to search."
            },
            "blocked_domains": {
                "type": "array",
                "description": "Optional denylist of domains to exclude."
            }
        },
        "required": ["query"]
    }));
    let resources = LoadedResources {
        tools: vec![web_search],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let tools = openai_tool_definitions(&registry, None, false).unwrap();
    match prev {
        Some(value) => std::env::set_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH", value),
        None => std::env::remove_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH"),
    }
    assert_eq!(tools[0].name, "WebSearch");
    assert_eq!(
        tools[0].parameters["properties"]["allowed_domains"]["items"]["type"].as_str(),
        Some("string")
    );
    assert_eq!(
        tools[0].parameters["properties"]["blocked_domains"]["items"]["type"].as_str(),
        Some("string")
    );
}

#[test]
fn openai_web_search_description_mentions_current_month_year() {
    // Description rendering only applies on the function-form fallback; the
    // native server-side tool has no description.
    let _guard = crate::test_locks::env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prev = std::env::var_os("PUFFER_OPENAI_NATIVE_WEB_SEARCH");
    std::env::set_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH", "0");
    let description = "\
- Allows Claude to search the web and use the results to inform responses

IMPORTANT - Use the correct year in search queries:
  - You MUST use the current year when searching for recent information, documentation, or current events.";
    let resources = LoadedResources {
        tools: vec![loaded_tool("WebSearch", description, "provider:web_search")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let tools = openai_tool_definitions(&registry, None, false).unwrap();
    match prev {
        Some(value) => std::env::set_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH", value),
        None => std::env::remove_var("PUFFER_OPENAI_NATIVE_WEB_SEARCH"),
    }
    assert!(tools[0].description.contains("The current month is"));
    assert!(!tools[0]
        .description
        .contains("You MUST use the current year when searching"));
}

#[test]
fn anthropic_web_search_description_mentions_current_month_year() {
    let description = "\
- Allows Claude to search the web and use the results to inform responses

IMPORTANT - Use the correct year in search queries:
  - You MUST use the current year when searching for recent information, documentation, or current events.";
    let resources = LoadedResources {
        tools: vec![loaded_tool("WebSearch", description, "provider:web_search")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let tools = anthropic_tool_definitions(&registry, None).unwrap();
    assert!(tools[0]["description"]
        .as_str()
        .unwrap_or_default()
        .contains("The current month is"));
}

#[test]
fn tool_definitions_filter_disabled_tools() {
    let mut denied = loaded_tool("deny_bash", "Denied", "bash");
    denied.value.approval_policy = Some("disabled".to_string());
    let mut gated = loaded_tool("off_bash", "Off", "bash");
    gated.value.enabled_if = Some("false".to_string());
    let resources = LoadedResources {
        tools: vec![denied, gated, loaded_tool("bash", "Run shell", "bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let openai_tools = openai_tool_definitions(&registry, None, false).unwrap();
    let anthropic_tools = anthropic_tool_definitions(&registry, None).unwrap();
    assert_eq!(openai_tools.len(), 1);
    assert_eq!(anthropic_tools.len(), 1);
    assert_eq!(openai_tools[0].name, "bash");
    assert_eq!(anthropic_tools[0]["name"], "bash");
}

#[test]
fn openai_tool_definitions_exclude_structured_output_workflow_helper() {
    let mut structured_output = loaded_tool(
        "StructuredOutput",
        "Structured output helper",
        "runtime:workflow:structured_output",
    );
    structured_output.value.input_schema = Some(json!({
        "type": "object",
        "description": "Dynamic structured output payload.",
        "additionalProperties": true
    }));
    let resources = LoadedResources {
        tools: vec![structured_output, loaded_tool("bash", "Run shell", "bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);

    let openai_tools = openai_tool_definitions(&registry, None, false).unwrap();

    assert!(!openai_tools
        .iter()
        .any(|tool| tool.name == "StructuredOutput"));
    assert!(openai_tools.iter().any(|tool| tool.name == "bash"));
}

#[test]
fn resolve_openai_execution_config_uses_codex_chatgpt_route_for_builtin_oauth() {
    let mut auth_store = AuthStore::default();
    auth_store.set_oauth(
        "openai",
        OAuthCredential {
            access_token: fake_jwt(json!({
                "https://api.openai.com/auth": {
                    "chatgpt_account_id": "acct-123"
                }
            })),
            refresh_token: "refresh-123".to_string(),
            expires_at_ms: 42,
            account_id: Some("acct-123".to_string()),
            organization_id: None,
            email: Some("dev@example.com".to_string()),
            plan_type: Some("pro".to_string()),
            rate_limit_tier: None,
            scopes: vec!["openid".to_string()],
            organization_name: None,
            organization_role: None,
            workspace_role: None,
        },
    );

    let config = resolve_openai_execution_config(
        &state(),
        &auth_store,
        &openai_provider("https://api.openai.com".to_string()),
    )
    .unwrap();

    assert_eq!(config.request_config.base_url, OPENAI_CHATGPT_BASE_URL);
    assert_eq!(config.request_config.version, OPENAI_CODEX_COMPAT_VERSION);
    assert_eq!(
        config.request_config.account_id.as_deref(),
        Some("acct-123")
    );
    assert!(config.codex_style);
    assert!(config
        .request_config
        .custom_headers
        .iter()
        .any(|(key, _)| key == "version"));
}

#[test]
fn build_codex_openai_request_body_matches_codex_shape() {
    let state = state();
    let body = build_codex_openai_request_body(
        &state,
        OPENAI_CHATGPT_BASE_URL,
        "gpt-5",
        "system instructions",
        Value::String("hello".to_string()),
        &Vec::new(),
        true,
        None,
        true,
    );

    assert_eq!(body["model"], json!("gpt-5"));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["include"], json!([]));
    assert_eq!(body["prompt_cache_key"], json!(Uuid::nil().to_string()));
    assert_eq!(body["instructions"], json!("system instructions"));
    assert_eq!(body["input"][0]["type"], json!("message"));
    assert_eq!(body["input"][0]["content"][0]["text"], json!("hello"));
    assert_eq!(body["reasoning"]["summary"], json!("auto"));
    assert_eq!(body["reasoning"]["effort"], json!("medium"));
    assert!(body.get("parallel_tool_calls").is_none());
    assert!(body.get("previous_response_id").is_none());
    assert!(body.get("service_tier").is_none());
}

#[test]
fn build_codex_openai_request_body_supports_xhigh_effort() {
    let mut state = state();
    state.effort_level = "xhigh".to_string();
    let body = build_codex_openai_request_body(
        &state,
        OPENAI_CHATGPT_BASE_URL,
        "gpt-5",
        "",
        Value::String("hello".to_string()),
        &Vec::new(),
        true,
        None,
        true,
    );

    assert_eq!(body["reasoning"]["effort"], json!("xhigh"));
}

#[test]
fn build_codex_openai_request_body_uses_priority_tier_for_fast_mode() {
    let mut state = state();
    state.fast_mode = true;
    let tools = vec![OpenAIResponsesTool {
        kind: "function".to_string(),
        name: "read_file".to_string(),
        description: "Reads a file.".to_string(),
        strict: false,
        parameters: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            }
        }),
        filters: None,
        user_location: None,
        external_web_access: None,
    }];
    let body = build_codex_openai_request_body(
        &state,
        OPENAI_CHATGPT_BASE_URL,
        "gpt-5",
        "",
        Value::String("hello".to_string()),
        &tools,
        true,
        None,
        true,
    );

    assert_eq!(body["service_tier"], json!("priority"));
    assert_eq!(body["parallel_tool_calls"], json!(true));
    // Fast mode no longer disables reasoning — it only sets service_tier.
    // Reasoning is controlled solely by effort_level, matching Anthropic behavior.
    assert_eq!(body["reasoning"]["effort"], json!("medium"));
    assert_eq!(body["include"], json!([]));
}

#[test]
fn build_codex_openai_request_body_includes_native_structured_output_config() {
    let state = state();
    let body = build_codex_openai_request_body(
        &state,
        OPENAI_CHATGPT_BASE_URL,
        "gpt-5",
        "",
        Value::String("hello".to_string()),
        &Vec::new(),
        true,
        Some(OpenAIResponsesTextConfig {
            format: Some(OpenAIResponsesTextFormat {
                kind: "json_schema".to_string(),
                name: "answer_shape".to_string(),
                description: None,
                schema: json!({
                    "type": "object",
                    "properties": {
                        "answer": { "type": "string" }
                    }
                }),
                strict: true,
            }),
            verbosity: None,
        }),
        false,
    );

    assert_eq!(body["text"]["format"]["type"], json!("json_schema"));
    assert_eq!(body["text"]["format"]["name"], json!("answer_shape"));
    assert_eq!(body["stream"], json!(false));
}

#[test]
fn openai_structured_output_falls_back_and_caches_unsupported_native_support() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);

    let server = thread::spawn(move || {
        for index in 0..5 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 131072];
            let bytes = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
            request_log.lock().unwrap().push(request);

            let body = match index {
                0 => json!({
                    "error": {
                        "message": "response_format json_schema is not supported for this model"
                    }
                })
                .to_string(),
                1 => json!({
                    "id": "resp_1",
                    "output": [{
                        "type": "function_call",
                        "call_id": "call_1",
                        "name": "StructuredOutput",
                        "arguments": { "answer": "ok" }
                    }]
                })
                .to_string(),
                2 => json!({
                    "id": "resp_2",
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": "{\"answer\":\"ok\"}"
                        }]
                    }]
                })
                .to_string(),
                3 => json!({
                    "id": "resp_3",
                    "output": [{
                        "type": "function_call",
                        "call_id": "call_2",
                        "name": "StructuredOutput",
                        "arguments": { "answer": "cached" }
                    }]
                })
                .to_string(),
                _ => json!({
                    "id": "resp_4",
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": "{\"answer\":\"cached\"}"
                        }]
                    }]
                })
                .to_string(),
            };
            let status = if index == 0 { 400 } else { 200 };
            let response = format!(
                "HTTP/1.1 {status} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                if status == 200 { "OK" } else { "Bad Request" },
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(format!("http://{address}")));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");
    let mut state = state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    let structured_output = StructuredOutputConfig::new(
        "answer_shape",
        json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"],
            "additionalProperties": false
        }),
    );

    let first = execute_user_prompt_with_structured_output(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "hello",
        &structured_output,
    )
    .unwrap();
    let second = execute_user_prompt_with_structured_output(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "again",
        &structured_output,
    )
    .unwrap();
    server.join().unwrap();

    assert_eq!(first.assistant_text, "{\"answer\":\"ok\"}");
    assert_eq!(second.assistant_text, "{\"answer\":\"cached\"}");
    assert!(state.is_native_structured_output_unsupported(
        "openai",
        "openai",
        "gpt-5",
        registry.provider("openai").unwrap().base_url.as_str()
    ));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 5);

    let first_body = request_json_body(&requests[0]);
    assert_eq!(first_body["text"]["format"]["type"], json!("json_schema"));
    assert!(first_body["tools"]
        .as_array()
        .is_none_or(|tools| tools.iter().all(|tool| tool["name"] != "StructuredOutput")));

    let fallback_body = request_json_body(&requests[1]);
    assert!(fallback_body.get("text").is_none());
    assert!(fallback_body["tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "StructuredOutput")));

    let cached_body = request_json_body(&requests[3]);
    assert!(cached_body.get("text").is_none());
    assert!(cached_body["tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "StructuredOutput")));
}

#[test]
fn native_structured_output_unsupported_cache_is_endpoint_scoped() {
    let mut state = state();
    state.mark_native_structured_output_unsupported(
        "openai",
        "openai",
        "gpt-5",
        "https://first.example/v1",
    );

    assert!(state.is_native_structured_output_unsupported(
        "openai",
        "openai",
        "gpt-5",
        "https://first.example/v1",
    ));
    assert!(!state.is_native_structured_output_unsupported(
        "openai",
        "openai",
        "gpt-5",
        "https://second.example/v1",
    ));
}

#[test]
fn parse_openai_sse_response_reconstructs_output_items() {
    let stream = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\"}}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_123\",\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"Cargo.toml\\\"}\"}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\"}}\n\n"
    );

    let parsed = parse_openai_sse_response(stream).unwrap();
    assert_eq!(parsed["id"], json!("resp_123"));
    assert_eq!(parsed["output"].as_array().map(Vec::len), Some(2));
    assert_eq!(parsed["output"][0]["type"], json!("message"));
    assert_eq!(parsed["output"][1]["type"], json!("function_call"));
}

#[test]
fn parse_openai_sse_response_streaming_emits_text_deltas() {
    let stream = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp1\"}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hey \"}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"there\"}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hey there\"}]}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp1\"}}\n\n"
    );
    let mut deltas = Vec::new();
    let parsed = parse_openai_sse_response_streaming(stream, &mut |event| {
        if let TurnStreamEvent::TextDelta(delta) = event {
            deltas.push(delta);
        }
    })
    .unwrap();
    assert_eq!(deltas, vec!["Hey ".to_string(), "there".to_string()]);
    assert_eq!(parsed["id"], json!("resp1"));
    assert_eq!(
        parsed["output"][0]["content"][0]["text"],
        json!("Hey there")
    );
}

#[test]
fn execute_user_prompt_refreshes_openai_oauth_after_401() {
    let _guard = refresh_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);

    let initial_access_token = fake_jwt(json!({
        "https://api.openai.com/auth": {
            "chatgpt_account_id": "acct-123"
        }
    }));
    let refreshed_access_token = fake_jwt(json!({
        "https://api.openai.com/auth": {
            "chatgpt_account_id": "acct-123"
        }
    }));
    let refreshed_id_token = fake_jwt(json!({
        "email": "dev@example.com",
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": "pro"
        }
    }));
    let refreshed_access_token_for_server = refreshed_access_token.clone();
    let refreshed_id_token_for_server = refreshed_id_token.clone();
    let refresh_url = format!("http://{address}/oauth/token");
    std::env::set_var("CODEX_REFRESH_TOKEN_URL_OVERRIDE", &refresh_url);

    let server = thread::spawn(move || {
        for index in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let bytes = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
            request_log.lock().unwrap().push(request);

            let (status, content_type, body) = match index {
                0 => (
                    401,
                    "application/json",
                    json!({ "error": "unauthorized" }).to_string(),
                ),
                1 => (
                    200,
                    "application/json",
                    json!({
                        "access_token": refreshed_access_token_for_server,
                        "refresh_token": "refresh-2",
                        "expires_in": 3600,
                        "id_token": refreshed_id_token_for_server,
                    })
                    .to_string(),
                ),
                _ => (
                    200,
                    "text/event-stream",
                    concat!(
                        "event: response.created\n",
                        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
                        "event: response.output_item.done\n",
                        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"refreshed ok\"}]}}\n\n",
                        "event: response.completed\n",
                        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\"}}\n\n"
                    )
                    .to_string(),
                ),
            };
            let response = format!(
                "HTTP/1.1 {status} {}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                if status == 200 { "OK" } else { "Unauthorized" },
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });

    let mut registry = ProviderRegistry::new();
    registry.register(openai_provider(format!("http://{address}/api/codex")));
    let mut auth_store = AuthStore::default();
    auth_store.set_oauth(
        "openai",
        OAuthCredential {
            access_token: initial_access_token.clone(),
            refresh_token: "refresh-1".to_string(),
            expires_at_ms: 0,
            account_id: Some("acct-123".to_string()),
            organization_id: None,
            email: None,
            plan_type: None,
            rate_limit_tier: None,
            scopes: vec!["openid".to_string()],
            organization_name: None,
            organization_role: None,
            workspace_role: None,
        },
    );
    let mut state = state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let turn = execute_user_prompt(
        &mut state,
        &LoadedResources::default(),
        &registry,
        &mut auth_store,
        "hello",
    )
    .unwrap();
    std::env::remove_var("CODEX_REFRESH_TOKEN_URL_OVERRIDE");
    server.join().unwrap();

    assert_eq!(turn.assistant_text, "refreshed ok");
    let stored = match auth_store.get("openai") {
        Some(StoredCredential::OAuth(credential)) => credential,
        other => panic!("expected oauth credential, got {other:?}"),
    };
    assert_eq!(stored.access_token, refreshed_access_token);
    assert_eq!(stored.refresh_token, "refresh-2");
    assert_eq!(stored.email.as_deref(), Some("dev@example.com"));
    assert_eq!(stored.plan_type.as_deref(), Some("pro"));

    let requests = requests.lock().unwrap();
    let first = requests[0].to_ascii_lowercase();
    let second = requests[1].to_ascii_lowercase();
    let third = requests[2].to_ascii_lowercase();
    assert!(first.contains("post /api/codex/responses http/1.1"));
    assert!(first.contains(&format!(
        "authorization: bearer {}",
        initial_access_token.to_ascii_lowercase()
    )));
    assert!(first.contains("originator: codex_cli_rs"));
    assert!(second.contains("post /oauth/token http/1.1"));
    assert!(third.contains(&format!(
        "authorization: bearer {}",
        refreshed_access_token.to_ascii_lowercase()
    )));
}

#[test]
fn tool_definitions_keep_never_approval_tools_enabled() {
    let mut always_allowed = loaded_tool("read_file", "Read", "read_file");
    always_allowed.value.approval_policy = Some("never".to_string());
    let resources = LoadedResources {
        tools: vec![always_allowed],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let openai_tools = openai_tool_definitions(&registry, None, false).unwrap();
    assert_eq!(openai_tools.len(), 1);
    assert_eq!(openai_tools[0].name, "read_file");
}

#[path = "tests/agent_loop_e2e.rs"]
mod agent_loop_e2e;
#[path = "tests/anthropic_sse_error_event.rs"]
mod anthropic_sse_error_event;
#[path = "tests/http_retries.rs"]
mod http_retries;
#[path = "tests/iteration_behavior.rs"]
mod iteration_behavior;
#[path = "tests/lambda_skill_agent_loop.rs"]
mod lambda_skill_agent_loop;
#[path = "tests/openai_stream_transport.rs"]
mod openai_stream_transport;
#[path = "tests/openai_tool_errors.rs"]
mod openai_tool_errors;
#[path = "tests/permissions.rs"]
mod permissions;
#[path = "tests/skill_action_obligation_e2e.rs"]
mod skill_action_obligation_e2e;
#[path = "tests/tool_execution.rs"]
mod tool_execution;
#[path = "tests/tool_visibility.rs"]
mod tool_visibility;

fn request_json_body(raw_request: &str) -> Value {
    let body = raw_request.split("\r\n\r\n").nth(1).unwrap_or_default();
    serde_json::from_str(body).unwrap()
}

mod persisted_output_preview {
    //! Tests for `build_persisted_output_message` — the formatter
    //! that produces the `<persisted-output>` message the model sees
    //! when a tool result is too large to inline. As of 2026-05-08
    //! long outputs render as head + tail (so terminal failure
    //! messages survive); short / borderline outputs keep CC-parity
    //! head-only wording.

    use crate::runtime::{build_persisted_output_message, PREVIEW_SIZE_CHARS};

    #[test]
    fn long_output_shows_head_and_tail_blocks() {
        let head_payload = "A".repeat(8_000);
        let tail_payload = "Z".repeat(8_000);
        let text = format!("{head_payload}\n{tail_payload}");

        let message = build_persisted_output_message("/tmp/persist.txt", &text);

        assert!(
            message.contains("head + last"),
            "head+tail format should advertise both blocks; got: {message}"
        );
        let marker_idx = message.find("truncated").expect("truncated marker present");
        let (before_marker, after_marker) = message.split_at(marker_idx);
        assert!(
            before_marker.contains('A'),
            "head should contain A's; before marker: {before_marker}"
        );
        assert!(
            !before_marker.contains('Z'),
            "head must not contain tail content; before marker has Z"
        );
        assert!(
            after_marker.contains('Z'),
            "tail should contain Z's; after marker: {after_marker}"
        );
    }

    #[test]
    fn borderline_output_keeps_head_only_wording_for_cc_parity() {
        // 1.5x preview budget: tail too short to be useful, so keep
        // CC-parity head-only wording.
        let text = "x".repeat(PREVIEW_SIZE_CHARS + PREVIEW_SIZE_CHARS / 2);

        let message = build_persisted_output_message("/tmp/persist.txt", &text);

        assert!(
            message.contains("Preview (first"),
            "borderline output should use head-only wording; got: {message}"
        );
        assert!(
            !message.contains("head + last"),
            "borderline output should NOT use head+tail wording"
        );
        assert!(
            !message.contains("chars truncated"),
            "borderline output should NOT show a truncated-chars marker"
        );
    }

    #[test]
    fn long_output_truncated_count_is_within_slack_of_expected() {
        let head = "h".repeat(2_000);
        let mid = "m".repeat(10_000);
        let tail = "t".repeat(2_000);
        let text = format!("{head}{mid}{tail}");
        let total = text.chars().count();

        let message = build_persisted_output_message("/tmp/persist.txt", &text);

        let marker = message
            .lines()
            .find(|line| line.contains("chars truncated"))
            .expect("truncated marker line");
        let n: usize = marker
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .expect("truncated count parses");
        let expected = total - PREVIEW_SIZE_CHARS;
        let slack = 8;
        assert!(
            n + slack >= expected && n <= expected + slack,
            "truncated count {n} should be within ±{slack} of {expected}"
        );
    }
}
