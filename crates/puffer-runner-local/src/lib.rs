//! In-process implementation of [`puffer_runner_api::ToolRunner`].
//!
//! The struct itself lives in `puffer-core::runner_adapter` so the runtime
//! can construct one without a circular dep on this crate. This crate
//! exists as the canonical "use the local runner" entry point for binaries
//! and tests.

pub use puffer_core::runner_adapter::LocalToolRunner;

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
