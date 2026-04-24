//! Slack connector for Puffer. Bridges inbound Slack messages into a
//! running Puffer process and sends the assistant's reply back to the
//! channel (or thread).
//!
//! Built on slack-morphism's Socket Mode listener so the bot does not
//! need a public HTTPS endpoint — only a bot token (`xoxb-…`) and an
//! app-level token (`xapp-…`) with `connections:write`.
//!
//! See [`puffer_connector_core`] for the shared conversation→session
//! bridge and the built-in `/help`/`/new`/`/status`/`/usage` commands;
//! this crate layers on Slack-specific concerns:
//! * bot-self filtering via `sender.bot_id` so we never loop on our own
//!   outgoing messages
//! * mention-gating in channels (group chats) via `require_mention`
//! * thread-scoped sessions: inbound messages with a `thread_ts` key by
//!   `{channel_id}:{thread_ts}` so a single channel can host multiple
//!   parallel Puffer conversations
//! * replies posted with `chat.postMessage`, preserving `thread_ts` so
//!   threaded conversations stay threaded
//! * [`MessageSplitter::SLACK`](puffer_connector_core::MessageSplitter::SLACK)
//!   chunking for long replies plus bounded exponential-backoff retries

mod config;
mod connector;
mod handler;

pub use config::SlackConfig;
pub use connector::SlackConnector;
pub use handler::handle_command;
pub use puffer_connector_core::{CommandOutcome, InboundMessage};
