//! MCP transport abstraction.
//!
//! The runner uses [`rmcp`]'s [`Transport`](rmcp::transport::Transport) trait
//! at the wire boundary, so the rest of the connection manager only sees a
//! single concrete type per transport variant. Today only stdio ships
//! ([`StdioTransport`]); HTTP / in-memory variants will plug in alongside
//! without touching `McpHost`.
//!
//! A future-friendly enum approach is preferred over a `Box<dyn Transport>`
//! because rmcp's `serve_client` is generic over the concrete transport type
//! (it only needs `Transport<RoleClient> + 'static`). Each transport variant
//! is therefore wired into the connection manager through a small
//! [`TransportRecipe`] enum that knows how to launch its concrete
//! [`Transport`] on demand. Pass 1.5a only carries the `Stdio` variant.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Spec for a stdio MCP server before the process is actually spawned.
///
/// Kept separate from the live process / transport handle so the connection
/// manager can re-launch on crash without losing the configuration.
#[derive(Debug, Clone)]
pub struct StdioTransportSpec {
    /// The program to execute (first token of the configured command line).
    pub program: String,
    /// Argv passed to the program, excluding `program` itself.
    pub args: Vec<String>,
    /// Extra environment variables merged on top of the inherited env.
    pub env: BTreeMap<String, String>,
    /// Working directory the child should start in. `None` means inherit
    /// from the parent (typical for in-process tests).
    pub cwd: Option<PathBuf>,
}

/// All supported MCP transport recipes.
///
/// Pass 1.5a only carries the `Stdio` variant; HTTP / in-memory transports
/// land in 1.5b+ without touching the connection-manager API.
#[derive(Debug, Clone)]
pub enum TransportRecipe {
    Stdio(StdioTransportSpec),
}

impl TransportRecipe {
    /// Returns a short human-readable label for log lines.
    pub fn kind(&self) -> &'static str {
        match self {
            TransportRecipe::Stdio(_) => "stdio",
        }
    }
}
