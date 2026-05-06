use crate::dtos::{ExternalCredentialDto, SettingsSnapshotDto};
use crate::settings_data::load_settings_snapshot;
use anyhow::{anyhow, Context, Result};
use puffer_config::{ensure_workspace_dirs, load_config, save_user_config, ConfigPaths};
use puffer_provider_openai::{
    build_authorization_url as build_openai_authorization_url,
    exchange_authorization_code as exchange_openai_authorization_code,
    generate_pkce as generate_openai_pkce,
    parse_authorization_input as parse_openai_authorization_input, OpenAIOAuthConfig,
};
use puffer_provider_registry::{
    detect_import_candidates, AuthMode, AuthStore, ExternalImportCandidate, ExternalImportFamily,
    ExternalImportSource, ProviderRegistry, StoredCredential,
};
use puffer_resources::load_resources;
use puffer_transport_anthropic::{
    build_authorization_url as build_anthropic_authorization_url,
    exchange_authorization_code as exchange_anthropic_authorization_code,
    generate_pkce as generate_anthropic_pkce,
    parse_authorization_input as parse_anthropic_authorization_input, should_use_claude_ai_auth,
    AnthropicOAuthConfig, AnthropicOAuthCredentials, ANTHROPIC_API_BASE_URL,
    ANTHROPIC_MANUAL_REDIRECT_URL, CONSOLE_AUTHORIZE_URL,
};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Performs an OAuth login for the selected provider and returns the updated settings snapshot.
pub(crate) fn login_with_oauth(provider_id: &str) -> Result<SettingsSnapshotDto> {
    let DesktopAuthContext {
        paths,
        mut auth_store,
        providers,
        auth_path,
    } = load_auth_context()?;
    let Some(provider) = providers.provider(provider_id) else {
        return Err(anyhow!("unknown provider `{provider_id}`"));
    };
    if !provider.auth_modes.contains(&AuthMode::OAuth) {
        return Err(anyhow!("provider `{provider_id}` does not support OAuth"));
    }
    let credential = acquire_oauth_credential(provider_id, &provider.default_api)?;
    store_credential(&mut auth_store, provider_id, credential);
    auth_store.save(&auth_path)?;
    let _ = ensure_default_routing(&paths, &providers, &auth_store, provider_id);

    let _ = ensure_workspace_dirs(&paths);
    load_settings_snapshot()
}

/// Performs the local browser/callback OAuth flow and returns the resulting credential.
pub(crate) fn acquire_oauth_credential(
    provider_id: &str,
    provider_default_api: &str,
) -> Result<StoredCredential> {
    let callback_listener = CallbackListener::bind_localhost("/callback")?;
    let bundle = oauth_login_bundle(
        provider_id,
        provider_default_api,
        callback_listener.redirect_uri(),
    )?;
    let launch_url = bundle
        .automatic_authorization_url
        .as_deref()
        .unwrap_or(bundle.authorization_url.as_str());
    if !open_browser(launch_url) {
        return Err(anyhow!(
            "could not open the system browser for `{provider_id}` login"
        ));
    }

    let callback = callback_listener
        .wait_for_callback_url(Duration::from_secs(180))?
        .ok_or_else(|| anyhow!("timed out waiting for OAuth callback"))?;
    exchange_oauth_credential(provider_id, provider_default_api, &bundle, &callback)
}

/// Stores an API key credential and returns the updated settings snapshot.
pub(crate) fn login_with_api_key(provider_id: &str, api_key: &str) -> Result<SettingsSnapshotDto> {
    let DesktopAuthContext {
        paths,
        mut auth_store,
        providers,
        auth_path,
    } = load_auth_context()?;
    let Some(provider) = providers.provider(provider_id) else {
        return Err(anyhow!("unknown provider `{provider_id}`"));
    };
    if !provider.auth_modes.contains(&AuthMode::ApiKey) {
        return Err(anyhow!(
            "provider `{provider_id}` does not support API keys"
        ));
    }
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("api key cannot be empty"));
    }
    auth_store.set_api_key(provider_id.to_string(), trimmed.to_string());
    auth_store.save(&auth_path)?;
    let _ = ensure_default_routing(&paths, &providers, &auth_store, provider_id);
    load_settings_snapshot()
}

/// Scans `~/.claude` and `~/.codex` for credentials Puffer can adopt
/// without an interactive flow, returns one entry per (provider, source).
/// The desktop UI surfaces these so the user does not have to paste an
/// API key they already have stored elsewhere.
pub(crate) fn list_external_credentials() -> Result<Vec<ExternalCredentialDto>> {
    let DesktopAuthContext { providers, .. } = load_auth_context()?;
    let mut out = Vec::new();
    for entry in providers.provider_entries() {
        let Some(family) = import_family(&entry.descriptor.default_api) else {
            continue;
        };
        let candidates = detect_import_candidates(family).unwrap_or_default();
        for candidate in candidates {
            out.push(ExternalCredentialDto {
                provider_id: entry.descriptor.id.clone(),
                source: source_label(candidate.source).to_string(),
                kind: match &candidate.credential {
                    StoredCredential::ApiKey { .. } => "api_key".to_string(),
                    StoredCredential::OAuth(_) => "oauth".to_string(),
                },
                description: candidate.description.clone(),
                source_path: candidate.source_path.display().to_string(),
            });
        }
    }
    Ok(out)
}

/// Adopts a credential discovered by `list_external_credentials` for the
/// given provider, then returns the refreshed settings snapshot. Mirrors
/// the TUI auth-picker import path so the OpenAI-flavored providers also
/// pick up `openai_base_url` / headers / query params from
/// `~/.codex/config.toml` when present.
pub(crate) fn import_external_credential(
    provider_id: &str,
    source: &str,
) -> Result<SettingsSnapshotDto> {
    let DesktopAuthContext {
        paths,
        mut auth_store,
        providers,
        auth_path,
    } = load_auth_context()?;
    let provider = providers
        .provider(provider_id)
        .ok_or_else(|| anyhow!("unknown provider `{provider_id}`"))?;
    let family = import_family(&provider.default_api)
        .ok_or_else(|| anyhow!("provider `{provider_id}` has no importable credential family"))?;
    let source_kind = match source {
        "claude" => ExternalImportSource::Claude,
        "codex" => ExternalImportSource::Codex,
        other => return Err(anyhow!("unknown import source `{other}`")),
    };
    let candidate = detect_import_candidates(family)?
        .into_iter()
        .find(|candidate| candidate.source == source_kind)
        .ok_or_else(|| {
            anyhow!(
                "no `{source}` credential available on disk for provider `{provider_id}`"
            )
        })?;
    apply_imported_credential(&paths, &mut auth_store, provider_id, candidate)?;
    auth_store.save(&auth_path)?;
    let _ = ensure_default_routing(&paths, &providers, &auth_store, provider_id);
    let _ = ensure_workspace_dirs(&paths);
    load_settings_snapshot()
}

fn import_family(default_api: &str) -> Option<ExternalImportFamily> {
    match default_api {
        "anthropic-messages" => Some(ExternalImportFamily::Anthropic),
        "openai-responses"
        | "openai-completions"
        | "openai-codex-responses"
        | "azure-openai-responses" => Some(ExternalImportFamily::OpenAi),
        _ => None,
    }
}

fn source_label(source: ExternalImportSource) -> &'static str {
    match source {
        ExternalImportSource::Claude => "claude",
        ExternalImportSource::Codex => "codex",
    }
}

/// Makes sure the workspace's default routing points at a provider that
/// actually has credentials, so a fresh agent turn won't immediately fail
/// with "no provider authenticated." Switches `default_provider` when the
/// existing one is unauthenticated, and picks the just-authed provider's
/// first model as `default_model` when none is set or the current one is
/// from a different provider. Falls back to a no-op silently — it's a
/// nice-to-have, not part of the auth contract.
fn ensure_default_routing(
    paths: &ConfigPaths,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
    just_authed: &str,
) -> Result<()> {
    let mut config = load_config(paths)?;
    let mut changed = false;

    let default_has_creds = config
        .default_provider
        .as_deref()
        .map(|id| auth_store.get(id).is_some())
        .unwrap_or(false);
    if !default_has_creds {
        config.default_provider = Some(just_authed.to_string());
        changed = true;
    }

    let default_provider_id = config.default_provider.clone().unwrap_or_default();
    let provider_models = providers
        .provider(&default_provider_id)
        .map(|provider| provider.models.clone())
        .unwrap_or_default();
    let default_model_belongs = config
        .default_model
        .as_deref()
        .map(|model_id| provider_models.iter().any(|model| model.id == model_id))
        .unwrap_or(false);
    if !default_model_belongs {
        if let Some(first) = provider_models.first() {
            config.default_model = Some(first.id.clone());
            changed = true;
        }
    }

    if changed {
        save_user_config(paths, &config)?;
    }
    Ok(())
}

/// Persists an imported credential and — for the OpenAI provider — the
/// matching `openai_base_url` / headers / query-params from the source
/// config. Returns Ok(()) on success.
fn apply_imported_credential(
    paths: &ConfigPaths,
    auth_store: &mut AuthStore,
    provider_id: &str,
    candidate: ExternalImportCandidate,
) -> Result<()> {
    let openai_base_url = candidate.openai_base_url.clone();
    let openai_headers = candidate.openai_headers.clone();
    let openai_query_params = candidate.openai_query_params.clone();
    match candidate.credential {
        StoredCredential::ApiKey { key } => {
            auth_store.set_api_key(provider_id.to_string(), key);
        }
        StoredCredential::OAuth(credential) => {
            auth_store.set_oauth(provider_id.to_string(), credential);
        }
    }
    if provider_id == "openai" {
        let mut config = load_config(paths)?;
        config.openai_base_url = openai_base_url;
        config.openai_headers = openai_headers;
        config.openai_query_params = openai_query_params;
        save_user_config(paths, &config)?;
    }
    Ok(())
}

/// Removes stored credentials for a provider and returns the updated settings snapshot.
pub(crate) fn logout_provider(provider_id: &str) -> Result<SettingsSnapshotDto> {
    let DesktopAuthContext {
        mut auth_store,
        auth_path,
        ..
    } = load_auth_context()?;
    auth_store.remove(provider_id);
    auth_store.save(&auth_path)?;
    load_settings_snapshot()
}

struct DesktopAuthContext {
    paths: ConfigPaths,
    auth_store: AuthStore,
    providers: ProviderRegistry,
    auth_path: PathBuf,
}

struct OauthStartBundle {
    authorization_url: String,
    automatic_authorization_url: Option<String>,
    verifier: String,
    state: String,
}

fn load_auth_context() -> Result<DesktopAuthContext> {
    let workspace_root = std::env::current_dir()?;
    let paths = ConfigPaths::discover(&workspace_root);
    ensure_workspace_dirs(&paths)?;
    let auth_path = paths.user_config_dir.join("auth.json");
    let auth_store = AuthStore::load(&auth_path)?;
    let resources = load_resources(&paths)?;
    let mut providers = ProviderRegistry::new();
    for provider in &resources.providers {
        providers.register_with_source(
            provider.value.clone().into_descriptor(),
            provider.source_info.as_provider_source(),
        );
    }
    let _ = providers.discover_and_merge_all(&auth_store);
    Ok(DesktopAuthContext {
        paths,
        auth_store,
        providers,
        auth_path,
    })
}

fn oauth_login_bundle(
    provider_id: &str,
    provider_default_api: &str,
    redirect_uri: &str,
) -> Result<OauthStartBundle> {
    match provider_default_api {
        "openai-responses"
        | "openai-completions"
        | "azure-openai-responses"
        | "openai-codex-responses" => {
            let pkce = generate_openai_pkce();
            let config = OpenAIOAuthConfig {
                state: pkce.state.clone(),
                code_challenge: pkce.challenge.clone(),
                redirect_uri: redirect_uri.to_string(),
                ..OpenAIOAuthConfig::default()
            };
            Ok(OauthStartBundle {
                authorization_url: build_openai_authorization_url(&config),
                automatic_authorization_url: None,
                verifier: pkce.verifier,
                state: pkce.state,
            })
        }
        "anthropic-messages" => {
            let pkce = generate_anthropic_pkce();
            let mut automatic = AnthropicOAuthConfig {
                state: pkce.state.clone(),
                code_challenge: pkce.challenge.clone(),
                redirect_uri: redirect_uri.to_string(),
                ..AnthropicOAuthConfig::default()
            };
            if provider_id != "anthropic" {
                automatic.authorize_url = CONSOLE_AUTHORIZE_URL.to_string();
            }
            let mut manual = automatic.clone();
            manual.redirect_uri = ANTHROPIC_MANUAL_REDIRECT_URL.to_string();
            Ok(OauthStartBundle {
                authorization_url: build_anthropic_authorization_url(&manual),
                automatic_authorization_url: Some(build_anthropic_authorization_url(&automatic)),
                verifier: pkce.verifier,
                state: pkce.state,
            })
        }
        other => Err(anyhow!(
            "oauth login is not implemented for provider api `{other}`"
        )),
    }
}

fn exchange_oauth_credential(
    _provider_id: &str,
    provider_default_api: &str,
    bundle: &OauthStartBundle,
    callback: &str,
) -> Result<StoredCredential> {
    match provider_default_api {
        "openai-responses"
        | "openai-completions"
        | "azure-openai-responses"
        | "openai-codex-responses" => {
            let (code, parsed_state) = parse_openai_authorization_input(callback);
            let code =
                code.ok_or_else(|| anyhow!("could not extract an OpenAI authorization code"))?;
            if let Some(parsed_state) = parsed_state {
                if parsed_state != bundle.state {
                    return Err(anyhow!("oauth state mismatch for openai"));
                }
            }
            let credential = exchange_openai_authorization_code(&code, &bundle.verifier, None)?;
            Ok(StoredCredential::OAuth(
                puffer_provider_registry::OAuthCredential {
                    access_token: credential.access_token,
                    refresh_token: credential.refresh_token,
                    expires_at_ms: credential.expires_at_ms,
                    account_id: credential.account_id,
                    organization_id: None,
                    email: credential.email,
                    plan_type: credential.plan_type,
                    rate_limit_tier: None,
                    scopes: Vec::new(),
                    organization_name: None,
                    organization_role: None,
                    workspace_role: None,
                },
            ))
        }
        "anthropic-messages" => {
            let (code, parsed_state) = parse_anthropic_authorization_input(callback);
            let code =
                code.ok_or_else(|| anyhow!("could not extract an Anthropic authorization code"))?;
            let parsed_state = parsed_state.unwrap_or_else(|| bundle.state.clone());
            if parsed_state != bundle.state {
                return Err(anyhow!("oauth state mismatch for anthropic"));
            }
            let redirect_uri = inferred_anthropic_redirect_uri(callback)
                .unwrap_or_else(|| ANTHROPIC_MANUAL_REDIRECT_URL.to_string());
            let credential = exchange_anthropic_authorization_code(
                &code,
                &bundle.verifier,
                &bundle.state,
                Some(&redirect_uri),
                Some(ANTHROPIC_API_BASE_URL),
            )?;
            anthropic_stored_credential(credential)
        }
        other => Err(anyhow!(
            "oauth exchange is not implemented for provider api `{other}`"
        )),
    }
}

fn anthropic_stored_credential(credential: AnthropicOAuthCredentials) -> Result<StoredCredential> {
    if should_use_claude_ai_auth(&credential.scopes) {
        Ok(StoredCredential::OAuth(
            puffer_provider_registry::OAuthCredential {
                access_token: credential.access_token,
                refresh_token: credential.refresh_token,
                expires_at_ms: credential.expires_at_ms,
                account_id: credential.account_uuid,
                organization_id: credential.organization_uuid,
                email: credential.email_address,
                plan_type: credential.plan_type,
                rate_limit_tier: credential.rate_limit_tier,
                scopes: credential.scopes,
                organization_name: credential.organization_name,
                organization_role: credential.organization_role,
                workspace_role: credential.workspace_role,
            },
        ))
    } else {
        let api_key = puffer_transport_anthropic::create_api_key(
            ANTHROPIC_API_BASE_URL,
            &credential.access_token,
        )?;
        Ok(StoredCredential::ApiKey { key: api_key })
    }
}

fn store_credential(auth_store: &mut AuthStore, provider_id: &str, credential: StoredCredential) {
    match credential {
        StoredCredential::ApiKey { key } => auth_store.set_api_key(provider_id.to_string(), key),
        StoredCredential::OAuth(credential) => {
            auth_store.set_oauth(provider_id.to_string(), credential)
        }
    }
}

fn inferred_anthropic_redirect_uri(input: &str) -> Option<String> {
    let mut url = url::Url::parse(input.trim()).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

struct CallbackListener {
    listener: TcpListener,
    host: String,
    port: u16,
    expected_path: String,
    redirect_uri: String,
}

impl CallbackListener {
    fn bind_localhost(path: &str) -> Result<Self> {
        let listener = TcpListener::bind(("localhost", 0))
            .with_context(|| format!("failed to bind callback listener for {path}"))?;
        listener.set_nonblocking(true)?;
        let port = listener
            .local_addr()
            .context("failed to read callback listener address")?
            .port();
        Ok(Self {
            listener,
            host: "localhost".to_string(),
            port,
            expected_path: path.to_string(),
            redirect_uri: format!("http://localhost:{port}{path}"),
        })
    }

    fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    fn wait_for_callback_url(&self, timeout: Duration) -> Result<Option<String>> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0_u8; 4096];
                    let bytes_read = stream.read(&mut buffer)?;
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
                    if let Some(callback_url) =
                        parse_callback_request(&request, &self.host, self.port, &self.expected_path)
                    {
                        let _ = stream.write_all(success_response().as_bytes());
                        return Ok(Some(callback_url));
                    }
                    let _ = stream.write_all(error_response().as_bytes());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(None)
    }
}

fn open_browser(url: &str) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command.spawn().is_ok()
}

fn parse_callback_request(
    request: &str,
    host: &str,
    port: u16,
    expected_path: &str,
) -> Option<String> {
    let line = request.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    if method != "GET" {
        return None;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != expected_path {
        return None;
    }
    let suffix = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    };
    Some(format!("http://{host}:{port}{suffix}"))
}

fn success_response() -> &'static str {
    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 74\r\n\r\n<html><body>Authentication completed. You can return to Puffer.</body></html>"
}

fn error_response() -> &'static str {
    "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 53\r\n\r\n<html><body>Invalid callback for Puffer.</body></html>"
}
