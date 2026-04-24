use crate::runtime::ConnectorRuntime;
use std::sync::Arc;
use thiserror::Error;

/// Errors returned when spawning a connector.
#[derive(Debug, Error)]
pub enum ConnectorStartError {
    #[error("connector `{id}` is not enabled in config")]
    Disabled { id: String },
    #[error("connector `{id}` is missing required configuration: {detail}")]
    MissingConfig { id: String, detail: String },
    #[error("connector `{id}` failed to start: {source}")]
    Other {
        id: String,
        #[source]
        source: anyhow::Error,
    },
}

impl ConnectorStartError {
    pub fn other(id: impl Into<String>, source: anyhow::Error) -> Self {
        Self::Other {
            id: id.into(),
            source,
        }
    }
}

/// Handle returned by [`Connector::start`] that owns the background
/// thread driving the platform client and exposes a cooperative shutdown
/// signal.
pub struct ConnectorHandle {
    /// Stable platform id (e.g. `"telegram"`); matches
    /// [`ConversationKey::platform`](crate::ConversationKey::platform).
    pub id: String,
    /// Fires a shutdown signal; the background thread observes it and
    /// unwinds. Call once.
    pub shutdown: Box<dyn FnOnce() + Send>,
    /// Handle to the background thread. Joining after `shutdown` returns
    /// the thread's final result.
    pub join: std::thread::JoinHandle<anyhow::Result<()>>,
}

/// Implemented by every platform-specific connector crate.
///
/// Connectors are started once at Puffer startup and shut down during
/// runtime teardown. They hold on to the shared [`ConnectorRuntime`] so
/// every inbound platform message can be dispatched through a single
/// bridge.
pub trait Connector: Send + 'static {
    /// Stable platform id used for session-map keys and diagnostics.
    fn id(&self) -> &str;

    /// Spawns the platform listener. Implementations typically start a
    /// std::thread::spawn that owns a Tokio runtime for the platform SDK
    /// and blocks on the listener loop until `shutdown` fires.
    fn start(
        self: Box<Self>,
        runtime: Arc<ConnectorRuntime>,
    ) -> Result<ConnectorHandle, ConnectorStartError>;
}
