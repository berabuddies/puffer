//! Pure dispatch from a [`puffer_runner_api::ToolRequest`] to the existing
//! claude-parity tool implementations living in `runtime::claude_tools`.
//!
//! This module is the seam used by `puffer-runner-local::LocalToolRunner`.
//! It contains no per-session mutable state — read-state staleness is the
//! dispatcher's job (see [`puffer_runner_api::check_read_freshness`]). Each
//! tool result carries the freshly-observed `mtime` for any path that the
//! caller's read-state map should track.

use anyhow::{anyhow, bail, Context, Result};
use puffer_runner_api::{
    ChunkSink, DirEntry, ElicitationHandler, McpPrompt, McpPromptContent, McpResourceContent,
    McpResourceRecord, McpResult, McpServerInfo, McpTool, OAuthStatus, OAuthTokensPayload,
    ReadStateUpdate, RunnerCapabilities, RunnerError, RunnerPing, ToolRequest, ToolResult,
    ToolRunner,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Instant, UNIX_EPOCH};
use uuid::Uuid;

use crate::runner_mcp::McpHost;
use crate::runtime::claude_tools::{bash, edit, glob, grep, notebook_edit, read, web_fetch};
use puffer_resources::McpServerSpec;

/// Tools the local runner is willing to execute through the
/// [`puffer_runner_api::ToolRunner::execute_tool`] surface.
///
/// Network search (`WebSearch`) currently requires a provider context that
/// hasn't been hoisted onto the trait yet, so it stays on the legacy path.
pub fn supported_runner_tools() -> &'static [&'static str] {
    &[
        "Bash",
        "Read",
        "Write",
        "Edit",
        "Glob",
        "Grep",
        "NotebookEdit",
        "WebFetch",
        "Sleep",
    ]
}

/// Returns true when [`execute_runner_tool`] knows how to handle `tool_id`.
pub fn is_runner_supported(tool_id: &str) -> bool {
    supported_runner_tools().contains(&tool_id)
}

/// Executes one tool request through the runner-shaped dispatcher.
///
/// This is the pure body of `LocalToolRunner::execute_tool`. The caller is
/// responsible for any pre-flight staleness checks; this function only
/// reports back which paths were touched so the caller can update its
/// read-state map.
pub fn execute_runner_tool(req: &ToolRequest, _sink: &mut dyn ChunkSink) -> Result<ToolResult> {
    let cwd = req.cwd.as_path();
    let working_dirs = req.working_dirs.as_slice();
    let input = req.input.clone();

    match req.tool_id.as_str() {
        "Bash" => {
            let session_id = parse_session_id(req.session_id.as_deref())?;
            let execution = bash::execute_from_value(cwd, &session_id, input)?;
            let stdout = serde_json::to_string_pretty(&execution.output)
                .context("failed to serialize Bash output")?;
            Ok(plain_result(
                req.tool_id.as_str(),
                execution.success,
                stdout,
            ))
        }
        "Read" => {
            let stdout =
                read::execute_claude_read_tool(cwd, working_dirs, &req.filesystem, input.clone())?;
            let updates = read_update_from_input(&input)?;
            Ok(result_with_updates(
                req.tool_id.as_str(),
                true,
                stdout,
                updates,
            ))
        }
        "Write" => {
            let path = input_file_path(&input, "file_path")?;
            let stdout = run_claude_write(cwd, working_dirs, &req.filesystem, input.clone())?;
            let updates = match path.as_deref() {
                Some(path) => vec![ReadStateUpdate {
                    path: path.to_path_buf(),
                    timestamp_ms: file_timestamp_ms(path)?,
                    is_partial_view: false,
                }],
                None => Vec::new(),
            };
            Ok(result_with_updates(
                req.tool_id.as_str(),
                true,
                stdout,
                updates,
            ))
        }
        "Edit" => {
            let path = input_file_path(&input, "file_path")?;
            let stdout =
                edit::execute_claude_edit(cwd, working_dirs, &req.filesystem, input.clone())?;
            let updates = match path.as_deref() {
                Some(path) => vec![ReadStateUpdate {
                    path: path.to_path_buf(),
                    timestamp_ms: file_timestamp_ms(path)?,
                    is_partial_view: false,
                }],
                None => Vec::new(),
            };
            Ok(result_with_updates(
                req.tool_id.as_str(),
                true,
                stdout,
                updates,
            ))
        }
        "Glob" => {
            let stdout = glob::execute_claude_glob(cwd, working_dirs, &req.filesystem, input)?;
            Ok(plain_result(req.tool_id.as_str(), true, stdout))
        }
        "Grep" => {
            let stdout = grep::execute_claude_grep(cwd, working_dirs, &req.filesystem, input)?;
            Ok(plain_result(req.tool_id.as_str(), true, stdout))
        }
        "NotebookEdit" => {
            let path = input_file_path(&input, "notebook_path")?;
            let stdout = notebook_edit::execute_notebook_edit_tool(
                cwd,
                working_dirs,
                &req.filesystem,
                input.clone(),
            )?;
            let updates = match path.as_deref() {
                Some(path) if path.exists() => vec![ReadStateUpdate {
                    path: path.to_path_buf(),
                    timestamp_ms: file_timestamp_ms(path)?,
                    is_partial_view: false,
                }],
                _ => Vec::new(),
            };
            Ok(result_with_updates(
                req.tool_id.as_str(),
                true,
                stdout,
                updates,
            ))
        }
        "WebFetch" => {
            let stdout = serde_json::to_string_pretty(&web_fetch::execute_claude_web_fetch(input)?)
                .context("failed to serialize WebFetch output")?;
            Ok(plain_result(req.tool_id.as_str(), true, stdout))
        }
        "Sleep" => {
            let stdout = sleep_dispatch(input)?;
            Ok(plain_result(req.tool_id.as_str(), true, stdout))
        }
        other => bail!("tool `{other}` is not supported by the local runner"),
    }
}

/// Computes the `ReadStateUpdate` that the runtime should record after a
/// successful `Read` call. Mirrors the partial-view bookkeeping that used to
/// live inside `record_read_from_input`.
fn read_update_from_input(input: &Value) -> Result<Vec<ReadStateUpdate>> {
    let Some(path) = input_file_path(input, "file_path")? else {
        return Ok(Vec::new());
    };
    let timestamp_ms = file_timestamp_ms(&path)?;
    let offset = input.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let limit = input.get("limit").and_then(Value::as_u64);
    let line_count = std::fs::read_to_string(&path)
        .map(|content| content.lines().count() as u64)
        .unwrap_or(u64::MAX);
    let has_partial_offset = offset > 1;
    let has_restrictive_limit = limit.is_some_and(|l| {
        let effective_remaining = line_count.saturating_sub(offset);
        l < effective_remaining
    });
    let is_partial_view =
        has_partial_offset || has_restrictive_limit || pages_field_is_present(input);
    Ok(vec![ReadStateUpdate {
        path,
        timestamp_ms,
        is_partial_view,
    }])
}

fn pages_field_is_present(input: &Value) -> bool {
    match input.get("pages") {
        None | Some(Value::Null) => false,
        Some(Value::String(value)) => !value.trim().is_empty(),
        Some(_) => true,
    }
}

fn input_file_path(input: &Value, field: &str) -> Result<Option<std::path::PathBuf>> {
    Ok(input
        .get(field)
        .and_then(Value::as_str)
        .map(std::path::PathBuf::from))
}

fn file_timestamp_ms(path: &Path) -> Result<u128> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat file {}", path.display()))?;
    let modified = metadata
        .modified()
        .with_context(|| format!("failed to read mtime for {}", path.display()))?;
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .with_context(|| format!("mtime for {} predates UNIX_EPOCH", path.display()))?;
    Ok(duration.as_millis())
}

fn parse_session_id(raw: Option<&str>) -> Result<Uuid> {
    match raw {
        None => Ok(Uuid::nil()),
        Some(value) => {
            Uuid::parse_str(value).map_err(|e| anyhow!("invalid session_id `{value}`: {e}"))
        }
    }
}

fn run_claude_write(
    cwd: &Path,
    working_dirs: &[std::path::PathBuf],
    filesystem: &puffer_runner_api::FilesystemExecutionPolicy,
    input: Value,
) -> Result<String> {
    use crate::runtime::claude_tools::write::{execute_claude_write_tool, ClaudeReadSnapshot};
    use std::collections::HashMap;
    let mut bypass: HashMap<std::path::PathBuf, ClaudeReadSnapshot> = HashMap::new();
    if let Some(path) = input_file_path(&input, "file_path")? {
        if path.exists() {
            bypass.insert(
                path.clone(),
                ClaudeReadSnapshot {
                    timestamp_ms: file_timestamp_ms(&path)?,
                    is_partial_view: false,
                },
            );
        }
    }
    execute_claude_write_tool(cwd, working_dirs, filesystem, input, &mut bypass)
}

fn sleep_dispatch(input: Value) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct SleepInput {
        duration_ms: u64,
        #[serde(default)]
        reason: Option<String>,
    }
    const MAX_DURATION_MS: u64 = 300_000;
    let parsed: SleepInput = serde_json::from_value(input)?;
    if parsed.duration_ms == 0 {
        bail!("Sleep duration_ms must be greater than zero");
    }
    let duration_ms = parsed.duration_ms.min(MAX_DURATION_MS);
    std::thread::sleep(std::time::Duration::from_millis(duration_ms));
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "duration_ms": duration_ms,
        "completed": true,
        "reason": parsed.reason,
    }))?)
}

fn plain_result(tool_id: &str, success: bool, stdout: String) -> ToolResult {
    ToolResult {
        tool_id: tool_id.to_string(),
        success,
        stdout,
        stderr: String::new(),
        metadata: Value::Null,
        read_state_updates: Vec::new(),
    }
}

fn result_with_updates(
    tool_id: &str,
    success: bool,
    stdout: String,
    updates: Vec<ReadStateUpdate>,
) -> ToolResult {
    ToolResult {
        tool_id: tool_id.to_string(),
        success,
        stdout,
        stderr: String::new(),
        metadata: Value::Null,
        read_state_updates: updates,
    }
}

/// In-process [`ToolRunner`]. Lives in `puffer-core` so the runtime can
/// instantiate one without depending on the higher-level
/// `puffer-runner-local` crate (which itself re-exports this type as
/// `LocalToolRunner` for external callers).
///
/// `mcp_host` is held behind `Arc<RwLock<_>>` so the MCP roster can be
/// hot-swapped via [`LocalToolRunner::replace_mcp_host`] without rebuilding
/// the surrounding `Arc<dyn ToolRunner>` stored in `AppState`. All clones of
/// `LocalToolRunner` share the same `RwLock`, so a swap is visible
/// immediately to every in-flight handle.
#[derive(Debug, Clone)]
pub struct LocalToolRunner {
    sandbox_roots: Vec<PathBuf>,
    mcp_host: Arc<RwLock<McpHost>>,
    started: Instant,
}

impl Default for LocalToolRunner {
    fn default() -> Self {
        Self {
            sandbox_roots: Vec::new(),
            mcp_host: Arc::new(RwLock::new(McpHost::default())),
            started: Instant::now(),
        }
    }
}

impl LocalToolRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_sandbox_roots(roots: Vec<PathBuf>) -> Self {
        let workspace = roots.first().cloned();
        Self {
            sandbox_roots: roots,
            mcp_host: Arc::new(RwLock::new(McpHost::new(Vec::new(), workspace))),
            started: Instant::now(),
        }
    }

    /// Configures the runner with an MCP manifest. The first sandbox root
    /// (or the explicit override passed via `with_workspace_root`) is used
    /// by the built-in `filesystem` server when present.
    pub fn with_mcp_servers(self, servers: Vec<McpServerSpec>) -> Self {
        let (workspace, handler) = {
            let host = self.mcp_host.read().expect("mcp host rwlock poisoned");
            (
                host.workspace_root()
                    .map(Path::to_path_buf)
                    .or_else(|| self.sandbox_roots.first().cloned()),
                host.elicitation_handler(),
            )
        };
        let new_host = McpHost::with_elicitation(servers, workspace, handler);
        *self.mcp_host.write().expect("mcp host rwlock poisoned") = new_host;
        self
    }

    /// Overrides the workspace root used by built-in MCP transports (today
    /// only the filesystem stub). Mostly useful when the sandbox roots are
    /// disjoint from the MCP workspace.
    pub fn with_mcp_workspace_root(self, workspace: PathBuf) -> Self {
        let (handler, servers) = {
            let mut host = self.mcp_host.write().expect("mcp host rwlock poisoned");
            let handler = host.elicitation_handler();
            let servers = std::mem::take(&mut *host).into_servers();
            (handler, servers)
        };
        let new_host = McpHost::with_elicitation(servers, Some(workspace), handler);
        *self.mcp_host.write().expect("mcp host rwlock poisoned") = new_host;
        self
    }

    /// Installs an elicitation handler that the connection manager will
    /// invoke whenever an MCP server sends `elicitation/create` mid-tool-call.
    /// Defaults to [`puffer_runner_api::DeclineAllElicitations`].
    pub fn with_elicitation_handler(self, handler: Arc<dyn ElicitationHandler>) -> Self {
        let new_host = {
            let host = self.mcp_host.read().expect("mcp host rwlock poisoned");
            host.clone().with_elicitation_handler(handler)
        };
        *self.mcp_host.write().expect("mcp host rwlock poisoned") = new_host;
        self
    }

    /// Pin the directory where MCP OAuth tokens are persisted. Tests use
    /// this to scope per-test runs to a `TempDir`; production callers
    /// rely on the [`puffer_mcp_oauth::default_token_dir`] default
    /// (`<config>/puffer/mcp-tokens`).
    pub fn with_oauth_token_dir(self, dir: PathBuf) -> Self {
        let new_host = {
            let host = self.mcp_host.read().expect("mcp host rwlock poisoned");
            host.clone().with_oauth_token_dir(dir)
        };
        *self.mcp_host.write().expect("mcp host rwlock poisoned") = new_host;
        self
    }

    /// Snapshot of the MCP host. Use for short read-only inspections; the
    /// returned `RwLockReadGuard` blocks concurrent hot-swaps.
    pub fn mcp_host(&self) -> std::sync::RwLockReadGuard<'_, McpHost> {
        self.mcp_host.read().expect("mcp host rwlock poisoned")
    }

    /// Hot-swap the MCP host roster. Used by the resource reload pathway to
    /// pick up additions, removals, or edits to MCP manifests without
    /// restarting the process. Existing live MCP subprocess connections
    /// owned by the previous host are dropped (their `kill_on_drop` children
    /// exit asynchronously); the next MCP call lazily spawns whatever the
    /// new roster requires.
    pub fn replace_mcp_host(&self, host: McpHost) {
        *self.mcp_host.write().expect("mcp host rwlock poisoned") = host;
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
    fn ping(&self) -> Result<RunnerPing, RunnerError> {
        Ok(RunnerPing {
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime: self.started.elapsed(),
        })
    }

    fn capabilities(&self) -> RunnerCapabilities {
        RunnerCapabilities {
            backend: "local".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            supported_tools: supported_runner_tools()
                .iter()
                .map(|tool| (*tool).to_string())
                .collect(),
            mcp_supported: true,
        }
    }

    fn execute_tool(
        &self,
        req: ToolRequest,
        sink: &mut dyn ChunkSink,
    ) -> Result<ToolResult, RunnerError> {
        if !is_runner_supported(req.tool_id.as_str()) {
            return Err(RunnerError::Unsupported(format!(
                "tool `{}` is not handled by the local runner",
                req.tool_id
            )));
        }
        execute_runner_tool(&req, sink).map_err(RunnerError::execution)
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
        let paths = ::glob::glob(combined_str)
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
        Ok(self.mcp_host().list_servers())
    }
    fn list_mcp_tools(&self, server: &str) -> Result<Vec<McpTool>, RunnerError> {
        self.mcp_host().list_tools(server)
    }
    fn call_mcp_tool(
        &self,
        server: &str,
        tool: &str,
        args: serde_json::Value,
        sink: &mut dyn ChunkSink,
    ) -> Result<McpResult, RunnerError> {
        // The connection manager routes any `notifications/progress` events
        // emitted by the underlying MCP server into `sink.event(...)` so
        // long-running tools can stream partial state to the model.
        self.mcp_host().call_tool(server, tool, args, sink)
    }
    fn list_mcp_resources(
        &self,
        server: Option<&str>,
    ) -> Result<Vec<McpResourceRecord>, RunnerError> {
        self.mcp_host().list_resources(server)
    }
    fn read_mcp_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> Result<McpResourceContent, RunnerError> {
        self.mcp_host().read_resource(server, uri)
    }
    fn list_mcp_prompts(&self, server: &str) -> Result<Vec<McpPrompt>, RunnerError> {
        self.mcp_host().list_prompts(server)
    }
    fn get_mcp_prompt(
        &self,
        server: &str,
        name: &str,
        args: serde_json::Value,
    ) -> Result<McpPromptContent, RunnerError> {
        self.mcp_host().get_prompt(server, name, args)
    }

    fn push_oauth_tokens(
        &self,
        server: &str,
        tokens: OAuthTokensPayload,
    ) -> Result<(), RunnerError> {
        self.mcp_host().push_oauth_tokens(server, tokens)
    }

    fn oauth_status(&self, server: &str) -> Result<OAuthStatus, RunnerError> {
        self.mcp_host().oauth_status(server)
    }

    fn clear_oauth_tokens(&self, server: &str) -> Result<(), RunnerError> {
        self.mcp_host().clear_oauth_tokens(server)
    }

    fn as_any(&self) -> Option<&(dyn std::any::Any + 'static)> {
        Some(self)
    }
}

impl LocalToolRunner {
    /// Hot-swap the MCP roster while preserving the live elicitation
    /// handler, OAuth token directory, and (if not overridden) workspace
    /// root. `workspace` overrides the host's filesystem-MCP root when
    /// `Some`; otherwise the current host's root is reused, falling back
    /// to the first sandbox root. The previous host (and its live MCP
    /// connections) is dropped — their `kill_on_drop` child processes
    /// exit asynchronously, and the next MCP call lazily spawns the new
    /// roster.
    pub fn replace_mcp_roster(
        &self,
        servers: Vec<McpServerSpec>,
        workspace: Option<PathBuf>,
    ) {
        let (handler, fallback_workspace, oauth_dir) = {
            let host = self.mcp_host();
            (
                host.elicitation_handler(),
                host.workspace_root().map(Path::to_path_buf),
                host.oauth_token_dir().map(Path::to_path_buf),
            )
        };
        let resolved_workspace = workspace
            .or(fallback_workspace)
            .or_else(|| self.sandbox_roots.first().cloned());
        let new_host = McpHost::with_elicitation_and_oauth_dir(
            servers,
            resolved_workspace,
            handler,
            oauth_dir,
        );
        self.replace_mcp_host(new_host);
    }
}
