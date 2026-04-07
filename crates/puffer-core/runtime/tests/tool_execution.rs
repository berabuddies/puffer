use super::*;
use puffer_config::PufferConfig;
use puffer_session_store::SessionMetadata;
use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};
use uuid::Uuid;

fn temp_state() -> AppState {
    let tempdir = tempfile::tempdir().unwrap();
    let cwd = tempdir.path().to_path_buf();
    std::mem::forget(tempdir);
    let session = SessionMetadata {
        id: Uuid::new_v4(),
        display_name: None,
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

    let error = execute_openai_tool_calls(
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
    .err()
    .unwrap()
    .to_string();

    assert!(error.contains("working director"));
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
fn tool_hooks_run_for_completed_tool_calls() {
    let temp = tempfile::tempdir().unwrap();
    let hook_output = temp.path().join("hook.txt");
    let resources = LoadedResources {
        hooks: vec![LoadedItem {
            value: puffer_resources::HookSpec {
                id: "tool-end".to_string(),
                event: "tool_end".to_string(),
                command: format!("printf \"$PUFFER_TOOL_ID\" > {}", hook_output.display()),
            },
            source_info: SourceInfo {
                path: "hook.yaml".into(),
                kind: SourceKind::Builtin,
            },
        }],
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
    assert_eq!(std::fs::read_to_string(hook_output).unwrap(), "bash");
}

#[test]
fn task_output_reads_runtime_background_agent_output_file() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let agent_output_dir = ConfigPaths::discover(&cwd)
        .workspace_config_dir
        .join("runtime")
        .join("agent_outputs");
    fs::create_dir_all(&agent_output_dir).unwrap();
    let output_file = agent_output_dir.join("agent-demo.json");
    fs::write(
        &output_file,
        serde_json::to_string_pretty(&json!({
            "status": "completed",
            "result": "background ok"
        }))
        .unwrap(),
    )
    .unwrap();

    let output = super::super::claude_tools::workflow::task_output::execute_task_output(
        &mut state,
        &cwd,
        json!({
            "task_id": "agent-demo",
            "block": false,
            "timeout": 50
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["retrieval_status"], "success");
    assert_eq!(parsed["task"]["task_type"], "agent");
    assert_eq!(parsed["task"]["status"], "completed");
    assert_eq!(parsed["task"]["output"], "background ok");
    assert_eq!(parsed["task"]["result"], "background ok");
    assert_eq!(
        parsed["task"]["outputFile"].as_str(),
        Some(output_file.display().to_string().as_str())
    );
}

#[test]
fn send_user_message_stores_resolved_attachment_paths() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let attachment = cwd.join("note.txt");
    fs::write(&attachment, "hello").unwrap();

    let output =
        super::super::claude_tools::workflow::send_user_message::execute_send_user_message(
            &mut state,
            &cwd,
            json!({
                "message": "hello",
                "attachments": ["note.txt"],
                "status": "normal"
            }),
        )
        .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(
        parsed["message"]["attachments"][0].as_str(),
        Some(attachment.display().to_string().as_str())
    );
}

#[test]
fn send_user_message_ignores_workspace_ask_permissions() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let permissions_dir = ConfigPaths::discover(&cwd).workspace_config_dir;
    fs::create_dir_all(&permissions_dir).unwrap();
    fs::write(
        permissions_dir.join("permissions.toml"),
        "[tools]\nsend_user_message = \"ask\"\nbrief = \"ask\"\n",
    )
    .unwrap();

    let mut tool = loaded_tool(
        "SendUserMessage",
        "Send a message to the user",
        "runtime:workflow:send_user_message",
    );
    tool.value.approval_policy = Some("auto".to_string());
    tool.value.sandbox_policy = Some("read-only".to_string());
    let resources = LoadedResources {
        tools: vec![tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let request_config = test_openai_request_config();

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
        "SendUserMessage",
        json!({
            "message": "hi",
            "status": "normal"
        }),
    )
    .unwrap();

    assert!(result.success);
    assert!(result.output.stdout.contains("\"message\": \"hi\""));
}

#[test]
fn execute_openai_tool_calls_respect_request_tool_filter() {
    let resources = LoadedResources {
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
    let request_config = test_openai_request_config();
    let filter = super::super::build_request_tool_filter(&["Read".to_string()])
        .unwrap()
        .unwrap();
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
fn execute_anthropic_tool_calls_respect_request_tool_filter() {
    let resources = LoadedResources {
        tools: vec![loaded_tool("bash", "Run shell", "bash")],
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
                "name": "bash",
                "input": {
                    "command": "printf hi"
                }
            }
        ]
    });
    let filter = super::super::build_request_tool_filter(&["Read".to_string()])
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
    .expect("anthropic tool results");

    assert!(!result.invocations[0].success);
    assert!(result.invocations[0]
        .output
        .contains("slash command tool scope denied this tool call"));
}

#[test]
fn todo_write_rejects_multiple_in_progress_items() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error = super::super::claude_tools::workflow::todo_write::execute_todo_write(
        &mut state,
        &cwd,
        json!({
            "todos": [
                {"content": "one", "status": "in_progress", "activeForm": "Doing one"},
                {"content": "two", "status": "in_progress", "activeForm": "Doing two"}
            ]
        }),
    )
    .unwrap_err();
    assert!(error.to_string().contains("at most one in_progress"));
}

#[test]
fn config_tool_supports_editor_mode() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = super::super::claude_tools::workflow::config::execute_config(
        &mut state,
        &cwd,
        json!({
            "setting": "editorMode",
            "value": "vim"
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["operation"], "set");
    assert_eq!(parsed["value"], "vim");
    assert_eq!(parsed["newValue"], "vim");
    assert!(state.vim_mode);
}

#[test]
fn config_tool_supports_openai_map_settings() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = super::super::claude_tools::workflow::config::execute_config(
        &mut state,
        &cwd,
        json!({
            "setting": "openai_headers",
            "value": {
                "x-test": "one",
                "x-another": "two"
            }
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["operation"], "set");
    assert_eq!(parsed["value"]["x-test"], "one");
    assert_eq!(parsed["newValue"]["x-another"], "two");
    assert_eq!(
        state
            .config
            .openai_headers
            .get("x-test")
            .map(String::as_str),
        Some("one")
    );
}

#[test]
fn config_tool_allows_null_to_clear_openai_map_settings() {
    let mut state = temp_state();
    state
        .config
        .openai_query_params
        .insert("user".to_string(), "alpha".to_string());
    let cwd = state.cwd.clone();
    let output = super::super::claude_tools::workflow::config::execute_config(
        &mut state,
        &cwd,
        json!({
            "setting": "openai_query_params",
            "value": null
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["operation"], "set");
    assert_eq!(parsed["value"], json!({}));
    assert!(state.config.openai_query_params.is_empty());
}

#[test]
fn config_tool_supports_camel_case_aliases_and_status_line_settings() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = super::super::claude_tools::workflow::config::execute_config(
        &mut state,
        &cwd,
        json!({
            "setting": "statusLineCommand",
            "value": "echo status"
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["persisted"], true);
    assert_eq!(parsed["value"], "echo status");
    assert_eq!(
        state
            .config
            .ui
            .status_line
            .as_ref()
            .map(|status_line| status_line.command.as_str()),
        Some("echo status")
    );
}

#[test]
fn config_tool_supports_session_only_settings_without_persisting() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = super::super::claude_tools::workflow::config::execute_config(
        &mut state,
        &cwd,
        json!({
            "setting": "fastMode",
            "value": true
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["persisted"], false);
    assert_eq!(parsed["path"], Value::Null);
    assert!(state.fast_mode);
}

#[test]
fn config_tool_allows_null_to_clear_model_override() {
    let mut state = temp_state();
    state.current_model = Some("openai/gpt-5".to_string());
    state.current_provider = Some("openai".to_string());
    let cwd = state.cwd.clone();
    let output = super::super::claude_tools::workflow::config::execute_config(
        &mut state,
        &cwd,
        json!({
            "setting": "model",
            "value": null
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["value"], Value::Null);
    assert_eq!(state.current_model, None);
}

#[test]
fn ask_user_question_rejects_duplicate_question_text() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error = super::super::claude_tools::workflow::ask_user_question::execute_ask_user_question(
        &mut state,
        &cwd,
        json!({
            "questions": [
                {
                    "question": "Pick one",
                    "header": "choice",
                    "options": [
                        {"label": "A", "description": "A"},
                        {"label": "B", "description": "B"}
                    ]
                },
                {
                    "question": "Pick one",
                    "header": "second",
                    "options": [
                        {"label": "C", "description": "C"},
                        {"label": "D", "description": "D"}
                    ]
                }
            ]
        }),
    )
    .unwrap_err();
    assert!(error.to_string().contains("question texts must be unique"));
}

#[test]
fn team_create_makes_dirs_and_team_delete_removes_them() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let created = super::super::claude_tools::workflow::team_create::execute_team_create(
        &mut state,
        &cwd,
        json!({
            "team_name": "alpha",
            "description": "Coordination team"
        }),
    )
    .unwrap();
    let created: Value = serde_json::from_str(&created).unwrap();
    let team_dir = created["teamDir"].as_str().unwrap();
    let task_dir = created["taskDir"].as_str().unwrap();
    assert!(std::path::Path::new(team_dir).exists());
    assert!(std::path::Path::new(task_dir).exists());

    let deleted = super::super::claude_tools::workflow::team_delete::execute_team_delete(
        &mut state,
        &cwd,
        json!({}),
    )
    .unwrap();
    let deleted: Value = serde_json::from_str(&deleted).unwrap();
    assert_eq!(deleted["deleted"][0], "alpha");
    assert!(!std::path::Path::new(team_dir).exists());
    assert!(!std::path::Path::new(task_dir).exists());
}

#[test]
fn task_update_sets_timestamps_for_progress() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let created = super::super::claude_tools::workflow::task_create::execute_task_create(
        &mut state,
        &cwd,
        json!({
            "subject": "Do thing",
            "description": "Do thing"
        }),
    )
    .unwrap();
    let created: Value = serde_json::from_str(&created).unwrap();
    let task_id = created["task"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("unexpected task create output: {created}"));

    let updated = super::super::claude_tools::workflow::task_update::execute_task_update(
        &mut state,
        &cwd,
        json!({
            "taskId": task_id,
            "status": "in_progress"
        }),
    )
    .unwrap();
    let updated: Value = serde_json::from_str(&updated).unwrap();
    assert_eq!(updated["success"], true);
    assert_eq!(updated["taskId"], task_id);
    assert_eq!(updated["updatedFields"], json!(["status"]));
    assert_eq!(
        updated["statusChange"],
        json!({
            "from": "pending",
            "to": "in_progress"
        })
    );

    let tasks_path = ConfigPaths::discover(&cwd)
        .workspace_config_dir
        .join("runtime/claude_workflow/tasks.json");
    let persisted: Value = serde_json::from_str(&fs::read_to_string(tasks_path).unwrap()).unwrap();
    let task = persisted["tasks"][0].clone();
    assert_eq!(task["task_id"], task_id);
    assert_eq!(task["status"], "in_progress");
    assert!(task["started_at_ms"].is_number());
    assert!(task["updated_at_ms"].is_number());
}

#[test]
fn task_output_waits_for_agent_completion() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let workflow_root = cwd.join(".puffer/runtime/claude_workflow");
    fs::create_dir_all(workflow_root.join("agent_outputs")).unwrap();

    let agent_output = workflow_root.join("agent_outputs/agent-1.md");
    fs::write(&agent_output, "initial").unwrap();
    let agents_path = workflow_root.join("agents.json");
    fs::write(
        &agents_path,
        serde_json::to_string_pretty(&json!({
            "agents": [{
                "agent_id": "agent-1",
                "name": "alpha",
                "description": "demo",
                "prompt": "do work",
                "subagent_type": null,
                "model": null,
                "team_name": null,
                "mode": null,
                "isolation": null,
                "cwd": null,
                "status": "async_launched",
                "output_file": agent_output.display().to_string()
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let agents_path_bg = agents_path.clone();
    let agent_output_bg = agent_output.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        fs::write(&agent_output_bg, "done").unwrap();
        fs::write(
            &agents_path_bg,
            serde_json::to_string_pretty(&json!({
                "agents": [{
                    "agent_id": "agent-1",
                    "name": "alpha",
                    "description": "demo",
                    "prompt": "do work",
                    "subagent_type": null,
                    "model": null,
                    "team_name": null,
                    "mode": null,
                    "isolation": null,
                    "cwd": null,
                    "status": "completed",
                    "output_file": agent_output_bg.display().to_string()
                }]
            }))
            .unwrap(),
        )
        .unwrap();
    });

    let output = super::super::claude_tools::workflow::task_output::execute_task_output(
        &mut state,
        &cwd,
        json!({
            "task_id": "agent-1",
            "block": true,
            "timeout": 1_000
        }),
    )
    .unwrap();
    let output: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(output["retrieval_status"], "success");
    assert_eq!(output["task"]["task_type"], "agent");
    assert_eq!(output["task"]["status"], "completed");
    assert_eq!(output["task"]["output"], "done");
    assert_eq!(output["task"]["result"], "done");
}

#[test]
fn task_stop_rejects_non_background_tasks() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let created = super::super::claude_tools::workflow::task_create::execute_task_create(
        &mut state,
        &cwd,
        json!({
            "subject": "Plan work",
            "description": "Track progress"
        }),
    )
    .unwrap();
    let created: Value = serde_json::from_str(&created).unwrap();
    let task_id = created["task"]["id"].as_str().unwrap();

    let error = super::super::claude_tools::workflow::task_stop::execute_task_stop(
        &mut state,
        &cwd,
        json!({
            "task_id": task_id
        }),
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("is not a running background task"));
}
