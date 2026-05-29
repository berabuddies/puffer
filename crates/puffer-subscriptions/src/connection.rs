use crate::catalog::ConnectorSlug;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

/// Stable user-facing slug for an authorized connector instance.
pub type ConnectionSlug = String;

/// Lifecycle state for a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    /// Connection record exists but auth has not started.
    Created,
    /// Authentication is currently waiting on user or provider input.
    Authenticating,
    /// Auth succeeded but no consumer is currently using the stream.
    Authenticated,
    /// At least one workflow, connector, or agent proxy consumer is active.
    Active,
    /// Auth or delivery is failing; the user should repair the connection.
    Degraded,
    /// Connection is intentionally disabled and cannot run.
    Disabled,
}

/// One authorized connector instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionRecord {
    /// Stable user-facing connection slug.
    pub slug: ConnectionSlug,
    /// Connector template slug this connection uses.
    pub connector_slug: ConnectorSlug,
    /// User-facing description.
    #[serde(default)]
    pub description: String,
    /// Current lifecycle state.
    pub state: ConnectionState,
    /// Whether a workflow or agent proxy is currently consuming the stream.
    #[serde(default)]
    pub has_consumer: bool,
    /// Last connector cursor acknowledged by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Whether Puffer has already surfaced the one-time broken-auth notice.
    #[serde(default)]
    pub auth_failure_notified: bool,
}

impl ConnectionRecord {
    /// Creates an authenticated but inactive connection record.
    pub fn authenticated(
        slug: impl Into<String>,
        connector_slug: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            slug: slug.into(),
            connector_slug: connector_slug.into(),
            description: description.into(),
            state: ConnectionState::Authenticated,
            has_consumer: false,
            cursor: None,
            auth_failure_notified: false,
        }
    }

    /// Returns whether a stream should be running for this connection.
    pub fn should_stream(&self) -> bool {
        self.has_consumer
            && matches!(
                self.state,
                ConnectionState::Authenticated | ConnectionState::Active
            )
    }

    /// Updates `state` when consumer count transitions across zero.
    pub fn set_has_consumer(&mut self, has_consumer: bool) {
        self.has_consumer = has_consumer;
        self.state = match (has_consumer, self.state) {
            (
                _,
                ConnectionState::Disabled
                | ConnectionState::Degraded
                | ConnectionState::Created
                | ConnectionState::Authenticating,
            ) => self.state,
            (true, ConnectionState::Authenticated) => ConnectionState::Active,
            (false, ConnectionState::Active) => ConnectionState::Authenticated,
            _ => self.state,
        };
    }
}

/// Errors returned by [`ConnectionStore`].
#[derive(Debug, Error)]
pub enum ConnectionStoreError {
    /// I/O failed while reading or writing state.
    #[error("connection store io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON failed to parse or encode.
    #[error("connection store json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Connection input is invalid.
    #[error("invalid connection: {0}")]
    Invalid(String),
    /// Connection was not found.
    #[error("connection `{0}` not found")]
    NotFound(String),
    /// Connection already exists.
    #[error("connection `{0}` already exists")]
    Conflict(String),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConnectionStoreFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    connections: Vec<ConnectionRecord>,
}

/// File-backed store for authorized connections.
pub struct ConnectionStore {
    path: PathBuf,
    inner: Mutex<ConnectionStoreFile>,
}

impl ConnectionStore {
    /// Loads a connection store. Missing files are treated as empty.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, ConnectionStoreError> {
        let path = path.into();
        let inner = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            if raw.trim().is_empty() {
                ConnectionStoreFile::default()
            } else {
                serde_json::from_str(&raw)?
            }
        } else {
            ConnectionStoreFile::default()
        };
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    /// Returns all connection records sorted by slug.
    pub fn list(&self) -> Vec<ConnectionRecord> {
        let mut list = self.inner.lock().unwrap().connections.clone();
        list.sort_by(|a, b| a.slug.cmp(&b.slug));
        list
    }

    /// Returns one connection by slug.
    pub fn get(&self, slug: &str) -> Option<ConnectionRecord> {
        self.inner
            .lock()
            .unwrap()
            .connections
            .iter()
            .find(|connection| connection.slug == slug)
            .cloned()
    }

    /// Creates a connection record.
    pub fn create(&self, connection: ConnectionRecord) -> Result<(), ConnectionStoreError> {
        validate_connection(&connection)?;
        let mut guard = self.inner.lock().unwrap();
        if guard
            .connections
            .iter()
            .any(|existing| existing.slug == connection.slug)
        {
            return Err(ConnectionStoreError::Conflict(connection.slug));
        }
        guard.connections.push(connection);
        write_atomic(&self.path, &*guard)
    }

    /// Deletes a connection by slug.
    pub fn delete(&self, slug: &str) -> Result<(), ConnectionStoreError> {
        let mut guard = self.inner.lock().unwrap();
        let before = guard.connections.len();
        guard
            .connections
            .retain(|connection| connection.slug != slug);
        if guard.connections.len() == before {
            return Err(ConnectionStoreError::NotFound(slug.to_string()));
        }
        write_atomic(&self.path, &*guard)
    }

    /// Updates a connection with a caller-supplied mutation.
    pub fn update<F>(&self, slug: &str, mutate: F) -> Result<ConnectionRecord, ConnectionStoreError>
    where
        F: FnOnce(&mut ConnectionRecord),
    {
        let mut guard = self.inner.lock().unwrap();
        let connection = guard
            .connections
            .iter_mut()
            .find(|connection| connection.slug == slug)
            .ok_or_else(|| ConnectionStoreError::NotFound(slug.to_string()))?;
        mutate(connection);
        validate_connection(connection)?;
        let updated = connection.clone();
        write_atomic(&self.path, &*guard)?;
        Ok(updated)
    }
}

fn validate_connection(connection: &ConnectionRecord) -> Result<(), ConnectionStoreError> {
    validate_slug("connection slug", &connection.slug)?;
    validate_slug("connector slug", &connection.connector_slug)?;
    Ok(())
}

fn validate_slug(label: &str, slug: &str) -> Result<(), ConnectionStoreError> {
    if slug.is_empty()
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(ConnectionStoreError::Invalid(format!(
            "{label} must be non-empty kebab-case ASCII"
        )));
    }
    Ok(())
}

fn write_atomic(path: &Path, store: &ConnectionStoreFile) -> Result<(), ConnectionStoreError> {
    let tmp = path.with_extension("tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tmp, serde_json::to_vec_pretty(store)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_store_roundtrips() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConnectionStore::load(temp.path().join("connections.json")).unwrap();
        store
            .create(ConnectionRecord::authenticated(
                "my-telegram",
                "telegram-login",
                "demo",
            ))
            .unwrap();

        let reopened = ConnectionStore::load(temp.path().join("connections.json")).unwrap();
        let connection = reopened.get("my-telegram").unwrap();
        assert_eq!(connection.connector_slug, "telegram-login");
        assert_eq!(connection.state, ConnectionState::Authenticated);
    }

    #[test]
    fn consumer_transitions_active_without_starting_at_auth() {
        let mut connection =
            ConnectionRecord::authenticated("my-telegram", "telegram-login", "demo");

        assert_eq!(connection.state, ConnectionState::Authenticated);
        assert!(!connection.should_stream());

        connection.set_has_consumer(true);
        assert_eq!(connection.state, ConnectionState::Active);
        assert!(connection.should_stream());

        connection.set_has_consumer(false);
        assert_eq!(connection.state, ConnectionState::Authenticated);
        assert!(!connection.should_stream());
    }
}
