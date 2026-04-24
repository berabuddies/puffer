use crate::handler::{handle_command, PLATFORM_ID};
use crate::DiscordConfig;
use anyhow::{Context as AnyhowContext, Result};
use puffer_connector_core::{
    CommandOutcome, Connector, ConnectorHandle, ConnectorRuntime, ConnectorStartError,
    InboundMessage, MessageSplitter,
};
use serenity::async_trait;
use serenity::client::{Client, Context, EventHandler};
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::model::id::UserId;
use serenity::prelude::GatewayIntents;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// Discord connector ready to be started by the puffer connector hub.
pub struct DiscordConnector {
    config: DiscordConfig,
}

impl DiscordConnector {
    pub fn new(config: DiscordConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &DiscordConfig {
        &self.config
    }
}

impl Connector for DiscordConnector {
    fn id(&self) -> &str {
        PLATFORM_ID
    }

    fn start(
        self: Box<Self>,
        runtime: Arc<ConnectorRuntime>,
    ) -> Result<ConnectorHandle, ConnectorStartError> {
        if self.config.token.trim().is_empty() {
            return Err(ConnectorStartError::MissingConfig {
                id: PLATFORM_ID.to_string(),
                detail: "bot token is empty".to_string(),
            });
        }

        // One-shot channel that fires serenity's cooperative shutdown.
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        // Signal back from the worker thread once the Tokio runtime is
        // ready to be interrupted.
        let (ready_tx, ready_rx) = mpsc::channel::<()>();

        let config = self.config.clone();
        let join = std::thread::Builder::new()
            .name("puffer-connector-discord".to_string())
            .spawn(move || -> Result<()> {
                let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .context("failed to build tokio runtime for discord connector")?;
                let _ = ready_tx.send(());
                tokio_runtime.block_on(run_dispatcher(config, runtime, shutdown_rx))
            })
            .map_err(|error| ConnectorStartError::other(PLATFORM_ID, error.into()))?;

        // Wait for the worker to stand up before returning. `recv` times
        // out on thread death, which we surface as a start error.
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

async fn run_dispatcher(
    config: DiscordConfig,
    runtime: Arc<ConnectorRuntime>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let handler = Handler {
        runtime: runtime.clone(),
        config: config.clone(),
    };

    let mut client = Client::builder(&config.token, intents)
        .event_handler(handler)
        .await
        .context("failed to build discord client")?;

    let shard_manager = client.shard_manager.clone();
    tokio::spawn(async move {
        if shutdown_rx.await.is_ok() {
            shard_manager.shutdown_all().await;
        }
    });

    if let Err(error) = client.start().await {
        return Err(anyhow::anyhow!("discord client error: {error}"));
    }
    Ok(())
}

struct Handler {
    runtime: Arc<ConnectorRuntime>,
    config: DiscordConfig,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, message: Message) {
        // Resolve our own bot user id so we can filter self-echoes and
        // detect raw `<@id>` mention tokens in the text body.
        let current_user_id: UserId = match ctx.http.get_current_user().await {
            Ok(user) => user.id,
            Err(error) => {
                eprintln!("discord connector: failed to fetch bot identity: {error}");
                return;
            }
        };

        // Short-circuit on our own outgoing messages before we spend a
        // `spawn_blocking` round-trip. Discord routes the bot's own
        // posts back through `message` events for guilds the bot is in.
        if message.author.bot && message.author.id == current_user_id {
            return;
        }

        // Discord embeds mentions as `<@id>` or `<@!id>` tokens inside
        // message.content. Strip ours so the agent sees clean prose, and
        // use presence of the token as a fallback mention signal.
        let mention_token = format!("<@{}>", current_user_id.get());
        let nick_mention_token = format!("<@!{}>", current_user_id.get());
        let raw_text = message.content.clone();
        let text_contains_mention =
            raw_text.contains(&mention_token) || raw_text.contains(&nick_mention_token);
        let cleaned_text = raw_text
            .replace(&mention_token, "")
            .replace(&nick_mention_token, "")
            .trim()
            .to_string();

        let bot_mentioned = message.mentions_user_id(current_user_id) || text_contains_mention;
        // Guild messages are group-like; DMs (no guild id) are 1:1.
        let is_group = message.guild_id.is_some();

        // Thread detection: Discord threads are modeled as channels
        // whose `parent_id` points at the parent text channel. We look
        // up the channel once to check, but fall back to `None` on
        // errors (no lookup permission, API transient failure).
        let thread_id = match message.channel_id.to_channel(&ctx.http).await {
            Ok(serenity::all::Channel::Guild(guild_channel)) => {
                use serenity::model::channel::ChannelType;
                if matches!(
                    guild_channel.kind,
                    ChannelType::PublicThread
                        | ChannelType::PrivateThread
                        | ChannelType::NewsThread
                ) {
                    Some(guild_channel.id.get().to_string())
                } else {
                    None
                }
            }
            _ => None,
        };

        let inbound = InboundMessage {
            conversation_id: message.channel_id.get().to_string(),
            user_id: Some(message.author.id.get().to_string()),
            text: cleaned_text,
            thread_id,
            is_group,
            bot_mentioned,
            from_bot: false, // early-returned above
        };

        let runtime = self.runtime.clone();
        let config = self.config.clone();

        // The dispatch itself blocks on the shared runtime mutex. Park on
        // a blocking worker so we don't starve the tokio reactor.
        let outcome = tokio::task::spawn_blocking(move || {
            handle_command(&runtime, &inbound, &config)
        })
        .await;

        let reply = match outcome {
            Ok(Ok(CommandOutcome::Ignored)) => return,
            Ok(Ok(CommandOutcome::Reply(text))) => text,
            Ok(Ok(CommandOutcome::AgentReply { text, .. })) => text,
            Ok(Err(error)) => format!("Puffer error: {error}"),
            Err(error) => format!("Puffer error: {error}"),
        };

        if let Err(error) = send_reply_chunks(&ctx, &message, &reply).await {
            eprintln!("discord connector: failed to send reply: {error}");
        }
    }

    async fn ready(&self, _: Context, _ready: Ready) {
        // Intentionally silent — the connector hub logs start/stop.
    }
}

async fn send_reply_chunks(
    ctx: &Context,
    message: &Message,
    body: &str,
) -> Result<(), serenity::Error> {
    for chunk in MessageSplitter::DISCORD.split(body) {
        send_with_retry(ctx, message, &chunk).await?;
    }
    Ok(())
}

/// Sends a single chunk with exponential backoff. Retries up to
/// `MAX_ATTEMPTS` times on transient errors before giving up. Matches
/// the telegram connector's `send_with_retry` shape so operators get
/// the same resiliency everywhere.
async fn send_with_retry(
    ctx: &Context,
    message: &Message,
    chunk: &str,
) -> Result<(), serenity::Error> {
    const MAX_ATTEMPTS: u32 = 3;
    let mut attempt = 0u32;
    loop {
        match message.channel_id.say(&ctx.http, chunk).await {
            Ok(_) => return Ok(()),
            Err(error) => {
                attempt += 1;
                if attempt >= MAX_ATTEMPTS {
                    return Err(error);
                }
                // Exponential backoff with a tiny jitter budget.
                let delay_ms = 250u64 * (1u64 << (attempt - 1));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}
