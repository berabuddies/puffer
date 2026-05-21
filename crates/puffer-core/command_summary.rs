use crate::{supported_commands, AppState, CommandSpec, MessageRole};
use puffer_provider_registry::{
    AuthStore, OAuthCredential, ProviderDescriptor, ProviderRegistry, StoredCredential,
};
use puffer_resources::{mascot_by_id, LoadedResources};
use puffer_tools::ToolRegistry;
use puffer_transport_anthropic::{
    fetch_oauth_usage, AnthropicExtraUsage, AnthropicRateLimit, ANTHROPIC_API_BASE_URL,
};

/// Renders a lightweight local cost-style summary for the active session.
pub fn render_cost_summary(state: &AppState) -> String {
    let elapsed_ms = now_ms().saturating_sub(state.session.created_at_ms);
    let assistant_messages = state
        .transcript
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .count();
    let user_messages = state
        .transcript
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .count();
    let tool_invocations = state
        .transcript
        .iter()
        .filter(|message| message.role == MessageRole::System && message.text.starts_with("Tool "))
        .count();
    format!(
        "Session cost summary:\nelapsed_ms={elapsed_ms}\nuser_messages={user_messages}\nassistant_messages={assistant_messages}\ntool_invocations={tool_invocations}\nrecorded_tasks={}\nestimated_cost_usd=unavailable",
        state.tasks().len()
    )
}

/// Renders a combined usage summary across runtime state and loaded resources.
pub(crate) fn render_usage_summary(
    state: &AppState,
    commands: &[CommandSpec],
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> String {
    let active_provider_id = state.current_provider.as_deref();
    let active_provider =
        active_provider_id.and_then(|provider_id| providers.provider(provider_id));
    let active_credential = active_provider_id.and_then(|provider_id| auth_store.get(provider_id));
    let mut sections = vec![String::from("Usage")];
    sections.extend(account_summary_lines(
        state,
        active_provider,
        active_credential,
    ));
    sections.push(String::new());
    sections.extend(provider_usage_lines(active_provider, active_credential));
    sections.push(String::new());
    sections.extend(runtime_summary_lines(
        state, commands, resources, providers, auth_store,
    ));
    sections.join("\n")
}

/// Renders the richer status summary used by `/status` and the interactive status overlay.
pub(crate) fn render_status_summary(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> String {
    let active_provider_id = state.current_provider.as_deref();
    let active_provider =
        active_provider_id.and_then(|provider_id| providers.provider(provider_id));
    let active_credential = active_provider_id.and_then(|provider_id| auth_store.get(provider_id));
    let commands = supported_commands();
    let registry = ToolRegistry::from_resources(resources);

    let mut sections = vec![String::from("Status")];
    sections.extend(account_summary_lines(
        state,
        active_provider,
        active_credential,
    ));
    sections.push(String::new());
    sections.extend(provider_status_lines(active_provider));
    sections.push(String::new());
    sections.extend(session_status_lines(state));
    sections.push(String::new());
    sections.push(String::from("Resource status"));
    sections.push(format!("Supported commands: {}", commands.len()));
    sections.push(format!("Executable tools: {}", registry.tools().count()));
    sections.push(format!("Loaded tools: {}", resources.tools.len()));
    sections.push(format!("Prompts: {}", resources.prompts.len()));
    sections.push(format!("Skills: {}", resources.skills.len()));
    sections.push(format!("Plugins: {}", resources.plugins.len()));
    sections.push(format!("MCP servers: {}", resources.mcp_servers.len()));
    sections.push(format!("IDEs: {}", resources.ides.len()));
    sections.push(format!("Hooks: {}", resources.hooks.len()));
    sections.push(format!(
        "Resource diagnostics: {}",
        resources.diagnostics.len()
    ));
    sections.push(String::new());
    sections.extend(runtime_summary_lines(
        state, &commands, resources, providers, auth_store,
    ));
    sections.join("\n")
}

/// Renders the current mascot summary, including any loaded introduction text.
pub(crate) fn render_buddy_summary(state: &AppState, resources: &LoadedResources) -> String {
    let intro = mascot_by_id(resources, &state.config.mascot.id)
        .map(|mascot| mascot.introduction.as_str())
        .unwrap_or("No mascot resource is currently loaded for this session.");
    format!(
        "{} is on duty.\nmascot_id={}\nenabled={}\n{}",
        state.config.mascot.display_name,
        state.config.mascot.id,
        state.config.mascot.enabled,
        intro
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn account_summary_lines(
    state: &AppState,
    provider: Option<&ProviderDescriptor>,
    credential: Option<&StoredCredential>,
) -> Vec<String> {
    let provider_label = provider
        .map(|provider| provider.display_name.as_str())
        .or(state.current_provider.as_deref())
        .unwrap_or("<unset>");
    let auth_label = credential_auth_label(state, credential);
    let mut lines = vec![
        format!("Provider: {provider_label}"),
        format!(
            "Model: {}",
            state.current_model.as_deref().unwrap_or("<unset>")
        ),
        format!("Authentication: {auth_label}"),
    ];

    if let Some(StoredCredential::OAuth(credential)) = credential {
        match provider_family(provider, state.current_provider.as_deref()) {
            ProviderSummaryFamily::Anthropic => {
                if let Some(email) = credential.email.as_deref() {
                    lines.push(format!("Logged in as: {email}"));
                }
                if let Some(organization_name) = credential.organization_name.as_deref() {
                    lines.push(format!(
                        "Organization: {}",
                        format_metadata_value(organization_name)
                    ));
                } else if let Some(organization_id) = credential.organization_id.as_deref() {
                    lines.push(format!("Organization: {organization_id}"));
                }
                if let Some(organization_role) = credential.organization_role.as_deref() {
                    lines.push(format!(
                        "Organization role: {}",
                        format_metadata_value(organization_role)
                    ));
                }
                if let Some(workspace_role) = credential.workspace_role.as_deref() {
                    lines.push(format!(
                        "Workspace role: {}",
                        format_metadata_value(workspace_role)
                    ));
                }
                if let Some(plan_type) = credential.plan_type.as_deref() {
                    lines.push(format!("Plan: {}", format_metadata_value(plan_type)));
                }
                if let Some(rate_limit_tier) = credential.rate_limit_tier.as_deref() {
                    lines.push(format!(
                        "Rate limit tier: {}",
                        format_metadata_value(rate_limit_tier)
                    ));
                }
                if let Some(account_id) = credential.account_id.as_deref() {
                    lines.push(format!("Account ID: {account_id}"));
                }
            }
            ProviderSummaryFamily::OpenAi => {
                if let Some(email) = credential.email.as_deref() {
                    lines.push(format!("Logged in as: {email}"));
                }
                if let Some(account_id) = credential.account_id.as_deref() {
                    lines.push(format!("Account ID: {account_id}"));
                }
                if let Some(plan_type) = credential.plan_type.as_deref() {
                    lines.push(format!("Plan: {}", format_metadata_value(plan_type)));
                }
            }
            ProviderSummaryFamily::Other => {
                if let Some(email) = credential.email.as_deref() {
                    lines.push(format!("Account: {email}"));
                }
                if let Some(account_id) = credential.account_id.as_deref() {
                    lines.push(format!("Account ID: {account_id}"));
                }
            }
        }
    }

    lines
}

fn provider_usage_lines(
    provider: Option<&ProviderDescriptor>,
    credential: Option<&StoredCredential>,
) -> Vec<String> {
    match provider_family(provider, provider.map(|provider| provider.id.as_str())) {
        ProviderSummaryFamily::Anthropic => anthropic_usage_lines(provider, credential),
        ProviderSummaryFamily::OpenAi => openai_usage_lines(credential),
        ProviderSummaryFamily::Other => generic_usage_lines(credential),
    }
}

fn anthropic_usage_lines(
    provider: Option<&ProviderDescriptor>,
    credential: Option<&StoredCredential>,
) -> Vec<String> {
    let mut lines = vec![String::from("Claude subscription usage")];
    let Some(StoredCredential::OAuth(credential)) = credential else {
        lines.push(String::from(
            "Sign in with Anthropic OAuth to view Claude subscription limits.",
        ));
        return lines;
    };

    if should_attempt_live_anthropic_usage(credential) {
        if let Some(provider) = provider {
            let usage_base_url = if provider.id == "anthropic" {
                ANTHROPIC_API_BASE_URL
            } else {
                &provider.base_url
            };
            match fetch_oauth_usage(usage_base_url, &credential.access_token) {
                Ok(usage) => {
                    lines.push(format_rate_limit_line(
                        "Current session",
                        usage.five_hour.as_ref(),
                    ));
                    lines.push(format_rate_limit_line(
                        "Current week (all models)",
                        usage.seven_day.as_ref(),
                    ));
                    if shows_anthropic_sonnet_limit(credential) {
                        lines.push(format_rate_limit_line(
                            "Current week (Sonnet only)",
                            usage.seven_day_sonnet.as_ref(),
                        ));
                    }
                    if supports_extra_usage(credential) {
                        lines.push(format_extra_usage_line(usage.extra_usage.as_ref()));
                    }
                    return lines;
                }
                Err(error) => {
                    lines.push(format!("Live Claude usage fetch failed: {error}"));
                }
            }
        }
    }
    lines.push(String::from(
        "Current session: unavailable in local summary",
    ));
    lines.push(String::from(
        "Current week (all models): unavailable in local summary",
    ));
    if shows_anthropic_sonnet_limit(credential) {
        lines.push(String::from(
            "Current week (Sonnet only): unavailable in local summary",
        ));
    }
    if supports_extra_usage(credential) {
        lines.push(String::from("Extra usage: unavailable in local summary"));
    }
    lines
}

fn openai_usage_lines(credential: Option<&StoredCredential>) -> Vec<String> {
    let mut lines = vec![String::from("OpenAI/Codex account usage")];
    match credential {
        Some(StoredCredential::OAuth(_)) => {
            lines.push(String::from(
                "Provider-reported usage is unavailable in the local summary.",
            ));
        }
        Some(StoredCredential::ApiKey { .. }) => {
            lines.push(String::from(
                "Sign in with OAuth to view account-linked usage details.",
            ));
        }
        None => lines.push(String::from(
            "Add credentials to view provider-specific account details.",
        )),
    }
    lines
}

fn generic_usage_lines(credential: Option<&StoredCredential>) -> Vec<String> {
    let mut lines = vec![String::from("Provider usage")];
    match credential {
        Some(StoredCredential::OAuth(_)) => lines.push(String::from(
            "Provider-specific usage details are unavailable in the local summary.",
        )),
        Some(StoredCredential::ApiKey { .. }) => lines.push(String::from(
            "OAuth-backed account usage is unavailable for API-key auth.",
        )),
        None => lines.push(String::from(
            "Select a provider and add credentials to view provider-specific usage.",
        )),
    }
    lines
}

fn runtime_summary_lines(
    state: &AppState,
    commands: &[CommandSpec],
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> Vec<String> {
    let providers_with_discovery = providers
        .provider_entries()
        .filter(|provider| provider.descriptor.discovery.is_some())
        .count();
    vec![
        String::from("Runtime summary"),
        format!("Commands: {}", commands.len()),
        format!("Messages: {}", state.transcript.len()),
        format!("Providers: {}", providers.providers().count()),
        format!("Models: {}", providers.models().count()),
        format!(
            "Authenticated providers: {}",
            auth_store.provider_ids().count()
        ),
        format!("Providers with discovery: {providers_with_discovery}"),
        format!("Prompts: {}", resources.prompts.len()),
        format!("Tools: {}", resources.tools.len()),
        format!("Skills: {}", resources.skills.len()),
        format!("Plugins: {}", resources.plugins.len()),
        format!("Hooks: {}", resources.hooks.len()),
    ]
}

fn provider_status_lines(provider: Option<&ProviderDescriptor>) -> Vec<String> {
    let Some(provider) = provider else {
        return vec![
            String::from("Provider details"),
            String::from("Base URL: <unset>"),
            String::from("Default API: <unset>"),
        ];
    };
    vec![
        String::from("Provider details"),
        format!("Base URL: {}", provider.base_url),
        format!("Default API: {}", provider.default_api),
    ]
}

fn session_status_lines(state: &AppState) -> Vec<String> {
    let mut lines = vec![
        String::from("Session"),
        format!("Session ID: {}", state.session.id),
        format!(
            "Display name: {}",
            state.session.display_name.as_deref().unwrap_or("<unnamed>")
        ),
        format!("Working directory: {}", state.cwd.display()),
        format!("Transcript messages: {}", state.transcript.len()),
        format!("Working dirs: {}", state.working_dirs.len()),
        format!("Prompt color: {}", state.prompt_color),
        format!("Effort: {}", state.effort_level),
        format!("Fast mode: {}", state.fast_mode),
        format!("Plan mode: {}", state.plan_mode),
        format!("Sandbox mode: {}", state.sandbox_mode),
        format!("Vim mode: {}", state.vim_mode),
        format!("Status line enabled: {}", state.statusline_enabled),
    ];
    if let Some(status_line) = state.config.ui.status_line.as_ref() {
        lines.push(format!("Status line command: {}", status_line.command));
        lines.push(format!("Status line padding: {}", status_line.padding));
    }
    if let Some(remote_name) = state.remote_name.as_deref() {
        lines.push(format!("Remote name: {remote_name}"));
    }
    if let Some(remote_environment) = state.remote_environment.as_deref() {
        lines.push(format!("Remote environment: {remote_environment}"));
    }
    if let Some(remote_status) = state.remote_session_status.as_deref() {
        lines.push(format!("Remote status: {remote_status}"));
    }
    if let Some(goal) = state.session_goal.as_ref() {
        lines.push(format!("Goal: {}", goal.objective));
        if let Some(budget) = goal.token_budget {
            lines.push(format!("Goal budget: {budget} tokens"));
        }
    }
    lines
}

fn credential_auth_label(state: &AppState, credential: Option<&StoredCredential>) -> &'static str {
    match credential {
        Some(StoredCredential::ApiKey { .. }) => "API key",
        Some(StoredCredential::OAuth(_)) => "OAuth",
        None if state.current_provider.is_some() => "missing",
        None => "n/a",
    }
}

fn shows_anthropic_sonnet_limit(credential: &OAuthCredential) -> bool {
    matches!(credential.plan_type.as_deref(), Some("max" | "team"))
        || credential.plan_type.is_none()
}

fn supports_extra_usage(credential: &OAuthCredential) -> bool {
    matches!(credential.plan_type.as_deref(), Some("pro" | "max"))
}

fn should_attempt_live_anthropic_usage(credential: &OAuthCredential) -> bool {
    credential.access_token.len() >= 20
}

fn format_rate_limit_line(title: &str, bucket: Option<&AnthropicRateLimit>) -> String {
    let Some(bucket) = bucket else {
        return format!("{title}: unavailable");
    };
    let utilization = bucket
        .utilization
        .map(|value| format!("{}% used", value.floor() as i64))
        .unwrap_or_else(|| "unavailable".to_string());
    let reset = bucket
        .resets_at
        .as_deref()
        .map(|value| format!(" · resets {value}"))
        .unwrap_or_default();
    format!("{title}: {utilization}{reset}")
}

fn format_extra_usage_line(extra_usage: Option<&AnthropicExtraUsage>) -> String {
    let Some(extra_usage) = extra_usage else {
        return String::from("Extra usage: unavailable");
    };
    if !extra_usage.is_enabled {
        return String::from("Extra usage: disabled");
    }
    let utilization = extra_usage
        .utilization
        .map(|value| format!("{}% used", value.floor() as i64))
        .unwrap_or_else(|| "enabled".to_string());
    let limit = extra_usage
        .monthly_limit
        .map(|value| format!(" · monthly limit {}", value.floor() as i64))
        .unwrap_or_default();
    format!("Extra usage: {utilization}{limit}")
}

fn format_metadata_value(value: &str) -> String {
    value
        .split(['_', '-', ' '])
        .filter(|part| !part.is_empty())
        .map(capitalize_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalize_word(value: &str) -> String {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    let mut text = String::new();
    text.push(first.to_ascii_uppercase());
    text.push_str(&characters.as_str().to_ascii_lowercase());
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderSummaryFamily {
    Anthropic,
    OpenAi,
    Other,
}

fn provider_family(
    provider: Option<&ProviderDescriptor>,
    provider_id: Option<&str>,
) -> ProviderSummaryFamily {
    let provider_id = provider_id.unwrap_or_default();
    let api = provider
        .map(|provider| provider.default_api.as_str())
        .unwrap_or_default();
    if provider_id == "anthropic" || api == "anthropic-messages" {
        ProviderSummaryFamily::Anthropic
    } else if provider_id == "openai"
        || api.starts_with("openai")
        || api.contains("responses")
        || api.contains("completions")
    {
        ProviderSummaryFamily::OpenAi
    } else {
        ProviderSummaryFamily::Other
    }
}
