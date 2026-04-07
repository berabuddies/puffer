use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use std::time::UNIX_EPOCH;

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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct StoredCronJob {
    pub(super) id: String,
    pub(super) cron: String,
    pub(super) prompt: String,
    pub(super) recurring: bool,
    pub(super) durable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct StoredMessage {
    pub(super) id: String,
    pub(super) to: String,
    pub(super) summary: Option<String>,
    pub(super) message: Value,
    pub(super) created_at_ms: u64,
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
pub(super) struct MessageStore {
    pub(super) messages: Vec<StoredMessage>,
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
    pub(super) to: String,
    #[serde(default)]
    pub(super) summary: Option<String>,
    pub(super) message: Value,
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
    #[serde(default, rename = "dangerouslyDisableSandbox")]
    pub(super) dangerously_disable_sandbox: bool,
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
    pub(super) id: String,
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

pub(super) fn resolve_recipients(cwd: &Path, target: &str) -> Result<Vec<String>> {
    let target = target.trim();
    if target.is_empty() {
        return Ok(Vec::new());
    }
    if target == "*" {
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

pub(super) fn get_config_value(state: &AppState, setting: &str) -> Result<Value> {
    match normalize_setting_key(setting) {
        "theme" => Ok(json!(state.config.theme)),
        "model" => Ok(json!(state.current_model)),
        "editorMode" => Ok(json!(state.config.editor_mode)),
        "default_provider" => Ok(json!(state.config.default_provider)),
        "default_model" => Ok(json!(state.config.default_model)),
        "openai_base_url" => Ok(json!(state.config.openai_base_url)),
        "openai_headers" => Ok(json!(state.config.openai_headers)),
        "openai_query_params" => Ok(json!(state.config.openai_query_params)),
        "no_alt_screen" => Ok(json!(state.config.ui.no_alt_screen)),
        "tmux_golden_mode" => Ok(json!(state.config.ui.tmux_golden_mode)),
        "status_line_command" => Ok(json!(state
            .config
            .ui
            .status_line
            .as_ref()
            .map(|status_line| status_line.command.as_str()))),
        "status_line_padding" => Ok(json!(state
            .config
            .ui
            .status_line
            .as_ref()
            .map(|status_line| status_line.padding))),
        "fastMode" => Ok(json!(state.config.fast_mode)),
        "effortLevel" => Ok(json!(state
            .config
            .effort_level
            .as_deref()
            .unwrap_or("auto"))),
        "promptColor" => Ok(json!(state.prompt_color)),
        "statuslineEnabled" => Ok(json!(state.statusline_enabled)),
        other => bail!("Unsupported config setting `{other}`"),
    }
}

pub(super) fn set_config_value(state: &mut AppState, setting: &str, value: Value) -> Result<()> {
    match normalize_setting_key(setting) {
        "theme" => {
            state.config.theme = value
                .as_str()
                .ok_or_else(|| anyhow!("theme must be a string"))?
                .to_string()
        }
        "model" => match value {
            Value::Null => {
                state.current_model = None;
                state.current_provider = state.config.default_provider.clone();
                state.config.default_model = None;
            }
            Value::String(model) => {
                state.current_model = Some(model.clone());
                state.current_provider = model
                    .split_once('/')
                    .map(|(provider, _)| provider.to_string())
                    .or_else(|| state.current_provider.clone());
                state.config.default_model = Some(model);
            }
            _ => bail!("model must be a string or null"),
        },
        "editorMode" => {
            let mode = value
                .as_str()
                .ok_or_else(|| anyhow!("editorMode must be a string"))?;
            match mode {
                "vim" => {
                    state.vim_mode = true;
                    state.config.editor_mode = "vim".to_string();
                }
                "default" | "normal" => {
                    state.vim_mode = false;
                    state.config.editor_mode = "normal".to_string();
                }
                other => bail!("unsupported editorMode `{other}`"),
            }
        }
        "default_provider" => {
            state.config.default_provider = match value {
                Value::Null => None,
                Value::String(text) => Some(text),
                _ => bail!("default_provider must be a string"),
            };
            state.current_provider = state.config.default_provider.clone();
        }
        "default_model" => {
            state.config.default_model = match value {
                Value::Null => None,
                Value::String(text) => Some(text),
                _ => bail!("default_model must be a string"),
            };
            state.current_model = state.config.default_model.clone();
        }
        "openai_base_url" => {
            state.config.openai_base_url = match value {
                Value::Null => None,
                Value::String(text) => Some(text),
                _ => bail!("openai_base_url must be a string"),
            }
        }
        "openai_headers" => {
            state.config.openai_headers = value_to_string_map(value, "openai_headers")?;
        }
        "openai_query_params" => {
            state.config.openai_query_params = value_to_string_map(value, "openai_query_params")?;
        }
        "no_alt_screen" => {
            state.config.ui.no_alt_screen = value
                .as_bool()
                .ok_or_else(|| anyhow!("no_alt_screen must be a boolean"))?
        }
        "tmux_golden_mode" => {
            state.config.ui.tmux_golden_mode = value
                .as_bool()
                .ok_or_else(|| anyhow!("tmux_golden_mode must be a boolean"))?
        }
        "status_line_command" => {
            state.config.ui.status_line = match value {
                Value::Null => None,
                Value::String(command) => Some(puffer_config::StatusLineConfig {
                    command,
                    padding: state
                        .config
                        .ui
                        .status_line
                        .as_ref()
                        .map(|status_line| status_line.padding)
                        .unwrap_or(0),
                }),
                _ => bail!("status_line_command must be a string or null"),
            };
        }
        "status_line_padding" => {
            let padding = match value {
                Value::Null => 0,
                Value::Number(number) => number
                    .as_u64()
                    .ok_or_else(|| anyhow!("status_line_padding must be an unsigned integer"))?
                    as u16,
                _ => bail!("status_line_padding must be an unsigned integer or null"),
            };
            let status_line =
                state
                    .config
                    .ui
                    .status_line
                    .get_or_insert(puffer_config::StatusLineConfig {
                        command: String::new(),
                        padding: 0,
                    });
            status_line.padding = padding;
        }
        "fastMode" => {
            let parsed = value
                .as_bool()
                .ok_or_else(|| anyhow!("fastMode must be a boolean"))?;
            state.fast_mode = parsed;
            state.config.fast_mode = parsed;
        }
        "effortLevel" => {
            let parsed = value
                .as_str()
                .ok_or_else(|| anyhow!("effortLevel must be a string"))?;
            match parsed {
                "auto" | "unset" | "default" => {
                    state.effort_level = "auto".to_string();
                    state.config.effort_level = None;
                }
                other => {
                    state.effort_level = other.to_string();
                    state.config.effort_level = Some(other.to_string());
                }
            }
        }
        "promptColor" => {
            state.prompt_color = value
                .as_str()
                .ok_or_else(|| anyhow!("promptColor must be a string"))?
                .to_string()
        }
        "statuslineEnabled" => {
            state.statusline_enabled = value
                .as_bool()
                .ok_or_else(|| anyhow!("statuslineEnabled must be a boolean"))?
        }
        other => bail!("Unsupported config setting `{other}`"),
    }
    Ok(())
}

pub(super) fn config_setting_persists_to_workspace_file(setting: &str) -> bool {
    matches!(
        normalize_setting_key(setting),
        "theme"
            | "model"
            | "editorMode"
            | "default_provider"
            | "default_model"
            | "openai_base_url"
            | "openai_headers"
            | "openai_query_params"
            | "no_alt_screen"
            | "tmux_golden_mode"
            | "status_line_command"
            | "status_line_padding"
    )
}

fn normalize_setting_key(setting: &str) -> &str {
    match setting.trim() {
        "defaultProvider" => "default_provider",
        "defaultModel" => "default_model",
        "openaiBaseUrl" => "openai_base_url",
        "openaiHeaders" => "openai_headers",
        "openaiQueryParams" => "openai_query_params",
        "noAltScreen" => "no_alt_screen",
        "tmuxGoldenMode" => "tmux_golden_mode",
        "statusLineCommand" => "status_line_command",
        "statusLinePadding" => "status_line_padding",
        other => other,
    }
}
fn value_to_string_map(value: Value, setting: &str) -> Result<BTreeMap<String, String>> {
    match value {
        Value::Null => Ok(BTreeMap::new()),
        Value::Object(entries) => entries
            .into_iter()
            .map(|(key, value)| match value {
                Value::String(text) => Ok((key, text)),
                _ => bail!("{setting} must be an object with string values"),
            })
            .collect(),
        _ => bail!("{setting} must be an object with string values"),
    }
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

pub(super) fn load_store<T>(path: &Path) -> Result<T>
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

pub(super) fn save_store<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(value)?;
    fs::write(path, raw).with_context(|| format!("failed to write {}", path.display()))
}

pub(super) fn workflow_root(cwd: &Path) -> Result<PathBuf> {
    let paths = ConfigPaths::discover(cwd);
    ensure_workspace_dirs(&paths)?;
    let root = paths.workspace_config_dir.join(WORKFLOW_DIR_NAME);
    fs::create_dir_all(&root)?;
    Ok(root)
}

pub(super) fn tasks_path(cwd: &Path) -> PathBuf {
    workflow_root(cwd).unwrap().join("tasks.json")
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

pub(super) fn messages_path(cwd: &Path) -> PathBuf {
    workflow_root(cwd).unwrap().join("messages.json")
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

pub(super) fn now_ms() -> u64 {
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
