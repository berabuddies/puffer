use crate::AppState;
use anyhow::{bail, Result};
use puffer_config::{save_user_config, ConfigPaths};
use puffer_provider_openai::{
    build_authorization_url as build_openai_authorization_url,
    generate_pkce as generate_openai_pkce, OpenAIOAuthConfig,
};
use puffer_provider_registry::{canonical_provider_id, AuthMode, AuthStore, ProviderDescriptor};
use puffer_transport_anthropic::{
    build_authorization_url as build_anthropic_authorization_url,
    generate_pkce as generate_anthropic_pkce, AnthropicOAuthConfig, CONSOLE_AUTHORIZE_URL,
};
use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoginFlowRequest {
    pub provider_id: String,
    pub auth_path: PathBuf,
}

thread_local! {
    static LOGIN_FLOW_HANDLER: RefCell<Option<Box<dyn FnMut(LoginFlowRequest) -> Result<()>>>> =
        const { RefCell::new(None) };
}

/// Returns whether a provider advertises the requested auth mode.
pub(crate) fn supports_auth_mode(
    provider: Option<&ProviderDescriptor>,
    auth_mode: AuthMode,
) -> bool {
    provider
        .map(|descriptor| descriptor.auth_modes.contains(&auth_mode))
        .unwrap_or(false)
}

/// Renders the supported auth-mode summary for a provider descriptor.
pub(crate) fn render_provider_auth_summary(provider: &ProviderDescriptor) -> String {
    let modes = if provider.auth_modes.is_empty() {
        String::from("none")
    } else {
        provider
            .auth_modes
            .iter()
            .map(|mode| match mode {
                AuthMode::ApiKey => "api_key",
                AuthMode::OAuth => "oauth",
                AuthMode::SessionIngress => "session_ingress",
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    if modes == "none" {
        "Supported auth modes: none (provider does not require stored credentials)".to_string()
    } else {
        format!("Supported auth modes: {modes}")
    }
}

/// Renders an OAuth hint string for a provider when Puffer knows the provider family.
pub(crate) fn render_oauth_hint(provider: &str, descriptor: Option<&ProviderDescriptor>) -> String {
    if !supports_auth_mode(descriptor, AuthMode::OAuth) {
        return format!("OAuth: not advertised for {provider}.");
    }

    match oauth_family(descriptor, provider) {
        Some("openai") => {
            let pkce = generate_openai_pkce();
            let config = OpenAIOAuthConfig {
                state: pkce.state.clone(),
                code_challenge: pkce.challenge.clone(),
                ..OpenAIOAuthConfig::default()
            };
            format!(
                "OAuth start bundle:\nurl={}\nverifier={}\nstate={}",
                build_openai_authorization_url(&config),
                pkce.verifier,
                pkce.state
            )
        }
        Some("anthropic") => {
            let pkce = generate_anthropic_pkce();
            let mut config = AnthropicOAuthConfig {
                state: pkce.state.clone(),
                code_challenge: pkce.challenge.clone(),
                ..AnthropicOAuthConfig::default()
            };
            if provider != "anthropic" {
                config.authorize_url = CONSOLE_AUTHORIZE_URL.to_string();
            }
            format!(
                "OAuth start bundle:\nurl={}\nverifier={}\nstate={}",
                build_anthropic_authorization_url(&config),
                pkce.verifier,
                pkce.state
            )
        }
        _ => format!(
            "OAuth: provider metadata advertises oauth, but Puffer has no built-in OAuth starter for {provider} yet."
        ),
    }
}

fn oauth_family(descriptor: Option<&ProviderDescriptor>, provider: &str) -> Option<&'static str> {
    match descriptor.map(|entry| entry.default_api.as_str()) {
        Some(
            "openai-responses"
            | "openai-completions"
            | "openai-codex-responses"
            | "azure-openai-responses",
        ) => Some("openai"),
        Some("anthropic-messages") => Some("anthropic"),
        Some(_) => None,
        None => match provider {
            "openai" | "openai-codex" | "openai-codex-responses" | "azure-openai-responses" => {
                Some("openai")
            }
            "anthropic" => Some("anthropic"),
            _ => None,
        },
    }
}

/// Renders the full `/login` guidance block for a provider.
pub(crate) fn render_login_guidance(
    provider: &str,
    descriptor: Option<&ProviderDescriptor>,
    has_auth: bool,
) -> String {
    if descriptor
        .map(|provider_descriptor| provider_descriptor.auth_modes.is_empty())
        .unwrap_or(false)
    {
        return format!("{provider} does not require stored credentials.");
    }

    let status = if has_auth {
        "Credentials are already stored."
    } else {
        "No credentials are currently stored."
    };
    let auth_summary = descriptor
        .map(render_provider_auth_summary)
        .unwrap_or_else(|| "Supported auth modes: unknown".to_string());
    let oauth_hint = render_oauth_hint(provider, descriptor);
    let api_key_hint = if supports_auth_mode(descriptor, AuthMode::ApiKey) || descriptor.is_none() {
        format!("API key: `puffer auth set-api-key {provider} --stdin`")
    } else {
        String::from("API key auth is not advertised for this provider.")
    };
    let session_hint = if supports_auth_mode(descriptor, AuthMode::SessionIngress) {
        String::from("Session ingress: exported session-ingress credentials are supported.")
    } else {
        String::new()
    };
    format!(
        "{status}\n{auth_summary}\n{api_key_hint}\n{oauth_hint}{}",
        if session_hint.is_empty() {
            String::new()
        } else {
            format!("\n{session_hint}")
        }
    )
}

/// Runs a closure while the current thread has an active login-flow test handler.
#[cfg(test)]
pub(crate) fn with_login_flow_handler<R>(
    handler: impl FnMut(LoginFlowRequest) -> Result<()> + 'static,
    run: impl FnOnce() -> R,
) -> R {
    LOGIN_FLOW_HANDLER.with(|slot| {
        let previous = slot.borrow_mut().take();
        *slot.borrow_mut() = Some(Box::new(handler));
        let result = run();
        let _ = slot.borrow_mut().take();
        *slot.borrow_mut() = previous;
        result
    })
}

/// Executes the provider-specific interactive login flow and reloads stored credentials.
pub(crate) fn run_provider_login_flow(
    state: &AppState,
    auth_store: &mut AuthStore,
    provider_id: &str,
) -> Result<String> {
    let provider_id = canonical_provider_id(provider_id);
    if provider_id.is_empty() {
        bail!("provider id is required");
    }

    let auth_path = ConfigPaths::discover(&state.cwd)
        .user_config_dir
        .join("auth.json");
    run_login_command(LoginFlowRequest {
        provider_id: provider_id.clone(),
        auth_path: auth_path.clone(),
    })?;
    *auth_store = AuthStore::load(&auth_path)?;
    if !auth_store.has_auth(&provider_id) {
        bail!("login flow for {provider_id} completed without storing credentials");
    }
    Ok(format!("Completed login flow for {provider_id}."))
}

/// Removes stored credentials for one provider and clears the active selection when needed.
pub(crate) fn remove_provider_credentials(
    state: &mut AppState,
    auth_store: &mut AuthStore,
    provider_id: &str,
) -> Result<String> {
    let provider_id = canonical_provider_id(provider_id);
    let removed = auth_store.remove(&provider_id);
    let cleared_active_provider = active_selection_uses_provider(state, &provider_id);

    if removed.is_some() {
        let auth_path = ConfigPaths::discover(&state.cwd)
            .user_config_dir
            .join("auth.json");
        auth_store.save(&auth_path)?;
    }

    if cleared_active_provider {
        state.current_provider = None;
        state.current_model = None;
        state.config.default_provider = None;
        state.config.default_model = None;
        let paths = ConfigPaths::discover(&state.cwd);
        save_user_config(&paths, &state.config)?;
    }

    let message = if removed.is_some() {
        if cleared_active_provider {
            format!(
                "Removed stored credentials for {provider_id} and cleared the active selection."
            )
        } else {
            format!("Removed stored credentials for {provider_id}.")
        }
    } else if cleared_active_provider {
        format!("No stored credentials exist for {provider_id}; cleared the active selection.")
    } else {
        format!("No stored credentials exist for {provider_id}.")
    };
    Ok(message)
}

fn run_login_command(request: LoginFlowRequest) -> Result<()> {
    if let Some(result) = LOGIN_FLOW_HANDLER.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        borrowed.as_mut().map(|handler| handler(request.clone()))
    }) {
        return result;
    }

    let status = Command::new(std::env::current_exe()?)
        .arg("auth")
        .arg("login")
        .arg(&request.provider_id)
        .status()?;
    if !status.success() {
        bail!(
            "login flow for {} exited with {}",
            request.provider_id,
            status
        );
    }
    Ok(())
}

fn active_selection_uses_provider(state: &AppState, provider_id: &str) -> bool {
    if state
        .current_provider
        .as_deref()
        .map(canonical_provider_id)
        .as_deref()
        == Some(provider_id)
    {
        return true;
    }
    state
        .current_model
        .as_deref()
        .and_then(|selector| selector.split_once('/'))
        .map(|(provider, _)| canonical_provider_id(provider) == provider_id)
        .unwrap_or(false)
}
