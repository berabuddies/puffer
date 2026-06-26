//! Runtime scaffolding that lets Puffer load "subscriber skills": out-of-process
//! children that speak a simple newline-delimited JSON protocol over stdio and
//! publish events onto an in-process broadcast bus.
//!
//! A subscriber is a directory on disk containing:
//! * `manifest.toml` — declares `id`, `topic`, the `run` command, and optional
//!   state directory.
//! * an executable (any language) invoked by `run` that writes one
//!   [`Event`] per line to stdout and optionally reads one [`Command`] per line
//!   from stdin for control (e.g. login).
//!
//! The runtime is deliberately minimal: it owns process lifecycle (spawn,
//! restart-with-backoff, shutdown), ndjson framing, and topic-keyed fanout.
//! Consumer logic (subscription matching, filtering, actions) lives above this
//! in `puffer-subscriptions`.

mod bus;
mod codec;
mod command;
mod event;
mod manifest;
mod supervisor;

pub use bus::{EventBus, EventReceiver};
pub use codec::{read_lines, write_line};
pub use command::{
    CommandSender, SendAuthorization, SendMediaAttachment, SendMediaKind, SubscriberCommand,
    TelegramPeerKind,
};
pub use event::{Event, EventEnvelope};
pub use manifest::{EnvEntry, Manifest, ManifestError, ManifestKind, StateSpec};
pub use supervisor::{
    resolve_manifest_program, SubscriberHandle, SubscriberSupervisor, SupervisorConfig,
};
