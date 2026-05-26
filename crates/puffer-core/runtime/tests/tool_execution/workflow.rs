use super::*;
use puffer_config::ConfigPaths;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

#[test]
fn todo_write_rejects_multiple_in_progress_items() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error = crate::runtime::claude_tools::workflow::todo_write::execute_todo_write(
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
fn process_control_lists_logs_and_stops_interactive_processes() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let process_id = {
        let mut store = state.process_store.lock().unwrap();
        let process_id = store.allocate_id();
        let entry = crate::runtime::process_store::spawn_tracked_process(
            "printf 'ready\\n'; sleep 30",
            &cwd,
            process_id,
            true,
        )
        .unwrap();
        store.insert(entry);
        process_id
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let has_output = state
            .process_store
            .lock()
            .unwrap()
            .peek(process_id)
            .map(|entry| String::from_utf8_lossy(&entry.collect_output()).contains("ready"))
            .unwrap_or(false);
        if has_output || std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let listed = crate::runtime::claude_tools::workflow::process_control::execute_process_control(
        &mut state,
        &cwd,
        json!({ "action": "list" }),
    )
    .unwrap();
    let listed: Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(listed["processes"][0]["sessionId"], process_id.to_string());

    let log = crate::runtime::claude_tools::workflow::process_control::execute_process_control(
        &mut state,
        &cwd,
        json!({ "action": "log", "sessionId": process_id.to_string() }),
    )
    .unwrap();
    let log: Value = serde_json::from_str(&log).unwrap();
    assert!(log["output"].as_str().unwrap_or_default().contains("ready"));

    let stopped = crate::runtime::claude_tools::workflow::process_control::execute_process_control(
        &mut state,
        &cwd,
        json!({ "action": "kill", "sessionId": process_id.to_string() }),
    )
    .unwrap();
    let stopped: Value = serde_json::from_str(&stopped).unwrap();
    assert_eq!(stopped["status"], "killed");
    assert!(state
        .process_store
        .lock()
        .unwrap()
        .peek(process_id)
        .is_none());
}

#[test]
fn task_flow_enforces_revisions_and_persists_state() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let created = crate::runtime::claude_tools::workflow::task_flow::execute_task_flow(
        &mut state,
        &cwd,
        json!({
            "action": "create_managed",
            "controllerId": "controller",
            "goal": "triage inbox",
            "currentStep": "classify",
            "stateJson": "{\"items\":[]}"
        }),
    )
    .unwrap();
    let created: Value = serde_json::from_str(&created).unwrap();
    let flow_id = created["flowId"].as_str().unwrap();
    assert_eq!(created["revision"], 1);
    assert_eq!(created["flow"]["status"], "running");

    let waiting = crate::runtime::claude_tools::workflow::task_flow::execute_task_flow(
        &mut state,
        &cwd,
        json!({
            "action": "set_waiting",
            "flowId": flow_id,
            "expectedRevision": 1,
            "currentStep": "await_reply",
            "stateJson": {"items": ["thread-1"]},
            "waitJson": {"kind": "slack_reply", "thread": "thread-1"}
        }),
    )
    .unwrap();
    let waiting: Value = serde_json::from_str(&waiting).unwrap();
    assert_eq!(waiting["applied"], true);
    assert_eq!(waiting["revision"], 2);

    let stale = crate::runtime::claude_tools::workflow::task_flow::execute_task_flow(
        &mut state,
        &cwd,
        json!({
            "action": "resume",
            "flowId": flow_id,
            "expectedRevision": 1,
            "currentStep": "route",
            "stateJson": "{}"
        }),
    )
    .unwrap();
    let stale: Value = serde_json::from_str(&stale).unwrap();
    assert_eq!(stale["applied"], false);
    assert_eq!(stale["code"], "revision_mismatch");
    assert_eq!(stale["currentRevision"], 2);

    let finished = crate::runtime::claude_tools::workflow::task_flow::execute_task_flow(
        &mut state,
        &cwd,
        json!({
            "action": "finish",
            "flowId": flow_id,
            "expectedRevision": "2",
            "stateJson": {"done": true}
        }),
    )
    .unwrap();
    let finished: Value = serde_json::from_str(&finished).unwrap();
    assert_eq!(finished["status"], "finished");
    assert_eq!(finished["revision"], 3);

    let summary = crate::runtime::claude_tools::workflow::task_flow::execute_task_flow(
        &mut state,
        &cwd,
        json!({
            "action": "get_task_summary",
            "flowId": flow_id
        }),
    )
    .unwrap();
    let summary: Value = serde_json::from_str(&summary).unwrap();
    assert_eq!(summary["summary"]["status"], "finished");
    assert_eq!(summary["summary"]["state"], json!({"done": true}));
}

#[test]
fn task_flow_bindings_and_buffers_do_not_spawn_or_call_network() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let binding = crate::runtime::claude_tools::workflow::task_flow::execute_task_flow(
        &mut state,
        &cwd,
        json!({
            "action": "bind_session",
            "sessionKey": "agent:main",
            "requesterOrigin": "local"
        }),
    )
    .unwrap();
    let binding: Value = serde_json::from_str(&binding).unwrap();
    assert_eq!(binding["flow"]["sessionKey"], "agent:main");

    let appended = crate::runtime::claude_tools::workflow::task_flow::execute_task_flow(
        &mut state,
        &cwd,
        json!({
            "action": "append_buffer",
            "namespace": "eod",
            "item": "send tomorrow"
        }),
    )
    .unwrap();
    let appended: Value = serde_json::from_str(&appended).unwrap();
    assert_eq!(appended["applied"], true);
    assert_eq!(appended["count"], 1);

    let error = crate::runtime::claude_tools::workflow::task_flow::execute_task_flow(
        &mut state,
        &cwd,
        json!({
            "action": "append_buffer",
            "namespace": "../eod",
            "item": "bad"
        }),
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("namespace must be a simple identifier"));
}

#[test]
fn http_request_sends_declared_json_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/search", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if String::from_utf8_lossy(&request).contains("\"query\":\"alpha\"") {
                break;
            }
        }
        let request_text = String::from_utf8_lossy(&request).to_string();
        let response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}";
        stream.write_all(response.as_bytes()).unwrap();
        request_text
    });

    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = crate::runtime::claude_tools::workflow::http_request::execute_http_request(
        &mut state,
        &cwd,
        json!({
            "method": "POST",
            "url": url,
            "headers": {
                "Authorization": "Bearer token",
                "Notion-Version": "2022-06-28"
            },
            "json": {"query": "alpha"},
            "timeoutMs": 5000
        }),
    )
    .unwrap();
    let request_text = server.join().unwrap();
    assert!(request_text.starts_with("POST /v1/search HTTP/1.1"));
    assert!(request_text.contains("authorization: Bearer token"));
    assert!(request_text.contains("notion-version: 2022-06-28"));
    assert!(request_text.contains("\"query\":\"alpha\""));

    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["status"], 200);
    assert_eq!(parsed["body"], "{\"ok\":true}");
}

#[test]
fn http_request_fails_closed_on_http_errors_by_default() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/denied", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 256];
        let _ = stream.read(&mut buffer).unwrap();
        let response = "HTTP/1.1 403 Forbidden\r\nContent-Length: 6\r\n\r\ndenied";
        stream.write_all(response.as_bytes()).unwrap();
    });

    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error = crate::runtime::claude_tools::workflow::http_request::execute_http_request(
        &mut state,
        &cwd,
        json!({
            "method": "GET",
            "url": url
        }),
    )
    .unwrap_err();
    server.join().unwrap();
    assert!(error
        .to_string()
        .contains("HTTP request failed with status 403"));
}

#[test]
fn config_tool_supports_editor_mode() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = crate::runtime::claude_tools::workflow::config::execute_config(
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
    let output = crate::runtime::claude_tools::workflow::config::execute_config(
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
    assert_eq!(parsed["persisted"], true);
    assert_eq!(
        parsed["path"],
        json!(ConfigPaths::discover(&cwd)
            .workspace_config_file()
            .display()
            .to_string())
    );
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
    let output = crate::runtime::claude_tools::workflow::config::execute_config(
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
fn config_tool_rejects_status_line_command_writes() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error = crate::runtime::claude_tools::workflow::config::execute_config(
        &mut state,
        &cwd,
        json!({
            "setting": "statusLineCommand",
            "value": "echo status"
        }),
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("cannot set executable status_line_command"));
    assert!(state.config.ui.status_line.is_none());
}

#[test]
fn config_tool_can_read_status_line_command() {
    let mut state = temp_state();
    state.config.ui.status_line = Some(puffer_config::StatusLineConfig {
        command: "echo status".to_string(),
        padding: 0,
    });
    let cwd = state.cwd.clone();
    let output = crate::runtime::claude_tools::workflow::config::execute_config(
        &mut state,
        &cwd,
        json!({
            "setting": "statusLineCommand"
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["operation"], "get");
    assert_eq!(parsed["value"], "echo status");
}

#[test]
fn config_tool_supports_copy_full_response_alias() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = crate::runtime::claude_tools::workflow::config::execute_config(
        &mut state,
        &cwd,
        json!({
            "setting": "copyFullResponse",
            "value": true
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["scope"], "user");
    assert_eq!(parsed["persisted"], true);
    assert_eq!(parsed["value"], true);
    assert!(state.config.copy_full_response);
}

#[test]
fn config_tool_persists_user_settings_to_user_config() {
    let tempdir = tempfile::tempdir().unwrap();
    let home = tempdir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = puffer_config::set_puffer_home_override(&home);
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = crate::runtime::claude_tools::workflow::config::execute_config(
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
    assert_eq!(parsed["scope"], "user");
    assert_eq!(parsed["persisted"], true);
    assert_eq!(
        parsed["path"],
        json!(ConfigPaths::discover(&cwd)
            .user_config_file()
            .display()
            .to_string())
    );
    assert!(state.fast_mode);
    assert!(
        fs::read_to_string(ConfigPaths::discover(&cwd).user_config_file())
            .unwrap()
            .contains("fast_mode = true")
    );
}

#[test]
fn config_tool_supports_session_only_settings_without_persisting() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = crate::runtime::claude_tools::workflow::config::execute_config(
        &mut state,
        &cwd,
        json!({
            "setting": "promptColor",
            "value": "amber"
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["scope"], "session");
    assert_eq!(parsed["persisted"], false);
    assert_eq!(parsed["path"], Value::Null);
    assert_eq!(state.prompt_color, "amber");
}

#[test]
fn config_tool_supports_integer_status_line_padding() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = crate::runtime::claude_tools::workflow::config::execute_config(
        &mut state,
        &cwd,
        json!({
            "setting": "statusLinePadding",
            "value": 2
        }),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["scope"], "workspace");
    assert_eq!(parsed["persisted"], true);
    assert_eq!(parsed["value"], 2);
    assert_eq!(
        parsed["path"],
        json!(ConfigPaths::discover(&cwd)
            .workspace_config_file()
            .display()
            .to_string())
    );
    assert_eq!(
        state
            .config
            .ui
            .status_line
            .as_ref()
            .map(|status_line| status_line.padding),
        Some(2)
    );
}

#[test]
fn config_tool_allows_null_to_clear_model_override() {
    let tempdir = tempfile::tempdir().unwrap();
    let home = tempdir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = puffer_config::set_puffer_home_override(&home);
    let mut state = temp_state();
    state.current_model = Some("openai/gpt-5".to_string());
    state.current_provider = Some("openai".to_string());
    let cwd = state.cwd.clone();
    let output = crate::runtime::claude_tools::workflow::config::execute_config(
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
fn team_create_makes_dirs_and_team_delete_removes_them() {
    let tempdir = tempfile::tempdir().unwrap();
    let home = tempdir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = puffer_config::set_puffer_home_override(&home);
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let created = crate::runtime::claude_tools::workflow::team_create::execute_team_create(
        &mut state,
        &cwd,
        json!({
            "team_name": "alpha",
            "description": "Coordination team"
        }),
    )
    .unwrap();
    let created: Value = serde_json::from_str(&created).unwrap();
    let team_file_path = created["team_file_path"].as_str().unwrap();
    let task_dir = home.join(".claude/tasks/alpha");
    assert_eq!(created["lead_agent_id"], "team-lead@alpha");
    assert!(std::path::Path::new(team_file_path).exists());
    assert!(task_dir.exists());
    let team_file: Value =
        serde_json::from_str(&fs::read_to_string(team_file_path).unwrap()).unwrap();
    assert_eq!(team_file["name"], "alpha");
    assert_eq!(team_file["leadAgentId"], "team-lead@alpha");
    assert_eq!(team_file["members"][0]["name"], "team-lead");
    assert_eq!(state.active_team_name.as_deref(), Some("alpha"));

    let deleted = crate::runtime::claude_tools::workflow::team_delete::execute_team_delete(
        &mut state,
        &cwd,
        json!({}),
    )
    .unwrap();
    let deleted: Value = serde_json::from_str(&deleted).unwrap();
    assert_eq!(deleted["success"], true);
    assert_eq!(deleted["team_name"], "alpha");
    assert!(!std::path::Path::new(team_file_path).exists());
    assert!(!task_dir.exists());
    assert!(state.active_team_name.is_none());
}

#[test]
fn team_create_rejects_path_components_in_team_name() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error = crate::runtime::claude_tools::workflow::team_create::execute_team_create(
        &mut state,
        &cwd,
        json!({ "team_name": "../outside" }),
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("simple identifier without path components"));
}

#[test]
fn enter_worktree_rejects_path_components_in_name() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error = crate::runtime::claude_tools::workflow::enter_worktree::execute_enter_worktree(
        &mut state,
        &cwd,
        json!({ "name": "../outside" }),
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("simple identifier without path components"));
}

#[test]
fn team_delete_only_removes_the_current_session_team() {
    let tempdir = tempfile::tempdir().unwrap();
    let home = tempdir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = puffer_config::set_puffer_home_override(&home);
    let mut first = temp_state();
    let mut second = temp_state();
    second.cwd = first.cwd.clone();
    second.session.cwd = first.session.cwd.clone();
    let cwd = first.cwd.clone();

    crate::runtime::claude_tools::workflow::team_create::execute_team_create(
        &mut first,
        &cwd,
        json!({ "team_name": "alpha" }),
    )
    .unwrap();
    crate::runtime::claude_tools::workflow::team_create::execute_team_create(
        &mut second,
        &cwd,
        json!({ "team_name": "beta" }),
    )
    .unwrap();

    let deleted = crate::runtime::claude_tools::workflow::team_delete::execute_team_delete(
        &mut first,
        &cwd,
        json!({}),
    )
    .unwrap();
    let deleted: Value = serde_json::from_str(&deleted).unwrap();
    assert_eq!(deleted["success"], true);
    assert_eq!(deleted["team_name"], "alpha");
    assert!(!home.join(".claude/teams/alpha").exists());
    assert!(home.join(".claude/teams/beta").exists());
}

#[test]
fn task_update_sets_timestamps_for_progress() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let created = crate::runtime::claude_tools::workflow::task_create::execute_task_create(
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

    let updated = crate::runtime::claude_tools::workflow::task_update::execute_task_update(
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
        .join(format!(
            "runtime/claude_workflow/sessions/{}/tasks.json",
            state.session.id
        ));
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

    let output = crate::runtime::claude_tools::workflow::task_output::execute_task_output(
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
fn task_output_rejects_path_components_in_task_id() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error = crate::runtime::claude_tools::workflow::task_output::execute_task_output(
        &mut state,
        &cwd,
        json!({
            "task_id": "../agent-output",
            "block": false
        }),
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("simple identifier without path components"));
}

#[test]
fn task_stop_rejects_non_background_tasks() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let created = crate::runtime::claude_tools::workflow::task_create::execute_task_create(
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

    let error = crate::runtime::claude_tools::workflow::task_stop::execute_task_stop(
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

#[test]
fn task_stop_rejects_unrecorded_shell_pid() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error = crate::runtime::claude_tools::workflow::task_stop::execute_task_stop(
        &mut state,
        &cwd,
        json!({
            "task_id": "shell-1"
        }),
    )
    .unwrap_err();

    assert!(error.to_string().contains("unknown task `shell-1`"));
}
