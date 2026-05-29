//! MCP transport abstraction.
//!
//! The runner uses [`rmcp`]'s [`Transport`](rmcp::transport::Transport) trait
//! at the wire boundary, so the rest of the connection manager only sees a
//! single concrete type per transport variant. Today the runner ships two
//! transports — stdio (subprocess) and `streamable-http` (rmcp's official
//! HTTP+SSE transport) — and in-memory / WebSocket variants can plug in
//! alongside them without touching `McpHost`.
//!
//! A future-friendly enum approach is preferred over a `Box<dyn Transport>`
//! because rmcp's `serve_client` is generic over the concrete transport type
//! (it only needs `Transport<RoleClient> + 'static`). Each transport variant
//! is therefore wired into the connection manager through a small
//! [`TransportRecipe`] enum that knows how to launch its concrete
//! [`Transport`] on demand.
//!
//! ## Manifest YAML examples
//!
//! Stdio MCP server (subprocess, default):
//!
//! ```yaml
//! id: filesystem-stub
//! transport: stdio
//! target: 'mcp-filesystem-server --root /workspace'
//! ```
//!
//! HTTP / streamable-HTTP MCP server (token auth via static header):
//!
//! ```yaml
//! id: github-hosted
//! transport: http
//! target: https://mcp.example.com/v1
//! headers:
//!   Authorization: "Bearer ${GITHUB_MCP_TOKEN}"
//!   X-Tenant: "puffer"
//! ```
//!
//! `${VAR}` and `$VAR` placeholders inside header values are expanded from
//! the runner process environment by [`expand_env`]; missing variables turn
//! into empty strings (with a warning logged) so a typo in `target` doesn't
//! silently leak the literal placeholder over the wire as an "Authorization"
//! token. OAuth flows land in pass 1.5e and slot in alongside the static
//! header path here.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

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
    /// Whether the child inherits the full Puffer process environment.
    pub inherit_env: bool,
    /// Per-tool-call timeout for requests routed to this server.
    pub timeout: Duration,
    /// Timeout for the initial MCP initialize handshake.
    pub connect_timeout: Duration,
    /// Working directory the child should start in. `None` means inherit
    /// from the parent (typical for in-process tests).
    pub cwd: Option<PathBuf>,
}

/// Spec for an HTTP (streamable-HTTP) MCP server before the underlying
/// rmcp transport is constructed.
///
/// `headers` carries arbitrary user-supplied header pairs (Authorization,
/// X-API-Key, tenant routing, etc.). Pass 1.5d does *not* implement OAuth
/// — that's pass 1.5e — so the only authentication mechanism here is
/// "the user pasted a static token into the manifest". Header values are
/// run through [`expand_env`] before reaching the wire.
#[derive(Debug, Clone)]
pub struct HttpTransportSpec {
    /// Absolute URL of the streamable-HTTP MCP endpoint
    /// (e.g. `https://mcp.example.com/v1`).
    pub url: String,
    /// Extra HTTP headers attached to every outbound request. Order is
    /// preserved so test fixtures can pin a deterministic header order.
    pub headers: Vec<(String, String)>,
    /// OAuth 2.0 configuration for this server (pass 1.5e). When `Some`,
    /// the runner wraps the reqwest client in rmcp's `AuthClient` and
    /// drives discovery / DCR / token refresh on demand. The token
    /// directory is supplied separately by the connection manager so
    /// tests can pin it to a `TempDir` without round-tripping through
    /// the manifest.
    pub oauth: Option<HttpOAuthSpec>,
    /// Per-tool-call timeout for requests routed to this server.
    pub timeout: Duration,
    /// Timeout for HTTP connection setup and the MCP initialize handshake.
    pub connect_timeout: Duration,
}

/// Per-server OAuth runtime state derived from the manifest.
///
/// Mirrors [`puffer_resources::McpOAuthSpec`] but in a transport-shaped
/// form (no untagged enum, no serde) so the connection manager can hand
/// it to the OAuth service without re-deriving the scope list at every
/// reconnect.
#[derive(Debug, Clone)]
pub struct HttpOAuthSpec {
    /// Scopes to request during the auth-code flow.
    pub scopes: Vec<String>,
    /// `client_name` to register with the auth server during DCR.
    pub client_name: String,
}

/// All supported MCP transport recipes.
///
/// Pass 1.5a shipped `Stdio`; pass 1.5d adds `Http`. Anything beyond that
/// (in-memory transport, WebSocket, OAuth-fronted HTTP) plugs in here
/// without touching the connection-manager API.
#[derive(Debug, Clone)]
pub enum TransportRecipe {
    Stdio(StdioTransportSpec),
    Http(HttpTransportSpec),
}

impl TransportRecipe {
    /// Returns a short human-readable label for log lines.
    pub fn kind(&self) -> &'static str {
        match self {
            TransportRecipe::Stdio(_) => "stdio",
            TransportRecipe::Http(_) => "http",
        }
    }

    /// Returns the per-tool-call timeout configured for this server.
    pub fn timeout(&self) -> Duration {
        match self {
            TransportRecipe::Stdio(spec) => spec.timeout,
            TransportRecipe::Http(spec) => spec.timeout,
        }
    }
}

/// Expand `$VAR` and `${VAR}` placeholders in `input` against the process
/// environment.
///
/// * Missing variables become empty strings (and emit a `tracing::warn`)
///   rather than substituting the literal placeholder. The latter would
///   ship a string like `"Bearer ${GITHUB_TOKEN}"` over the wire as an
///   actual auth header — confusing at best, a credential-exfiltration
///   footgun at worst.
/// * `$$` is treated as an escaped literal `$`.
/// * Anything else (including bare `$` followed by punctuation) is passed
///   through unchanged.
///
/// Used by [`crate::runner_mcp::connection_manager::entry_from_spec`] to
/// expand header values for HTTP MCP servers; kept in the same module as
/// the recipe enum so future transport variants (e.g. WebSocket) can reach
/// for the same helper.
pub fn expand_env(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                // `$$` -> literal `$`
                let _ = chars.next();
                out.push('$');
            }
            Some('{') => {
                let _ = chars.next();
                let mut name = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '}' {
                        closed = true;
                        break;
                    }
                    name.push(inner);
                }
                if !closed {
                    // Unterminated `${...` — pass through unchanged so we
                    // don't mangle pre-existing strings that just happen
                    // to contain a `$`.
                    out.push('$');
                    out.push('{');
                    out.push_str(&name);
                    continue;
                }
                push_env_value(&mut out, &name);
            }
            Some(c) if c.is_ascii_alphabetic() || *c == '_' => {
                let mut name = String::new();
                while let Some(c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || *c == '_' {
                        name.push(*c);
                        let _ = chars.next();
                    } else {
                        break;
                    }
                }
                push_env_value(&mut out, &name);
            }
            _ => out.push('$'),
        }
    }
    out
}

fn push_env_value(out: &mut String, name: &str) {
    if name.is_empty() {
        return;
    }
    match std::env::var(name) {
        Ok(value) => out.push_str(&value),
        Err(_) => {
            tracing::warn!(
                target = "puffer::mcp",
                "manifest references undefined env var `{name}`; substituting empty string"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::expand_env;

    fn with_env<R>(key: &str, value: Option<&str>, run: impl FnOnce() -> R) -> R {
        // SAFETY: tests run serially via `cargo test --test ...` per
        // integration-test binary; we still snapshot+restore the variable
        // for hygiene.
        let prior = std::env::var(key).ok();
        match value {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
        let out = run();
        match prior {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
        out
    }

    #[test]
    fn expands_braced_variable() {
        with_env("PUFFER_MCP_EXPAND_TEST", Some("xyz"), || {
            assert_eq!(expand_env("Bearer ${PUFFER_MCP_EXPAND_TEST}"), "Bearer xyz");
        });
    }

    #[test]
    fn expands_bare_variable() {
        with_env("PUFFER_MCP_EXPAND_BARE", Some("abc"), || {
            assert_eq!(expand_env("$PUFFER_MCP_EXPAND_BARE!"), "abc!");
        });
    }

    #[test]
    fn missing_variable_becomes_empty() {
        with_env("PUFFER_MCP_EXPAND_MISSING", None, || {
            assert_eq!(expand_env("Bearer ${PUFFER_MCP_EXPAND_MISSING}"), "Bearer ");
        });
    }

    #[test]
    fn double_dollar_is_literal() {
        assert_eq!(expand_env("$$VAR"), "$VAR");
    }

    #[test]
    fn unterminated_brace_is_passed_through() {
        assert_eq!(expand_env("${UNCLOSED"), "${UNCLOSED");
    }

    #[test]
    fn lone_dollar_is_passed_through() {
        assert_eq!(expand_env("price: $5.00"), "price: $5.00");
    }
}
