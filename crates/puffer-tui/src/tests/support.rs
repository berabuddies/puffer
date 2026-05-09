use super::*;

pub(super) fn sample_state() -> AppState {
    let mut state = AppState::new(
        PufferConfig::default(),
        PathBuf::from("/workspace/puffer"),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: Some("demo".to_string()),
            generated_title: None,
            cwd: PathBuf::from("/workspace/puffer"),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: Some("demo-session".to_string()),
            tags: vec!["review".to_string()],
            note: Some("Focus on transport parity".to_string()),
        },
    );
    state.statusline_enabled = true;
    state.current_provider = Some("anthropic".to_string());
    state.current_model = Some("anthropic/claude-sonnet-4-5".to_string());
    state.prompt_color = "amber".to_string();
    state.effort_level = "high".to_string();
    state.fast_mode = true;
    state.sandbox_mode = "workspace-write".to_string();
    state.remote_name = Some("buildbox".to_string());
    state.remote_environment = Some("linux".to_string());
    state.push_message(MessageRole::User, "/review");
    state.push_message(
        MessageRole::Assistant,
        "# Review\n- Check command coverage\n- Tighten request parity",
    );
    state
}

pub(super) fn sample_resources() -> LoadedResources {
    LoadedResources {
        tools: vec![
            loaded_item(
                "tools/bash.yaml",
                ToolSpec {
                    id: "bash".to_string(),
                    name: "bash".to_string(),
                    description: "Run shell commands".to_string(),
                    handler: "bash".to_string(),
                    aliases: Vec::new(),
                    handler_args: Vec::new(),
                    approval_policy: Some("on-request".to_string()),
                    sandbox_policy: Some("workspace-write".to_string()),
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: None,
                    metadata: Default::default(),
                    display: Default::default(),
                },
            ),
            loaded_item(
                "tools/read_file.yaml",
                ToolSpec {
                    id: "read_file".to_string(),
                    name: "read_file".to_string(),
                    description: "Read a file".to_string(),
                    handler: "read_file".to_string(),
                    aliases: Vec::new(),
                    handler_args: Vec::new(),
                    approval_policy: Some("never".to_string()),
                    sandbox_policy: Some("read-only".to_string()),
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: None,
                    metadata: Default::default(),
                    display: Default::default(),
                },
            ),
            loaded_item(
                "tools/write_file.yaml",
                ToolSpec {
                    id: "write_file".to_string(),
                    name: "write_file".to_string(),
                    description: "Write a file".to_string(),
                    handler: "write_file".to_string(),
                    aliases: Vec::new(),
                    handler_args: Vec::new(),
                    approval_policy: Some("on-request".to_string()),
                    sandbox_policy: Some("workspace-write".to_string()),
                    shared_lib: None,
                    enabled_if: None,
                    input_schema: None,
                    metadata: Default::default(),
                    display: Default::default(),
                },
            ),
        ],
        prompts: vec![loaded_item(
            "prompts/review.yaml",
            PromptTemplate {
                id: "review".to_string(),
                description: "Review pending changes".to_string(),
                template: "Review $ARGUMENTS".to_string(),
                variables: Vec::new(),
                allowed_tools: Vec::new(),
                provider_override: None,
                model_override: None,
                mode: None,
                chained_from: Vec::new(),
                for_provider: None,
                for_model: None,
            },
        )],
        skills: vec![loaded_item(
            "skills/reviewer.yaml",
            SkillSpec {
                name: "reviewer".to_string(),
                description: "Code review helper".to_string(),
                content: "Review code carefully".to_string(),
                disable_model_invocation: false,
                ..SkillSpec::default()
            },
        )],
        mascots: vec![loaded_item(
            "mascots/clawd.yaml",
            MascotSpec {
                id: "clawd".to_string(),
                display_name: "Clawd".to_string(),
                introduction: "A diligent pufferfish".to_string(),
            },
        )],
        plugins: vec![loaded_item(
            "plugins/git.yaml",
            PluginSpec {
                id: "git".to_string(),
                display_name: "Git".to_string(),
                description: "Git helpers".to_string(),
                commands: vec![PluginCommandSpec {
                    name: "review".to_string(),
                    description: "Review a diff".to_string(),
                }],
                skills: vec!["reviewer".to_string()],
                agents: Vec::new(),
                mcp_servers: vec![McpServerSpec {
                    id: "git-mcp".to_string(),
                    display_name: "Git MCP".to_string(),
                    transport: "stdio".to_string(),
                    endpoint: String::new(),
                    target: "git".to_string(),
                    description: "Git bridge".to_string(),
                    headers: Default::default(),
                    oauth: None,
                }],
                lsp_servers: Vec::new(),
            },
        )],
        mcp_servers: vec![loaded_item(
            "mcp_servers/local.yaml",
            McpServerSpec {
                id: "local".to_string(),
                display_name: "Local MCP".to_string(),
                transport: "stdio".to_string(),
                endpoint: String::new(),
                target: "local".to_string(),
                description: "Local tool bridge".to_string(),
                headers: Default::default(),
                oauth: None,
            },
        )],
        ides: vec![loaded_item(
            "ides/vscode.yaml",
            IdeSpec {
                id: "vscode".to_string(),
                display_name: "VS Code".to_string(),
                description: "VS Code bridge".to_string(),
            },
        )],
        ..LoadedResources::default()
    }
}

pub(super) fn openai_provider_resources() -> LoadedResources {
    LoadedResources {
        providers: vec![loaded_item(
            "providers/openai.yaml",
            ProviderPack {
                id: "openai".to_string(),
                display_name: "OpenAI".to_string(),
                base_url: "https://api.openai.com".to_string(),
                default_api: "openai-responses".to_string(),
                auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
                headers: Default::default(),
                query_params: Default::default(),
                discovery: None,
                models: vec![ModelDescriptor {
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
            },
        )],
        ..LoadedResources::default()
    }
}

pub(super) fn sample_providers() -> ProviderRegistry {
    let mut providers = ProviderRegistry::default();
    providers.register(ProviderDescriptor {
        id: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        default_api: "anthropic-messages".to_string(),
        auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![
            ModelDescriptor {
                id: "claude-sonnet-4-5".to_string(),
                display_name: "Claude Sonnet 4.5".to_string(),
                provider: "anthropic".to_string(),
                api: "anthropic-messages".to_string(),
                context_window: 200_000,
                max_output_tokens: 8_192,
                supports_reasoning: true,
                compat: None,
                input: vec![puffer_provider_registry::Modality::Text],
                cost: None,
            },
            ModelDescriptor {
                id: "claude-opus-4-1".to_string(),
                display_name: "Claude Opus 4.1".to_string(),
                provider: "anthropic".to_string(),
                api: "anthropic-messages".to_string(),
                context_window: 200_000,
                max_output_tokens: 8_192,
                supports_reasoning: true,
                compat: None,
                input: vec![puffer_provider_registry::Modality::Text],
                cost: None,
            },
        ],
        chat_completions_path: None,
    });
    providers.register(ProviderDescriptor {
        id: "openai".to_string(),
        display_name: "OpenAI".to_string(),
        base_url: "https://api.openai.com".to_string(),
        default_api: "responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey, AuthMode::OAuth],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            provider: "openai".to_string(),
            api: "responses".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    });
    providers.register(ProviderDescriptor {
        id: "ollama".to_string(),
        display_name: "Ollama".to_string(),
        base_url: "http://127.0.0.1:11434".to_string(),
        default_api: "openai-completions".to_string(),
        auth_modes: Vec::new(),
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            id: "qwen3:14b".to_string(),
            display_name: "qwen3:14b".to_string(),
            provider: "ollama".to_string(),
            api: "openai-completions".to_string(),
            context_window: 32_768,
            max_output_tokens: 8_192,
            supports_reasoning: false,
            compat: None,
            input: vec![puffer_provider_registry::Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    });
    providers
}

pub(super) fn sample_auth_store() -> AuthStore {
    let mut auth_store = AuthStore::default();
    auth_store.set_oauth(
        "anthropic",
        OAuthCredential {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at_ms: 100,
            account_id: Some("acct".to_string()),
            organization_id: None,
            email: Some("operator@example.com".to_string()),
            plan_type: None,
            rate_limit_tier: None,
            scopes: vec!["org:create_api_key".to_string()],
            organization_name: None,
            organization_role: None,
            workspace_role: None,
        },
    );
    auth_store
}

pub(super) fn loaded_item<T>(path: &str, value: T) -> LoadedItem<T> {
    LoadedItem {
        value,
        source_info: SourceInfo {
            path: PathBuf::from(path),
            kind: SourceKind::Builtin,
        },
    }
}

pub(super) fn buffer_to_string(buffer: &Buffer) -> String {
    let area = buffer.area();
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
