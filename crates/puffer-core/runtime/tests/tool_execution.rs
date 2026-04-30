use super::*;
use puffer_config::PufferConfig;
use puffer_session_store::SessionMetadata;
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

#[path = "tool_execution/agent_team_e2e.rs"]
mod agent_team_e2e;
#[path = "tool_execution/multi_agent_e2e.rs"]
mod multi_agent_e2e;
#[path = "tool_execution/remote_runner.rs"]
mod remote_runner;
#[path = "tool_execution/remote_session_e2e.rs"]
mod remote_session_e2e;
#[path = "tool_execution/request_scope_tests.rs"]
mod request_scope_tests;

fn request_json_body(raw_request: &str) -> Value {
    let body = raw_request.split("\r\n\r\n").nth(1).unwrap_or_default();
    serde_json::from_str(body).expect("parse request body as json")
}

fn temp_state() -> AppState {
    let tempdir = tempfile::tempdir().unwrap();
    let cwd = tempdir.path().to_path_buf();
    std::mem::forget(tempdir);
    let session = SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
        generated_title: None,
        cwd: cwd.clone(),
        created_at_ms: 0,
        updated_at_ms: 0,
        parent_session_id: None,
        slug: None,
        tags: Vec::new(),
        note: None,
    };
    AppState::new(PufferConfig::default(), cwd, session)
}

fn write_sample_notebook(path: &Path) {
    fs::write(
        path,
        serde_json::to_string_pretty(&json!({
            "nbformat": 4,
            "nbformat_minor": 5,
            "metadata": {
                "language_info": { "name": "python" }
            },
            "cells": [
                {
                    "id": "alpha",
                    "cell_type": "code",
                    "source": "print('old')",
                    "metadata": {},
                    "execution_count": 1,
                    "outputs": [{"output_type": "stream", "text": "old"}]
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
}

fn mark_file_fully_read(state: &mut AppState, path: &Path) {
    let timestamp_ms = fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    state.claude_read_state.insert(
        path.to_path_buf(),
        crate::state::ClaudeReadState {
            timestamp_ms,
            is_partial_view: false,
        },
    );
}

fn configure_remote_tool_runner(
    state: &mut AppState,
    runner: &remote_runner::RemoteToolRunnerHandle,
) {
    state.config.remote_tool_runner = Some(puffer_config::RemoteToolRunnerConfig {
        endpoint: Some(runner.endpoint().to_string()),
        auth_token: Some(runner.token().to_string()),
        auth_token_env: None,
        remote_cwd: Some(state.cwd.display().to_string()),
        path_map: None,
    });
}

#[path = "tool_execution/workflow.rs"]
mod workflow;

#[test]
fn execute_openai_tool_calls_serializes_outputs() {
    let resources = LoadedResources {
        tools: vec![loaded_tool("bash", "Run shell", "bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    let provider = openai_provider("http://127.0.0.1".to_string());
    providers.register(provider);
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_1".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_1".to_string(),
        name: "bash".to_string(),
        arguments: json!({ "command": "printf hi" }),
    }];
    let mut state = state();
    let request_config = test_openai_request_config();
    let result = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &tool_calls,
        &registry,
        std::env::current_dir().unwrap().as_path(),
        &request_config,
        "gpt-5",
        None,
        None,
    )
    .unwrap();
    assert_eq!(result.outputs[0].kind, "function_call_output");
    assert_eq!(result.outputs[0].call_id, "call_1");
    assert!(result.outputs[0].output.contains("hi"));
    assert_eq!(result.invocations[0].tool_id, "bash");
}

#[test]
fn execute_openai_tool_calls_return_permission_denials_as_tool_results() {
    let mut state = temp_state();
    let permissions_dir = ConfigPaths::discover(&state.cwd).workspace_config_dir;
    std::fs::create_dir_all(&permissions_dir).unwrap();
    std::fs::write(
        permissions_dir.join("permissions.toml"),
        "[tools]\nbash = \"deny\"\n",
    )
    .unwrap();

    let resources = LoadedResources {
        tools: vec![loaded_tool("bash", "Run shell", "bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    let provider = openai_provider("http://127.0.0.1".to_string());
    providers.register(provider);
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_1".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_1".to_string(),
        name: "bash".to_string(),
        arguments: json!({ "command": "printf hi" }),
    }];
    let request_config = test_openai_request_config();
    let cwd = state.cwd.clone();

    let result = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &tool_calls,
        &registry,
        &cwd,
        &request_config,
        "gpt-5",
        None,
        None,
    )
    .unwrap();

    assert!(!result.invocations[0].success);
    assert!(result.outputs[0].output.contains("Permission denied"));
}

#[test]
fn execute_openai_tool_calls_enforce_working_directory_access_for_claude_file_tools() {
    let mut state = temp_state();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("secret.txt");
    fs::write(&outside_file, "secret\n").unwrap();
    let resources = LoadedResources {
        tools: vec![loaded_tool("Read", "Read file", "runtime:claude_read")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_read".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_read".to_string(),
        name: "Read".to_string(),
        arguments: json!({ "file_path": outside_file.display().to_string() }),
    }];
    let request_config = test_openai_request_config();
    let cwd = state.cwd.clone();

    let result = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &tool_calls,
        &registry,
        &cwd,
        &request_config,
        "gpt-5",
        None,
        None,
    )
    .unwrap();

    assert!(!result.invocations[0].success);
    assert!(result.outputs[0].output.contains("working director"));
}

#[test]
fn execute_openai_tool_calls_block_tools_outside_request_scope() {
    let resources = LoadedResources {
        tools: vec![loaded_tool("Bash", "Run shell", "runtime:claude_bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_1".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_1".to_string(),
        name: "Bash".to_string(),
        arguments: json!({ "command": "printf hi" }),
    }];
    let filter = build_request_tool_filter(&["Read".to_string()])
        .unwrap()
        .unwrap();
    let request_config = test_openai_request_config();
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let result = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &tool_calls,
        &registry,
        &cwd,
        &request_config,
        "gpt-5",
        None,
        Some(&filter),
    )
    .unwrap();

    assert!(!result.invocations[0].success);
    assert!(result.outputs[0]
        .output
        .contains("slash command tool scope denied this tool call"));
}

#[test]
fn execute_openai_tool_calls_support_runtime_sleep() {
    let mut tool = loaded_tool("Sleep", "Wait for a specified duration", "runtime:sleep");
    tool.value.approval_policy = Some("never".to_string());
    tool.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let request_config = test_openai_request_config();
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_sleep".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_sleep".to_string(),
        name: "Sleep".to_string(),
        arguments: json!({
            "duration_ms": 1,
            "reason": "wait briefly"
        }),
    }];
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let result = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &tool_calls,
        &registry,
        &cwd,
        &request_config,
        "gpt-5",
        None,
        None,
    )
    .unwrap();

    assert!(result.invocations[0].success);
    assert_eq!(result.invocations[0].tool_id, "Sleep");
    assert!(result.outputs[0].output.contains("\"completed\": true"));
    assert!(result.outputs[0]
        .output
        .contains("\"reason\": \"wait briefly\""));
}

#[test]
fn execute_anthropic_tool_calls_support_runtime_sleep() {
    let mut tool = loaded_tool("Sleep", "Wait for a specified duration", "runtime:sleep");
    tool.value.approval_policy = Some("never".to_string());
    tool.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(provider());
    let request_config = test_anthropic_request_config();
    let response = json!({
        "content": [
            {
                "type": "tool_use",
                "id": "toolu_sleep",
                "name": "Sleep",
                "input": {
                    "duration_ms": 1,
                    "reason": "wait briefly"
                }
            }
        ]
    });
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let result = execute_anthropic_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &response,
        &registry,
        &cwd,
        &request_config,
        "claude-sonnet-4-5",
        None,
        None,
    )
    .unwrap();

    let result = result.expect("anthropic sleep tool results");

    assert!(result.invocations[0].success);
    assert_eq!(result.invocations[0].tool_id, "Sleep");
    assert!(result.invocations[0].output.contains("\"completed\": true"));
    assert!(result.invocations[0]
        .output
        .contains("\"reason\": \"wait briefly\""));
}

#[test]
fn execute_openai_tool_calls_support_runtime_notebook_edit() {
    let mut tool = loaded_tool(
        "NotebookEdit",
        "Edit notebook cells",
        "runtime:notebook_edit",
    );
    tool.value.approval_policy = Some("auto".to_string());
    tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let request_config = test_openai_request_config();
    let mut state = temp_state();
    let notebook = state.cwd.join("demo.ipynb");
    write_sample_notebook(&notebook);
    mark_file_fully_read(&mut state, &notebook);

    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_nb".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_nb".to_string(),
        name: "NotebookEdit".to_string(),
        arguments: json!({
            "notebook_path": notebook.display().to_string(),
            "cell_id": "alpha",
            "new_source": "print('updated')",
            "edit_mode": "replace"
        }),
    }];
    let cwd = state.cwd.clone();

    let result = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &tool_calls,
        &registry,
        &cwd,
        &request_config,
        "gpt-5",
        None,
        None,
    )
    .unwrap();

    assert!(result.invocations[0].success);
    assert_eq!(result.invocations[0].tool_id, "NotebookEdit");
    assert!(result.outputs[0]
        .output
        .contains("\"edit_mode\": \"replace\""));
    let updated = fs::read_to_string(&notebook).unwrap();
    assert!(updated.contains("print('updated')"));
}

#[test]
fn execute_anthropic_tool_calls_support_runtime_notebook_edit() {
    let mut tool = loaded_tool(
        "NotebookEdit",
        "Edit notebook cells",
        "runtime:notebook_edit",
    );
    tool.value.approval_policy = Some("auto".to_string());
    tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(provider());
    let request_config = test_anthropic_request_config();
    let mut state = temp_state();
    let notebook = state.cwd.join("demo.ipynb");
    write_sample_notebook(&notebook);
    mark_file_fully_read(&mut state, &notebook);

    let response = json!({
        "content": [
            {
                "type": "tool_use",
                "id": "toolu_nb",
                "name": "NotebookEdit",
                "input": {
                    "notebook_path": notebook.display().to_string(),
                    "cell_id": "alpha",
                    "new_source": "print('updated')",
                    "edit_mode": "replace"
                }
            }
        ]
    });
    let cwd = state.cwd.clone();

    let result = execute_anthropic_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &response,
        &registry,
        &cwd,
        &request_config,
        "claude-sonnet-4-5",
        None,
        None,
    )
    .unwrap()
    .expect("anthropic notebook edit tool result");

    assert!(result.invocations[0].success);
    assert_eq!(result.invocations[0].tool_id, "NotebookEdit");
    assert!(result.invocations[0]
        .output
        .contains("\"edit_mode\": \"replace\""));
    let updated = fs::read_to_string(&notebook).unwrap();
    assert!(updated.contains("print('updated')"));
}

#[test]
fn execute_tool_call_requires_prior_read_for_notebook_edit() {
    let mut tool = loaded_tool(
        "NotebookEdit",
        "Edit notebook cells",
        "runtime:notebook_edit",
    );
    tool.value.approval_policy = Some("auto".to_string());
    tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let request_config = test_openai_request_config();
    let mut state = temp_state();
    let notebook = state.cwd.join("demo.ipynb");
    write_sample_notebook(&notebook);
    let cwd = state.cwd.clone();

    let result = execute_tool_call(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &registry,
        "gpt-5",
        &cwd,
        ToolExecutionBackend::OpenAi {
            request_config: &request_config,
            structured_output: None,
        },
        None,
        Some("call_1"),
        "NotebookEdit",
        json!({
            "notebook_path": notebook.display().to_string(),
            "cell_id": "alpha",
            "new_source": "print('updated')",
            "edit_mode": "replace"
        }),
    )
    .unwrap();

    assert!(!result.success);
    assert!(result
        .output
        .stdout
        .contains("File has not been read yet. Read it first before writing to it."));
}

#[test]
fn execute_anthropic_tool_calls_block_tools_outside_request_scope() {
    let resources = LoadedResources {
        tools: vec![loaded_tool("Bash", "Run shell", "runtime:claude_bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(provider());
    let request_config = test_anthropic_request_config();
    let response = json!({
        "content": [
            {
                "type": "tool_use",
                "id": "toolu_1",
                "name": "Bash",
                "input": {
                    "command": "printf hi"
                }
            }
        ]
    });
    let filter = build_request_tool_filter(&["Read".to_string()])
        .unwrap()
        .unwrap();
    let mut state = temp_state();
    let cwd = state.cwd.clone();

    let result = execute_anthropic_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &response,
        &registry,
        &cwd,
        &request_config,
        "claude-sonnet-4-5",
        None,
        Some(&filter),
    )
    .unwrap()
    .expect("anthropic blocked tool result");

    assert!(!result.invocations[0].success);
    assert!(result.invocations[0]
        .output
        .contains("slash command tool scope denied this tool call"));
}

#[test]
fn anthropic_tool_hooks_run_for_completed_tool_calls() {
    let temp = tempfile::tempdir().unwrap();
    let hook_output = temp.path().join("hook.txt");
    let resources = LoadedResources {
        hooks: vec![
            LoadedItem {
                value: puffer_resources::HookSpec {
                    id: "tool-start".to_string(),
                    event: "tool_start".to_string(),
                    command: format!(
                        "printf 'start:%s\\n' \"$PUFFER_TOOL_ID\" >> {}",
                        hook_output.display()
                    ),
                },
                source_info: SourceInfo {
                    path: "hook_start.yaml".into(),
                    kind: SourceKind::Builtin,
                },
            },
            LoadedItem {
                value: puffer_resources::HookSpec {
                    id: "tool-end".to_string(),
                    event: "tool_end".to_string(),
                    command: format!(
                        "printf 'end:%s:%s\\n' \"$PUFFER_TOOL_ID\" \"$PUFFER_TOOL_SUCCESS\" >> {}",
                        hook_output.display()
                    ),
                },
                source_info: SourceInfo {
                    path: "hook_end.yaml".into(),
                    kind: SourceKind::Builtin,
                },
            },
        ],
        tools: vec![loaded_tool("bash", "Run shell", "bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    let provider = provider();
    providers.register(provider.clone());
    let response = json!({
        "content": [
            {
                "type": "tool_use",
                "id": "toolu_1",
                "name": "bash",
                "input": {
                    "command": "printf hi"
                }
            }
        ]
    });
    let mut state = state();
    let request_config = test_anthropic_request_config();
    let _ = execute_anthropic_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &response,
        &registry,
        temp.path(),
        &request_config,
        "claude-sonnet-4-5",
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(hook_output).unwrap(),
        "start:bash\nend:bash:true\n"
    );
}

#[test]
fn openai_tool_hooks_run_for_completed_tool_calls() {
    let temp = tempfile::tempdir().unwrap();
    let hook_output = temp.path().join("hook.txt");
    let resources = LoadedResources {
        hooks: vec![
            LoadedItem {
                value: puffer_resources::HookSpec {
                    id: "tool-start".to_string(),
                    event: "tool_start".to_string(),
                    command: format!(
                        "printf 'start:%s\\n' \"$PUFFER_TOOL_ID\" >> {}",
                        hook_output.display()
                    ),
                },
                source_info: SourceInfo {
                    path: "hook_start.yaml".into(),
                    kind: SourceKind::Builtin,
                },
            },
            LoadedItem {
                value: puffer_resources::HookSpec {
                    id: "tool-end".to_string(),
                    event: "tool_end".to_string(),
                    command: format!(
                        "printf 'end:%s:%s\\n' \"$PUFFER_TOOL_ID\" \"$PUFFER_TOOL_SUCCESS\" >> {}",
                        hook_output.display()
                    ),
                },
                source_info: SourceInfo {
                    path: "hook_end.yaml".into(),
                    kind: SourceKind::Builtin,
                },
            },
        ],
        tools: vec![loaded_tool("bash", "Run shell", "bash")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_1".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_1".to_string(),
        name: "bash".to_string(),
        arguments: json!({ "command": "printf hi" }),
    }];
    let mut state = state();
    let request_config = test_openai_request_config();
    let _ = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &tool_calls,
        &registry,
        temp.path(),
        &request_config,
        "gpt-5",
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(hook_output).unwrap(),
        "start:bash\nend:bash:true\n"
    );
}

#[test]
fn execute_openai_tool_calls_route_bash_through_remote_runner_and_stream_output() {
    let runner = remote_runner::spawn_remote_tool_runner();
    let mut bash = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    bash.value.approval_policy = Some("never".to_string());
    bash.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![bash],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let request_config = test_openai_request_config();
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_remote_bash".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_remote_bash".to_string(),
        name: "Bash".to_string(),
        arguments: json!({
            "command": "printf remote-stdout; printf remote-stderr >&2"
        }),
    }];
    let mut state = temp_state();
    configure_remote_tool_runner(&mut state, &runner);
    let cwd = state.cwd.clone();
    let mut deltas = Vec::new();
    let mut on_delta = |delta| deltas.push(delta);

    let result = super::super::tool_stream::with_tool_stream_handler(&mut on_delta, || {
        execute_openai_tool_calls(
            &mut state,
            &resources,
            &providers,
            &mut AuthStore::default(),
            &tool_calls,
            &registry,
            &cwd,
            &request_config,
            "gpt-5",
            None,
            None,
        )
    })
    .unwrap();

    assert_eq!(result.invocations.len(), 1);
    assert!(result.invocations[0].success);
    assert_eq!(result.invocations[0].tool_id, "Bash");
    assert!(result.outputs[0]
        .output
        .contains("\"stdout\": \"remote-stdout\""));
    assert!(result.outputs[0]
        .output
        .contains("\"stderr\": \"remote-stderr\""));
    assert!(deltas.iter().any(|delta| {
        delta.call_id == "call_remote_bash"
            && delta.tool_id == "Bash"
            && delta.stream == ToolOutputStream::Stdout
            && delta.text.contains("remote-stdout")
    }));
    assert!(deltas.iter().any(|delta| {
        delta.call_id == "call_remote_bash"
            && delta.tool_id == "Bash"
            && delta.stream == ToolOutputStream::Stderr
            && delta.text.contains("remote-stderr")
    }));
}

#[test]
fn execute_openai_tool_calls_route_web_search_through_remote_runner() {
    let runner = remote_runner::spawn_remote_tool_runner();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let request_log = Arc::clone(&requests);
    let server = thread::spawn(move || {
        for _ in 0..100 {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0_u8; 65_536];
                    let bytes = stream.read(&mut buffer).unwrap();
                    let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
                    request_log.lock().unwrap().push(request);
                    let body = json!({
                        "id": "resp_web_search",
                        "output": [
                            {
                                "type": "web_search_call",
                                "action": {
                                    "sources": [
                                        {
                                            "title": "Rust",
                                            "url": "https://www.rust-lang.org"
                                        }
                                    ]
                                }
                            },
                            {
                                "type": "message",
                                "role": "assistant",
                                "content": [
                                    {
                                        "type": "output_text",
                                        "text": "Rust is a systems programming language."
                                    }
                                ]
                            }
                        ]
                    })
                    .to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(error) => panic!("accept mock web search request: {error}"),
            }
        }
    });

    let mut web_search = loaded_tool("WebSearch", "Search the web", "provider:web_search");
    web_search.value.approval_policy = Some("never".to_string());
    web_search.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![web_search],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider(format!("http://{address}")));
    let mut request_config = test_openai_request_config();
    request_config.base_url = format!("http://{address}");
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_remote_web_search".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_remote_web_search".to_string(),
        name: "WebSearch".to_string(),
        arguments: json!({
            "query": "rust language"
        }),
    }];
    let mut state = temp_state();
    state.session_allow_all = true;
    configure_remote_tool_runner(&mut state, &runner);
    let cwd = state.cwd.clone();

    let result = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &tool_calls,
        &registry,
        &cwd,
        &request_config,
        "gpt-5",
        None,
        None,
    )
    .unwrap();
    server.join().unwrap();

    assert_eq!(result.invocations.len(), 1);
    assert!(
        result.invocations[0].success,
        "tool output: {}",
        result.outputs[0].output
    );
    assert_eq!(result.invocations[0].tool_id, "WebSearch");
    assert!(result.outputs[0]
        .output
        .contains("Rust is a systems programming language."));
    assert!(result.outputs[0]
        .output
        .contains("- [Rust](https://www.rust-lang.org)"));

    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "tool output: {}",
        result.outputs[0].output
    );
    assert!(requests[0]
        .to_ascii_lowercase()
        .contains("post /v1/responses"));
    let body = request_json_body(&requests[0]);
    assert_eq!(body["input"], json!("rust language"));
    assert_eq!(body["tools"][0]["type"], json!("web_search"));
}

#[test]
fn execute_openai_tool_calls_do_not_touch_local_read_state_after_remote_write_and_edit() {
    let runner = remote_runner::spawn_remote_tool_runner();
    let mut write = loaded_tool("Write", "Write file", "runtime:claude_write");
    write.value.approval_policy = Some("auto".to_string());
    write.value.sandbox_policy = Some("workspace-write".to_string());
    let mut edit = loaded_tool("Edit", "Edit file", "runtime:claude_edit");
    edit.value.approval_policy = Some("auto".to_string());
    edit.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![write, edit],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let request_config = test_openai_request_config();
    let mut state = temp_state();
    configure_remote_tool_runner(&mut state, &runner);
    let file = state.cwd.join("remote-edit.txt");
    let tool_calls = vec![
        OpenAIResponseToolCall {
            item_id: Some("fc_remote_write".to_string()),
            status: Some("completed".to_string()),
            call_id: "call_remote_write".to_string(),
            name: "Write".to_string(),
            arguments: json!({
                "file_path": file.display().to_string(),
                "content": "hello from remote\n"
            }),
        },
        OpenAIResponseToolCall {
            item_id: Some("fc_remote_edit".to_string()),
            status: Some("completed".to_string()),
            call_id: "call_remote_edit".to_string(),
            name: "Edit".to_string(),
            arguments: json!({
                "file_path": file.display().to_string(),
                "old_string": "hello from remote",
                "new_string": "updated remotely"
            }),
        },
    ];
    let cwd = state.cwd.clone();

    let result = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &tool_calls,
        &registry,
        &cwd,
        &request_config,
        "gpt-5",
        None,
        None,
    )
    .unwrap();

    assert_eq!(result.invocations.len(), 2);
    assert!(result
        .invocations
        .iter()
        .all(|invocation| invocation.success));
    assert_eq!(fs::read_to_string(&file).unwrap(), "updated remotely\n");
    assert!(
        !state.claude_read_state.contains_key(&file),
        "remote write/edit should not refresh local read state"
    );
}

#[test]
fn execute_openai_tool_calls_force_supported_tools_through_remote_runner() {
    let mut state = temp_state();
    let file = state.cwd.join("remote-read.txt");
    fs::write(&file, "hello from local disk\n").unwrap();
    state.config.remote_tool_runner = Some(puffer_config::RemoteToolRunnerConfig {
        endpoint: Some("not a valid endpoint".to_string()),
        auth_token: None,
        auth_token_env: None,
        remote_cwd: Some(state.cwd.display().to_string()),
        path_map: None,
    });

    let resources = LoadedResources {
        tools: vec![loaded_tool("Read", "Read file", "runtime:claude_read")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let request_config = test_openai_request_config();
    let tool_calls = vec![OpenAIResponseToolCall {
        item_id: Some("fc_remote_read".to_string()),
        status: Some("completed".to_string()),
        call_id: "call_remote_read".to_string(),
        name: "Read".to_string(),
        arguments: json!({
            "file_path": file.display().to_string(),
        }),
    }];
    let cwd = state.cwd.clone();

    let result = execute_openai_tool_calls(
        &mut state,
        &resources,
        &providers,
        &mut AuthStore::default(),
        &tool_calls,
        &registry,
        &cwd,
        &request_config,
        "gpt-5",
        None,
        None,
    )
    .unwrap();

    assert!(
        !result.invocations[0].success,
        "remote runner failures should be surfaced as failed tool results"
    );
    assert!(
        result.outputs[0]
            .output
            .contains("invalid tool runner endpoint `not a valid endpoint`"),
        "expected remote execution error, got: {}",
        result.outputs[0].output
    );
}
