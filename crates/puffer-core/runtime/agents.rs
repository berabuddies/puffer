use crate::agent_catalog::load_agent_resources;
use crate::plan_mode::enter_plan_mode;
use crate::runtime::claude_tools::workflow::store::{register_team_member, ClaudeTeamMember};
use crate::tool_names::tool_spec_matches_selector;
use crate::{AppState, MessageRole};
use anyhow::{anyhow, bail, Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::{
    agent_by_id, plugin_mcp_servers, skill_by_name, AgentSpec, LoadedResources, ToolSpec,
};
use puffer_session_store::{MessageActor, MessageActorKind};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
struct AgentToolInput {
    description: String,
    prompt: String,
    #[serde(default)]
    subagent_type: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<AgentEffortInput>,
    #[serde(default, alias = "permissionMode")]
    permission_mode: Option<String>,
    #[serde(default, alias = "maxTurns")]
    max_turns: Option<u32>,
    #[serde(default)]
    run_in_background: bool,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    isolation: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    team_name: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default, alias = "initialPrompt")]
    initial_prompt: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
enum AgentEffortInput {
    Name(String),
    Number(u32),
}

impl AgentEffortInput {
    fn as_label(&self) -> String {
        match self {
            Self::Name(name) => name.trim().to_string(),
            Self::Number(value) => value.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
struct AgentCompletedOutput {
    status: &'static str,
    #[serde(rename = "agentId")]
    agent_id: String,
    #[serde(rename = "agentType")]
    agent_type: String,
    description: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "teamName")]
    team_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "maxTurns")]
    max_turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    isolation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "worktreePath")]
    worktree_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "worktreeBranch")]
    worktree_branch: Option<String>,
    #[serde(rename = "toolUses")]
    tool_uses: usize,
    result: String,
}

#[derive(Debug, Serialize)]
struct AgentAsyncOutput {
    status: &'static str,
    #[serde(rename = "agentId")]
    agent_id: String,
    #[serde(rename = "agentType")]
    agent_type: String,
    description: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "teamName")]
    team_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "maxTurns")]
    max_turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    isolation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "worktreePath")]
    worktree_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "worktreeBranch")]
    worktree_branch: Option<String>,
    #[serde(rename = "outputFile")]
    output_file: String,
    #[serde(rename = "canReadOutputFile")]
    can_read_output_file: bool,
}

#[derive(Debug)]
struct AgentWorktree {
    repo_root: PathBuf,
    path: PathBuf,
    branch: String,
    preserve_on_completion: bool,
}

#[derive(Debug)]
struct PreparedAgentExecution {
    agent_id: String,
    agent_type: String,
    description: String,
    prompt: String,
    name: Option<String>,
    run_in_background: bool,
    nested_cwd: PathBuf,
    nested_state: AppState,
    nested_resources: LoadedResources,
    resolved_model: Option<String>,
    resolved_effort: Option<String>,
    isolation: Option<String>,
    team_name: Option<String>,
    mode: Option<String>,
    max_turns: Option<u32>,
    worktree: Option<AgentWorktree>,
}

const IMPLICIT_AGENT_DISALLOWED_TOOLS: &[&str] = &[
    "Agent",
    "TaskOutput",
    "EnterPlanMode",
    "ExitPlanMode",
    "AskUserQuestion",
    "TaskStop",
];

const ASYNC_AGENT_ALLOWED_TOOLS: &[&str] = &[
    "Bash",
    "PowerShell",
    "Read",
    "Edit",
    "Write",
    "NotebookEdit",
    "Glob",
    "Grep",
    "WebFetch",
    "WebSearch",
    "TodoWrite",
    "Skill",
    "ToolSearch",
    "EnterWorktree",
    "ExitWorktree",
];

/// Executes the runtime-backed `Agent` tool by running a nested model turn.
pub(super) fn execute_agent_tool(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let input: AgentToolInput = serde_json::from_value(input).context("invalid Agent input")?;
    if input.prompt.trim().is_empty() {
        bail!("Agent prompt cannot be empty");
    }
    if input.cwd.is_some() && input.isolation.as_deref() == Some("worktree") {
        bail!("agent cwd override is incompatible with isolation=worktree");
    }
    let requested_isolation = input.isolation.clone();
    if let Some(isolation) = requested_isolation
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if isolation != "worktree" {
            bail!("unsupported agent isolation `{isolation}`");
        }
    }
    if input.max_turns == Some(0) {
        bail!("agent max_turns must be greater than zero");
    }

    let prepared = prepare_agent_execution(state, resources, providers, cwd, input)?;
    if prepared.prompt.trim().is_empty() {
        bail!("Agent prompt cannot be empty");
    }
    if prepared.nested_state.current_provider.is_none() && providers.providers().next().is_none() {
        bail!("no providers are registered");
    }
    if let Some(team_name) = prepared.team_name.as_deref() {
        register_team_member(
            state.session.cwd.as_path(),
            team_name,
            ClaudeTeamMember {
                agent_id: prepared.agent_id.clone(),
                name: prepared
                    .name
                    .clone()
                    .unwrap_or_else(|| prepared.agent_id.clone()),
                agent_type: prepared.agent_type.clone(),
                joined_at: 0,
                model: prepared.resolved_model.clone(),
                cwd: prepared.nested_cwd.display().to_string(),
            },
        )?;
    }

    if prepared.run_in_background {
        return launch_background_agent(prepared, providers.clone(), auth_store.clone());
    }

    run_agent_synchronously(prepared, providers, auth_store)
}

fn prepare_agent_execution(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    cwd: &Path,
    input: AgentToolInput,
) -> Result<PreparedAgentExecution> {
    let current_resources = load_agent_resources(cwd, state.current_model.as_deref())
        .unwrap_or_else(|_| resources.clone());
    let selected_agent = input
        .subagent_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("general-purpose");
    let (agent_source, agent) = agent_by_id(&current_resources, selected_agent)
        .or_else(|| {
            current_resources
                .agents
                .iter()
                .find(|item| item.value.id.eq_ignore_ascii_case(selected_agent))
        })
        .map(|agent| (&current_resources, agent))
        .or_else(|| {
            agent_by_id(resources, selected_agent)
                .or_else(|| {
                    resources
                        .agents
                        .iter()
                        .find(|item| item.value.id.eq_ignore_ascii_case(selected_agent))
                })
                .map(|agent| (resources, agent))
        })
        .ok_or_else(|| {
            let available = current_resources
                .agents
                .iter()
                .map(|item| item.value.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow!("unknown agent `{selected_agent}`. Available agents: {available}")
        })?;
    ensure_required_mcp_servers(agent_source, &agent.value.required_mcp_servers)?;

    let nested_cwd = resolve_agent_cwd(cwd, input.cwd.as_deref())?;
    let nested_resources = filter_resources_for_agent(
        agent_source,
        &agent.value.tools,
        &agent.value.disallowed_tools,
    );
    let agent_id = format!("agent-{}", Uuid::new_v4().simple());
    let agent_name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let team_name = input
        .team_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let mut nested_state = state.clone();
    // Mint a fresh session id for the subagent and link back to the
    // parent via the canonical `SessionMetadata::parent_session_id`
    // field. The trace pipeline reads this to emit
    // `puffer.parent.session_id` + `puffer.subagent.kind=agent_tool`
    // on the subagent's root agent_loop span; Langfuse can then pivot
    // parent → spawned subagent traces via the link. Mirrors Codex's
    // `parent_task_id` model.
    nested_state.session.parent_session_id = Some(state.session.id);
    nested_state.session.id = Uuid::new_v4();
    let mut nested_cwd = nested_cwd;
    let mut worktree = None;
    let effective_isolation = input
        .isolation
        .clone()
        .or_else(|| agent.value.isolation.clone());
    if let Some(isolation) = effective_isolation
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if isolation != "worktree" {
            bail!("unsupported agent isolation `{isolation}`");
        }
    }
    if effective_isolation.as_deref() == Some("worktree") {
        let created = create_agent_worktree(cwd, &Uuid::new_v4().simple().to_string())?;
        nested_cwd = created.path.clone();
        worktree = Some(created);
    }
    nested_state.cwd = nested_cwd.clone();
    nested_state.transcript.clear();
    nested_state.push_message(
        MessageRole::System,
        build_agent_system_prompt(agent_source, &agent.value)?,
    );
    let effective_mode = input
        .mode
        .as_deref()
        .or(input.permission_mode.as_deref())
        .or(agent.value.permission_mode.as_deref());
    if effective_mode == Some("plan") {
        enter_plan_mode(&mut nested_state)?;
    }
    nested_state.active_team_name = team_name.clone();
    nested_state.set_current_actor(MessageActor {
        kind: MessageActorKind::Subagent,
        id: agent_id.clone(),
        agent_id: Some(agent_id.clone()),
        agent_type: Some(agent.value.id.clone()),
        name: agent_name.clone(),
        team_name: team_name.clone(),
        session_id: Some(nested_state.session.id),
        parent_session_id: Some(state.session.id),
    });
    if let Some(effort) = input
        .effort
        .as_ref()
        .map(AgentEffortInput::as_label)
        .or_else(|| agent.value.effort.clone())
        .filter(|value| !value.trim().is_empty())
    {
        nested_state.effort_level = effort;
    }

    if let Some(model) = input
        .model
        .as_deref()
        .or(agent.value.model.as_deref())
        .filter(|value| !value.trim().is_empty() && !value.eq_ignore_ascii_case("inherit"))
    {
        let resolved = providers
            .resolve_model(model)
            .or_else(|| resolve_model_case_insensitive(providers, model));
        let selector = resolved
            .as_ref()
            .map(|descriptor| format!("{}/{}", descriptor.provider, descriptor.id))
            .unwrap_or_else(|| model.to_string());
        nested_state.current_model = Some(selector.clone());
        nested_state.current_provider = resolved
            .map(|descriptor| descriptor.provider.clone())
            .or_else(|| {
                selector
                    .split_once('/')
                    .map(|(provider, _)| provider.to_string())
            })
            .or_else(|| state.current_provider.clone());
    }
    let prompt = combine_agent_prompt(
        input
            .initial_prompt
            .as_deref()
            .or(agent.value.initial_prompt.as_deref()),
        &input.prompt,
    );
    Ok(PreparedAgentExecution {
        agent_id,
        agent_type: agent.value.id.clone(),
        description: input.description.trim().to_string(),
        prompt,
        name: agent_name,
        run_in_background: input.run_in_background || agent.value.background,
        nested_cwd,
        resolved_model: nested_state.current_model.clone(),
        resolved_effort: Some(nested_state.effort_level.clone()),
        nested_state,
        nested_resources,
        isolation: effective_isolation,
        team_name,
        mode: effective_mode
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        max_turns: input.max_turns.or(agent.value.max_turns),
        worktree,
    })
}

fn run_agent_synchronously(
    mut prepared: PreparedAgentExecution,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
) -> Result<String> {
    let max_outer_turns = prepared.max_turns.unwrap_or(DEFAULT_AGENT_MAX_TURNS);
    let mut total_tool_uses = 0usize;
    let mut last_text = String::new();

    // Outer loop: each iteration calls execute_user_prompt which itself
    // runs up to 8 internal tool-use rounds. We re-invoke when the model
    // used tools in the previous turn (it may need more rounds).
    for outer in 0..max_outer_turns {
        let prompt = if outer == 0 {
            prepared.prompt.clone()
        } else {
            // Subsequent turns: feed the previous assistant output back so
            // the model can continue its chain of thought.
            format!("[Continue from previous turn]\n{last_text}")
        };
        let turn = super::execute_user_prompt(
            &mut prepared.nested_state,
            &prepared.nested_resources,
            providers,
            auth_store,
            &prompt,
        )?;
        total_tool_uses += turn.tool_invocations.len();
        last_text = turn.assistant_text.trim().to_string();

        // Stop when the model finished without calling any tools — it has
        // nothing more to do.
        if turn.tool_invocations.is_empty() {
            break;
        }
    }

    let payload = AgentCompletedOutput {
        status: "completed",
        agent_id: prepared.agent_id,
        agent_type: prepared.agent_type,
        description: prepared.description,
        prompt: prepared.prompt,
        name: prepared.name,
        cwd: prepared.nested_cwd.display().to_string(),
        model: prepared.resolved_model,
        effort: prepared.resolved_effort,
        team_name: prepared.team_name,
        mode: prepared.mode,
        max_turns: prepared.max_turns,
        isolation: prepared.isolation.clone(),
        worktree_path: prepared
            .worktree
            .as_ref()
            .map(|worktree| worktree.path.display().to_string()),
        worktree_branch: prepared
            .worktree
            .as_ref()
            .map(|worktree| worktree.branch.clone()),
        tool_uses: total_tool_uses,
        result: last_text,
    };
    if let Some(worktree) = prepared.worktree.take() {
        cleanup_agent_worktree(worktree)?;
    }
    Ok(serde_json::to_string_pretty(&payload)?)
}

const DEFAULT_AGENT_MAX_TURNS: u32 = 10;

fn launch_background_agent(
    mut prepared: PreparedAgentExecution,
    providers: ProviderRegistry,
    auth_store: AuthStore,
) -> Result<String> {
    use super::background_tasks::task_manager;

    if let Some(worktree) = prepared.worktree.as_mut() {
        worktree.preserve_on_completion = true;
    }
    let output_file = agent_output_path(&prepared.nested_state.session.cwd, &prepared.agent_id)?;

    // Check concurrent task limit before proceeding.
    if !task_manager().has_capacity() {
        bail!(
            "concurrent background task limit reached ({}). \
             Wait for existing tasks to complete before launching new agents.",
            task_manager().active_count()
        );
    }

    fs::write(
        &output_file,
        serde_json::to_string_pretty(&json!({
            "status": "running",
            "agentId": prepared.agent_id,
            "agentType": prepared.agent_type,
            "description": prepared.description,
            "prompt": prepared.prompt,
            "name": prepared.name,
            "cwd": prepared.nested_cwd.display().to_string(),
            "model": prepared.resolved_model,
            "effort": prepared.resolved_effort.clone(),
        }))?,
    )
    .with_context(|| format!("failed to initialize {}", output_file.display()))?;

    // Register with the centralized task manager for tracking and limit enforcement.
    let task_output_buf = task_manager()
        .register(
            &prepared.agent_id,
            &prepared.description,
            Some(&prepared.agent_id),
            Some(&output_file.display().to_string()),
            false, // not auto-backgrounded
        )
        .map_err(|err| anyhow!(err))?;

    let response = AgentAsyncOutput {
        status: "async_launched",
        agent_id: prepared.agent_id.clone(),
        agent_type: prepared.agent_type.clone(),
        description: prepared.description.clone(),
        prompt: prepared.prompt.clone(),
        name: prepared.name.clone(),
        cwd: prepared.nested_cwd.display().to_string(),
        model: prepared.resolved_model.clone(),
        effort: prepared.resolved_effort.clone(),
        team_name: prepared.team_name.clone(),
        mode: prepared.mode.clone(),
        max_turns: prepared.max_turns,
        isolation: prepared.isolation.clone(),
        worktree_path: prepared
            .worktree
            .as_ref()
            .map(|worktree| worktree.path.display().to_string()),
        worktree_branch: prepared
            .worktree
            .as_ref()
            .map(|worktree| worktree.branch.clone()),
        output_file: output_file.display().to_string(),
        can_read_output_file: true,
    };

    // If this agent belongs to a team, spawn as a multi-turn teammate loop.
    // Otherwise, spawn as a one-shot background agent.
    if prepared.team_name.is_some() {
        use super::teammate_loop::{spawn_teammate, teammate_registry, TeammateLoopConfig};
        let config = TeammateLoopConfig {
            agent_id: prepared.agent_id.clone(),
            agent_name: prepared
                .name
                .clone()
                .unwrap_or_else(|| prepared.agent_id.clone()),
            team_name: prepared.team_name.clone().unwrap_or_default(),
            prompt: prepared.prompt.clone(),
            max_turns: prepared.max_turns,
            state: prepared.nested_state,
            resources: prepared.nested_resources,
            providers,
            auth_store,
            output_file: output_file.clone(),
        };
        spawn_teammate(config, teammate_registry());
    } else {
        let notification_cwd = prepared.nested_state.session.cwd.clone();
        let agent_id_for_notify = prepared.agent_id.clone();
        let description_for_notify = prepared.description.clone();
        let actor_for_notify = prepared.nested_state.assistant_actor();

        thread::spawn(move || {
            let mut nested_state = prepared.nested_state;
            let nested_resources = prepared.nested_resources;
            let max_outer = prepared.max_turns.unwrap_or(DEFAULT_AGENT_MAX_TURNS);
            let mut total_tool_uses = 0usize;
            let mut last_text = String::new();
            let mut failed = false;

            for outer in 0..max_outer {
                let prompt = if outer == 0 {
                    prepared.prompt.clone()
                } else {
                    format!("[Continue from previous turn]\n{last_text}")
                };
                let result = {
                    let mut nested_auth_store = auth_store.clone();
                    super::execute_user_prompt(
                        &mut nested_state,
                        &nested_resources,
                        &providers,
                        &mut nested_auth_store,
                        &prompt,
                    )
                };
                match result {
                    Ok(turn) => {
                        total_tool_uses += turn.tool_invocations.len();
                        last_text = turn.assistant_text.trim().to_string();
                        // Stream turn output into the HeadTailBuffer for
                        // efficient capture (Codex-style head+tail preservation).
                        if let Ok(mut buf) = task_output_buf.lock() {
                            buf.write_str(&format!(
                                "--- Turn {} ---\n{}\nTool calls: {}\n\n",
                                outer + 1,
                                last_text,
                                turn.tool_invocations.len()
                            ));
                        }
                        if turn.tool_invocations.is_empty() {
                            break;
                        }
                    }
                    Err(error) => {
                        last_text = error.to_string();
                        if let Ok(mut buf) = task_output_buf.lock() {
                            buf.write_str(&format!(
                                "--- Turn {} ---\nError: {last_text}\n",
                                outer + 1
                            ));
                        }
                        failed = true;
                        break;
                    }
                }
            }

            // Mark task as completed/failed in the centralized manager.
            task_manager().complete(&prepared.agent_id, !failed);

            let final_payload = if failed {
                json!({
                    "status": "failed",
                    "agentId": prepared.agent_id,
                    "agentType": prepared.agent_type,
                    "description": prepared.description,
                    "prompt": prepared.prompt,
                    "name": prepared.name,
                    "cwd": prepared.nested_cwd.display().to_string(),
                    "model": prepared.resolved_model,
                    "effort": prepared.resolved_effort,
                    "teamName": prepared.team_name,
                    "mode": prepared.mode,
                    "maxTurns": prepared.max_turns,
                    "isolation": prepared.isolation,
                    "worktreePath": prepared
                        .worktree
                        .as_ref()
                        .map(|worktree| worktree.path.display().to_string()),
                    "worktreeBranch": prepared
                        .worktree
                        .as_ref()
                        .map(|worktree| worktree.branch.clone()),
                    "error": last_text,
                })
            } else {
                json!(AgentCompletedOutput {
                    status: "completed",
                    agent_id: prepared.agent_id,
                    agent_type: prepared.agent_type,
                    description: prepared.description,
                    prompt: prepared.prompt,
                    name: prepared.name,
                    cwd: prepared.nested_cwd.display().to_string(),
                    model: prepared.resolved_model,
                    effort: prepared.resolved_effort,
                    team_name: prepared.team_name,
                    mode: prepared.mode,
                    max_turns: prepared.max_turns,
                    isolation: prepared.isolation,
                    worktree_path: prepared
                        .worktree
                        .as_ref()
                        .map(|worktree| worktree.path.display().to_string()),
                    worktree_branch: prepared
                        .worktree
                        .as_ref()
                        .map(|worktree| worktree.branch.clone()),
                    tool_uses: total_tool_uses,
                    result: last_text.clone(),
                })
            };
            let _ = fs::write(
                &output_file,
                serde_json::to_string_pretty(&final_payload)
                    .unwrap_or_else(|_| "{\"status\":\"failed\"}".to_string()),
            );

            // Notify leader: write a completion notification to the
            // messages store so TaskOutput / TaskList can detect it.
            write_agent_completion_notification(
                &notification_cwd,
                &agent_id_for_notify,
                &description_for_notify,
                if failed { "failed" } else { "completed" },
                &last_text,
                actor_for_notify,
            );

            // Periodic cleanup of old completed tasks to prevent unbounded growth.
            task_manager().cleanup_older_than(std::time::Duration::from_secs(3600));
        });
    }

    Ok(serde_json::to_string_pretty(&response)?)
}

/// Writes a completion notification to the messages store so the leader
/// can detect that a background agent has finished.
fn write_agent_completion_notification(
    cwd: &Path,
    agent_id: &str,
    description: &str,
    status: &str,
    result_summary: &str,
    actor: MessageActor,
) {
    use crate::runtime::claude_tools::workflow::store::{
        load_store, messages_path, now_ms, save_store, MessageStore, StoredMessage,
    };
    let Ok(mut store) = load_store::<MessageStore>(&messages_path(cwd)) else {
        return;
    };
    let preview = if result_summary.chars().count() > 200 {
        let truncated: String = result_summary.chars().take(200).collect();
        format!("{truncated}...")
    } else {
        result_summary.to_string()
    };
    store.messages.push(StoredMessage {
        id: format!("notify-{}", Uuid::new_v4().simple()),
        to: "team-lead".to_string(),
        from: agent_id.to_string(),
        read: false,
        summary: Some(format!("agent {status}: {description}")),
        message: json!({
            "type": "agent_completion",
            "agent_id": agent_id,
            "status": status,
            "description": description,
            "result": preview,
        }),
        actor: Some(actor),
        created_at_ms: now_ms(),
    });
    let _ = save_store(&messages_path(cwd), &store);
}

fn resolve_agent_cwd(parent_cwd: &Path, override_cwd: Option<&str>) -> Result<PathBuf> {
    let Some(override_cwd) = override_cwd.filter(|value| !value.trim().is_empty()) else {
        return Ok(parent_cwd.to_path_buf());
    };
    let requested = PathBuf::from(override_cwd.trim());
    let resolved = if requested.is_absolute() {
        requested
    } else {
        parent_cwd.join(requested)
    };
    let metadata = std::fs::metadata(&resolved)
        .with_context(|| format!("agent cwd {} does not exist", resolved.display()))?;
    if !metadata.is_dir() {
        bail!("agent cwd {} is not a directory", resolved.display());
    }
    Ok(resolved)
}

fn create_agent_worktree(parent_cwd: &Path, suffix: &str) -> Result<AgentWorktree> {
    let repo_root = git_toplevel(parent_cwd)
        .ok_or_else(|| anyhow!("agent worktree isolation requires a git repository"))?;
    let worktree_root = repo_root.join(".worktree").join("agents");
    fs::create_dir_all(&worktree_root)
        .with_context(|| format!("failed to create {}", worktree_root.display()))?;
    let branch = format!("puffer-agent-{suffix}");
    let path = worktree_root.join(suffix);
    let status = Command::new("git")
        .args([
            "-C",
            repo_root.to_string_lossy().as_ref(),
            "worktree",
            "add",
            "-b",
            &branch,
            path.to_string_lossy().as_ref(),
        ])
        .status()
        .with_context(|| format!("failed to launch git worktree add for {}", path.display()))?;
    if !status.success() {
        bail!("git worktree add failed for {}", path.display());
    }
    Ok(AgentWorktree {
        repo_root,
        path,
        branch,
        preserve_on_completion: false,
    })
}

fn cleanup_agent_worktree(worktree: AgentWorktree) -> Result<()> {
    if worktree.preserve_on_completion {
        return Ok(());
    }
    let status = Command::new("git")
        .args([
            "-C",
            worktree.path.to_string_lossy().as_ref(),
            "status",
            "--porcelain",
        ])
        .output()
        .with_context(|| format!("failed to inspect {}", worktree.path.display()))?;
    if !status.status.success() || !String::from_utf8_lossy(&status.stdout).trim().is_empty() {
        return Ok(());
    }
    let remove = Command::new("git")
        .args([
            "-C",
            worktree.repo_root.to_string_lossy().as_ref(),
            "worktree",
            "remove",
            "--force",
            worktree.path.to_string_lossy().as_ref(),
        ])
        .status()
        .with_context(|| format!("failed to remove {}", worktree.path.display()))?;
    if !remove.success() {
        bail!("git worktree remove failed for {}", worktree.path.display());
    }
    let _ = Command::new("git")
        .args([
            "-C",
            worktree.repo_root.to_string_lossy().as_ref(),
            "branch",
            "-D",
            &worktree.branch,
        ])
        .status();
    Ok(())
}

fn git_toplevel(cwd: &Path) -> Option<PathBuf> {
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

fn agent_output_path(session_cwd: &Path, agent_id: &str) -> Result<PathBuf> {
    let paths = ConfigPaths::discover(session_cwd);
    ensure_workspace_dirs(&paths)?;
    let dir = paths
        .workspace_config_dir
        .join("runtime")
        .join("agent_outputs");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir.join(format!("{agent_id}.json")))
}

fn filter_resources_for_agent(
    resources: &LoadedResources,
    tools: &[String],
    disallowed_tools: &[String],
) -> LoadedResources {
    let mut filtered = resources.clone();
    let wildcard = tools.is_empty() || tools.iter().any(|tool| tool == "*");
    filtered.tools.retain(|tool| {
        if IMPLICIT_AGENT_DISALLOWED_TOOLS
            .iter()
            .any(|blocked| tool_matches_selector(&tool.value, blocked))
        {
            return false;
        }
        if disallowed_tools
            .iter()
            .any(|blocked| tool_matches_selector(&tool.value, blocked))
        {
            return false;
        }
        if !ASYNC_AGENT_ALLOWED_TOOLS
            .iter()
            .any(|allowed| tool_matches_selector(&tool.value, allowed))
        {
            return false;
        }
        wildcard
            || tools
                .iter()
                .any(|allowed| tool_matches_selector(&tool.value, allowed))
    });
    filtered
}

fn tool_matches_selector(tool: &ToolSpec, selector: &str) -> bool {
    tool_spec_matches_selector(tool, selector)
}

fn build_agent_system_prompt(resources: &LoadedResources, agent: &AgentSpec) -> Result<String> {
    let mut sections = vec![agent.prompt.trim().to_string()];
    for skill_name in &agent.skills {
        let Some(skill) = skill_by_name(resources, skill_name) else {
            bail!(
                "agent `{}` references unknown skill `{skill_name}`",
                agent.id
            );
        };
        sections.push(format!(
            "<skill name=\"{}\">\n{}\n</skill>",
            skill.value.name,
            skill.value.content.trim()
        ));
    }
    Ok(sections.join("\n\n"))
}

fn combine_agent_prompt(initial_prompt: Option<&str>, prompt: &str) -> String {
    if let Some(initial_prompt) = initial_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        format!("{initial_prompt}\n\n{}", prompt.trim())
    } else {
        prompt.to_string()
    }
}

fn ensure_required_mcp_servers(resources: &LoadedResources, required: &[String]) -> Result<()> {
    if required.is_empty() {
        return Ok(());
    }
    let available = resources
        .mcp_servers
        .iter()
        .map(|server| server.value.id.to_ascii_lowercase())
        .chain(
            plugin_mcp_servers(resources)
                .into_iter()
                .map(|(_, server)| server.id.to_ascii_lowercase()),
        )
        .collect::<Vec<_>>();
    let missing = required
        .iter()
        .filter(|pattern| {
            let normalized = pattern.trim().to_ascii_lowercase();
            !normalized.is_empty()
                && !available
                    .iter()
                    .any(|candidate| candidate.contains(normalized.as_str()))
        })
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        bail!(
            "required MCP servers are unavailable for this agent: {}",
            missing.join(", ")
        )
    }
}

fn resolve_model_case_insensitive<'a>(
    providers: &'a ProviderRegistry,
    selector: &str,
) -> Option<&'a puffer_provider_registry::ModelDescriptor> {
    providers.providers().find_map(|provider| {
        provider.models.iter().find(|model| {
            format!("{}/{}", model.provider, model.id).eq_ignore_ascii_case(selector)
                || model.id.eq_ignore_ascii_case(selector)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::filter_resources_for_agent;
    use puffer_resources::{LoadedItem, LoadedResources, SourceInfo, SourceKind, ToolSpec};
    use std::path::PathBuf;

    fn tool(id: &str, handler: &str) -> LoadedItem<ToolSpec> {
        LoadedItem {
            value: ToolSpec {
                id: id.to_string(),
                name: id.to_string(),
                description: id.to_string(),
                handler: handler.to_string(),
                aliases: Vec::new(),
                handler_args: Vec::new(),
                approval_policy: None,
                sandbox_policy: None,
                shared_lib: None,
                enabled_if: None,
                input_schema: None,
                metadata: Default::default(),
                display: Default::default(),
            },
            source_info: SourceInfo {
                path: PathBuf::from(format!("{id}.yaml")),
                kind: SourceKind::Builtin,
            },
        }
    }

    #[test]
    fn wildcard_agents_only_receive_async_safe_tool_pool() {
        let resources = LoadedResources {
            tools: vec![
                tool("Agent", "runtime:agent"),
                tool("Bash", "runtime:claude_bash"),
                tool("Read", "runtime:claude_read"),
                tool("Edit", "runtime:claude_edit"),
                tool("Config", "runtime:workflow:config"),
                tool("TaskCreate", "runtime:workflow:task_create"),
                tool("TaskStop", "runtime:workflow:task_stop"),
            ],
            ..LoadedResources::default()
        };

        let filtered = filter_resources_for_agent(&resources, &[], &[]);
        let ids = filtered
            .tools
            .iter()
            .map(|tool| tool.value.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["Bash", "Read", "Edit"]);
    }

    #[test]
    fn agent_tool_filters_accept_legacy_lowercase_aliases() {
        let resources = LoadedResources {
            tools: vec![
                tool("Read", "runtime:claude_read"),
                tool("Glob", "runtime:claude_glob"),
                tool("Grep", "runtime:claude_grep"),
                tool("Write", "runtime:claude_write"),
            ],
            ..LoadedResources::default()
        };

        let filtered = filter_resources_for_agent(
            &resources,
            &[
                "read_file".to_string(),
                "list_dir".to_string(),
                "search_text".to_string(),
            ],
            &[],
        );
        let ids = filtered
            .tools
            .iter()
            .map(|tool| tool.value.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["Read", "Glob", "Grep"]);
    }
}
