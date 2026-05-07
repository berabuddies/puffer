//! High-level orchestration around rmcp's [`AuthorizationManager`].
//!
//! Two entry points:
//!
//! * [`OAuthService::resolve`] — silent / non-interactive. Loads any
//!   stored tokens, refreshes if expired, and returns a ready-to-use
//!   [`AuthClient<reqwest::Client>`] that the streamable-HTTP transport
//!   can consume directly. Returns [`OAuthError::OAuthRequired`] when no
//!   stored tokens exist or refresh definitively failed — the runner's
//!   caller decides whether to drive the interactive flow or surface the
//!   error to the orchestrator.
//! * [`OAuthService::interactive_login`] — drives the full RFC-6749
//!   authorization-code-with-PKCE flow: discovery → DCR → browser open →
//!   localhost callback → token exchange → persist. Synchronous in the
//!   sense that the future doesn't resolve until the user finishes the
//!   browser dance (or `cancel_token` fires).

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client as HttpClient;
use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationManager, OAuthClientConfig, OAuthState,
};
use thiserror::Error;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::callback::{spawn_callback_server, CallbackHandle, CallbackParams};
use crate::store::FileCredentialStore;

/// Configuration for one MCP server's OAuth integration.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    /// Server identifier — used as the on-disk filename prefix.
    pub server_id: String,
    /// Base URL of the MCP server (used as the OAuth resource URL for
    /// metadata discovery via [`AuthorizationManager`]).
    pub server_url: String,
    /// Scopes to request. Empty = use the auth server's default.
    pub scopes: Vec<String>,
    /// Display name registered with the auth server during DCR.
    pub client_name: String,
    /// Directory where token files live. Created lazily on first save.
    pub token_dir: std::path::PathBuf,
}

/// Public error surface for the OAuth service.
///
/// `OAuthRequired` is the headless-mode escape hatch: returned when no
/// stored tokens are present (or refresh failed in a way that requires a
/// fresh user interaction). The carried `authorization_url` may be
/// `None` if discovery hasn't run yet — callers that want a URL to show
/// the user should call `interactive_login` themselves to drive
/// discovery + DCR + URL minting in one shot.
#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("OAuth required for MCP server `{server_id}`")]
    OAuthRequired {
        server_id: String,
        authorization_url: Option<String>,
    },
    #[error("OAuth metadata discovery failed for `{server_id}`: {source}")]
    Discovery {
        server_id: String,
        #[source]
        source: AuthError,
    },
    #[error("OAuth dynamic client registration failed for `{server_id}`: {source}")]
    Registration {
        server_id: String,
        #[source]
        source: AuthError,
    },
    #[error("OAuth token exchange failed for `{server_id}`: {source}")]
    TokenExchange {
        server_id: String,
        #[source]
        source: AuthError,
    },
    #[error("OAuth callback returned error for `{server_id}`: {error}{}",
        .description.as_deref().map(|d| format!(" ({d})")).unwrap_or_default())]
    Callback {
        server_id: String,
        error: String,
        description: Option<String>,
    },
    #[error("OAuth flow cancelled for `{server_id}`")]
    Cancelled { server_id: String },
    #[error("OAuth I/O error for `{server_id}`: {message}")]
    Io { server_id: String, message: String },
}

/// Outcome of a successful resolve: the authenticated HTTP client to hand
/// to rmcp's streamable-HTTP transport, plus the underlying credential
/// store (kept alive so the auth manager's persistence path keeps writing
/// to the same on-disk file across refreshes).
pub struct ResolvedAuth {
    pub client: AuthClient<HttpClient>,
    pub _store: Arc<FileCredentialStore>,
    pub auth_manager: Arc<Mutex<AuthorizationManager>>,
}

/// Stateless façade — every call rebuilds the underlying
/// `AuthorizationManager` from disk. Cheap because there's no live
/// network state to keep alive between calls; the per-server file is the
/// source of truth.
pub struct OAuthService {
    pub config: OAuthConfig,
}

impl OAuthService {
    pub fn new(config: OAuthConfig) -> Self {
        Self { config }
    }

    /// Returns true when a stored token exists for this server (regardless
    /// of expiry). Used by the CLI to decide whether `puffer mcp login`
    /// has anything to display ("logged in" vs "not logged in").
    pub fn has_stored_tokens(&self) -> bool {
        let store = self.build_store();
        store.read_persisted().is_some()
    }

    fn build_store(&self) -> FileCredentialStore {
        let store = FileCredentialStore::new(
            self.config.token_dir.clone(),
            &self.config.server_id,
            &self.config.server_url,
        );
        store.prime_from_disk();
        store
    }

    /// Build an [`AuthorizationManager`] pre-loaded with the persisted
    /// credential store, returning `Some` only if there's actually a
    /// client_id on disk to configure with. Used by both `resolve` and
    /// `interactive_login`.
    async fn build_manager(
        &self,
        store: Arc<FileCredentialStore>,
    ) -> Result<AuthorizationManager, OAuthError> {
        let mut manager = AuthorizationManager::new(self.config.server_url.clone())
            .await
            .map_err(|e| OAuthError::Discovery {
                server_id: self.config.server_id.clone(),
                source: e,
            })?;
        manager.set_credential_store_arc(store);
        Ok(manager)
    }

    /// Force a token refresh ignoring the expiry skew window. Used by
    /// the connection manager's reactive 401 path: when an MCP server
    /// returns 401 even though our local clock thinks the access token
    /// is still valid (server invalidated it early, e.g. revocation,
    /// clock skew, or scope change), we force a refresh and respawn the
    /// transport with the new token.
    ///
    /// Returns `OAuthRequired` when there's nothing on disk to refresh
    /// from (or when the refresh attempt itself fails) — the caller
    /// surfaces that as `RunnerError::OAuthRequired` so `puffer-cli` can
    /// drive a fresh interactive login.
    pub async fn force_refresh(&self) -> Result<(), OAuthError> {
        let store = Arc::new(self.build_store());
        let mut manager = self.build_manager(Arc::clone(&store)).await?;
        let stored = store.read_persisted();
        let Some(persisted) = stored else {
            return Err(OAuthError::OAuthRequired {
                server_id: self.config.server_id.clone(),
                authorization_url: None,
            });
        };
        let metadata = manager
            .discover_metadata()
            .await
            .map_err(|e| OAuthError::Discovery {
                server_id: self.config.server_id.clone(),
                source: e,
            })?;
        manager.set_metadata(metadata);
        let client_secret = store.cached_client_secret().await;
        manager
            .configure_client(OAuthClientConfig {
                client_id: persisted.client_id.clone(),
                client_secret,
                scopes: self.config.scopes.clone(),
                redirect_uri: format!("http://127.0.0.1:0/callback"),
            })
            .map_err(|e| OAuthError::Registration {
                server_id: self.config.server_id.clone(),
                source: e,
            })?;
        if let Err(e) = manager.refresh_token().await {
            tracing::warn!(
                target = "puffer::mcp::oauth",
                "force_refresh failed for {}: {e}",
                self.config.server_id
            );
            return Err(OAuthError::OAuthRequired {
                server_id: self.config.server_id.clone(),
                authorization_url: None,
            });
        }
        Ok(())
    }

    /// Silent path. Returns an `AuthClient` ready to drop into the
    /// streamable-HTTP transport, refreshing the token first if it's
    /// within the expiry skew window. Returns `OAuthRequired` when no
    /// usable credentials are on disk.
    pub async fn resolve(&self) -> Result<ResolvedAuth, OAuthError> {
        let store = Arc::new(self.build_store());
        let mut manager = self.build_manager(Arc::clone(&store)).await?;

        // Bail early if there's nothing on disk to authenticate with.
        let stored = store.read_persisted();
        let Some(persisted) = stored else {
            return Err(OAuthError::OAuthRequired {
                server_id: self.config.server_id.clone(),
                authorization_url: None,
            });
        };

        // Discover metadata once so refresh has the token endpoint.
        let metadata = manager
            .discover_metadata()
            .await
            .map_err(|e| OAuthError::Discovery {
                server_id: self.config.server_id.clone(),
                source: e,
            })?;
        manager.set_metadata(metadata);

        let client_secret = store.cached_client_secret().await;
        manager
            .configure_client(OAuthClientConfig {
                client_id: persisted.client_id.clone(),
                client_secret,
                scopes: self.config.scopes.clone(),
                redirect_uri: format!("http://127.0.0.1:0/callback"),
            })
            .map_err(|e| OAuthError::Registration {
                server_id: self.config.server_id.clone(),
                source: e,
            })?;

        // Force a refresh if the token is within REFRESH_SKEW or already
        // expired. rmcp's `get_access_token` performs the same check; we
        // do it explicitly here so refresh failures surface as a typed
        // `OAuthRequired` rather than leak through later as a 401 on the
        // first MCP request.
        if needs_refresh(persisted.expires_at_ms) {
            if let Err(e) = manager.refresh_token().await {
                tracing::warn!(
                    target = "puffer::mcp::oauth",
                    "refresh_token failed for {}: {e}",
                    self.config.server_id
                );
                return Err(OAuthError::OAuthRequired {
                    server_id: self.config.server_id.clone(),
                    authorization_url: None,
                });
            }
        }

        let client = AuthClient::new(self.build_http_client(), manager);
        let auth_manager = Arc::clone(&client.auth_manager);
        Ok(ResolvedAuth {
            client,
            _store: store,
            auth_manager,
        })
    }

    /// Interactive flow: discovery → DCR (or reuse stored client_id) →
    /// browser → localhost callback → token exchange → persist.
    ///
    /// `open_browser`: invoked with the authorization URL. Default impl
    /// uses `webbrowser::open`. Tests provide a closure that programmatically
    /// hits the URL to drive the auto-redirect.
    pub async fn interactive_login(
        &self,
        cancel: Option<CancellationToken>,
        open_browser: impl FnOnce(&str) -> std::io::Result<()> + Send + 'static,
    ) -> Result<(), OAuthError> {
        let store = Arc::new(self.build_store());
        let cancel = cancel.unwrap_or_default();

        // Bind the local callback BEFORE starting the OAuth flow so
        // `redirect_uri` is known at session-creation time.
        let callback = spawn_callback_server().await.map_err(|e| OAuthError::Io {
            server_id: self.config.server_id.clone(),
            message: format!("bind callback server: {e}"),
        })?;

        // Drive discovery + DCR + URL via OAuthState so we don't have to
        // hand-roll the state machine.
        let mut state = OAuthState::new(
            self.config.server_url.clone(),
            Some(self.build_http_client()),
        )
        .await
        .map_err(|e| OAuthError::Discovery {
            server_id: self.config.server_id.clone(),
            source: e,
        })?;
        // Replace the in-memory credential store with our file-backed one
        // so the eventual `exchange_code_for_token` save path lands on
        // disk. OAuthState exposes the manager only on `Authorized`, so
        // we stash a clone of the store separately (see `flush_after_session`).
        let scopes_owned: Vec<String> = self.config.scopes.clone();
        let scopes_borrowed: Vec<&str> = scopes_owned.iter().map(|s| s.as_str()).collect();
        // Override the OAuthState's manager's credential store with ours.
        // OAuthState doesn't expose this directly; replace it ourselves.
        state = override_state_credential_store(state, Arc::clone(&store));

        state
            .start_authorization(
                &scopes_borrowed,
                &callback.redirect_uri,
                Some(&self.config.client_name),
            )
            .await
            .map_err(|e| OAuthError::Registration {
                server_id: self.config.server_id.clone(),
                source: e,
            })?;

        let auth_url =
            state
                .get_authorization_url()
                .await
                .map_err(|e| OAuthError::Registration {
                    server_id: self.config.server_id.clone(),
                    source: e,
                })?;

        // Best-effort: open the URL. If the spawn fails we still print the
        // URL so the user can copy-paste it.
        eprintln!("[puffer mcp oauth] open this URL to sign in: {}", auth_url);
        if let Err(e) = open_browser(&auth_url) {
            tracing::warn!(
                target = "puffer::mcp::oauth",
                "failed to launch browser: {e}; please open the URL manually"
            );
        }

        // Wait for callback or cancellation.
        let params = wait_for_callback(callback, cancel.clone()).await?;
        if let Some(error) = params.error.clone() {
            return Err(OAuthError::Callback {
                server_id: self.config.server_id.clone(),
                error,
                description: params.error_description,
            });
        }
        let code = params.code.ok_or_else(|| OAuthError::Callback {
            server_id: self.config.server_id.clone(),
            error: "missing_code".to_string(),
            description: None,
        })?;
        let csrf = params.state.ok_or_else(|| OAuthError::Callback {
            server_id: self.config.server_id.clone(),
            error: "missing_state".to_string(),
            description: None,
        })?;

        state
            .handle_callback(&code, &csrf)
            .await
            .map_err(|e| OAuthError::TokenExchange {
                server_id: self.config.server_id.clone(),
                source: e,
            })?;

        // After token exchange, capture the dynamically-registered
        // client_secret (if any) so the silent-resolve path can use it on
        // refresh. OAuthState doesn't expose configure-time inputs, so we
        // fish it out of the underlying manager via `into_authorization_manager`.
        let manager =
            state
                .into_authorization_manager()
                .ok_or_else(|| OAuthError::TokenExchange {
                    server_id: self.config.server_id.clone(),
                    source: AuthError::InternalError("not in authorized state".into()),
                })?;
        let secret = client_secret_of(&manager);
        if secret.is_some() {
            store.set_client_secret(secret).await;
            // Trigger a no-op save to flush the secret to disk.
            // The `CredentialStore::save` path walks `cache` -> file; the
            // cache was populated by rmcp's `exchange_code_for_token`.
            // Re-save explicitly so `client_secret` lands.
            if let Some(snapshot) = manager.get_credentials().await.ok() {
                let stored = rmcp::transport::auth::StoredCredentials {
                    client_id: snapshot.0,
                    token_response: snapshot.1,
                };
                use rmcp::transport::auth::CredentialStore;
                if let Err(e) = store.save(stored).await {
                    tracing::warn!(
                        target = "puffer::mcp::oauth",
                        "failed to flush client_secret for {}: {e}",
                        self.config.server_id
                    );
                }
            }
        }

        Ok(())
    }

    fn build_http_client(&self) -> HttpClient {
        HttpClient::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| HttpClient::new())
    }
}

const REFRESH_SKEW: Duration = Duration::from_secs(60);

fn needs_refresh(expires_at_ms: Option<u64>) -> bool {
    let Some(expires_at_ms) = expires_at_ms else {
        // No expiry recorded — assume long-lived; rmcp will refresh on
        // demand from a 401.
        return false;
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis() as u64;
    now_ms.saturating_add(REFRESH_SKEW.as_millis() as u64) >= expires_at_ms
}

async fn wait_for_callback(
    mut handle: CallbackHandle,
    cancel: CancellationToken,
) -> Result<CallbackParams, OAuthError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(OAuthError::Cancelled {
            server_id: String::new(),
        }),
        params = &mut handle.callback => {
            params.map_err(|_| OAuthError::Cancelled {
                server_id: String::new(),
            })
        }
    }
}

/// Replace the in-memory credential store backing an `OAuthState`'s
/// internal manager with our file-backed one. rmcp's public API only
/// exposes the setter on `AuthorizationManager`, and `OAuthState`
/// hides the manager behind its enum — so we destructure-rebuild here.
fn override_state_credential_store(
    state: OAuthState,
    store: Arc<FileCredentialStore>,
) -> OAuthState {
    match state {
        OAuthState::Unauthorized(mut m) => {
            m.set_credential_store_arc(store);
            OAuthState::Unauthorized(m)
        }
        other => other,
    }
}

/// Retrieve the configured client_secret from a (possibly already
/// authenticated) `AuthorizationManager`. rmcp doesn't expose a getter
/// for this, so we fall back to whatever the credential store has — the
/// store is the source of truth after any persisted save.
fn client_secret_of(_manager: &AuthorizationManager) -> Option<String> {
    // rmcp 0.15's `AuthorizationManager` doesn't expose a public
    // `client_secret` getter. The DCR response carried it through into
    // the `OAuthClient`, but the wire didn't preserve it for us. For
    // public clients (token_endpoint_auth_method=none, the default DCR
    // request rmcp sends) this is correct — return None.
    //
    // If we ever switch to confidential clients, we'd need to either
    // patch rmcp to expose the secret or stop driving DCR through
    // `OAuthState` and call `register_client` directly so we can keep
    // the response.
    None
}

/// Trait extension: rmcp 0.15's `AuthorizationManager::set_credential_store`
/// is generic over `S: CredentialStore + 'static` and consumes the value.
/// We have an `Arc<FileCredentialStore>` and want to keep it shared with
/// the rest of the service. Wrap it in an adapter that defers to the Arc.
mod credential_store_arc_adapter {
    use super::FileCredentialStore;
    use async_trait::async_trait;
    use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};
    use std::sync::Arc;

    #[derive(Clone)]
    pub struct ArcStoreAdapter(pub Arc<FileCredentialStore>);

    #[async_trait]
    impl CredentialStore for ArcStoreAdapter {
        async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
            self.0.load().await
        }

        async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
            self.0.save(credentials).await
        }

        async fn clear(&self) -> Result<(), AuthError> {
            self.0.clear().await
        }
    }
}

trait AuthorizationManagerExt {
    fn set_credential_store_arc(&mut self, store: Arc<FileCredentialStore>);
}

impl AuthorizationManagerExt for AuthorizationManager {
    fn set_credential_store_arc(&mut self, store: Arc<FileCredentialStore>) {
        self.set_credential_store(credential_store_arc_adapter::ArcStoreAdapter(store));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_refresh_handles_missing_expiry() {
        assert!(!needs_refresh(None));
    }

    #[test]
    fn needs_refresh_returns_true_when_expired() {
        assert!(needs_refresh(Some(1)));
    }

    #[test]
    fn needs_refresh_returns_false_for_far_future() {
        let far = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 3_600_000;
        assert!(!needs_refresh(Some(far)));
    }
}
