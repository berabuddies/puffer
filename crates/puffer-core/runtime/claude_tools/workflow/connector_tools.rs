//! Connector and connection workflow tools.

use super::store::{load_store, monitor_tasks_path, now_ms, save_store, StoredTask, TaskStore};
use crate::runtime::subscription_manager;
use crate::AppState;
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::{Event, EventEnvelope};
use puffer_subscriptions::{
    append_gate_audit, connector_runtime_hints, connector_workflow_trigger_supported,
    suggested_connection_slug, ActionDispatcher, ActionSpec, AuditEntry, BuiltinActionDispatcher,
    ConnectionAuthStatus, ConnectionRecord, ConnectionState, ConnectorActionRequest,
    ConnectorTemplate, GateDecision, NewOutboundDraft, OutboundOrigin, OutboundStore,
    RecipientSource, SendOrigin, SubscriberManifestRoots, AUDIT_DECISION_DRAFT_REQUIRED,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
struct ConnectorSlugInput {
    connector_slug: String,
}

#[derive(Debug, Deserialize)]
struct ConnectorRegisterInput {
    #[serde(default)]
    template: Option<ConnectorTemplate>,
    #[serde(default)]
    connector_slug: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    skill: Option<String>,
    #[serde(default)]
    binary: Option<String>,
    #[serde(default)]
    command: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ConnectorUpdateInput {
    connector_slug: String,
    #[serde(default)]
    new_skill: Option<String>,
    #[serde(default)]
    template: Option<ConnectorTemplate>,
}

#[derive(Debug, Deserialize)]
struct ConnectorActInput {
    connector_slug: String,
    #[serde(
        default,
        alias = "connection",
        alias = "account",
        alias = "account_slug"
    )]
    connection_slug: Option<String>,
    action: String,
    #[serde(default)]
    input: Value,
}

#[derive(Debug, Deserialize)]
struct ConnectorActionDraftInput {
    connector_slug: String,
    #[serde(
        default,
        alias = "connection",
        alias = "account",
        alias = "account_slug"
    )]
    connection_slug: Option<String>,
    action: String,
    #[serde(default, rename = "taskId", alias = "task_id")]
    task_id: Option<String>,
    #[serde(default)]
    input: Value,
}

#[derive(Debug, Deserialize)]
struct ConnectionCreateInput {
    #[serde(alias = "subscription_slug")]
    slug: String,
    connector_slug: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    auth_ok: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ConnectionDeleteInput {
    #[serde(alias = "subscription_slug")]
    slug: String,
}

/// Executes `ConnectorList`.
pub fn execute_connector_list(_state: &mut AppState, cwd: &Path, _input: Value) -> Result<String> {
    let manager = subscription_manager()?;
    let roots = subscriber_manifest_roots(cwd);
    let connectors = manager
        .connector_store()
        .list_with_builtins()
        .into_iter()
        .map(|template| connector_list_row(template, &roots))
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(
        &json!({ "connectors": connectors }),
    )?)
}

fn connector_list_row(template: ConnectorTemplate, roots: &SubscriberManifestRoots) -> Value {
    let suggested_connection = suggested_connection_slug(&template.slug);
    let connect_command = format!("/connect {} {}", template.slug, suggested_connection);
    let can_trigger_workflow = connector_workflow_trigger_supported(roots, &template);
    json!({
        "connector_slug": template.slug,
        "description": template.description,
        "skill": template.skill,
        "binary": template.binary,
        "command": template.command,
        "runtime_hints": connector_runtime_hints(roots, &template),
        "requires_auth": template.requires_auth,
        "can_subscribe": template.can_subscribe,
        "can_proxy_agent": template.can_proxy_agent,
        "suggested_connection_slug": suggested_connection,
        "connect_command": connect_command,
        "can_trigger_workflow": can_trigger_workflow,
        "actions": template.actions,
    })
}

fn subscriber_manifest_roots(cwd: &Path) -> SubscriberManifestRoots {
    let paths = ConfigPaths::discover(cwd);
    SubscriberManifestRoots::new(
        paths.workspace_config_dir,
        paths.user_config_dir,
        paths.builtin_resources_dir,
    )
}

/// Executes `ConnectorCreation`.
pub fn execute_connector_creation(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    execute_connector_template(state, cwd, input)
}

/// Executes `ConnectorTemplate`.
pub fn execute_connector_template(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: ConnectorSlugInput =
        serde_json::from_value(input).context("invalid ConnectorTemplate input")?;
    let manager = subscription_manager()?;
    let template = manager
        .connector_store()
        .get(&parsed.connector_slug)
        .unwrap_or_else(|| starter_template(&parsed.connector_slug));
    let skill = connector_skill_template(&template);
    let python_program = connector_python_template(&template);
    Ok(serde_json::to_string_pretty(&json!({
        "template": template,
        "skill": skill,
        "python_program": python_program,
    }))?)
}

/// Executes `ConnectorRegister`.
pub fn execute_connector_register(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: ConnectorRegisterInput =
        serde_json::from_value(input).context("invalid ConnectorRegister input")?;
    let template = parsed.template.unwrap_or_else(|| ConnectorTemplate {
        slug: parsed.connector_slug.unwrap_or_default(),
        description: parsed.description.unwrap_or_default(),
        skill: parsed.skill.unwrap_or_default(),
        binary: parsed.binary.unwrap_or_default(),
        command: parsed.command.unwrap_or_default(),
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: json!({}),
        actions: Default::default(),
    });
    let manager = subscription_manager()?;
    let registered = manager.connector_store().upsert(template)?;
    manager.refresh_connection_consumers()?;
    Ok(serde_json::to_string_pretty(&registered)?)
}

/// Executes `ConnectorUpdate`.
pub fn execute_connector_update(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: ConnectorUpdateInput =
        serde_json::from_value(input).context("invalid ConnectorUpdate input")?;
    let manager = subscription_manager()?;
    let mut template = parsed
        .template
        .or_else(|| manager.connector_store().get(&parsed.connector_slug))
        .ok_or_else(|| anyhow::anyhow!("connector `{}` not found", parsed.connector_slug))?;
    template.slug = parsed.connector_slug;
    if let Some(skill) = parsed.new_skill {
        template.skill = skill;
    }
    let updated = manager.connector_store().upsert(template)?;
    manager.refresh_connection_consumers()?;
    Ok(serde_json::to_string_pretty(&updated)?)
}

/// Executes `ConnectorDelete`.
pub fn execute_connector_delete(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: ConnectorSlugInput =
        serde_json::from_value(input).context("invalid ConnectorDelete input")?;
    let manager = subscription_manager()?;
    manager.connector_store().delete(&parsed.connector_slug)?;
    manager.refresh_connection_consumers()?;
    Ok(serde_json::to_string_pretty(&json!({
        "deleted": true,
        "connector_slug": parsed.connector_slug,
    }))?)
}

/// Executes `ConnectorAct`.
pub fn execute_connector_act(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let dispatcher = BuiltinActionDispatcher::new();
    execute_connector_act_with_dispatcher(state, cwd, input, &dispatcher)
}

fn execute_connector_act_with_dispatcher(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
    dispatcher: &dyn ActionDispatcher,
) -> Result<String> {
    let parsed: ConnectorActInput =
        serde_json::from_value(input).context("invalid ConnectorAct input")?;
    let manager = subscription_manager()?;
    let template = manager
        .connector_store()
        .get(&parsed.connector_slug)
        .ok_or_else(|| anyhow::anyhow!("connector `{}` not found", parsed.connector_slug))?;
    let action_definition = template.actions.get(&parsed.action).ok_or_else(|| {
        anyhow::anyhow!(
            "connector `{}` does not define action `{}`",
            parsed.connector_slug,
            parsed.action
        )
    })?;
    let origin = SendOrigin::LlmInitiated {
        session_id: state.session.id.to_string(),
        turn_id: state
            .monitor_reply_scope
            .as_ref()
            .map(|scope| scope.turn_id.clone()),
        task_id: state
            .monitor_reply_scope
            .as_ref()
            .map(|scope| scope.task_id.clone()),
    };
    let decision = puffer_subscriptions::outbound_gate::evaluate(
        &origin,
        &parsed.connector_slug,
        &template,
        &parsed.action,
    );
    let paths = ConfigPaths::discover(cwd);
    append_gate_audit(
        &outbound_audit_path(&paths),
        &audit_entry_for(
            &origin,
            &parsed.connector_slug,
            &parsed.action,
            &decision,
            None,
        ),
    );
    if matches!(decision, GateDecision::RequiresDraft) {
        anyhow::bail!(
            "connector action `{}`/`{}` sends an external message and requires human review; create a draft with ConnectorActionDraft instead",
            parsed.connector_slug,
            parsed.action
        );
    }
    let connection = parsed
        .connection_slug
        .clone()
        .or_else(|| {
            parsed
                .input
                .get("connection_slug")
                .or_else(|| parsed.input.get("account_slug"))
                .or_else(|| parsed.input.get("connection"))
                .or_else(|| parsed.input.get("account"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| parsed.connector_slug.clone());
    let mut action_input = parsed.input.clone();
    if let Some(object) = action_input.as_object_mut() {
        object
            .entry("connection_slug")
            .or_insert_with(|| Value::String(connection.clone()));
        object
            .entry("connector_slug")
            .or_insert_with(|| Value::String(parsed.connector_slug.clone()));
    }
    if parsed.action == "requestuserbrowseraction" {
        let output = super::request_user_browser_action::execute_request_user_browser_action(
            state,
            cwd,
            action_input,
        )?;
        let output_value: Value =
            serde_json::from_str(&output).context("parse requestuserbrowseraction output")?;
        return Ok(serde_json::to_string_pretty(&json!({
            "success": true,
            "summary": "requested user browser action",
            "output": output_value,
            "retryable": false,
            "permission": action_definition.permission,
        }))?);
    }
    let request = ConnectorActionRequest {
        connection: connection.clone(),
        action: parsed.action.clone(),
        input: action_input.clone(),
        idempotency_key: None,
    };
    if let Some(response) = manager.run_connector_action(&template, &request)? {
        if !response.success {
            anyhow::bail!("{} [retryable={}]", response.summary, response.retryable);
        }
        return Ok(serde_json::to_string_pretty(&json!({
            "success": response.success,
            "summary": response.summary,
            "output": response.output,
            "retryable": response.retryable,
            "permission": action_definition.permission,
        }))?);
    }
    let envelope = synthetic_envelope(&connection, &action_input);
    let result = dispatcher.dispatch(
        &ActionSpec::ConnectorAct {
            connector_slug: parsed.connector_slug.clone(),
            action: parsed.action.clone(),
            input: action_input,
        },
        &envelope,
    );
    if !result.success {
        anyhow::bail!("{}", result.summary);
    }
    Ok(serde_json::to_string_pretty(&json!({
        "success": result.success,
        "summary": result.summary,
        "permission": action_definition.permission,
    }))?)
}

/// Executes `ConnectorActionDraft`.
pub fn execute_connector_action_draft(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: ConnectorActionDraftInput =
        serde_json::from_value(input).context("invalid ConnectorActionDraft input")?;
    let manager = subscription_manager()?;

    // Resolve the effective connector, action, connection, and recipient. When a
    // task id is present these are all stamped from the monitor task source, so
    // the model's connector_slug/action/recipient inputs are advisory only and
    // cannot redirect the draft to a different destination.
    let mut action_input = parsed.input.clone();
    let mut recipient_source = RecipientSource::Model;
    let (connector_slug, action, connection, recipient_stable_id) =
        if let Some(task_id) = parsed.task_id.as_deref() {
            ensure_task_id_matches_scope(state, task_id)?;
            let task = load_monitor_task_for_draft(state.session.cwd.as_path(), task_id)?;
            let target = monitor_reply_target(&task)?;
            if let Some(object) = action_input.as_object_mut() {
                target.stamp_input(object);
            }
            recipient_source = RecipientSource::Stamped;
            (
                target.connector_slug().to_string(),
                target.action().to_string(),
                target.connection_slug().to_string(),
                target.recipient_stable_id(),
            )
        } else {
            let connection = connector_action_connection(
                &parsed.connector_slug,
                parsed.connection_slug.as_deref(),
                &parsed.input,
            );
            if let Some(object) = action_input.as_object_mut() {
                object
                    .entry("connection_slug")
                    .or_insert_with(|| Value::String(connection.clone()));
                object
                    .entry("connector_slug")
                    .or_insert_with(|| Value::String(parsed.connector_slug.clone()));
            }
            let recipient_stable_id = draft_message_target(&action_input)
                .context("ConnectorActionDraft requires a send recipient")?;
            (
                parsed.connector_slug.clone(),
                parsed.action.clone(),
                connection,
                recipient_stable_id,
            )
        };

    let template = manager
        .connector_store()
        .get(&connector_slug)
        .or_else(|| puffer_subscriptions::builtin_connector_template(&connector_slug))
        .ok_or_else(|| anyhow::anyhow!("connector `{}` not found", connector_slug))?;
    let _action_definition = template.actions.get(&action).ok_or_else(|| {
        anyhow::anyhow!(
            "connector `{}` does not define action `{}`",
            connector_slug,
            action
        )
    })?;
    let origin = SendOrigin::LlmInitiated {
        session_id: state.session.id.to_string(),
        turn_id: state
            .monitor_reply_scope
            .as_ref()
            .map(|scope| scope.turn_id.clone()),
        task_id: parsed.task_id.clone(),
    };
    let decision =
        puffer_subscriptions::outbound_gate::evaluate(&origin, &connector_slug, &template, &action);
    if matches!(decision, GateDecision::Allowed { .. }) {
        let paths = ConfigPaths::discover(cwd);
        append_gate_audit(
            &outbound_audit_path(&paths),
            &audit_entry_for(&origin, &connector_slug, &action, &decision, None),
        );
        anyhow::bail!(
            "ConnectorActionDraft is only for external actions that require human review; use ConnectorAct for `{}`/`{}`",
            connector_slug,
            action
        );
    }

    ensure_connector_action_draft_connection(&manager, &connector_slug, &connection)?;
    let message = draft_message_text(&action_input)
        .context("ConnectorActionDraft requires a message body")?;
    let paths = ConfigPaths::discover(cwd);
    let store = OutboundStore::load(outbound_actions_path(&paths))?;
    let action_record = store.create_draft(NewOutboundDraft {
        connector_slug: connector_slug.clone(),
        connection_slug: connection.clone(),
        action: action.clone(),
        input: action_input,
        recipient_stable_id,
        recipient_source,
        message,
        origin: OutboundOrigin {
            session_id: state.session.id.to_string(),
            turn_id: state
                .monitor_reply_scope
                .as_ref()
                .map(|scope| scope.turn_id.clone()),
            task_id: parsed.task_id.clone(),
        },
        ttl_ms: None,
    })?;
    append_gate_audit(
        &outbound_audit_path(&paths),
        &audit_entry_for(
            &origin,
            &connector_slug,
            &action,
            &decision,
            Some(action_record.id.clone()),
        ),
    );
    if let Some(task_id) = parsed.task_id.as_deref() {
        write_outbound_action_reference(state.session.cwd.as_path(), task_id, &action_record.id)?;
    }

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "draft": {
            "id": action_record.id,
            "status": "draft_ready",
            "version": action_record.version,
            "connectorSlug": connector_slug,
            "connectionSlug": connection,
            "action": action,
            "recipientStableId": action_record.recipient_stable_id,
            "recipientSource": recipient_source_json(action_record.recipient_source),
            "message": action_record.message,
            "contentHash": action_record.content_hash,
            "taskId": action_record.origin.task_id,
        }
    }))?)
}

/// Executes `ConnectionList`.
pub fn execute_connection_list(
    _state: &mut AppState,
    _cwd: &Path,
    _input: Value,
) -> Result<String> {
    let manager = subscription_manager()?;
    let auth_notices = manager.refresh_connection_auth()?;
    manager.refresh_connection_consumers()?;
    let connections = manager.connection_store().list();
    Ok(serde_json::to_string_pretty(&json!({
        "connections": connections,
        "auth_notices": auth_notices.iter().map(|connection| json!({
            "slug": connection.slug,
            "connector_slug": connection.connector_slug,
            "message": format!(
                "Connection auth is no longer functioning; run `/connect {} {}` to repair it.",
                connection.connector_slug, connection.slug
            )
        })).collect::<Vec<_>>(),
    }))?)
}

/// Executes `ConnectionCreate`.
pub fn execute_connection_create(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: ConnectionCreateInput =
        serde_json::from_value(input).context("invalid ConnectionCreate input")?;
    let manager = subscription_manager()?;
    let template = manager
        .connector_store()
        .get(&parsed.connector_slug)
        .ok_or_else(|| anyhow::anyhow!("connector `{}` not found", parsed.connector_slug))?;
    let auth_status = if template.requires_auth {
        match manager.check_connection_auth(&template, &parsed.slug)? {
            Some(status) => Some(status),
            None => parsed.auth_ok.map(connection_auth_status_from_bool),
        }
    } else {
        Some(ConnectionAuthStatus::Healthy)
    };
    if template.requires_auth && auth_status == Some(ConnectionAuthStatus::Broken) {
        anyhow::bail!(
            "connector `{}` reported auth is not ready; run `/connect {} {}` first",
            parsed.connector_slug,
            parsed.connector_slug,
            parsed.slug
        );
    }
    let mut record =
        ConnectionRecord::authenticated(parsed.slug, parsed.connector_slug, parsed.description);
    record.state = ConnectionState::Authenticated;
    manager.connection_store().create(record.clone())?;
    manager.refresh_connection_consumers()?;
    manager.refresh_connection_auth()?;
    Ok(serde_json::to_string_pretty(&record)?)
}

fn connection_auth_status_from_bool(ok: bool) -> ConnectionAuthStatus {
    if ok {
        ConnectionAuthStatus::Healthy
    } else {
        ConnectionAuthStatus::Broken
    }
}

fn connector_action_connection(
    connector_slug: &str,
    connection_slug: Option<&str>,
    input: &Value,
) -> String {
    connection_slug
        .map(ToString::to_string)
        .or_else(|| {
            input
                .get("connection_slug")
                .or_else(|| input.get("account_slug"))
                .or_else(|| input.get("connection"))
                .or_else(|| input.get("account"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| connector_slug.to_string())
}

fn ensure_connector_action_draft_connection(
    manager: &puffer_subscriptions::SubscriptionManager,
    connector_slug: &str,
    connection_slug: &str,
) -> Result<()> {
    let connection = manager
        .connection_store()
        .get(connection_slug)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "connection `{connection_slug}` is not connected; run `/connect {connector_slug} {connection_slug}` before drafting a message"
            )
        })?;
    if connection.connector_slug != connector_slug {
        anyhow::bail!(
            "connection `{connection_slug}` uses connector `{}`, not `{connector_slug}`",
            connection.connector_slug
        );
    }
    if !matches!(
        connection.state,
        ConnectionState::Authenticated | ConnectionState::Active
    ) {
        anyhow::bail!(
            "connection `{connection_slug}` is not connected (state: {:?}); run `/connect {connector_slug} {connection_slug}` before drafting a message",
            connection.state
        );
    }
    Ok(())
}

/// Executes `ConnectionDelete`.
pub fn execute_connection_delete(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: ConnectionDeleteInput =
        serde_json::from_value(input).context("invalid ConnectionDelete input")?;
    let manager = subscription_manager()?;
    manager.connection_store().delete(&parsed.slug)?;
    manager.refresh_connection_consumers()?;
    Ok(serde_json::to_string_pretty(&json!({
        "deleted": true,
        "slug": parsed.slug,
    }))?)
}

fn starter_template(slug: &str) -> ConnectorTemplate {
    ConnectorTemplate {
        slug: slug.to_string(),
        description: format!("{slug} connector"),
        skill: slug.to_string(),
        binary: format!("puffer-connector-{slug}"),
        command: vec![format!("puffer-connector-{slug}")],
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        subscriber: None,
        output_schema: json!({
            "type": "object",
            "properties": {"message": {"type": "string"}},
            "required": ["message"]
        }),
        actions: Default::default(),
    }
}

fn synthetic_envelope(topic: &str, payload: &Value) -> EventEnvelope {
    EventEnvelope {
        envelope_id: format!(
            "connector-act-{}",
            OffsetDateTime::now_utc().unix_timestamp_nanos()
        ),
        subscriber_id: topic.to_string(),
        received_at_ms: OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000,
        event: Event {
            topic: topic.to_string(),
            kind: "connector_action".to_string(),
            control: false,
            dedup_key: None,
            text: payload
                .get("message")
                .or_else(|| payload.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            payload: payload.clone(),
        },
    }
}

fn outbound_actions_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("outbound_actions.json")
}

fn outbound_audit_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("outbound_audit.ndjson")
}

fn audit_entry_for(
    origin: &SendOrigin,
    connector: &str,
    action: &str,
    decision: &GateDecision,
    action_id: Option<String>,
) -> AuditEntry {
    AuditEntry {
        origin: match origin {
            SendOrigin::LlmInitiated { .. } => "llm".to_string(),
            SendOrigin::RuleAutomation { .. } => "rule".to_string(),
        },
        connector: connector.to_string(),
        action: action.to_string(),
        decision: gate_decision_label(decision).to_string(),
        action_id,
        rule_id: match origin {
            SendOrigin::RuleAutomation { rule_id } => Some(rule_id.clone()),
            SendOrigin::LlmInitiated { .. } => None,
        },
    }
}

fn gate_decision_label(decision: &GateDecision) -> &'static str {
    match decision {
        GateDecision::RequiresDraft => AUDIT_DECISION_DRAFT_REQUIRED,
        GateDecision::Allowed { reason } => reason,
    }
}

fn recipient_source_json(source: RecipientSource) -> &'static str {
    match source {
        RecipientSource::Stamped => "stamped",
        RecipientSource::Model => "model",
    }
}

fn ensure_task_id_matches_scope(state: &AppState, task_id: &str) -> Result<()> {
    let Some(scope) = state.monitor_reply_scope.as_ref() else {
        anyhow::bail!(
            "ConnectorActionDraft taskId `{task_id}` requires a scoped monitor task turn"
        );
    };
    if scope.task_id != task_id {
        anyhow::bail!(
            "ConnectorActionDraft taskId `{task_id}` does not match scoped monitor task `{}`",
            scope.task_id
        );
    }
    Ok(())
}

fn load_monitor_task_for_draft(cwd: &Path, task_id: &str) -> Result<StoredTask> {
    let store = load_store::<TaskStore>(&monitor_tasks_path(cwd))?;
    store
        .tasks
        .into_iter()
        .find(|task| task.task_id == task_id)
        .ok_or_else(|| anyhow::anyhow!("monitor task `{task_id}` not found"))
}

fn write_outbound_action_reference(cwd: &Path, task_id: &str, action_id: &str) -> Result<()> {
    let path = monitor_tasks_path(cwd);
    let mut store = load_store::<TaskStore>(&path)?;
    let Some(task) = store.tasks.iter_mut().find(|task| task.task_id == task_id) else {
        anyhow::bail!("monitor task `{task_id}` not found");
    };
    task.metadata.insert(
        "outbound_action_id".to_string(),
        Value::String(action_id.to_string()),
    );
    task.updated_at_ms = Some(now_ms());
    save_store(&path, &store)
}

fn draft_message_target(input: &Value) -> Option<String> {
    first_draft_message_value(
        input,
        &[
            "to",
            "target",
            "channel",
            "chat_id",
            "open_id",
            "user",
            "receive_id",
        ],
        true,
    )
}

fn draft_message_text(input: &Value) -> Option<String> {
    first_draft_message_value(input, &["message", "text", "caption", "body"], false)
}

fn first_draft_message_value(input: &Value, keys: &[&str], accept_numbers: bool) -> Option<String> {
    keys.iter()
        .filter_map(|key| input.get(*key))
        .find_map(|value| draft_message_value(value, accept_numbers))
}

fn draft_message_value(value: &Value, accept_numbers: bool) -> Option<String> {
    if let Some(text) = value.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if accept_numbers && value.is_number() {
        return Some(value.to_string());
    }
    None
}

/// A monitor reply target resolved from the task source. The connector,
/// connection, reply action, and recipient are all stamped from here so the
/// model can never redirect a task-scoped draft to a different destination.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MonitorReplyTarget {
    Telegram {
        connector_slug: String,
        connection_slug: String,
        chat_id: String,
    },
    Gmail {
        connector_slug: String,
        connection_slug: String,
        thread_id: String,
        message_id: Option<String>,
        account: Option<String>,
    },
}

impl MonitorReplyTarget {
    fn connector_slug(&self) -> &str {
        match self {
            Self::Telegram { connector_slug, .. } | Self::Gmail { connector_slug, .. } => {
                connector_slug
            }
        }
    }

    fn connection_slug(&self) -> &str {
        match self {
            Self::Telegram {
                connection_slug, ..
            }
            | Self::Gmail {
                connection_slug, ..
            } => connection_slug,
        }
    }

    /// The canonical reply action for this target. Task-scoped drafts always use
    /// this action regardless of what the model passed, so a Gmail task always
    /// drafts a `draft_reply` and a Telegram task always drafts a `send_message`.
    fn action(&self) -> &'static str {
        match self {
            Self::Telegram { .. } => "send_message",
            Self::Gmail { .. } => "draft_reply",
        }
    }

    fn recipient_stable_id(&self) -> String {
        match self {
            Self::Telegram { chat_id, .. } => format!("telegram:{chat_id}"),
            Self::Gmail { thread_id, .. } => format!("gmail:{thread_id}"),
        }
    }

    /// Removes any model-supplied recipient/target keys and stamps the resolved
    /// destination onto the connector action input.
    fn stamp_input(&self, object: &mut serde_json::Map<String, Value>) {
        for key in [
            "to",
            "target",
            "channel",
            "chat_id",
            "open_id",
            "user",
            "receive_id",
            "thread_id",
            "gmail_thread_id",
            "message_id",
            "id",
            "account",
        ] {
            object.remove(key);
        }
        object.insert(
            "connection_slug".to_string(),
            Value::String(self.connection_slug().to_string()),
        );
        object.insert(
            "connector_slug".to_string(),
            Value::String(self.connector_slug().to_string()),
        );
        match self {
            Self::Telegram { chat_id, .. } => {
                object.insert("chat_id".to_string(), Value::String(chat_id.clone()));
            }
            Self::Gmail {
                thread_id,
                message_id,
                account,
                ..
            } => {
                object.insert("thread_id".to_string(), Value::String(thread_id.clone()));
                if let Some(message_id) = message_id {
                    object.insert("message_id".to_string(), Value::String(message_id.clone()));
                }
                if let Some(account) = account {
                    object.insert("account".to_string(), Value::String(account.clone()));
                }
            }
        }
    }
}

fn monitor_reply_target(task: &StoredTask) -> Result<MonitorReplyTarget> {
    if !is_monitor_task_metadata(&task.metadata) {
        anyhow::bail!("task `{}` is not a monitor task", task.task_id);
    }
    let source_context = monitor_source_context(&task.metadata)
        .ok_or_else(|| anyhow::anyhow!("monitor task `{}` has no source_context", task.task_id))?;
    let Some(context) = source_context.as_object() else {
        anyhow::bail!(
            "monitor task `{}` source_context is not an object",
            task.task_id
        );
    };
    let connector_slug = string_field_from_map(context, &["connector_slug", "connectorSlug"])
        .or_else(|| metadata_string(&task.metadata, &["monitor_connector", "monitorConnector"]))
        .ok_or_else(|| anyhow::anyhow!("monitor task `{}` has no connector slug", task.task_id))?;
    let connection_slug = string_field_from_map(context, &["connection_slug", "connectionSlug"])
        .or_else(|| metadata_string(&task.metadata, &["monitor_connection", "monitorConnection"]))
        .unwrap_or_else(|| connector_slug.clone());
    let delivery_target = source_context_delivery_target(&source_context)
        .ok_or_else(|| anyhow::anyhow!("monitor task `{}` has no delivery target", task.task_id))?;
    let Some(delivery_target) = delivery_target.as_object() else {
        anyhow::bail!(
            "monitor task `{}` delivery target is not an object",
            task.task_id
        );
    };
    let target_type = string_field_from_map(delivery_target, &["type"]);
    match target_type.as_deref() {
        Some("telegram_chat") => {
            let chat_id = string_field_from_map(delivery_target, &["chat_id", "chatId"])
                .ok_or_else(|| {
                    anyhow::anyhow!("monitor task `{}` has no Telegram chat_id", task.task_id)
                })?;
            Ok(MonitorReplyTarget::Telegram {
                connector_slug,
                connection_slug,
                chat_id,
            })
        }
        Some("gmail_thread") => {
            let thread_id = string_field_from_map(delivery_target, &["thread_id", "threadId"])
                .ok_or_else(|| {
                    anyhow::anyhow!("monitor task `{}` has no Gmail thread_id", task.task_id)
                })?;
            let message_id = string_field_from_map(delivery_target, &["message_id", "messageId"]);
            let account = string_field_from_map(
                delivery_target,
                &["account", "account_id", "accountId"],
            );
            Ok(MonitorReplyTarget::Gmail {
                connector_slug,
                connection_slug,
                thread_id,
                message_id,
                account,
            })
        }
        other => anyhow::bail!(
            "ConnectorActionDraft supports telegram_chat and gmail_thread monitor task replies only, got {}",
            other.unwrap_or("<missing>")
        ),
    }
}

fn is_monitor_task_metadata(metadata: &serde_json::Map<String, Value>) -> bool {
    metadata
        .get("_monitor")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || metadata.contains_key("monitor_connection")
        || metadata.contains_key("monitorConnection")
}

fn monitor_source_context(metadata: &serde_json::Map<String, Value>) -> Option<Value> {
    metadata
        .get("source_context")
        .or_else(|| metadata.get("sourceContext"))
        .cloned()
        .or_else(|| derived_monitor_source_context(metadata))
}

fn derived_monitor_source_context(metadata: &serde_json::Map<String, Value>) -> Option<Value> {
    let connector_slug = metadata_string(metadata, &["monitor_connector", "monitorConnector"])?;
    if !connector_slug.contains("telegram") {
        return None;
    }
    let chat_id = metadata_string(metadata, &["chat_id", "chatId"])?;
    let chat_kind = metadata_string(metadata, &["chat_kind", "chatKind"])
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "user".to_string());
    let connection_slug = metadata_string(metadata, &["monitor_connection", "monitorConnection"]);
    Some(json!({
        "kind": "telegram_direct_message",
        "connection_slug": connection_slug,
        "connector_slug": connector_slug,
        "summary": format!("Telegram message from chat_id {chat_id}"),
        "delivery_target": {
            "type": "telegram_chat",
            "chat_id": chat_id,
            "chat_kind": chat_kind,
        },
    }))
}

fn source_context_delivery_target(context: &Value) -> Option<&Value> {
    context
        .get("delivery_target")
        .or_else(|| context.get("deliveryTarget"))
}

fn metadata_string(metadata: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| metadata.get(*key))
        .and_then(value_to_string)
}

fn string_field_from_map(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(value_to_string)
}

fn value_to_string(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    value
        .as_i64()
        .map(|number| number.to_string())
        .or_else(|| value.as_u64().map(|number| number.to_string()))
}

fn connector_skill_template(template: &ConnectorTemplate) -> String {
    format!(
        r#"# {description}

Use this guide when creating, registering, or operating the `{slug}` connector.

## Choose the Runtime Shape

- Use a connector protocol command when a standalone executable can implement
  `auth-ok`, `subscribe`, and `act` for one connection.
- Use a reusable subscriber manifest when multiple connections share one
  long-lived poller or browser session. Store per-connection state under the
  connector's `state_root`.
- Use an internal/tool-backed connector only when the host must own the runtime
  session, browser profile, or privileged API client.
- Keep platform-specific parsing, auth, and filtering inside the connector or
  subscriber. Do not add connector-specific behavior to the generic router.

## Template Contract

- `slug`: stable kebab-case connector id. `skill` should usually match it.
- `requires_auth`: true unless the connector can run without user credentials.
- `can_subscribe`: true only when workflow monitors can receive events.
- `subscriber`: set this when events come from a reusable manifest; otherwise
  command-backed connectors stream through `subscribe`.
- `output_schema`: describe emitted event payloads, including stable IDs,
  cursors, timestamps, sender/account identifiers, and target URLs when present.
- `actions`: define every action the agent may call. Each action needs an input
  schema, output schema, permission category, summary, and side-effect flag.

## Auth and Setup

- Route user-facing setup through `/connect <connector> <connection>` and
  standard AskUserQuestion/browser questions.
- `auth-ok <connection>` must be deterministic and safe to call repeatedly.
  Return boolean output or JSON with `ok`/`success`.
- Never import or mutate another app's live session unless the connector is
  explicitly designed to do that. Prefer an independent Puffer-owned session.
- Auth failures should be actionable: say which account/connection is broken
  and what setup step should be rerun.

## Streaming

- `subscribe` receives one JSON command on stdin:
  `{{"op":"subscribe","connection":"...","cursor":"..."}}`.
- Emit newline-delimited JSON frames only: `event`, `checkpoint`, or `health`.
- Every event must include a durable `id`, an ackable `cursor`, and a concise
  payload. Use monotonic provider cursors when available.
- Resume from the provided cursor. Avoid slow full backfills on restart.
- After the host sends `ack`, the connector may persist that cursor. Do not
  drop unacked events silently.

## Actions

- `act <connection> <action>` reads one JSON payload from stdin.
- Return JSON with `success`, `summary`, optional `output`, and `retryable`.
- Add list/search/read actions for any action that needs a target ID. Do not
  make agents guess IDs before `get_detail`, `reply`, `accept`, `deny`, etc.
- Side-effecting actions must use precise permissions and idempotency keys when
  the provider supports them.

## Verification

- Unit-test template metadata, auth-ok parsing, action routing, and event frame
  parsing.
- Add an update spec for each touched component.
- For stream connectors, test cursor resume, ack persistence, reconnects, and
  duplicate suppression.
- For browser-backed connectors, manually verify setup, list/search, detail,
  and one safe action against the daemon-managed browser profile.
"#,
        description = template.description,
        slug = template.slug
    )
}

fn connector_python_template(template: &ConnectorTemplate) -> String {
    format!(
        r#"#!/usr/bin/env python3
import json
import sys

CONNECTOR = {connector:?}

def auth(connection):
    print(json.dumps({{"ok": True, "connection": connection}}))

def auth_ok(connection):
    print(json.dumps({{"ok": True, "connection": connection}}))

def subscribe(connection, cursor=None):
    for line in sys.stdin:
        command = json.loads(line)
        if command.get("op") == "ack":
            continue

def act(connection, action, payload):
    print(json.dumps({{
        "success": True,
        "summary": f"{{CONNECTOR}}.{{action}} accepted for {{connection}}",
        "output": {{"completed": True}},
        "retryable": False,
    }}))

def main():
    op = sys.argv[1] if len(sys.argv) > 1 else ""
    if op == "auth":
        auth(sys.argv[2])
    elif op == "auth-ok":
        auth_ok(sys.argv[2])
    elif op == "subscribe":
        subscribe(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else None)
    elif op == "act":
        act(sys.argv[2], sys.argv[3], json.load(sys.stdin))
    else:
        raise SystemExit(f"unknown op {{op}}")

if __name__ == "__main__":
    main()
"#,
        connector = template.slug
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::{ensure_workspace_dirs, set_puffer_home_override, PufferConfig};
    use puffer_session_store::SessionStore;
    use puffer_subscriptions::{ActionResult, SubscriptionManagerBuilder};
    use std::fs;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    fn make_state() -> (AppState, TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tmp.path());
        ensure_workspace_dirs(&paths).unwrap();
        let store = SessionStore::from_paths(&paths).unwrap();
        let session = store.create_session(tmp.path().to_path_buf()).unwrap();
        let state = AppState::new(PufferConfig::default(), tmp.path().to_path_buf(), session);
        (state, tmp)
    }

    fn ensure_test_subscription_manager() {
        if subscription_manager().is_ok() {
            return;
        }
        let temp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let runtime = Box::leak(Box::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        ));
        let manager = Arc::new(
            SubscriptionManagerBuilder::new(temp.path().join("subscriptions.json"))
                .build(runtime.handle().clone())
                .unwrap(),
        );
        let _ = crate::install_subscription_manager(manager);
    }

    fn ensure_connected_test_connection(connection_slug: &str, connector_slug: &str) {
        ensure_test_subscription_manager();
        let manager = subscription_manager().unwrap();
        if manager.connection_store().get(connection_slug).is_some() {
            manager
                .connection_store()
                .update(connection_slug, |connection| {
                    connection.connector_slug = connector_slug.to_string();
                    connection.state = ConnectionState::Authenticated;
                })
                .unwrap();
            return;
        }
        manager
            .connection_store()
            .create(ConnectionRecord::authenticated(
                connection_slug,
                connector_slug,
                "test connection",
            ))
            .unwrap();
    }

    fn delete_test_connection(connection_slug: &str) {
        ensure_test_subscription_manager();
        let manager = subscription_manager().unwrap();
        if manager.connection_store().get(connection_slug).is_some() {
            manager.connection_store().delete(connection_slug).unwrap();
        }
    }

    fn write_monitor_task_with_delivery_target(
        cwd: &Path,
        task_id: &str,
        connection_slug: &str,
        chat_id: &str,
    ) {
        let path = ConfigPaths::discover(cwd)
            .workspace_config_dir
            .join("runtime/claude_workflow/monitor_tasks.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let store = json!({
            "tasks": [{
                "task_id": task_id,
                "subject": "Reply to Telegram",
                "description": "Draft a reply",
                "active_form": "Drafting a Telegram reply",
                "status": "pending",
                "owner": null,
                "blocks": [],
                "blocked_by": [],
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": connection_slug,
                    "monitor_connector": "telegram-login",
                    "source_context": {
                        "kind": "telegram_direct_message",
                        "connector_slug": "telegram-login",
                        "connection_slug": connection_slug,
                        "summary": "Telegram direct message from chat_id 42",
                        "delivery_target": {
                            "type": "telegram_chat",
                            "chat_id": chat_id,
                            "chat_kind": "user"
                        }
                    }
                },
                "output": null
            }]
        });
        fs::write(path, serde_json::to_string_pretty(&store).unwrap()).unwrap();
    }

    fn write_gmail_monitor_task_with_delivery_target(
        cwd: &Path,
        task_id: &str,
        connection_slug: &str,
        thread_id: &str,
        message_id: &str,
        account: &str,
    ) {
        let path = ConfigPaths::discover(cwd)
            .workspace_config_dir
            .join("runtime/claude_workflow/monitor_tasks.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let store = json!({
            "tasks": [{
                "task_id": task_id,
                "subject": "Reply to Gmail",
                "description": "Draft a reply",
                "active_form": "Drafting a Gmail reply",
                "status": "pending",
                "owner": null,
                "blocks": [],
                "blocked_by": [],
                "metadata": {
                    "_monitor": true,
                    "monitor_connection": connection_slug,
                    "monitor_connector": "gmail-browser",
                    "source_context": {
                        "kind": "gmail_message",
                        "connector_slug": "gmail-browser",
                        "connection_slug": connection_slug,
                        "summary": "Gmail message in thread",
                        "delivery_target": {
                            "type": "gmail_thread",
                            "account": account,
                            "thread_id": thread_id,
                            "message_id": message_id
                        }
                    }
                },
                "output": null
            }]
        });
        fs::write(path, serde_json::to_string_pretty(&store).unwrap()).unwrap();
    }

    #[derive(Default)]
    struct RecordingDispatcher {
        calls: Mutex<Vec<String>>,
    }

    impl ActionDispatcher for RecordingDispatcher {
        fn dispatch(&self, action: &ActionSpec, _envelope: &EventEnvelope) -> ActionResult {
            self.calls.lock().unwrap().push(format!("{action:?}"));
            ActionResult::success("dispatched")
        }
    }

    #[test]
    fn connector_list_row_includes_connect_hints() {
        let temp = tempfile::tempdir().unwrap();
        let template = puffer_subscriptions::builtin_connector_template("telegram-login").unwrap();
        let row = connector_list_row(template, &subscriber_manifest_roots(temp.path()));

        assert_eq!(row["connector_slug"], "telegram-login");
        assert_eq!(row["suggested_connection_slug"], "telegram-user");
        assert_eq!(
            row["connect_command"],
            "/connect telegram-login telegram-user"
        );
        assert_eq!(row["runtime_hints"], json!(["internal-tool"]));
        assert_eq!(row["can_trigger_workflow"], false);
    }

    #[test]
    fn connector_list_row_defaults_custom_connection_to_connector_slug() {
        let temp = tempfile::tempdir().unwrap();
        let row = connector_list_row(
            starter_template("custom-feed"),
            &subscriber_manifest_roots(temp.path()),
        );

        assert_eq!(row["connector_slug"], "custom-feed");
        assert_eq!(row["suggested_connection_slug"], "custom-feed");
        assert_eq!(row["connect_command"], "/connect custom-feed custom-feed");
        assert_eq!(row["runtime_hints"], json!(["command"]));
        assert_eq!(row["can_trigger_workflow"], true);
    }

    #[test]
    fn interactive_connector_act_telegram_send_rejects_before_dispatch() {
        ensure_test_subscription_manager();
        let (mut state, tmp) = make_state();
        let dispatcher = RecordingDispatcher::default();

        let err = execute_connector_act_with_dispatcher(
            &mut state,
            tmp.path(),
            json!({
                "connector_slug": "telegram-login",
                "connection_slug": "telegram-user",
                "action": "send_message",
                "input": {
                    "chat_id": 123456789,
                    "message": "this must not be sent directly"
                }
            }),
            &dispatcher,
        )
        .unwrap_err();

        assert!(err.to_string().contains("ConnectorActionDraft"));
        assert!(
            dispatcher.calls.lock().unwrap().is_empty(),
            "direct ConnectorAct send must reject before deepest dispatch exit"
        );
    }

    #[test]
    fn connector_act_send_message_requires_draft_via_gate() {
        ensure_test_subscription_manager();
        let (mut state, tmp) = make_state();
        let dispatcher = RecordingDispatcher::default();

        let err = execute_connector_act_with_dispatcher(
            &mut state,
            tmp.path(),
            json!({
                "connector_slug": "telegram-login",
                "connection_slug": "telegram-user",
                "action": "send_message",
                "input": {
                    "chat_id": 123456789,
                    "message": "this must become a draft"
                }
            }),
            &dispatcher,
        )
        .unwrap_err();

        assert!(err.to_string().contains("ConnectorActionDraft"));
        assert!(dispatcher.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn connector_action_draft_saves_side_effect_free_external_send() {
        let home = tempfile::tempdir().unwrap();
        let _home_override = set_puffer_home_override(home.path());
        let connection_slug = "telegram-user-draft-ok";
        ensure_connected_test_connection(connection_slug, "telegram-login");
        let (mut state, tmp) = make_state();

        let raw = execute_connector_action_draft(
            &mut state,
            tmp.path(),
            json!({
                "connector_slug": "telegram-login",
                "connection_slug": connection_slug,
                "action": "send_message",
                "input": {
                    "chat_id": 123456789,
                    "message": "deployment is finished"
                }
            }),
        )
        .expect("draft should be saved without sending");
        let payload: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(payload["success"], true);
        assert_eq!(payload["draft"]["status"], "draft_ready");
        assert_eq!(payload["draft"]["recipientStableId"], "123456789");
        assert_eq!(payload["draft"]["message"], "deployment is finished");
        assert!(payload["draft"]["contentHash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
    }

    #[test]
    fn connector_action_draft_rejects_deleted_connection_after_disconnect() {
        let home = tempfile::tempdir().unwrap();
        let _home_override = set_puffer_home_override(home.path());
        let connection_slug = "telegram-user-deleted-draft";
        delete_test_connection(connection_slug);
        let (mut state, tmp) = make_state();

        let err = execute_connector_action_draft(
            &mut state,
            tmp.path(),
            json!({
                "connector_slug": "telegram-login",
                "connection_slug": connection_slug,
                "action": "send_message",
                "input": {
                    "chat_id": 123456789,
                    "message": "this should not become a sendable draft"
                }
            }),
        )
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("is not connected"));
        assert!(message.contains("/connect telegram-login"));
    }

    #[test]
    fn connector_action_draft_with_task_id_stamps_recipient_from_task() {
        let home = tempfile::tempdir().unwrap();
        let _home_override = set_puffer_home_override(home.path());
        let connection_slug = "telegram-user-task-draft";
        ensure_connected_test_connection(connection_slug, "telegram-login");
        let (mut state, tmp) = make_state();
        let task_id = "monitor-1";
        write_monitor_task_with_delivery_target(tmp.path(), task_id, connection_slug, "42");
        let session_id = state.session.id.to_string();
        state.set_monitor_reply_scope_for_turn(task_id.into(), session_id, "turn-1".into());

        // The model passes a wrong connector slug and recipient; the server must
        // stamp both the connector and the recipient from the task source.
        let raw = execute_connector_action_draft(
            &mut state,
            tmp.path(),
            json!({
                "connector_slug": "gmail-browser",
                "connection_slug": "some-other-connection",
                "action": "send_email",
                "taskId": task_id,
                "input": {
                    "chat_id": 99,
                    "to": "attacker@example.com",
                    "message": "deployment is finished"
                }
            }),
        )
        .expect("task-scoped draft should be saved");
        let payload: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(payload["draft"]["recipientStableId"], "telegram:42");
        assert_eq!(payload["draft"]["recipientSource"], "stamped");
        assert_eq!(payload["draft"]["taskId"], task_id);
        // Server-stamped connector/connection/action override the model input.
        assert_eq!(payload["draft"]["connectorSlug"], "telegram-login");
        assert_eq!(payload["draft"]["connectionSlug"], connection_slug);
        assert_eq!(payload["draft"]["action"], "send_message");
    }

    #[test]
    fn connector_action_draft_task_id_outside_scope_rejected() {
        let home = tempfile::tempdir().unwrap();
        let _home_override = set_puffer_home_override(home.path());
        let connection_slug = "telegram-user-task-draft-mismatch";
        ensure_connected_test_connection(connection_slug, "telegram-login");
        let (mut state, tmp) = make_state();
        write_monitor_task_with_delivery_target(tmp.path(), "monitor-2", connection_slug, "42");
        let session_id = state.session.id.to_string();
        state.set_monitor_reply_scope_for_turn("monitor-1".into(), session_id, "turn-1".into());

        let err = execute_connector_action_draft(
            &mut state,
            tmp.path(),
            json!({
                "connector_slug": "telegram-login",
                "connection_slug": connection_slug,
                "action": "send_message",
                "taskId": "monitor-2",
                "input": {
                    "chat_id": 42,
                    "message": "deployment is finished"
                }
            }),
        )
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("taskId"));
        assert!(message.contains("does not match scoped monitor task"));
    }

    #[test]
    fn connector_action_draft_with_gmail_task_id_stamps_thread_from_task() {
        let home = tempfile::tempdir().unwrap();
        let _home_override = set_puffer_home_override(home.path());
        let connection_slug = "gmail-user-task-draft";
        ensure_connected_test_connection(connection_slug, "gmail-browser");
        let (mut state, tmp) = make_state();
        let task_id = "monitor-gmail-1";
        write_gmail_monitor_task_with_delivery_target(
            tmp.path(),
            task_id,
            connection_slug,
            "thread-abc",
            "msg-42",
            "owner@gmail.com",
        );
        let session_id = state.session.id.to_string();
        state.set_monitor_reply_scope_for_turn(task_id.into(), session_id, "turn-1".into());

        // The model does not know the Gmail reply action or thread; both are
        // stamped from the task source.
        let raw = execute_connector_action_draft(
            &mut state,
            tmp.path(),
            json!({
                "connector_slug": "gmail-browser",
                "connection_slug": connection_slug,
                "action": "send_email",
                "taskId": task_id,
                "input": {
                    "to": "attacker@example.com",
                    "thread_id": "wrong-thread",
                    "message": "Thanks, sending the report now."
                }
            }),
        )
        .expect("gmail task-scoped draft should be saved");
        let payload: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(payload["draft"]["recipientStableId"], "gmail:thread-abc");
        assert_eq!(payload["draft"]["recipientSource"], "stamped");
        assert_eq!(payload["draft"]["connectorSlug"], "gmail-browser");
        assert_eq!(payload["draft"]["connectionSlug"], connection_slug);
        assert_eq!(payload["draft"]["action"], "draft_reply");
        assert_eq!(
            payload["draft"]["message"],
            "Thanks, sending the report now."
        );

        // The stored connector action input must carry the Gmail reply intent in
        // the shape the gmail-browser executor parses (thread_id/message_id), and
        // the model's forged recipient keys must be stripped.
        let draft_id = payload["draft"]["id"].as_str().unwrap();
        let paths = ConfigPaths::discover(tmp.path());
        let store = OutboundStore::load(outbound_actions_path(&paths)).unwrap();
        let stored = store.get(draft_id).unwrap().expect("stored draft");
        assert_eq!(stored.input["thread_id"], "thread-abc");
        assert_eq!(stored.input["message_id"], "msg-42");
        assert_eq!(stored.input["account"], "owner@gmail.com");
        assert_eq!(stored.input["connector_slug"], "gmail-browser");
        assert_eq!(stored.input["connection_slug"], connection_slug);
        assert!(stored.input.get("to").is_none());
    }

    #[test]
    fn connector_action_draft_without_task_id_in_task_scope_uses_model_recipient() {
        // Spec test-matrix item 4: a task-session context is active but the model
        // omits taskId, so the draft is a free-form third-party send with a
        // model-sourced recipient rather than a stamped one.
        let home = tempfile::tempdir().unwrap();
        let _home_override = set_puffer_home_override(home.path());
        let connection_slug = "telegram-user-no-task-draft";
        ensure_connected_test_connection(connection_slug, "telegram-login");
        let (mut state, tmp) = make_state();
        write_monitor_task_with_delivery_target(tmp.path(), "monitor-1", connection_slug, "42");
        let session_id = state.session.id.to_string();
        state.set_monitor_reply_scope_for_turn("monitor-1".into(), session_id, "turn-1".into());

        let raw = execute_connector_action_draft(
            &mut state,
            tmp.path(),
            json!({
                "connector_slug": "telegram-login",
                "connection_slug": connection_slug,
                "action": "send_message",
                "input": {
                    "chat_id": 777,
                    "message": "message to a different person"
                }
            }),
        )
        .expect("free-form draft should be saved even inside a task scope");
        let payload: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(payload["draft"]["recipientSource"], "model");
        assert_eq!(payload["draft"]["recipientStableId"], "777");
        assert_eq!(payload["draft"]["taskId"], Value::Null);
    }

    #[test]
    fn monitor_action_prompts_keep_connector_action_draft_tool() {
        // #634 regression guard: a future prompt edit must not silently re-lock
        // the draft tool, or every monitor reply turn loses its only send path.
        for prompt in [
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../resources/prompts/monitor-telegram-action.yaml"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../resources/prompts/monitor-reply-action.yaml"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../resources/prompts/monitor-gmail-action.yaml"
            )),
        ] {
            let doc: serde_yaml::Value = serde_yaml::from_str(prompt).unwrap();
            let allowed = doc["allowed_tools"]
                .as_sequence()
                .expect("allowed_tools must be a list");
            assert!(
                allowed
                    .iter()
                    .any(|tool| tool.as_str() == Some("ConnectorActionDraft")),
                "monitor action prompt lost ConnectorActionDraft from allowed_tools"
            );
        }
    }
}
