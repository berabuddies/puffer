use super::*;
use puffer_config::ConfigPaths;
use std::fs;
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
fn ask_user_question_rejects_duplicate_question_text() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let error =
        crate::runtime::claude_tools::workflow::ask_user_question::execute_ask_user_question(
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
fn ask_user_question_uses_prompt_handler_answers() {
    let mut state = temp_state();
    let cwd = state.cwd.clone();
    let output = crate::runtime::with_user_question_prompt_handler(
        |_request| crate::runtime::UserQuestionPromptResponse {
            answers: serde_json::Map::from_iter([(
                "Where is Lily?".to_string(),
                json!("In the garden"),
            )]),
            annotations: serde_json::Map::new(),
        },
        || {
            crate::runtime::claude_tools::workflow::ask_user_question::execute_ask_user_question(
                &mut state,
                &cwd,
                json!({
                    "questions": [
                        {
                            "question": "Where is Lily?",
                            "header": "Location",
                            "options": [
                                {"label": "In the garden", "description": "Lily is outside"},
                                {"label": "In the kitchen", "description": "Lily is inside"}
                            ]
                        }
                    ]
                }),
            )
        },
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["pending"], false);
    assert_eq!(parsed["answers"]["Where is Lily?"], "In the garden");
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
