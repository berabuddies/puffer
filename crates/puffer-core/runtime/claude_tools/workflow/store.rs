use anyhow::{bail, Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_session_store::MessageActor;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

const WORKFLOW_DIR_NAME: &str = "runtime/claude_workflow";

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct StoredTask {
    pub(super) task_id: String,
    pub(super) subject: String,
    pub(super) description: String,
    pub(super) active_form: String,
    pub(super) status: String,
    pub(super) owner: Option<String>,
    pub(super) blocks: Vec<String>,
    pub(super) blocked_by: Vec<String>,
    pub(super) metadata: Map<String, Value>,
    pub(super) output: Option<String>,
    #[serde(default)]
    pub(super) task_type: Option<String>,
    #[serde(default)]
    pub(super) command: Option<String>,
    #[serde(default)]
    pub(super) process_id: Option<u32>,
    #[serde(default)]
    pub(super) output_file: Option<String>,
    #[serde(default)]
    pub(super) started_at_ms: Option<u64>,
    #[serde(default)]
    pub(super) updated_at_ms: Option<u64>,
    #[serde(default)]
    pub(super) exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct StoredTodo {
    pub(super) content: String,
    pub(super) status: String,
    pub(super) active_form: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct StoredAgent {
    pub(super) agent_id: String,
    pub(super) name: Option<String>,
    pub(super) description: String,
    pub(super) prompt: String,
    pub(super) subagent_type: Option<String>,
    pub(super) model: Option<String>,
    pub(super) team_name: Option<String>,
    pub(super) mode: Option<String>,
    pub(super) isolation: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) status: String,
    pub(super) output_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct StoredTeam {
    pub(super) team_name: String,
    pub(super) description: Option<String>,
    pub(super) agent_type: Option<String>,
    pub(super) members: Vec<String>,
    #[serde(default)]
    pub(super) lead_session_id: Option<String>,
    #[serde(default)]
    pub(super) lead_agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct ClaudeTeamFile {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(rename = "createdAt")]
    pub(crate) created_at: u64,
    #[serde(rename = "leadAgentId")]
    pub(crate) lead_agent_id: String,
    #[serde(rename = "leadSessionId")]
    pub(crate) lead_session_id: String,
    pub(crate) members: Vec<ClaudeTeamMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ClaudeTeamMember {
    #[serde(rename = "agentId")]
    pub(crate) agent_id: String,
    pub(crate) name: String,
    #[serde(rename = "agentType")]
    pub(crate) agent_type: String,
    #[serde(rename = "joinedAt")]
    pub(crate) joined_at: u64,
    #[serde(default)]
    pub(crate) model: Option<String>,
    pub(crate) cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct StoredCronJob {
    pub(crate) id: String,
    pub(super) cron: String,
    pub(super) prompt: String,
    pub(super) recurring: bool,
    pub(super) durable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StoredMessage {
    pub(crate) id: String,
    pub(crate) to: String,
    #[serde(default)]
    pub(crate) from: String,
    #[serde(default)]
    pub(crate) read: bool,
    pub(crate) summary: Option<String>,
    pub(crate) message: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) actor: Option<MessageActor>,
    pub(crate) created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct StoredWorktree {
    pub(super) name: String,
    pub(super) path: String,
    pub(super) base_cwd: String,
    pub(super) branch: Option<String>,
    #[serde(default)]
    pub(super) original_head_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(super) struct TaskStore {
    pub(super) tasks: Vec<StoredTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(super) struct TodoStore {
    pub(super) todos: Vec<StoredTodo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(super) struct AgentStore {
    pub(super) agents: Vec<StoredAgent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(super) struct TeamStore {
    pub(super) teams: Vec<StoredTeam>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(super) struct CronStore {
    pub(super) jobs: Vec<StoredCronJob>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct MessageStore {
    pub(crate) messages: Vec<StoredMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct PendingShutdownRequest {
    pub(super) request_id: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(super) reason: Option<String>,
    pub(crate) created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(super) struct ShutdownRequestStore {
    pub(super) requests: Vec<PendingShutdownRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(super) struct WorktreeStore {
    pub(super) worktrees: Vec<StoredWorktree>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AgentInput {
    pub(super) description: String,
    pub(super) prompt: String,
    #[serde(default)]
    pub(super) subagent_type: Option<String>,
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default)]
    pub(super) run_in_background: bool,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) team_name: Option<String>,
    #[serde(default)]
    pub(super) mode: Option<String>,
    #[serde(default)]
    pub(super) isolation: Option<String>,
    #[serde(default)]
    pub(super) cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SendMessageInput {
    pub(crate) to: String,
    #[serde(default)]
    pub(crate) summary: Option<String>,
    pub(crate) message: Value,
}

#[derive(Debug, Deserialize)]
pub(super) struct TeamCreateInput {
    pub(super) team_name: String,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    pub(super) agent_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TodoWriteInput {
    pub(super) todos: Vec<TodoInputItem>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TodoInputItem {
    pub(super) content: String,
    pub(super) status: String,
    #[serde(rename = "activeForm")]
    pub(super) active_form: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct TaskCreateInput {
    pub(super) subject: String,
    pub(super) description: String,
    #[serde(default, rename = "activeForm")]
    pub(super) active_form: Option<String>,
    #[serde(default)]
    pub(super) metadata: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TaskIdInput {
    #[serde(rename = "taskId")]
    pub(super) task_id: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct TaskUpdateInput {
    #[serde(rename = "taskId")]
    pub(super) task_id: String,
    #[serde(default)]
    pub(super) subject: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default, rename = "activeForm")]
    pub(super) active_form: Option<String>,
    #[serde(default)]
    pub(super) status: Option<String>,
    #[serde(default, rename = "addBlocks")]
    pub(super) add_blocks: Vec<String>,
    #[serde(default, rename = "addBlockedBy")]
    pub(super) add_blocked_by: Vec<String>,
    #[serde(default)]
    pub(super) owner: Option<String>,
    #[serde(default)]
    pub(super) metadata: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TaskStopInput {
    #[serde(default)]
    pub(super) task_id: Option<String>,
    #[serde(default)]
    pub(super) shell_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TaskOutputInput {
    pub(super) task_id: String,
    #[serde(default)]
    pub(super) block: Option<bool>,
    #[serde(default)]
    pub(super) timeout: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AskUserQuestionInput {
    pub(super) questions: Vec<AskUserQuestionItem>,
    #[serde(default)]
    pub(super) answers: Map<String, Value>,
    #[serde(default)]
    pub(super) annotations: Map<String, Value>,
    #[serde(default)]
    pub(super) metadata: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct AskUserQuestionItem {
    pub(super) question: String,
    pub(super) header: String,
    pub(super) options: Vec<AskUserQuestionOption>,
    #[serde(default, rename = "multiSelect")]
    pub(super) multi_select: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct AskUserQuestionOption {
    pub(super) label: String,
    pub(super) description: String,
    #[serde(default)]
    pub(super) preview: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct EnterWorktreeInput {
    #[serde(default)]
    pub(super) name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ExitWorktreeInput {
    pub(super) action: String,
    #[serde(default)]
    pub(super) discard_changes: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct ConfigInput {
    pub(super) setting: String,
    #[serde(default)]
    pub(super) value: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PowerShellInput {
    pub(super) command: String,
    #[serde(default)]
    pub(super) timeout: Option<u64>,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    pub(super) run_in_background: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct CronCreateInput {
    pub(super) cron: String,
    pub(super) prompt: String,
    #[serde(default = "default_true")]
    pub(super) recurring: bool,
    #[serde(default)]
    pub(super) durable: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct CronDeleteInput {
    pub(crate) id: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct SendUserMessageInput {
    pub(super) message: String,
    #[serde(default)]
    pub(super) attachments: Vec<String>,
    pub(super) status: String,
}

pub(super) fn append_agent_message(output_file: &Path, message: &Value) -> Result<()> {
    let previous = fs::read_to_string(output_file).unwrap_or_default();
    let next = format!(
        "{}\n\nMessage:\n{}\n",
        previous.trim_end(),
        serde_json::to_string_pretty(message)?
    );
    fs::write(output_file, next)
        .with_context(|| format!("failed to write {}", output_file.display()))
}

pub(super) fn resolve_recipients(
    cwd: &Path,
    active_team_name: Option<&str>,
    target: &str,
) -> Result<Vec<String>> {
    let target = target.trim();
    if target.is_empty() {
        return Ok(Vec::new());
    }
    if target == "*" {
        if let Some(team_name) = active_team_name {
            let teams = load_store::<TeamStore>(&teams_path(cwd))?;
            if let Some(team) = teams
                .teams
                .iter()
                .find(|team| team.team_name.eq_ignore_ascii_case(team_name))
            {
                return Ok(team.members.clone());
            }
        }
        let agents = load_store::<AgentStore>(&agents_path(cwd))?;
        return Ok(agents
            .agents
            .into_iter()
            .map(|agent| agent.agent_id)
            .collect());
    }

    let teams = load_store::<TeamStore>(&teams_path(cwd))?;
    if let Some(team) = teams
        .teams
        .iter()
        .find(|team| team.team_name.eq_ignore_ascii_case(target))
    {
        return Ok(team.members.clone());
    }

    let agents = load_store::<AgentStore>(&agents_path(cwd))?;
    if let Some(agent) = agents.agents.iter().find(|agent| {
        agent.agent_id.eq_ignore_ascii_case(target)
            || agent
                .name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(target))
    }) {
        return Ok(vec![agent.agent_id.clone()]);
    }

    Ok(Vec::new())
}

pub(super) fn detect_powershell_binary() -> Result<String> {
    for candidate in ["pwsh", "powershell"] {
        if Command::new(candidate)
            .arg("-NoLogo")
            .arg("-Command")
            .arg("$PSVersionTable.PSVersion.ToString()")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return Ok(candidate.to_string());
        }
    }
    bail!("PowerShell is not installed on this system")
}

pub(crate) fn load_store<T>(path: &Path) -> Result<T>
where
    T: DeserializeOwned + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read workflow store {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse workflow store {}", path.display()))
}

pub(crate) fn save_store<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(value)?;
    fs::write(path, raw).with_context(|| format!("failed to write {}", path.display()))
}

fn home_root(paths: &ConfigPaths) -> PathBuf {
    paths
        .user_config_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| paths.user_config_dir.clone())
}

pub(super) fn workflow_root(cwd: &Path) -> Result<PathBuf> {
    let paths = ConfigPaths::discover(cwd);
    ensure_workspace_dirs(&paths)?;
    let root = paths.workspace_config_dir.join(WORKFLOW_DIR_NAME);
    fs::create_dir_all(&root)?;
    Ok(root)
}

/// Non-panicking variant — returns `None` when the workflow directory cannot
/// be created (e.g. read-only filesystem in tests).
pub(super) fn workflow_root_opt(cwd: &Path) -> Option<PathBuf> {
    workflow_root(cwd).ok()
}

pub(crate) fn team_lead_agent_id(team_name: &str) -> String {
    format!("team-lead@{team_name}")
}

pub(crate) fn claude_dir(cwd: &Path) -> Result<PathBuf> {
    let paths = ConfigPaths::discover(cwd);
    ensure_workspace_dirs(&paths)?;
    let root = home_root(&paths).join(".claude");
    fs::create_dir_all(&root)?;
    Ok(root)
}

pub(crate) fn claude_team_dir(cwd: &Path, team_name: &str) -> Result<PathBuf> {
    let dir = claude_dir(cwd)?.join("teams").join(team_name);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub(crate) fn claude_team_file_path(cwd: &Path, team_name: &str) -> Result<PathBuf> {
    Ok(claude_team_dir(cwd, team_name)?.join("config.json"))
}

pub(crate) fn claude_task_dir(cwd: &Path, team_name: &str) -> Result<PathBuf> {
    let dir = claude_dir(cwd)?.join("tasks").join(team_name);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub(crate) fn write_claude_team_file(cwd: &Path, team_file: &ClaudeTeamFile) -> Result<PathBuf> {
    let path = claude_team_file_path(cwd, &team_file.name)?;
    save_store(&path, team_file)?;
    Ok(path)
}

pub(crate) fn load_claude_team_file(cwd: &Path, team_name: &str) -> Result<Option<ClaudeTeamFile>> {
    let path = claude_team_file_path(cwd, team_name)?;
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(load_store::<ClaudeTeamFile>(&path)?))
}

pub(crate) fn remove_claude_team_artifacts(cwd: &Path, team_name: &str) -> Result<()> {
    let claude_root = claude_dir(cwd)?;
    let team_dir = claude_root.join("teams").join(team_name);
    if team_dir.exists() {
        fs::remove_dir_all(&team_dir)
            .with_context(|| format!("failed to remove {}", team_dir.display()))?;
    }
    let task_dir = claude_root.join("tasks").join(team_name);
    if task_dir.exists() {
        fs::remove_dir_all(&task_dir)
            .with_context(|| format!("failed to remove {}", task_dir.display()))?;
    }
    Ok(())
}

pub(super) fn find_team_for_session(cwd: &Path, session_id: &str) -> Result<Option<StoredTeam>> {
    let teams = load_store::<TeamStore>(&teams_path(cwd))?;
    Ok(teams
        .teams
        .into_iter()
        .find(|team| team.lead_session_id.as_deref() == Some(session_id)))
}

pub(crate) fn register_team_member(
    cwd: &Path,
    team_name: &str,
    member: ClaudeTeamMember,
) -> Result<()> {
    let mut teams = load_store::<TeamStore>(&teams_path(cwd))?;
    let Some(team_index) = teams
        .teams
        .iter()
        .position(|team| team.team_name == team_name)
    else {
        bail!("unknown team `{team_name}`");
    };
    let (team_changed, lead_agent_id, lead_session_id, team_description) = {
        let team = &mut teams.teams[team_index];
        let mut changed = false;
        if !team
            .members
            .iter()
            .any(|existing| existing == &member.agent_id)
        {
            team.members.push(member.agent_id.clone());
            changed = true;
        }
        (
            changed,
            team.lead_agent_id
                .clone()
                .unwrap_or_else(|| team_lead_agent_id(team_name)),
            team.lead_session_id.clone().unwrap_or_default(),
            team.description.clone(),
        )
    };
    if team_changed {
        save_store(&teams_path(cwd), &teams)?;
    }

    let mut team_file = load_claude_team_file(cwd, team_name)?.unwrap_or(ClaudeTeamFile {
        name: team_name.to_string(),
        description: team_description,
        created_at: now_ms(),
        lead_agent_id,
        lead_session_id,
        members: Vec::new(),
    });
    if !team_file
        .members
        .iter()
        .any(|existing| existing.agent_id == member.agent_id)
    {
        team_file.members.push(member);
        let _ = write_claude_team_file(cwd, &team_file)?;
    }
    Ok(())
}

pub(super) fn tasks_path(cwd: &Path, session_id: &Uuid) -> PathBuf {
    let dir = workflow_root(cwd)
        .unwrap()
        .join("sessions")
        .join(session_id.to_string());
    let _ = fs::create_dir_all(&dir);
    dir.join("tasks.json")
}

/// Returns the directory used to persist one team's structured task list.
pub(super) fn team_tasks_dir(cwd: &Path, team_name: &str) -> Result<PathBuf> {
    let dir = workflow_root(cwd)?
        .join("team_tasks")
        .join(team_name.trim());
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Returns the path used to persist one team's structured task list.
pub(super) fn team_tasks_path(cwd: &Path, team_name: &str) -> Result<PathBuf> {
    Ok(team_tasks_dir(cwd, team_name)?.join("tasks.json"))
}

/// Returns the structured task store path for the active team when present.
pub(super) fn structured_tasks_path(
    cwd: &Path,
    session_id: &Uuid,
    active_team_name: Option<&str>,
) -> Result<PathBuf> {
    match active_team_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        Some(team_name) => team_tasks_path(cwd, team_name),
        None => Ok(tasks_path(cwd, session_id)),
    }
}

pub(super) fn todos_path(cwd: &Path) -> PathBuf {
    workflow_root(cwd).unwrap().join("todos.json")
}

pub(super) fn agents_path(cwd: &Path) -> PathBuf {
    workflow_root(cwd).unwrap().join("agents.json")
}

pub(super) fn teams_path(cwd: &Path) -> PathBuf {
    workflow_root(cwd).unwrap().join("teams.json")
}

pub(super) fn crons_path(cwd: &Path) -> PathBuf {
    workflow_root(cwd).unwrap().join("crons.json")
}

pub(crate) fn messages_path(cwd: &Path) -> PathBuf {
    workflow_root(cwd).unwrap().join("messages.json")
}

pub(super) fn shutdown_requests_path(cwd: &Path) -> PathBuf {
    workflow_root(cwd).unwrap().join("shutdown_requests.json")
}

pub(super) fn worktrees_path(cwd: &Path) -> PathBuf {
    workflow_root(cwd).unwrap().join("worktrees.json")
}

/// Returns the directory used to persist background shell task output.
pub(super) fn shell_output_dir(cwd: &Path) -> Result<PathBuf> {
    let dir = workflow_root(cwd)?.join("shell_outputs");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Returns the log path used to persist one background shell task's output.
pub(super) fn shell_output_path(cwd: &Path, task_id: &str) -> Result<PathBuf> {
    Ok(shell_output_dir(cwd)?.join(format!("{task_id}.log")))
}

/// Returns the directory used to persist background workflow task output.
pub(super) fn task_output_dir(cwd: &Path) -> Result<PathBuf> {
    let dir = workflow_root(cwd)?.join("task_outputs");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Returns the log path used to persist one background workflow task's output.
pub(super) fn task_output_path(cwd: &Path, task_id: &str) -> Result<PathBuf> {
    Ok(task_output_dir(cwd)?.join(format!("{task_id}.log")))
}

pub(super) fn git_toplevel(cwd: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args([
            "-C",
            cwd.to_string_lossy().as_ref(),
            "rev-parse",
            "--show-toplevel",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

pub(super) fn is_git_repo(path: &Path) -> bool {
    Command::new("git")
        .args([
            "-C",
            path.to_string_lossy().as_ref(),
            "rev-parse",
            "--git-dir",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(super) fn git_dirty(path: &Path) -> Result<bool> {
    let output = Command::new("git")
        .args([
            "-C",
            path.to_string_lossy().as_ref(),
            "status",
            "--porcelain",
        ])
        .output()
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

/// Returns the current HEAD commit for the git repository at the given path.
pub(super) fn git_head_commit(path: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["-C", path.to_string_lossy().as_ref(), "rev-parse", "HEAD"])
        .output()
        .with_context(|| format!("failed to read HEAD for {}", path.display()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!commit.is_empty()).then_some(commit))
}

/// Returns the number of commits present in `path` after the provided base commit.
pub(super) fn git_ahead_count(path: &Path, base_commit: &str) -> Result<u64> {
    let output = Command::new("git")
        .args([
            "-C",
            path.to_string_lossy().as_ref(),
            "rev-list",
            "--count",
            &format!("{base_commit}..HEAD"),
        ])
        .output()
        .with_context(|| format!("failed to inspect commit divergence for {}", path.display()))?;
    if !output.status.success() {
        bail!("failed to inspect commit divergence for {}", path.display());
    }
    let count = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .with_context(|| format!("failed to parse commit count for {}", path.display()))?;
    Ok(count)
}

pub(super) fn resolve_path(cwd: &Path, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    }
}

pub(super) fn ensure_safe_identifier(value: &str, field: &str) -> Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("{field} must not be empty");
    }
    let path = Path::new(trimmed);
    if path.is_absolute()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("{field} must be a simple identifier without path components");
    }
    Ok(())
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Parses the operating-system process id from a shell task identifier.
pub(super) fn parse_shell_task_pid(task_id: &str) -> Option<u32> {
    task_id.strip_prefix("shell-")?.parse::<u32>().ok()
}

/// Returns the next sequential workflow task id.
pub(super) fn next_task_id(tasks: &[StoredTask]) -> String {
    let next = tasks
        .iter()
        .filter_map(|task| task.task_id.strip_prefix("task-"))
        .filter_map(|suffix| suffix.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    format!("task-{next}")
}

/// Returns true when the operating-system process is still running.
pub(super) fn process_is_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}")])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .is_ok_and(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .any(|line| line.contains(&pid.to_string()))
            })
    }
}

/// Waits for the process to exit, returning true when it stops within the timeout.
pub(super) fn wait_for_process_exit(pid: u32, timeout_ms: u64) -> bool {
    let deadline = now_ms().saturating_add(timeout_ms);
    while now_ms() < deadline {
        if !process_is_running(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    !process_is_running(pid)
}

/// Attempts to terminate a background shell process by pid.
pub(super) fn terminate_process(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .with_context(|| format!("failed to send SIGTERM to process {pid}"))?;
        if !status.success() {
            bail!("failed to stop process {pid}");
        }
    }
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .with_context(|| format!("failed to terminate process {pid}"))?;
        if !status.success() {
            bail!("failed to stop process {pid}");
        }
    }
    Ok(())
}

/// Validates the minimal five-field cron expression shape used by workflow tools.
pub(super) fn validate_cron_expression(cron: &str) -> Result<()> {
    let fields = cron.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5 || fields.iter().any(|field| field.trim().is_empty()) {
        bail!("cron expression must contain exactly 5 non-empty fields");
    }
    Ok(())
}

/// Validates the bounded multiple-choice shape used by `AskUserQuestion`.
pub(super) fn validate_ask_user_questions(items: &[AskUserQuestionItem]) -> Result<()> {
    if items.is_empty() || items.len() > 4 {
        bail!("AskUserQuestion requires between 1 and 4 questions");
    }
    let mut seen_questions = std::collections::BTreeSet::new();
    for item in items {
        if item.question.trim().is_empty() {
            bail!("AskUserQuestion questions must not be empty");
        }
        if !seen_questions.insert(item.question.trim().to_ascii_lowercase()) {
            bail!("AskUserQuestion question texts must be unique");
        }
        if item.header.trim().is_empty() {
            bail!("AskUserQuestion headers must not be empty");
        }
        if item.options.len() < 2 || item.options.len() > 4 {
            bail!(
                "AskUserQuestion question `{}` must provide between 2 and 4 options",
                item.header
            );
        }
        let mut seen_labels = std::collections::BTreeSet::new();
        if item.multi_select && item.options.iter().any(|option| option.preview.is_some()) {
            bail!(
                "AskUserQuestion question `{}` cannot use previews with multiSelect",
                item.header
            );
        }
        for option in &item.options {
            if option.label.trim().is_empty() || option.description.trim().is_empty() {
                bail!(
                    "AskUserQuestion question `{}` has an option with empty label or description",
                    item.header
                );
            }
            if !seen_labels.insert(option.label.to_ascii_lowercase()) {
                bail!(
                    "AskUserQuestion question `{}` has duplicate option labels",
                    item.header
                );
            }
        }
    }
    Ok(())
}
