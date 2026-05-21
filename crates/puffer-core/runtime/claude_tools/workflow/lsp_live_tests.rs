use super::super::lsp::shutdown_lsp_services;
use super::super::lsp_live_diagnostics::diagnostics_for_file;
use super::*;
use puffer_resources::{LoadedItem, LspServerSpec, PluginSpec, SourceInfo, SourceKind};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tempfile::tempdir;

#[test]
fn execute_lsp_uses_real_stdio_session_for_hover() {
    let _guard = test_env_lock().lock().unwrap();
    let temp = tempdir().unwrap();
    let source = temp.path().join("main.rs");
    fs::write(&source, "fn main() { println!(\"hi\"); }\n").unwrap();
    let server = temp.path().join("mock-rust-analyzer");
    fs::write(&server, mock_lsp_server_script()).unwrap();
    make_executable(&server);
    std::env::set_var(
        "PUFFER_LSP_COMMAND_RUST_ANALYZER",
        server.display().to_string(),
    );
    let resources = test_resources();

    let output = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "hover",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 4,
        }),
    )
    .unwrap();
    std::env::remove_var("PUFFER_LSP_COMMAND_RUST_ANALYZER");

    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["operation"], "hover");
    assert!(parsed["result"]
        .as_str()
        .is_some_and(|value| value.contains("fn main()")));
}

#[test]
fn execute_lsp_rejects_file_outside_workspace() {
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let source = outside.path().join("main.rs");
    fs::write(&source, "fn main() {}\n").unwrap();

    let output = execute_lsp(
        &LoadedResources::default(),
        workspace.path(),
        json!({
            "operation": "hover",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 4,
        }),
    )
    .unwrap();

    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert!(parsed["result"]
        .as_str()
        .is_some_and(|value| value.contains("escapes workspace")));
}

#[test]
fn execute_lsp_runs_call_hierarchy_and_workspace_symbol_requests() {
    let _guard = test_env_lock().lock().unwrap();
    let temp = tempdir().unwrap();
    let source = temp.path().join("lib.rs");
    fs::write(&source, "pub fn demo() {}\n").unwrap();
    let server = temp.path().join("mock-rust-analyzer");
    fs::write(&server, mock_lsp_server_script()).unwrap();
    make_executable(&server);
    std::env::set_var(
        "PUFFER_LSP_COMMAND_RUST_ANALYZER",
        server.display().to_string(),
    );
    let resources = test_resources();

    let outgoing = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "outgoingCalls",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 8,
        }),
    )
    .unwrap();
    let workspace = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "workspaceSymbol",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 1,
        }),
    )
    .unwrap();
    std::env::remove_var("PUFFER_LSP_COMMAND_RUST_ANALYZER");

    let outgoing_json: Value = serde_json::from_str(&outgoing).unwrap();
    let workspace_json: Value = serde_json::from_str(&workspace).unwrap();
    assert!(outgoing_json["result"]
        .as_str()
        .is_some_and(|value| value.contains("helper")));
    assert!(workspace_json["result"]
        .as_str()
        .is_some_and(|value| value.contains("workspace_symbol")));
}

#[test]
fn execute_lsp_reuses_session_and_sends_did_change_for_updated_files() {
    let _guard = test_env_lock().lock().unwrap();
    let temp = tempdir().unwrap();
    let source = temp.path().join("main.rs");
    fs::write(&source, "fn first() {}\n").unwrap();
    let server = temp.path().join("mock-rust-analyzer");
    let log_path = temp.path().join("lsp.log");
    fs::write(&server, mock_lsp_server_script()).unwrap();
    make_executable(&server);
    std::env::set_var(
        "PUFFER_LSP_COMMAND_RUST_ANALYZER",
        server.display().to_string(),
    );
    std::env::set_var("PUFFER_LSP_MOCK_LOG", log_path.display().to_string());
    let resources = test_resources();

    let _ = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "hover",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 4,
        }),
    )
    .unwrap();

    fs::write(&source, "fn second() {}\n").unwrap();

    let _ = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "hover",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 4,
        }),
    )
    .unwrap();

    std::env::remove_var("PUFFER_LSP_COMMAND_RUST_ANALYZER");
    std::env::remove_var("PUFFER_LSP_MOCK_LOG");

    let log = fs::read_to_string(&log_path).unwrap();
    assert_eq!(log.matches("initialize\n").count(), 1);
    assert_eq!(log.matches("textDocument/didOpen\n").count(), 1);
    assert_eq!(log.matches("textDocument/didChange\n").count(), 1);
    assert_eq!(log.matches("textDocument/hover\n").count(), 2);
}

#[test]
#[ignore = "requires a real pyright-langserver binary via PUFFER_REAL_PYRIGHT_LANGSERVER"]
fn execute_lsp_works_on_real_python_workspace_with_pyright() {
    let _guard = test_env_lock().lock().unwrap();
    let server = std::env::var("PUFFER_REAL_PYRIGHT_LANGSERVER")
        .expect("PUFFER_REAL_PYRIGHT_LANGSERVER must point to pyright-langserver");
    let temp = tempdir().unwrap();
    let helper = temp.path().join("helper.py");
    let main = temp.path().join("main.py");
    fs::write(
        &helper,
        "def add(x: int, y: int) -> int:\n    return x + y\n",
    )
    .unwrap();
    fs::write(&main, "from helper import add\n\nresult = add(1, 2)\n").unwrap();
    std::env::set_var("PUFFER_LSP_COMMAND_PYRIGHT_LANGSERVER", server);
    let resources = python_test_resources();

    let definition = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "goToDefinition",
            "filePath": main.display().to_string(),
            "line": 3,
            "character": 10,
        }),
    )
    .unwrap();
    let hover = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "hover",
            "filePath": main.display().to_string(),
            "line": 3,
            "character": 10,
        }),
    )
    .unwrap();
    let document = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "documentSymbol",
            "filePath": helper.display().to_string(),
            "line": 1,
            "character": 1,
        }),
    )
    .unwrap();

    std::env::remove_var("PUFFER_LSP_COMMAND_PYRIGHT_LANGSERVER");

    let definition_json: Value = serde_json::from_str(&definition).unwrap();
    let hover_json: Value = serde_json::from_str(&hover).unwrap();
    let document_json: Value = serde_json::from_str(&document).unwrap();
    assert!(definition_json["result"]
        .as_str()
        .is_some_and(|value| value.contains("helper.py")));
    assert!(hover_json["result"]
        .as_str()
        .is_some_and(|value| value.contains("add")));
    assert!(document_json["result"]
        .as_str()
        .is_some_and(|value| value.contains("add")));
}

#[test]
fn execute_lsp_persists_publish_diagnostics_notifications() {
    let _guard = test_env_lock().lock().unwrap();
    let temp = tempdir().unwrap();
    let source = temp.path().join("main.rs");
    fs::write(&source, "fn first() {}\n").unwrap();
    let server = temp.path().join("mock-rust-analyzer");
    fs::write(&server, mock_lsp_server_script()).unwrap();
    make_executable(&server);
    std::env::set_var(
        "PUFFER_LSP_COMMAND_RUST_ANALYZER",
        server.display().to_string(),
    );
    std::env::set_var("PUFFER_LSP_MOCK_DIAGNOSTICS", "1");
    let resources = test_resources();
    let file_uri = url::Url::from_file_path(&source).unwrap().to_string();

    let _ = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "hover",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 4,
        }),
    )
    .unwrap();

    let initial = diagnostics_for_file(temp.path(), &file_uri).unwrap();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].message, "opened diagnostic");

    fs::write(&source, "fn second() {}\n").unwrap();
    let _ = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "hover",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 4,
        }),
    )
    .unwrap();

    let updated = diagnostics_for_file(temp.path(), &file_uri).unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].message, "changed diagnostic");

    std::env::remove_var("PUFFER_LSP_COMMAND_RUST_ANALYZER");
    std::env::remove_var("PUFFER_LSP_MOCK_DIAGNOSTICS");
}

#[test]
fn execute_lsp_reports_persisted_diagnostics() {
    let _guard = test_env_lock().lock().unwrap();
    let temp = tempdir().unwrap();
    let source = temp.path().join("main.rs");
    fs::write(&source, "fn first() {}\n").unwrap();
    let server = temp.path().join("mock-rust-analyzer");
    fs::write(&server, mock_lsp_server_script()).unwrap();
    make_executable(&server);
    std::env::set_var(
        "PUFFER_LSP_COMMAND_RUST_ANALYZER",
        server.display().to_string(),
    );
    std::env::set_var("PUFFER_LSP_MOCK_DIAGNOSTICS", "1");
    let resources = test_resources();

    let _ = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "hover",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 4,
        }),
    )
    .unwrap();

    let diagnostics = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "diagnostics",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 1,
        }),
    )
    .unwrap();

    std::env::remove_var("PUFFER_LSP_COMMAND_RUST_ANALYZER");
    std::env::remove_var("PUFFER_LSP_MOCK_DIAGNOSTICS");

    let diagnostics_json: Value = serde_json::from_str(&diagnostics).unwrap();
    assert!(diagnostics_json["result"]
        .as_str()
        .is_some_and(|value| value.contains("opened diagnostic")));
}

#[test]
fn execute_lsp_reports_missing_server_install_guidance() {
    let temp = tempdir().unwrap();
    let source = temp.path().join("main.py");
    fs::write(&source, "print('hi')\n").unwrap();
    let resources = missing_python_server_resources();

    let output = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "hover",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 1,
        }),
    )
    .unwrap();

    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert!(parsed["result"]
        .as_str()
        .is_some_and(|value| value.contains("npm install -g pyright")));
    assert!(parsed["result"]
        .as_str()
        .is_some_and(|value| value.contains("No LSP server is installed")));
}

#[test]
fn shutdown_lsp_services_stops_cached_sessions() {
    let _guard = test_env_lock().lock().unwrap();
    let temp = tempdir().unwrap();
    let source = temp.path().join("main.rs");
    fs::write(&source, "fn first() {}\n").unwrap();
    let server = temp.path().join("mock-rust-analyzer");
    let log_path = temp.path().join("shutdown.log");
    fs::write(&server, mock_lsp_server_script()).unwrap();
    make_executable(&server);
    std::env::set_var(
        "PUFFER_LSP_COMMAND_RUST_ANALYZER",
        server.display().to_string(),
    );
    std::env::set_var("PUFFER_LSP_MOCK_LOG", log_path.display().to_string());
    let resources = test_resources();

    let _ = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "hover",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 4,
        }),
    )
    .unwrap();

    shutdown_lsp_services().unwrap();

    let _ = execute_lsp(
        &resources,
        temp.path(),
        json!({
            "operation": "hover",
            "filePath": source.display().to_string(),
            "line": 1,
            "character": 4,
        }),
    )
    .unwrap();

    std::env::remove_var("PUFFER_LSP_COMMAND_RUST_ANALYZER");
    std::env::remove_var("PUFFER_LSP_MOCK_LOG");

    let log = fs::read_to_string(&log_path).unwrap();
    assert_eq!(log.matches("initialize\n").count(), 2);
}

fn test_resources() -> puffer_resources::LoadedResources {
    puffer_resources::LoadedResources {
        plugins: vec![LoadedItem {
            value: PluginSpec {
                id: "puffer-builtins".to_string(),
                display_name: "Puffer Builtins".to_string(),
                description: "builtin lsp".to_string(),
                commands: Vec::new(),
                skills: Vec::new(),
                agents: Vec::new(),
                mcp_servers: Vec::new(),
                lsp_servers: vec![LspServerSpec {
                    id: "rust-analyzer".to_string(),
                    display_name: "Rust Analyzer".to_string(),
                    command: "rust-analyzer".to_string(),
                    install_hint: Some("rustup component add rust-analyzer".to_string()),
                    args: Vec::new(),
                    extension_to_language: std::collections::BTreeMap::from([(
                        ".rs".to_string(),
                        "rust".to_string(),
                    )]),
                    env: Default::default(),
                    workspace_folder: None,
                }],
            },
            source_info: SourceInfo {
                path: "resources/plugins/puffer-builtins.yaml".into(),
                kind: SourceKind::Builtin,
            },
        }],
        ..Default::default()
    }
}

fn python_test_resources() -> puffer_resources::LoadedResources {
    puffer_resources::LoadedResources {
        plugins: vec![LoadedItem {
            value: PluginSpec {
                id: "puffer-builtins".to_string(),
                display_name: "Puffer Builtins".to_string(),
                description: "builtin lsp".to_string(),
                commands: Vec::new(),
                skills: Vec::new(),
                agents: Vec::new(),
                mcp_servers: Vec::new(),
                lsp_servers: vec![LspServerSpec {
                    id: "pyright-langserver".to_string(),
                    display_name: "Pyright".to_string(),
                    command: "pyright-langserver".to_string(),
                    install_hint: Some("npm install -g pyright".to_string()),
                    args: vec!["--stdio".to_string()],
                    extension_to_language: std::collections::BTreeMap::from([(
                        ".py".to_string(),
                        "python".to_string(),
                    )]),
                    env: Default::default(),
                    workspace_folder: None,
                }],
            },
            source_info: SourceInfo {
                path: "resources/plugins/puffer-builtins.yaml".into(),
                kind: SourceKind::Builtin,
            },
        }],
        ..Default::default()
    }
}

fn missing_python_server_resources() -> puffer_resources::LoadedResources {
    puffer_resources::LoadedResources {
        plugins: vec![LoadedItem {
            value: PluginSpec {
                id: "puffer-builtins".to_string(),
                display_name: "Puffer Builtins".to_string(),
                description: "builtin lsp".to_string(),
                commands: Vec::new(),
                skills: Vec::new(),
                agents: Vec::new(),
                mcp_servers: Vec::new(),
                lsp_servers: vec![LspServerSpec {
                    id: "pyright-langserver".to_string(),
                    display_name: "Pyright".to_string(),
                    command: "missing-pyright-langserver".to_string(),
                    install_hint: Some("npm install -g pyright".to_string()),
                    args: vec!["--stdio".to_string()],
                    extension_to_language: std::collections::BTreeMap::from([(
                        ".py".to_string(),
                        "python".to_string(),
                    )]),
                    env: Default::default(),
                    workspace_folder: None,
                }],
            },
            source_info: SourceInfo {
                path: "resources/plugins/puffer-builtins.yaml".into(),
                kind: SourceKind::Builtin,
            },
        }],
        ..Default::default()
    }
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn test_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn mock_lsp_server_script() -> &'static str {
    r#"#!/usr/bin/env bash
set -euo pipefail

send() {
  local body="$1"
  printf 'Content-Length: %s\r\n\r\n%s' "${#body}" "$body"
}

while true; do
  len=""
  while IFS= read -r line; do
    line="${line%$'\r'}"
    [ -z "$line" ] && break
    case "$line" in
      Content-Length:*) len="${line#Content-Length: }" ;;
    esac
  done
  [ -z "$len" ] && exit 0
  body="$(dd bs=1 count="$len" 2>/dev/null)"
  method="$(printf '%s' "$body" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')"
  id="$(printf '%s' "$body" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')"
  if [ -n "${PUFFER_LSP_MOCK_LOG:-}" ] && [ -n "$method" ]; then
    printf '%s\n' "$method" >> "$PUFFER_LSP_MOCK_LOG"
  fi

  case "$method" in
    initialize)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"capabilities\":{\"hoverProvider\":true,\"definitionProvider\":true,\"referencesProvider\":true,\"documentSymbolProvider\":true,\"workspaceSymbolProvider\":true,\"implementationProvider\":true,\"callHierarchyProvider\":true}}}"
      ;;
    textDocument/didOpen)
      if [ "${PUFFER_LSP_MOCK_DIAGNOSTICS:-}" = "1" ]; then
        uri="$(printf '%s' "$body" | sed -n 's/.*"uri":"\([^"]*\)".*/\1/p')"
        send "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{\"uri\":\"$uri\",\"diagnostics\":[{\"range\":{\"start\":{\"line\":0,\"character\":0},\"end\":{\"line\":0,\"character\":3}},\"severity\":1,\"source\":\"mock-lsp\",\"message\":\"opened diagnostic\"}]}}"
      fi
      ;;
    textDocument/didChange)
      if [ "${PUFFER_LSP_MOCK_DIAGNOSTICS:-}" = "1" ]; then
        uri="$(printf '%s' "$body" | sed -n 's/.*"uri":"\([^"]*\)".*/\1/p')"
        send "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{\"uri\":\"$uri\",\"diagnostics\":[{\"range\":{\"start\":{\"line\":0,\"character\":0},\"end\":{\"line\":0,\"character\":3}},\"severity\":2,\"source\":\"mock-lsp\",\"message\":\"changed diagnostic\"}]}}"
      fi
      ;;
    textDocument/hover)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"contents\":{\"kind\":\"markdown\",\"value\":\"fn main() -> ()\"}}}"
      ;;
    textDocument/definition)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"uri\":\"file:///tmp/mock.rs\",\"range\":{\"start\":{\"line\":0,\"character\":0},\"end\":{\"line\":0,\"character\":2}}}}"
      ;;
    textDocument/references)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":[{\"uri\":\"file:///tmp/mock.rs\",\"range\":{\"start\":{\"line\":1,\"character\":4},\"end\":{\"line\":1,\"character\":8}}},{\"uri\":\"file:///tmp/other.rs\",\"range\":{\"start\":{\"line\":2,\"character\":1},\"end\":{\"line\":2,\"character\":5}}}]}"
      ;;
    textDocument/documentSymbol)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":[{\"name\":\"demo\",\"kind\":12,\"range\":{\"start\":{\"line\":0,\"character\":0},\"end\":{\"line\":0,\"character\":4}},\"selectionRange\":{\"start\":{\"line\":0,\"character\":0},\"end\":{\"line\":0,\"character\":4}}}]}"
      ;;
    workspace/symbol)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":[{\"name\":\"workspace_symbol\",\"kind\":12,\"location\":{\"uri\":\"file:///tmp/workspace.rs\",\"range\":{\"start\":{\"line\":4,\"character\":2},\"end\":{\"line\":4,\"character\":10}}}}]}"
      ;;
    textDocument/implementation)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":[{\"uri\":\"file:///tmp/impl.rs\",\"range\":{\"start\":{\"line\":5,\"character\":3},\"end\":{\"line\":5,\"character\":7}}}]}"
      ;;
    textDocument/prepareCallHierarchy)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":[{\"name\":\"demo\",\"kind\":12,\"uri\":\"file:///tmp/calls.rs\",\"range\":{\"start\":{\"line\":0,\"character\":0},\"end\":{\"line\":0,\"character\":4}},\"selectionRange\":{\"start\":{\"line\":0,\"character\":0},\"end\":{\"line\":0,\"character\":4}}}]}"
      ;;
    callHierarchy/incomingCalls)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":[{\"from\":{\"name\":\"caller\",\"kind\":12,\"uri\":\"file:///tmp/caller.rs\",\"range\":{\"start\":{\"line\":2,\"character\":1},\"end\":{\"line\":2,\"character\":7}},\"selectionRange\":{\"start\":{\"line\":2,\"character\":1},\"end\":{\"line\":2,\"character\":7}}},\"fromRanges\":[]}]}"
      ;;
    callHierarchy/outgoingCalls)
      send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":[{\"to\":{\"name\":\"helper\",\"kind\":12,\"uri\":\"file:///tmp/helper.rs\",\"range\":{\"start\":{\"line\":3,\"character\":1},\"end\":{\"line\":3,\"character\":7}},\"selectionRange\":{\"start\":{\"line\":3,\"character\":1},\"end\":{\"line\":3,\"character\":7}}},\"fromRanges\":[]}]}"
      ;;
    "")
      if [ -n "$id" ]; then
        send "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":null}"
      fi
      ;;
  esac
done
"#
}
