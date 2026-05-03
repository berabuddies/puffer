//! In-process implementation of [`puffer_runner_api::ToolRunner`].
//!
//! This wraps the existing `claude_tools::*` executors via the `runner_adapter`
//! shim in `puffer-core`. Filesystem methods (`read_file` / `list_dir` /
//! `glob`) are implemented directly on top of `std::fs` and `glob`. MCP and
//! `request_permission` remain stubs (Phase 1 / Phase 3).

use puffer_core::runner_adapter;
use puffer_runner_api::{
    ChunkSink, DirEntry, McpPrompt, McpPromptContent, McpResourceContent, McpResourceRecord,
    McpResult, McpServerInfo, McpTool, PermissionDecision, PermissionRequest, RunnerCapabilities,
    RunnerError, ToolRequest, ToolResult, ToolRunner,
};
use std::path::{Path, PathBuf};

/// In-process tool runner backed by `std::fs` and the existing puffer-core
/// claude-tool executors.
#[derive(Debug, Clone, Default)]
pub struct LocalToolRunner {
    sandbox_roots: Vec<PathBuf>,
}

impl LocalToolRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_sandbox_roots(roots: Vec<PathBuf>) -> Self {
        Self {
            sandbox_roots: roots,
        }
    }

    fn check_sandbox(&self, path: &Path) -> Result<(), RunnerError> {
        if self.sandbox_roots.is_empty() {
            return Ok(());
        }
        let canonical = std::fs::canonicalize(path)
            .map_err(|e| RunnerError::InvalidArgument(format!("canonicalize {path:?}: {e}")))?;
        let allowed = self.sandbox_roots.iter().any(|root| {
            std::fs::canonicalize(root)
                .map(|root| canonical.starts_with(&root))
                .unwrap_or(false)
        });
        if !allowed {
            return Err(RunnerError::PermissionDenied(format!(
                "path {path:?} escapes the configured sandbox roots"
            )));
        }
        Ok(())
    }
}

impl ToolRunner for LocalToolRunner {
    fn capabilities(&self) -> RunnerCapabilities {
        RunnerCapabilities {
            backend: "local".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            supported_tools: runner_adapter::supported_runner_tools()
                .iter()
                .map(|tool| (*tool).to_string())
                .collect(),
            mcp_supported: false,
            permission_relay_supported: false,
        }
    }

    fn execute_tool(
        &self,
        req: ToolRequest,
        sink: &mut dyn ChunkSink,
    ) -> Result<ToolResult, RunnerError> {
        if !runner_adapter::is_runner_supported(req.tool_id.as_str()) {
            return Err(RunnerError::Unsupported(format!(
                "tool `{}` is not handled by the local runner",
                req.tool_id
            )));
        }
        runner_adapter::execute_runner_tool(&req, sink).map_err(RunnerError::execution)
    }

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, RunnerError> {
        self.check_sandbox(path)?;
        std::fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => RunnerError::NotFound(path.display().to_string()),
            std::io::ErrorKind::PermissionDenied => {
                RunnerError::PermissionDenied(path.display().to_string())
            }
            _ => RunnerError::Other(format!("read {path:?}: {e}")),
        })
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, RunnerError> {
        self.check_sandbox(path)?;
        let read = std::fs::read_dir(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => RunnerError::NotFound(path.display().to_string()),
            std::io::ErrorKind::PermissionDenied => {
                RunnerError::PermissionDenied(path.display().to_string())
            }
            _ => RunnerError::Other(format!("read_dir {path:?}: {e}")),
        })?;
        let mut entries = Vec::new();
        for entry in read {
            let entry =
                entry.map_err(|e| RunnerError::Other(format!("dir entry {path:?}: {e}")))?;
            let file_type = entry
                .file_type()
                .map_err(|e| RunnerError::Other(format!("file_type for {entry:?}: {e}")))?;
            entries.push(DirEntry {
                path: entry.path(),
                is_dir: file_type.is_dir(),
                is_file: file_type.is_file(),
                is_symlink: file_type.is_symlink(),
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    fn glob(&self, root: &Path, pattern: &str) -> Result<Vec<PathBuf>, RunnerError> {
        self.check_sandbox(root)?;
        let combined = root.join(pattern);
        let combined_str = combined
            .to_str()
            .ok_or_else(|| RunnerError::InvalidArgument(format!("non-utf8 glob: {combined:?}")))?;
        let paths = glob::glob(combined_str)
            .map_err(|e| RunnerError::InvalidArgument(format!("invalid glob: {e}")))?;
        let mut results = Vec::new();
        for entry in paths {
            match entry {
                Ok(path) => results.push(path),
                Err(e) => return Err(RunnerError::Other(format!("glob iter: {e}"))),
            }
        }
        results.sort();
        Ok(results)
    }

    fn list_mcp_servers(&self) -> Result<Vec<McpServerInfo>, RunnerError> {
        Err(RunnerError::Unsupported(
            "MCP centralization (Phase 1) is not yet implemented".into(),
        ))
    }

    fn list_mcp_tools(&self, _server: &str) -> Result<Vec<McpTool>, RunnerError> {
        Err(RunnerError::Unsupported(
            "MCP centralization (Phase 1) is not yet implemented".into(),
        ))
    }

    fn call_mcp_tool(
        &self,
        _server: &str,
        _tool: &str,
        _args: serde_json::Value,
        _sink: &mut dyn ChunkSink,
    ) -> Result<McpResult, RunnerError> {
        Err(RunnerError::Unsupported(
            "MCP centralization (Phase 1) is not yet implemented".into(),
        ))
    }

    fn list_mcp_resources(
        &self,
        _server: Option<&str>,
    ) -> Result<Vec<McpResourceRecord>, RunnerError> {
        Err(RunnerError::Unsupported(
            "MCP centralization (Phase 1) is not yet implemented".into(),
        ))
    }

    fn read_mcp_resource(
        &self,
        _server: &str,
        _uri: &str,
    ) -> Result<McpResourceContent, RunnerError> {
        Err(RunnerError::Unsupported(
            "MCP centralization (Phase 1) is not yet implemented".into(),
        ))
    }

    fn list_mcp_prompts(&self, _server: &str) -> Result<Vec<McpPrompt>, RunnerError> {
        Err(RunnerError::Unsupported(
            "MCP centralization (Phase 1) is not yet implemented".into(),
        ))
    }

    fn get_mcp_prompt(
        &self,
        _server: &str,
        _name: &str,
        _args: serde_json::Value,
    ) -> Result<McpPromptContent, RunnerError> {
        Err(RunnerError::Unsupported(
            "MCP centralization (Phase 1) is not yet implemented".into(),
        ))
    }

    fn request_permission(
        &self,
        _req: PermissionRequest,
    ) -> Result<PermissionDecision, RunnerError> {
        Err(RunnerError::Unsupported(
            "permission relay (Phase 3) is not yet implemented; default-deny".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_runner_api::NullChunkSink;
    use std::fs;
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
