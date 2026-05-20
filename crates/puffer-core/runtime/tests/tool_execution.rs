use super::*;
use puffer_config::PufferConfig;
use puffer_session_store::SessionMetadata;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use uuid::Uuid;

#[path = "tool_execution/agent_team_e2e.rs"]
mod agent_team_e2e;
#[path = "tool_execution/legacy_alias_permissions.rs"]
mod legacy_alias_permissions;
#[path = "tool_execution/multi_agent_e2e.rs"]
mod multi_agent_e2e;
#[path = "tool_execution/request_scope_tests.rs"]
mod request_scope_tests;

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

fn outside_workspace_tempdir(cwd: &Path) -> tempfile::TempDir {
    let mut bases = vec![PathBuf::from("/var/tmp")];
    if let Some(home) = std::env::var_os("HOME") {
        bases.push(PathBuf::from(home));
    }
    if let Ok(current) = std::env::current_dir() {
        bases.push(current.join("target"));
    }
    let workspace_roots = crate::workspace_paths::workspace_roots(cwd, &[]);
    for base in bases {
        if fs::create_dir_all(&base).is_err() {
            continue;
        }
        let Ok(tempdir) = tempfile::Builder::new()
            .prefix("puffer-external-path-")
            .tempdir_in(&base)
        else {
            continue;
        };
        if !workspace_roots
            .iter()
            .any(|root| tempdir.path().starts_with(root))
        {
            return tempdir;
        }
    }
    panic!("failed to create tempdir outside default workspace roots");
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
fn execute_openai_tool_calls_prompt_before_external_file_access() {
    let mut state = temp_state();
    let outside_file = std::path::PathBuf::from("/__puffer_test_outside_writable_set__/secret.txt");
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
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_log = prompts.clone();

    let result = with_permission_prompt_handler(
        move |request| {
            prompt_log.lock().unwrap().push(format!(
                "{}\n{}",
                request.summary,
                request.reason.unwrap_or_default()
            ));
            PermissionPromptAction::Deny
        },
        || {
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
        },
    )
    .unwrap();

    assert!(!result.invocations[0].success);
    assert!(result.outputs[0]
        .output
        .contains("permission denied by user"));
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("Allow Read to access"));
    assert!(prompts[0].contains("outside the current working directories"));
}

#[test]
fn allow_once_external_file_access_executes_without_session_grant() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let outside = outside_workspace_tempdir(&cwd);
    let outside_file = outside.path().join("secret.txt");
    fs::write(&outside_file, "from outside workspace").unwrap();
    let resources = LoadedResources {
        tools: vec![loaded_tool("Read", "Read file", "runtime:claude_read")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let request_config = test_openai_request_config();
    let prompts = Arc::new(Mutex::new(0_usize));
    let prompt_count = prompts.clone();

    let result = with_permission_prompt_handler(
        move |_request| {
            *prompt_count.lock().unwrap() += 1;
            PermissionPromptAction::AllowOnce
        },
        || {
            execute_tool_call(
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
                "Read",
                json!({ "file_path": outside_file.display().to_string() }),
            )
        },
    )
    .unwrap();

    assert!(result.success);
    assert!(result.output.stdout.contains("from outside workspace"));
    assert_eq!(*prompts.lock().unwrap(), 1);
    assert!(state
        .session_permission_state()
        .grants()
        .profile_view(false)
        .path_prefix_grants
        .is_empty());
}

#[test]
fn allow_session_external_path_access_is_reused() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let outside = outside_workspace_tempdir(&cwd);
    fs::write(outside.path().join("note.txt"), "hello").unwrap();
    let resources = LoadedResources {
        tools: vec![loaded_tool("Glob", "Find files", "runtime:claude_glob")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_log = prompts.clone();
    let outside_path = outside.path().display().to_string();
    let outside_path_for_prompt = outside_path.clone();

    with_permission_prompt_handler(
        move |request| {
            prompt_log.lock().unwrap().push(request.summary);
            PermissionPromptAction::AllowSession
        },
        || {
            let first = resolve_tool_permission(
                &mut state,
                &resources,
                &registry,
                &cwd,
                "Glob",
                &json!({
                    "pattern": "*.txt",
                    "path": outside_path_for_prompt.clone()
                }),
                None,
            )
            .unwrap();
            assert!(matches!(first, PermissionOutcome::Allowed(_)));

            let second = resolve_tool_permission(
                &mut state,
                &resources,
                &registry,
                &cwd,
                "Glob",
                &json!({
                    "pattern": "*.txt",
                    "path": outside_path_for_prompt.clone()
                }),
                None,
            )
            .unwrap();
            assert!(matches!(second, PermissionOutcome::Allowed(_)));
        },
    );

    let prompts = prompts.lock().unwrap();
    assert_eq!(
        prompts.as_slice(),
        &[format!("Allow Glob to access {outside_path}")]
    );
    assert_eq!(
        state
            .session_permission_state()
            .grants()
            .profile_view(false)
            .path_prefix_grants,
        vec![outside.path().to_path_buf()]
    );
}

#[test]
fn agent_cwd_external_path_uses_manual_approval() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let outside = outside_workspace_tempdir(&cwd);
    let resources = LoadedResources {
        tools: vec![loaded_tool("Agent", "Delegate work", "runtime:agent")],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_log = prompts.clone();

    let outcome = with_permission_prompt_handler(
        move |request| {
            prompt_log.lock().unwrap().push(format!(
                "{}\n{}",
                request.summary,
                request.reason.unwrap_or_default()
            ));
            PermissionPromptAction::Deny
        },
        || {
            resolve_tool_permission(
                &mut state,
                &resources,
                &registry,
                &cwd,
                "Agent",
                &json!({
                    "description": "Inspect external project",
                    "prompt": "List skills",
                    "cwd": outside.path().display().to_string()
                }),
                None,
            )
        },
    )
    .unwrap();

    let PermissionOutcome::Denied(result) = outcome else {
        panic!("external Agent cwd should wait for manual approval");
    };
    assert!(!result.success);
    assert!(result.output.stdout.contains("permission denied by user"));
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("Allow Agent to access"));
    assert!(prompts[0].contains("outside the current working directories"));
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
fn allow_session_persists_runtime_bash_approval_for_the_session() {
    let mut tool = loaded_tool("Bash", "Run shell", "runtime:claude_bash");
    tool.value.approval_policy = Some("ask".to_string());
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
    let cwd = state.cwd.clone();
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_log = prompts.clone();

    with_permission_prompt_handler(
        move |request| {
            prompt_log.lock().unwrap().push(request.tool_id);
            PermissionPromptAction::AllowSession
        },
        || {
            let first = execute_tool_call(
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
                "Bash",
                json!({"command": "printf hi"}),
            )
            .unwrap();
            assert!(first.success);
            let reloaded_permission_context =
                crate::permissions::load_runtime_permission_context_with_inputs(
                    &cwd,
                    &resources,
                    &state,
                    crate::permissions::RuntimePermissionInputs::default(),
                )
                .unwrap();
            let reloaded_decision = reloaded_permission_context.decision_for_tool_call(
                registry.definition("Bash").unwrap(),
                &json!({"command": "printf hi-again"}),
            );
            assert_eq!(
                reloaded_decision.behavior,
                crate::permissions::ToolPermissionBehavior::Allow
            );

            let second = execute_tool_call(
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
                "Bash",
                json!({"command": "printf hi-again"}),
            )
            .unwrap();
            assert!(second.success);
        },
    );

    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.as_slice(), &["Bash".to_string()]);
    assert_eq!(
        state
            .session_tool_permissions
            .get("bash")
            .map(String::as_str),
        Some("allow")
    );
}

#[test]
fn allow_session_scopes_browser_approval_to_evaluate_actions() {
    let mut tool = loaded_tool("Browser", "Managed browser", "runtime:browser");
    tool.value.approval_policy = Some("auto".to_string());
    tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut providers = ProviderRegistry::new();
    providers.register(openai_provider("http://127.0.0.1".to_string()));
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompt_log = prompts.clone();

    with_permission_prompt_handler(
        move |request| {
            prompt_log.lock().unwrap().push(
                request
                    .reason
                    .clone()
                    .unwrap_or_else(|| request.tool_id.clone()),
            );
            PermissionPromptAction::AllowSession
        },
        || {
            let first = resolve_tool_permission(
                &mut state,
                &resources,
                &registry,
                &cwd,
                "Browser",
                &json!({"action": "evaluate", "script": "document.title"}),
                None,
            )
            .unwrap();
            assert!(matches!(first, PermissionOutcome::Allowed(_)));

            let second = resolve_tool_permission(
                &mut state,
                &resources,
                &registry,
                &cwd,
                "Browser",
                &json!({"action": "evaluate", "script": "window.location.href"}),
                None,
            )
            .unwrap();
            assert!(matches!(second, PermissionOutcome::Allowed(_)));

            let third = resolve_tool_permission(
                &mut state,
                &resources,
                &registry,
                &cwd,
                "Browser",
                &json!({"action": "snapshot"}),
                None,
            )
            .unwrap();
            assert!(matches!(third, PermissionOutcome::Allowed(_)));
        },
    );

    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("executes page JavaScript"));
    assert!(!state.session_tool_permissions.contains_key("browser"));
}

#[test]
fn cross_session_browser_evaluate_can_be_denied_at_prompt_time() {
    let mut tool = loaded_tool("Browser", "Managed browser", "runtime:browser");
    tool.value.approval_policy = Some("auto".to_string());
    tool.value.sandbox_policy = Some("workspace-write".to_string());
    let resources = LoadedResources {
        tools: vec![tool],
        ..LoadedResources::default()
    };
    let registry = ToolRegistry::from_resources(&resources);
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let other_session = "b4f239fd-1493-4be7-a3a1-9e58fe612576";

    with_permission_prompt_handler(
        move |_request| PermissionPromptAction::Deny,
        || {
            let first = resolve_tool_permission(
                &mut state,
                &resources,
                &registry,
                &cwd,
                "Browser",
                &json!({
                    "action": "evaluate",
                    "sessionId": other_session,
                    "script": "document.title"
                }),
                None,
            )
            .unwrap();
            let PermissionOutcome::Denied(result) = first else {
                panic!("expected denied browser permission outcome");
            };
            assert!(result.output.stdout.contains("permission denied by user"));
        },
    );
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
