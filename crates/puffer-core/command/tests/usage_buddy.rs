use super::*;
use puffer_provider_registry::OAuthCredential;

#[test]
fn usage_command_reports_runtime_and_resource_counts() {
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
    let mut providers = ProviderRegistry::new();
    providers.register(ProviderDescriptor {
        id: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        default_api: "anthropic-messages".to_string(),
        auth_modes: Vec::new(),
        headers: Default::default(),
        query_params: Default::default(),
        discovery: Some(puffer_provider_registry::ModelDiscoveryConfig {
            path: "/v1/models".to_string(),
            response: puffer_provider_registry::ModelDiscoveryFormat::AnthropicModels,
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            items_field: "data".to_string(),
            id_field: "id".to_string(),
            display_name_field: Some("display_name".to_string()),
            headers: Default::default(),
        }),
        models: vec![puffer_provider_registry::ModelDescriptor {
            id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            provider: "anthropic".to_string(),
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
        }],
    });
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("anthropic", "sk-ant");
    let resources = LoadedResources {
        prompts: vec![LoadedItem {
            value: puffer_resources::PromptTemplate {
                id: "review".to_string(),
                description: "review".to_string(),
                template: "review".to_string(),
                variables: Vec::new(),
                allowed_tools: Vec::new(),
                provider_override: None,
                model_override: None,
                mode: None,
                chained_from: Vec::new(),
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("prompts/review.yaml"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        tools: vec![LoadedItem {
            value: puffer_resources::ToolSpec {
                id: "bash".to_string(),
                name: "bash".to_string(),
                description: "Run bash".to_string(),
                handler: "bash".to_string(),
                aliases: Vec::new(),
                handler_args: Vec::new(),
                approval_policy: Some("ask".to_string()),
                sandbox_policy: Some("workspace-write".to_string()),
                shared_lib: None,
                enabled_if: None,
                input_schema: None,
                metadata: Default::default(),
                display: Default::default(),
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("tools/bash.yaml"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        hooks: vec![LoadedItem {
            value: puffer_resources::HookSpec {
                id: "tool-end".to_string(),
                event: "tool_end".to_string(),
                command: "echo done".to_string(),
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("hooks/tool_end.yaml"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        skills: vec![LoadedItem {
            value: puffer_resources::SkillSpec {
                name: "reviewer".to_string(),
                description: "review".to_string(),
                content: "review".to_string(),
                disable_model_invocation: false,
                ..puffer_resources::SkillSpec::default()
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("skills/reviewer/SKILL.md"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        plugins: vec![LoadedItem {
            value: puffer_resources::PluginSpec {
                id: "core".to_string(),
                display_name: "Core".to_string(),
                description: "core".to_string(),
                commands: Vec::new(),
                skills: Vec::new(),
                agents: Vec::new(),
                mcp_servers: Vec::new(),
                lsp_servers: Vec::new(),
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("plugins/core.yaml"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        ..LoadedResources::default()
    };

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut providers,
        &mut auth_store,
        &session_store,
        "/usage",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Usage")
            && text.contains("Provider: Anthropic")
            && text.contains("Authentication: API key")
            && text.contains("Claude subscription usage")
            && text.contains("Sign in with Anthropic OAuth")
            && text.contains("Runtime summary")
            && text.contains("Authenticated providers: 1")
            && text.contains("Providers with discovery: 1")
            && text.contains("Prompts: 1")
            && text.contains("Tools: 1")
            && text.contains("Hooks: 1")
    ));
}

#[test]
fn usage_command_prefers_claude_style_anthropic_oauth_sections() {
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
    let mut providers = ProviderRegistry::new();
    providers.register(anthropic_provider());
    let mut auth_store = AuthStore::default();
    auth_store.set_oauth(
        "anthropic",
        OAuthCredential {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at_ms: 42,
            account_id: Some("acct-ant".to_string()),
            organization_id: Some("org-123".to_string()),
            email: Some("dev@example.com".to_string()),
            plan_type: Some("max".to_string()),
            rate_limit_tier: Some("team_tier".to_string()),
            scopes: vec!["openid".to_string()],
            organization_name: None,
            organization_role: None,
            workspace_role: None,
        },
    );

    dispatch_command(
        &mut state,
        &supported_commands(),
        &sample_resources(),
        &mut providers,
        &mut auth_store,
        &session_store,
        "/usage",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Provider: Anthropic")
            && text.contains("Authentication: OAuth")
            && text.contains("Logged in as: dev@example.com")
            && text.contains("Organization: org-123")
            && text.contains("Plan: Max")
            && text.contains("Rate limit tier: Team Tier")
            && text.contains("Claude subscription usage")
            && text.contains("Current session: unavailable in local summary")
            && text.contains("Current week (all models): unavailable in local summary")
            && text.contains("Current week (Sonnet only): unavailable in local summary")
            && text.contains("Extra usage: unavailable in local summary")
    ));
}

#[test]
fn usage_command_shows_best_effort_openai_identity() {
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
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider());
    let mut auth_store = AuthStore::default();
    auth_store.set_oauth(
        "openai",
        OAuthCredential {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at_ms: 42,
            account_id: Some("acct-openai".to_string()),
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

    dispatch_command(
        &mut state,
        &supported_commands(),
        &sample_resources(),
        &mut providers,
        &mut auth_store,
        &session_store,
        "/usage",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Provider: OpenAI")
            && text.contains("Authentication: OAuth")
            && text.contains("Logged in as: dev@example.com")
            && text.contains("Account ID: acct-openai")
            && text.contains("Plan: Pro")
            && text.contains("OpenAI/Codex account usage")
            && text.contains("Provider-reported usage is unavailable in the local summary.")
    ));
}

#[test]
fn buddy_command_uses_loaded_mascot_intro() {
    let tempdir = tempdir().unwrap();
    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store
        .create_session(tempdir.path().to_path_buf())
        .unwrap();
    let mut config = PufferConfig::default();
    config.mascot.id = "clawd".to_string();
    config.mascot.display_name = "Clawd".to_string();
    let mut state = AppState::new(config, tempdir.path().to_path_buf(), session);
    let resources = LoadedResources {
        mascots: vec![LoadedItem {
            value: puffer_resources::MascotSpec {
                id: "clawd".to_string(),
                display_name: "Clawd".to_string(),
                introduction: "A sharp-eyed dockside reviewer.".to_string(),
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("mascots/clawd.yaml"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        ..LoadedResources::default()
    };

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/buddy",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Clawd is on duty.")
            && text.contains("mascot_id=clawd")
            && text.contains("A sharp-eyed dockside reviewer.")
    ));
}

fn anthropic_provider() -> ProviderDescriptor {
    ProviderDescriptor {
        id: "anthropic".to_string(),
        display_name: "Anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        default_api: "anthropic-messages".to_string(),
        auth_modes: Vec::new(),
        headers: Default::default(),
        query_params: Default::default(),
        discovery: Some(puffer_provider_registry::ModelDiscoveryConfig {
            path: "/v1/models".to_string(),
            response: puffer_provider_registry::ModelDiscoveryFormat::AnthropicModels,
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_reasoning: true,
            items_field: "data".to_string(),
            id_field: "id".to_string(),
            display_name_field: Some("display_name".to_string()),
            headers: Default::default(),
        }),
        models: vec![puffer_provider_registry::ModelDescriptor {
            id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            provider: "anthropic".to_string(),
            api: "anthropic-messages".to_string(),
            context_window: 200_000,
            max_output_tokens: 8_192,
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
        auth_modes: Vec::new(),
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![puffer_provider_registry::ModelDescriptor {
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            provider: "openai".to_string(),
            api: "openai-responses".to_string(),
            context_window: 272_000,
            max_output_tokens: 16_384,
            supports_reasoning: true,
        }],
    }
}

fn sample_resources() -> LoadedResources {
    LoadedResources {
        prompts: vec![LoadedItem {
            value: puffer_resources::PromptTemplate {
                id: "review".to_string(),
                description: "review".to_string(),
                template: "review".to_string(),
                variables: Vec::new(),
                allowed_tools: Vec::new(),
                provider_override: None,
                model_override: None,
                mode: None,
                chained_from: Vec::new(),
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("prompts/review.yaml"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        tools: vec![LoadedItem {
            value: puffer_resources::ToolSpec {
                id: "bash".to_string(),
                name: "bash".to_string(),
                description: "Run bash".to_string(),
                handler: "bash".to_string(),
                aliases: Vec::new(),
                handler_args: Vec::new(),
                approval_policy: Some("ask".to_string()),
                sandbox_policy: Some("workspace-write".to_string()),
                shared_lib: None,
                enabled_if: None,
                input_schema: None,
                metadata: Default::default(),
                display: Default::default(),
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("tools/bash.yaml"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        hooks: vec![LoadedItem {
            value: puffer_resources::HookSpec {
                id: "tool-end".to_string(),
                event: "tool_end".to_string(),
                command: "echo done".to_string(),
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("hooks/tool_end.yaml"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        skills: vec![LoadedItem {
            value: puffer_resources::SkillSpec {
                name: "reviewer".to_string(),
                description: "review".to_string(),
                content: "review".to_string(),
                disable_model_invocation: false,
                ..puffer_resources::SkillSpec::default()
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("skills/reviewer/SKILL.md"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        plugins: vec![LoadedItem {
            value: puffer_resources::PluginSpec {
                id: "core".to_string(),
                display_name: "Core".to_string(),
                description: "core".to_string(),
                commands: Vec::new(),
                skills: Vec::new(),
                agents: Vec::new(),
                mcp_servers: Vec::new(),
                lsp_servers: Vec::new(),
            },
            source_info: puffer_resources::SourceInfo {
                path: PathBuf::from("plugins/core.yaml"),
                kind: puffer_resources::SourceKind::Builtin,
            },
        }],
        ..LoadedResources::default()
    }
}
