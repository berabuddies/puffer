//! OAuth 2.0 client support for puffer's HTTP MCP transport.
//!
//! Pass-1.5e of the puffer MCP work — covers the slice of RFC 6749 +
//! 6750 + 7591 + 7636 + 8414 + 9728 needed to talk to the
//! registry-distributed MCP servers (GitHub, Cloudflare, Atlassian, etc.)
//! without the operator pasting a long-lived bearer token into the
//! manifest.
//!
//! The heavy lifting (PKCE generation, metadata discovery, dynamic
//! client registration, token refresh, [`AuthClient`] wrapping for the
//! streamable-HTTP transport) is delegated to rmcp 0.15's
//! [`rmcp::transport::auth`] machinery. This crate adds the bits rmcp
//! does not ship out-of-the-box:
//!
//! 1. **File-backed credential store** ([`store::FileCredentialStore`])
//!    that persists tokens under `<user-config>/mcp-tokens/<server>.json`
//!    with `0600` perms on Unix, surviving runner restarts.
//! 2. **Local callback receiver** ([`callback::spawn_callback_server`])
//!    — a tiny axum server bound to a free loopback port that captures
//!    the authorization-code redirect.
//! 3. **High-level orchestration** ([`service::OAuthService`]) that
//!    wires (1) and (2) together and exposes the silent /
//!    [`OAuthService::resolve`] vs interactive /
//!    [`OAuthService::interactive_login`] paths the runner asks for.
//!
//! ## Out-of-scope (deferred to future passes)
//!
//! * Device flow (RFC 8628). Useful for headless multi-tenant deployments;
//!   document in pickup notes.
//! * OS-keychain credential storage. Plain JSON with `0600` perms is
//!   acceptable for v1.
//! * Confidential clients (DCR with token_endpoint_auth_method=client_secret_*).
//!   The DCR rmcp drives uses `none` (public client + PKCE) which covers
//!   every MCP server we've seen in the registry to date.
//! * `client_credentials` grant — different flow shape, separate need.

pub mod callback;
pub mod service;
pub mod store;

pub use callback::{spawn_callback_server, CallbackHandle, CallbackParams};
pub use service::{OAuthConfig, OAuthError, OAuthService, ResolvedAuth};
pub use store::{store_key, FileCredentialStore, PersistedTokens};

/// Default token-store directory: `<user-config>/puffer/mcp-tokens`.
///
/// Falls back to the system temp dir on the (very rare) platforms where
/// `dirs::config_dir()` returns `None` so the caller doesn't have to
/// special-case the result. The fallback is a `warn!` log because
/// tokens persisted to /tmp evaporate on reboot.
pub fn default_token_dir() -> std::path::PathBuf {
    if let Some(base) = dirs::config_dir() {
        return base.join("puffer").join("mcp-tokens");
    }
    tracing::warn!(
        target = "puffer::mcp::oauth",
        "no user config dir found; persisting MCP OAuth tokens under temp dir"
    );
    std::env::temp_dir().join("puffer-mcp-tokens")
}
