//! Axum-mounted streamable-HTTP wrapper around `StubServer`.
//!
//! Used by the HTTP integration tests to spin up a real rmcp HTTP server
//! on a free loopback port; the test then points a puffer
//! `LocalToolRunner` at the URL and exercises tools/list, tools/call,
//! resources, prompts, progress, and the optional Authorization-header
//! gate.
//!
//! Single endpoint: `POST/GET/DELETE http://127.0.0.1:<port>/mcp` per
//! rmcp's streamable-HTTP convention. If `auth_token` is `Some`, every
//! POST that doesn't carry `Authorization: Bearer <token>` is rejected
//! with 401 — lets tests assert that user-supplied headers actually
//! reach the wire.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{self, Next},
    response::Response,
    Router,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, tower::StreamableHttpService, StreamableHttpServerConfig,
};
use tokio_util::sync::CancellationToken;

use super::stub_server::StubServer;

/// A live HTTP MCP stub plus the addresses / shutdown handle the test
/// needs to talk to it and tear it down.
pub struct HttpStubHandle {
    pub addr: SocketAddr,
    cancel: CancellationToken,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl HttpStubHandle {
    /// Full `http://host:port/mcp` URL, ready for an `McpServerSpec`'s
    /// `target` field.
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    /// Cancel the server's accept loop and wait for it to drain. Called
    /// implicitly by `Drop`; tests can call it explicitly when they want
    /// to assert that subsequent client calls fail in a controlled way.
    pub fn shutdown(&mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.join.take() {
            // The accept loop returns once the cancellation token fires.
            // Block on the join from a fresh runtime so this works from
            // any context (including sync `Drop`).
            tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("shutdown runtime");
                let _ = rt.block_on(async {
                    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
                });
            });
        }
    }
}

impl Drop for HttpStubHandle {
    fn drop(&mut self) {
        if self.join.is_some() {
            self.shutdown();
        }
    }
}

/// Spawn the stub on a free loopback port. Returns once the listener is
/// bound and the accept loop is running, so tests can immediately point
/// a client at `handle.url()`.
///
/// `auth_token`: when `Some(token)`, the server enforces
/// `Authorization: Bearer <token>` on every POST and returns 401 otherwise.
pub async fn spawn_http_stub(auth_token: Option<&str>) -> anyhow::Result<HttpStubHandle> {
    let cancel = CancellationToken::new();
    let service: StreamableHttpService<StubServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(StubServer),
            Default::default(),
            StreamableHttpServerConfig {
                stateful_mode: true,
                sse_keep_alive: Some(Duration::from_secs(5)),
                cancellation_token: cancel.child_token(),
                ..Default::default()
            },
        );

    let mut router = Router::new().nest_service("/mcp", service);
    if let Some(token) = auth_token {
        let auth_state = Arc::new(format!("Bearer {token}"));
        router = router.layer(middleware::from_fn_with_state(
            auth_state,
            require_auth_header,
        ));
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let cancel_for_shutdown = cancel.clone();
    let join = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { cancel_for_shutdown.cancelled_owned().await })
            .await;
    });

    Ok(HttpStubHandle {
        addr,
        cancel,
        join: Some(join),
    })
}

/// Reject any request whose `Authorization` header doesn't match the
/// expected `Bearer <token>` value. Lets tests prove the user-supplied
/// `headers` map actually reaches the wire.
async fn require_auth_header(
    State(expected): State<Arc<String>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok());
    match header {
        Some(value) if value == expected.as_str() => Ok(next.run(request).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Variant of [`spawn_http_stub`] that accepts every `Authorization:
/// Bearer <token>` value matching `prefix`, used by the OAuth e2e test
/// to whitelist any access token issued by the in-tree OAuth stub
/// (which mints `stub-access-N`).
pub async fn spawn_http_stub_accepting_bearer_prefix(
    prefix: &'static str,
) -> anyhow::Result<HttpStubHandle> {
    let cancel = CancellationToken::new();
    let service: StreamableHttpService<StubServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(StubServer),
            Default::default(),
            StreamableHttpServerConfig {
                stateful_mode: true,
                sse_keep_alive: Some(Duration::from_secs(5)),
                cancellation_token: cancel.child_token(),
                ..Default::default()
            },
        );

    let router = Router::new().nest_service("/mcp", service).layer(
        middleware::from_fn_with_state(prefix, require_bearer_prefix),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;

    let cancel_for_shutdown = cancel.clone();
    let join = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { cancel_for_shutdown.cancelled_owned().await })
            .await;
    });
    Ok(HttpStubHandle {
        addr,
        cancel,
        join: Some(join),
    })
}

async fn require_bearer_prefix(
    State(prefix): State<&'static str>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok());
    match header {
        Some(value) if value.starts_with("Bearer ") && value[7..].starts_with(prefix) => {
            Ok(next.run(request).await)
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}
