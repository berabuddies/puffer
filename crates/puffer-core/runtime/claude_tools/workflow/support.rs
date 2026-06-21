use super::store::{
    agents_path, append_agent_message, claude_task_dir, ensure_safe_identifier,
    find_team_for_session, git_ahead_count, git_dirty, git_head_commit, git_toplevel, is_git_repo,
    load_store, messages_path, now_ms, register_team_member, remove_claude_team_artifacts,
    resolve_recipients, save_store, shutdown_requests_path, tasks_path, team_lead_agent_id,
    teams_path, todos_path, workflow_root, worktrees_path, write_claude_team_file, AgentInput,
    AgentStore, ClaudeTeamFile, ClaudeTeamMember, ConfigInput, EnterWorktreeInput,
    ExitWorktreeInput, MessageStore, PendingShutdownRequest, SendMessageInput,
    ShutdownRequestStore, StoredAgent, StoredMessage, StoredTask, StoredTeam, StoredTodo,
    StoredWorktree, TaskStore, TeamCreateInput, TeamStore, TodoStore, TodoWriteInput,
    WorktreeStore,
};
use super::task_runtime::{terminal_task_status, validate_todos};
use crate::config_settings::{
    config_setting_path, config_setting_scope, get_config_value, normalize_config_key,
    persist_config_setting, scope_label, set_config_value,
};
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_session_store::MessageActor;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

fn current_team_name(state: &AppState) -> Option<&str> {
    state
        .active_team_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
}

fn unique_team_name(teams: &TeamStore, requested: &str) -> String {
    let trimmed = requested.trim();
    if !teams.teams.iter().any(|team| team.team_name == trimmed) {
        return trimmed.to_string();
    }
    let mut suffix = 2_u64;
    loop {
        let candidate = format!("{trimmed}-{suffix}");
        if !teams.teams.iter().any(|team| team.team_name == candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

/// Executes the live `Agent` workflow tool.
pub(super) fn execute_agent(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: AgentInput = serde_json::from_value(input).context("invalid Agent input")?;
    let agent_id = format!("agent-{}", Uuid::new_v4().simple());
    let store_cwd = state.session.cwd.as_path();
    let root = workflow_root(store_cwd)?;
    let output_dir = root.join("agent_outputs");
    fs::create_dir_all(&output_dir)?;
    let output_file = output_dir.join(format!("{agent_id}.md"));
    let output_text = format!(
        "# Agent {}\n\nDescription: {}\n\nPrompt:\n{}\n",
        agent_id, parsed.description, parsed.prompt
    );
    fs::write(&output_file, output_text)?;

    let mut agents = load_store::<AgentStore>(&agents_path(store_cwd))?;
    let agent = StoredAgent {
        agent_id: agent_id.clone(),
        name: parsed.name.clone(),
        description: parsed.description.clone(),
        prompt: parsed.prompt.clone(),
        subagent_type: parsed.subagent_type.clone(),
        model: parsed.model.clone(),
        team_name: parsed.team_name.clone(),
        mode: parsed.mode.clone(),
        isolation: parsed.isolation.clone(),
        cwd: parsed.cwd.clone(),
        status: if parsed.run_in_background {
            "async_launched".to_string()
        } else {
            "completed".to_string()
        },
        output_file: output_file.display().to_string(),
    };
    agents.agents.push(agent.clone());
    save_store(&agents_path(store_cwd), &agents)?;

    if let Some(team_name) = parsed.team_name.as_deref() {
        let member = ClaudeTeamMember {
            agent_id: agent_id.clone(),
            name: parsed.name.clone().unwrap_or_else(|| agent_id.clone()),
            agent_type: parsed
                .subagent_type
                .clone()
                .unwrap_or_else(|| "general-purpose".to_string()),
            joined_at: now_ms(),
            model: parsed.model.clone(),
            cwd: parsed
                .cwd
                .clone()
                .unwrap_or_else(|| store_cwd.display().to_string()),
        };
        register_team_member(store_cwd, team_name, member)?;
    }

    state.record_task("Agent", parsed.description.clone(), true);
    Ok(serde_json::to_string_pretty(&json!({
        "status": agent.status,
        "agentId": agent.agent_id,
        "description": agent.description,
        "prompt": agent.prompt,
        "outputFile": agent.output_file,
        "canReadOutputFile": true
    }))?)
}

/// Executes the live `SendMessage` workflow tool.
pub(super) fn execute_send_message(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let _ = cwd;
    let parsed: SendMessageInput =
        serde_json::from_value(input).context("invalid SendMessage input")?;

    // --- Validation ---
    let to = parsed.to.trim().to_string();
    if to.is_empty() {
        bail!("SendMessage requires a non-empty recipient");
    }
    if to.contains('@') && to != "*" {
        bail!("to must be a bare teammate name or \"*\" — do not include @");
    }
    if parsed
        .message
        .as_str()
        .is_some_and(|text| text.trim().is_empty())
    {
        bail!("SendMessage plain-text messages must not be empty");
    }
    let is_structured = parsed.message.get("type").and_then(Value::as_str).is_some();
    if parsed.message.is_string()
        && parsed
            .summary
            .as_ref()
            .map_or(true, |s| s.trim().is_empty())
    {
        bail!("summary is required when message is a string");
    }
    if to == "*" && is_structured {
        bail!("structured messages cannot be broadcast (to: \"*\")");
    }
    if parsed
        .message
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t == "shutdown_response")
        && to != "team-lead"
    {
        bail!("shutdown_response must be sent to \"team-lead\"");
    }

    // --- Sender identity ---
    let from = state.workflow_sender_id();
    let actor = Some(state.assistant_actor());

    let store_cwd = state.session.cwd.as_path();
    let recipients = resolve_recipients(store_cwd, state.active_team_name.as_deref(), &to)?;
    if recipients.is_empty() {
        bail!("SendMessage could not resolve recipient `{to}`");
    }

    // --- Structured message routing ---
    let message_type = parsed
        .message
        .get("type")
        .and_then(Value::as_str)
        .map(String::from);

    match message_type.as_deref() {
        Some("shutdown_request") => {
            return handle_shutdown_request(store_cwd, &from, &recipients, &parsed, actor);
        }
        Some("shutdown_response") => {
            return handle_shutdown_response(store_cwd, &from, &parsed, actor);
        }
        Some("plan_approval_response") => {
            return handle_plan_approval_response(store_cwd, &from, &recipients, &parsed, actor);
        }
        _ => {}
    }

    // --- Default delivery ---
    let mut messages = load_store::<MessageStore>(&messages_path(store_cwd))?;
    let mut agents = load_store::<AgentStore>(&agents_path(store_cwd))?;
    let mut message_ids = Vec::new();
    let mut resumed_agents: Vec<String> = Vec::new();
    for recipient in &recipients {
        let msg_id = format!("msg-{}", Uuid::new_v4().simple());
        message_ids.push(msg_id.clone());
        let stored_msg = StoredMessage {
            id: msg_id,
            to: recipient.clone(),
            from: from.clone(),
            read: false,
            summary: parsed.summary.clone(),
            message: parsed.message.clone(),
            actor: actor.clone(),
            created_at_ms: now_ms(),
        };
        // Try in-process delivery via teammate registry.
        {
            use crate::runtime::teammate_loop::{
                teammate_registry, IncomingMessage, TeammateMessage,
            };
            let registry = teammate_registry().lock().unwrap();
            if let Some(tx) = registry.get(recipient) {
                let text = stored_msg
                    .message
                    .as_str()
                    .unwrap_or(&stored_msg.message.to_string())
                    .to_string();
                let _ = tx.send(TeammateMessage::Incoming(IncomingMessage {
                    from: from.clone(),
                    text,
                }));
            }
        }
        messages.messages.push(stored_msg);
        if let Some(agent) = agents.agents.iter_mut().find(|agent| {
            agent.agent_id == *recipient || agent.name.as_deref() == Some(recipient.as_str())
        }) {
            append_agent_message(Path::new(&agent.output_file), &parsed.message)?;
            // Resume stopped agents: mark as running so the next poll picks them up.
            if matches!(agent.status.as_str(), "stopped" | "completed" | "failed") {
                agent.status = "running".to_string();
                resumed_agents.push(recipient.clone());
            }
        }
    }
    save_store(&messages_path(store_cwd), &messages)?;
    save_store(&agents_path(store_cwd), &agents)?;
    let mut result = json!({
        "delivered": recipients,
        "from": from,
        "messageIds": message_ids,
        "summary": parsed.summary,
        "message": parsed.message
    });
    if !resumed_agents.is_empty() {
        result["resumed"] = json!(resumed_agents);
    }
    Ok(serde_json::to_string_pretty(&result)?)
}

fn handle_shutdown_request(
    store_cwd: &Path,
    from: &str,
    recipients: &[String],
    parsed: &SendMessageInput,
    actor: Option<MessageActor>,
) -> Result<String> {
    let request_id = format!("shutdown-{}", Uuid::new_v4().simple());
    let reason = parsed
        .message
        .get("reason")
        .and_then(Value::as_str)
        .map(String::from);

    let mut store = load_store::<ShutdownRequestStore>(&shutdown_requests_path(store_cwd))?;
    for recipient in recipients {
        store.requests.push(PendingShutdownRequest {
            request_id: request_id.clone(),
            from: from.to_string(),
            to: recipient.clone(),
            reason: reason.clone(),
            created_at_ms: now_ms(),
        });
    }
    save_store(&shutdown_requests_path(store_cwd), &store)?;

    // Deliver the request message with the generated request_id to each recipient.
    let enriched = json!({
        "type": "shutdown_request",
        "request_id": request_id,
        "from": from,
        "reason": reason,
    });
    let mut messages = load_store::<MessageStore>(&messages_path(store_cwd))?;
    let agents = load_store::<AgentStore>(&agents_path(store_cwd))?;
    for recipient in recipients {
        messages.messages.push(StoredMessage {
            id: format!("msg-{}", Uuid::new_v4().simple()),
            to: recipient.clone(),
            from: from.to_string(),
            read: false,
            summary: Some("shutdown request".to_string()),
            message: enriched.clone(),
            actor: actor.clone(),
            created_at_ms: now_ms(),
        });
        if let Some(agent) = agents
            .agents
            .iter()
            .find(|a| a.agent_id == *recipient || a.name.as_deref() == Some(recipient.as_str()))
        {
            append_agent_message(Path::new(&agent.output_file), &enriched)?;
        }
    }
    save_store(&messages_path(store_cwd), &messages)?;

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "message": format!("Shutdown request sent. Request ID: {request_id}"),
        "request_id": request_id,
        "targets": recipients,
    }))?)
}

fn handle_shutdown_response(
    store_cwd: &Path,
    from: &str,
    parsed: &SendMessageInput,
    actor: Option<MessageActor>,
) -> Result<String> {
    let request_id = parsed
        .message
        .get("request_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("shutdown_response requires request_id"))?;
    let approve = parsed
        .message
        .get("approve")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let reason = parsed
        .message
        .get("reason")
        .and_then(Value::as_str)
        .map(String::from);

    if !approve && reason.as_ref().map_or(true, |r| r.trim().is_empty()) {
        bail!("reason is required when rejecting a shutdown request");
    }

    // Validate request_id exists.
    let store = load_store::<ShutdownRequestStore>(&shutdown_requests_path(store_cwd))?;
    if !store.requests.iter().any(|r| r.request_id == request_id) {
        bail!("unknown shutdown request_id `{request_id}`");
    }

    if approve {
        // Set the responding agent's status to stopped.
        let mut agents = load_store::<AgentStore>(&agents_path(store_cwd))?;
        if let Some(agent) = agents
            .agents
            .iter_mut()
            .find(|a| a.agent_id == from || a.name.as_deref() == Some(from))
        {
            agent.status = "stopped".to_string();
        }
        save_store(&agents_path(store_cwd), &agents)?;
    }

    // Deliver response to team-lead mailbox.
    let response_msg = json!({
        "type": "shutdown_response",
        "request_id": request_id,
        "from": from,
        "approve": approve,
        "reason": reason,
    });
    let mut messages = load_store::<MessageStore>(&messages_path(store_cwd))?;
    messages.messages.push(StoredMessage {
        id: format!("msg-{}", Uuid::new_v4().simple()),
        to: "team-lead".to_string(),
        from: from.to_string(),
        read: false,
        summary: Some(if approve {
            "shutdown approved".to_string()
        } else {
            "shutdown rejected".to_string()
        }),
        message: response_msg,
        actor,
        created_at_ms: now_ms(),
    });
    save_store(&messages_path(store_cwd), &messages)?;

    let status = if approve { "approved" } else { "rejected" };
    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "message": format!("Shutdown {status}. Sent confirmation to team-lead."),
        "request_id": request_id,
    }))?)
}

fn handle_plan_approval_response(
    store_cwd: &Path,
    from: &str,
    recipients: &[String],
    parsed: &SendMessageInput,
    actor: Option<MessageActor>,
) -> Result<String> {
    let request_id = parsed
        .message
        .get("request_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("plan_approval_response requires request_id"))?;
    let approve = parsed
        .message
        .get("approve")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let feedback = parsed
        .message
        .get("feedback")
        .and_then(Value::as_str)
        .map(String::from);

    let response_msg = json!({
        "type": "plan_approval_response",
        "request_id": request_id,
        "from": from,
        "approve": approve,
        "feedback": feedback,
    });

    let mut messages = load_store::<MessageStore>(&messages_path(store_cwd))?;
    let agents = load_store::<AgentStore>(&agents_path(store_cwd))?;
    for recipient in recipients {
        messages.messages.push(StoredMessage {
            id: format!("msg-{}", Uuid::new_v4().simple()),
            to: recipient.clone(),
            from: from.to_string(),
            read: false,
            summary: Some(if approve {
                "plan approved".to_string()
            } else {
                format!("plan rejected: {}", feedback.as_deref().unwrap_or(""))
            }),
            message: response_msg.clone(),
            actor: actor.clone(),
            created_at_ms: now_ms(),
        });
        if let Some(agent) = agents
            .agents
            .iter()
            .find(|a| a.agent_id == *recipient || a.name.as_deref() == Some(recipient.as_str()))
        {
            append_agent_message(Path::new(&agent.output_file), &response_msg)?;
        }
    }
    save_store(&messages_path(store_cwd), &messages)?;

    let action = if approve { "approved" } else { "rejected" };
    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "message": format!("Plan {action} for {}", recipients.join(", ")),
        "request_id": request_id,
    }))?)
}

/// Executes the live `TeamCreate` workflow tool.
pub(super) fn execute_team_create(
    state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: TeamCreateInput =
        serde_json::from_value(input).context("invalid TeamCreate input")?;
    let requested_team_name = parsed.team_name.trim();
    ensure_safe_identifier(requested_team_name, "team_name")?;
    let store_cwd = state.session.cwd.as_path();
    if let Some(existing_team_name) = current_team_name(state).map(str::to_string).or_else(|| {
        find_team_for_session(store_cwd, &state.session.id.to_string())
            .ok()
            .flatten()
            .map(|team| team.team_name)
    }) {
        bail!(
            "Already leading team \"{existing_team_name}\". A leader can only manage one team at a time. Use TeamDelete to end the current team before creating a new one."
        );
    }
    let mut teams = load_store::<TeamStore>(&teams_path(store_cwd))?;
    let team_name = unique_team_name(&teams, requested_team_name);
    let workflow = workflow_root(store_cwd)?;
    let team_dir = workflow.join("teams").join(&team_name);
    let task_dir = workflow.join("team_tasks").join(&team_name);
    fs::create_dir_all(&team_dir)?;
    fs::create_dir_all(&task_dir)?;
    // Reset the team task list on create to avoid stale tasks from previous teams.
    save_store(
        &task_dir.join("tasks.json"),
        &TaskStore { tasks: Vec::new() },
    )?;
    let lead_agent_id = team_lead_agent_id(&team_name);
    let lead_agent_type = parsed
        .agent_type
        .clone()
        .unwrap_or_else(|| "team-lead".to_string());
    let team_file = ClaudeTeamFile {
        name: team_name.clone(),
        description: parsed.description.clone(),
        created_at: now_ms(),
        lead_agent_id: lead_agent_id.clone(),
        lead_session_id: state.session.id.to_string(),
        members: vec![ClaudeTeamMember {
            agent_id: lead_agent_id.clone(),
            name: "team-lead".to_string(),
            agent_type: lead_agent_type.clone(),
            joined_at: now_ms(),
            model: state.current_model.clone(),
            cwd: store_cwd.display().to_string(),
        }],
    };
    let team_file_path = write_claude_team_file(store_cwd, &team_file)?;
    let _ = claude_task_dir(store_cwd, &team_name)?;
    teams.teams.push(StoredTeam {
        team_name: team_name.clone(),
        description: parsed.description.clone(),
        agent_type: Some(lead_agent_type),
        members: vec![lead_agent_id.clone()],
        lead_session_id: Some(state.session.id.to_string()),
        lead_agent_id: Some(lead_agent_id.clone()),
    });
    save_store(&teams_path(store_cwd), &teams)?;
    state.active_team_name = Some(team_name.clone());
    Ok(serde_json::to_string_pretty(&json!({
        "team_name": team_name,
        "team_file_path": team_file_path.display().to_string(),
        "lead_agent_id": lead_agent_id,
    }))?)
}

/// Executes the live `TeamDelete` workflow tool.
pub(super) fn execute_team_delete(
    state: &mut AppState,
    _cwd: &Path,
    _input: Value,
) -> Result<String> {
    let store_cwd = state.session.cwd.as_path();
    let team_name = current_team_name(state).map(str::to_string).or_else(|| {
        find_team_for_session(store_cwd, &state.session.id.to_string())
            .ok()
            .flatten()
            .map(|team| team.team_name)
    });
    let Some(team_name) = team_name else {
        state.active_team_name = None;
        return Ok(serde_json::to_string_pretty(&json!({
            "success": true,
            "message": "No team name found, nothing to clean up",
            "team_name": Value::Null,
        }))?);
    };
    let mut teams = load_store::<TeamStore>(&teams_path(store_cwd))?;
    let Some(index) = teams
        .teams
        .iter()
        .position(|team| team.team_name == team_name)
    else {
        state.active_team_name = None;
        return Ok(serde_json::to_string_pretty(&json!({
            "success": true,
            "message": format!("No stored team named \"{team_name}\" was found"),
            "team_name": team_name,
        }))?);
    };
    let team = teams.teams[index].clone();
    let agents = load_store::<AgentStore>(&agents_path(store_cwd))?;
    let lead_agent_id = team
        .lead_agent_id
        .clone()
        .unwrap_or_else(|| team_lead_agent_id(&team.team_name));
    let active_members = team
        .members
        .iter()
        .filter(|member| member.as_str() != lead_agent_id)
        .filter_map(|member| {
            agents
                .agents
                .iter()
                .find(|agent| &agent.agent_id == member)
                .filter(|agent| !terminal_task_status(&agent.status))
                .map(|agent| agent.name.clone().unwrap_or_else(|| agent.agent_id.clone()))
        })
        .collect::<Vec<_>>();
    if !active_members.is_empty() {
        return Ok(serde_json::to_string_pretty(&json!({
            "success": false,
            "message": format!(
                "Cannot cleanup team with {} active member(s): {}. Use requestShutdown to gracefully terminate teammates first.",
                active_members.len(),
                active_members.join(", "),
            ),
            "team_name": team.team_name,
        }))?);
    }

    teams.teams.remove(index);
    save_store(&teams_path(store_cwd), &teams)?;
    let mut agents = agents;
    agents
        .agents
        .retain(|agent| agent.team_name.as_deref() != Some(team.team_name.as_str()));
    save_store(&agents_path(store_cwd), &agents)?;

    let workflow = workflow_root(store_cwd)?;
    let _ = fs::remove_dir_all(workflow.join("teams").join(&team.team_name));
    let _ = fs::remove_dir_all(workflow.join("team_tasks").join(&team.team_name));
    remove_claude_team_artifacts(store_cwd, &team.team_name)?;
    state.active_team_name = None;

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "message": format!("Cleaned up directories and worktrees for team \"{}\"", team.team_name),
        "team_name": team.team_name,
    }))?)
}

/// Executes the live `TodoWrite` workflow tool.
pub(super) fn execute_todo_write(
    state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: TodoWriteInput =
        serde_json::from_value(input).context("invalid TodoWrite input")?;
    validate_todos(&parsed.todos)?;
    let mut store = load_store::<TodoStore>(&todos_path(state.session.cwd.as_path()))?;
    let old = store.todos.clone();
    store.todos = parsed
        .todos
        .into_iter()
        .map(|todo| StoredTodo {
            content: todo.content,
            status: todo.status,
            active_form: todo.active_form,
        })
        .collect();
    save_store(&todos_path(state.session.cwd.as_path()), &store)?;
    Ok(serde_json::to_string_pretty(&json!({
        "oldTodos": old,
        "newTodos": store.todos
    }))?)
}

/// Persists one background Bash task into the session-scoped workflow task store.
pub(super) fn register_background_shell_task(
    cwd: &Path,
    session_id: &Uuid,
    task_id: &str,
    subject: &str,
    command: &str,
    process_id: u32,
    output_file: &Path,
) -> Result<()> {
    let mut store = load_store::<TaskStore>(&tasks_path(cwd, session_id))?;
    let started_at_ms = now_ms();
    store.tasks.retain(|task| task.task_id != task_id);
    store.tasks.push(StoredTask {
        task_id: task_id.to_string(),
        subject: subject.to_string(),
        description: command.to_string(),
        active_form: "Running bash command".to_string(),
        status: "running".to_string(),
        owner: None,
        blocks: Vec::new(),
        blocked_by: Vec::new(),
        metadata: Default::default(),
        output: None,
        task_type: Some("shell".to_string()),
        command: Some(command.to_string()),
        process_id: Some(process_id),
        output_file: Some(output_file.display().to_string()),
        received_at: None,
        expires_at: None,
        started_at_ms: Some(started_at_ms),
        created_at_ms: Some(started_at_ms),
        updated_at_ms: Some(started_at_ms),
        exit_code: None,
        completed_via: None,
    });
    save_store(&tasks_path(cwd, session_id), &store)
}

/// Marks a background shell task as completed in the persistent store.
/// Called from the reaper thread after `child.wait()` returns.
pub(super) fn mark_shell_task_completed(
    cwd: &Path,
    session_id: &Uuid,
    task_id: &str,
    exit_code: Option<i32>,
) -> Result<()> {
    let mut store = load_store::<TaskStore>(&tasks_path(cwd, session_id))?;
    if let Some(task) = store.tasks.iter_mut().find(|t| t.task_id == task_id) {
        if task.status == "running" {
            task.status = "completed".to_string();
            task.exit_code = exit_code;
            task.process_id = None;
            task.updated_at_ms = Some(now_ms());
            if task.output.is_none() {
                task.output = super::task_runtime::read_task_output(task);
            }
        }
    }
    save_store(&tasks_path(cwd, session_id), &store)?;
    // Write a completion marker so the agent loop can detect newly finished tasks.
    let marker_dir = cwd
        .join(".puffer")
        .join("runtime")
        .join("claude_workflow")
        .join("completed_tasks");
    let _ = std::fs::create_dir_all(&marker_dir);
    let _ = std::fs::write(marker_dir.join(task_id), "");
    Ok(())
}

/// Drains completion markers for background shell tasks that finished since
/// the last call.  Returns a human-readable description for each.
pub(super) fn drain_completed_shell_tasks(cwd: &Path, session_id: &Uuid) -> Vec<String> {
    let marker_dir = cwd
        .join(".puffer")
        .join("runtime")
        .join("claude_workflow")
        .join("completed_tasks");
    let entries = match std::fs::read_dir(&marker_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut descriptions = Vec::new();
    let store = load_store::<TaskStore>(&tasks_path(cwd, session_id)).ok();
    for entry in entries.flatten() {
        let task_id = entry.file_name().to_string_lossy().to_string();
        // Remove marker so we don't report it again.
        let _ = std::fs::remove_file(entry.path());
        // Build a description from the stored task metadata.
        let desc = store
            .as_ref()
            .and_then(|s| s.tasks.iter().find(|t| t.task_id == task_id))
            .map(|t| {
                let exit = t
                    .exit_code
                    .map(|c| format!(" (exit code {c})"))
                    .unwrap_or_default();
                format!(
                    "Background task \"{}\" ({}) completed{}",
                    t.subject, t.task_id, exit
                )
            })
            .unwrap_or_else(|| format!("Background task {task_id} completed"));
        descriptions.push(desc);
    }
    descriptions
}

/// Executes the live `EnterWorktree` workflow tool.
pub(super) fn execute_enter_worktree(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: EnterWorktreeInput =
        serde_json::from_value(input).context("invalid EnterWorktree input")?;
    let mut store = load_store::<WorktreeStore>(&worktrees_path(state.session.cwd.as_path()))?;
    if store
        .worktrees
        .iter()
        .any(|worktree| Path::new(&worktree.path) == cwd)
    {
        bail!("already in a managed worktree session");
    }
    let worktree_name = parsed
        .name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("worktree-{}", Uuid::new_v4().simple()));
    ensure_safe_identifier(&worktree_name, "worktree name")?;
    if store
        .worktrees
        .iter()
        .any(|worktree| worktree.name == worktree_name)
    {
        bail!("worktree `{worktree_name}` already exists in this session");
    }
    let base_cwd = cwd.to_path_buf();
    let repo_root = git_toplevel(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let worktree_root = repo_root.join(".worktree");
    fs::create_dir_all(&worktree_root)?;
    let worktree_path = worktree_root.join(&worktree_name);
    if worktree_path.exists() {
        bail!("worktree path {} already exists", worktree_path.display());
    }
    let mut branch = None;
    if is_git_repo(&repo_root) {
        let branch_name = format!("puffer-{}", Uuid::new_v4().simple());
        let status = Command::new("git")
            .args([
                "-C",
                repo_root.to_string_lossy().as_ref(),
                "worktree",
                "add",
                "-b",
                &branch_name,
                worktree_path.to_string_lossy().as_ref(),
            ])
            .status()
            .context("failed to launch git worktree add")?;
        if !status.success() {
            bail!("git worktree add failed for {}", worktree_path.display());
        }
        branch = Some(branch_name);
    } else {
        fs::create_dir_all(&worktree_path)?;
    }
    store.worktrees.push(StoredWorktree {
        name: worktree_name.clone(),
        path: worktree_path.display().to_string(),
        base_cwd: base_cwd.display().to_string(),
        branch: branch.clone(),
        original_head_commit: git_head_commit(&repo_root)?,
    });
    save_store(&worktrees_path(state.session.cwd.as_path()), &store)?;

    if !state.working_dirs.iter().any(|path| path == &worktree_path) {
        state.working_dirs.push(worktree_path.clone());
    }
    state.cwd = worktree_path.clone();
    Ok(serde_json::to_string_pretty(&json!({
        "name": worktree_name,
        "path": worktree_path.display().to_string(),
        "branch": branch,
        "baseCwd": base_cwd.display().to_string()
    }))?)
}

/// Executes the live `ExitWorktree` workflow tool.
pub(super) fn execute_exit_worktree(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: ExitWorktreeInput =
        serde_json::from_value(input).context("invalid ExitWorktree input")?;
    if !matches!(parsed.action.as_str(), "keep" | "remove") {
        bail!("ExitWorktree action must be `keep` or `remove`");
    }
    let mut store = load_store::<WorktreeStore>(&worktrees_path(state.session.cwd.as_path()))?;
    let index = store
        .worktrees
        .iter()
        .position(|worktree| Path::new(&worktree.path) == cwd)
        .ok_or_else(|| anyhow!("no active worktree session found"))?;
    let entry = store.worktrees[index].clone();
    let worktree_path = PathBuf::from(&entry.path);
    let base_cwd = PathBuf::from(&entry.base_cwd);

    if parsed.action == "remove" {
        if is_git_repo(&base_cwd) {
            let dirty = git_dirty(&worktree_path).unwrap_or(false);
            let ahead = entry
                .original_head_commit
                .as_deref()
                .map(|head| git_ahead_count(&worktree_path, head))
                .transpose()?
                .unwrap_or(0);
            if (dirty || ahead > 0) && !parsed.discard_changes {
                bail!("worktree has uncommitted changes; set discard_changes=true to remove it");
            }
            let mut args = vec![
                "-C".to_string(),
                base_cwd.to_string_lossy().to_string(),
                "worktree".to_string(),
                "remove".to_string(),
            ];
            if parsed.discard_changes {
                args.push("--force".to_string());
            }
            args.push(worktree_path.to_string_lossy().to_string());
            let status = Command::new("git")
                .args(args)
                .status()
                .context("failed to launch git worktree remove")?;
            if !status.success() {
                bail!("git worktree remove failed for {}", worktree_path.display());
            }
        } else if worktree_path.exists() {
            fs::remove_dir_all(&worktree_path)
                .with_context(|| format!("failed to remove {}", worktree_path.display()))?;
        }
    }
    store.worktrees.remove(index);
    save_store(&worktrees_path(state.session.cwd.as_path()), &store)?;
    state.cwd = base_cwd.clone();
    state.working_dirs.retain(|path| path != &worktree_path);
    Ok(serde_json::to_string_pretty(&json!({
        "action": parsed.action,
        "returnedTo": base_cwd.display().to_string(),
        "worktreePath": worktree_path.display().to_string(),
        "worktreeBranch": entry.branch,
    }))?)
}

/// Executes the live `Config` workflow tool.
pub(super) fn execute_config(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let has_value = input.get("value").is_some();
    let parsed: ConfigInput = serde_json::from_value(input).context("invalid Config input")?;
    let paths = ConfigPaths::discover(cwd);
    ensure_workspace_dirs(&paths)?;
    let previous = get_config_value(state, &parsed.setting)?;
    let operation = if has_value { "set" } else { "get" };
    let scope = config_setting_scope(&parsed.setting)?;
    let storage_path = config_setting_path(&paths, &parsed.setting)?;
    if has_value {
        let value = parsed.value.unwrap_or(Value::Null);
        if normalize_config_key(&parsed.setting) == Some("status_line_command") && !value.is_null()
        {
            bail!("Config tool cannot set executable status_line_command");
        }
        set_config_value(state, &parsed.setting, value)?;
        let _ = persist_config_setting(&paths, state, &parsed.setting)?;
    }
    let current = get_config_value(state, &parsed.setting)?;
    let path_value = storage_path
        .as_ref()
        .map(|path: &PathBuf| Value::String(path.display().to_string()))
        .unwrap_or(Value::Null);
    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "operation": operation,
        "scope": scope_label(scope),
        "setting": parsed.setting,
        "value": current,
        "previousValue": previous,
        "newValue": if operation == "set" { current.clone() } else { Value::Null },
        "persisted": storage_path.is_some(),
        "path": path_value
    }))?)
}
