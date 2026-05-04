//! Library surface for the standalone `puffer-tool-runner` binary.
//!
//! Splitting the assembly logic out of `main.rs` lets in-process tests
//! exercise the exact wiring the binary uses (resource discovery, MCP
//! manifest hydration, sandbox configuration) without shelling out to a
//! child process or duplicating the construction in test fixtures.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_resources::load_resources;
use puffer_runner_api::{ElicitationHandler, ToolRunner};
use puffer_runner_grpc::{BidiElicitationRouter, ToolRunnerService};
use puffer_runner_local::{local_runner_from_resources, LocalToolRunner};

/// Discovers MCP server specs from `cwd`'s resource roots and returns a
/// `ToolRunnerService` whose underlying `LocalToolRunner` is hydrated with
/// them.
///
/// The runner is sandboxed to `cwd` (matching the binary's existing
/// behaviour) and the MCP workspace root is set to `cwd` so the built-in
/// `filesystem` transport rolls onto the caller's working directory.
///
/// `auth_token` mirrors `ToolRunnerService::with_auth_token` semantics:
/// `None` disables bearer-token gating, `Some(token)` requires it.
///
/// Returns the assembled service plus the count of MCP servers discovered
/// so callers (the binary, in particular) can log it at startup.
pub fn build_service_from_cwd(
    cwd: &Path,
    auth_token: Option<String>,
) -> Result<(ToolRunnerService, usize)> {
    let paths = ConfigPaths::discover(cwd);

    // The probe runner is only used by `load_resources` to perform read /
    // list / glob calls against the filesystem layer. It never sees an MCP
    // RPC, so leaving it bare is intentional and safe — there's no
    // chicken-and-egg with the real runner we build below.
    let probe_runner = LocalToolRunner::new();
    let resources = load_resources(&paths, &probe_runner)
        .with_context(|| format!("load resources from {}", cwd.display()))?;

    let router = Arc::new(BidiElicitationRouter::default());
    let local = local_runner_from_resources(
        &resources,
        cwd.to_path_buf(),
        Some(vec![cwd.to_path_buf()]),
    )
    .with_elicitation_handler(Arc::clone(&router) as Arc<dyn ElicitationHandler>);
    // The MCP host owns the post-dedup, post-plugin-merge view of the
    // manifest, so query it for the count we surface to logs and tests
    // rather than relying on `resources.mcp_servers.len()` (which is the
    // pre-merge length).
    let mcp_count = local.mcp_host().servers().len();
    let runner: Arc<dyn ToolRunner> = Arc::new(local);
    let service = ToolRunnerService::with_router(runner, router).with_auth_token(auth_token);
    Ok((service, mcp_count))
}

/// Convenience wrapper that resolves `cwd` from an `Option`, falling back
/// to the process's current directory and validating the result is an
/// existing directory before building the service.
pub fn resolve_cwd(cwd: Option<PathBuf>) -> Result<PathBuf> {
    let resolved = match cwd {
        Some(p) => p,
        None => std::env::current_dir().context("cwd")?,
    };
    if !resolved.is_dir() {
        anyhow::bail!("--cwd {:?} is not a directory", resolved);
    }
    Ok(resolved)
}
