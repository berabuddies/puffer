//! Email connector for Puffer. Bridges inbound email messages into a
//! running Puffer process and sends the assistant's reply back to the
//! sender.
//!
//! Backed by [`async-imap`](https://docs.rs/async-imap) for inbound
//! polling and [`lettre`](https://docs.rs/lettre) for SMTP replies. See
//! [`puffer_connector_core`] for the shared conversation→session
//! bridge and the built-in `/help`/`/new`/`/status`/`/usage` commands;
//! this crate provides:
//! * polling-based IMAP listener with graceful shutdown
//! * bot-self filter (ignores mail where the sender equals the
//!   configured `from_address`) to prevent mailing-list echo loops
//! * `allowed_senders` access control (case-insensitive on the `From:`
//!   address)
//! * threading via `References` / `In-Reply-To` so follow-up replies
//!   land on the same Puffer session
//! * [`MessageSplitter::EMAIL`](puffer_connector_core::MessageSplitter::EMAIL)
//!   chunking for very long replies plus bounded exponential-backoff
//!   retries on SMTP errors

mod config;
mod connector;
mod handler;

pub use config::EmailConfig;
pub use connector::EmailConnector;
pub use handler::handle_command;
pub use puffer_connector_core::{CommandOutcome, InboundMessage};
