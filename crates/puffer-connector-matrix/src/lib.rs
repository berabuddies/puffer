//! Matrix connector for Puffer. Bridges inbound Matrix room messages into
//! a running Puffer process and sends the assistant's reply back to the
//! room.
//!
//! Backed by [`matrix-sdk`](https://docs.rs/matrix-sdk). See
//! [`puffer_connector_core`] for the shared conversation→session bridge
//! and the built-in `/help`/`/new`/`/status`/`/usage` commands; this
//! crate provides:
//! * sync-based inbound listener with graceful shutdown
//! * bot-self filtering so we never loop on our own outgoing messages
//! * localpart/MXID substring mention detection for group rooms
//! * [`MessageSplitter::MATRIX`](puffer_connector_core::MessageSplitter::MATRIX)
//!   chunking for long replies plus bounded exponential-backoff retries
//!
//! E2EE rooms are intentionally out of scope for v1: the `matrix-sdk`
//! default features (sqlite + olm/e2ee) are disabled in `Cargo.toml` to
//! keep the build lean. Supporting encrypted rooms would re-enable the
//! `e2e-encryption` / `sqlite` features and layer a crypto store on top.

mod config;
mod connector;
mod handler;

pub use config::MatrixConfig;
pub use connector::MatrixConnector;
pub use handler::handle_command;
pub use puffer_connector_core::{CommandOutcome, InboundMessage};
