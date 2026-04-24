use crate::handler::{handle_command, PLATFORM_ID};
use crate::TelegramConfig;
use anyhow::{Context, Result};
use puffer_connector_core::{
    Connector, ConnectorHandle, ConnectorRuntime, ConnectorStartError, CommandOutcome,
    InboundMessage, MessageSplitter,
};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::{ChatId, ChatKind};
use tokio::sync::oneshot;

/// Telegram connector ready to be started by the puffer connector hub.
pub struct TelegramConnector {
    config: TelegramConfig,
}

impl TelegramConnector {
    pub fn new(config: TelegramConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &TelegramConfig {
        &self.config
    }
}

impl Connector for TelegramConnector {
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

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<()>();

        let config = self.config.clone();
        let join = std::thread::Builder::new()
            .name("puffer-connector-telegram".to_string())
            .spawn(move || -> Result<()> {
                let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .context("failed to build tokio runtime for telegram connector")?;
                let _ = ready_tx.send(());
                tokio_runtime.block_on(run_dispatcher(config, runtime, shutdown_rx))
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

async fn run_dispatcher(
    config: TelegramConfig,
    runtime: Arc<ConnectorRuntime>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let bot = Bot::new(config.token.clone());

    // Cache our own bot id once so message handlers can filter
    // bot-self updates and detect mentions correctly.
    let me = bot
        .get_me()
        .await
        .context("failed to fetch bot identity from Telegram")?;
    let bot_user_id: u64 = me.id.0;
    let bot_username = me.username.clone().unwrap_or_default();

    let runtime_for_handler = runtime.clone();
    let config_for_handler = config.clone();
    let bot_username_for_handler = bot_username.clone();

    let handler = Update::filter_message().endpoint(
        move |bot: Bot, message: Message| {
            let runtime = runtime_for_handler.clone();
            let config = config_for_handler.clone();
            let bot_username = bot_username_for_handler.clone();
            async move {
                process_message(bot, message, runtime, config, bot_user_id, bot_username).await
            }
        },
    );

    let mut dispatcher = Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .build();

    let shutdown_token = dispatcher.shutdown_token();
    tokio::spawn(async move {
        if shutdown_rx.await.is_ok() {
            let _ = shutdown_token.shutdown();
        }
    });

    dispatcher.dispatch().await;
    Ok(())
}

async fn process_message(
    bot: Bot,
    message: Message,
    runtime: Arc<ConnectorRuntime>,
    config: TelegramConfig,
    bot_user_id: u64,
    bot_username: String,
) -> Result<(), teloxide::RequestError> {
    let chat_id = message.chat.id.0;
    let user_id = message.from.as_ref().map(|u| u.id.0 as i64);

    // Short-circuit on bot-self updates. Telegram re-delivers messages
    // the bot posts in some group configurations; this avoids the
    // spawn_blocking round trip just so the handler can filter them.
    let from_bot = message
        .from
        .as_ref()
        .map(|u| u.is_bot && u.id.0 == bot_user_id)
        .unwrap_or(false);
    if from_bot {
        return Ok(());
    }

    let Some(text) = message.text().map(str::to_string) else {
        return Ok(());
    };

    let is_group = matches!(message.chat.kind, ChatKind::Public(_));

    // Detect explicit mention (`@botname`) or reply-to-bot. Telegram
    // delivers `entities` describing mention ranges, but for the simple
    // `@botname` case matching the lowercase substring is sufficient.
    let mentioned_by_username = !bot_username.is_empty()
        && text
            .to_ascii_lowercase()
            .contains(&format!("@{}", bot_username.to_ascii_lowercase()));
    let replied_to_bot = message
        .reply_to_message()
        .and_then(|m| m.from.as_ref())
        .map(|u| u.id.0 == bot_user_id)
        .unwrap_or(false);
    let bot_mentioned = !is_group || mentioned_by_username || replied_to_bot;

    // Strip our own `@botname` so the agent sees clean prose.
    let cleaned_text = if !bot_username.is_empty() {
        text.replace(&format!("@{}", bot_username), "")
            .trim()
            .to_string()
    } else {
        text.clone()
    };

    let inbound = InboundMessage {
        conversation_id: chat_id.to_string(),
        user_id: user_id.map(|id| id.to_string()),
        text: cleaned_text,
        thread_id: message.thread_id.map(|id| id.0 .0.to_string()),
        is_group,
        bot_mentioned,
        from_bot,
    };

    let outcome_runtime = runtime.clone();
    let outcome_config = config.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        handle_command(&outcome_runtime, &inbound, &outcome_config)
    })
    .await
    .map_err(|error| {
        teloxide::RequestError::from(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            error.to_string(),
        )))
    })?;

    let reply_text = match outcome {
        Ok(CommandOutcome::Ignored) => return Ok(()),
        Ok(CommandOutcome::Reply(text)) => text,
        Ok(CommandOutcome::AgentReply { text, .. }) => text,
        Err(error) => format!("Puffer error: {error}"),
    };

    send_reply_chunks(&bot, ChatId(chat_id), &reply_text).await?;
    Ok(())
}

async fn send_reply_chunks(
    bot: &Bot,
    chat_id: ChatId,
    body: &str,
) -> Result<(), teloxide::RequestError> {
    for chunk in MessageSplitter::TELEGRAM.split(body) {
        send_with_retry(bot, chat_id, &chunk).await?;
    }
    Ok(())
}

/// Sends a single chunk with exponential backoff. Retries up to
/// `MAX_ATTEMPTS` times on transient errors before giving up. Matches
/// Hermes's `_send_with_retry` behavior in spirit.
async fn send_with_retry(
    bot: &Bot,
    chat_id: ChatId,
    chunk: &str,
) -> Result<(), teloxide::RequestError> {
    const MAX_ATTEMPTS: u32 = 3;
    let mut attempt = 0u32;
    loop {
        match bot.send_message(chat_id, chunk.to_string()).await {
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
