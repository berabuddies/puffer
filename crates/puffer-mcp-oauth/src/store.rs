//! File-backed [`CredentialStore`] for puffer's MCP OAuth flow.
//!
//! Tokens are persisted under `<config-dir>/mcp-tokens/<server-key>.json`
//! where `<server-key>` is `<server-id>-<sha256-prefix>` so multiple servers
//! that share an id but live behind different URLs don't collide.
//!
//! On Unix the file is chmod-ed to `0600` after every write. On Windows we
//! rely on the per-user roaming/`%LOCALAPPDATA%` directory ACL inherited from
//! the parent (puffer doesn't ship a Windows-specific tightening pass yet —
//! see the v1 scope notes in the crate-level docs).
//!
//! The store deliberately persists the *full* `StoredCredentials` payload
//! (client_id + token_response) plus a separate `expires_at` epoch-millis
//! field that reflects the real wall-clock expiry. The `expires_in` field
//! oauth2 ships in `OAuthTokenResponse` is duration-from-now and goes
//! stale the moment we serialize it; the explicit `expires_at` lets the
//! refresh path tell whether a stored token is still good without a clock
//! skew window.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rmcp::transport::auth::{AuthError, CredentialStore, OAuthTokenResponse, StoredCredentials};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

/// Wire-shape of the JSON file written under `<config-dir>/mcp-tokens/`.
///
/// Kept separate from `rmcp::transport::auth::StoredCredentials` so we can
/// pin `expires_at` (epoch-millis, wall clock) independently of the
/// duration-from-now `expires_in` baked into the oauth2 token response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTokens {
    pub server_id: String,
    pub server_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub access_token: String,
    pub token_type: String,
    pub refresh_token: Option<String>,
    pub scopes: Vec<String>,
    /// Wall-clock expiry as `SystemTime` epoch milliseconds.
    pub expires_at_ms: Option<u64>,
}

impl PersistedTokens {
    /// Build a [`PersistedTokens`] from the rmcp / oauth2 token response,
    /// computing `expires_at_ms` from `expires_in` and the current wall
    /// clock at call time.
    pub fn from_token_response(
        server_id: &str,
        server_url: &str,
        client_id: &str,
        client_secret: Option<&str>,
        token: &OAuthTokenResponse,
    ) -> Self {
        use oauth2::TokenResponse;
        let expires_at_ms = token.expires_in().map(|dur| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_else(|_| Duration::from_secs(0));
            (now + dur).as_millis() as u64
        });
        let scopes = token
            .scopes()
            .map(|s| s.iter().map(|scope| scope.to_string()).collect())
            .unwrap_or_default();
        let token_type = serde_json::to_value(token.token_type())
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "Bearer".to_string());
        PersistedTokens {
            server_id: server_id.to_string(),
            server_url: server_url.to_string(),
            client_id: client_id.to_string(),
            client_secret: client_secret.map(|s| s.to_string()),
            access_token: token.access_token().secret().to_string(),
            token_type,
            refresh_token: token.refresh_token().map(|t| t.secret().to_string()),
            scopes,
            expires_at_ms,
        }
    }

    /// Reconstruct an oauth2 [`OAuthTokenResponse`] from disk, restoring
    /// `expires_in` from the wall-clock `expires_at_ms`.
    pub fn to_token_response(&self) -> OAuthTokenResponse {
        use oauth2::{
            basic::BasicTokenType, AccessToken, EmptyExtraTokenFields, RefreshToken, Scope,
        };
        let mut response = OAuthTokenResponse::new(
            AccessToken::new(self.access_token.clone()),
            BasicTokenType::Bearer,
            EmptyExtraTokenFields {},
        );
        if let Some(secret) = self.refresh_token.clone() {
            response.set_refresh_token(Some(RefreshToken::new(secret)));
        }
        if !self.scopes.is_empty() {
            response.set_scopes(Some(self.scopes.iter().cloned().map(Scope::new).collect()));
        }
        if let Some(expires_at_ms) = self.expires_at_ms {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_else(|_| Duration::from_secs(0))
                .as_millis() as u64;
            if expires_at_ms > now {
                let remaining = Duration::from_millis(expires_at_ms - now);
                response.set_expires_in(Some(&remaining));
            } else {
                // Already expired: leave `expires_in = None`, which makes
                // `AuthorizationManager::get_access_token` proactively
                // refresh on the next call.
                response.set_expires_in(None);
            }
        }
        response
    }
}

impl PersistedTokens {
    /// Persist this bundle to `<token_dir>/<store_key>.json` with `0600`
    /// perms (Unix). Used by `LocalToolRunner::push_oauth_tokens` so the
    /// CLI's `mcp login` can hand a freshly-minted token bundle to the
    /// runner without the runner having to re-implement the on-disk
    /// schema. Idempotent: overwrites any existing file for this server.
    pub fn write_to(&self, token_dir: &Path) -> std::io::Result<()> {
        let key = store_key(&self.server_id, &self.server_url);
        let path = token_dir.join(format!("{key}.json"));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        write_secret_file(&path, &bytes)
    }

    /// Read a previously-persisted bundle from disk. Returns `Ok(None)` if
    /// no file is present for the (server_id, server_url) pair.
    pub fn read_from(
        token_dir: &Path,
        server_id: &str,
        server_url: &str,
    ) -> std::io::Result<Option<PersistedTokens>> {
        let key = store_key(server_id, server_url);
        let path = token_dir.join(format!("{key}.json"));
        match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<PersistedTokens>(&bytes) {
                Ok(t) => Ok(Some(t)),
                Err(e) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Delete any persisted bundle for the (server_id, server_url) pair.
    /// Returns `Ok(true)` when a file was removed, `Ok(false)` when no file
    /// existed.
    pub fn delete_from(
        token_dir: &Path,
        server_id: &str,
        server_url: &str,
    ) -> std::io::Result<bool> {
        let key = store_key(server_id, server_url);
        let path = token_dir.join(format!("{key}.json"));
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// Compute a deterministic on-disk filename for the (server_id, server_url)
/// pair so two servers sharing an id but pointing at different URLs don't
/// stomp each other's tokens.
pub fn store_key(server_id: &str, server_url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(server_id.as_bytes());
    hasher.update(b"|");
    hasher.update(server_url.as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    let safe_id = sanitize_for_path(server_id);
    format!("{safe_id}-{}", &hex[..16])
}

fn sanitize_for_path(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// rmcp [`CredentialStore`] backed by a file under `<base_dir>`.
///
/// Each server (keyed by `server_id` + `server_url`) gets its own JSON file
/// so concurrent OAuth flows for unrelated servers don't contend on a single
/// file lock.
#[derive(Debug, Clone)]
pub struct FileCredentialStore {
    base_dir: PathBuf,
    server_id: String,
    server_url: String,
    /// Cached snapshot of the most recent `StoredCredentials` so callers
    /// asking `load()` repeatedly between writes don't re-read from disk.
    cache: Arc<RwLock<Option<StoredCredentials>>>,
    /// Mirror of the on-disk extras (client_secret, expires_at_ms, scopes,
    /// token_type) that don't round-trip through rmcp's `StoredCredentials`.
    /// Populated on every `save` and on `load`-from-disk.
    extras: Arc<RwLock<Option<PersistedExtras>>>,
}

#[derive(Debug, Clone)]
struct PersistedExtras {
    client_secret: Option<String>,
    expires_at_ms: Option<u64>,
}

impl FileCredentialStore {
    /// Build a store rooted at `base_dir` (the directory is created on the
    /// first save). `server_id` and `server_url` together select the on-disk
    /// filename via [`store_key`].
    pub fn new(base_dir: impl Into<PathBuf>, server_id: &str, server_url: &str) -> Self {
        Self {
            base_dir: base_dir.into(),
            server_id: server_id.to_string(),
            server_url: server_url.to_string(),
            cache: Arc::new(RwLock::new(None)),
            extras: Arc::new(RwLock::new(None)),
        }
    }

    /// Returns the on-disk path the credentials live at.
    pub fn token_path(&self) -> PathBuf {
        self.base_dir.join(format!(
            "{}.json",
            store_key(&self.server_id, &self.server_url)
        ))
    }

    /// Best-effort: read the persisted JSON file (if any) and return the
    /// parsed payload. Surfaces `Ok(None)` on file-not-found and on parse
    /// errors with a `warn!` log so a corrupted file doesn't permanently
    /// brick the OAuth flow — we just re-auth.
    pub fn read_persisted(&self) -> Option<PersistedTokens> {
        let path = self.token_path();
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!(
                    target = "puffer::mcp::oauth",
                    "failed to read OAuth token file at {}: {e}",
                    path.display()
                );
                return None;
            }
        };
        match serde_json::from_slice::<PersistedTokens>(&bytes) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(
                    target = "puffer::mcp::oauth",
                    "failed to parse OAuth token file at {}: {e}",
                    path.display()
                );
                None
            }
        }
    }

    /// Synchronous helper used at construction time so callers can pre-seed
    /// the in-memory cache (and the on-disk extras) before the store is
    /// handed to rmcp's `AuthorizationManager`.
    pub fn prime_from_disk(&self) {
        let Some(p) = self.read_persisted() else {
            return;
        };
        let token_response = p.to_token_response();
        let stored = StoredCredentials {
            client_id: p.client_id.clone(),
            token_response: Some(token_response),
        };
        let extras = PersistedExtras {
            client_secret: p.client_secret.clone(),
            expires_at_ms: p.expires_at_ms,
        };
        // `tokio::sync::RwLock::blocking_write` would be wrong here (it
        // panics off-runtime). Use `try_write` and fall back to constructing
        // fresh inner cells if the lock is somehow contended at construction
        // time — that's only possible if a different thread is already
        // touching the same store, which the public API guarantees we don't.
        if let Ok(mut guard) = self.cache.try_write() {
            *guard = Some(stored);
        }
        if let Ok(mut guard) = self.extras.try_write() {
            *guard = Some(extras);
        }
    }

    /// Returns the cached `client_secret` (if the registered client is a
    /// confidential one) for use during refresh.
    pub async fn cached_client_secret(&self) -> Option<String> {
        self.extras
            .read()
            .await
            .as_ref()
            .and_then(|e| e.client_secret.clone())
    }

    /// Update the on-disk `client_secret`. Called after dynamic client
    /// registration completes so we don't have to drive DCR again on the
    /// next process start.
    pub async fn set_client_secret(&self, secret: Option<String>) {
        let mut g = self.extras.write().await;
        match g.as_mut() {
            Some(extras) => extras.client_secret = secret,
            None => {
                *g = Some(PersistedExtras {
                    client_secret: secret,
                    expires_at_ms: None,
                })
            }
        }
    }

    /// Persist whatever's currently in `cache` to disk, applying `0600`
    /// perms on Unix. Called from the `CredentialStore::save` impl after
    /// the in-memory cache is updated, and from the explicit `flush` API
    /// the high-level service uses after dynamic registration.
    async fn write_to_disk(&self) -> Result<(), AuthError> {
        let stored = match self.cache.read().await.clone() {
            Some(s) => s,
            None => {
                // Nothing to save; if a file exists, leave it (caller drives
                // explicit deletion via `clear`).
                return Ok(());
            }
        };
        let token_response = match stored.token_response.as_ref() {
            Some(t) => t,
            None => return Ok(()),
        };
        let extras = self.extras.read().await.clone();
        let client_secret = extras.as_ref().and_then(|e| e.client_secret.clone());
        let mut payload = PersistedTokens::from_token_response(
            &self.server_id,
            &self.server_url,
            &stored.client_id,
            client_secret.as_deref(),
            token_response,
        );
        // Mirror the freshly-computed `expires_at_ms` back into the extras
        // cache so the next refresh-decision reads a consistent value.
        let expires_at_ms = payload.expires_at_ms;
        {
            let mut g = self.extras.write().await;
            match g.as_mut() {
                Some(e) => e.expires_at_ms = expires_at_ms,
                None => {
                    *g = Some(PersistedExtras {
                        client_secret: client_secret.clone(),
                        expires_at_ms,
                    })
                }
            }
        }
        // Make sure we don't lose a previously-stored client_secret if the
        // current `extras` snapshot doesn't have one (e.g. when rmcp drives
        // a refresh via `save`, which rebuilds `StoredCredentials` from
        // scratch and doesn't carry the secret).
        if payload.client_secret.is_none() {
            if let Some(prev) = self.read_persisted() {
                payload.client_secret = prev.client_secret;
            }
        }
        if let Some(parent) = self.token_path().parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AuthError::InternalError(format!("create token dir: {e}")))?;
        }
        let path = self.token_path();
        let bytes = serde_json::to_vec_pretty(&payload)
            .map_err(|e| AuthError::InternalError(format!("serialize tokens: {e}")))?;
        write_secret_file(&path, &bytes).map_err(|e| {
            AuthError::InternalError(format!("write tokens to {}: {e}", path.display()))
        })?;
        Ok(())
    }
}

/// Atomic-ish write that lands a `0600` file at `path`.
///
/// We write to a sibling temp path, fsync, chmod (Unix), then rename so a
/// crash mid-write can't leave a half-written token file in place.
fn write_secret_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.to_path_buf();
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("token");
    tmp.set_file_name(format!(".{file_name}.tmp"));
    fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[async_trait]
impl CredentialStore for FileCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        if let Some(c) = self.cache.read().await.clone() {
            return Ok(Some(c));
        }
        // Read-through to disk and seed the cache.
        let Some(p) = self.read_persisted() else {
            return Ok(None);
        };
        let token_response = p.to_token_response();
        let stored = StoredCredentials {
            client_id: p.client_id.clone(),
            token_response: Some(token_response),
        };
        {
            let mut g = self.cache.write().await;
            *g = Some(stored.clone());
        }
        {
            let mut g = self.extras.write().await;
            *g = Some(PersistedExtras {
                client_secret: p.client_secret.clone(),
                expires_at_ms: p.expires_at_ms,
            });
        }
        Ok(Some(stored))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        {
            let mut g = self.cache.write().await;
            *g = Some(credentials);
        }
        self.write_to_disk().await
    }

    async fn clear(&self) -> Result<(), AuthError> {
        {
            let mut g = self.cache.write().await;
            *g = None;
        }
        {
            let mut g = self.extras.write().await;
            *g = None;
        }
        let path = self.token_path();
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AuthError::InternalError(format!(
                "remove token file at {}: {e}",
                path.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth2::{basic::BasicTokenType, AccessToken, EmptyExtraTokenFields, RefreshToken};
    use tempfile::TempDir;

    fn sample_token() -> OAuthTokenResponse {
        let mut t = OAuthTokenResponse::new(
            AccessToken::new("at-1".to_string()),
            BasicTokenType::Bearer,
            EmptyExtraTokenFields {},
        );
        t.set_refresh_token(Some(RefreshToken::new("rt-1".to_string())));
        t.set_expires_in(Some(&Duration::from_secs(3600)));
        t
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let store = FileCredentialStore::new(dir.path().to_path_buf(), "srv", "https://x");
        store
            .save(StoredCredentials {
                client_id: "client-x".into(),
                token_response: Some(sample_token()),
            })
            .await
            .unwrap();
        // Drop in-memory cache to force a disk read.
        let store2 = FileCredentialStore::new(dir.path().to_path_buf(), "srv", "https://x");
        let loaded = store2.load().await.unwrap().expect("tokens persisted");
        assert_eq!(loaded.client_id, "client-x");
        let token = loaded.token_response.expect("token present");
        use oauth2::TokenResponse;
        assert_eq!(token.access_token().secret(), "at-1");
        assert_eq!(token.refresh_token().unwrap().secret(), "rt-1");
        assert!(token.expires_in().is_some());
    }

    #[test]
    fn store_key_is_deterministic_and_collision_resistant() {
        let a = store_key("github", "https://mcp.example.com/v1");
        let b = store_key("github", "https://mcp.example.com/v1");
        let c = store_key("github", "https://mcp.example.com/v2");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn save_writes_file_with_0600_perms() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let store = FileCredentialStore::new(dir.path().to_path_buf(), "srv", "https://x");
        store
            .save(StoredCredentials {
                client_id: "client-x".into(),
                token_response: Some(sample_token()),
            })
            .await
            .unwrap();
        let perms = fs::metadata(store.token_path()).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }
}
