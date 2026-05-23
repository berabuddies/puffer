//! Wiring that boots the process-wide subscription manager and the
//! built-in subscriber discovery. Mirrors the connector wiring style.
//!
//! The subscription manager owns the in-process event bus, the spec
//! store on disk, the supervised subscriber children, and the router
//! task. Workflow tools and internal tools (`SubscriptionCreate`, `Telegram`, ...)
//! reach into it through a `OnceLock` installed here.

#[path = "lark_connector_actions.rs"]
mod lark_connector_actions;
#[path = "slack_connector_actions.rs"]
mod slack_connector_actions;

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::install_subscription_manager;
use puffer_provider_registry::{AuthStore, StoredCredential};
use puffer_subscriber_runtime::{
    Manifest, SendMediaAttachment, SendMediaKind, StateSpec, SubscriberCommand,
};
use puffer_subscriptions::{
    install_connector_action_executor, install_outbound, ClassifyDecision, ConnectorActionExecutor,
    ConnectorActionRequest, Outbound, RemoteClassifier, SubscriptionManager,
    SubscriptionManagerBuilder,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::Duration;
use tokio::runtime::Runtime;

use self::lark_connector_actions::{
    is_lark_action, is_lark_connector, run_lark_action, LarkConnectionAuthChecker,
};
use self::slack_connector_actions::{
    is_slack_action, is_slack_connector, run_slack_action, SlackConnectionAuthChecker,
};

const SUBSCRIBER_ACTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Owned wrapper around the dedicated Tokio runtime used by the
/// subscription manager and its supervised subscribers. Dropping it
/// after [`SubscriptionRuntime::shutdown`] joins all background tasks.
pub(crate) struct SubscriptionRuntime {
    runtime: Option<Runtime>,
    manager: Arc<SubscriptionManager>,
    auth_stop: Arc<AtomicBool>,
    auth_thread: Option<thread::JoinHandle<()>>,
}

impl SubscriptionRuntime {
    /// Returns the manager handle.
    pub(crate) fn manager(&self) -> Arc<SubscriptionManager> {
        self.manager.clone()
    }

    /// Best-effort shutdown: stops router + every subscriber, then drops
    /// the underlying runtime.
    pub(crate) fn shutdown(mut self) {
        self.shutdown_inner();
    }

    fn shutdown_inner(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        self.auth_stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.auth_thread.take() {
            let _ = thread.join();
        }
        shutdown_manager(&self.manager);
        runtime.shutdown_background();
    }
}

impl Drop for SubscriptionRuntime {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

/// Builds the subscription runtime, installs it as the process-wide
/// manager so workflow tools can find it, and (best-effort) starts the
/// bundled subscribers whose manifests are present on disk.
///
/// Failures are non-fatal: a broken subscription store or missing
/// manifest must not stop the rest of puffer from starting. Errors are
/// printed to stderr and the function still returns the manager
/// (empty) so workflow tools can report a meaningful error.
pub(crate) fn install(
    paths: &ConfigPaths,
    auth_store: &AuthStore,
    anthropic_base_url: Option<&str>,
) -> Result<SubscriptionRuntime> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .thread_name("puffer-subscriptions")
        .build()
        .context("failed to build subscription tokio runtime")?;
    let handle = runtime.handle().clone();

    let store_path = subscriptions_path(paths);
    let mut builder = SubscriptionManagerBuilder::new(&store_path);
    if let Some(classifier) = build_anthropic_classifier(auth_store, anthropic_base_url) {
        builder = builder.with_classifier(classifier);
    }
    builder = builder.with_connection_auth_checker(Arc::new(BuiltinConnectionAuthChecker {
        paths: paths.clone(),
    }));
    let manager = Arc::new(
        builder
            .build(handle)
            .context("failed to build subscription manager")?,
    );

    install_subscription_manager(manager.clone())
        .context("failed to install subscription manager")?;
    install_outbound(Arc::new(ManagerOutbound {
        manager: Arc::downgrade(&manager),
    }))
    .context("failed to install subscription outbound")?;
    install_connector_action_executor(Arc::new(ManagerConnectorActionExecutor {
        manager: Arc::downgrade(&manager),
        paths: paths.clone(),
    }))
    .context("failed to install connector action executor")?;

    autostart_subscribers(&manager, paths);
    let auth_stop = Arc::new(AtomicBool::new(false));
    let auth_thread = spawn_auth_monitor(manager.clone(), paths.clone(), auth_stop.clone())?;

    Ok(SubscriptionRuntime {
        runtime: Some(runtime),
        manager,
        auth_stop,
        auth_thread: Some(auth_thread),
    })
}

fn spawn_auth_monitor(
    manager: Arc<SubscriptionManager>,
    paths: ConfigPaths,
    stop: Arc<AtomicBool>,
) -> Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("puffer-connection-auth".to_string())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Err(error) = run_auth_monitor_tick(&manager, &paths) {
                    eprintln!("connection auth check failed: {error:#}");
                }
                for _ in 0..60 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_secs(1));
                }
            }
        })
        .context("failed to start connection auth monitor")
}

fn run_auth_monitor_tick(manager: &SubscriptionManager, paths: &ConfigPaths) -> Result<()> {
    manager.refresh_connection_consumers()?;
    ensure_auth_checkable_subscribers(manager, paths);
    let notices = manager.refresh_connection_auth()?;
    manager.refresh_connection_consumers()?;
    for connection in notices {
        eprintln!(
            "connection `{}` ({}) auth is no longer functioning; re-run the connector skill to repair it",
            connection.slug, connection.connector_slug
        );
    }
    Ok(())
}

fn ensure_auth_checkable_subscribers(manager: &SubscriptionManager, paths: &ConfigPaths) {
    for connection in manager.connection_store().list() {
        if connection.connector_slug != "telegram-login" || !connection.has_consumer {
            continue;
        }
        if let Err(error) = ensure_telegram_subscriber_running(manager, paths, &connection.slug) {
            eprintln!(
                "subscription: failed to start Telegram auth probe for `{}`: {error:#}",
                connection.slug
            );
        }
    }
}

fn shutdown_manager(manager: &Arc<SubscriptionManager>) {
    if tokio::runtime::Handle::try_current().is_ok() {
        let manager = manager.clone();
        let _ = std::thread::Builder::new()
            .name("puffer-subscriptions-shutdown".to_string())
            .spawn(move || manager.shutdown())
            .and_then(|thread| {
                thread.join().map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "subscription shutdown thread panicked",
                    )
                })
            });
        return;
    }
    manager.shutdown();
}

const DEFAULT_CLASSIFY_MODEL: &str = "claude-haiku-4-5";

/// Returns a classifier backed by Anthropic's `/v1/messages` endpoint
/// using the API key already in the auth store. Returns `None` when no
/// Anthropic API key is stored - the manager then falls back to the
/// default `NullClassifier`, and subscriptions with `classify_prompt`
/// will simply pass every event.
fn build_anthropic_classifier(
    auth_store: &AuthStore,
    anthropic_base_url: Option<&str>,
) -> Option<Arc<dyn puffer_subscriptions::Classifier>> {
    let key = match auth_store.providers.get("anthropic")? {
        StoredCredential::ApiKey { key } => key.clone(),
        StoredCredential::OAuth(_) => return None,
    };
    let base_url = anthropic_base_url
        .unwrap_or("https://api.anthropic.com")
        .trim_end_matches('/')
        .to_string();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .ok()?;
    let callable = move |prompt: &str, model_hint: Option<&str>, text: &str| -> ClassifyDecision {
        let model = parse_model_hint(model_hint).unwrap_or(DEFAULT_CLASSIFY_MODEL);
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 4,
            "system": "You are a strict yes/no classifier. Reply with exactly `yes` or `no` and nothing else.",
            "messages": [{
                "role": "user",
                "content": format!(
                    "Question: {prompt}\n\nMessage:\n{text}\n\nAnswer with exactly 'yes' or 'no'."
                )
            }]
        });
        let request = client
            .post(format!("{base_url}/v1/messages"))
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
            .json(&body);
        let response = match request.send() {
            Ok(r) => r,
            Err(_) => return ClassifyDecision::Inconclusive,
        };
        let parsed: serde_json::Value = match response.json() {
            Ok(v) => v,
            Err(_) => return ClassifyDecision::Inconclusive,
        };
        decision_from_anthropic(&parsed)
    };
    Some(Arc::new(RemoteClassifier::new(callable)))
}

fn parse_model_hint(hint: Option<&str>) -> Option<&str> {
    let hint = hint?;
    if let Some((provider, model)) = hint.split_once('/') {
        if provider.eq_ignore_ascii_case("anthropic") {
            return Some(model);
        }
        return None;
    }
    Some(hint)
}

fn decision_from_anthropic(parsed: &serde_json::Value) -> ClassifyDecision {
    let text = parsed
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        })
        .and_then(|b| b.get("text").and_then(|t| t.as_str()))
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if text.starts_with('y') {
        ClassifyDecision::Pass
    } else if text.starts_with('n') {
        ClassifyDecision::Reject
    } else {
        ClassifyDecision::Inconclusive
    }
}

/// Outbound impl that translates `(platform, target, text)` action calls
/// into [`SubscriberCommand::SendMessage`] commands routed through the
/// subscriber that owns the platform.
///
/// The compatibility outbound path is still platform based; connector
/// actions below use connection slugs when the caller needs a specific
/// authorized account.
struct ManagerOutbound {
    manager: Weak<SubscriptionManager>,
}

impl Outbound for ManagerOutbound {
    fn send(&self, platform: &str, target: &str, text: &str) -> Result<String> {
        let subscriber_id = subscriber_for_platform(platform).ok_or_else(|| {
            anyhow::anyhow!("no subscriber is configured to send messages on platform `{platform}`")
        })?;
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("subscription manager is no longer running"))?;
        let command = SubscriberCommand::SendMessage {
            peer: target.to_string(),
            text: text.to_string(),
            reply_to: None,
            media: Vec::new(),
        };
        let event = manager.send_command_and_wait(
            subscriber_id,
            subscriber_id,
            &command,
            &["send_complete", "send_error", "send_unsupported"],
            SUBSCRIBER_ACTION_TIMEOUT,
        )?;
        send_event_summary(&event, subscriber_id, platform, target, text.len(), 0)
    }
}

struct ManagerConnectorActionExecutor {
    manager: Weak<SubscriptionManager>,
    paths: ConfigPaths,
}

struct BuiltinConnectionAuthChecker {
    paths: ConfigPaths,
}

impl puffer_subscriptions::ConnectionAuthChecker for BuiltinConnectionAuthChecker {
    fn check(
        &self,
        template: &puffer_subscriptions::ConnectorTemplate,
        connection_slug: &str,
    ) -> Result<Option<bool>> {
        let slack = SlackConnectionAuthChecker {
            paths: self.paths.clone(),
        }
        .check(template, connection_slug)?;
        if slack.is_some() {
            return Ok(slack);
        }
        LarkConnectionAuthChecker {
            paths: self.paths.clone(),
        }
        .check(template, connection_slug)
    }
}

impl ConnectorActionExecutor for ManagerConnectorActionExecutor {
    fn run_connector_action(
        &self,
        connector_slug: &str,
        action: &str,
        input: serde_json::Value,
        trigger: serde_json::Value,
    ) -> Result<String> {
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("subscription manager is no longer running"))?;
        let template = manager
            .connector_store()
            .get(connector_slug)
            .ok_or_else(|| anyhow::anyhow!("connector `{connector_slug}` not found"))?;
        let action_definition = template.actions.get(action).ok_or_else(|| {
            anyhow::anyhow!("connector `{connector_slug}` does not define action `{action}`")
        })?;
        let connection = input
            .get("connection_slug")
            .or_else(|| input.get("account_slug"))
            .or_else(|| input.get("connection"))
            .or_else(|| input.get("account"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .or_else(|| {
                trigger
                    .get("connection_id")
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| connector_slug.to_string());
        let request = ConnectorActionRequest {
            connection: connection.clone(),
            action: action.to_string(),
            input: input.clone(),
            idempotency_key: trigger
                .get("envelope_id")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
        };
        if let Some(response) = manager.run_connector_action(&template, &request)? {
            if response.success {
                return Ok(format!(
                    "{} [{}]",
                    response.summary, action_definition.permission.category
                ));
            }
            anyhow::bail!(
                "{} [{}; retryable={}]",
                response.summary,
                action_definition.permission.category,
                response.retryable
            );
        }
        if is_slack_connector(connector_slug) && is_slack_action(action) {
            let summary = run_slack_action(&self.paths, &connection, action, &input)?;
            return Ok(format!(
                "{} [{}]",
                summary, action_definition.permission.category
            ));
        }
        if is_lark_connector(connector_slug) && is_lark_action(action) {
            let summary = run_lark_action(&self.paths, &connection, action, &input)?;
            return Ok(format!(
                "{} [{}]",
                summary, action_definition.permission.category
            ));
        }
        if action == "send_message" {
            let summary = send_message_via_subscriber(
                &manager,
                &self.paths,
                connector_slug,
                &connection,
                &input,
            )?;
            return Ok(format!(
                "{} [{}]",
                summary, action_definition.permission.category
            ));
        }
        if is_telegram_connector(connector_slug) && is_telegram_action(action) {
            let summary = telegram_action_via_subscriber(
                &manager,
                &self.paths,
                connector_slug,
                &connection,
                action,
                &input,
            )?;
            return Ok(format!(
                "{} [{}]",
                summary, action_definition.permission.category
            ));
        }
        anyhow::bail!("connector `{connector_slug}` has no `act` command for action `{action}`")
    }
}

fn send_message_via_subscriber(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connector_slug: &str,
    connection_slug: &str,
    input: &serde_json::Value,
) -> Result<String> {
    let target = input
        .get("to")
        .or_else(|| input.get("target"))
        .or_else(|| input.get("channel"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let text = input
        .get("message")
        .or_else(|| input.get("text"))
        .or_else(|| input.get("caption"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let media = parse_media_attachments(input)?;
    if target.trim().is_empty() || (text.trim().is_empty() && media.is_empty()) {
        anyhow::bail!("send_message requires `to`/`channel` and `message`, `caption`, or `media`");
    }
    let reply_to = parse_reply_to(input)?;
    let subscriber_id = subscriber_for_action(manager, paths, connector_slug, connection_slug)?
        .ok_or_else(|| {
            anyhow::anyhow!("no subscriber is configured for connector `{connector_slug}`")
        })?;
    let command = SubscriberCommand::SendMessage {
        peer: target.to_string(),
        text: text.to_string(),
        reply_to,
        media: media.clone(),
    };
    let event = manager.send_command_and_wait(
        &subscriber_id,
        &subscriber_id,
        &command,
        &["send_complete", "send_error", "send_unsupported"],
        SUBSCRIBER_ACTION_TIMEOUT,
    )?;
    send_event_summary(
        &event,
        &subscriber_id,
        connector_slug,
        target,
        text.len(),
        media.len(),
    )
}

fn telegram_action_via_subscriber(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connector_slug: &str,
    connection_slug: &str,
    action: &str,
    input: &serde_json::Value,
) -> Result<String> {
    let subscriber_id = subscriber_for_action(manager, paths, connector_slug, connection_slug)?
        .ok_or_else(|| {
            anyhow::anyhow!("no subscriber is configured for connector `{connector_slug}`")
        })?;
    let command = SubscriberCommand::Custom {
        op: "telegram_act".to_string(),
        args: serde_json::json!({
            "action": action,
            "input": input,
        }),
    };
    let event = manager.send_command_and_wait(
        &subscriber_id,
        &subscriber_id,
        &command,
        &["telegram_act_complete", "telegram_act_error", "login_error"],
        SUBSCRIBER_ACTION_TIMEOUT,
    )?;
    telegram_action_event_summary(&event, &subscriber_id, connector_slug, action)
}

fn send_event_summary(
    event: &puffer_subscriber_runtime::EventEnvelope,
    subscriber_id: &str,
    connector_slug: &str,
    target: &str,
    bytes: usize,
    media_count: usize,
) -> Result<String> {
    match event.event.kind.as_str() {
        "send_complete" => Ok(format!(
            "sent via {subscriber_id} -> {connector_slug}:{target} ({bytes} bytes, {media_count} media)"
        )),
        "send_error" | "send_unsupported" => {
            anyhow::bail!("send via {subscriber_id} failed: {}", event_error(event))
        }
        other => anyhow::bail!("send via {subscriber_id} returned unexpected event `{other}`"),
    }
}

fn telegram_action_event_summary(
    event: &puffer_subscriber_runtime::EventEnvelope,
    subscriber_id: &str,
    connector_slug: &str,
    action: &str,
) -> Result<String> {
    match event.event.kind.as_str() {
        "telegram_act_complete" => {
            let summary = event
                .event
                .payload
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("completed");
            Ok(format!(
                "Telegram action `{action}` via {subscriber_id} -> {connector_slug}: {summary}"
            ))
        }
        "telegram_act_error" | "login_error" => anyhow::bail!(
            "Telegram action `{action}` via {subscriber_id} failed: {}",
            event_error(event)
        ),
        other => {
            anyhow::bail!("Telegram action `{action}` returned unexpected event `{other}`")
        }
    }
}

fn event_error(event: &puffer_subscriber_runtime::EventEnvelope) -> String {
    event
        .event
        .payload
        .get("error")
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| {
            if event.event.text.trim().is_empty() {
                "unknown error"
            } else {
                event.event.text.as_str()
            }
        })
        .to_string()
}

fn parse_media_attachments(input: &serde_json::Value) -> Result<Vec<SendMediaAttachment>> {
    let mut media = Vec::new();
    for key in ["media", "attachments", "files"] {
        if let Some(value) = input.get(key) {
            parse_media_value(value, &mut media)?;
        }
    }
    for key in ["file", "path"] {
        if let Some(value) = input.get(key) {
            parse_media_value(value, &mut media)?;
        }
    }
    Ok(media)
}

fn parse_media_value(
    value: &serde_json::Value,
    media: &mut Vec<SendMediaAttachment>,
) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    if let Some(items) = value.as_array() {
        for item in items {
            media.push(parse_media_attachment(item)?);
        }
        return Ok(());
    }
    media.push(parse_media_attachment(value)?);
    Ok(())
}

fn parse_media_attachment(value: &serde_json::Value) -> Result<SendMediaAttachment> {
    if let Some(path) = value.as_str() {
        return Ok(SendMediaAttachment {
            path: path.to_string(),
            ..SendMediaAttachment::default()
        });
    }
    let Some(object) = value.as_object() else {
        anyhow::bail!("media attachment must be a string path/URL or object");
    };
    let path = object
        .get("path")
        .or_else(|| object.get("file"))
        .or_else(|| object.get("url"))
        .or_else(|| object.get("source"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if path.trim().is_empty() {
        anyhow::bail!("media attachment object requires `path`, `file`, or `url`");
    }
    let caption = object
        .get("caption")
        .or_else(|| object.get("message"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    let kind = object
        .get("kind")
        .or_else(|| object.get("type"))
        .or_else(|| object.get("media_type"))
        .and_then(serde_json::Value::as_str)
        .map(parse_media_kind)
        .transpose()?;
    let mime_type = object
        .get("mime_type")
        .or_else(|| object.get("mime"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    let thumbnail = object
        .get("thumbnail")
        .or_else(|| object.get("thumb"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    Ok(SendMediaAttachment {
        path: path.to_string(),
        caption,
        kind,
        mime_type,
        thumbnail,
    })
}

fn parse_media_kind(kind: &str) -> Result<SendMediaKind> {
    match kind.trim().to_lowercase().as_str() {
        "" | "auto" => Ok(SendMediaKind::Auto),
        "photo" | "image" => Ok(SendMediaKind::Photo),
        "document" | "doc" => Ok(SendMediaKind::Document),
        "file" => Ok(SendMediaKind::File),
        "audio" | "voice" | "video" | "media" => Ok(SendMediaKind::Document),
        other => anyhow::bail!("unsupported media kind `{other}`"),
    }
}

fn parse_reply_to(input: &serde_json::Value) -> Result<Option<i32>> {
    let Some(value) = input
        .get("reply_to")
        .or_else(|| input.get("reply_to_message_id"))
    else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if let Some(id) = value.as_i64() {
        return i32::try_from(id)
            .map(Some)
            .map_err(|_| anyhow::anyhow!("reply_to message id is outside i32 range"));
    }
    if let Some(id) = value.as_str() {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        return trimmed
            .parse::<i32>()
            .map(Some)
            .map_err(|_| anyhow::anyhow!("reply_to must be a Telegram message id"));
    }
    if let Some(id) = value
        .get("message_id")
        .or_else(|| value.get("id"))
        .and_then(serde_json::Value::as_i64)
    {
        return i32::try_from(id)
            .map(Some)
            .map_err(|_| anyhow::anyhow!("reply_to message id is outside i32 range"));
    }
    anyhow::bail!("reply_to must be a message id or object with `message_id`")
}

fn subscriber_for_platform(platform: &str) -> Option<&'static str> {
    match platform {
        "telegram" | "telegram-user" | "telegram-login" => Some("telegram-user"),
        "email" => Some("email"),
        _ => None,
    }
}

fn subscriber_for_action(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connector_slug: &str,
    connection_slug: &str,
) -> Result<Option<String>> {
    if is_telegram_connector(connector_slug) {
        let subscriber_id = telegram_subscriber_id(connection_slug);
        ensure_telegram_subscriber_running(manager, paths, &subscriber_id)?;
        return Ok(Some(subscriber_id));
    }
    Ok(subscriber_for_platform(connector_slug).map(ToString::to_string))
}

fn telegram_subscriber_id(connection_slug: &str) -> String {
    if connection_slug.trim().is_empty() || connection_slug == "telegram-login" {
        return "telegram-user".to_string();
    }
    connection_slug.to_string()
}

fn ensure_telegram_subscriber_running(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connection_slug: &str,
) -> Result<()> {
    validate_telegram_connection_slug(connection_slug)?;
    if manager
        .subscriber_ids()
        .iter()
        .any(|subscriber_id| subscriber_id == connection_slug)
    {
        return Ok(());
    }
    let manifest = telegram_connection_manifest(paths, connection_slug)?;
    manager.start_subscriber(manifest)?;
    Ok(())
}

fn telegram_connection_manifest(paths: &ConfigPaths, connection_slug: &str) -> Result<Manifest> {
    let dir = find_subscriber_manifest(paths, "telegram-user")
        .ok_or_else(|| anyhow::anyhow!("telegram-user subscriber manifest not found"))?;
    let mut manifest = Manifest::load(&dir)?;
    if connection_slug != "telegram-user" {
        manifest.spec.id = connection_slug.to_string();
        manifest.spec.topic = Some(connection_slug.to_string());
        manifest.spec.display_name = Some(format!("Telegram ({connection_slug})"));
        manifest.spec.state = Some(StateSpec {
            dir: paths
                .user_config_dir
                .join("telegram-accounts")
                .join(connection_slug)
                .to_string_lossy()
                .to_string(),
        });
    }
    Ok(manifest)
}

fn validate_telegram_connection_slug(connection_slug: &str) -> Result<()> {
    if connection_slug.is_empty()
        || !connection_slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        anyhow::bail!("Telegram connection slug must be non-empty kebab-case ASCII");
    }
    Ok(())
}

fn is_telegram_connector(connector_slug: &str) -> bool {
    matches!(
        connector_slug,
        "telegram" | "telegram-user" | "telegram-login"
    )
}

fn is_telegram_action(action: &str) -> bool {
    matches!(
        action,
        "vote_poll"
            | "edit_message"
            | "delete_message"
            | "delete_messages"
            | "forward_message"
            | "forward_messages"
            | "pin_message"
            | "unpin_message"
            | "unpin_all_messages"
            | "react"
            | "send_reaction"
            | "mark_read"
            | "clear_mentions"
            | "send_typing"
            | "send_chat_action"
            | "join_chat"
            | "leave_chat"
            | "kick_participant"
            | "ban_participant"
            | "unban_participant"
            | "invite_users"
            | "add_chat_users"
            | "update_profile"
            | "update_username"
            | "update_avatar"
            | "upload_avatar"
            | "update_group_title"
            | "update_group_name"
            | "update_group_username"
            | "update_group_photo"
            | "send_story"
    )
}

fn subscriptions_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("subscriptions.json")
}

#[cfg(test)]
#[path = "subscriptions_tests.rs"]
mod tests;

fn autostart_subscribers(manager: &SubscriptionManager, paths: &ConfigPaths) {
    for topic in autostart_topics(manager, paths) {
        match find_subscriber_manifest(paths, &topic) {
            Some(dir) => match Manifest::load(&dir) {
                Ok(manifest) => {
                    if let Err(error) = manager.start_subscriber(manifest) {
                        eprintln!("subscription: failed to start subscriber `{topic}`: {error:#}");
                    }
                }
                Err(error) => {
                    eprintln!("subscription: invalid manifest for `{topic}`: {error}");
                }
            },
            None => {
                if let Some(connection) = manager.connection_store().get(&topic) {
                    if connection.connector_slug == "telegram-login" {
                        if let Err(error) =
                            ensure_telegram_subscriber_running(manager, paths, &topic)
                        {
                            eprintln!(
                                "subscription: failed to start Telegram subscriber `{topic}`: {error:#}"
                            );
                        }
                        continue;
                    }
                }
                eprintln!(
                    "subscription: no manifest installed for source `{topic}` referenced by an existing subscription; events will not flow until one is added"
                );
            }
        }
    }
}

fn autostart_topics(manager: &SubscriptionManager, paths: &ConfigPaths) -> Vec<String> {
    let mut needed = std::collections::BTreeSet::new();
    for binding in manager.store().list() {
        if binding.status != puffer_subscriptions::WorkflowBindingStatus::Enabled {
            continue;
        }
        needed.insert(resolve_autostart_topic(
            manager,
            paths,
            &binding.connection_slug,
            binding.connector_slug.as_deref(),
        ));
    }
    for binding in manager.proxy_store().list() {
        if !binding.enabled {
            continue;
        }
        let connector_slug = manager
            .connection_store()
            .get(&binding.connection_slug)
            .map(|connection| connection.connector_slug);
        needed.insert(resolve_autostart_topic(
            manager,
            paths,
            &binding.connection_slug,
            connector_slug.as_deref(),
        ));
    }
    needed.into_iter().collect()
}

fn resolve_autostart_topic(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connection_slug: &str,
    connector_slug: Option<&str>,
) -> String {
    if find_subscriber_manifest(paths, connection_slug).is_some()
        || is_telegram_login_connection(manager, connection_slug)
    {
        return connection_slug.to_string();
    }
    if let Some(connector_slug) = connector_slug {
        if find_subscriber_manifest(paths, connector_slug).is_some() {
            return connector_slug.to_string();
        }
    }
    connection_slug.to_string()
}

fn is_telegram_login_connection(manager: &SubscriptionManager, connection_slug: &str) -> bool {
    manager
        .connection_store()
        .get(connection_slug)
        .is_some_and(|connection| connection.connector_slug == "telegram-login")
}

fn find_subscriber_manifest(paths: &ConfigPaths, topic: &str) -> Option<PathBuf> {
    let workspace = paths.workspace_config_dir.join("subscribers").join(topic);
    if workspace.join("manifest.toml").exists() {
        return Some(workspace);
    }
    let user = paths.user_config_dir.join("subscribers").join(topic);
    if user.join("manifest.toml").exists() {
        return Some(user);
    }
    let bundled = paths.builtin_resources_dir.join("subscribers").join(topic);
    if bundled.join("manifest.toml").exists() {
        return Some(bundled);
    }
    None
}
