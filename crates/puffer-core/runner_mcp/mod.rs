//! MCP server lifecycle + resource/tool dispatch owned by [`LocalToolRunner`].
//!
//! This module hosts:
//!
//! * [`host::McpHost`] — the trait-facing surface that the runner adapter
//!   calls into. It owns the configured server roster, the built-in
//!   filesystem walker, and (lazily) the connection manager that drives real
//!   subprocess MCP servers via [`rmcp`].
//! * [`transport`] — a thin abstraction over MCP transports. Today there is
//!   one impl ([`transport::StdioTransport`]) backed by the `rmcp`
//!   `transport-child-process` feature; HTTP / in-memory transports plug in
//!   alongside it without a rewrite.
//! * [`launcher`] — spawns configured stdio commands as child processes and
//!   hands the resulting transport back to the connection manager.
//! * [`connection_manager`] — owns the long-lived rmcp clients keyed by MCP
//!   server id. Lazy-spawn on first call, crash-respawn with a bounded retry
//!   counter, drop sends `shutdown` to every live server.
//!
//! Anything beyond stdio + `tools/list` + `tools/call` (resources, prompts,
//! HTTP, OAuth, elicitation, sampling, progress streaming) is intentionally
//! deferred — see the pass 1.5b notes in the runner README.

pub mod connection_manager;
pub mod host;
pub mod http_launcher;
pub mod launcher;
pub mod transport;

pub use host::{http_url_for_server, is_live_filesystem_server, McpHost};
