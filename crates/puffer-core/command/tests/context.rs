use super::*;
use puffer_provider_registry::{AuthMode, ModelDescriptor};

fn sample_tool(
    id: &str,
    handler: &str,
    description: &str,
) -> LoadedItem<puffer_resources::ToolSpec> {
    LoadedItem {
        value: puffer_resources::ToolSpec {
            id: id.to_string(),
            name: id.to_string(),
            description: description.to_string(),
            handler: handler.to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            approval_policy: Some("auto".to_string()),
            sandbox_policy: Some("read-only".to_string()),
            shared_lib: None,
            enabled_if: None,
            input_schema: None,
            metadata: Default::default(),
            display: Default::default(),
        },
        source_info: SourceInfo {
            path: PathBuf::from(format!("tools/{handler}.yaml")),
            kind: SourceKind::Builtin,
        },
    }
}

#[test]
fn context_command_renders_anthropic_context_breakdown() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
    state.push_message(MessageRole::User, "Audit the refactor.");
    state.push_message(MessageRole::Assistant, "I inspected the main modules.");
    state.push_message(
        MessageRole::System,
        "Tool Read [ok]\ninput: {\"file_path\":\"/tmp/main.rs\"}",
    );

    let mut resources = LoadedResources::default();
    resources.tools.push(sample_tool(
        "Bash",
        "runtime:claude_bash",
        "Run shell commands",
    ));
    resources.tools.push(sample_tool(
        "Read",
        "runtime:claude_read",
        "Read file contents",
    ));

    let mut providers = ProviderRegistry::new();
    providers.register(ProviderDescriptor {
        id: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        default_api: "anthropic-messages".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            provider: "anthropic".to_string(),
            id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    });

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/context",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("## Context Usage")
            && text.contains("**Model:** anthropic/claude-sonnet-4-5")
            && text.contains("| Conversation |")
            && text.contains("| System prompt |")
            && text.contains("### Conversation")
            && text.contains("| User | 1 |")
            && text.contains("### Tools")
            && text.contains("| Bash |")
            && text.contains("| Read |")
    ));
}

#[test]
fn context_command_renders_openai_context_breakdown() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut state = AppState::new(
        PufferConfig::default(),
        tempdir.path().to_path_buf(),
        session,
    );
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());
    state.push_message(MessageRole::User, "Check the migration.");
    state.push_message(MessageRole::Assistant, "I found one edge case.");

    let mut resources = LoadedResources::default();
    resources.tools.push(sample_tool(
        "Read",
        "runtime:claude_read",
        "Read file contents",
    ));

    let mut providers = ProviderRegistry::new();
    providers.register(ProviderDescriptor {
        id: "openai".to_string(),
        display_name: "OpenAI".to_string(),
        base_url: "https://api.openai.com".to_string(),
        default_api: "openai-responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            provider: "openai".to_string(),
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            api: "openai-responses".to_string(),
            context_window: 272_000,
            max_output_tokens: 16_384,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    });

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut providers,
        &mut AuthStore::default(),
        &session_store,
        "/context",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("**Model:** openai/gpt-5")
            && text.contains("272,000")
            && text.contains("| Tools |")
            && text.contains("| Read |")
    ));
}
