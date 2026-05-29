//! Email subscriber skill.
//!
//! This crate is compiled into the Puffer binary and reached via the hidden
//! `puffer __subscriber email` subcommand. It polls IMAP for new mail, emits
//! one ndjson [`Event`](puffer_subscriber_runtime::Event) per new message on
//! stdout, reads ndjson [`SubscriberCommand`](puffer_subscriber_runtime::SubscriberCommand)
//! values on stdin, and sends outbound mail over SMTP when it receives a
//! `SendMessage` command.
//!
//! Design notes:
//! * Stdout is reserved for ndjson event lines consumed by the subscriber
//!   runtime bus. Diagnostics go to stderr via `tracing`.
//! * Persistent state (config + high-water UID) lives under
//!   `$PUFFER_SKILL_STATE_DIR`, falling back to `./state` if the env var is
//!   not set. The supervisor is expected to set the env var.
//! * Configuration arrives at runtime via an
//!   [`EmailConfigure`](puffer_subscriber_runtime::SubscriberCommand::EmailConfigure)
//!   command; on first boot with no saved config the skill emits a
//!   `config_required` control event and waits on stdin.

mod commands;
mod config;
mod events;
mod imap_poll;
mod run;
mod smtp_send;
mod state;

pub use crate::run::run;

/// Narrow re-exports for external callers (see `puffer-cli`'s `connect
/// email configure`). Keeps the rest of `config` (`load`, `config_path`,
/// `is_valid`, port constants) module-internal.
pub use crate::config::{save as save_email_config, EmailConfig};
