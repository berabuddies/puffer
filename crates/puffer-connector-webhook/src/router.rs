use crate::handler::{extract_bearer, handle_command};
use crate::WebhookConfig;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use puffer_connector_core::{CommandOutcome, ConnectorRuntime, InboundMessage, MessageSplitter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Shared axum state: the runtime plus a snapshot of the config.
#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<ConnectorRuntime>,
    pub config: Arc<WebhookConfig>,
}

/// Native Puffer JSON payload accepted by the webhook endpoint.
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
    Json(payload): Json<Value>,
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

    let inbound = match inbound_from_payload(&headers, &payload) {
        Some(inbound) => inbound,
        None => return (StatusCode::BAD_REQUEST, "unsupported webhook payload").into_response(),
    };
    let runtime = state.runtime.clone();
    let config = state.config.clone();

    // Dispatch is blocking (it acquires the runtime mutex and may run a
    // whole agent turn); run it on a blocking worker so the reactor
    // stays responsive.
    let result =
        tokio::task::spawn_blocking(move || handle_command(&runtime, &inbound, config.as_ref()))
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

fn inbound_from_payload(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    puffer_inbound(payload).or_else(|| github_inbound(headers, payload))
}

fn puffer_inbound(payload: &Value) -> Option<InboundMessage> {
    let conversation_id = string_field(payload, "conversation_id")?;
    let message = string_field(payload, "message")?;
    Some(InboundMessage {
        conversation_id: conversation_id.to_string(),
        user_id: string_field(payload, "user_id").map(str::to_string),
        text: message.to_string(),
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    })
}

fn github_inbound(headers: &HeaderMap, payload: &Value) -> Option<InboundMessage> {
    let repository = pointer_string(payload, "/repository/full_name")
        .or_else(|| pointer_string(payload, "/repository/name"))?;
    let event = header_value(headers, "x-github-event").unwrap_or("github");
    let action = string_field(payload, "action").unwrap_or("received");
    let delivery = header_value(headers, "x-github-delivery");
    let sender = pointer_string(payload, "/sender/login")
        .or_else(|| pointer_string(payload, "/pusher/name"))
        .unwrap_or("github");
    let subject = github_subject(payload);
    let conversation_id =
        github_conversation_id(repository, event, delivery, payload, subject.as_ref());
    let text = github_message(repository, event, action, sender, payload, subject.as_ref());

    Some(InboundMessage {
        conversation_id,
        user_id: Some(sender.to_string()),
        text,
        thread_id: None,
        is_group: false,
        bot_mentioned: true,
        from_bot: false,
    })
}

struct GithubSubject {
    kind: &'static str,
    conversation_kind: &'static str,
    number: Option<String>,
    title: Option<String>,
    body: Option<String>,
    url: Option<String>,
}

fn github_subject(payload: &Value) -> Option<GithubSubject> {
    if let Some(comment) = payload.get("comment") {
        let parent = github_parent_subject(payload);
        return Some(GithubSubject {
            kind: "comment",
            conversation_kind: parent
                .as_ref()
                .map(|subject| subject.conversation_kind)
                .unwrap_or("comment"),
            number: parent.as_ref().and_then(|subject| subject.number.clone()),
            title: parent.as_ref().and_then(|subject| subject.title.clone()),
            body: string_field(comment, "body").map(snippet),
            url: string_field(comment, "html_url")
                .map(str::to_string)
                .or_else(|| parent.and_then(|subject| subject.url)),
        });
    }
    github_parent_subject(payload)
}

fn github_parent_subject(payload: &Value) -> Option<GithubSubject> {
    [
        ("issue", github_issue_kind(payload)),
        ("pull_request", "pull request"),
        ("discussion", "discussion"),
    ]
    .into_iter()
    .find_map(|(field, kind)| {
        let value = payload.get(field)?;
        Some(GithubSubject {
            kind,
            conversation_kind: kind,
            number: value
                .get("number")
                .and_then(number_or_string)
                .or_else(|| payload.get("number").and_then(number_or_string)),
            title: string_field(value, "title").map(str::to_string),
            body: string_field(value, "body").map(snippet),
            url: string_field(value, "html_url").map(str::to_string),
        })
    })
}

fn github_conversation_id(
    repository: &str,
    event: &str,
    delivery: Option<&str>,
    payload: &Value,
    subject: Option<&GithubSubject>,
) -> String {
    if let Some(subject) = subject {
        if let Some(number) = &subject.number {
            return format!(
                "github:{repository}:{}:{number}",
                subject.conversation_kind.replace(' ', "-")
            );
        }
    }
    if let Some(reference) = string_field(payload, "ref") {
        return format!("github:{repository}:{event}:{reference}");
    }
    format!(
        "github:{repository}:{event}:{}",
        delivery.unwrap_or("event")
    )
}

fn github_message(
    repository: &str,
    event: &str,
    action: &str,
    sender: &str,
    payload: &Value,
    subject: Option<&GithubSubject>,
) -> String {
    let mut lines = vec![
        format!("GitHub {event} {action} in {repository}"),
        format!("Sender: {sender}"),
    ];
    if let Some(subject) = subject {
        let number = subject
            .number
            .as_ref()
            .map(|value| format!("#{value} "))
            .unwrap_or_default();
        if let Some(title) = &subject.title {
            if subject.kind == "comment" && subject.conversation_kind != "comment" {
                lines.push(format!(
                    "Subject: comment on {} {number}{title}",
                    subject.conversation_kind
                ));
            } else {
                lines.push(format!("Subject: {} {number}{title}", subject.kind));
            }
        }
        if let Some(url) = &subject.url {
            lines.push(format!("URL: {url}"));
        }
        if let Some(body) = &subject.body {
            lines.push(String::new());
            lines.push(body.clone());
        }
    } else if event == "push" {
        append_push_summary(&mut lines, payload);
    }
    lines.join("\n")
}

fn github_issue_kind(payload: &Value) -> &'static str {
    payload
        .get("issue")
        .and_then(|issue| issue.get("pull_request"))
        .map(|_| "pull request")
        .unwrap_or("issue")
}

fn append_push_summary(lines: &mut Vec<String>, payload: &Value) {
    if let Some(reference) = string_field(payload, "ref") {
        lines.push(format!("Ref: {reference}"));
    }
    let commits = payload
        .get("commits")
        .and_then(Value::as_array)
        .map(|commits| commits.iter().take(3).collect::<Vec<_>>())
        .unwrap_or_default();
    if commits.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("Commits:".to_string());
    for commit in commits {
        let id = string_field(commit, "id")
            .map(|value| value.chars().take(7).collect::<String>())
            .unwrap_or_else(|| "commit".to_string());
        let message = string_field(commit, "message")
            .map(snippet)
            .unwrap_or_default();
        lines.push(format!("- {id}: {message}"));
    }
}

fn string_field<'a>(payload: &'a Value, field: &str) -> Option<&'a str> {
    payload.get(field).and_then(Value::as_str)
}

fn pointer_string<'a>(payload: &'a Value, pointer: &str) -> Option<&'a str> {
    payload.pointer(pointer).and_then(Value::as_str)
}

fn number_or_string(value: &Value) -> Option<String> {
    value
        .as_u64()
        .map(|number| number.to_string())
        .or_else(|| value.as_str().map(str::to_string))
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn snippet(value: &str) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_CHARS: usize = 500;
    if collapsed.chars().count() <= MAX_CHARS {
        return collapsed;
    }
    let mut out = collapsed.chars().take(MAX_CHARS).collect::<String>();
    out.push_str("...");
    out
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
    use puffer_connector_core::{ConnectorRuntime, ConnectorRuntimeConfig, ConversationSessionMap};
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

    #[test]
    fn github_issue_payload_maps_to_inbound_message() {
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "issues".parse().unwrap());
        headers.insert("x-github-delivery", "delivery-1".parse().unwrap());
        let payload = serde_json::json!({
            "action": "opened",
            "repository": {"full_name": "berabuddies/puffer"},
            "sender": {"login": "tonykebot"},
            "issue": {
                "number": 42,
                "title": "Workflow drafts should be visible",
                "body": "Please expose this in connector UX.",
                "html_url": "https://github.com/berabuddies/puffer/issues/42"
            }
        });

        let inbound = inbound_from_payload(&headers, &payload).expect("github inbound");

        assert_eq!(
            inbound.conversation_id,
            "github:berabuddies/puffer:issue:42"
        );
        assert_eq!(inbound.user_id.as_deref(), Some("tonykebot"));
        assert!(inbound
            .text
            .contains("GitHub issues opened in berabuddies/puffer"));
        assert!(inbound
            .text
            .contains("Subject: issue #42 Workflow drafts should be visible"));
        assert!(inbound
            .text
            .contains("https://github.com/berabuddies/puffer/issues/42"));
    }

    #[test]
    fn github_comment_payload_keeps_parent_thread_and_comment_body() {
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "issue_comment".parse().unwrap());
        let payload = serde_json::json!({
            "action": "created",
            "repository": {"full_name": "berabuddies/puffer"},
            "sender": {"login": "tonykebot"},
            "issue": {
                "number": 42,
                "title": "Workflow drafts should be visible",
                "html_url": "https://github.com/berabuddies/puffer/issues/42"
            },
            "comment": {
                "body": "This should appear in Puffer.",
                "html_url": "https://github.com/berabuddies/puffer/issues/42#issuecomment-1"
            }
        });

        let inbound = inbound_from_payload(&headers, &payload).expect("github inbound");

        assert_eq!(
            inbound.conversation_id,
            "github:berabuddies/puffer:issue:42"
        );
        assert!(inbound
            .text
            .contains("Subject: comment on issue #42 Workflow drafts should be visible"));
        assert!(inbound.text.contains("This should appear in Puffer."));
        assert!(inbound
            .text
            .contains("https://github.com/berabuddies/puffer/issues/42#issuecomment-1"));
    }

    #[test]
    fn github_push_payload_maps_commit_summary() {
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "push".parse().unwrap());
        let payload = serde_json::json!({
            "repository": {"full_name": "berabuddies/puffer"},
            "pusher": {"name": "tonykebot"},
            "ref": "refs/heads/master",
            "commits": [
                {"id": "abcdef1234567890", "message": "Ship workflow UX"}
            ]
        });

        let inbound = inbound_from_payload(&headers, &payload).expect("github inbound");

        assert_eq!(
            inbound.conversation_id,
            "github:berabuddies/puffer:push:refs/heads/master"
        );
        assert!(inbound
            .text
            .contains("GitHub push received in berabuddies/puffer"));
        assert!(inbound.text.contains("- abcdef1: Ship workflow UX"));
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
