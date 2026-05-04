//! gRPC transport for the puffer `ToolRunner` trait.
//!
//! This crate is split into two halves:
//!
//! * [`client`] — `RemoteToolRunner`, a `ToolRunner` impl that forwards every
//!   call over a tonic gRPC connection. The rest of the puffer codebase only
//!   needs this half.
//! * [`server`] — generated server bindings + a thin service wrapper
//!   ([`server::ToolRunnerService`]) that adapts an arbitrary `Arc<dyn
//!   ToolRunner>` into the tonic service. Used by the standalone
//!   `puffer-tool-runner` binary and by integration tests that bring up a
//!   server in-process.
//!
//! `puffer-runner-api` stays gRPC-free; conversions between proto types and
//! the api DTOs live in [`convert`].

pub mod client;
pub mod convert;
pub mod proto;
pub mod server;

pub use client::RemoteToolRunner;
pub use server::{BidiElicitationRouter, ToolRunnerService};

/// Bearer-token metadata key. Exposed so callers can attach the same key
/// when wiring custom interceptors.
pub const AUTH_METADATA_KEY: &str = "authorization";
