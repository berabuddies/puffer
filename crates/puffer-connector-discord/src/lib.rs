//! Discord connector for Puffer. Bridges inbound chat messages into a
//! running Puffer process and sends the assistant's reply back to the
//! user.
//!
//! Backed by [`serenity`](https://docs.rs/serenity) for the bot SDK.
//! See [`puffer_connector_core`] for the shared conversation→session
//! bridge and the built-in `/help`/`/new`/`/status`/`/usage` commands;
//! this crate provides:
//! * gateway-based inbound listener with graceful shard shutdown
//! * bot-self filtering so we never loop on our own outgoing messages
//! * `<@botid>` mention detection (and stripping) for guild channels
//! * [`MessageSplitter::DISCORD`](puffer_connector_core::MessageSplitter::DISCORD)
//!   chunking for long replies plus bounded exponential-backoff retries

mod config;
mod connector;
mod handler;

pub use config::DiscordConfig;
pub use connector::DiscordConnector;
pub use handler::handle_command;
pub use puffer_connector_core::{CommandOutcome, InboundMessage};
