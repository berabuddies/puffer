use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{
    connection_workflow_trigger_supported, normalize_contact_id, ActionSpec, ConnectionRecord,
    ConnectionState, WorkflowBindingSpec, WorkflowBindingStatus,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

const RESEARCH_ACTION_PROMPT_GUIDANCE: &str = "\n\nResearch action prompt guidance:\n- For any Research action, actionPrompt must define the specific research question, include source chat/contact context, cap web research to at most 3 web searches and 8 total research/tool steps, avoid repeated equivalent queries, prefer official or primary sources, and tell the action agent to stop once it has enough evidence for a concise reply.";
const TASK_UPDATE_SOURCE_ISOLATION_POLICY: &str = "\n\nTaskUpdate source isolation policy:\n- Same chat/contact is not enough to call something a duplicate. If the current source event asks a new question, changes topic, or creates a separate request, create a new monitor task even when another task from the same sender is still pending.\n- Task source provenance is server-owned. Do not include monitor_envelope_id, monitor_envelope_ids, source_context, source_text, source_message_id, delivery_target, or other source/delivery metadata in TaskUpdate. Use TaskUpdate for status changes or ordinary non-source content updates only.";
const MONITOR_SOURCE_PROVENANCE_POLICY: &str = "\n\nMonitor source provenance policy:\n- The current workflow trigger or current digest triggers are the only valid source events for TaskCreate.\n- Conversation context may explain short or referential messages, but never copy envelope_id or source identity from conversation_context into TaskCreate metadata.\n- For TaskCreate, pass `sourceEnvelopeId` copied from the relevant current workflow trigger's `envelope_id`. If one review-only task depends on several current digest triggers, pass `sourceEnvelopeIds` containing only current contributing trigger `envelope_id` values.\n- Do not write `metadata.monitor_envelope_id`, `metadata.monitor_envelope_ids`, `metadata.source_context`, `metadata.monitor`, `metadata.pending_action`, or source hash fields. The daemon stamps those fields after validating the source envelope selector.\n- If TaskCreate returns skipped=true with reason duplicate_source, duplicate_monitor_task, or untrusted_monitor_source, do not retry by changing source metadata.";
const TYPED_MONITOR_CONTRACT_POLICY: &str = "\n\nTyped monitor task contract (daemon-stamped)\n- Every new monitor TaskCreate MUST provide `sourceEnvelopeId` unless there is exactly one current workflow trigger.\n- The daemon derives `metadata.monitor.schema_version=2`, `kind`, `source`, `action`, `source_context`, completion policy, and pending action state from the selected current trigger. Do not create or update those metadata fields.\n- The daemon chooses exactly one kind for each task: `telegram.reply` for Telegram reply tasks, `gmail.reply` for Gmail reply/draft tasks, `calendar.rsvp` for Google Calendar RSVP tasks, or `generic.review` when the item requires review but has no safe app-specific executable action.\n- Do not create one executable task from multiple unrelated sources. If several current envelope IDs are needed only as evidence for one review item, use one TaskCreate with `sourceEnvelopeIds`; the daemon will stamp it as `generic.review`.\n- Continue copying stable scalar source identity fields to top-level metadata for legacy duplicate and ignore matching (for example `chat_id`, `sender_id`, `thread_id`, `from_email`, `calendar_id`, `event_id`, `account_id`). Never infer these fields from unstructured text.";
const TASK_FIELD_GUIDANCE: &str = "\n\nMonitor task field guidance\n- Write the subject as a next-step title for the user, not a neutral summary of the message. Prefer imperative verbs such as Reply, Review, Approve, Decide, Confirm, Schedule, Pay, Update, or Investigate.\n- The subject should answer \"what should I do next?\" without requiring the user to open the description. For example, use \"Reply with Friday availability\" instead of \"Friday availability question\".\n- Write the description to explain why this needs attention, who or what triggered it, any exact deadline/value that matters, and the concrete next step. Keep it concise, but include enough context for the user or action agent to act.\n- For awareness-only score-4 items, phrase the subject around the required user follow-up or decision. If there is no follow-up, decision, risk, deadline, or changed commitment, do not create a task.\n- Action names should be concrete user choices, and action prompts should carry the selected next step forward rather than merely summarize the source event.";

#[derive(Debug, Deserialize)]
struct MonitorCreateParams {
    #[serde(alias = "connectionSlug")]
    connection_slug: String,
    #[serde(default, deserialize_with = "deserialize_model_update")]
    model: Option<Option<String>>,
    #[serde(default)]
    contact_ids: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MonitorModelUpdate {
    Preserve,
    Set(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MonitorContactUpdate {
    Preserve,
    Set(Vec<String>),
}

/// Creates or resumes a monitor workflow binding and returns the refreshed snapshot.
pub(crate) fn handle_monitor_create(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: MonitorCreateParams =
        serde_json::from_value(params.clone()).context("invalid monitor create params")?;
    let connection_slug = normalized_connection_slug(&params.connection_slug)?;
    let model_update = normalized_model_update(params.model);
    let contact_update = normalized_contact_update(params.contact_ids);
    create_or_resume_monitor(paths, &connection_slug, model_update, contact_update)?;
    super::handle_workflow_list(paths)
}

fn create_or_resume_monitor(
    paths: &ConfigPaths,
    connection_slug: &str,
    model_update: MonitorModelUpdate,
    contact_update: MonitorContactUpdate,
) -> Result<()> {
    let manager = subscription_manager()?;
    manager.refresh_connection_auth()?;
    let connection = manager
        .connection_store()
        .get(connection_slug)
        .ok_or_else(|| anyhow::anyhow!("connection `{connection_slug}` not found"))?;
    eprintln!(
        "monitor-create: connection={connection_slug} connector={} state={:?} has_consumer={} auth_failure_notified={}",
        connection.connector_slug,
        connection.state,
        connection.has_consumer,
        connection.auth_failure_notified
    );
    ensure_connection_auth_usable(&connection)?;
    let template = manager
        .connector_store()
        .get(&connection.connector_slug)
        .ok_or_else(|| anyhow::anyhow!("connector `{}` not found", connection.connector_slug))?;
    if !connection_workflow_trigger_supported(
        &super::subscriber_manifest_roots(paths),
        &connection,
        &template,
    ) {
        anyhow::bail!(
            "connector `{}` cannot produce workflow trigger events",
            connection.connector_slug
        );
    }
    let contact_update = expand_monitor_contact_update(
        paths,
        connection_slug,
        &connection.connector_slug,
        contact_update,
    );

    let monitor_slug = monitor_slug(connection_slug);
    let memory_path = monitor_memory_path(paths, connection_slug)?;
    ensure_monitor_memory(&memory_path, connection_slug, &connection.connector_slug)?;
    let previous = manager.store().get(&monitor_slug);
    let binding = match previous.clone() {
        Some(mut binding) => {
            binding.status = WorkflowBindingStatus::Enabled;
            refresh_monitor_binding(
                &mut binding,
                connection_slug,
                &connection.connector_slug,
                &template.description,
                &memory_path,
                &model_update,
                &contact_update,
            )?;
            binding
        }
        None => monitor_binding(
            connection_slug,
            &connection.connector_slug,
            &template.description,
            &memory_path,
            model_update.model(None).as_deref(),
            contact_update.ids(None),
        )?,
    };
    manager.store().upsert(binding)?;
    eprintln!(
        "monitor-create: binding={} connection={connection_slug} action=triage_agent previous_exists={}",
        monitor_slug,
        previous.is_some()
    );
    let setup_result = (|| -> Result<()> {
        ensure_workflow_subscriber_started(manager.as_ref(), paths, &connection, &template)?;
        manager.refresh_connection_consumers()?;
        Ok(())
    })();
    if let Err(error) = setup_result {
        rollback_monitor_binding(manager.as_ref(), &monitor_slug, previous)?;
        manager.refresh_connection_consumers()?;
        return Err(error).with_context(|| format!("monitor `{monitor_slug}` setup failed"));
    }
    eprintln!("monitor-create: monitor={monitor_slug} setup_complete=true");
    Ok(())
}

fn ensure_connection_auth_usable(connection: &ConnectionRecord) -> Result<()> {
    if matches!(
        connection.state,
        ConnectionState::Authenticated | ConnectionState::Active
    ) && !connection.auth_failure_notified
    {
        return Ok(());
    }
    anyhow::bail!(
        "connection `{}` auth is not functioning; run `/connect {} {}` to repair it",
        connection.slug,
        connection.connector_slug,
        connection.slug
    );
}

fn rollback_monitor_binding(
    manager: &puffer_subscriptions::SubscriptionManager,
    monitor_slug: &str,
    previous: Option<WorkflowBindingSpec>,
) -> Result<()> {
    if let Some(previous) = previous {
        manager.store().upsert(previous)?;
    } else {
        manager.store().delete(monitor_slug)?;
    }
    Ok(())
}

fn monitor_binding(
    connection_slug: &str,
    connector_slug: &str,
    connector_description: &str,
    memory_path: &Path,
    model: Option<&str>,
    contact_ids: Vec<String>,
) -> Result<WorkflowBindingSpec> {
    Ok(WorkflowBindingSpec {
        slug: monitor_slug(connection_slug),
        description: format!("Monitor {connection_slug} for actionable tasks"),
        connection_slug: connection_slug.to_string(),
        connector_slug: Some(connector_slug.to_string()),
        status: super::workflow_status(true),
        filter: None,
        ignore_filters: Vec::new(),
        contact_ids: contact_ids.clone(),
        classify_prompt: None,
        classify_model: None,
        action: monitor_action(
            connection_slug,
            connector_slug,
            connector_description,
            memory_path,
            model,
            &contact_ids,
        )?,
        created_at_ms: puffer_subscriptions::now_ms(),
    })
}

fn refresh_monitor_binding(
    binding: &mut WorkflowBindingSpec,
    connection_slug: &str,
    connector_slug: &str,
    connector_description: &str,
    memory_path: &Path,
    model_update: &MonitorModelUpdate,
    contact_update: &MonitorContactUpdate,
) -> Result<()> {
    let model = model_update.model(monitor_action_model(&binding.action));
    let contact_ids = contact_update.ids(Some(binding.contact_ids.clone()));
    binding.description = format!("Monitor {connection_slug} for actionable tasks");
    binding.connection_slug = connection_slug.to_string();
    binding.connector_slug = Some(connector_slug.to_string());
    binding.contact_ids = contact_ids.clone();
    binding.action = monitor_action(
        connection_slug,
        connector_slug,
        connector_description,
        memory_path,
        model.as_deref(),
        &contact_ids,
    )?;
    Ok(())
}

fn monitor_action(
    connection_slug: &str,
    connector_slug: &str,
    connector_description: &str,
    memory_path: &Path,
    model: Option<&str>,
    contact_ids: &[String],
) -> Result<ActionSpec> {
    let action_prompt = monitor_triage_prompt(
        connection_slug,
        connector_slug,
        connector_description,
        memory_path,
        contact_ids,
    );
    Ok(ActionSpec::TriageAgent {
        prompt: action_prompt,
        model: model.map(ToOwned::to_owned),
    })
}

fn normalized_connection_slug(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("missing connection_slug");
    }
    Ok(value.to_string())
}

fn normalized_model_update(value: Option<Option<String>>) -> MonitorModelUpdate {
    match value {
        None => MonitorModelUpdate::Preserve,
        Some(None) => MonitorModelUpdate::Set(None),
        Some(Some(model)) => {
            let model = model.trim();
            MonitorModelUpdate::Set((!model.is_empty()).then(|| model.to_string()))
        }
    }
}

fn normalized_contact_update(value: Option<Vec<String>>) -> MonitorContactUpdate {
    match value {
        None => MonitorContactUpdate::Preserve,
        Some(ids) => MonitorContactUpdate::Set(puffer_subscriptions::normalize_contact_ids(ids)),
    }
}

fn expand_monitor_contact_update(
    paths: &ConfigPaths,
    connection_slug: &str,
    connector_slug: &str,
    update: MonitorContactUpdate,
) -> MonitorContactUpdate {
    match update {
        MonitorContactUpdate::Preserve => MonitorContactUpdate::Preserve,
        MonitorContactUpdate::Set(ids) => MonitorContactUpdate::Set(expand_monitor_contact_ids(
            paths,
            connection_slug,
            connector_slug,
            ids,
        )),
    }
}

fn expand_monitor_contact_ids(
    paths: &ConfigPaths,
    connection_slug: &str,
    connector_slug: &str,
    ids: Vec<String>,
) -> Vec<String> {
    if connector_slug != "telegram-login" {
        return puffer_subscriptions::normalize_contact_ids(ids);
    }
    let username_aliases = telegram_username_contact_id_aliases(paths, connection_slug);
    ids.into_iter()
        .filter_map(|id| normalize_contact_id(&id))
        .map(|id| username_aliases.get(&id).cloned().unwrap_or(id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn telegram_username_contact_id_aliases(
    paths: &ConfigPaths,
    connection_slug: &str,
) -> HashMap<String, String> {
    let path = paths
        .user_config_dir
        .join("telegram-accounts")
        .join(connection_slug)
        .join("peer-cache.json");
    let Ok(raw) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(cache) = serde_json::from_str::<Value>(&raw) else {
        return HashMap::new();
    };
    let Some(peers) = cache.get("peers").and_then(Value::as_array) else {
        return HashMap::new();
    };
    let mut aliases = HashMap::new();
    for peer in peers {
        if peer.get("kind").and_then(Value::as_str) != Some("user")
            || peer.get("is_bot").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(numeric_id) = telegram_peer_numeric_id(peer) else {
            continue;
        };
        let Some(numeric_contact_id) =
            normalize_contact_id(&format!("telegram-user-id@{numeric_id}"))
        else {
            continue;
        };
        for username in telegram_peer_usernames(peer) {
            if let Some(username_contact_id) = normalize_contact_id(&format!("telegram@{username}"))
            {
                aliases.insert(username_contact_id, numeric_contact_id.clone());
            }
        }
    }
    aliases
}

fn telegram_peer_numeric_id(peer: &Value) -> Option<i64> {
    peer.get("numeric_id")
        .and_then(Value::as_i64)
        .or_else(|| {
            peer.get("id")
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<i64>().ok())
        })
        .filter(|id| *id > 0)
}

fn telegram_peer_usernames(peer: &Value) -> Vec<String> {
    let mut usernames = BTreeSet::new();
    if let Some(username) = peer.get("username").and_then(Value::as_str) {
        let username = username.trim().trim_start_matches('@');
        if !username.is_empty() {
            usernames.insert(username.to_string());
        }
    }
    if let Some(values) = peer.get("usernames").and_then(Value::as_array) {
        for value in values {
            let Some(username) = value.as_str() else {
                continue;
            };
            let username = username.trim().trim_start_matches('@');
            if !username.is_empty() {
                usernames.insert(username.to_string());
            }
        }
    }
    usernames.into_iter().collect()
}

fn deserialize_model_update<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

impl MonitorModelUpdate {
    fn model(&self, current: Option<String>) -> Option<String> {
        match self {
            Self::Preserve => current,
            Self::Set(model) => model.clone(),
        }
    }
}

impl MonitorContactUpdate {
    fn ids(&self, current: Option<Vec<String>>) -> Vec<String> {
        match self {
            Self::Preserve => current.unwrap_or_default(),
            Self::Set(ids) => ids.clone(),
        }
    }
}

fn monitor_action_model(action: &ActionSpec) -> Option<String> {
    match action {
        ActionSpec::TriageAgent { model, .. } => model.clone(),
        _ => None,
    }
}

fn monitor_slug(connection_slug: &str) -> String {
    format!("monitor-{connection_slug}")
}

fn monitor_memory_path(paths: &ConfigPaths, connection_slug: &str) -> Result<PathBuf> {
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

fn ensure_workflow_subscriber_started(
    manager: &puffer_subscriptions::SubscriptionManager,
    paths: &ConfigPaths,
    connection: &puffer_subscriptions::ConnectionRecord,
    template: &puffer_subscriptions::ConnectorTemplate,
) -> Result<()> {
    super::ensure_workflow_subscriber_started(manager, paths, connection, template)
}

fn monitor_triage_prompt(
    connection_slug: &str,
    connector_slug: &str,
    connector_description: &str,
    memory_path: &Path,
    contact_ids: &[String],
) -> String {
    let contact_scope = if contact_ids.is_empty() {
        "all contacts".to_string()
    } else {
        contact_ids.join(", ")
    };
    let mut prompt = format!(
        "You are my personal information triage assistant.\nYour job is NOT to summarize messages.\nYour job is to determine what I should pay attention to, what requires action from me, and what could materially affect my responsibilities, plans, commitments, or goals.\nYou will review messages from Telegram, Gmail, Lark, and other communication channels.\n\nConnection context: `{connection_slug}` ({connector_slug}). Connector description: {connector_description}. Contact scope: {contact_scope}.\n\nGroup Chat Gate (MVP)\nFor group chats, only process messages where I am explicitly mentioned (@mention).\nIf I am not mentioned, ignore the message entirely.\nDo not summarize it.\nDo not score it.\nDo not infer relevance.\nDo not create tasks from it.\nDo not include it in the final output.\nTreat all non-mentioned group chat messages as noise.\n\nCore Principle\nDo NOT surface information simply because it exists.\nOnly surface information that is relevant to me.\nIf a message does not require my attention, action, response, approval, decision, or awareness, ignore it.\n\nHigh Priority Signals\nSurface information when:\nAction Required\n- Someone asks me a question.\n- Someone requests something from me.\n- Someone expects a response from me.\n- Someone assigns work or responsibility to me.\n- Someone needs my approval, review, feedback, or decision.\n\nAwareness Required\n- A deadline changes.\n- A meeting changes.\n- A commitment changes.\n- A risk, blocker, incident, or escalation appears.\n- A financial, travel, legal, or personal matter requires attention.\n- An important update could affect my plans or responsibilities.\n\nDirect Relevance\n- I am explicitly mentioned.\n- The message is directed at me.\n- The message creates a task, decision, responsibility, risk, or opportunity relevant to me.\n\nIgnore or Heavily Deprioritize\n- Greetings.\n- Small talk.\n- Casual conversations.\n- Memes, jokes, stickers, GIFs, emojis, and reactions.\n- Discussions that do not require my involvement.\n- Status updates with no action required.\n- Duplicate information.\n- Long conversations that contain no actionable outcome.\n\nScoring\nScore every candidate item:\n5 = Immediate action required\n4 = Important awareness or follow-up likely needed\n3 = Relevant but can wait\n2 = Background information\n1 = Noise\n\nOnly surface items scored 4 or 5.\n\nLanguage policy\n- Use the same language as the source message's primary language for generated monitor task fields: subject, description, actions[].actionPrompt, possibleIgnoreReasons, and any reply text drafted inside an action prompt.\n- If the source message is mixed-language and you cannot identify a primary language, use the user's preferred language or owner language from available profile/context. If no user language is available, preserve the source's dominant actionable language and do not default to English only because this prompt is English.\n- English source messages follow the same source-primary-language rule: English source messages should create English task fields and reply prompts.\n- Preserve explicit product names, person names, company names, file names, commands, URLs, quoted text, and domain terms exactly; translate only surrounding explanatory prose.\n- For reply actions, the final draft should use the source message's primary language unless the selected action explicitly requires a different language.\n\nSource fidelity policy\n- Copy every number, percentage, amount, date, time, duration, and identifier into task fields exactly as written in the relevant workflow trigger's event text. Never round, convert, infer, or substitute values.\n- Never reuse values from unrelated messages, earlier tasks, or prior context; the relevant trigger text is authoritative for task fields.\n- When the source message contains critical values (numbers, deadlines, amounts), quote the relevant sentence verbatim inside the task description instead of paraphrasing it.\n\nBurst handling policy\n- The workflow input may be a batch of multiple same-conversation triggers collected after a short quiet period.\n- Read the full batch in chronological order before deciding whether there is a task.\n- Create or update the task that captures the user's actual intent across the burst, rather than creating one task per message or focusing only on the final message.\n- If several messages form one request, write one concise subject and description for the combined request.\n- If different messages in the batch contain unrelated actionable requests, create separate tasks for those requests.\n\nConversation context policy\n- When the workflow trigger payload contains `conversation_context`, read it before deciding what the current source event means.\n- If `conversation_context.source` is `telegram_server_history_cache`, treat `conversation_context.messages` as bounded recent Telegram server-history messages from the same direct chat before the current trigger, including my outgoing replies and the peer's messages.\n- If `conversation_context.source` is `subscriber_diagnostics`, treat the messages as best-effort observed subscriber diagnostics that may have gaps; do not assume they are complete or immediately adjacent to the current trigger.\n- Use conversation context only to disambiguate short or referential messages such as \"聊下？\", \"可以吗？\", or \"按刚才说的来\".\n- Do not create tasks from prior context alone. The current workflow trigger text or trigger batch remains the source event that decides whether a task exists.\n- Do not replace current source values (numbers, deadlines, amounts, requests) with prior-message values. Prior messages only explain the current message's intent and topic.\n\nPuffer task creation protocol\n1. Read `{}` if it exists and use it only as task-creation guidance.\n2. If the event matches ignore memory, do not create a task. Do not edit memory or subscription filters.\n3. Muted or silent notification events are filtered before this agent runs. If an event payload still says `notification_muted` or `notification_silent`, do not create a task.\n4. For candidate items scored 1, 2, or 3, do not call TaskList, TaskCreate, or TaskUpdate.\n5. For candidate items scored 4 or 5, use TaskList first to avoid duplicates. Use TaskCreate for new tasks and TaskUpdate for materially changed existing monitor tasks. For ordinary content edits, update content fields only and leave `status` unchanged. Task completion is governed by the Completion protocol below.\n6. Every monitor TaskCreate MUST include `receivedAt` from the relevant workflow trigger's RFC3339 `receivedAt` field and an RFC3339 `expiresAt` chosen from the event urgency. If several triggers form one request, use the latest trigger's `receivedAt`. If no better deadline is evident, set `expiresAt` 24 hours after `receivedAt`.\n7. Every monitor TaskCreate MUST include metadata with `_monitor: true`, `monitor_memory_path: \"{}\"`, and `monitor_contact_ids: {:?}`. Select the source event by passing top-level `sourceEnvelopeId` copied from the relevant workflow trigger's `envelope_id`; for one review-only task that depends on several current triggers, pass top-level `sourceEnvelopeIds`.\n8. Do not write source identity or delivery metadata such as `monitor_connection`, `monitor_connector`, `monitor_envelope_id`, `chat_id`, `sender_id`, `thread_id`, `from_email`, `calendar_id`, `event_id`, `source_context`, `monitor`, or `pending_action`. The daemon stamps these fields from the selected current source envelope for duplicate detection, ignore matching, and typed actions.\n9. Never infer source identity from unstructured text. Do not use per-event fields such as `message_id`, `event_id`, `dedup_key`, timestamps, message text, body, subject, content, or title as ignore identity metadata.\n10. Do not add any metadata field named `monitor_ignore_filter`, `event_ignore_filter`, or `ignore_filter`; ignore filter installation is daemon-owned.\n11. Every monitor TaskCreate SHOULD include `actions`: an array of objects with `actionName` and `actionPrompt`, and `possibleIgnoreReasons`: a short array of suggested ignore reasons.\n12. Keep action prompts ready to send to the current coding agent. Include enough source context from the connector event or trigger batch for the agent to act without rereading the whole stream.\n\nCompletion protocol\n- The workflow trigger has a `direction` field: `incoming` (a message from a contact) or `outgoing` (a message I sent myself).\n- When the current message indicates that an existing open monitor task in THIS conversation has been done/handled/resolved, complete that task with TaskUpdate with `status: completed`. Match the conversation by the task metadata identity fields (e.g. `chat_id`) you previously stored.\n- Resolution is broader than \"I did it\": a decisive decision, refusal, or dismissal also completes the task — choosing NOT to act still closes the loop (e.g. outgoing \"不给你擦\" / \"不弄了\" / \"不需要了\" / \"算了，不做了\"). An incoming message where the contact reports the thing is done/handled also completes the matching task (e.g. \"擦完了\" / \"搞定了\" / \"已处理\" / \"弄好了\").\n- First TaskList; only complete a task you can see in this conversation. If several are open and the current message is ambiguous about which one, complete none — UNLESS the message quotes or replies to an earlier message: a quote/reply is NOT ambiguous, so match the quoted text to the task whose stored source matches it and complete that one.\n- A message that defers rather than resolves must NOT complete a task: \"not done yet\" / \"还没\" / \"等下\" / \"晚点再说\" / \"在看\" never complete.\n- If `direction` is `outgoing`: you may ONLY complete or update existing tasks — never create a task from my own message.\n- Completing a task only marks it done and never sends a reply, so you may complete tasks that have a reply/delivery target too.\n\nDo not send connector replies unless a selected action later asks for it.",
        memory_path.display(),
        memory_path.display(),
        contact_ids
    );
    prompt.push_str(MONITOR_SOURCE_PROVENANCE_POLICY);
    prompt.push_str(TYPED_MONITOR_CONTRACT_POLICY);
    prompt.push_str(TASK_UPDATE_SOURCE_ISOLATION_POLICY);
    prompt.push_str(TASK_FIELD_GUIDANCE);
    prompt.push_str(RESEARCH_ACTION_PROMPT_GUIDANCE);
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn monitor_create_requires_connection_slug() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());

        let error = handle_monitor_create(&paths, &json!({"connection_slug": "  "})).unwrap_err();

        assert!(error.to_string().contains("missing connection_slug"));
    }

    #[test]
    fn monitor_create_params_distinguish_missing_and_null_model() {
        let missing: MonitorCreateParams =
            serde_json::from_value(json!({"connection_slug": "telegram-user"})).unwrap();
        let cleared: MonitorCreateParams =
            serde_json::from_value(json!({"connection_slug": "telegram-user", "model": null}))
                .unwrap();
        let selected: MonitorCreateParams = serde_json::from_value(json!({
            "connection_slug": "telegram-user",
            "model": " openai/gpt-5.4 "
        }))
        .unwrap();

        assert_eq!(missing.model, None);
        assert_eq!(cleared.model, Some(None));
        assert_eq!(
            normalized_model_update(selected.model),
            MonitorModelUpdate::Set(Some("openai/gpt-5.4".to_string()))
        );
    }

    #[test]
    fn monitor_create_rejects_degraded_connection_auth() {
        let mut connection =
            ConnectionRecord::authenticated("telegram-user", "telegram-login", "Telegram");
        connection.state = ConnectionState::Degraded;
        connection.auth_failure_notified = true;

        let error = ensure_connection_auth_usable(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains("/connect telegram-login telegram-user"));
    }

    #[test]
    fn monitor_memory_initializes_without_overwrite() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let path = monitor_memory_path(&paths, "telegram-user").unwrap();

        ensure_monitor_memory(&path, "telegram-user", "telegram-login").unwrap();
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("telegram-login"));

        fs::write(&path, "custom ignore rules").unwrap();
        ensure_monitor_memory(&path, "telegram-user", "telegram-login").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "custom ignore rules");
    }

    #[test]
    fn monitor_prompt_names_task_metadata() {
        let tempdir = tempfile::tempdir().unwrap();
        let memory_path = tempdir.path().join("telegram-user.md");

        let prompt = monitor_triage_prompt(
            "telegram-user",
            "telegram-login",
            "Personal Telegram",
            &memory_path,
            &["telegram@alice".to_string()],
        );

        assert!(prompt.contains("You are my personal information triage assistant"));
        assert!(prompt.contains("Group Chat Gate (MVP)"));
        assert!(prompt.contains("Only surface items scored 4 or 5"));
        assert!(prompt.contains("Someone needs my approval, review, feedback, or decision"));
        assert!(prompt.contains("Status updates with no action required"));
        assert!(prompt.contains("For candidate items scored 1, 2, or 3"));
        assert!(prompt.contains("TaskCreate MUST include metadata"));
        assert!(!prompt.contains("monitor_connection: \"telegram-user\""));
        assert!(!prompt.contains("monitor_connector: \"telegram-login\""));
        assert!(prompt.contains("monitor_contact_ids"));
        assert!(prompt.contains("Do not write source identity or delivery metadata"));
        assert!(prompt.contains("The daemon stamps these fields"));
        assert!(prompt.contains("Source fidelity policy"));
        assert!(prompt.contains("exactly as written in the relevant workflow trigger's event text"));
        assert!(prompt.contains("Completion protocol"));
        assert!(prompt.contains("Burst handling policy"));
        assert!(prompt.contains("multiple same-conversation triggers"));
        assert!(prompt.contains("captures the user's actual intent across the burst"));
        assert!(prompt.contains("telegram@alice"));
        assert!(prompt.contains("sourceEnvelopeId"));
        assert!(prompt.contains("Never infer source identity"));
        assert!(prompt.contains("Do not add any metadata field named"));
        assert!(prompt.contains("Research action prompt guidance"));
        assert!(prompt.contains("TaskUpdate source isolation policy"));
        assert!(prompt.contains("Monitor task field guidance"));
        assert!(prompt.contains("what should I do next?"));
        assert!(prompt.contains("Reply with Friday availability"));
        assert!(prompt.contains("Same chat/contact is not enough"));
        assert!(prompt.contains("Monitor source provenance policy"));
        assert!(prompt.contains("current workflow trigger or current digest triggers"));
        assert!(prompt.contains("Task source provenance is server-owned"));
        assert!(prompt.contains("Typed monitor task contract"));
        assert!(prompt.contains("daemon derives `metadata.monitor.schema_version=2`"));
        assert!(prompt.contains("telegram.reply"));
        assert!(prompt.contains("gmail.reply"));
        assert!(prompt.contains("calendar.rsvp"));
        assert!(prompt.contains("generic.review"));
        assert!(prompt.contains("source_context"));
        assert!(prompt.contains("pending_action"));
        assert!(prompt.contains("daemon-stamped"));
        assert!(prompt.contains("do not retry by changing source metadata"));
        assert!(prompt.contains("avoid repeated equivalent queries"));
        assert!(prompt.contains("Use the same language as the source message's primary language"));
        assert!(prompt.contains("subject, description, actions[].actionPrompt"));
        assert!(prompt.contains("If the source message is mixed-language"));
        assert!(prompt.contains("English source messages"));
        assert!(prompt.contains("Conversation context policy"));
        assert!(prompt.contains("conversation_context.messages"));
        assert!(prompt.contains("bounded recent Telegram server-history messages"));
        assert!(prompt.contains("best-effort observed subscriber diagnostics"));
        assert!(prompt.contains("Prior messages only explain the current message's intent"));
        assert!(prompt.contains("Preserve explicit product names"));
        assert!(!prompt.contains("Telegram monitoring handles"));
    }

    #[test]
    fn monitor_prompt_allows_direction_aware_completion() {
        let tempdir = tempfile::tempdir().unwrap();
        let memory_path = tempdir.path().join("telegram-user.md");
        let prompt = monitor_triage_prompt(
            "telegram-user",
            "telegram-login",
            "Personal Telegram",
            &memory_path,
            &["telegram@alice".to_string()],
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
    fn monitor_refresh_updates_prompt_without_dropping_filters() {
        let tempdir = tempfile::tempdir().unwrap();
        let old_path = tempdir.path().join("old.md");
        let new_path = tempdir.path().join("new.md");
        let mut binding = monitor_binding(
            "feed-connection",
            "feed",
            "old feed",
            &old_path,
            Some("openai/gpt-5.4"),
            vec!["google@alerts@example.com".to_string()],
        )
        .unwrap();
        binding
            .ignore_filters
            .push(puffer_subscriptions::FilterSpec::Json(json!({
                "room_id": "security-room",
                "author_handle": "alert-bot"
            })));

        refresh_monitor_binding(
            &mut binding,
            "feed-connection",
            "feed",
            "updated feed",
            &new_path,
            &MonitorModelUpdate::Preserve,
            &MonitorContactUpdate::Preserve,
        )
        .unwrap();

        assert_eq!(binding.ignore_filters.len(), 1);
        assert_eq!(binding.contact_ids, vec!["google@alerts@example.com"]);
        match binding.action {
            ActionSpec::TriageAgent { prompt, model } => {
                assert!(prompt.contains("updated feed"));
                assert!(prompt.contains("Never infer source identity"));
                assert!(prompt.contains("Do not add any metadata field named"));
                assert!(!prompt.contains("Telegram monitoring handles"));
                assert_eq!(model.as_deref(), Some("openai/gpt-5.4"));
            }
            _ => panic!("expected triage agent"),
        }
    }

    #[test]
    fn monitor_refresh_can_replace_or_clear_model() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("memory.md");
        let mut binding = monitor_binding(
            "feed-connection",
            "feed",
            "old feed",
            &path,
            Some("openai/gpt-5.4"),
            Vec::new(),
        )
        .unwrap();

        refresh_monitor_binding(
            &mut binding,
            "feed-connection",
            "feed",
            "updated feed",
            &path,
            &MonitorModelUpdate::Set(Some("anthropic/claude-sonnet-4-5".to_string())),
            &MonitorContactUpdate::Set(vec!["telegram@alice".to_string()]),
        )
        .unwrap();

        assert_eq!(
            monitor_action_model(&binding.action).as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
        assert_eq!(binding.contact_ids, vec!["telegram@alice"]);

        refresh_monitor_binding(
            &mut binding,
            "feed-connection",
            "feed",
            "updated feed",
            &path,
            &MonitorModelUpdate::Set(None),
            &MonitorContactUpdate::Set(Vec::new()),
        )
        .unwrap();

        assert_eq!(monitor_action_model(&binding.action), None);
        assert!(binding.contact_ids.is_empty());
    }

    #[test]
    fn telegram_monitor_contact_update_resolves_usernames_to_numeric_ids() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let account_dir = paths
            .user_config_dir
            .join("telegram-accounts")
            .join("telegram-user");
        fs::create_dir_all(&account_dir).unwrap();
        fs::write(
            account_dir.join("peer-cache.json"),
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "peers": [{
                    "id": "8689648954",
                    "numeric_id": 8689648954_i64,
                    "kind": "user",
                    "title": "dawei",
                    "username": "daweitaozi",
                    "is_bot": false
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let update = expand_monitor_contact_update(
            &paths,
            "telegram-user",
            "telegram-login",
            MonitorContactUpdate::Set(vec![
                "telegram@daweitaozi".to_string(),
                "telegram-user-id@8759047281".to_string(),
            ]),
        );

        assert_eq!(
            update,
            MonitorContactUpdate::Set(vec![
                "telegram-user-id@8689648954".to_string(),
                "telegram-user-id@8759047281".to_string()
            ])
        );
    }
}
