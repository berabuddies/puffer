use std::fs;
use std::path::{Path, PathBuf};

const MAX_RUST_FILE_LINES: usize = 1000;

const OVERSIZED_FILE_BASELINE: &[&str] = &[
    "crates/puffer-cli/src/benchmark_run.rs",
    "crates/puffer-cli/src/command_surface.rs",
    "crates/puffer-cli/src/daemon.rs",
    "crates/puffer-cli/src/desktop_api.rs",
    "crates/puffer-cli/src/main.rs",
    "crates/puffer-cli/tests/daemon_turn_smoke.rs",
    "crates/puffer-core/command.rs",
    "crates/puffer-core/command_helpers/tasks.rs",
    "crates/puffer-core/memory.rs",
    "crates/puffer-core/runner_mcp/connection_manager.rs",
    "crates/puffer-core/runtime/agent_loop.rs",
    "crates/puffer-core/runtime/anthropic.rs",
    "crates/puffer-core/runtime/browser_auto_review.rs",
    "crates/puffer-core/runtime/claude_tools/mod.rs",
    "crates/puffer-core/runtime/claude_tools/workflow/support.rs",
    "crates/puffer-core/runtime/openai.rs",
    "crates/puffer-core/runtime/openai/conversation.rs",
    "crates/puffer-core/runtime/reflection.rs",
    "crates/puffer-core/runtime/tests.rs",
    "crates/puffer-core/runtime/tests/agent_loop_e2e.rs",
    "crates/puffer-core/runtime/tests/tool_execution.rs",
    "crates/puffer-core/runtime/tests/tool_execution/browser_permissions.rs",
    "crates/puffer-core/runtime.rs",
    "crates/puffer-core/state.rs",
    "crates/puffer-resources/src/loader.rs",
    "crates/puffer-runner-grpc/tests/grpc_e2e.rs",
    "crates/puffer-tui/src/flow.rs",
    "crates/puffer-tui/src/flow_tests.rs",
    "crates/puffer-tui/src/lib.rs",
    "crates/puffer-tui/src/render/tests.rs",
    "crates/puffer-tui/src/state.rs",
];

const MISSING_DOC_BASELINE: &[(&str, &str)] = &[
    ("crates/puffer-cli/src/daemon_fs_watch.rs", "new"),
    ("crates/puffer-connector-core/src/runtime.rs", "new"),
    (
        "crates/puffer-connector-core/src/session_map.rs",
        "is_empty",
    ),
    ("crates/puffer-connector-core/src/traits.rs", "other"),
    ("crates/puffer-connector-discord/src/connector.rs", "config"),
    ("crates/puffer-connector-discord/src/connector.rs", "new"),
    ("crates/puffer-connector-email/src/connector.rs", "config"),
    ("crates/puffer-connector-email/src/connector.rs", "new"),
    ("crates/puffer-connector-matrix/src/connector.rs", "config"),
    ("crates/puffer-connector-matrix/src/connector.rs", "new"),
    ("crates/puffer-connector-slack/src/connector.rs", "config"),
    ("crates/puffer-connector-slack/src/connector.rs", "new"),
    (
        "crates/puffer-connector-telegram/src/connector.rs",
        "config",
    ),
    ("crates/puffer-connector-telegram/src/connector.rs", "new"),
    ("crates/puffer-connector-webhook/src/connector.rs", "config"),
    ("crates/puffer-connector-webhook/src/connector.rs", "new"),
    ("crates/puffer-core/memory.rs", "execute_memory_tool"),
    ("crates/puffer-core/memory.rs", "flush_project_memory"),
    ("crates/puffer-core/memory.rs", "load"),
    ("crates/puffer-core/memory.rs", "project_memory_path"),
    ("crates/puffer-core/memory.rs", "project_memory_status"),
    (
        "crates/puffer-core/memory.rs",
        "project_memory_turn_completed",
    ),
    (
        "crates/puffer-core/memory.rs",
        "spawn_project_memory_review",
    ),
    ("crates/puffer-core/runner_adapter.rs", "new"),
    ("crates/puffer-core/runner_adapter.rs", "with_sandbox_roots"),
    ("crates/puffer-core/runner_mcp/connection_manager.rs", "new"),
    ("crates/puffer-core/runner_mcp/host.rs", "get_prompt"),
    ("crates/puffer-core/runner_mcp/host.rs", "list_prompts"),
    (
        "crates/puffer-core/runtime/claude_tools/mod.rs",
        "execute_workflow_tool",
    ),
    (
        "crates/puffer-core/runtime/claude_tools/workflow/goal.rs",
        "execute_create_goal",
    ),
    (
        "crates/puffer-core/runtime/claude_tools/workflow/goal.rs",
        "execute_get_goal",
    ),
    (
        "crates/puffer-core/runtime/claude_tools/workflow/goal.rs",
        "execute_update_goal",
    ),
    (
        "crates/puffer-core/runtime/claude_tools/write_stdin.rs",
        "execute",
    ),
    (
        "crates/puffer-core/runtime/openai/conversation.rs",
        "assistant_message",
    ),
    ("crates/puffer-core/runtime/openai/conversation.rs", "error"),
    (
        "crates/puffer-core/runtime/openai/conversation.rs",
        "success",
    ),
    (
        "crates/puffer-core/runtime/openai/conversation.rs",
        "system_message",
    ),
    (
        "crates/puffer-core/runtime/openai/conversation.rs",
        "user_message",
    ),
    ("crates/puffer-core/runtime/process_store.rs", "allocate_id"),
    (
        "crates/puffer-core/runtime/process_store.rs",
        "collect_output",
    ),
    (
        "crates/puffer-core/runtime/process_store.rs",
        "collect_output_since",
    ),
    (
        "crates/puffer-core/runtime/process_store.rs",
        "drain_exited",
    ),
    ("crates/puffer-core/runtime/process_store.rs", "exit_code"),
    ("crates/puffer-core/runtime/process_store.rs", "get_mut"),
    ("crates/puffer-core/runtime/process_store.rs", "has_exited"),
    ("crates/puffer-core/runtime/process_store.rs", "insert"),
    ("crates/puffer-core/runtime/process_store.rs", "peek"),
    ("crates/puffer-core/runtime/process_store.rs", "remove"),
    ("crates/puffer-core/runtime/process_store.rs", "terminate"),
    (
        "crates/puffer-core/runtime/process_store.rs",
        "terminate_all",
    ),
    (
        "crates/puffer-core/runtime/process_store.rs",
        "total_output_bytes",
    ),
    ("crates/puffer-core/runtime/process_store.rs", "write_stdin"),
    ("crates/puffer-core/state.rs", "is_active"),
    ("crates/puffer-core/state.rs", "memory_flush_enabled"),
    ("crates/puffer-core/state.rs", "memory_flush_min_turns"),
    ("crates/puffer-core/state.rs", "memory_review_enabled"),
    (
        "crates/puffer-core/state.rs",
        "memory_review_nudge_interval",
    ),
    ("crates/puffer-core/state.rs", "refresh_project_memory"),
    (
        "crates/puffer-core/tests/mcp_stub/stub_server.rs",
        "echo_schema",
    ),
    (
        "crates/puffer-core/tests/mcp_stub/stub_server.rs",
        "empty_object_schema",
    ),
    (
        "crates/puffer-core/tests/mcp_stub/stub_server.rs",
        "encode_base64",
    ),
    (
        "crates/puffer-core/tests/mcp_stub/stub_server.rs",
        "slow_echo_schema",
    ),
    ("crates/puffer-mcp-oauth/src/service.rs", "new"),
    (
        "crates/puffer-mcp-oauth/tests/oauth_stub_server.rs",
        "metrics",
    ),
    (
        "crates/puffer-mcp-oauth/tests/oauth_stub_server.rs",
        "shutdown",
    ),
    (
        "crates/puffer-mcp-oauth/tests/oauth_stub_server.rs",
        "spawn_oauth_stub",
    ),
    ("crates/puffer-runner-api/src/lib.rs", "deserialize"),
    ("crates/puffer-runner-api/src/lib.rs", "execution"),
    ("crates/puffer-runner-api/src/lib.rs", "mcp"),
    ("crates/puffer-runner-api/src/lib.rs", "message"),
    ("crates/puffer-runner-api/src/lib.rs", "new"),
    ("crates/puffer-runner-api/src/lib.rs", "other"),
    ("crates/puffer-runner-api/src/lib.rs", "serialize"),
    ("crates/puffer-runner-grpc/tests/grpc_e2e.rs", "spawn"),
];

#[test]
fn exported_functions_have_doc_comments() {
    let mut missing = Vec::new();
    for path in rust_files() {
        let display_path = display_path(&path);
        let contents = fs::read_to_string(&path).expect("read Rust file");
        let lines = contents.lines().collect::<Vec<_>>();
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if !is_exported_function(trimmed) {
                continue;
            }
            if !has_doc_comment(&lines, index) {
                let function_name = exported_function_name(trimmed).unwrap_or("<unknown>");
                if missing_doc_is_baselined(&display_path, function_name) {
                    continue;
                }
                missing.push(format!("{}:{} ({function_name})", display_path, index + 1));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "missing doc comments for exported functions:\n{}",
        missing.join("\n")
    );
}

#[test]
fn rust_files_stay_under_line_limit() {
    let mut oversized = Vec::new();
    for path in rust_files() {
        let display_path = display_path(&path);
        let contents = fs::read_to_string(&path).expect("read Rust file");
        let line_count = contents.lines().count();
        if line_count > MAX_RUST_FILE_LINES
            && !OVERSIZED_FILE_BASELINE.contains(&display_path.as_str())
        {
            oversized.push(format!("{display_path} ({line_count})"));
        }
    }

    assert!(
        oversized.is_empty(),
        "Rust files exceed {} lines:\n{}",
        MAX_RUST_FILE_LINES,
        oversized.join("\n")
    );
}

fn rust_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rust_files(&repo_root().join("crates"), &mut files);
    files.sort();
    files
}

fn collect_rust_files(root: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(root).expect("read directory");
    for entry in entries {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some("target") {
                continue;
            }
            collect_rust_files(&path, files);
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn is_exported_function(trimmed: &str) -> bool {
    trimmed.starts_with("pub fn ") || trimmed.starts_with("pub async fn ")
}

fn exported_function_name(trimmed: &str) -> Option<&str> {
    let rest = trimmed
        .strip_prefix("pub async fn ")
        .or_else(|| trimmed.strip_prefix("pub fn "))?;
    rest.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .next()
}

fn missing_doc_is_baselined(path: &str, function_name: &str) -> bool {
    MISSING_DOC_BASELINE
        .iter()
        .any(|(baseline_path, baseline_function)| {
            *baseline_path == path && *baseline_function == function_name
        })
}

fn has_doc_comment(lines: &[&str], function_index: usize) -> bool {
    let mut index = function_index;
    while index > 0 {
        index -= 1;
        let trimmed = lines[index].trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("#[") {
            continue;
        }
        return trimmed.starts_with("///");
    }
    false
}

fn display_path(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .expect("repo-relative path")
        .display()
        .to_string()
}
