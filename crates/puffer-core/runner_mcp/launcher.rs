//! Spawn stdio MCP server processes and hand them to rmcp.
//!
//! Adapted from `references/codex/codex-rs/rmcp-client/src/stdio_server_launcher.rs`,
//! but trimmed to the puffer-only needs:
//!
//! * No executor backend (codex's remote process API doesn't apply here).
//! * Env policy is manifest-controlled: legacy manifests inherit the parent
//!   environment, while verified-skill manifests can request a safe baseline
//!   plus explicit env only.
//! * Lifecycle is owned by the connection manager, so this module just builds
//!   a [`TokioChildProcess`] and a stderr forwarder.
//!
//! `rmcp::transport::TokioChildProcess` already implements
//! `rmcp::transport::Transport<RoleClient>`, so the connection manager can
//! pass it straight to `rmcp::service::serve_client` once it's spawned.

use std::process::Stdio;

use rmcp::transport::child_process::TokioChildProcess;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{info, warn};

use super::transport::{expand_env, StdioTransportSpec};

const SAFE_STDIO_ENV_KEYS: &[&str] = &[
    "PATH", "HOME", "USER", "LANG", "LC_ALL", "TERM", "SHELL", "TMPDIR",
];

/// Build and spawn a child-process transport for one stdio MCP server.
///
/// The returned [`TokioChildProcess`] both owns the child handle (with
/// `kill_on_drop(true)`) and implements `Transport<RoleClient>` so the
/// connection manager can hand it straight to rmcp's `serve_client`.
///
/// `server_id` is used purely for log prefixes on the stderr forwarder.
pub(crate) fn spawn_stdio_child(
    server_id: &str,
    spec: &StdioTransportSpec,
) -> std::io::Result<TokioChildProcess> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    if !spec.inherit_env {
        command.env_clear();
        for key in SAFE_STDIO_ENV_KEYS {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for (key, value) in std::env::vars() {
            if key.starts_with("XDG_") {
                command.env(key, value);
            }
        }
    }
    for (key, value) in &spec.env {
        command.env(key, expand_env(value));
    }

    let (transport, stderr) = TokioChildProcess::builder(command)
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(stderr) = stderr {
        let id = server_id.to_string();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => info!(target = "puffer::mcp", server = %id, "stderr: {line}"),
                    Ok(None) => break,
                    Err(error) => {
                        warn!(target = "puffer::mcp", server = %id, "stderr read failed: {error}");
                        break;
                    }
                }
            }
        });
    }

    Ok(transport)
}
