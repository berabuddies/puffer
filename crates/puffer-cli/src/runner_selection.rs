//! Picks a `ToolRunner` implementation for `AppState` based on the layered
//! `PufferConfig`.
//!
//! When `config.remote_runner` is `Some`, we attempt to connect a
//! [`puffer_runner_grpc::RemoteToolRunner`]. On failure we log the reason and
//! fall back to the in-process `LocalToolRunner` so the binary stays usable
//! even if the operator misconfigures the remote endpoint.

use std::path::PathBuf;
use std::sync::Arc;

use puffer_config::PufferConfig;
use puffer_resources::{plugin_mcp_servers, LoadedResources, McpServerSpec};
use puffer_runner_api::ToolRunner;
use puffer_runner_grpc::RemoteToolRunner;
use puffer_runner_local::LocalToolRunner;

/// Returns the `ToolRunner` instance that `AppState` should use for this
/// process: a remote gRPC runner when `config.remote_runner` is set, or a
/// local in-process runner otherwise. Falls back to local on connect
/// failure so a stale config doesn't brick the binary.
///
/// The local runner is hydrated with the configured MCP server manifest
/// (top-level + plugin-embedded) and the workspace root used by the
/// built-in `filesystem` MCP transport.
pub fn select_tool_runner(
    config: &PufferConfig,
    resources: &LoadedResources,
    workspace_root: PathBuf,
) -> Arc<dyn ToolRunner> {
    if let Some(remote) = config.remote_runner.as_ref() {
        let token = remote.resolve_auth_token();
        match RemoteToolRunner::connect(&remote.endpoint, token.as_deref()) {
            Ok(runner) => return Arc::new(runner),
            Err(err) => {
                eprintln!(
                    "puffer: failed to connect to remote tool runner at {}: {err}; falling back to local",
                    remote.endpoint
                );
            }
        }
    }
    Arc::new(local_runner_from_resources(resources, workspace_root))
}

/// Builds a `LocalToolRunner` configured with the MCP servers loaded into
/// `resources` and rooted at `workspace_root` for the built-in filesystem
/// transport. The sandbox is left wide open so resource loaders can still
/// reach `~/.config/puffer` and other non-workspace paths.
pub fn local_runner_from_resources(
    resources: &LoadedResources,
    workspace_root: PathBuf,
) -> LocalToolRunner {
    let servers = collect_mcp_servers(resources);
    LocalToolRunner::new()
        .with_mcp_servers(servers)
        .with_mcp_workspace_root(workspace_root)
}

fn collect_mcp_servers(resources: &LoadedResources) -> Vec<McpServerSpec> {
    let mut servers: Vec<McpServerSpec> = resources
        .mcp_servers
        .iter()
        .map(|item| item.value.clone())
        .collect();
    for (_plugin, spec) in plugin_mcp_servers(resources) {
        if servers
            .iter()
            .any(|existing| existing.id.eq_ignore_ascii_case(&spec.id))
        {
            continue;
        }
        servers.push(spec.clone());
    }
    servers
}
