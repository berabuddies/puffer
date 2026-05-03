//! In-process implementation of [`puffer_runner_api::ToolRunner`].
//!
//! The struct itself lives in `puffer-core::runner_adapter` so the runtime
//! can construct one without a circular dep on this crate. This crate
//! exists as the canonical "use the local runner" entry point for binaries
//! and tests.

use std::path::PathBuf;

use puffer_resources::{plugin_mcp_servers, LoadedResources, McpServerSpec};

pub use puffer_core::runner_adapter::LocalToolRunner;

/// Builds a `LocalToolRunner` configured with the MCP servers loaded into
/// `resources` and rooted at `workspace_root` for the built-in filesystem
/// transport.
///
/// `sandbox_roots` controls path-access policy:
/// * `None` — no sandbox (the in-process / TUI flow keeps the sandbox wide
///   open so resource loaders can still reach `~/.config/puffer` and other
///   non-workspace paths).
/// * `Some(roots)` — restrict file I/O to the listed roots (the standalone
///   `puffer-tool-runner` binary uses this so its remote callers can't
///   reach outside the working directory).
///
/// MCP servers are taken from `resources.mcp_servers` plus any
/// plugin-embedded servers exposed via [`plugin_mcp_servers`], deduplicated
/// by id (case-insensitive). The MCP workspace root is set to
/// `workspace_root` so the built-in `filesystem` transport rolls onto the
/// caller's `--cwd` rather than the first sandbox root.
pub fn local_runner_from_resources(
    resources: &LoadedResources,
    workspace_root: PathBuf,
    sandbox_roots: Option<Vec<PathBuf>>,
) -> LocalToolRunner {
    let servers = collect_mcp_servers(resources);
    let runner = match sandbox_roots {
        Some(roots) => LocalToolRunner::with_sandbox_roots(roots),
        None => LocalToolRunner::new(),
    };
    runner
        .with_mcp_servers(servers)
        .with_mcp_workspace_root(workspace_root)
}

/// Merges MCP server specs discovered through resource loading with those
/// embedded in plugin manifests. On id collision the resource-loaded entry
/// wins (it already reflects workspace > user > builtin > embedded merge
/// order from [`puffer_resources::load_resources`]).
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

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_runner_api::{NullChunkSink, RunnerError, ToolRequest, ToolRunner};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[test]
    fn read_file_returns_bytes() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("hello.txt");
        fs::write(&path, b"hello").unwrap();
        let runner = LocalToolRunner::new();
        let bytes = runner.read_file(&path).unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn read_missing_file_is_not_found() {
        let runner = LocalToolRunner::new();
        let err = runner
            .read_file(Path::new("/nonexistent/thing-puffer-runner-test"))
            .unwrap_err();
        assert!(matches!(err, RunnerError::NotFound(_)));
    }

    #[test]
    fn list_dir_returns_sorted_entries() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("b.txt"), "").unwrap();
        fs::write(temp.path().join("a.txt"), "").unwrap();
        fs::create_dir(temp.path().join("c")).unwrap();
        let runner = LocalToolRunner::new();
        let entries = runner.list_dir(temp.path()).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries[0].path.ends_with("a.txt"));
        assert!(entries[1].path.ends_with("b.txt"));
        assert!(entries[2].path.ends_with("c"));
        assert!(entries[2].is_dir);
    }

    #[test]
    fn glob_resolves_star_under_root() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("alpha.yaml"), "").unwrap();
        fs::write(temp.path().join("beta.yaml"), "").unwrap();
        fs::write(temp.path().join("readme.md"), "").unwrap();
        let runner = LocalToolRunner::new();
        let results = runner.glob(temp.path(), "*.yaml").unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn unknown_tool_id_is_unsupported() {
        let runner = LocalToolRunner::new();
        let req = ToolRequest {
            tool_id: "DefinitelyUnknown".into(),
            cwd: PathBuf::from("/"),
            working_dirs: Vec::new(),
            allow_all_paths: false,
            input: serde_json::json!({}),
            session_id: None,
        };
        let mut sink = NullChunkSink;
        let err = runner.execute_tool(req, &mut sink).unwrap_err();
        assert!(matches!(err, RunnerError::Unsupported(_)));
    }

    #[test]
    fn capabilities_advertise_local_backend() {
        let runner = LocalToolRunner::new();
        let caps = runner.capabilities();
        assert_eq!(caps.backend, "local");
        assert!(caps.supported_tools.iter().any(|name| name == "Bash"));
        assert!(caps.supported_tools.iter().any(|name| name == "Sleep"));
    }

    #[test]
    fn sandbox_blocks_paths_outside_roots() {
        let temp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        let runner = LocalToolRunner::with_sandbox_roots(vec![temp.path().to_path_buf()]);
        let err = runner
            .read_file(&outside.path().join("secret.txt"))
            .unwrap_err();
        assert!(matches!(err, RunnerError::PermissionDenied(_)));
    }
}
