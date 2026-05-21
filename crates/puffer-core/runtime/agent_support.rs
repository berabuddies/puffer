use crate::tool_names::tool_spec_matches_selector;
use crate::{AppState, MessageRole};
use anyhow::{bail, Result};
use puffer_provider_registry::ProviderRegistry;
use puffer_resources::{plugin_mcp_servers, skill_by_name, AgentSpec, LoadedResources, ToolSpec};

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

/// Returns the effective team name for one agent invocation.
pub(crate) fn resolve_effective_team_name(
    state: &AppState,
    requested: Option<&str>,
    spawn_name: Option<&str>,
) -> Option<String> {
    requested
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            spawn_name
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .and(state.active_team_name.clone())
        })
}

/// Returns true when the invocation is happening inside a nested in-process teammate/subagent.
pub(crate) fn is_in_process_teammate_context(state: &AppState) -> bool {
    state.active_team_name.is_some()
        && state.transcript.first().is_some_and(|message| {
            message.role == MessageRole::System
                && (message.text.contains("You are a coding subagent")
                    || message.text.contains("You are a verification specialist"))
        })
}

/// Filters the loaded resource pool down to the tools available to one agent.
pub(crate) fn filter_resources_for_agent(
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

/// Returns true when one tool matches a selector or alias.
pub(crate) fn tool_matches_selector(tool: &ToolSpec, selector: &str) -> bool {
    tool_spec_matches_selector(tool, selector)
}

/// Builds the system prompt for a spawned agent, including preloaded skills.
pub(crate) fn build_agent_system_prompt(
    resources: &LoadedResources,
    agent: &AgentSpec,
) -> Result<String> {
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

/// Combines an optional initial prompt prefix with the delegated prompt body.
pub(crate) fn combine_agent_prompt(initial_prompt: Option<&str>, prompt: &str) -> String {
    if let Some(initial_prompt) = initial_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        format!("{initial_prompt}\n\n{}", prompt.trim())
    } else {
        prompt.to_string()
    }
}

/// Ensures that all MCP servers required by the selected agent are available.
pub(crate) fn ensure_required_mcp_servers(
    resources: &LoadedResources,
    required: &[String],
) -> Result<()> {
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

/// Resolves a provider/model selector while tolerating case differences.
pub(crate) fn resolve_model_case_insensitive<'a>(
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
    use super::{
        filter_resources_for_agent, is_in_process_teammate_context, resolve_effective_team_name,
    };
    use crate::state::{AppState, MessageRole};
    use puffer_config::PufferConfig;
    use puffer_resources::{LoadedItem, LoadedResources, SourceInfo, SourceKind, ToolSpec};
    use puffer_session_store::SessionMetadata;
    use std::path::PathBuf;
    use uuid::Uuid;

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

    fn state() -> AppState {
        AppState::new(
            PufferConfig::default(),
            PathBuf::from("/tmp"),
            SessionMetadata {
                id: Uuid::new_v4(),
                display_name: None,
                generated_title: None,
                cwd: PathBuf::from("/tmp"),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        )
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

    #[test]
    fn resolve_effective_team_name_inherits_current_team_for_named_spawn() {
        let mut state = state();
        state.active_team_name = Some("alpha".to_string());
        assert_eq!(
            resolve_effective_team_name(&state, None, Some("researcher")),
            Some("alpha".to_string())
        );
    }

    #[test]
    fn resolve_effective_team_name_ignores_empty_values() {
        let state = state();
        assert_eq!(
            resolve_effective_team_name(&state, Some("  "), Some(" ")),
            None
        );
    }

    #[test]
    fn nested_agent_context_is_not_required_for_team_name_resolution() {
        let mut state = state();
        state.active_team_name = Some("alpha".to_string());
        state.push_message(MessageRole::System, "system note");
        assert_eq!(
            resolve_effective_team_name(&state, Some("beta"), Some("researcher")),
            Some("beta".to_string())
        );
    }

    #[test]
    fn teammate_context_detection_ignores_generic_main_thread_system_messages() {
        let mut state = state();
        state.active_team_name = Some("alpha".to_string());
        state.push_message(MessageRole::System, "Team created.");
        assert!(!is_in_process_teammate_context(&state));

        state.transcript.clear();
        state.push_message(MessageRole::System, "You are a coding subagent.");
        assert!(is_in_process_teammate_context(&state));
    }
}
