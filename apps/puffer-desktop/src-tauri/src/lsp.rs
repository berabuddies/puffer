use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;
use url::Url;

use crate::files::validate_path;

const LSP_REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_LSP_FILE_SIZE_BYTES: u64 = 10 * 1024 * 1024;

const LSP_OPERATIONS: &[&str] = &[
    "hover",
    "goToDefinition",
    "findReferences",
    "incomingCalls",
    "outgoingCalls",
];

pub(crate) fn inspect(params: &Value, allowed_roots: &[PathBuf]) -> Result<Value> {
    let raw = params
        .get("path")
        .and_then(Value::as_str)
        .context("missing path")?;
    let file_path = validate_path(allowed_roots, raw)?;
    let cwd = params
        .get("cwd")
        .and_then(Value::as_str)
        .and_then(|raw| validate_path(allowed_roots, raw).ok())
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| {
            file_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        });
    let line = params.get("line").and_then(Value::as_u64).unwrap_or(0) as usize;
    let character = params.get("character").and_then(Value::as_u64).unwrap_or(0) as usize;

    let mut operations = serde_json::Map::new();
    for operation in LSP_OPERATIONS {
        let result = execute_lsp_query(operation, &cwd, &file_path, line + 1, character + 1)?;
        let stop_after_error = result
            .get("result")
            .and_then(Value::as_str)
            .is_some_and(|text| {
                text.starts_with("Error performing ")
                    || text.starts_with("No LSP server available")
                    || text.starts_with("No LSP server installed")
            });
        operations.insert((*operation).to_string(), result);
        if stop_after_error {
            break;
        }
    }

    Ok(json!({
        "path": file_path.display().to_string(),
        "cwd": cwd.display().to_string(),
        "line": line,
        "character": character,
        "operations": operations,
    }))
}

fn execute_lsp_query(
    operation: &str,
    cwd: &Path,
    file_path: &Path,
    line: usize,
    character: usize,
) -> Result<Value> {
    let output = match execute_lsp_inner(operation, cwd, file_path, line, character) {
        Ok(output) => output,
        Err(error) => LspOperationOutput {
            operation: operation.to_string(),
            file_path: file_path.display().to_string(),
            result: format!("Error performing {operation}: {error:#}"),
            result_count: None,
            file_count: None,
        },
    };
    Ok(serde_json::to_value(output)?)
}

fn execute_lsp_inner(
    operation: &str,
    cwd: &Path,
    file_path: &Path,
    line: usize,
    character: usize,
) -> Result<LspOperationOutput> {
    validate_lsp_input(file_path)?;
    let Some(server) = resolve_lsp_server(file_path) else {
        return Ok(LspOperationOutput::message(
            operation,
            file_path,
            format!(
                "No LSP server available for file type: {}",
                file_extension(file_path)
            ),
        ));
    };
    if !command_exists(&server.command) {
        return Ok(LspOperationOutput::message(
            operation,
            file_path,
            format!(
                "No LSP server installed for file type: {}\nConfigured server: {} (`{}`)\nInstall: {}",
                file_extension(file_path),
                server.display_name,
                server.command,
                server.install_hint,
            ),
        ));
    }

    let workspace_root = workspace_root(cwd, file_path);
    let file_uri = file_uri(file_path)?;
    let content = read_lsp_file(file_path)?;
    let mut session = LspSession::start(&server, &workspace_root)?;
    session.initialize(&workspace_root)?;
    session.open_file(&file_uri, &server.language_id, &content)?;
    let result = run_operation(
        &mut session,
        operation,
        &file_uri,
        line,
        character,
        &workspace_root,
    )?;
    let _ = session.shutdown();
    Ok(result.with_metadata(operation, file_path))
}

fn run_operation(
    session: &mut LspSession,
    operation: &str,
    file_uri: &str,
    line: usize,
    character: usize,
    workspace_root: &Path,
) -> Result<LspExecutionResult> {
    match operation {
        "hover" => {
            let response = session.request(
                "textDocument/hover",
                json!({
                    "textDocument": { "uri": file_uri },
                    "position": lsp_position(line, character),
                }),
            )?;
            Ok(format_hover(response))
        }
        "goToDefinition" => {
            let response = session.request(
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": file_uri },
                    "position": lsp_position(line, character),
                }),
            )?;
            format_locations("definition", response, workspace_root)
        }
        "findReferences" => {
            let response = session.request(
                "textDocument/references",
                json!({
                    "textDocument": { "uri": file_uri },
                    "position": lsp_position(line, character),
                    "context": { "includeDeclaration": true },
                }),
            )?;
            format_references(response, workspace_root)
        }
        "incomingCalls" | "outgoingCalls" => Ok(LspExecutionResult {
            result: "No call hierarchy item found at this position.".to_string(),
            result_count: Some(0),
            file_count: Some(0),
        }),
        other => bail!("unsupported LSP operation `{other}`"),
    }
}

#[derive(Clone)]
struct LspServer {
    display_name: &'static str,
    command: String,
    args: Vec<String>,
    language_id: &'static str,
    install_hint: &'static str,
}

fn resolve_lsp_server(file_path: &Path) -> Option<LspServer> {
    let ext = file_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let server = match ext.as_str() {
        "rs" => LspServer {
            display_name: "rust-analyzer",
            command: env_override("rust-analyzer"),
            args: vec![],
            language_id: "rust",
            install_hint: "rustup component add rust-analyzer",
        },
        "ts" | "tsx" | "js" | "jsx" => LspServer {
            display_name: "TypeScript Language Server",
            command: env_override("typescript-language-server"),
            args: vec!["--stdio".to_string()],
            language_id: if ext.starts_with('t') {
                "typescript"
            } else {
                "javascript"
            },
            install_hint: "npm install -g typescript typescript-language-server",
        },
        "py" => LspServer {
            display_name: "Pyright",
            command: env_override("pyright-langserver"),
            args: vec!["--stdio".to_string()],
            language_id: "python",
            install_hint: "npm install -g pyright",
        },
        "go" => LspServer {
            display_name: "gopls",
            command: env_override("gopls"),
            args: vec![],
            language_id: "go",
            install_hint: "go install golang.org/x/tools/gopls@latest",
        },
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" => LspServer {
            display_name: "clangd",
            command: env_override("clangd"),
            args: vec![],
            language_id: if ext == "c" { "c" } else { "cpp" },
            install_hint: "install clangd from LLVM or your system package manager",
        },
        "json" => LspServer {
            display_name: "vscode-json-language-server",
            command: env_override("vscode-json-language-server"),
            args: vec!["--stdio".to_string()],
            language_id: "json",
            install_hint: "npm install -g vscode-langservers-extracted",
        },
        "svelte" => LspServer {
            display_name: "svelte-language-server",
            command: env_override("svelteserver"),
            args: vec!["--stdio".to_string()],
            language_id: "svelte",
            install_hint: "npm install -g svelte-language-server",
        },
        _ => return None,
    };
    Some(server)
}

struct LspSession {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Result<Value>>,
    next_id: u64,
}

impl LspSession {
    fn start(server: &LspServer, workspace_root: &Path) -> Result<Self> {
        let mut child = Command::new(&server.command)
            .args(&server.args)
            .current_dir(workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start LSP server `{}`", server.command))?;
        let stdin = child.stdin.take().context("LSP stdin unavailable")?;
        let stdout = child.stdout.take().context("LSP stdout unavailable")?;
        Ok(Self {
            child,
            stdin,
            messages: spawn_message_reader(stdout),
            next_id: 1,
        })
    }

    fn initialize(&mut self, workspace_root: &Path) -> Result<()> {
        let workspace_uri = file_uri(workspace_root)?;
        self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "clientInfo": { "name": "corbina", "version": env!("CARGO_PKG_VERSION") },
                "rootPath": workspace_root.display().to_string(),
                "rootUri": workspace_uri,
                "workspaceFolders": [{
                    "uri": workspace_uri,
                    "name": workspace_root.file_name().and_then(|v| v.to_str()).unwrap_or("workspace")
                }],
                "capabilities": {
                    "workspace": { "configuration": false, "workspaceFolders": false },
                    "textDocument": {
                        "hover": { "dynamicRegistration": false, "contentFormat": ["markdown", "plaintext"] },
                        "definition": { "dynamicRegistration": false, "linkSupport": true },
                        "references": { "dynamicRegistration": false },
                        "synchronization": { "dynamicRegistration": false, "didSave": true }
                    },
                    "general": { "positionEncodings": ["utf-16"] }
                }
            }),
        )?;
        self.notify("initialized", json!({}))
    }

    fn open_file(&mut self, file_uri: &str, language_id: &str, content: &str) -> Result<()> {
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

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
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
                .recv_timeout(LSP_REQUEST_TIMEOUT)
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
            if message.get("method").is_some() {
                continue;
            }
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("LSP request `{method}` failed: {error}");
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

    fn respond_to_server_request(&mut self, message: &Value) -> Result<()> {
        let id = message
            .get("id")
            .cloned()
            .ok_or_else(|| anyhow!("LSP server request missing id"))?;
        let result = match message.get("method").and_then(Value::as_str).unwrap_or("") {
            "workspace/configuration" => {
                let items = message
                    .pointer("/params/items")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().map(|_| Value::Null).collect::<Vec<_>>())
                    .unwrap_or_default();
                Value::Array(items)
            }
            _ => Value::Null,
        };
        self.write_message(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
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

    fn shutdown(&mut self) -> Result<()> {
        let _ = self.request("shutdown", json!({}));
        let _ = self.notify("exit", json!({}));
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LspOperationOutput {
    operation: String,
    file_path: String,
    result: String,
    result_count: Option<usize>,
    file_count: Option<usize>,
}

impl LspOperationOutput {
    fn message(operation: &str, file_path: &Path, result: String) -> Self {
        Self {
            operation: operation.to_string(),
            file_path: file_path.display().to_string(),
            result,
            result_count: None,
            file_count: None,
        }
    }
}

struct LspExecutionResult {
    result: String,
    result_count: Option<usize>,
    file_count: Option<usize>,
}

impl LspExecutionResult {
    fn with_metadata(self, operation: &str, file_path: &Path) -> LspOperationOutput {
        LspOperationOutput {
            operation: operation.to_string(),
            file_path: file_path.display().to_string(),
            result: self.result,
            result_count: self.result_count,
            file_count: self.file_count,
        }
    }
}

#[derive(Clone)]
struct LocationSummary {
    file_path: String,
    line: u64,
    character: u64,
}

fn format_hover(result: Value) -> LspExecutionResult {
    let text = extract_hover_text(&result);
    LspExecutionResult {
        result: if text.trim().is_empty() {
            "No hover information available.".to_string()
        } else {
            text
        },
        result_count: Some(usize::from(!result.is_null())),
        file_count: Some(usize::from(!result.is_null())),
    }
}

fn format_locations(kind: &str, result: Value, cwd: &Path) -> Result<LspExecutionResult> {
    let locations = extract_locations(&result, cwd)?;
    if locations.is_empty() {
        return Ok(LspExecutionResult {
            result: format!("No {kind} found."),
            result_count: Some(0),
            file_count: Some(0),
        });
    }
    let mut lines = vec![format!(
        "Found {} {}{}:",
        locations.len(),
        kind,
        if locations.len() == 1 { "" } else { "s" }
    )];
    for location in &locations {
        lines.push(format!(
            "- {}:{}:{}",
            location.file_path, location.line, location.character
        ));
    }
    Ok(LspExecutionResult {
        result: lines.join("\n"),
        result_count: Some(locations.len()),
        file_count: Some(unique_file_count(&locations)),
    })
}

fn format_references(result: Value, cwd: &Path) -> Result<LspExecutionResult> {
    let locations = extract_locations(&result, cwd)?;
    if locations.is_empty() {
        return Ok(LspExecutionResult {
            result: "No references found.".to_string(),
            result_count: Some(0),
            file_count: Some(0),
        });
    }
    let mut grouped = std::collections::BTreeMap::<String, Vec<LocationSummary>>::new();
    for location in locations.clone() {
        grouped
            .entry(location.file_path.clone())
            .or_default()
            .push(location);
    }
    let mut lines = vec![format!(
        "Found {} references across {} files:",
        locations.len(),
        grouped.len()
    )];
    for (file, entries) in grouped {
        lines.push(format!("\n{file}:"));
        for entry in entries {
            lines.push(format!("  - line {}:{}", entry.line, entry.character));
        }
    }
    Ok(LspExecutionResult {
        result: lines.join("\n"),
        result_count: Some(locations.len()),
        file_count: Some(unique_file_count(&locations)),
    })
}

fn extract_hover_text(value: &Value) -> String {
    let Some(contents) = value.get("contents") else {
        return String::new();
    };
    match contents {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(extract_marked_string)
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(_) => extract_marked_string(contents),
        _ => String::new(),
    }
}

fn extract_marked_string(value: &Value) -> String {
    value
        .get("value")
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .unwrap_or("")
        .to_string()
}

fn extract_locations(value: &Value, cwd: &Path) -> Result<Vec<LocationSummary>> {
    let mut out = Vec::new();
    match value {
        Value::Array(values) => {
            for value in values {
                if let Some(location) = parse_location(value, cwd)? {
                    out.push(location);
                }
            }
        }
        Value::Object(_) => {
            if let Some(location) = parse_location(value, cwd)? {
                out.push(location);
            }
        }
        _ => {}
    }
    Ok(out)
}

fn parse_location(value: &Value, cwd: &Path) -> Result<Option<LocationSummary>> {
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))
        .and_then(Value::as_str);
    let range = value
        .get("range")
        .or_else(|| value.get("targetSelectionRange"))
        .or_else(|| value.get("targetRange"));
    let (Some(uri), Some(range)) = (uri, range) else {
        return Ok(None);
    };
    let file_path = path_from_uri(uri)?;
    let display = file_path
        .strip_prefix(cwd)
        .unwrap_or(file_path.as_path())
        .display()
        .to_string();
    Ok(Some(LocationSummary {
        file_path: display,
        line: range
            .pointer("/start/line")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1,
        character: range
            .pointer("/start/character")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1,
    }))
}

fn unique_file_count(locations: &[LocationSummary]) -> usize {
    locations
        .iter()
        .map(|location| location.file_path.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

fn spawn_message_reader<R>(stdout: R) -> Receiver<Result<Value>>
where
    R: Read + Send + 'static,
{
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
    Ok(Some(serde_json::from_slice::<Value>(&body)?))
}

fn is_server_request(message: &Value) -> bool {
    message.get("id").is_some() && message.get("method").is_some()
}

fn validate_lsp_input(file_path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(file_path)
        .with_context(|| format!("failed to stat {}", file_path.display()))?;
    if !metadata.is_file() {
        bail!("Path is not a file: {}", file_path.display());
    }
    Ok(())
}

fn read_lsp_file(file_path: &Path) -> Result<String> {
    let metadata = std::fs::metadata(file_path)
        .with_context(|| format!("failed to stat {}", file_path.display()))?;
    if metadata.len() > MAX_LSP_FILE_SIZE_BYTES {
        bail!(
            "File too large for LSP analysis ({}MB exceeds 10MB limit)",
            ((metadata.len() as f64) / 1_000_000.0).ceil() as u64
        );
    }
    std::fs::read_to_string(file_path)
        .with_context(|| format!("failed to read {}", file_path.display()))
}

fn lsp_position(line: usize, character: usize) -> Value {
    json!({
        "line": line.saturating_sub(1),
        "character": character.saturating_sub(1),
    })
}

fn workspace_root(cwd: &Path, file_path: &Path) -> PathBuf {
    file_path
        .parent()
        .and_then(git_toplevel)
        .or_else(|| git_toplevel(cwd))
        .unwrap_or_else(|| cwd.to_path_buf())
}

fn git_toplevel(path: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string()))
}

fn file_uri(path: &Path) -> Result<String> {
    Url::from_file_path(path)
        .map(|url| url.to_string())
        .map_err(|_| anyhow!("failed to encode file URI for {}", path.display()))
}

fn path_from_uri(uri: &str) -> Result<PathBuf> {
    let url = Url::parse(uri).context("parse file uri")?;
    url.to_file_path()
        .map_err(|_| anyhow!("failed to decode file URI `{uri}`"))
}

fn file_extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_else(|| "<unknown>".to_string())
}

fn env_override(command: &str) -> String {
    let key = format!(
        "CORBINA_LSP_COMMAND_{}",
        command
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            })
            .collect::<String>()
    );
    std::env::var(&key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| command.to_string())
}

fn command_exists(command: &str) -> bool {
    let path = Path::new(command);
    if path.is_absolute() {
        return path.is_file();
    }
    std::env::var_os("PATH")
        .map(|path_var| std::env::split_paths(&path_var).any(|entry| entry.join(command).is_file()))
        .unwrap_or(false)
}
