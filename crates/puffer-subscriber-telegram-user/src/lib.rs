//! Telegram user-account subscriber skill.
//!
//! This crate is compiled into the Puffer binary and reached via the hidden
//! `puffer __subscriber telegram-user` subcommand. It connects to Telegram via
//! MTProto using the user's personal account (grammers-client 0.7), streams
//! incoming messages from every chat on stdout as ndjson events, and reads
//! control commands on stdin to drive the three-step login flow
//! (phone -> code -> optional 2FA password).
//!
//! Design notes:
//! * Stdout is reserved for ndjson event lines consumed by the subscriber
//!   runtime bus. Diagnostics go to stderr via `tracing`.
//! * The session file lives at `$PUFFER_SKILL_STATE_DIR/telegram.session`,
//!   falling back to `./telegram.session` if the env var is not set. The
//!   supervisor is expected to set the env var for real deployments.
//! * The login flow is driven entirely by inbound
//!   [`SubscriberCommand`](puffer_subscriber_runtime::SubscriberCommand)
//!   lines; there is no terminal prompting.

mod actions;
mod client;
mod commands;
mod delivery;
mod events;
mod import;
mod login;
mod outbound;
mod peers;
mod polls;
mod qr_login;
mod reply;
mod state;

pub use crate::client::run;
