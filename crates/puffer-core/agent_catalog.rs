use crate::runner_adapter::LocalToolRunner;
use anyhow::Result;
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_resources::{load_resources, plugin_mcp_servers, LoadedResources, SourceKind};
use std::fs;
use std::path::{Path, PathBuf};

/// Describes one agent visible to the current workspace and UI pickers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCatalogEntry {
    pub selector: String,
    pub description: String,
    pub prompt: String,
    pub tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub skills: Vec<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub max_turns: Option<u32>,
    pub initial_prompt: Option<String>,
    pub background: bool,
    pub memory: Option<String>,
    pub required_mcp_servers: Vec<String>,
    pub isolation: Option<String>,
    pub source_kind: SourceKind,
    pub source_path: PathBuf,
}

/// Loads the current agent catalog from declarative resource files on disk.
pub fn load_agent_catalog(
    cwd: &Path,
    current_model: Option<&str>,
) -> Result<Vec<AgentCatalogEntry>> {
    let resources = load_agent_resources(cwd, current_model)?;
    let available_mcp_servers = available_mcp_server_names(&resources);
    let mut agents = resources
        .agents
        .iter()
        .filter(|item| {
            agent_has_required_mcp_servers(&available_mcp_servers, &item.value.required_mcp_servers)
        })
        .map(|item| AgentCatalogEntry {
            selector: item.value.id.clone(),
            description: item.value.description.clone(),
            prompt: item.value.prompt.clone(),
            tools: item.value.tools.clone(),
            disallowed_tools: item.value.disallowed_tools.clone(),
            skills: item.value.skills.clone(),
            model: item.value.model.clone(),
            effort: item.value.effort.clone(),
            permission_mode: item.value.permission_mode.clone(),
            max_turns: item.value.max_turns,
            initial_prompt: item.value.initial_prompt.clone(),
            background: item.value.background,
            memory: item
                .value
                .memory
                .as_ref()
                .map(|scope| format!("{scope:?}").to_ascii_lowercase()),
            required_mcp_servers: item.value.required_mcp_servers.clone(),
            isolation: item.value.isolation.clone(),
            source_kind: item.source_info.kind,
            source_path: item.source_info.path.clone(),
        })
        .collect::<Vec<_>>();
    agents.sort_by(|left, right| {
        left.selector
            .to_ascii_lowercase()
            .cmp(&right.selector.to_ascii_lowercase())
            .then_with(|| left.source_path.cmp(&right.source_path))
    });
    Ok(agents)
}

pub(crate) fn load_agent_resources(
    cwd: &Path,
    current_model: Option<&str>,
) -> Result<LoadedResources> {
    let paths = ConfigPaths::discover(cwd);
    ensure_workspace_dirs(&paths)?;
    let agents_dir = workspace_agents_dir(&paths);
    fs::create_dir_all(&agents_dir)?;
    let workspace_manifest = workspace_agent_manifest_path(&paths);
    if !workspace_manifest.exists() {
        fs::write(&workspace_manifest, default_agent_manifest(current_model))?;
    }
    load_resources(&paths, &LocalToolRunner::new())
}

pub(crate) fn workspace_agent_manifest_path(paths: &ConfigPaths) -> PathBuf {
    workspace_agents_dir(paths).join("workspace.yaml")
}

fn workspace_agents_dir(paths: &ConfigPaths) -> PathBuf {
    paths.workspace_config_dir.join("resources/agents")
}

fn default_agent_manifest(model: Option<&str>) -> String {
    format!(
        "id: default\n\
description: Workspace coding agent preset.\n\
prompt: \"You are a coding subagent for Puffer Code.\\nWork directly on the task you are given. Use the available tools to inspect, edit, and verify code.\\nKeep your final response concise and factual. Include the outcome, key files touched, and any verification you ran.\"\n\
tools:\n\
  - \"*\"\n\
disallowed_tools:\n\
  - Agent\n\
model: {}\n",
        model.unwrap_or("anthropic/claude-sonnet-4-5")
    )
}

fn available_mcp_server_names(resources: &LoadedResources) -> Vec<String> {
    let mut available = resources
        .mcp_servers
        .iter()
        .map(|server| server.value.id.to_ascii_lowercase())
        .collect::<Vec<_>>();
    available.extend(
        plugin_mcp_servers(resources)
            .into_iter()
            .map(|(_, server)| server.id.to_ascii_lowercase()),
    );
    available
}

fn agent_has_required_mcp_servers(available: &[String], patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return true;
    }
    patterns.iter().all(|pattern| {
        let pattern = pattern.to_ascii_lowercase();
        available
            .iter()
            .any(|candidate| candidate.contains(&pattern))
    })
}
