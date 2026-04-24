//! Telegram connector for Puffer. Bridges inbound chat messages into a
//! running Puffer process and sends the assistant's reply back to the
//! user.
//!
//! Backed by [`teloxide`](https://docs.rs/teloxide) for the bot SDK.
//! See [`puffer_connector_core`] for the shared conversation→session
//! bridge and the built-in `/help`/`/new`/`/status`/`/usage` commands;
//! this crate provides:
//! * polling-based inbound listener with graceful shutdown
//! * bot-self filtering so we never loop on our own outgoing messages
//! * `@botname` and reply-to-bot mention detection for groups
//! * [`MessageSplitter::TELEGRAM`](puffer_connector_core::MessageSplitter::TELEGRAM)
//!   chunking for long replies plus bounded exponential-backoff retries

mod config;
mod connector;
mod handler;

pub use config::TelegramConfig;
pub use connector::TelegramConnector;
pub use handler::handle_command;
pub use puffer_connector_core::{CommandOutcome, InboundMessage};
