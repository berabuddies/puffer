use crate::runtime::claude_tools::{execute_workflow_tool, workflow::workflow_tools};
use crate::runtime::subscription_manager;
use crate::{AppState, TurnExecution};
use anyhow::{anyhow, bail, Context, Result};
use puffer_config::ConfigPaths;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_subscriptions::{
    connection_workflow_trigger_supported, ConnectionRecord, ConnectorTemplate,
    SubscriberManifestRoots,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;
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
    let connections = resolve_monitor_connections(state, resources, args)?;
    let mut lines = Vec::new();
    for connection_slug in connections {
        match create_monitor(state, resources, providers, auth_store, &connection_slug) {
            Ok(line) => lines.push(line),
            Err(error) => lines.push(format!("{connection_slug}: failed: {error:#}")),
        }
    }
    Ok(format!("Monitor setup\n{}", lines.join("\n")))
}

/// Runs the deterministic `/monitor` flow without a provider turn.
pub fn execute_monitor_flow(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    args: &str,
) -> Result<TurnExecution> {
    Ok(TurnExecution {
        assistant_text: handle_monitor_command(state, resources, providers, auth_store, args)?,
        tool_invocations: Vec::new(),
        reflection_traces: Vec::new(),
    })
}

fn resolve_monitor_connections(
    state: &mut AppState,
    resources: &LoadedResources,
    args: &str,
) -> Result<Vec<String>> {
    let options = monitor_connection_options(state)?;
    resolve_monitor_connections_from_options(state, resources, args, &options)
}

fn resolve_monitor_connections_from_options(
    state: &mut AppState,
    resources: &LoadedResources,
    args: &str,
    options: &[MonitorConnectionOption],
) -> Result<Vec<String>> {
    if options.is_empty() {
        bail!("no event-capable connections are available; run /connect first");
    }
    let slugs = options
        .iter()
        .map(|option| option.slug.as_str())
        .collect::<BTreeSet<_>>();
    let explicit = parse_connection_args(args);
    if explicit.is_empty() {
        return Ok(vec![ask_monitor_connection(state, resources, options)?]);
    }
    if explicit.iter().all(|slug| slugs.contains(slug.as_str())) {
        return Ok(explicit);
    }
    if !args.contains(',') {
        let matches = matching_monitor_connections(options, args);
        return match matches.len() {
            0 => Ok(explicit),
            1 => Ok(vec![matches[0].slug.clone()]),
            _ => Ok(vec![ask_monitor_connection(state, resources, &matches)?]),
        };
    }
    Ok(explicit)
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct MonitorConnectionOption {
    slug: String,
    description: String,
    search_text: String,
}

fn monitor_connection_options(state: &AppState) -> Result<Vec<MonitorConnectionOption>> {
    let manager = subscription_manager()?;
    manager.refresh_connection_auth()?;
    manager.refresh_connection_consumers()?;
    let paths = ConfigPaths::discover(&state.cwd);
    let roots = SubscriberManifestRoots::new(
        paths.workspace_config_dir,
        paths.user_config_dir,
        paths.builtin_resources_dir,
    );
    let connector_store = manager.connector_store();
    let options = manager
        .connection_store()
        .list()
        .into_iter()
        .filter_map(|connection| {
            let template = connector_store.get(&connection.connector_slug)?;
            connection_workflow_trigger_supported(&roots, &connection, &template)
                .then(|| monitor_connection_option(&connection, &template))
        })
        .collect::<Vec<_>>();
    Ok(options)
}

fn monitor_connection_option(
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> MonitorConnectionOption {
    let connection_description = connection.description.trim();
    let description = if connection_description.is_empty() {
        template.description.as_str()
    } else {
        connection_description
    };
    let consumer = if connection.has_consumer {
        "consumer=active"
    } else {
        "consumer=idle"
    };
    let description = format!(
        "{} via {} (state={:?}; {consumer})",
        description, connection.connector_slug, connection.state
    );
    let search_text = format!(
        "{} {} {} {:?}",
        connection.slug, connection.connector_slug, description, connection.state
    )
    .to_ascii_lowercase();
    MonitorConnectionOption {
        slug: connection.slug.clone(),
        description,
        search_text,
    }
}

fn matching_monitor_connections(
    options: &[MonitorConnectionOption],
    query: &str,
) -> Vec<MonitorConnectionOption> {
    let terms = search_terms(query);
    if terms.is_empty() {
        return options.to_vec();
    }
    options
        .iter()
        .filter(|option| terms.iter().all(|term| option.search_text.contains(term)))
        .cloned()
        .collect()
}

fn search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn ask_monitor_connection(
    state: &mut AppState,
    resources: &LoadedResources,
    options: &[MonitorConnectionOption],
) -> Result<String> {
    if options.len() > 50 {
        bail!(
            "too many matching connections ({}); run /monitor <search> to narrow the list",
            options.len()
        );
    }
    let tool_options = options
        .iter()
        .map(|option| {
            json!({
                "label": option.slug,
                "description": option.description
            })
        })
        .collect::<Vec<_>>();
    let question = "Which connection should Puffer monitor?";
    let output = call_tool(
        state,
        resources,
        "AskUserQuestion",
        json!({
            "questions": [{
                "type": "choice",
                "header": "Monitor",
                "question": question,
                "searchable": true,
                "options": tool_options
            }]
        }),
    )?;
    let answer = answer_string(&output, question)?;
    if options.iter().any(|option| option.slug == answer) {
        Ok(answer)
    } else {
        bail!("selected connection `{answer}` is not event-capable")
    }
}

fn call_tool(
    state: &mut AppState,
    resources: &LoadedResources,
    tool_id: &str,
    input: Value,
) -> Result<Value> {
    let cwd = state.cwd.clone();
    let output = execute_workflow_tool(state, resources, &cwd, tool_id, input, None)?;
    serde_json::from_str(&output).or_else(|_| Ok(json!({ "status": "complete", "output": output })))
}

fn answer_string(output: &Value, question: &str) -> Result<String> {
    if output
        .get("pending")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        bail!("interactive AskUserQuestion response is required for /monitor");
    }
    let value = output
        .get("answers")
        .and_then(|answers| answers.get(question))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("no answer provided for `{question}`"))?;
    Ok(value.to_string())
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
    use crate::runtime::{with_user_question_prompt_handler, UserQuestionPromptResponse};
    use puffer_config::PufferConfig;
    use puffer_session_store::SessionMetadata;
    use serde_json::Map;
    use std::sync::{Arc, Mutex};

    fn temp_state() -> AppState {
        let tempdir = tempfile::tempdir().unwrap();
        let cwd = tempdir.keep();
        let session = SessionMetadata {
            id: uuid::Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: cwd.clone(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        AppState::new(PufferConfig::default(), cwd, session)
    }

    fn option(slug: &str, description: &str) -> MonitorConnectionOption {
        MonitorConnectionOption {
            slug: slug.to_string(),
            description: description.to_string(),
            search_text: format!("{slug} {description}").to_ascii_lowercase(),
        }
    }

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

    #[test]
    fn monitor_connection_query_resolves_unique_match() {
        let mut state = temp_state();
        let resources = LoadedResources::default();
        let options = vec![
            option("telegram-user", "Telegram personal account"),
            option("slack-team", "Slack workspace"),
        ];

        let resolved =
            resolve_monitor_connections_from_options(&mut state, &resources, "telegram", &options)
                .expect("resolved");

        assert_eq!(resolved, vec!["telegram-user"]);
    }

    #[test]
    fn monitor_without_args_asks_searchable_connection_choice() {
        let mut state = temp_state();
        let resources = LoadedResources::default();
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let request_log = Arc::clone(&requests);
        let options = vec![
            option("telegram-user", "Telegram personal account"),
            option("slack-team", "Slack workspace"),
        ];

        let resolved = with_user_question_prompt_handler(
            move |request| {
                request_log.lock().unwrap().push(request.questions.clone());
                UserQuestionPromptResponse {
                    answers: Map::from_iter([(
                        "Which connection should Puffer monitor?".to_string(),
                        json!("slack-team"),
                    )]),
                    annotations: Map::new(),
                }
            },
            || resolve_monitor_connections_from_options(&mut state, &resources, "", &options),
        )
        .expect("resolved");

        assert_eq!(resolved, vec!["slack-team"]);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0][0]["searchable"], true);
        assert!(requests[0][0]["options"]
            .as_array()
            .is_some_and(|options| options.iter().any(|option| option["label"] == "slack-team")));
    }

    #[test]
    fn ambiguous_monitor_query_asks_searchable_connection_choice() {
        let mut state = temp_state();
        let resources = LoadedResources::default();
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let request_log = Arc::clone(&requests);
        let options = vec![
            option("slack-team", "Slack workspace"),
            option("slack-alerts", "Slack alerts"),
        ];

        let resolved = with_user_question_prompt_handler(
            move |request| {
                request_log.lock().unwrap().push(request.questions.clone());
                UserQuestionPromptResponse {
                    answers: Map::from_iter([(
                        "Which connection should Puffer monitor?".to_string(),
                        json!("slack-alerts"),
                    )]),
                    annotations: Map::new(),
                }
            },
            || resolve_monitor_connections_from_options(&mut state, &resources, "slack", &options),
        )
        .expect("resolved");

        assert_eq!(resolved, vec!["slack-alerts"]);
        let requests = requests.lock().unwrap();
        assert_eq!(requests[0][0]["searchable"], true);
        let labels = requests[0][0]["options"]
            .as_array()
            .expect("options")
            .iter()
            .map(|option| option["label"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["slack-team", "slack-alerts"]);
    }

    #[test]
    fn monitor_selection_rejects_unlisted_answer() {
        let mut state = temp_state();
        let resources = LoadedResources::default();
        let options = vec![option("telegram-user", "Telegram personal account")];

        let error = with_user_question_prompt_handler(
            |_| UserQuestionPromptResponse {
                answers: Map::from_iter([(
                    "Which connection should Puffer monitor?".to_string(),
                    json!("not-a-connection"),
                )]),
                annotations: Map::new(),
            },
            || resolve_monitor_connections_from_options(&mut state, &resources, "", &options),
        )
        .expect_err("selection should be rejected");

        assert!(error
            .to_string()
            .contains("selected connection `not-a-connection` is not event-capable"));
    }

    #[test]
    fn monitor_selection_requires_narrowed_matches_before_prompting() {
        let mut state = temp_state();
        let resources = LoadedResources::default();
        let options = (0..51)
            .map(|index| option(&format!("demo-{index:02}"), "Demo connection"))
            .collect::<Vec<_>>();

        let error = resolve_monitor_connections_from_options(&mut state, &resources, "", &options)
            .expect_err("too many matches should be rejected");

        assert!(error
            .to_string()
            .contains("too many matching connections (51)"));
    }
}
