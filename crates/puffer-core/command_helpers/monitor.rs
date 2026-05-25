use crate::runtime::claude_tools::workflow::workflow_tools;
use crate::runtime::subscription_manager;
use crate::AppState;
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Creates monitor workflows for one or more connector connections.
pub(crate) fn handle_monitor_command(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    args: &str,
) -> Result<String> {
    let connections = parse_connection_args(args);
    if connections.is_empty() {
        return Ok("Usage: /monitor <connection> [connection ...]".to_string());
    }
    let mut lines = Vec::new();
    for connection_slug in connections {
        match create_monitor(state, resources, providers, auth_store, &connection_slug) {
            Ok(line) => lines.push(line),
            Err(error) => lines.push(format!("{connection_slug}: failed: {error:#}")),
        }
    }
    Ok(format!("Monitor setup\n{}", lines.join("\n")))
}

fn create_monitor(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    connection_slug: &str,
) -> Result<String> {
    let manager = subscription_manager()?;
    let connection = manager
        .connection_store()
        .get(connection_slug)
        .ok_or_else(|| anyhow::anyhow!("connection `{connection_slug}` not found"))?;
    let template = manager
        .connector_store()
        .get(&connection.connector_slug)
        .ok_or_else(|| anyhow::anyhow!("connector `{}` not found", connection.connector_slug))?;
    let monitor_slug = monitor_slug(connection_slug);
    let memory_path = monitor_memory_path(&state.cwd, connection_slug)?;
    ensure_monitor_memory(&memory_path, connection_slug, &connection.connector_slug)?;
    let action_prompt = monitor_triage_prompt(
        connection_slug,
        &connection.connector_slug,
        &template.description,
        &memory_path,
    );
    let cwd = state.cwd.clone();
    if manager.store().get(&monitor_slug).is_none() {
        let raw = workflow_tools::execute_workflow_create(
            state,
            &cwd,
            json!({
                "slug": monitor_slug,
                "description": format!("Monitor {} for actionable tasks", connection_slug),
                "connection_slug": connection_slug,
                "action": {
                    "type": "triage_agent",
                    "prompt": action_prompt
                },
                "enabled": true
            }),
        )
        .with_context(|| format!("failed to create workflow `{monitor_slug}`"))?;
        let _: Value = serde_json::from_str(&raw).context("invalid WorkflowCreate output")?;
    } else {
        let _ = workflow_tools::execute_workflow_toggle(
            state,
            &cwd,
            json!({
                "slug": monitor_slug,
                "enabled": true
            }),
        )?;
    }
    let backfill = maybe_spawn_backfill_agent(
        state,
        resources,
        providers,
        auth_store,
        connection_slug,
        &connection.connector_slug,
        &memory_path,
    );
    let backfill = match backfill {
        Ok(Some(agent_id)) => format!("backfill_agent={agent_id}"),
        Ok(None) => "backfill_agent=<not needed>".to_string(),
        Err(error) => format!("backfill_agent=<not launched: {error}>"),
    };
    Ok(format!(
        "{}: workflow={} memory={} {}",
        connection_slug,
        monitor_slug,
        memory_path.display(),
        backfill
    ))
}

fn parse_connection_args(args: &str) -> Vec<String> {
    args.split(|ch: char| ch.is_whitespace() || ch == ',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn monitor_slug(connection_slug: &str) -> String {
    format!("monitor-{connection_slug}")
}

fn monitor_memory_path(cwd: &Path, connection_slug: &str) -> Result<PathBuf> {
    let paths = ConfigPaths::discover(cwd);
    let dir = paths.workspace_config_dir.join("runtime").join("monitors");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir.join(format!("{connection_slug}.md")))
}

fn ensure_monitor_memory(path: &Path, connection_slug: &str, connector_slug: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(
        path,
        format!(
            "# Monitor Memory: {connection_slug}\n\nConnector: {connector_slug}\n\nAdd ignore rules and examples below. Monitor triage must read this file before creating tasks.\n"
        ),
    )
    .with_context(|| format!("failed to initialize {}", path.display()))
}

fn monitor_triage_prompt(
    connection_slug: &str,
    connector_slug: &str,
    connector_description: &str,
    memory_path: &Path,
) -> String {
    format!(
        "You are the background monitor triage agent for connection `{connection_slug}` ({connector_slug}). Connector description: {connector_description}\n\nFor every new connector event:\n1. Read `{}` if it exists and apply its ignore rules.\n2. If the event matches ignore memory, do not create a task; briefly report that it was ignored.\n3. Otherwise decide whether the event represents an ongoing actionable task. Telegram monitoring handles every non-ignored message. Email monitoring handles every message. Slack monitoring handles every message visible to the connection.\n4. Use TaskList first to avoid duplicates. Use TaskCreate for new tasks and TaskUpdate for materially changed existing monitor tasks.\n5. Every monitor TaskCreate MUST include metadata with `_monitor: true`, `monitor_connection: \"{connection_slug}\"`, `monitor_connector: \"{connector_slug}\"`, and `monitor_memory_path: \"{}\"`.\n6. Every monitor TaskCreate SHOULD include `actions`: an array of objects with `actionName` and `actionPrompt`, and `possibleIgnoreReasons`: a short array of suggested ignore reasons.\n7. Keep action prompts ready to send to the current coding agent. Include enough source context from the connector event for the agent to act without rereading the whole stream.\n\nDo not send connector replies unless a selected action later asks for it.",
        memory_path.display(),
        memory_path.display()
    )
}

fn maybe_spawn_backfill_agent(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    connection_slug: &str,
    connector_slug: &str,
    memory_path: &Path,
) -> Result<Option<String>> {
    if !connector_slug.contains("telegram") && !connector_slug.contains("slack") {
        return Ok(None);
    }
    let backfill_hint = if connector_slug.contains("telegram") {
        "Use Telegram list_peers and list_messages with the exact connection_slug named above and succinct=true to inspect recent messages."
    } else {
        "Use Slack list_conversations and read_messages with the exact connection_slug named above to inspect recent messages."
    };
    let prompt = format!(
        "Backfill monitor tasks for connection `{connection_slug}` ({connector_slug}). Every connector lookup tool call MUST pass `connection_slug: \"{connection_slug}\"`.\n\n{backfill_hint}\n\nRead monitor memory `{}` first and skip ignored examples. For actionable current work, create monitor tasks using TaskCreate with metadata `_monitor: true`, `monitor_connection: \"{connection_slug}\"`, `monitor_connector: \"{connector_slug}\"`, and `monitor_memory_path: \"{}\"`. Include `actions` and `possibleIgnoreReasons` for each task. Avoid duplicates by calling TaskList first.",
        memory_path.display(),
        memory_path.display()
    );
    let output = crate::runtime::execute_agent_tool_once(
        state,
        resources,
        providers,
        auth_store,
        &state.cwd,
        json!({
            "description": "Backfill monitor tasks",
            "prompt": prompt,
            "subagent_type": "general-purpose",
            "name": format!("monitor-backfill-{connection_slug}"),
            "run_in_background": true,
            "max_turns": 6
        }),
    )?;
    let payload: Value = serde_json::from_str(&output).unwrap_or_else(|_| json!({}));
    Ok(payload
        .get("agentId")
        .or_else(|| payload.get("agent_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connection_args_accepts_spaces_and_commas() {
        assert_eq!(
            parse_connection_args("telegram-user, slack-login team-email"),
            vec!["telegram-user", "slack-login", "team-email"]
        );
    }

    #[test]
    fn monitor_prompt_requires_monitor_task_metadata() {
        let prompt = monitor_triage_prompt(
            "telegram-user",
            "telegram-login",
            "Telegram",
            Path::new("/tmp/memory.md"),
        );
        assert!(prompt.contains("TaskCreate MUST include metadata"));
        assert!(prompt.contains("possibleIgnoreReasons"));
        assert!(prompt.contains("Telegram monitoring handles every non-ignored message"));
    }
}
