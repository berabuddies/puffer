//! Tiny axum-mounted OAuth 2.0 / OIDC server used by the integration tests.
//!
//! Implements just the slice of RFC 6749 / 7591 / 7636 / 8414 the runner
//! actually drives:
//!
//! * `GET /.well-known/oauth-authorization-server` — returns RFC 8414
//!   metadata (authorization_endpoint, token_endpoint, registration_endpoint).
//! * `POST /register` — RFC 7591 dynamic client registration. Issues a
//!   stable `client_id` (server-internal counter) and returns it.
//! * `GET /authorize` — auto-approves: redirects back to the supplied
//!   `redirect_uri` with `code=<random>&state=<echo>` immediately.
//!   Verifies the request carries `code_challenge` + `code_challenge_method=S256`.
//! * `POST /token` — exchanges `code` for an access_token (Bearer, 1h
//!   expiry) + refresh_token. Verifies `code_verifier` matches the
//!   PKCE challenge stored at `/authorize` time. Also handles
//!   `grant_type=refresh_token` for the refresh path.
//!
//! Knobs the tests can flip:
//!
//! * `with_short_token_lifetime`: returns `expires_in: 1` so the
//!   refresh-on-stale path fires deterministically.
//! * `with_failing_refresh`: rejects every refresh-token grant with 400,
//!   exercising the "expired refresh token" headless error path.
//!
//! Lives under `tests/` because it's only referenced from integration
//! tests in this crate (and re-pathed into puffer-core's test binary
//! via `#[path = ...]`).

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Form, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, Response, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize)]
struct AsMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
    response_types_supported: Vec<String>,
    grant_types_supported: Vec<String>,
    code_challenge_methods_supported: Vec<String>,
    token_endpoint_auth_methods_supported: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OAuthStubConfig {
    /// `expires_in` returned from `/token` (and refresh). Defaults to 3600s.
    pub access_token_ttl: Duration,
    /// When true, every refresh-token grant returns 400 Bad Request.
    pub fail_refresh: bool,
}

impl Default for OAuthStubConfig {
    fn default() -> Self {
        Self {
            access_token_ttl: Duration::from_secs(3600),
            fail_refresh: false,
        }
    }
}

#[derive(Debug, Clone)]
struct AuthRequestRecord {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    state: String,
    scope: Option<String>,
}

#[derive(Debug, Default)]
struct StubInner {
    config: OAuthStubConfig,
    /// counter for issued client_ids
    next_client: u32,
    /// counter for issued access tokens (so refresh changes the value)
    next_access_token: u32,
    /// `code` -> auth request snapshot (one-time use)
    pending_codes: HashMap<String, AuthRequestRecord>,
    /// `refresh_token` -> client_id (so refresh validates the binding)
    refresh_tokens: HashMap<String, String>,
    /// number of times /token has been hit with grant_type=authorization_code
    pub auth_code_grants: u32,
    /// number of times /token has been hit with grant_type=refresh_token
    pub refresh_grants: u32,
    /// /authorize hits (== full interactive flow attempts)
    pub authorize_hits: u32,
    /// /register hits
    pub register_hits: u32,
}

#[derive(Clone)]
pub struct OAuthStub {
    pub addr: SocketAddr,
    pub base_url: String,
    inner: Arc<Mutex<StubInner>>,
    cancel: CancellationToken,
    join: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl OAuthStub {
    pub async fn metrics(&self) -> Metrics {
        let g = self.inner.lock().await;
        Metrics {
            auth_code_grants: g.auth_code_grants,
            refresh_grants: g.refresh_grants,
            authorize_hits: g.authorize_hits,
            register_hits: g.register_hits,
        }
    }

    pub async fn shutdown(&self) {
        self.cancel.cancel();
        let mut g = self.join.lock().await;
        if let Some(h) = g.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Metrics {
    pub auth_code_grants: u32,
    pub refresh_grants: u32,
    pub authorize_hits: u32,
    pub register_hits: u32,
}

pub async fn spawn_oauth_stub(config: OAuthStubConfig) -> anyhow::Result<OAuthStub> {
    let inner = Arc::new(Mutex::new(StubInner {
        config,
        ..Default::default()
    }));
    let cancel = CancellationToken::new();

    let app = router_for(inner.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let base_url = format!("http://{}", addr);

    let cancel_for_shutdown = cancel.clone();
    let join = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { cancel_for_shutdown.cancelled_owned().await })
            .await;
    });

    Ok(OAuthStub {
        addr,
        base_url,
        inner,
        cancel,
        join: Arc::new(Mutex::new(Some(join))),
    })
}

// Real metadata handler: captures the inbound `Host` header so rmcp's
// absolute-URL parser doesn't choke on relative endpoint paths.
async fn metadata(headers: HeaderMap, State(_state): State<Arc<Mutex<StubInner>>>) -> impl IntoResponse {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("127.0.0.1");
    let scheme = "http";
    let base = format!("{scheme}://{host}");
    let metadata = AsMetadata {
        issuer: base.clone(),
        authorization_endpoint: format!("{base}/authorize"),
        token_endpoint: format!("{base}/token"),
        registration_endpoint: format!("{base}/register"),
        response_types_supported: vec!["code".to_string()],
        grant_types_supported: vec![
            "authorization_code".to_string(),
            "refresh_token".to_string(),
        ],
        code_challenge_methods_supported: vec!["S256".to_string()],
        token_endpoint_auth_methods_supported: vec!["none".to_string()],
    };
    Json(metadata)
}

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    #[serde(default)]
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
    #[serde(default)]
    grant_types: Vec<String>,
    #[serde(default)]
    response_types: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Serialize)]
struct RegisterResponse {
    client_id: String,
    client_name: Option<String>,
    redirect_uris: Vec<String>,
    grant_types: Vec<String>,
    response_types: Vec<String>,
    token_endpoint_auth_method: String,
}

async fn register_handler(
    State(state): State<Arc<Mutex<StubInner>>>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    let mut g = state.lock().await;
    g.register_hits += 1;
    g.next_client += 1;
    let client_id = format!("stub-client-{}", g.next_client);
    Json(RegisterResponse {
        client_id,
        client_name: req.client_name,
        redirect_uris: req.redirect_uris,
        grant_types: if req.grant_types.is_empty() {
            vec!["authorization_code".into(), "refresh_token".into()]
        } else {
            req.grant_types
        },
        response_types: if req.response_types.is_empty() {
            vec!["code".into()]
        } else {
            req.response_types
        },
        token_endpoint_auth_method: req
            .token_endpoint_auth_method
            .unwrap_or_else(|| "none".to_string()),
    })
}

#[derive(Debug, Deserialize)]
struct AuthorizeQuery {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
    #[serde(default)]
    scope: Option<String>,
}

async fn authorize_handler(
    State(state): State<Arc<Mutex<StubInner>>>,
    Query(q): Query<AuthorizeQuery>,
) -> impl IntoResponse {
    if q.response_type != "code" {
        return error_response(StatusCode::BAD_REQUEST, "unsupported_response_type");
    }
    if q.code_challenge_method != "S256" {
        return error_response(StatusCode::BAD_REQUEST, "invalid_pkce_method");
    }
    let mut g = state.lock().await;
    g.authorize_hits += 1;
    let code = format!("stub-code-{}", g.authorize_hits);
    g.pending_codes.insert(
        code.clone(),
        AuthRequestRecord {
            client_id: q.client_id,
            redirect_uri: q.redirect_uri.clone(),
            code_challenge: q.code_challenge,
            state: q.state.clone(),
            scope: q.scope,
        },
    );
    let mut sep = '?';
    if q.redirect_uri.contains('?') {
        sep = '&';
    }
    let location = format!("{}{}code={}&state={}", q.redirect_uri, sep, code, q.state);
    Redirect::temporary(&location).into_response()
}

fn error_response(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            "{{\"error\":\"{}\"}}",
            message
        )))
        .unwrap()
}

#[derive(Debug, Deserialize)]
struct TokenForm {
    grant_type: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    code_verifier: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct TokenResponseBody {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: String,
    scope: Option<String>,
}

async fn token_handler(
    State(state): State<Arc<Mutex<StubInner>>>,
    Form(form): Form<TokenForm>,
) -> impl IntoResponse {
    let mut g = state.lock().await;
    let ttl = g.config.access_token_ttl;
    let fail_refresh = g.config.fail_refresh;
    match form.grant_type.as_str() {
        "authorization_code" => {
            g.auth_code_grants += 1;
            let Some(code) = form.code.clone() else {
                return error_response(StatusCode::BAD_REQUEST, "missing_code");
            };
            let Some(verifier) = form.code_verifier.clone() else {
                return error_response(StatusCode::BAD_REQUEST, "missing_verifier");
            };
            let Some(record) = g.pending_codes.remove(&code) else {
                return error_response(StatusCode::BAD_REQUEST, "invalid_code");
            };
            if !verify_pkce(&verifier, &record.code_challenge) {
                return error_response(StatusCode::BAD_REQUEST, "invalid_pkce");
            }
            g.next_access_token += 1;
            let access = format!("stub-access-{}", g.next_access_token);
            let refresh = format!("stub-refresh-{}", g.next_access_token);
            g.refresh_tokens
                .insert(refresh.clone(), record.client_id.clone());
            let body = TokenResponseBody {
                access_token: access,
                token_type: "Bearer".to_string(),
                expires_in: ttl.as_secs(),
                refresh_token: refresh,
                scope: record.scope,
            };
            (StatusCode::OK, Json(body)).into_response()
        }
        "refresh_token" => {
            g.refresh_grants += 1;
            if fail_refresh {
                return error_response(StatusCode::BAD_REQUEST, "invalid_grant");
            }
            let Some(refresh) = form.refresh_token.clone() else {
                return error_response(StatusCode::BAD_REQUEST, "missing_refresh");
            };
            let Some(client_id) = g.refresh_tokens.get(&refresh).cloned() else {
                return error_response(StatusCode::BAD_REQUEST, "unknown_refresh");
            };
            // Rotate the refresh token to exercise that path.
            g.refresh_tokens.remove(&refresh);
            g.next_access_token += 1;
            let new_access = format!("stub-access-{}", g.next_access_token);
            let new_refresh = format!("stub-refresh-{}", g.next_access_token);
            g.refresh_tokens.insert(new_refresh.clone(), client_id);
            let body = TokenResponseBody {
                access_token: new_access,
                token_type: "Bearer".to_string(),
                expires_in: ttl.as_secs(),
                refresh_token: new_refresh,
                scope: None,
            };
            (StatusCode::OK, Json(body)).into_response()
        }
        _ => error_response(StatusCode::BAD_REQUEST, "unsupported_grant_type"),
    }
}

fn verify_pkce(verifier: &str, challenge: &str) -> bool {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let digest = hasher.finalize();
    let computed = base64_url_no_pad(&digest);
    computed == challenge
}

fn base64_url_no_pad(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    let mut i = 0;
    while i < input.len() {
        let b0 = input[i];
        let b1 = if i + 1 < input.len() { input[i + 1] } else { 0 };
        let b2 = if i + 2 < input.len() { input[i + 2] } else { 0 };
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize] as char);
        if i + 1 < input.len() {
            out.push(ALPHABET[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize] as char);
        }
        if i + 2 < input.len() {
            out.push(ALPHABET[(b2 & 0b111111) as usize] as char);
        }
        i += 3;
    }
    out
}

// The first metadata handler is unused; replace router with a concrete
// builder that uses the real handler.
fn router_for(state: Arc<Mutex<StubInner>>) -> Router {
    Router::new()
        .route("/.well-known/oauth-authorization-server", get(metadata))
        .route("/register", post(register_handler))
        .route("/authorize", get(authorize_handler))
        .route("/token", post(token_handler))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_round_trips() {
        // Known RFC 7636 sample: verifier="dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        // challenge="E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        assert!(verify_pkce(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
    }
}
