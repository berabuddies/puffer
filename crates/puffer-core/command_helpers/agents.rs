use super::emit_system;
use crate::agent_catalog::{
    load_agent_catalog_for_runtime, workspace_agent_manifest_path, AgentCatalogEntry,
};
use crate::AppState;
use anyhow::Result;
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_resources::SourceKind;
use puffer_session_store::SessionStore;
use std::fmt::Write as _;
use std::fs;

/// Handles `/agents` by listing and selecting declarative agent resources.
pub(crate) fn handle_agents_command(
    state: &mut AppState,
    session_store: &SessionStore,
    args: &str,
) -> Result<()> {
    let remote_tool_runner = state.config.remote_tool_runner.as_ref();
    let remote_tool_runner_enabled = remote_tool_runner.is_some();
    let paths = ConfigPaths::discover(&state.cwd);
    let agents_dir = paths.workspace_config_dir.join("resources/agents");
    if !remote_tool_runner_enabled {
        ensure_workspace_dirs(&paths)?;
        fs::create_dir_all(&agents_dir)?;
    }
    let workspace_manifest = workspace_agent_manifest_path(&paths);
    let agents = load_agent_catalog_for_runtime(
        &state.cwd,
        state.current_model.as_deref(),
        remote_tool_runner,
    )?;
    let trimmed = args.trim();

    match trimmed {
        "" | "show" => emit_system(
            state,
            session_store,
            render_agents_summary(
                &agents_dir.display().to_string(),
                &workspace_manifest.display().to_string(),
                &agents,
            ),
        ),
        "list" => emit_system(state, session_store, render_agents_listing(&agents)),
        "path" => emit_system(
            state,
            session_store,
            format!(
                "Agents directory: {}\nWorkspace agent manifest: {}",
                agents_dir.display(),
                workspace_manifest.display()
            ),
        ),
        _ if trimmed.starts_with("show ") => {
            let agent_id = trimmed.trim_start_matches("show ").trim();
            if let Some(agent) = find_agent(&agents, agent_id) {
                emit_system(state, session_store, render_agent_details(agent))
            } else {
                emit_system(state, session_store, format!("Unknown agent `{agent_id}`."))
            }
        }
        _ if trimmed.starts_with("use ") => {
            let agent_id = trimmed.trim_start_matches("use ").trim();
            if let Some(agent) = find_agent(&agents, agent_id) {
                if let Some(model) = agent.model.as_deref() {
                    state.current_model = Some(model.to_string());
                    if let Some((provider, _)) = model.split_once('/') {
                        state.current_provider = Some(provider.to_string());
                    }
                }
                emit_system(state, session_store, render_agent_selection(agent, state))
            } else {
                emit_system(state, session_store, format!("Unknown agent `{agent_id}`."))
            }
        }
        _ => emit_system(
            state,
            session_store,
            "Usage: /agents [show|list|path|show <id>|use <id>]".to_string(),
        ),
    }
}

fn find_agent<'a>(
    agents: &'a [AgentCatalogEntry],
    selector: &str,
) -> Option<&'a AgentCatalogEntry> {
    agents
        .iter()
        .find(|agent| agent.selector == selector)
        .or_else(|| {
            agents
                .iter()
                .find(|agent| agent.selector.eq_ignore_ascii_case(selector))
        })
}

fn render_agents_summary(
    agents_dir: &str,
    workspace_manifest: &str,
    agents: &[AgentCatalogEntry],
) -> String {
    format!(
        "Agents directory: {agents_dir}\nworkspace_agent_manifest={workspace_manifest}\nloaded_agents={}\n{}\nUse `/agents show <id>` or `/agents use <id>` to inspect or select an agent.",
        agents.len(),
        render_agents_listing(agents)
    )
}

fn render_agents_listing(agents: &[AgentCatalogEntry]) -> String {
    if agents.is_empty() {
        return "No agents are configured.".to_string();
    }
    let mut text = String::from("Agents:\n");
    for agent in agents {
        let _ = writeln!(
            &mut text,
            "- {} [{}] model={} tools={} skills={} {}",
            agent.selector,
            source_kind_label(agent.source_kind),
            agent_model_label(agent),
            agent_tools_label(agent),
            agent_skills_label(agent),
            agent.description.trim()
        );
    }
    text.trim_end().to_string()
}

fn render_agent_details(agent: &AgentCatalogEntry) -> String {
    format!(
        "Agent {}\nSource: {} ({})\nDescription: {}\nModel: {}\nEffort: {}\nPermission mode: {}\nMax turns: {}\nBackground: {}\nIsolation: {}\nMemory: {}\nTools: {}\nDisallowed tools: {}\nSkills: {}\nRequired MCP servers: {}\n\nPrompt:\n{}",
        agent.selector,
        source_kind_label(agent.source_kind),
        agent.source_path.display(),
        agent.description.trim(),
        agent_model_label(agent),
        agent.effort.as_deref().unwrap_or("<inherit>"),
        agent.permission_mode.as_deref().unwrap_or("<inherit>"),
        agent
            .max_turns
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<inherit>".to_string()),
        agent.background,
        agent.isolation.as_deref().unwrap_or("<inherit>"),
        agent.memory.as_deref().unwrap_or("<none>"),
        agent_tools_label(agent),
        list_or_placeholder(&agent.disallowed_tools),
        agent_skills_label(agent),
        list_or_placeholder(&agent.required_mcp_servers),
        agent.prompt.trim()
    )
}

fn render_agent_selection(agent: &AgentCatalogEntry, state: &AppState) -> String {
    let model_line = if let Some(model) = agent.model.as_deref() {
        format!("Active model set to {model}.")
    } else {
        format!(
            "Agent inherits the current session model ({}).",
            state.current_model.as_deref().unwrap_or("<unset>")
        )
    };
    format!(
        "Selected agent {}.\nSource: {} ({})\nDescription: {}\nModel: {}\n{}",
        agent.selector,
        source_kind_label(agent.source_kind),
        agent.source_path.display(),
        agent.description.trim(),
        agent_model_label(agent),
        model_line
    )
}

fn agent_model_label(agent: &AgentCatalogEntry) -> &str {
    agent.model.as_deref().unwrap_or("<inherit>")
}

fn agent_tools_label(agent: &AgentCatalogEntry) -> String {
    if agent.tools.is_empty() {
        "<inherit>".to_string()
    } else {
        agent.tools.join(", ")
    }
}

fn agent_skills_label(agent: &AgentCatalogEntry) -> String {
    list_or_placeholder(&agent.skills)
}

fn list_or_placeholder(values: &[String]) -> String {
    if values.is_empty() {
        "<none>".to_string()
    } else {
        values.join(", ")
    }
}

fn source_kind_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Builtin => "builtin",
        SourceKind::User => "user",
        SourceKind::Workspace => "workspace",
    }
}
