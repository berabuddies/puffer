//! Wiring that boots the process-wide subscription manager and the
//! built-in subscriber discovery. Mirrors the connector wiring style.
//!
//! The subscription manager owns the in-process event bus, the spec
//! store on disk, the supervised subscriber children, and the router
//! task. Workflow tools and internal tools (`SubscriptionCreate`, `Telegram`, ...)
//! reach into it through a `OnceLock` installed here.

#[path = "email_connector_actions.rs"]
mod email_connector_actions;
#[path = "gcal_connector_actions.rs"]
mod gcal_connector_actions;
#[path = "slack_connector_actions.rs"]
mod slack_connector_actions;
#[path = "telegram_connector_actions.rs"]
mod telegram_connector_actions;

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::install_subscription_manager;
use puffer_provider_registry::{AuthStore, StoredCredential};
use puffer_subscriber_runtime::{
    EnvEntry, Manifest, SendMediaAttachment, SendMediaKind, SubscriberCommand,
};
use puffer_subscriptions::{
    connection_subscriber_manifest, direct_subscriber_manifest, find_subscriber_manifest,
    install_connector_action_executor, install_outbound, ClassifyDecision, ConnectionRecord,
    ConnectorActionExecutor, ConnectorActionRequest, ConnectorTemplate, Outbound, RemoteClassifier,
    SubscriberManifestRoots, SubscriptionManager, SubscriptionManagerBuilder,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::Duration;
use tokio::runtime::Runtime;

use self::email_connector_actions::{
    email_action_via_subscriber, email_subscriber_for_action, email_subscriber_for_platform,
    is_email_action, is_email_connector,
};
use self::gcal_connector_actions::{
    gcal_action_via_subscriber, gcal_subscriber_for_action, is_gcal_action, is_gcal_connector,
};
use self::slack_connector_actions::{
    is_slack_action, is_slack_connector, run_slack_action, SlackConnectionAuthChecker,
};
use self::telegram_connector_actions::{
    is_telegram_action, is_telegram_connector, telegram_action_via_subscriber,
    telegram_subscriber_for_action, telegram_subscriber_for_platform,
    TelegramConnectionAuthChecker,
};
#[cfg(test)]
use self::telegram_connector_actions::{telegram_subscriber_id, validate_telegram_connection_slug};

const SUBSCRIBER_ACTION_TIMEOUT: Duration = Duration::from_secs(60);
const SEND_MESSAGE_RECIPIENT_KEYS: &[&str] = &[
    "to",
    "target",
    "channel",
    "chat_id",
    "open_id",
    "user",
    "receive_id",
];
const SEND_MESSAGE_TEXT_KEYS: &[&str] = &["message", "text", "caption"];

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

    let store_path = paths.user_config_dir.join("subscriptions.json");
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
            "connection `{}` ({}) auth is no longer functioning; run `/connect {} {}` to repair it",
            connection.slug, connection.connector_slug, connection.connector_slug, connection.slug
        );
    }
    Ok(())
}

fn ensure_auth_checkable_subscribers(manager: &SubscriptionManager, paths: &ConfigPaths) {
    for connection in manager.connection_store().list() {
        if !connection.has_consumer {
            continue;
        }
        let Some(template) = manager.connector_store().get(&connection.connector_slug) else {
            continue;
        };
        if template.subscriber.is_none() {
            continue;
        }
        if let Err(error) = start_connection_subscriber(manager, paths, &connection, &template) {
            eprintln!(
                "subscription: failed to start auth probe subscriber for `{}`: {error:#}",
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
        manager: &SubscriptionManager,
        template: &puffer_subscriptions::ConnectorTemplate,
        connection_slug: &str,
    ) -> Result<Option<bool>> {
        if template.slug == crate::gmail_browser::CONNECTOR_SLUG {
            return Ok(Some(crate::gmail_browser::connection_auth_ok(
                &self.paths,
                connection_slug,
            )?));
        }
        if template.slug == crate::gcal_browser::CONNECTOR_SLUG {
            let configured = crate::gcal_browser::load_config(&self.paths, connection_slug)?
                .is_some_and(|config| config.is_configured());
            return Ok(Some(configured));
        }
        let slack = SlackConnectionAuthChecker {
            paths: self.paths.clone(),
        }
        .check(manager, template, connection_slug)?;
        if slack.is_some() {
            return Ok(slack);
        }
        TelegramConnectionAuthChecker.check(manager, template, connection_slug)
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
        if is_email_connector(connector_slug) && is_email_action(action) {
            let summary = email_action_via_subscriber(
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
        if is_gcal_connector(connector_slug) && is_gcal_action(action) {
            let summary = gcal_action_via_subscriber(
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
    let target = send_message_target(input).unwrap_or_default();
    let text = send_message_text(input).unwrap_or_default();
    let media = parse_media_attachments(input)?;
    if target.trim().is_empty() || (text.trim().is_empty() && media.is_empty()) {
        anyhow::bail!(
            "send_message requires a recipient (`to`, `target`, `channel`, `chat_id`, `open_id`, `user`, or `receive_id`) and message body (`message`, `text`, `caption`, `media`, `file`, `files`, `attachments`, or `path`)"
        );
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
        &target,
        text.len(),
        media.len(),
    )
}

fn send_message_target(input: &serde_json::Value) -> Option<String> {
    first_send_message_value(input, SEND_MESSAGE_RECIPIENT_KEYS, true)
}

fn send_message_text(input: &serde_json::Value) -> Option<String> {
    first_send_message_value(input, SEND_MESSAGE_TEXT_KEYS, false)
}

fn first_send_message_value(
    input: &serde_json::Value,
    keys: &[&str],
    accept_numbers: bool,
) -> Option<String> {
    keys.iter()
        .filter_map(|key| input.get(*key))
        .find_map(|value| send_message_value(value, accept_numbers))
}

fn send_message_value(value: &serde_json::Value, accept_numbers: bool) -> Option<String> {
    if let Some(text) = value.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(if accept_numbers {
                trimmed.to_string()
            } else {
                text.to_string()
            });
        }
        return None;
    }
    if accept_numbers {
        return value.as_number().map(ToString::to_string);
    }
    None
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
    if let Some(subscriber) = email_subscriber_for_platform(platform) {
        return Some(subscriber);
    }
    if let Some(subscriber) = telegram_subscriber_for_platform(platform) {
        return Some(subscriber);
    }
    None
}

fn subscriber_for_action(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connector_slug: &str,
    connection_slug: &str,
) -> Result<Option<String>> {
    if let Some(subscriber_id) =
        telegram_subscriber_for_action(manager, paths, connector_slug, connection_slug)?
    {
        return Ok(Some(subscriber_id));
    }
    if let Some(subscriber_id) =
        email_subscriber_for_action(manager, paths, connector_slug, connection_slug)?
    {
        return Ok(Some(subscriber_id));
    }
    if let Some(subscriber_id) =
        gcal_subscriber_for_action(manager, paths, connector_slug, connection_slug)?
    {
        return Ok(Some(subscriber_id));
    }
    Ok(subscriber_for_platform(connector_slug).map(ToString::to_string))
}

#[cfg(test)]
#[path = "subscriptions_tests.rs"]
mod tests;

fn autostart_subscribers(manager: &SubscriptionManager, paths: &ConfigPaths) {
    for topic in autostart_topics(manager, paths) {
        match autostart_manifest(manager, paths, &topic) {
            Ok(Some(mut manifest)) => {
                set_workspace_config_env(&mut manifest, paths);
                if let Err(error) = manager.start_subscriber(manifest) {
                    eprintln!("subscription: failed to start subscriber `{topic}`: {error:#}");
                }
            }
            Ok(None) => {
                eprintln!(
                    "subscription: no manifest installed for source `{topic}` referenced by an existing subscription; events will not flow until one is added"
                );
            }
            Err(error) => {
                eprintln!("subscription: invalid manifest for `{topic}`: {error}");
            }
        }
    }
}

fn autostart_manifest(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    topic: &str,
) -> Result<Option<Manifest>> {
    if let Some(connection) = manager.connection_store().get(topic) {
        if let Some(template) = manager.connector_store().get(&connection.connector_slug) {
            return connection_subscriber_manifest(
                &subscriber_manifest_roots(paths),
                &connection,
                &template,
            );
        }
    }
    if let Some(manifest) = autostart_manifest_from_binding(manager, paths, topic)? {
        return Ok(Some(manifest));
    }
    direct_subscriber_manifest(&subscriber_manifest_roots(paths), topic)
}

fn autostart_manifest_from_binding(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    topic: &str,
) -> Result<Option<Manifest>> {
    for binding in manager.store().list() {
        if binding.status != puffer_subscriptions::WorkflowBindingStatus::Enabled {
            continue;
        }
        if binding.connection_slug != topic {
            continue;
        }
        let Some(connector_slug) = binding.connector_slug.as_deref() else {
            continue;
        };
        let Some(template) = manager.connector_store().get(connector_slug) else {
            continue;
        };
        let connection =
            ConnectionRecord::authenticated(topic.to_string(), connector_slug.to_string(), topic);
        if let Some(manifest) = connection_subscriber_manifest(
            &subscriber_manifest_roots(paths),
            &connection,
            &template,
        )? {
            return Ok(Some(manifest));
        }
    }
    Ok(None)
}

fn autostart_topics(manager: &SubscriptionManager, paths: &ConfigPaths) -> Vec<String> {
    let mut needed = std::collections::BTreeSet::new();
    for binding in manager.store().list() {
        if binding.status != puffer_subscriptions::WorkflowBindingStatus::Enabled {
            continue;
        }
        if connector_uses_command_stream(manager, binding.connector_slug.as_deref()) {
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
        if connector_uses_command_stream(manager, connector_slug.as_deref()) {
            continue;
        }
        needed.insert(resolve_autostart_topic(
            manager,
            paths,
            &binding.connection_slug,
            connector_slug.as_deref(),
        ));
    }
    needed.into_iter().collect()
}

/// Command-backed connectors stream via `connector_stream`, not a manifest subscriber.
fn connector_uses_command_stream(
    manager: &SubscriptionManager,
    connector_slug: Option<&str>,
) -> bool {
    connector_slug
        .and_then(|slug| manager.connector_store().get(slug))
        .is_some_and(|template| template.command_argv().is_some() && template.can_subscribe)
}

fn resolve_autostart_topic(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connection_slug: &str,
    connector_slug: Option<&str>,
) -> String {
    if find_subscriber_manifest(&subscriber_manifest_roots(paths), connection_slug).is_some()
        || connection_has_instantiated_subscriber(manager, paths, connection_slug)
    {
        return connection_slug.to_string();
    }
    if let Some(connector_slug) = connector_slug {
        if find_subscriber_manifest(&subscriber_manifest_roots(paths), connector_slug).is_some() {
            return connector_slug.to_string();
        }
    }
    connection_slug.to_string()
}

fn connection_has_instantiated_subscriber(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connection_slug: &str,
) -> bool {
    let Some(connection) = manager.connection_store().get(connection_slug) else {
        return false;
    };
    let Some(template) = manager.connector_store().get(&connection.connector_slug) else {
        return false;
    };
    let Some(subscriber) = &template.subscriber else {
        return false;
    };
    find_subscriber_manifest(&subscriber_manifest_roots(paths), &subscriber.manifest_slug).is_some()
}

fn start_connection_subscriber(
    manager: &SubscriptionManager,
    paths: &ConfigPaths,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> Result<()> {
    if let Some(mut manifest) =
        connection_subscriber_manifest(&subscriber_manifest_roots(paths), connection, template)?
    {
        set_workspace_config_env(&mut manifest, paths);
        manager.start_subscriber(manifest)?;
    }
    Ok(())
}

fn set_workspace_config_env(manifest: &mut Manifest, paths: &ConfigPaths) {
    manifest
        .spec
        .run
        .env
        .retain(|entry| entry.name != "PUFFER_WORKSPACE_CONFIG_DIR");
    manifest.spec.run.env.push(EnvEntry {
        name: "PUFFER_WORKSPACE_CONFIG_DIR".to_string(),
        value: paths.workspace_config_dir.to_string_lossy().to_string(),
    });
}

fn subscriber_manifest_roots(paths: &ConfigPaths) -> SubscriberManifestRoots {
    SubscriberManifestRoots::new(
        paths.workspace_config_dir.clone(),
        paths.user_config_dir.clone(),
        paths.builtin_resources_dir.clone(),
    )
}
