//! One-shot localhost OAuth callback receiver.
//!
//! The interactive login flow opens an authorization URL in the user's
//! browser, which redirects back to `http://127.0.0.1:<port>/callback?code=...&state=...`
//! on success (or `?error=...` on failure). This module spins up a tiny
//! axum server bound to a free loopback port that captures the first
//! callback, returns a friendly HTML page to the browser, and resolves
//! a `oneshot` channel with the parsed query parameters so the calling
//! task can exchange the code for a token.
//!
//! The server shuts itself down as soon as a callback lands, or when the
//! caller drops the returned [`CallbackHandle`].

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

const SUCCESS_HTML: &str = r#"<!doctype html>
<html><head><title>puffer / MCP OAuth</title>
<style>body{font-family:-apple-system,BlinkMacSystemFont,sans-serif;margin:4em auto;max-width:32em;text-align:center;color:#222}
h1{font-size:1.6em}p{font-size:1em;line-height:1.6}</style></head>
<body><h1>Sign-in complete</h1>
<p>You can close this tab and return to the terminal.</p></body></html>
"#;

const FAILURE_HTML: &str = r#"<!doctype html>
<html><head><title>puffer / MCP OAuth</title>
<style>body{font-family:-apple-system,BlinkMacSystemFont,sans-serif;margin:4em auto;max-width:32em;text-align:center;color:#222}
h1{font-size:1.6em;color:#b00}p{font-size:1em;line-height:1.6}</style></head>
<body><h1>Sign-in failed</h1>
<p>The authorization server returned an error. Check the terminal for details.</p></body></html>
"#;

/// One callback's parsed query parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Handle to a running callback server. Drop to stop the listener.
pub struct CallbackHandle {
    pub addr: SocketAddr,
    /// `redirect_uri` to feed into the OAuth flow; kept here so the caller
    /// doesn't have to re-build it.
    pub redirect_uri: String,
    /// Resolves once the first `/callback` request lands.
    pub callback: oneshot::Receiver<CallbackParams>,
    cancel: CancellationToken,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for CallbackHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(j) = self.join.take() {
            j.abort();
        }
    }
}

#[derive(Clone)]
struct AppState {
    sender: Arc<Mutex<Option<oneshot::Sender<CallbackParams>>>>,
    cancel: CancellationToken,
}

async fn callback_handler(
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
) -> impl IntoResponse {
    let html = if params.error.is_some() {
        FAILURE_HTML
    } else {
        SUCCESS_HTML
    };
    let mut sender_slot = state.sender.lock().await;
    if let Some(tx) = sender_slot.take() {
        let _ = tx.send(params);
    }
    // Trigger graceful shutdown so the caller's `await` on `join` resolves
    // promptly after the browser tab closes.
    state.cancel.cancel();
    Html(html)
}

/// Bind a callback listener on a free loopback port, mounted at
/// `/callback`, and return a [`CallbackHandle`] whose `redirect_uri`
/// matches the bound socket.
pub async fn spawn_callback_server() -> anyhow::Result<CallbackHandle> {
    let cancel = CancellationToken::new();
    let (tx, rx) = oneshot::channel();
    let state = AppState {
        sender: Arc::new(Mutex::new(Some(tx))),
        cancel: cancel.clone(),
    };
    let app = Router::new()
        .route("/callback", get(callback_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let cancel_for_shutdown = cancel.clone();
    let join = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { cancel_for_shutdown.cancelled_owned().await })
            .await;
    });

    Ok(CallbackHandle {
        addr,
        redirect_uri: format!("http://{}/callback", addr),
        callback: rx,
        cancel,
        join: Some(join),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn callback_server_returns_query_params() {
        let mut handle = spawn_callback_server().await.unwrap();
        let url = format!("{}?code=abc123&state=xyz", handle.redirect_uri);
        // Hit the callback to satisfy the oneshot.
        let _ = reqwest::Client::new().get(&url).send().await.unwrap();
        let params = (&mut handle.callback).await.expect("callback");
        assert_eq!(params.code.as_deref(), Some("abc123"));
        assert_eq!(params.state.as_deref(), Some("xyz"));
        assert!(params.error.is_none());
    }

    #[tokio::test]
    async fn callback_server_surfaces_error() {
        let mut handle = spawn_callback_server().await.unwrap();
        let url = format!(
            "{}?error=access_denied&error_description=user+said+no",
            handle.redirect_uri
        );
        let _ = reqwest::Client::new().get(&url).send().await.unwrap();
        let params = (&mut handle.callback).await.expect("callback");
        assert_eq!(params.error.as_deref(), Some("access_denied"));
        assert!(params.code.is_none());
    }
}
