use super::emit_system;
use super::CommandActionEntry;
use crate::runtime::claude_tools::workflow::{task_get, task_list, task_output, task_stop};
use crate::AppState;
use anyhow::{Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

mod monitor;
mod render;
use monitor::*;
use render::*;

/// Backward-compatible alias for task action picker rows.
pub type TaskActionEntry = CommandActionEntry;

/// Handles `/tasks` by inspecting and managing persisted workflow task state.
pub(crate) fn handle_tasks_command(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let trimmed = args.trim();
    match trimmed {
        "" | "show" | "list" => {
            let text = render_tasks_dashboard(state)?;
            emit_system(state, session_store, text)
        }
        "path" => emit_system(state, session_store, render_task_paths(state)?),
        "agents" => emit_system(state, session_store, render_agent_list(state)?),
        "teams" => emit_system(state, session_store, render_team_list(state)?),
        "worktrees" => emit_system(state, session_store, render_worktree_list(state)?),
        "todos" => emit_system(state, session_store, render_todo_list(state)?),
        _ if trimmed.starts_with("show ") || trimmed.starts_with("get ") => {
            let task_id = task_argument(trimmed)?;
            let text = render_task_details(state, &task_id)?;
            emit_system(state, session_store, text)
        }
        _ if trimmed.starts_with("output ") => {
            let task_id = trimmed.trim_start_matches("output ").trim();
            if task_id.is_empty() {
                return emit_system(
                    state,
                    session_store,
                    "Usage: /tasks output <task-id>".to_string(),
                );
            }
            let text = render_task_output(state, task_id)?;
            emit_system(state, session_store, text)
        }
        _ if trimmed.starts_with("stop ") => {
            let task_id = trimmed.trim_start_matches("stop ").trim();
            if task_id.is_empty() {
                return emit_system(
                    state,
                    session_store,
                    "Usage: /tasks stop <task-id>".to_string(),
                );
            }
            let text = stop_task(state, task_id)?;
            emit_system(state, session_store, text)
        }
        _ if trimmed.starts_with("ignore ") => {
            let text = ignore_task(state, resources, providers, auth_store, trimmed)?;
            emit_system(state, session_store, text)
        }
        _ => emit_system(
            state,
            session_store,
            "Usage: /tasks [show|list|path|agents|teams|worktrees|todos|get <task-id>|show <task-id>|output <task-id>|stop <task-id>|ignore <task-id> [reason]]".to_string(),
        ),
    }
}

/// Renders read-only `/tasks` subcommands for inline TUI overlays.
pub(crate) fn render_tasks_panel_text(state: &mut AppState, args: &str) -> Result<Option<String>> {
    let trimmed = args.trim();
    let text = match trimmed {
        "" => return Ok(None),
        "show" | "list" => render_tasks_dashboard(state)?,
        "path" => render_task_paths(state)?,
        "agents" => render_agent_list(state)?,
        "teams" => render_team_list(state)?,
        "worktrees" => render_worktree_list(state)?,
        "todos" => render_todo_list(state)?,
        _ if trimmed.starts_with("show ") || trimmed.starts_with("get ") => {
            let task_id = task_argument(trimmed)?;
            render_task_details(state, &task_id)?
        }
        _ if trimmed.starts_with("output ") => {
            let task_id = trimmed.trim_start_matches("output ").trim();
            if task_id.is_empty() {
                return Ok(None);
            }
            render_task_output(state, task_id)?
        }
        _ => return Ok(None),
    };
    Ok(Some(text))
}

/// Builds the interactive `/tasks` action list used by the TUI picker.
pub(crate) fn render_task_actions(state: &mut AppState) -> Result<Vec<TaskActionEntry>> {
    let tasks = load_workflow_tasks(state)?;
    let agents = load_workflow_agents(state)?;
    let mut actions = vec![
        TaskActionEntry {
            command: "/tasks show".to_string(),
            description: "Show task dashboard".to_string(),
        },
        TaskActionEntry {
            command: "/tasks todos".to_string(),
            description: "Show current todo list".to_string(),
        },
        TaskActionEntry {
            command: "/tasks agents".to_string(),
            description: "Show background agents".to_string(),
        },
        TaskActionEntry {
            command: "/tasks teams".to_string(),
            description: "Show workflow teams".to_string(),
        },
        TaskActionEntry {
            command: "/tasks worktrees".to_string(),
            description: "Show active worktrees".to_string(),
        },
        TaskActionEntry {
            command: "/tasks path".to_string(),
            description: "Show workflow storage paths".to_string(),
        },
    ];

    for task in tasks.iter().filter(|task| !task_is_ignored(&task.metadata)) {
        actions.push(TaskActionEntry {
            command: format!("/tasks show {}", task.task_id),
            description: format!(
                "{} [{}:{}] {}",
                task.task_id,
                task_kind(task),
                task.status,
                shorten(&task.subject, 80)
            ),
        });
        if task_is_monitor(&task.metadata) {
            actions.push(TaskActionEntry {
                command: ignore_command(&task.task_id, None),
                description: format!("Ignore {} ({})", task.task_id, shorten(&task.subject, 72)),
            });
            for reason in task_ignore_reasons(&task.metadata) {
                actions.push(TaskActionEntry {
                    command: ignore_command(&task.task_id, Some(&reason)),
                    description: format!("Ignore {}: {}", task.task_id, shorten(&reason, 72)),
                });
            }
            for action in task_actions(&task.metadata) {
                actions.push(TaskActionEntry {
                    command: action_prompt(
                        &task.task_id,
                        &task.subject,
                        &task.description,
                        &action,
                        &task.metadata,
                    ),
                    description: format!(
                        "{} [{}] {}",
                        task.task_id,
                        action.name,
                        shorten(&task.subject, 72)
                    ),
                });
            }
        }
        if supports_task_stop(task) && matches!(task.status.as_str(), "running" | "in_progress") {
            actions.push(TaskActionEntry {
                command: format!("/tasks stop {}", task.task_id),
                description: format!("Stop {} ({})", task.task_id, shorten(&task.subject, 72)),
            });
        }
        if task.output_file.is_some()
            || task
                .output
                .as_ref()
                .is_some_and(|output| !output.trim().is_empty())
        {
            actions.push(TaskActionEntry {
                command: format!("/tasks output {}", task.task_id),
                description: format!("Read output for {}", task.task_id),
            });
        }
    }

    for agent in &agents {
        actions.push(TaskActionEntry {
            command: format!("/tasks show {}", agent.agent_id),
            description: format!(
                "{} [agent:{}] {}",
                agent.agent_id,
                agent.status,
                shorten(
                    agent.name.as_deref().unwrap_or(agent.description.as_str()),
                    72
                )
            ),
        });
        if agent.can_stop && !agent_status_is_terminal(&agent.status) {
            actions.push(TaskActionEntry {
                command: format!("/tasks stop {}", agent.agent_id),
                description: format!(
                    "Stop agent {}",
                    agent.name.as_deref().unwrap_or(agent.agent_id.as_str())
                ),
            });
        }
        actions.push(TaskActionEntry {
            command: format!("/tasks output {}", agent.agent_id),
            description: format!("Read output for agent {}", agent.agent_id),
        });
    }

    Ok(actions)
}

#[derive(Debug, Deserialize)]
struct WorkflowTaskView {
    #[serde(alias = "id", alias = "taskId")]
    task_id: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    description: String,
    #[serde(default, alias = "activeForm")]
    active_form: String,
    status: String,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    blocks: Vec<String>,
    #[serde(default, alias = "blockedBy")]
    blocked_by: Vec<String>,
    #[serde(default)]
    metadata: serde_json::Map<String, Value>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default, alias = "taskType")]
    task_type: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default, alias = "processId")]
    process_id: Option<u32>,
    #[serde(default, alias = "outputFile")]
    output_file: Option<String>,
    #[serde(default, rename = "receivedAt")]
    received_at: Option<String>,
    #[serde(default, rename = "expiresAt")]
    expires_at: Option<String>,
    #[serde(default, alias = "startedAtMs")]
    started_at_ms: Option<u64>,
    #[serde(default, alias = "updatedAtMs")]
    updated_at_ms: Option<u64>,
    #[serde(default, alias = "exitCode")]
    exit_code: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct WorkflowTaskOutputView {
    #[serde(alias = "id", alias = "taskId")]
    task_id: String,
    #[serde(default, alias = "taskType")]
    task_type: String,
    status: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    output: String,
    #[serde(default, alias = "exitCode")]
    exit_code: Option<i32>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default, alias = "outputFile", alias = "output_file")]
    output_file: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WorkflowTaskGetPayload {
    Wrapped { task: Option<WorkflowTaskView> },
    Bare(WorkflowTaskView),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WorkflowTaskListPayload {
    Wrapped { tasks: Vec<WorkflowTaskView> },
    Bare(Vec<WorkflowTaskView>),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WorkflowTaskOutputPayload {
    Wrapped {
        retrieval_status: String,
        task: WorkflowTaskOutputView,
    },
    Flat {
        retrieval_status: String,
        #[serde(flatten)]
        task: WorkflowTaskOutputView,
    },
}

#[derive(Debug, Deserialize, Default)]
struct WorkflowTaskStoreView {
    #[serde(default)]
    tasks: Vec<WorkflowTaskView>,
}

#[derive(Debug, Deserialize, Default)]
struct WorkflowTodoStoreView {
    #[serde(default)]
    todos: Vec<WorkflowTodoView>,
}

#[derive(Debug, Deserialize)]
struct WorkflowTodoView {
    content: String,
    status: String,
    active_form: String,
}

#[derive(Debug, Deserialize, Default)]
struct WorkflowAgentStoreView {
    #[serde(default)]
    agents: Vec<WorkflowAgentView>,
}

#[derive(Debug, Deserialize, Default)]
struct WorkflowTeamStoreView {
    #[serde(default)]
    teams: Vec<WorkflowTeamView>,
}

#[derive(Debug, Deserialize, Default)]
struct WorkflowWorktreeStoreView {
    #[serde(default)]
    worktrees: Vec<WorkflowWorktreeView>,
}

#[derive(Debug, Deserialize)]
struct WorkflowAgentView {
    agent_id: String,
    #[serde(default)]
    name: Option<String>,
    description: String,
    prompt: String,
    #[serde(default)]
    subagent_type: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    team_name: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    isolation: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    status: String,
    output_file: String,
    #[serde(default = "default_can_stop")]
    can_stop: bool,
}

#[derive(Debug, Deserialize)]
struct WorkflowTeamView {
    team_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct WorkflowWorktreeView {
    name: String,
    path: String,
    base_cwd: String,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    original_head_commit: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RuntimeAgentOutputView {
    status: String,
    #[serde(alias = "agentId")]
    agent_id: String,
    #[serde(default, alias = "agentType")]
    agent_type: Option<String>,
    description: String,
    prompt: String,
    #[serde(default)]
    name: Option<String>,
    cwd: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, alias = "teamName")]
    team_name: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    isolation: Option<String>,
}

fn default_can_stop() -> bool {
    true
}

#[derive(Debug)]
struct WorkflowPaths {
    root: PathBuf,
    tasks: PathBuf,
    monitor_tasks: PathBuf,
    todos: PathBuf,
    agents: PathBuf,
    teams: PathBuf,
    worktrees: PathBuf,
    shell_outputs: PathBuf,
    agent_outputs: PathBuf,
}

fn render_tasks_dashboard(state: &mut AppState) -> Result<String> {
    let paths = workflow_paths(state)?;
    let tasks = load_workflow_tasks(state)?;
    let agents = load_workflow_agents(state)?;
    let teams = load_workflow_teams(state, &agents)?;
    let worktrees = load_json_store::<WorkflowWorktreeStoreView>(&paths.worktrees)?;
    let todos = load_json_store::<WorkflowTodoStoreView>(&paths.todos)?;
    let mut text = String::new();
    let structured_tasks = tasks
        .iter()
        .filter(|task| task_kind(task) == "task" && !task_is_ignored(&task.metadata))
        .collect::<Vec<_>>();
    let background_tasks = tasks
        .iter()
        .filter(|task| task_kind(task) != "task" && !task_is_ignored(&task.metadata))
        .collect::<Vec<_>>();

    let _ = writeln!(
        &mut text,
        "Task dashboard\nworkflow_root={}\nstructured_tasks={}\nbackground_tasks={}\nbackground_agents={}\ntodos={}\nrecent_runtime={}",
        paths.root.display(),
        structured_tasks.len(),
        background_tasks.len(),
        agents.len(),
        todos.todos.len(),
        state.tasks().len()
    );
    append_task_section(&mut text, "Task list", &structured_tasks);
    append_task_section(&mut text, "Background tasks", &background_tasks);
    append_agent_section(&mut text, &agents);
    append_team_section(&mut text, &teams);
    append_worktree_section(&mut text, &worktrees.worktrees);
    append_todo_section(&mut text, &todos.todos);
    append_runtime_section(&mut text, state);
    Ok(text.trim_end().to_string())
}

fn render_task_paths(state: &AppState) -> Result<String> {
    let paths = workflow_paths(state)?;
    Ok(format!(
        "Task paths\nworkflow_root={}\ntasks_json={}\nmonitor_tasks_json={}\ntodos_json={}\nagents_json={}\nteams_json={}\nworktrees_json={}\nshell_outputs={}\nagent_outputs={}",
        paths.root.display(),
        paths.tasks.display(),
        paths.monitor_tasks.display(),
        paths.todos.display(),
        paths.agents.display(),
        paths.teams.display(),
        paths.worktrees.display(),
        paths.shell_outputs.display(),
        paths.agent_outputs.display()
    ))
}

fn render_agent_list(state: &AppState) -> Result<String> {
    let agents = load_workflow_agents(state)?;
    let mut text = String::new();
    append_agent_section(&mut text, &agents);
    Ok(text.trim_end().to_string())
}

fn render_team_list(state: &AppState) -> Result<String> {
    let agents = load_workflow_agents(state)?;
    let teams = load_workflow_teams(state, &agents)?;
    let mut text = String::new();
    append_team_section(&mut text, &teams);
    Ok(text.trim_end().to_string())
}

fn render_worktree_list(state: &AppState) -> Result<String> {
    let worktrees =
        load_json_store::<WorkflowWorktreeStoreView>(&workflow_paths(state)?.worktrees)?;
    let mut text = String::new();
    append_worktree_section(&mut text, &worktrees.worktrees);
    Ok(text.trim_end().to_string())
}

fn render_todo_list(state: &AppState) -> Result<String> {
    let todos = load_json_store::<WorkflowTodoStoreView>(&workflow_paths(state)?.todos)?;
    let mut text = String::new();
    append_todo_section(&mut text, &todos.todos);
    Ok(text.trim_end().to_string())
}

fn render_task_details(state: &mut AppState, task_id: &str) -> Result<String> {
    if let Ok(task) = load_task(state, task_id) {
        return Ok(render_task_detail(&task));
    }

    let agents = load_workflow_agents(state)?;
    if let Some(agent) = agents.iter().find(|agent| agent.agent_id == task_id) {
        return Ok(render_agent_detail(agent));
    }

    match render_task_output(state, task_id) {
        Ok(text) => Ok(text),
        Err(_) => Ok(format!("Unknown task `{task_id}`.")),
    }
}

fn render_task_output(state: &mut AppState, task_id: &str) -> Result<String> {
    let cwd = state.cwd.clone();
    let raw = match task_output::execute_task_output(
        state,
        &cwd,
        json!({
            "task_id": task_id,
            "block": false
        }),
    ) {
        Ok(raw) => raw,
        Err(_) => return Ok(format!("Unknown task `{task_id}`.")),
    };
    let payload: WorkflowTaskOutputPayload =
        serde_json::from_str(&raw).context("invalid TaskOutput payload")?;
    let (retrieval_status, task_payload) = match payload {
        WorkflowTaskOutputPayload::Wrapped {
            retrieval_status,
            task,
        }
        | WorkflowTaskOutputPayload::Flat {
            retrieval_status,
            task,
        } => (retrieval_status, task),
    };
    let mut text = String::from("Task output\n");
    let _ = writeln!(
        &mut text,
        "task_id={}\ntask_type={}\nstatus={}\nretrieval_status={}\noutput_file={}",
        task_payload.task_id,
        task_payload.task_type,
        task_payload.status,
        retrieval_status,
        task_payload
            .output_file
            .unwrap_or_else(|| "<none>".to_string())
    );
    let output = if task_payload.output.trim().is_empty() {
        "<empty>"
    } else {
        task_payload.output.as_str()
    };
    let _ = writeln!(&mut text, "\n{output}");
    Ok(text.trim_end().to_string())
}

fn stop_task(state: &mut AppState, task_id: &str) -> Result<String> {
    let cwd = state.cwd.clone();
    let raw = match task_stop::execute_task_stop(
        state,
        &cwd,
        json!({
            "task_id": task_id
        }),
    ) {
        Ok(raw) => raw,
        Err(error) => return Ok(error.to_string()),
    };
    let payload: Value = serde_json::from_str(&raw).context("invalid TaskStop payload")?;
    Ok(format!(
        "Stopped task\nmessage={}\ntask_id={}\ntask_type={}\ncommand={}\noutput_file={}",
        payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Task stopped."),
        payload
            .get("task_id")
            .and_then(Value::as_str)
            .unwrap_or(task_id),
        payload
            .get("task_type")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>"),
        value_as_display(payload.get("command")),
        payload
            .get("outputFile")
            .or_else(|| payload.get("output_file"))
            .and_then(Value::as_str)
            .unwrap_or("<none>")
    ))
}

fn load_task(state: &mut AppState, task_id: &str) -> Result<WorkflowTaskView> {
    let cwd = state.cwd.clone();
    let raw = task_get::execute_task_get(
        state,
        &cwd,
        json!({
            "taskId": task_id
        }),
    )?;
    let task = match serde_json::from_str::<WorkflowTaskGetPayload>(&raw)
        .context("invalid TaskGet payload")?
    {
        WorkflowTaskGetPayload::Wrapped { task: Some(task) }
        | WorkflowTaskGetPayload::Bare(task) => task,
        WorkflowTaskGetPayload::Wrapped { task: None } => {
            anyhow::bail!("unknown task `{task_id}`")
        }
    };
    let paths = workflow_paths(state)?;
    let stored = load_json_store::<WorkflowTaskStoreView>(&paths.tasks)?
        .tasks
        .into_iter()
        .chain(
            load_json_store::<WorkflowTaskStoreView>(&paths.monitor_tasks)?
                .tasks
                .into_iter(),
        )
        .find(|entry| entry.task_id == task.task_id);
    Ok(merge_task_get(stored, task))
}

fn load_workflow_tasks(state: &mut AppState) -> Result<Vec<WorkflowTaskView>> {
    let mut stored = load_json_store::<WorkflowTaskStoreView>(&workflow_paths(state)?.tasks)?
        .tasks
        .into_iter()
        .map(|task| (task.task_id.clone(), task))
        .collect::<BTreeMap<_, _>>();
    let paths = workflow_paths(state)?;
    stored.extend(
        load_json_store::<WorkflowTaskStoreView>(&paths.monitor_tasks)?
            .tasks
            .into_iter()
            .map(|task| (task.task_id.clone(), task)),
    );
    let cwd = state.cwd.clone();
    let tasks = match task_list::execute_task_list(state, &cwd, json!({})) {
        Ok(raw) => match serde_json::from_str::<WorkflowTaskListPayload>(&raw)
            .context("invalid TaskList payload")?
        {
            WorkflowTaskListPayload::Wrapped { tasks } | WorkflowTaskListPayload::Bare(tasks) => {
                tasks
            }
        },
        Err(_) => return Ok(stored.into_values().collect()),
    };
    Ok(tasks
        .into_iter()
        .map(|task| merge_task_list(stored.remove(&task.task_id), task))
        .collect())
}

fn load_workflow_agents(state: &AppState) -> Result<Vec<WorkflowAgentView>> {
    let paths = workflow_paths(state)?;
    let mut agents = load_json_store::<WorkflowAgentStoreView>(&paths.agents)?
        .agents
        .into_iter()
        .map(|mut agent| {
            agent.can_stop = true;
            (agent.agent_id.clone(), agent)
        })
        .collect::<BTreeMap<_, _>>();

    for agent in load_runtime_agents(&paths.agent_outputs)? {
        agents.insert(agent.agent_id.clone(), agent);
    }

    Ok(agents.into_values().collect())
}

fn load_runtime_agents(agent_outputs_dir: &Path) -> Result<Vec<WorkflowAgentView>> {
    let entries = match fs::read_dir(agent_outputs_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read {}", agent_outputs_dir.display()));
        }
    };
    let mut agents = Vec::new();
    for entry in entries {
        let entry =
            entry.with_context(|| format!("failed to read {}", agent_outputs_dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()))
            }
        };
        let Ok(payload) = serde_json::from_str::<RuntimeAgentOutputView>(&raw) else {
            continue;
        };
        agents.push(WorkflowAgentView {
            agent_id: payload.agent_id,
            name: payload.name,
            description: payload.description,
            prompt: payload.prompt,
            subagent_type: payload.agent_type,
            model: payload.model,
            team_name: payload.team_name,
            mode: payload.mode,
            isolation: payload.isolation,
            cwd: Some(payload.cwd),
            status: payload.status,
            output_file: path.display().to_string(),
            can_stop: false,
        });
    }
    Ok(agents)
}

fn load_workflow_teams(
    state: &AppState,
    agents: &[WorkflowAgentView],
) -> Result<Vec<WorkflowTeamView>> {
    let mut teams = load_json_store::<WorkflowTeamStoreView>(&workflow_paths(state)?.teams)?
        .teams
        .into_iter()
        .map(|team| (team.team_name.clone(), team))
        .collect::<BTreeMap<_, _>>();

    for agent in agents {
        let Some(team_name) = agent.team_name.as_deref() else {
            continue;
        };
        let team = teams
            .entry(team_name.to_string())
            .or_insert_with(|| WorkflowTeamView {
                team_name: team_name.to_string(),
                description: None,
                agent_type: None,
                members: Vec::new(),
            });
        if !team.members.iter().any(|member| member == &agent.agent_id) {
            team.members.push(agent.agent_id.clone());
        }
    }

    for team in teams.values_mut() {
        team.members.sort();
    }

    Ok(teams.into_values().collect())
}

fn workflow_paths(state: &AppState) -> Result<WorkflowPaths> {
    let paths = ConfigPaths::discover(&state.cwd);
    ensure_workspace_dirs(&paths)?;
    let root = paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow");
    fs::create_dir_all(&root).with_context(|| format!("failed to create {}", root.display()))?;
    let agent_outputs = paths
        .workspace_config_dir
        .join("runtime")
        .join("agent_outputs");
    fs::create_dir_all(&agent_outputs)
        .with_context(|| format!("failed to create {}", agent_outputs.display()))?;
    Ok(WorkflowPaths {
        tasks: root.join("tasks.json"),
        monitor_tasks: root.join("monitor_tasks.json"),
        todos: root.join("todos.json"),
        agents: root.join("agents.json"),
        teams: root.join("teams.json"),
        worktrees: root.join("worktrees.json"),
        shell_outputs: root.join("shell_outputs"),
        agent_outputs,
        root,
    })
}

fn task_argument(args: &str) -> Result<String> {
    let task_id = args
        .split_once(' ')
        .map(|(_, task_id)| task_id.trim())
        .unwrap_or_default();
    if task_id.is_empty() {
        anyhow::bail!("expected a task id");
    }
    Ok(task_id.to_string())
}

fn load_json_store<T>(path: &Path) -> Result<T>
where
    T: DeserializeOwned + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn merge_task_get(stored: Option<WorkflowTaskView>, task: WorkflowTaskView) -> WorkflowTaskView {
    let mut merged = stored.unwrap_or_else(|| default_task_view(&task.task_id));
    merged.task_id = task.task_id;
    if !task.subject.is_empty() {
        merged.subject = task.subject;
    }
    if !task.description.is_empty() {
        merged.description = task.description;
    }
    merged.status = task.status;
    merged.blocks = task.blocks;
    merged.blocked_by = task.blocked_by;
    merged.owner = task.owner.or(merged.owner);
    merged.received_at = task.received_at.or(merged.received_at);
    merged.expires_at = task.expires_at.or(merged.expires_at);
    merged
}

fn merge_task_list(stored: Option<WorkflowTaskView>, task: WorkflowTaskView) -> WorkflowTaskView {
    let mut merged = stored.unwrap_or_else(|| default_task_view(&task.task_id));
    merged.task_id = task.task_id;
    if !task.subject.is_empty() {
        merged.subject = task.subject;
    }
    merged.status = task.status;
    merged.owner = task.owner;
    merged.blocked_by = task.blocked_by;
    merged.received_at = task.received_at;
    merged.expires_at = task.expires_at;
    merged
}

fn default_task_view(task_id: &str) -> WorkflowTaskView {
    WorkflowTaskView {
        task_id: task_id.to_string(),
        subject: String::new(),
        description: String::new(),
        active_form: String::new(),
        status: "pending".to_string(),
        owner: None,
        blocks: Vec::new(),
        blocked_by: Vec::new(),
        metadata: serde_json::Map::new(),
        output: None,
        task_type: Some("task".to_string()),
        command: None,
        process_id: None,
        output_file: None,
        received_at: None,
        expires_at: None,
        started_at_ms: None,
        updated_at_ms: None,
        exit_code: None,
    }
}
