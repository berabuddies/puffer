use crate::handler::{handle_command, PLATFORM_ID};
use crate::SlackConfig;
use anyhow::{Context, Result};
use puffer_connector_core::{
    CommandOutcome, Connector, ConnectorHandle, ConnectorRuntime, ConnectorStartError,
    InboundMessage, MessageSplitter,
};
use slack_morphism::prelude::*;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Slack connector ready to be started by the puffer connector hub.
///
/// Uses slack-morphism's Socket Mode listener (so the bot does not need
/// a public HTTPS endpoint). Inbound `message` events are converted to
/// [`InboundMessage`] and run through [`handle_command`]; the reply is
/// sent back via `chat.postMessage`, preserving `thread_ts` when the
/// original message came from a thread.
pub struct SlackConnector {
    config: SlackConfig,
}

impl SlackConnector {
    /// Creates a Slack connector from parsed configuration.
    pub fn new(config: SlackConfig) -> Self {
        Self { config }
    }

    /// Returns the Slack connector configuration.
    pub fn config(&self) -> &SlackConfig {
        &self.config
    }
}

impl Connector for SlackConnector {
    fn id(&self) -> &str {
        PLATFORM_ID
    }

    fn start(
        self: Box<Self>,
        runtime: Arc<ConnectorRuntime>,
    ) -> Result<ConnectorHandle, ConnectorStartError> {
        if self.config.bot_token.trim().is_empty() {
            return Err(ConnectorStartError::MissingConfig {
                id: PLATFORM_ID.to_string(),
                detail: "bot_token is empty".to_string(),
            });
        }
        if self.config.app_token.trim().is_empty() {
            return Err(ConnectorStartError::MissingConfig {
                id: PLATFORM_ID.to_string(),
                detail: "app_token is empty".to_string(),
            });
        }

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<()>();

        let config = self.config.clone();
        let join = std::thread::Builder::new()
            .name("puffer-connector-slack".to_string())
            .spawn(move || -> Result<()> {
                let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .context("failed to build tokio runtime for slack connector")?;
                let _ = ready_tx.send(());
                tokio_runtime.block_on(run_socket_mode(config, runtime, shutdown_rx))
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

/// Package every piece of state the push-event callback needs in a
/// single `Arc` so slack-morphism's callback can clone it cheaply.
#[derive(Clone)]
struct HandlerCtx {
    runtime: Arc<ConnectorRuntime>,
    config: Arc<SlackConfig>,
    bot_token: SlackApiToken,
    /// Bot's own Slack user id, fetched once at startup via
    /// `auth.test`. Used for mention detection, mention stripping, and
    /// bot-self loop prevention.
    bot_user_id: Option<SlackUserId>,
}

async fn run_socket_mode(
    config: SlackConfig,
    runtime: Arc<ConnectorRuntime>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let client = Arc::new(SlackClient::new(
        SlackClientHyperConnector::new().context("failed to build slack hyper connector")?,
    ));

    let bot_token_value: SlackApiTokenValue = config.bot_token.clone().into();
    let bot_token = SlackApiToken::new(bot_token_value);
    let app_token_value: SlackApiTokenValue = config.app_token.clone().into();
    let app_token = SlackApiToken::new(app_token_value);

    // Fetch our own user id once so the event handler can detect
    // `<@UBOTID>` mentions and strip them before forwarding to the
    // agent. If `auth.test` fails (rate-limited, scope missing) we
    // degrade to the conservative `!is_group` fallback.
    let bot_user_id = {
        let session = client.open_session(&bot_token);
        match session.auth_test().await {
            Ok(response) => Some(response.user_id),
            Err(error) => {
                eprintln!(
                    "slack connector: auth.test failed; mention detection will degrade: {error}"
                );
                None
            }
        }
    };

    let ctx = HandlerCtx {
        runtime,
        config: Arc::new(config),
        bot_token,
        bot_user_id,
    };

    // Stash the handler context in slack-morphism's user-state bag so
    // the callbacks can reach it without the usual lifetimes headache.
    let environment = Arc::new(
        SlackClientEventsListenerEnvironment::new(client.clone())
            .with_error_handler(slack_error_handler)
            .with_user_state(ctx),
    );

    let callbacks = SlackSocketModeListenerCallbacks::new().with_push_events(on_push_event);

    let listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        environment.clone(),
        callbacks,
    );

    listener
        .listen_for(&app_token)
        .await
        .context("failed to start slack socket mode listener")?;

    // `serve` runs until the socket closes; wrap it in a select with
    // the shutdown signal so we can cancel cleanly from outside.
    tokio::select! {
        _ = listener.serve() => {}
        _ = shutdown_rx => {}
    }
    Ok(())
}

fn slack_error_handler(
    err: Box<dyn std::error::Error + Send + Sync>,
    _client: Arc<SlackHyperClient>,
    _states: SlackClientEventsUserState,
) -> HttpStatusCode {
    eprintln!("slack connector: listener error: {err}");
    HttpStatusCode::OK
}

/// Entry point registered with slack-morphism for every inbound push
/// event. We filter down to `Message` events, convert them, hand them to
/// the handler, and post the reply.
async fn on_push_event(
    event: SlackPushEventCallback,
    client: Arc<SlackHyperClient>,
    states: SlackClientEventsUserState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ctx = {
        let guard = states.read().await;
        guard.get_user_state::<HandlerCtx>().cloned()
    };
    let Some(ctx) = ctx else {
        return Ok(());
    };

    let SlackEventCallbackBody::Message(message_event) = event.event else {
        return Ok(());
    };

    let Some(inbound) = inbound_from(&message_event, ctx.bot_user_id.as_ref()) else {
        return Ok(());
    };

    let reply_target = SlackReplyTarget {
        channel: inbound_channel(&message_event),
        thread_ts: message_event.origin.thread_ts.clone(),
    };

    let runtime = ctx.runtime.clone();
    let config = ctx.config.clone();
    let outcome =
        tokio::task::spawn_blocking(move || handle_command(&runtime, &inbound, &config)).await?;

    let reply_text = match outcome {
        Ok(CommandOutcome::Ignored) => return Ok(()),
        Ok(CommandOutcome::Reply(text)) => text,
        Ok(CommandOutcome::AgentReply { text, .. }) => text,
        Err(error) => format!("Puffer error: {error}"),
    };

    if let Some(channel) = reply_target.channel {
        send_reply_chunks(
            &client,
            &ctx.bot_token,
            &channel,
            reply_target.thread_ts,
            &reply_text,
        )
        .await?;
    }
    Ok(())
}

/// The bits of a [`SlackMessageEvent`] we pass to the sending side.
struct SlackReplyTarget {
    channel: Option<SlackChannelId>,
    thread_ts: Option<SlackTs>,
}

fn inbound_channel(event: &SlackMessageEvent) -> Option<SlackChannelId> {
    event.origin.channel.clone()
}

fn inbound_from(
    event: &SlackMessageEvent,
    bot_user_id: Option<&SlackUserId>,
) -> Option<InboundMessage> {
    // Skip message subtypes we don't care about (edits, deletes, joins,
    // channel topic changes). The default `None` subtype is a regular
    // user message, which is the only thing we act on.
    if event.subtype.is_some() {
        return None;
    }

    let channel = event.origin.channel.as_ref()?.to_string();
    let raw_text = event
        .content
        .as_ref()
        .and_then(|c| c.text.clone())
        .unwrap_or_default();

    let user_id = event.sender.user.as_ref().map(|u| u.to_string());
    let thread_ts = event.origin.thread_ts.as_ref().map(|ts| ts.to_string());
    let is_group = !matches!(
        event.origin.channel_type,
        Some(SlackChannelType(ref t)) if t == "im"
    );

    // Bot-self: `sender.bot_id` is set when Slack attributes the
    // message to any app; `sender.user == bot_user_id` is the stricter
    // check that covers only *our* bot's outgoing messages.
    let sender_matches_bot = match (event.sender.user.as_ref(), bot_user_id) {
        (Some(sender), Some(bot)) => sender == bot,
        _ => false,
    };
    let from_bot = event.sender.bot_id.is_some() || sender_matches_bot;

    // Mention detection + stripping: Slack encodes a user mention as
    // `<@UBOTID>`. We search + strip that specific token so the agent
    // sees the prose without the bot reference.
    let (bot_mentioned, cleaned_text) = match bot_user_id {
        Some(bot) => {
            let token = format!("<@{bot}>");
            let mentioned = raw_text.contains(&token);
            let cleaned = raw_text.replace(&token, "");
            (!is_group || mentioned, collapse_whitespace(&cleaned))
        }
        None => (!is_group, raw_text.trim().to_string()),
    };

    Some(InboundMessage {
        conversation_id: channel,
        user_id,
        text: cleaned_text,
        thread_id: thread_ts,
        is_group,
        bot_mentioned,
        from_bot,
    })
}

/// Collapses any run of whitespace the mention stripping may have left
/// behind and trims the ends.
fn collapse_whitespace(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn send_reply_chunks(
    client: &Arc<SlackHyperClient>,
    bot_token: &SlackApiToken,
    channel: &SlackChannelId,
    thread_ts: Option<SlackTs>,
    body: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let session = client.open_session(bot_token);
    for chunk in MessageSplitter::SLACK.split(body) {
        send_chunk_with_retry(&session, channel, thread_ts.clone(), &chunk).await?;
    }
    Ok(())
}

async fn send_chunk_with_retry<'a>(
    session: &SlackClientSession<'a, SlackClientHyperHttpsConnector>,
    channel: &SlackChannelId,
    thread_ts: Option<SlackTs>,
    chunk: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    const MAX_ATTEMPTS: u32 = 3;
    let mut attempt = 0u32;
    loop {
        let content = SlackMessageContent::new().with_text(chunk.to_string());
        let mut request = SlackApiChatPostMessageRequest::new(channel.clone(), content);
        if let Some(ts) = thread_ts.clone() {
            request = request.with_thread_ts(ts);
        }
        match session.chat_post_message(&request).await {
            Ok(_) => return Ok(()),
            Err(error) => {
                attempt += 1;
                if attempt >= MAX_ATTEMPTS {
                    return Err(error.into());
                }
                let delay_ms = 250u64 * (1u64 << (attempt - 1));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_connector_core::GroupKeyPolicy;

    #[test]
    fn collapse_whitespace_strips_doubled_spaces_and_trims() {
        assert_eq!(collapse_whitespace("hey   there  "), "hey there");
        assert_eq!(collapse_whitespace(""), "");
        assert_eq!(collapse_whitespace("  x\t\ny  "), "x y");
    }

    fn sample_config() -> SlackConfig {
        SlackConfig {
            bot_token: "xoxb-t".to_string(),
            app_token: "xapp-t".to_string(),
            allowed_users: Vec::new(),
            welcome_message: None,
            require_mention: true,
            group_key_policy: GroupKeyPolicy::PerUser,
        }
    }

    #[test]
    fn connector_reports_platform_id() {
        let connector = SlackConnector::new(sample_config());
        assert_eq!(connector.id(), PLATFORM_ID);
    }

    #[test]
    fn missing_bot_token_is_reported() {
        let mut config = sample_config();
        config.bot_token = "".to_string();
        let connector = Box::new(SlackConnector::new(config));
        let runtime = dummy_runtime();
        match connector.start(runtime) {
            Err(ConnectorStartError::MissingConfig { detail, .. }) => {
                assert!(detail.contains("bot_token"));
            }
            Err(other) => panic!("expected MissingConfig, got {other:?}"),
            Ok(_) => panic!("expected start to fail without bot_token"),
        }
    }

    #[test]
    fn missing_app_token_is_reported() {
        let mut config = sample_config();
        config.app_token = "".to_string();
        let connector = Box::new(SlackConnector::new(config));
        let runtime = dummy_runtime();
        match connector.start(runtime) {
            Err(ConnectorStartError::MissingConfig { detail, .. }) => {
                assert!(detail.contains("app_token"));
            }
            Err(other) => panic!("expected MissingConfig, got {other:?}"),
            Ok(_) => panic!("expected start to fail without app_token"),
        }
    }

    fn dummy_runtime() -> Arc<ConnectorRuntime> {
        use puffer_config::{ConfigPaths, PufferConfig};
        use puffer_connector_core::{ConnectorRuntimeConfig, ConversationSessionMap};
        use puffer_provider_registry::{AuthStore, ProviderRegistry};
        use puffer_resources::LoadedResources;
        use puffer_session_store::SessionStore;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
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
}
