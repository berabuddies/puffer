use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{
    connection_workflow_trigger_supported, ActionSpec, ConnectionRecord, ConnectionState,
    WorkflowBindingSpec, WorkflowBindingStatus,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct MonitorCreateParams {
    #[serde(alias = "connectionSlug")]
    connection_slug: String,
    #[serde(default, deserialize_with = "deserialize_model_update")]
    model: Option<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MonitorModelUpdate {
    Preserve,
    Set(Option<String>),
}

/// Creates or resumes a monitor workflow binding and returns the refreshed snapshot.
pub(crate) fn handle_monitor_create(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let params: MonitorCreateParams =
        serde_json::from_value(params.clone()).context("invalid monitor create params")?;
    let connection_slug = normalized_connection_slug(&params.connection_slug)?;
    let model_update = normalized_model_update(params.model);
    create_or_resume_monitor(paths, &connection_slug, model_update)?;
    super::handle_workflow_list(paths)
}

fn create_or_resume_monitor(
    paths: &ConfigPaths,
    connection_slug: &str,
    model_update: MonitorModelUpdate,
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
            )?;
            binding
        }
        None => monitor_binding(
            connection_slug,
            &connection.connector_slug,
            &template.description,
            &memory_path,
            model_update.model(None).as_deref(),
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
) -> Result<WorkflowBindingSpec> {
    Ok(WorkflowBindingSpec {
        slug: monitor_slug(connection_slug),
        description: format!("Monitor {connection_slug} for actionable tasks"),
        connection_slug: connection_slug.to_string(),
        connector_slug: Some(connector_slug.to_string()),
        status: super::workflow_status(true),
        filter: None,
        ignore_filters: Vec::new(),
        classify_prompt: None,
        classify_model: None,
        action: monitor_action(
            connection_slug,
            connector_slug,
            connector_description,
            memory_path,
            model,
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
) -> Result<()> {
    let model = model_update.model(monitor_action_model(&binding.action));
    binding.description = format!("Monitor {connection_slug} for actionable tasks");
    binding.connection_slug = connection_slug.to_string();
    binding.connector_slug = Some(connector_slug.to_string());
    binding.action = monitor_action(
        connection_slug,
        connector_slug,
        connector_description,
        memory_path,
        model.as_deref(),
    )?;
    Ok(())
}

fn monitor_action(
    connection_slug: &str,
    connector_slug: &str,
    connector_description: &str,
    memory_path: &Path,
    model: Option<&str>,
) -> Result<ActionSpec> {
    let action_prompt = monitor_triage_prompt(
        connection_slug,
        connector_slug,
        connector_description,
        memory_path,
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
) -> String {
    format!(
        "You are the background monitor triage agent for connection `{connection_slug}` ({connector_slug}). Connector description: {connector_description}\n\nFor every new connector event:\n1. Read `{}` if it exists and use it only as task-creation guidance.\n2. If the event matches ignore memory, do not create a task; briefly report that it was ignored. Do not edit memory or subscription filters.\n3. Muted or silent notification events are filtered before this agent runs. If an event payload still says `notification_muted` or `notification_silent`, do not create a task.\n4. Otherwise decide whether the event represents an ongoing actionable task based on the connector description, event text, and structured payload.\n5. Use TaskList first to avoid duplicates. Use TaskCreate for new tasks and TaskUpdate for materially changed existing monitor tasks.\n6. Every monitor TaskCreate MUST include `receivedAt` from the workflow trigger's RFC3339 `receivedAt` field and an RFC3339 `expiresAt` chosen from the event urgency. If no better deadline is evident, set `expiresAt` 24 hours after `receivedAt`.\n7. Every monitor TaskCreate MUST include metadata with `_monitor: true`, `monitor_connection: \"{connection_slug}\"`, `monitor_connector: \"{connector_slug}\"`, and `monitor_memory_path: \"{}\"`, and `monitor_envelope_id` copied from the workflow trigger's `envelope_id`.\n8. If the structured event payload contains stable scalar source identity fields, copy those exact key/value pairs into top-level task metadata using the original payload key names. Stable identity means durable scope fields such as `chat_id`, `channel_id`, `room_id`, `conversation_id`, `thread_id`, `mailbox_id`, or `project_id`, paired with durable actor/source fields such as `sender_id`, `sender_username`, `from_email`, `author_id`, `author_handle`, or `account_id`.\n9. Never infer source identity from unstructured text. Do not use per-event fields such as `message_id`, `event_id`, `dedup_key`, timestamps, message text, body, subject, content, or title as ignore identity metadata.\n10. Do not add any metadata field named `monitor_ignore_filter`, `event_ignore_filter`, or `ignore_filter`; ignore filter installation is daemon-owned.\n11. Every monitor TaskCreate SHOULD include `actions`: an array of objects with `actionName` and `actionPrompt`, and `possibleIgnoreReasons`: a short array of suggested ignore reasons.\n12. Keep action prompts ready to send to the current coding agent. Include enough source context from the connector event for the agent to act without rereading the whole stream.\n\nDo not send connector replies unless a selected action later asks for it.",
        memory_path.display(),
        memory_path.display()
    )
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
        );

        assert!(prompt.contains("TaskCreate MUST include metadata"));
        assert!(prompt.contains("monitor_connection: \"telegram-user\""));
        assert!(prompt.contains("monitor_connector: \"telegram-login\""));
        assert!(prompt.contains("monitor_envelope_id"));
        assert!(prompt.contains("Never infer source identity"));
        assert!(prompt.contains("Do not add any metadata field named"));
        assert!(!prompt.contains("Telegram monitoring handles"));
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
        )
        .unwrap();

        assert_eq!(binding.ignore_filters.len(), 1);
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
        )
        .unwrap();

        refresh_monitor_binding(
            &mut binding,
            "feed-connection",
            "feed",
            "updated feed",
            &path,
            &MonitorModelUpdate::Set(Some("anthropic/claude-sonnet-4-5".to_string())),
        )
        .unwrap();

        assert_eq!(
            monitor_action_model(&binding.action).as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );

        refresh_monitor_binding(
            &mut binding,
            "feed-connection",
            "feed",
            "updated feed",
            &path,
            &MonitorModelUpdate::Set(None),
        )
        .unwrap();

        assert_eq!(monitor_action_model(&binding.action), None);
    }
}
