use crate::handler::PLATFORM_ID;
use crate::router::{build_router, AppState};
use crate::WebhookConfig;
use anyhow::{Context, Result};
use puffer_connector_core::{
    Connector, ConnectorHandle, ConnectorRuntime, ConnectorStartError,
};
use std::sync::mpsc;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Webhook connector ready to be started by the puffer connector hub.
pub struct WebhookConnector {
    config: WebhookConfig,
}

impl WebhookConnector {
    pub fn new(config: WebhookConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &WebhookConfig {
        &self.config
    }
}

impl Connector for WebhookConnector {
    fn id(&self) -> &str {
        PLATFORM_ID
    }

    fn start(
        self: Box<Self>,
        runtime: Arc<ConnectorRuntime>,
    ) -> Result<ConnectorHandle, ConnectorStartError> {
        if self.config.bind_address.trim().is_empty() {
            return Err(ConnectorStartError::MissingConfig {
                id: PLATFORM_ID.to_string(),
                detail: "bind_address is empty".to_string(),
            });
        }

        // One-shot channel that fires axum's graceful shutdown.
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        // Signal back from the worker thread once the Tokio runtime is
        // ready to be interrupted.
        let (ready_tx, ready_rx) = mpsc::channel::<()>();

        let config = self.config.clone();
        let join = std::thread::Builder::new()
            .name("puffer-connector-webhook".to_string())
            .spawn(move || -> Result<()> {
                let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .context("failed to build tokio runtime for webhook connector")?;
                let _ = ready_tx.send(());
                tokio_runtime.block_on(run_server(config, runtime, shutdown_rx))
            })
            .map_err(|error| ConnectorStartError::other(PLATFORM_ID, error.into()))?;

        ready_rx
            .recv()
            .map_err(|error| ConnectorStartError::other(PLATFORM_ID, error.into()))?;

        let shutdown: Box<dyn FnOnce() + Send> = Box::new(move || {
            let _ = shutdown_tx.send(());
        });

        Ok(ConnectorHandle {
            id: PLATFORM_ID.to_string(),
            shutdown,
            join,
        })
    }
}

async fn run_server(
    config: WebhookConfig,
    runtime: Arc<ConnectorRuntime>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let state = AppState {
        runtime,
        config: Arc::new(config.clone()),
    };
    let router = build_router(state);

    let listener = tokio::net::TcpListener::bind(&config.bind_address)
        .await
        .with_context(|| {
            format!(
                "failed to bind webhook connector to {}",
                config.bind_address
            )
        })?;

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await
        .context("webhook connector axum server error")?;
    Ok(())
}
