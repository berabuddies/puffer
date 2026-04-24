use crate::handler::{extract_bearer, handle_command};
use crate::WebhookConfig;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use puffer_connector_core::{CommandOutcome, ConnectorRuntime, InboundMessage, MessageSplitter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Shared axum state: the runtime plus a snapshot of the config.
#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<ConnectorRuntime>,
    pub config: Arc<WebhookConfig>,
}

/// JSON payload accepted by the webhook endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookRequest {
    pub conversation_id: String,
    pub message: String,
    /// Optional caller-supplied user identity. Webhook sessions are
    /// keyed per-conversation, so this is informational — it's threaded
    /// through `InboundMessage` for logging and future routing.
    #[serde(default)]
    pub user_id: Option<String>,
}

/// JSON payload returned on a successful dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookResponse {
    /// The bound session id when the agent was consulted. `None` for
    /// local replies like `/help`.
    pub session_id: Option<String>,
    /// The text to display to the user.
    pub reply: String,
    /// `true` when the reply body exceeded the recommended soft cap
    /// (derived from `MessageSplitter::SLACK.max_chars`). The body is
    /// still returned in full — this flag lets clients surface a hint
    /// that the content may be awkward to render in a single bubble.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub oversized: bool,
    /// Human-readable warning attached when `oversized` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Soft cap on response body size used for the `oversized` flag. Chosen
/// to match `MessageSplitter::SLACK.max_chars` so a webhook-fronted
/// integration that later fans out to Slack won't be surprised.
const RESPONSE_SOFT_CAP_CHARS: usize = MessageSplitter::SLACK.max_chars;

/// Builds the axum router used by both the live connector and the
/// integration tests.
pub fn build_router(state: AppState) -> Router {
    let config_path = state.config.resolved_path();
    Router::new()
        .route("/health", get(health))
        .route(&config_path, post(webhook))
        .with_state(state)
}

async fn health() -> Response {
    (StatusCode::OK, "puffer").into_response()
}

async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<WebhookRequest>,
) -> Response {
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if !state.config.is_origin_allowed(origin) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }

    // Auth check runs in the transport so the handler can stay pure
    // `InboundMessage` logic. We preserve the legacy semantics: reject
    // when a token is configured but the request's Bearer token does
    // not match.
    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let presented_token = auth_header.and_then(extract_bearer);
    if !state.config.is_token_allowed(presented_token.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "request rejected").into_response();
    }

    let runtime = state.runtime.clone();
    let config = state.config.clone();
    let inbound = InboundMessage {
        conversation_id: payload.conversation_id.clone(),
        user_id: payload.user_id.clone(),
        text: payload.message.clone(),
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    };

    // Dispatch is blocking (it acquires the runtime mutex and may run a
    // whole agent turn); run it on a blocking worker so the reactor
    // stays responsive.
    let result = tokio::task::spawn_blocking(move || {
        handle_command(&runtime, &inbound, config.as_ref())
    })
    .await;

    let outcome = match result {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Puffer error: {error}"),
            )
                .into_response();
        }
        Err(join_error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Puffer worker panicked: {join_error}"),
            )
                .into_response();
        }
    };

    match outcome {
        CommandOutcome::Ignored => {
            // Handler rejected an empty body (auth was already handled
            // upstream). Empty payloads are a client error.
            let had_auth = state.config.auth_token.is_some();
            let status = if had_auth {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, "request rejected").into_response()
        }
        CommandOutcome::Reply(text) => build_reply_response(&state.config, None, text),
        CommandOutcome::AgentReply {
            session_id, text, ..
        } => build_reply_response(&state.config, Some(session_id.to_string()), text),
    }
}

/// Packs the reply into the webhook response JSON, attaching the
/// `oversized` flag and logging a warning when the body exceeds the
/// recommended soft cap. Webhooks are a single HTTP body so we never
/// split — the caller asked for the full reply in one payload.
fn build_reply_response(
    config: &WebhookConfig,
    session_id: Option<String>,
    text: String,
) -> Response {
    let chars = text.chars().count();
    let mut oversized = false;
    let mut warning = None;
    if chars > RESPONSE_SOFT_CAP_CHARS {
        oversized = true;
        warning = Some(format!(
            "response exceeds recommended size ({chars} chars > {RESPONSE_SOFT_CAP_CHARS})"
        ));
        if config.split_long_responses {
            // split_long_responses is advisory here — webhook never
            // actually splits the body across HTTP responses, it just
            // loudly flags the caller. Surface it in stderr so ops can
            // see a recurring pattern.
            eprintln!(
                "puffer-connector-webhook: {}",
                warning.as_deref().unwrap_or("oversized reply")
            );
        }
    }
    (
        StatusCode::OK,
        Json(WebhookResponse {
            session_id,
            reply: text,
            oversized,
            warning,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt as _;
    use puffer_config::{ConfigPaths, PufferConfig};
    use puffer_connector_core::{
        ConnectorRuntime, ConnectorRuntimeConfig, ConversationSessionMap,
    };
    use puffer_provider_registry::{AuthStore, ProviderRegistry};
    use puffer_resources::LoadedResources;
    use puffer_session_store::SessionStore;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tower::ServiceExt; // for `oneshot`

    fn test_runtime(root: std::path::PathBuf) -> Arc<ConnectorRuntime> {
        let paths = ConfigPaths {
            workspace_root: root.clone(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join(".puffer-user"),
            builtin_resources_dir: root.join("resources"),
        };
        std::fs::create_dir_all(&paths.user_config_dir).unwrap();
        Arc::new(ConnectorRuntime::new(ConnectorRuntimeConfig {
            config: PufferConfig::default(),
            resources: LoadedResources::default(),
            providers: ProviderRegistry::default(),
            auth_store: AuthStore::default(),
            auth_path: root.join("auth.json"),
            session_store: SessionStore::from_paths(&paths).unwrap(),
            session_map: ConversationSessionMap::in_memory(),
            default_cwd: root,
        }))
    }

    fn base_config() -> WebhookConfig {
        WebhookConfig {
            bind_address: "127.0.0.1:0".to_string(),
            path: None,
            auth_token: None,
            allowed_origins: Vec::new(),
            welcome_message: None,
            split_long_responses: true,
        }
    }

    #[tokio::test]
    async fn health_endpoint_returns_200_ok() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let router = build_router(AppState {
            runtime,
            config: Arc::new(base_config()),
        });
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"puffer");
    }

    #[tokio::test]
    async fn webhook_help_returns_local_reply_over_http() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let router = build_router(AppState {
            runtime,
            config: Arc::new(base_config()),
        });
        let body = serde_json::to_vec(&serde_json::json!({
            "conversation_id": "conv-xyz",
            "message": "/help"
        }))
        .unwrap();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/puffer")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let parsed: WebhookResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed.reply.contains("/help"));
        assert!(parsed.session_id.is_none());
        assert!(!parsed.oversized);
    }

    #[tokio::test]
    async fn webhook_accepts_optional_user_id_field() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let router = build_router(AppState {
            runtime,
            config: Arc::new(base_config()),
        });
        let body = serde_json::to_vec(&serde_json::json!({
            "conversation_id": "conv-xyz",
            "message": "/help",
            "user_id": "alice"
        }))
        .unwrap();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/puffer")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn webhook_rejects_missing_auth_when_token_set() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut config = base_config();
        config.auth_token = Some("sekret".to_string());
        let router = build_router(AppState {
            runtime,
            config: Arc::new(config),
        });
        let body = serde_json::to_vec(&serde_json::json!({
            "conversation_id": "conv-xyz",
            "message": "/help"
        }))
        .unwrap();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/puffer")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_accepts_valid_bearer_token() {
        let runtime = test_runtime(tempdir().unwrap().path().to_path_buf());
        let mut config = base_config();
        config.auth_token = Some("sekret".to_string());
        let router = build_router(AppState {
            runtime,
            config: Arc::new(config),
        });
        let body = serde_json::to_vec(&serde_json::json!({
            "conversation_id": "conv-xyz",
            "message": "/help"
        }))
        .unwrap();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/puffer")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer sekret")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn oversized_reply_sets_flag_but_returns_full_body() {
        // Exercise the response helper directly — producing >38k chars
        // via the real agent would need a wired-up provider.
        let config = base_config();
        let big = "x".repeat(RESPONSE_SOFT_CAP_CHARS + 10);
        let response = build_reply_response(&config, None, big.clone());
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let parsed: WebhookResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed.oversized);
        assert!(parsed.warning.is_some());
        assert_eq!(parsed.reply.chars().count(), big.chars().count());
    }

    #[tokio::test]
    async fn normal_reply_omits_oversized_and_warning_fields() {
        let config = base_config();
        let response = build_reply_response(&config, None, "hi".to_string());
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        let raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // Skipped via `skip_serializing_if` when default-ish.
        assert!(raw.get("oversized").is_none());
        assert!(raw.get("warning").is_none());
    }
}
