use super::store::{git_toplevel, resolve_path};
use anyhow::{anyhow, bail, Context, Result};
use puffer_resources::{plugin_lsp_servers, LoadedResources};
use serde::Deserialize;
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;
use url::Url;

#[path = "lsp_live_format.rs"]
mod format;
#[path = "lsp_live_manager.rs"]
mod manager;
#[cfg(test)]
#[path = "lsp_live_tests.rs"]
mod tests;

use self::format::{
    format_call_hierarchy_result, format_diagnostics_result, format_document_symbol_result,
    format_hover_result, format_location_result, format_prepare_call_hierarchy_result,
    format_references_result, format_workspace_symbol_result,
};
use self::manager::with_lsp_session;
use super::lsp_live_diagnostics::{diagnostics_for_file, record_publish_diagnostics};

const LSP_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const LSP_SHUTDOWN_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_LSP_FILE_SIZE_BYTES: u64 = 10_000_000;

#[derive(Debug, Clone, Deserialize)]
struct LspInput {
    operation: String,
    #[serde(rename = "filePath")]
    file_path: String,
    line: usize,
    character: usize,
}

#[derive(Debug, Serialize)]
struct LspToolOutput {
    operation: String,
    #[serde(rename = "filePath")]
    file_path: String,
    result: String,
    #[serde(rename = "resultCount", skip_serializing_if = "Option::is_none")]
    result_count: Option<usize>,
    #[serde(rename = "fileCount", skip_serializing_if = "Option::is_none")]
    file_count: Option<usize>,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedLspServer {
    id: String,
    command: String,
    args: Vec<String>,
    env: std::collections::BTreeMap<String, String>,
    workspace_folder: Option<String>,
    language_id: String,
}

#[derive(Debug, Clone)]
struct MissingLspServer {
    display_name: String,
    command: String,
    install_hint: Option<String>,
}

#[derive(Debug, Clone)]
enum LspServerResolution {
    Available(ResolvedLspServer),
    Missing {
        extension: String,
        servers: Vec<MissingLspServer>,
    },
    Unconfigured {
        extension: String,
    },
}

#[derive(Debug)]
pub(super) struct LspSession {
    workspace_root: PathBuf,
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Result<Value>>,
    next_id: u64,
}

#[derive(Debug, Clone)]
struct LocationSummary {
    file_path: String,
    line: usize,
    character: usize,
}

#[derive(Debug, Clone)]
struct LspExecutionResult {
    result: String,
    result_count: Option<usize>,
    file_count: Option<usize>,
}

#[derive(Debug, Clone)]
pub(super) struct LspFileSync {
    file_uri: String,
    language_id: String,
    content: String,
}

/// Executes the Claude-compatible `LSP` tool through a real stdio LSP session.
pub(super) fn execute_lsp(resources: &LoadedResources, cwd: &Path, input: Value) -> Result<String> {
    let parsed: LspInput = serde_json::from_value(input).context("invalid LSP input")?;
    let operation = parsed.operation.clone();
    let file_path = resolve_path(cwd, &parsed.file_path);
    let output = match validate_lsp_input(cwd, &file_path)
        .and_then(|file_path| execute_lsp_inner(resources, cwd, &parsed, &file_path))
    {
        Ok(result) => LspToolOutput {
            operation,
            file_path: file_path.display().to_string(),
            result: result.result,
            result_count: result.result_count,
            file_count: result.file_count,
        },
        Err(error) => LspToolOutput {
            operation,
            file_path: file_path.display().to_string(),
            result: format!("Error performing {}: {}", parsed.operation, error),
            result_count: None,
            file_count: None,
        },
    };
    Ok(serde_json::to_string_pretty(&output)?)
}

pub(super) fn shutdown_lsp_services() -> Result<()> {
    manager::shutdown_all_lsp_sessions()
}

fn execute_lsp_inner(
    resources: &LoadedResources,
    cwd: &Path,
    input: &LspInput,
    file_path: &Path,
) -> Result<LspExecutionResult> {
    let server = match resolve_lsp_server(resources, file_path) {
        LspServerResolution::Available(server) => server,
        LspServerResolution::Missing { extension, servers } => {
            return Ok(LspExecutionResult {
                result: format_missing_lsp_server_message(&extension, &servers),
                result_count: None,
                file_count: None,
            });
        }
        LspServerResolution::Unconfigured { extension } => {
            return Ok(LspExecutionResult {
                result: format!("No LSP server available for file type: {extension}"),
                result_count: None,
                file_count: None,
            });
        }
    };
    let workspace_root = workspace_root(cwd, file_path);
    let file_uri = file_uri(file_path)?;
    let file_sync = if matches!(input.operation.as_str(), "workspaceSymbol" | "diagnostics") {
        None
    } else {
        Some(LspFileSync {
            file_uri: file_uri.clone(),
            language_id: server.language_id.clone(),
            content: read_lsp_file(file_path)?,
        })
    };
    with_lsp_session(&server, &workspace_root, file_sync, |session| {
        run_lsp_operation(session, input, &file_uri, &workspace_root)
    })
}

fn validate_lsp_input(cwd: &Path, file_path: &Path) -> Result<PathBuf> {
    let canonical_cwd = fs::canonicalize(cwd)
        .with_context(|| format!("failed to canonicalize {}", cwd.display()))?;
    let canonical_file = fs::canonicalize(file_path)
        .with_context(|| format!("failed to stat {}", file_path.display()))?;
    if !canonical_file.starts_with(&canonical_cwd) {
        bail!(
            "LSP file path escapes workspace: {}",
            canonical_file.display()
        );
    }
    let metadata = fs::metadata(&canonical_file)
        .with_context(|| format!("failed to stat {}", canonical_file.display()))?;
    if !metadata.is_file() {
        bail!("Path is not a file: {}", canonical_file.display());
    }
    Ok(canonical_file)
}

fn run_lsp_operation(
    session: &mut LspSession,
    input: &LspInput,
    file_uri: &str,
    workspace_root: &Path,
) -> Result<LspExecutionResult> {
    let result = match input.operation.as_str() {
        "hover" => {
            let response = session.request(
                "textDocument/hover",
                json!({
                    "textDocument": { "uri": file_uri },
                    "position": lsp_position(input.line, input.character),
                }),
            )?;
            format_hover_result(response)
        }
        "goToDefinition" => {
            let response = session.request(
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": file_uri },
                    "position": lsp_position(input.line, input.character),
                }),
            )?;
            format_location_result("definition", response, workspace_root)
        }
        "findReferences" => {
            let response = session.request(
                "textDocument/references",
                json!({
                    "textDocument": { "uri": file_uri },
                    "position": lsp_position(input.line, input.character),
                    "context": { "includeDeclaration": true },
                }),
            )?;
            format_references_result(response, workspace_root)
        }
        "documentSymbol" => {
            let response = session.request(
                "textDocument/documentSymbol",
                json!({
                    "textDocument": { "uri": file_uri },
                }),
            )?;
            format_document_symbol_result(response, workspace_root)
        }
        "diagnostics" => {
            session.drain_pending_messages()?;
            Ok(format_diagnostics_result(&diagnostics_for_file(
                workspace_root,
                file_uri,
            )?))
        }
        "workspaceSymbol" => {
            let response = session.request("workspace/symbol", json!({ "query": "" }))?;
            format_workspace_symbol_result(response, workspace_root)
        }
        "goToImplementation" => {
            let response = session.request(
                "textDocument/implementation",
                json!({
                    "textDocument": { "uri": file_uri },
                    "position": lsp_position(input.line, input.character),
                }),
            )?;
            format_location_result("implementation", response, workspace_root)
        }
        "prepareCallHierarchy" => {
            let response = session.request(
                "textDocument/prepareCallHierarchy",
                json!({
                    "textDocument": { "uri": file_uri },
                    "position": lsp_position(input.line, input.character),
                }),
            )?;
            format_prepare_call_hierarchy_result(response, workspace_root)
        }
        "incomingCalls" => {
            let response = prepare_call_hierarchy(session, file_uri, input)?;
            format_call_hierarchy_result(
                session,
                "callHierarchy/incomingCalls",
                "incoming calls",
                response,
                workspace_root,
            )
        }
        "outgoingCalls" => {
            let response = prepare_call_hierarchy(session, file_uri, input)?;
            format_call_hierarchy_result(
                session,
                "callHierarchy/outgoingCalls",
                "outgoing calls",
                response,
                workspace_root,
            )
        }
        other => bail!("unsupported LSP operation `{other}`"),
    }?;

    Ok(result)
}

fn prepare_call_hierarchy(
    session: &mut LspSession,
    file_uri: &str,
    input: &LspInput,
) -> Result<Value> {
    session.request(
        "textDocument/prepareCallHierarchy",
        json!({
            "textDocument": { "uri": file_uri },
            "position": lsp_position(input.line, input.character),
        }),
    )
}

impl LspSession {
    pub(super) fn start(server: &ResolvedLspServer, workspace_root: &Path) -> Result<Self> {
        let mut child = Command::new(&server.command)
            .args(&server.args)
            .current_dir(resolve_server_cwd(server, workspace_root))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .envs(&server.env)
            .spawn()
            .with_context(|| format!("failed to start LSP server `{}`", server.command))?;
        let stdin = child.stdin.take().context("LSP stdin unavailable")?;
        let stdout = child.stdout.take().context("LSP stdout unavailable")?;
        let messages = spawn_message_reader(stdout);
        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            child,
            stdin,
            messages,
            next_id: 1,
        })
    }

    pub(super) fn initialize(&mut self, workspace_root: &Path) -> Result<()> {
        let workspace_uri = file_uri(workspace_root)?;
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": {
                "name": "puffer-code",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "rootPath": workspace_root.display().to_string(),
            "rootUri": workspace_uri,
            "workspaceFolders": [
                {
                    "uri": workspace_uri,
                    "name": workspace_root
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("workspace"),
                }
            ],
            "capabilities": {
                "workspace": {
                    "configuration": false,
                    "workspaceFolders": false,
                },
                "textDocument": {
                    "hover": {
                        "dynamicRegistration": false,
                        "contentFormat": ["markdown", "plaintext"],
                    },
                    "definition": {
                        "dynamicRegistration": false,
                        "linkSupport": true,
                    },
                    "references": {
                        "dynamicRegistration": false,
                    },
                    "documentSymbol": {
                        "dynamicRegistration": false,
                        "hierarchicalDocumentSymbolSupport": true,
                    },
                    "callHierarchy": {
                        "dynamicRegistration": false,
                    },
                    "synchronization": {
                        "dynamicRegistration": false,
                        "didSave": true,
                    }
                },
                "general": {
                    "positionEncodings": ["utf-16"],
                }
            },
        });
        let _ = self.request("initialize", params)?;
        self.notify("initialized", json!({}))?;
        Ok(())
    }

    pub(super) fn open_file(
        &mut self,
        file_uri: &str,
        language_id: &str,
        content: &str,
    ) -> Result<()> {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": file_uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": content,
                }
            }),
        )
    }

    pub(super) fn change_file(
        &mut self,
        file_uri: &str,
        version: i64,
        content: &str,
    ) -> Result<()> {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {
                    "uri": file_uri,
                    "version": version,
                },
                "contentChanges": [
                    {
                        "text": content,
                    }
                ],
            }),
        )
    }

    pub(super) fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.request_with_timeout(method, params, LSP_REQUEST_TIMEOUT)
    }

    fn request_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        self.drain_pending_messages()?;
        let id = self.next_id;
        self.next_id += 1;
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;

        loop {
            let message = self
                .messages
                .recv_timeout(timeout)
                .map_err(|error| match error {
                    RecvTimeoutError::Timeout => {
                        anyhow!("timed out waiting for LSP response to `{method}`")
                    }
                    RecvTimeoutError::Disconnected => {
                        anyhow!("LSP server exited before responding to `{method}`")
                    }
                })??;
            if is_server_request(&message) {
                self.respond_to_server_request(&message)?;
                continue;
            }
            if self.handle_notification(&message)? {
                continue;
            }
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("LSP request `{method}` failed: {}", error);
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    pub(super) fn drain_pending_messages(&mut self) -> Result<()> {
        loop {
            match self.messages.try_recv() {
                Ok(message) => {
                    let message = message?;
                    if is_server_request(&message) {
                        self.respond_to_server_request(&message)?;
                        continue;
                    }
                    let _ = self.handle_notification(&message)?;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(()),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }

    fn drain_pending_messages_until_idle(&mut self, idle_timeout: Duration) -> Result<()> {
        loop {
            match self.messages.recv_timeout(idle_timeout) {
                Ok(message) => {
                    let message = message?;
                    if is_server_request(&message) {
                        self.respond_to_server_request(&message)?;
                        continue;
                    }
                    let _ = self.handle_notification(&message)?;
                }
                Err(RecvTimeoutError::Timeout) => return Ok(()),
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }

    fn handle_notification(&mut self, message: &Value) -> Result<bool> {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Ok(false);
        };
        if method == "textDocument/publishDiagnostics" {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let uri = params
                .get("uri")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("publishDiagnostics notification missing uri"))?;
            let diagnostics = params
                .get("diagnostics")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            record_publish_diagnostics(&self.workspace_root, uri, &diagnostics)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn respond_to_server_request(&mut self, message: &Value) -> Result<()> {
        let id = message
            .get("id")
            .cloned()
            .ok_or_else(|| anyhow!("LSP server request missing id"))?;
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("LSP server request missing method"))?;
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "workspace/configuration" => {
                let items = params
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().map(|_| Value::Null).collect::<Vec<_>>())
                    .unwrap_or_default();
                Value::Array(items)
            }
            _ => Value::Null,
        };
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    fn write_message(&mut self, value: &Value) -> Result<()> {
        let body = serde_json::to_vec(value)?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())
            .context("failed to write LSP headers")?;
        self.stdin
            .write_all(&body)
            .context("failed to write LSP body")?;
        self.stdin.flush().context("failed to flush LSP body")
    }

    pub(super) fn shutdown(&mut self) -> Result<()> {
        let _ = self.request_with_timeout(
            "shutdown",
            Value::Object(Default::default()),
            LSP_SHUTDOWN_REQUEST_TIMEOUT,
        );
        let _ = self.notify("exit", Value::Object(Default::default()));
        if let Ok(Some(_)) = self.child.try_wait() {
            return Ok(());
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }

    pub(super) fn has_exited(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_some()
    }
}

fn spawn_message_reader(stdout: ChildStdout) -> Receiver<Result<Value>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_lsp_message(&mut reader) {
                Ok(Some(message)) => {
                    if sender.send(Ok(message)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = sender.send(Err(error));
                    break;
                }
            }
        }
    });
    receiver
}

fn read_lsp_message<R>(reader: &mut BufReader<R>) -> Result<Option<Value>>
where
    R: Read,
{
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .context("failed to read LSP header")?;
        if bytes == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .context("invalid LSP Content-Length header")?,
            );
        }
    }

    let length = content_length.ok_or_else(|| anyhow!("missing LSP Content-Length header"))?;
    let mut body = vec![0_u8; length];
    reader
        .read_exact(&mut body)
        .context("failed to read LSP message body")?;
    let value = serde_json::from_slice::<Value>(&body).context("invalid LSP JSON body")?;
    Ok(Some(value))
}

fn is_server_request(message: &Value) -> bool {
    message.get("id").is_some() && message.get("method").is_some()
}

fn resolve_lsp_server(resources: &LoadedResources, file_path: &Path) -> LspServerResolution {
    let extension = file_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_else(|| "<unknown>".to_string());
    let mut missing_servers = Vec::new();
    for (_, server) in plugin_lsp_servers(resources) {
        let Some(language_id) = server.extension_to_language.get(&extension) else {
            continue;
        };
        let Some(command) = resolved_command(server.command.as_str()) else {
            continue;
        };
        if command_is_usable(&command) {
            return LspServerResolution::Available(ResolvedLspServer {
                id: server.id.clone(),
                command,
                args: server.args.clone(),
                env: server.env.clone(),
                workspace_folder: server.workspace_folder.clone(),
                language_id: language_id.clone(),
            });
        }
        missing_servers.push(MissingLspServer {
            display_name: if server.display_name.trim().is_empty() {
                server.id.clone()
            } else {
                server.display_name.clone()
            },
            command,
            install_hint: server.install_hint.clone(),
        });
    }
    if missing_servers.is_empty() {
        LspServerResolution::Unconfigured { extension }
    } else {
        LspServerResolution::Missing {
            extension,
            servers: missing_servers,
        }
    }
}

fn format_missing_lsp_server_message(extension: &str, servers: &[MissingLspServer]) -> String {
    let mut lines = vec![format!(
        "No LSP server is installed for file type: {extension}"
    )];
    lines.push("Configured servers for this file type:".to_string());
    for server in servers {
        lines.push(format!("- {} (`{}`)", server.display_name, server.command));
        if let Some(install_hint) = server.install_hint.as_deref() {
            lines.push(format!("  Install: {install_hint}"));
        }
    }
    lines.push(
        "Install one of the servers above or configure a plugin-provided LSP server for this file type."
            .to_string(),
    );
    lines.join("\n")
}

fn resolve_server_cwd(server: &ResolvedLspServer, workspace_root: &Path) -> PathBuf {
    server
        .workspace_folder
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.to_path_buf())
}

fn command_exists(command: &str) -> bool {
    let path = Path::new(command);
    if path.is_absolute() {
        return path.is_file();
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|entry| {
        let candidate = entry.join(command);
        candidate.is_file()
    })
}

fn command_is_usable(command: &str) -> bool {
    if !command_exists(command) {
        return false;
    }
    if Path::new(command)
        .file_name()
        .and_then(|value| value.to_str())
        != Some("rust-analyzer")
    {
        return true;
    }
    Command::new(command)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn resolved_command(command: &str) -> Option<String> {
    let override_name = format!(
        "PUFFER_LSP_COMMAND_{}",
        command
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            })
            .collect::<String>()
    );
    if let Ok(value) = std::env::var(&override_name) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    Some(command.to_string())
}

fn workspace_root(cwd: &Path, file_path: &Path) -> PathBuf {
    file_path
        .parent()
        .and_then(git_toplevel)
        .or_else(|| git_toplevel(cwd))
        .unwrap_or_else(|| cwd.to_path_buf())
}

fn read_lsp_file(file_path: &Path) -> Result<String> {
    let metadata = fs::metadata(file_path)
        .with_context(|| format!("failed to stat {}", file_path.display()))?;
    if metadata.len() > MAX_LSP_FILE_SIZE_BYTES {
        bail!(
            "File too large for LSP analysis ({}MB exceeds 10MB limit)",
            ((metadata.len() as f64) / 1_000_000.0).ceil() as u64
        );
    }
    fs::read_to_string(file_path).with_context(|| format!("failed to read {}", file_path.display()))
}

fn lsp_position(line: usize, character: usize) -> Value {
    json!({
        "line": line.saturating_sub(1),
        "character": character.saturating_sub(1),
    })
}

fn file_uri(path: &Path) -> Result<String> {
    Url::from_file_path(path)
        .map(|url| url.to_string())
        .map_err(|_| anyhow!("failed to encode file URI for {}", path.display()))
}
