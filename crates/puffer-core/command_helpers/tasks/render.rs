use crate::{AppState, TaskStatus};
use serde_json::Value;
use std::fmt::Write as _;

use super::{
    task_actions, task_ignore_reasons, task_is_ignored, task_is_monitor, WorkflowAgentView,
    WorkflowTaskView, WorkflowTeamView, WorkflowTodoView, WorkflowWorktreeView,
};

pub(super) fn append_task_section<'a>(
    text: &mut String,
    title: &str,
    tasks: &[&'a WorkflowTaskView],
) {
    let _ = writeln!(text, "\n{title}:");
    if tasks.is_empty() {
        let _ = writeln!(text, "- <none>");
        return;
    }
    for task in tasks {
        let monitor_actions = monitor_action_labels(task);
        let _ = writeln!(
            text,
            "- {} [{}:{}] {}{}",
            task.task_id,
            task_kind(task),
            task.status,
            task.subject,
            monitor_actions
        );
        if let Some(owner) = task.owner.as_deref() {
            let _ = writeln!(text, "  owner={owner}");
        }
        if !task.blocked_by.is_empty() {
            let _ = writeln!(text, "  blocked_by={}", task.blocked_by.join(", "));
        }
        if !task.blocks.is_empty() {
            let _ = writeln!(text, "  blocks={}", task.blocks.join(", "));
        }
        if let Some(command) = task.command.as_deref() {
            let _ = writeln!(text, "  command={}", shorten(command, 120));
        } else {
            let _ = writeln!(text, "  detail={}", shorten(&task.description, 120));
        }
    }
}

pub(super) fn append_agent_section(text: &mut String, agents: &[WorkflowAgentView]) {
    let _ = writeln!(text, "\nBackground agents:");
    if agents.is_empty() {
        let _ = writeln!(text, "- <none>");
        return;
    }
    for agent in agents {
        let label = agent.name.as_deref().unwrap_or(agent.agent_id.as_str());
        let _ = writeln!(text, "- {} [{}] {}", agent.agent_id, agent.status, label);
        let _ = writeln!(text, "  detail={}", shorten(&agent.description, 120));
        if let Some(model) = agent.model.as_deref() {
            let _ = writeln!(text, "  model={model}");
        }
        if let Some(team_name) = agent.team_name.as_deref() {
            let _ = writeln!(text, "  team={team_name}");
        }
    }
}

pub(super) fn append_team_section(text: &mut String, teams: &[WorkflowTeamView]) {
    let _ = writeln!(text, "\nTeams:");
    if teams.is_empty() {
        let _ = writeln!(text, "- <none>");
        return;
    }
    for team in teams {
        let _ = writeln!(
            text,
            "- {} members={} agent_type={}",
            team.team_name,
            team.members.len(),
            team.agent_type.as_deref().unwrap_or("<none>")
        );
        if let Some(description) = team.description.as_deref() {
            let _ = writeln!(text, "  detail={}", shorten(description, 120));
        }
        if !team.members.is_empty() {
            let _ = writeln!(text, "  members={}", team.members.join(", "));
        }
    }
}

pub(super) fn append_worktree_section(text: &mut String, worktrees: &[WorkflowWorktreeView]) {
    let _ = writeln!(text, "\nWorktrees:");
    if worktrees.is_empty() {
        let _ = writeln!(text, "- <none>");
        return;
    }
    for worktree in worktrees {
        let _ = writeln!(
            text,
            "- {} branch={} path={}",
            worktree.name,
            worktree.branch.as_deref().unwrap_or("<none>"),
            worktree.path
        );
        let _ = writeln!(text, "  base_cwd={}", worktree.base_cwd);
        if let Some(commit) = worktree.original_head_commit.as_deref() {
            let _ = writeln!(text, "  original_head_commit={commit}");
        }
    }
}

pub(super) fn append_todo_section(text: &mut String, todos: &[WorkflowTodoView]) {
    let _ = writeln!(text, "\nTodos:");
    if todos.is_empty() {
        let _ = writeln!(text, "- <none>");
        return;
    }
    for todo in todos {
        let _ = writeln!(
            text,
            "- [{}] {} ({})",
            todo.status, todo.content, todo.active_form
        );
    }
}

pub(super) fn append_runtime_section(text: &mut String, state: &AppState) {
    let _ = writeln!(text, "\nRecent runtime activity:");
    if state.tasks().is_empty() {
        let _ = writeln!(text, "- <none>");
        return;
    }
    for task in state.tasks().iter().rev().take(10) {
        let status = match task.status {
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
        };
        let _ = writeln!(text, "- #{} {} [{}]", task.id, task.label, status);
        let _ = writeln!(text, "  {}", shorten(&task.detail, 120));
    }
}

pub(super) fn render_task_detail(task: &WorkflowTaskView) -> String {
    let mut text = String::new();
    let _ = writeln!(
        &mut text,
        "Task {}\ntype={}\nstatus={}\nsubject={}\ndescription={}\nactive_form={}\nowner={}\ncommand={}\nprocess_id={}\noutput_file={}\nstarted_at_ms={}\nupdated_at_ms={}\nexit_code={}",
        task.task_id,
        task_kind(task),
        task.status,
        task.subject,
        task.description,
        task.active_form,
        task.owner.as_deref().unwrap_or("<none>"),
        task.command.as_deref().unwrap_or("<none>"),
        task.process_id
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        task.output_file.as_deref().unwrap_or("<none>"),
        display_optional_u64(task.started_at_ms),
        display_optional_u64(task.updated_at_ms),
        task.exit_code
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<none>".to_string())
    );
    let _ = writeln!(
        &mut text,
        "blocked_by={}\nblocks={}",
        render_list(&task.blocked_by),
        render_list(&task.blocks)
    );
    let _ = writeln!(
        &mut text,
        "metadata={}",
        if task.metadata.is_empty() {
            "<none>".to_string()
        } else {
            serde_json::to_string_pretty(&task.metadata).unwrap_or_default()
        }
    );
    if let Some(output) = task.output.as_deref() {
        let _ = writeln!(&mut text, "\nOutput preview:\n{}", preview_text(output, 20));
    }
    text.trim_end().to_string()
}

pub(super) fn render_agent_detail(agent: &WorkflowAgentView) -> String {
    format!(
        "Agent {}\nname={}\nstatus={}\ndescription={}\nprompt={}\nsubagent_type={}\nmodel={}\nteam_name={}\nmode={}\nisolation={}\ncwd={}\noutput_file={}",
        agent.agent_id,
        agent.name.as_deref().unwrap_or("<none>"),
        agent.status,
        agent.description,
        agent.prompt,
        agent.subagent_type.as_deref().unwrap_or("<none>"),
        agent.model.as_deref().unwrap_or("<none>"),
        agent.team_name.as_deref().unwrap_or("<none>"),
        agent.mode.as_deref().unwrap_or("<none>"),
        agent.isolation.as_deref().unwrap_or("<none>"),
        agent.cwd.as_deref().unwrap_or("<none>"),
        agent.output_file
    )
}

pub(super) fn task_kind(task: &WorkflowTaskView) -> &str {
    task.task_type.as_deref().unwrap_or("task")
}

pub(super) fn supports_task_stop(task: &WorkflowTaskView) -> bool {
    task.process_id.is_some()
        || task.command.is_some()
        || matches!(task.task_type.as_deref(), Some(kind) if kind != "task")
}

pub(super) fn agent_status_is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "stopped" | "deleted")
}

pub(super) fn shorten(text: &str, limit: usize) -> String {
    let mut shortened = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= limit {
            shortened.push_str("...");
            return shortened;
        }
        shortened.push(ch);
    }
    shortened
}

pub(super) fn value_as_display(value: Option<&Value>) -> String {
    match value {
        Some(Value::Null) | None => "<none>".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

fn render_list(values: &[String]) -> String {
    if values.is_empty() {
        "<none>".to_string()
    } else {
        values.join(", ")
    }
}

fn display_optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "<none>".to_string())
}

fn preview_text(text: &str, max_lines: usize) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return text.to_string();
    }
    let mut preview = lines[..max_lines].join("\n");
    let _ = write!(
        &mut preview,
        "\n... ({} more lines, use `/tasks output <task-id>` for full output)",
        lines.len() - max_lines
    );
    preview
}

fn monitor_action_labels(task: &WorkflowTaskView) -> String {
    if !task_is_monitor(&task.metadata) || task_is_ignored(&task.metadata) {
        return String::new();
    }
    let mut labels = vec!["Ignore".to_string()];
    for reason in task_ignore_reasons(&task.metadata).into_iter().take(2) {
        labels.push(format!("Ignore: {}", shorten(&reason, 28)));
    }
    for action in task_actions(&task.metadata) {
        labels.push(shorten(&action.name, 32));
    }
    format!(
        " {}",
        labels
            .into_iter()
            .map(|label| format!("[{label}]"))
            .collect::<Vec<_>>()
            .join(" ")
    )
}
