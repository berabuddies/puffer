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

const RESEARCH_ACTION_PROMPT_GUIDANCE: &str = "\n\nResearch action prompt guidance:\n- For any Research action, actionPrompt must define the specific research question, include source chat/contact context, cap web research to at most 3 web searches and 8 total research/tool steps, avoid repeated equivalent queries, prefer official or primary sources, and tell the action agent to stop once it has enough evidence for a concise reply.";
const TASK_UPDATE_SOURCE_ISOLATION_POLICY: &str = "\n\nTaskUpdate source isolation policy:\n- Same chat/contact is not enough to call something a duplicate. If the current source event asks a new question, changes topic, or creates a separate request, create a new monitor task even when another task from the same sender is still pending.\n- If TaskUpdate changes an existing monitor task's subject, description, or activeForm, metadata MUST include monitor_envelope_id copied from the current workflow trigger and MUST replace or clear actions. When one consolidated task update depends on multiple workflow triggers, metadata SHOULD also include monitor_envelope_ids with every contributing trigger envelope_id. Never leave action prompts from the previous source message attached to updated task text.";
const TASK_FIELD_GUIDANCE: &str = "\n\nMonitor task field guidance\n- Write the subject as a next-step title for the user, not a neutral summary of the message. Prefer imperative verbs such as Reply, Review, Approve, Decide, Confirm, Schedule, Pay, Update, or Investigate.\n- The subject should answer \"what should I do next?\" without requiring the user to open the description. For example, use \"Reply with Friday availability\" instead of \"Friday availability question\".\n- Write the description to explain why this needs attention, who or what triggered it, any exact deadline/value that matters, and the concrete next step. Keep it concise, but include enough context for the user or action agent to act.\n- For awareness-only score-4 items, phrase the subject around the required user follow-up or decision. If there is no follow-up, decision, risk, deadline, or changed commitment, do not create a task.\n- Action names should be concrete user choices, and action prompts should carry the selected next step forward rather than merely summarize the source event.";

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
    Ok(format!(
        "Monitor setup\nnext=/workflows tasks | /tasks show <task-id> | /workflows connections\n{}",
        lines.join("\n")
    ))
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
    let mut prompt = format!(
        r#"You are my personal information triage assistant.
Your job is NOT to summarize messages.
Your job is to determine what I should pay attention to, what requires action from me, and what could materially affect my responsibilities, plans, commitments, or goals.
You will review messages from Telegram, Gmail, Lark, and other communication channels.

Connection context: `{connection_slug}` ({connector_slug}). Connector description: {connector_description}.

Group Chat Gate (MVP)
For group chats, only process messages where I am explicitly mentioned (@mention).
If I am not mentioned, ignore the message entirely.
Do not summarize it.
Do not score it.
Do not infer relevance.
Do not create tasks from it.
Do not include it in the final output.
Treat all non-mentioned group chat messages as noise.

Core Principle
Do NOT surface information simply because it exists.
Only surface information that is relevant to me.
If a message does not require my attention, action, response, approval, decision, or awareness, ignore it.

High Priority Signals
Surface information when:
Action Required
- Someone asks me a question.
- Someone requests something from me.
- Someone expects a response from me.
- Someone assigns work or responsibility to me.
- Someone needs my approval, review, feedback, or decision.

Awareness Required
- A deadline changes.
- A meeting changes.
- A commitment changes.
- A risk, blocker, incident, or escalation appears.
- A financial, travel, legal, or personal matter requires attention.
- An important update could affect my plans or responsibilities.

Direct Relevance
- I am explicitly mentioned.
- The message is directed at me.
- The message creates a task, decision, responsibility, risk, or opportunity relevant to me.

Ignore or Heavily Deprioritize
- Greetings.
- Small talk.
- Casual conversations.
- Memes, jokes, stickers, GIFs, emojis, and reactions.
- Discussions that do not require my involvement.
- Status updates with no action required.
- Duplicate information.
- Long conversations that contain no actionable outcome.

Scoring
Score every candidate item:
5 = Immediate action required
4 = Important awareness or follow-up likely needed
3 = Relevant but can wait
2 = Background information
1 = Noise

Only surface items scored 4 or 5.

Language policy
- Use the same language as the source message's primary language for generated monitor task fields: subject, description, actions[].actionPrompt, possibleIgnoreReasons, and any reply text drafted inside an action prompt.
- If the source message is mixed-language and you cannot identify a primary language, use the user's preferred language or owner language from available profile/context. If no user language is available, preserve the source's dominant actionable language and do not default to English only because this prompt is English.
- English source messages follow the same source-primary-language rule: English source messages should create English task fields and reply prompts.
- Preserve explicit product names, person names, company names, file names, commands, URLs, quoted text, and domain terms exactly; translate only surrounding explanatory prose.
- For reply actions, the final draft should use the source message's primary language unless the selected action explicitly requires a different language.

Puffer task creation protocol
1. Read `{}` if it exists and use it only as task-creation guidance.
2. If the event matches ignore memory, do not create a task. Do not edit memory or subscription filters.
3. Muted or silent notification events are filtered before this agent runs. If an event payload still says `notification_muted` or `notification_silent`, do not create a task.
4. For candidate items scored 1, 2, or 3, do not call TaskList, TaskCreate, or TaskUpdate.
5. For candidate items scored 4 or 5, use TaskList first to avoid duplicates. Use TaskCreate for new tasks and TaskUpdate for materially changed existing monitor tasks. For ordinary content edits, update content fields only and leave `status` unchanged. Task completion is governed by the Completion protocol below.
6. Every monitor TaskCreate MUST include `receivedAt` from the workflow trigger's RFC3339 `receivedAt` field and an RFC3339 `expiresAt` chosen from the event urgency. If no better deadline is evident, set `expiresAt` 24 hours after `receivedAt`.
7. Every monitor TaskCreate MUST include metadata with `_monitor: true`, `monitor_connection: "{connection_slug}"`, `monitor_connector: "{connector_slug}"`, and `monitor_memory_path: "{}"`. Copy the primary source trigger's `envelope_id` into `metadata.monitor_envelope_id`; when one consolidated task depends on multiple triggers, also copy every contributing trigger `envelope_id` into `metadata.monitor_envelope_ids` as a JSON array.
8. Every monitor TaskCreate SHOULD include `actions`: an array of objects with `actionName` and `actionPrompt`, and `possibleIgnoreReasons`: a short array of suggested ignore reasons.
9. Keep action prompts ready to send to the current coding agent. Include enough source context from the connector event for the agent to act without rereading the whole stream.

Completion protocol
- The workflow trigger has a `direction` field: `incoming` (a message from a contact) or `outgoing` (a message I sent myself).
- When the current message indicates that an existing open monitor task in THIS conversation has been done/handled/resolved, complete that task with TaskUpdate with `status: completed`. Match the conversation by the task metadata identity fields (e.g. `chat_id`) you previously stored.
- First TaskList; only complete a task you can see in this conversation. If several are open and the message is ambiguous, complete none. If the message is a reply quoting an earlier message, prefer the task whose stored source matches that quote.
- A message like "not done yet" / "还没" must NOT complete a task.
- If `direction` is `outgoing`: you may ONLY complete or update existing tasks — never create a task from my own message.
- Completing a task only marks it done and never sends a reply, so you may complete tasks that have a reply/delivery target too.

Do not send connector replies unless a selected action later asks for it."#,
        memory_path.display(),
        memory_path.display()
    );
    prompt.push_str(TASK_UPDATE_SOURCE_ISOLATION_POLICY);
    prompt.push_str(TASK_FIELD_GUIDANCE);
    prompt.push_str(RESEARCH_ACTION_PROMPT_GUIDANCE);
    prompt
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
        "Use Telegram list_peers first and skip peers where notification_muted=true. Then use list_messages with the exact connection_slug named above and succinct=true to inspect recent messages only for unmuted peers."
    } else {
        "Use Slack list_conversations and read_messages with the exact connection_slug named above to inspect recent messages."
    };
    let mut prompt = format!(
        r#"Backfill monitor tasks for connection `{connection_slug}` ({connector_slug}). Every connector lookup tool call MUST pass `connection_slug: "{connection_slug}"`.

{backfill_hint}

Apply this personal triage policy to historical messages:
- Do NOT summarize messages.
- Surface only information that requires my attention, action, response, approval, decision, or awareness.
- For group chats, only process messages where I am explicitly mentioned (@mention). Treat non-mentioned group chat messages as noise.
- Ignore greetings, small talk, casual conversations, memes, jokes, stickers, GIFs, emojis, reactions, status updates with no action required, duplicate information, and long conversations with no actionable outcome.
- Score every candidate item: 5 = immediate action required; 4 = important awareness or follow-up likely needed; 3 = relevant but can wait; 2 = background information; 1 = noise.
- Only create or update tasks for items scored 4 or 5. For items scored 1, 2, or 3, do not call TaskCreate or TaskUpdate.
- Use the same language as the source message's primary language for generated monitor task fields: subject, description, actions[].actionPrompt, possibleIgnoreReasons, and any reply text drafted inside an action prompt.
- If the source message is mixed-language and you cannot identify a primary language, use the user's preferred language or owner language from available profile/context. If no user language is available, preserve the source's dominant actionable language and do not default to English only because this prompt is English.
- English source messages follow the same source-primary-language rule: English source messages should create English task fields and reply prompts.
- Preserve explicit product names, person names, company names, file names, commands, URLs, quoted text, and domain terms exactly; translate only surrounding explanatory prose.
- Write task subjects as next-step titles for the user, not neutral message summaries. The subject should answer "what should I do next?" without requiring the user to open the description.
- Write descriptions with the source context, why it needs attention, and the concrete next step.

Read monitor memory `{}` first and skip ignored examples. Skip muted or silent notifications. Avoid duplicates by calling TaskList first.

For scored 4 or 5 current work, create monitor tasks using TaskCreate with RFC3339 `receivedAt`, RFC3339 `expiresAt`, metadata `_monitor: true`, `monitor_connection: "{connection_slug}"`, `monitor_connector: "{connector_slug}"`, and `monitor_memory_path: "{}"`. Include `actions` and `possibleIgnoreReasons` for each task."#,
        memory_path.display(),
        memory_path.display()
    );
    prompt.push_str(RESEARCH_ACTION_PROMPT_GUIDANCE);
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
        assert!(prompt.contains("You are my personal information triage assistant"));
        assert!(prompt.contains("Group Chat Gate (MVP)"));
        assert!(prompt.contains("Only surface items scored 4 or 5"));
        assert!(prompt.contains("Someone needs my approval, review, feedback, or decision"));
        assert!(prompt.contains("Status updates with no action required"));
        assert!(prompt.contains("For candidate items scored 1, 2, or 3"));
        assert!(prompt.contains("do not call TaskList, TaskCreate, or TaskUpdate"));
        assert!(prompt.contains("notification_muted"));
        assert!(prompt.contains("receivedAt"));
        assert!(prompt.contains("expiresAt"));
        assert!(prompt.contains("Research action prompt guidance"));
        assert!(prompt.contains("TaskUpdate source isolation policy"));
        assert!(prompt.contains("Monitor task field guidance"));
        assert!(prompt.contains("what should I do next?"));
        assert!(prompt.contains("Reply with Friday availability"));
        assert!(prompt.contains("Same chat/contact is not enough"));
        assert!(prompt.contains("replace or clear actions"));
        assert!(prompt.contains("avoid repeated equivalent queries"));
        assert!(prompt.contains("Use the same language as the source message's primary language"));
        assert!(prompt.contains("subject, description, actions[].actionPrompt"));
        assert!(prompt.contains("If the source message is mixed-language"));
        assert!(prompt.contains("English source messages"));
        assert!(prompt.contains("Preserve explicit product names"));
    }

    #[test]
    fn monitor_prompt_allows_direction_aware_completion() {
        let prompt = monitor_triage_prompt(
            "telegram-user",
            "telegram-login",
            "Personal Telegram",
            Path::new("/tmp/memory.md"),
        );
        // The blanket "never change status" rule is gone…
        assert!(!prompt.contains("never change its `status`"));
        // …replaced by an explicit completion rule keyed on trigger direction.
        assert!(prompt.contains("Completion protocol"));
        assert!(prompt.contains("`direction`"));
        assert!(prompt.contains("TaskUpdate with `status: completed`"));
        // Outgoing must never create tasks.
        assert!(prompt.contains("never create a task"));
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
