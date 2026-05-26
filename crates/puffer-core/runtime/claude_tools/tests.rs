use super::*;
use crate::permissions::profile::{EffectiveApprovalPolicy, EffectiveSandboxMode};
use crate::permissions::FilesystemPermissionPolicy;
use puffer_resources::LoadedResources;

#[test]
fn workflow_terminate_metadata_only_fires_for_completed_update_goal() {
    // Anything other than update_goal: never set terminate.
    assert_eq!(
        workflow_terminate_metadata("create_goal", "{\"goal\":{\"status\":\"complete\"}}"),
        Value::Null
    );
    assert_eq!(
        workflow_terminate_metadata("get_goal", "{\"goal\":{\"status\":\"complete\"}}"),
        Value::Null
    );
    // update_goal but the goal didn't actually flip to complete:
    // also no terminate (defensive — shouldn't happen given our
    // serde lock, but the helper is the only post-process site).
    assert_eq!(
        workflow_terminate_metadata("update_goal", "{\"goal\":{\"status\":\"active\"}}"),
        Value::Null
    );
    // update_goal with completed goal: terminate set.
    let metadata = workflow_terminate_metadata(
        "update_goal",
        "{\"goal\":{\"status\":\"complete\",\"objective\":\"x\"}}",
    );
    assert_eq!(metadata.get("terminate"), Some(&Value::Bool(true)));
}

#[test]
fn workflow_terminate_metadata_handles_malformed_json_gracefully() {
    // Defensive — workflow handler always emits valid JSON, but
    // a malformed payload must not panic the dispatcher.
    assert_eq!(
        workflow_terminate_metadata("update_goal", "not json"),
        Value::Null
    );
    assert_eq!(workflow_terminate_metadata("update_goal", ""), Value::Null);
}

use puffer_runner_api::{
    ChunkSink, DirEntry, McpPrompt, McpPromptContent, McpResourceContent, McpResourceRecord,
    McpResult, McpServerInfo, McpTool, RunnerCapabilities, RunnerError, ToolRequest, ToolResult,
    ToolRunner,
};
use puffer_tools::{
    ToolDefinition, ToolDisplayHints, ToolInputSchema, ToolKind, ToolMetadata, ToolPolicyHints,
    ToolRegistry,
};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use uuid::Uuid;

/// Records every `execute_tool` call and forwards execution to an inner
/// `LocalToolRunner`. Used to prove that the parallel-batch path actually
/// dispatches through the trait instead of bypassing it.
#[derive(Debug)]
struct RecordingRunner {
    inner: Arc<dyn ToolRunner>,
    execute_calls: AtomicUsize,
}

impl RecordingRunner {
    fn new(inner: Arc<dyn ToolRunner>) -> Self {
        Self {
            inner,
            execute_calls: AtomicUsize::new(0),
        }
    }

    fn execute_calls(&self) -> usize {
        self.execute_calls.load(Ordering::SeqCst)
    }
}

impl ToolRunner for RecordingRunner {
    fn ping(&self) -> Result<puffer_runner_api::RunnerPing, RunnerError> {
        self.inner.ping()
    }
    fn capabilities(&self) -> RunnerCapabilities {
        self.inner.capabilities()
    }
    fn execute_tool(
        &self,
        req: ToolRequest,
        sink: &mut dyn ChunkSink,
    ) -> Result<ToolResult, RunnerError> {
        self.execute_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.execute_tool(req, sink)
    }
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, RunnerError> {
        self.inner.read_file(path)
    }
    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, RunnerError> {
        self.inner.list_dir(path)
    }
    fn glob(&self, root: &Path, pattern: &str) -> Result<Vec<std::path::PathBuf>, RunnerError> {
        self.inner.glob(root, pattern)
    }
    fn list_mcp_servers(&self) -> Result<Vec<McpServerInfo>, RunnerError> {
        self.inner.list_mcp_servers()
    }
    fn list_mcp_tools(&self, server: &str) -> Result<Vec<McpTool>, RunnerError> {
        self.inner.list_mcp_tools(server)
    }
    fn call_mcp_tool(
        &self,
        server: &str,
        tool: &str,
        args: serde_json::Value,
        sink: &mut dyn ChunkSink,
    ) -> Result<McpResult, RunnerError> {
        self.inner.call_mcp_tool(server, tool, args, sink)
    }
    fn list_mcp_resources(
        &self,
        server: Option<&str>,
    ) -> Result<Vec<McpResourceRecord>, RunnerError> {
        self.inner.list_mcp_resources(server)
    }
    fn read_mcp_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> Result<McpResourceContent, RunnerError> {
        self.inner.read_mcp_resource(server, uri)
    }
    fn list_mcp_prompts(&self, server: &str) -> Result<Vec<McpPrompt>, RunnerError> {
        self.inner.list_mcp_prompts(server)
    }
    fn get_mcp_prompt(
        &self,
        server: &str,
        name: &str,
        args: serde_json::Value,
    ) -> Result<McpPromptContent, RunnerError> {
        self.inner.get_mcp_prompt(server, name, args)
    }
}

/// Verifies the parallel-tool path routes runner-supported tools through
/// `Arc<dyn ToolRunner>::execute_tool` instead of calling in-process
/// helpers directly.
#[test]
fn parallel_path_dispatches_through_runner() {
    let inner: Arc<dyn ToolRunner> = Arc::new(crate::runner_adapter::LocalToolRunner::new());
    let recording = Arc::new(RecordingRunner::new(inner));
    let runner: Arc<dyn ToolRunner> = recording.clone();

    let resources = LoadedResources::default();
    let registry = ToolRegistry::default();
    let provider_context = ProviderToolContext::None;
    let session_id = Uuid::new_v4();
    let workspace = tempfile::tempdir().expect("tempdir");
    let cwd = workspace.path().to_path_buf();
    let working_dirs: Vec<std::path::PathBuf> = Vec::new();

    // Claude-parity tools use capitalized ids that the dispatcher
    // matches on; build minimal definitions directly so neither the
    // builtin lowercase ids nor a `runtime:` handler mismatch
    // perturbs the dispatch path under test.
    fn claude_tool_def(id: &str, handler: &str) -> ToolDefinition {
        ToolDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: id.to_string(),
            handler: handler.to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        }
    }
    std::fs::write(cwd.join("sample.txt"), "parallel-runner\n").expect("write sample");
    let grep_def = claude_tool_def("Grep", "runtime:claude_grep");
    let glob_def = claude_tool_def("Glob", "runtime:claude_glob");

    let filesystem_policy = FilesystemPermissionPolicy {
        approval: EffectiveApprovalPolicy::Allow,
        sandbox_mode: EffectiveSandboxMode::DangerFullAccess,
        workspace_roots: vec![cwd.clone()],
        session_granted: true,
        allow_all_paths: true,
    };
    let grep_input = json!({"pattern": "parallel-runner", "path": "sample.txt"});
    let grep_result = execute_parallel_tool(
        &grep_def,
        &cwd,
        &working_dirs,
        &filesystem_policy,
        &session_id,
        grep_input,
        &resources,
        &registry,
        &provider_context,
        &runner,
    )
    .expect("Grep through runner");
    assert!(grep_result.success, "Grep should succeed");
    let grep_stdout: Value =
        serde_json::from_str(&grep_result.output.stdout).expect("Grep JSON stdout");
    assert_eq!(
        grep_stdout
            .get("filenames")
            .and_then(Value::as_array)
            .and_then(|filenames| filenames.first())
            .and_then(Value::as_str),
        Some("sample.txt")
    );

    let glob_input = json!({"pattern": "*"});
    let glob_result = execute_parallel_tool(
        &glob_def,
        &cwd,
        &working_dirs,
        &filesystem_policy,
        &session_id,
        glob_input,
        &resources,
        &registry,
        &provider_context,
        &runner,
    )
    .expect("Glob through runner");
    assert!(glob_result.success, "Glob should succeed");

    assert_eq!(
        recording.execute_calls(),
        2,
        "expected the runner to be invoked once per parallel-safe runner-supported tool",
    );
}

#[test]
fn blank_pages_do_not_make_read_partial() {
    let input = json!({
        "file_path": "/tmp/demo.txt",
        "pages": "   ",
    });

    assert!(is_full_read_request(&input));
    assert!(!read_pages_field_is_present(&input));
}

#[test]
fn null_optional_read_fields_are_treated_as_absent() {
    let input = json!({
        "file_path": "/tmp/demo.txt",
        "offset": null,
        "limit": null,
        "pages": null,
    });

    assert!(is_full_read_request(&input));
    assert!(!read_field_is_present(&input, "offset"));
    assert!(!read_field_is_present(&input, "limit"));
}
