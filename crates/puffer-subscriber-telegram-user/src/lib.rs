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
mod notifications;
mod outbound;
mod peers;
mod polls;
mod qr_login;
mod reply;
mod state;

pub use crate::client::run;

/// Narrow re-exports for external callers that drive the login flow
/// directly (see `puffer-cli`'s `connect` subcommand). The internal
/// `login` and `state` modules stay private so unrelated helpers
/// (`PersistedCredentials`, `default_init_params`, …) don't leak.
pub use crate::login::{
    submit_code as login_submit_code, submit_password as login_submit_password, CodeSubmitOutcome,
};
pub use crate::state::{LoginState, SkillEnv};

/// Re-export of the underlying `grammers_client::Client`. Public so
/// callers driving the login flow can hold the in-flight client between
/// successive command calls without depending on `grammers-client`
/// themselves.
pub use grammers_client::Client;

/// Starts a Telegram login attempt. Renamed re-export of
/// `login::start` to avoid the bare `start` symbol leaking through.
pub use crate::login::start as login_start;
