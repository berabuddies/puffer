//! Connector and connection workflow tools.

use crate::runtime::subscription_manager;
use crate::AppState;
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::{Event, EventEnvelope};
use puffer_subscriptions::{
    connector_runtime_hints, connector_workflow_trigger_supported, suggested_connection_slug,
    ActionDispatcher, ActionSpec, BuiltinActionDispatcher, ConnectionAuthStatus, ConnectionRecord,
    ConnectionState, ConnectorActionDefinition, ConnectorActionRequest, ConnectorTemplate,
    SubscriberManifestRoots,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
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
    if connector_action_requires_human_review_for_input(action_definition, &parsed.input) {
        anyhow::bail!(
            "connector action `{}`/`{}` sends an external message and requires human draft review before execution",
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
    let template = subscription_manager()
        .ok()
        .and_then(|manager| manager.connector_store().get(&parsed.connector_slug))
        .or_else(|| puffer_subscriptions::builtin_connector_template(&parsed.connector_slug))
        .ok_or_else(|| anyhow::anyhow!("connector `{}` not found", parsed.connector_slug))?;
    let action_definition = template.actions.get(&parsed.action).ok_or_else(|| {
        anyhow::anyhow!(
            "connector `{}` does not define action `{}`",
            parsed.connector_slug,
            parsed.action
        )
    })?;
    if !connector_action_requires_human_review_for_input(action_definition, &parsed.input) {
        anyhow::bail!(
            "ConnectorActionDraft is only for external message send actions that require human review"
        );
    }

    let connection = connector_action_connection(
        &parsed.connector_slug,
        parsed.connection_slug.as_deref(),
        &parsed.input,
    );
    let mut action_input = parsed.input.clone();
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
    let message = draft_message_text(&action_input)
        .context("ConnectorActionDraft requires a message body")?;
    let paths = ConfigPaths::discover(cwd);
    let store_path = outbound_action_drafts_path(&paths);
    let mut store = read_outbound_action_draft_store(&store_path)?;
    let drafts = store
        .get_mut("drafts")
        .and_then(Value::as_array_mut)
        .context("outbound action draft store missing drafts array")?;
    let version = drafts
        .iter()
        .filter(|draft| {
            draft.get("session_id").and_then(Value::as_str)
                == Some(state.session.id.to_string().as_str())
        })
        .filter_map(|draft| draft.get("version").and_then(Value::as_u64))
        .max()
        .unwrap_or(0)
        + 1;
    let now_ms = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    let draft_id = format!("draft-action-{}-{now_ms}", state.session.id.simple());
    let created_at = OffsetDateTime::now_utc().to_string();
    let content_hash = draft_content_hash(&recipient_stable_id, &message);
    let draft = json!({
        "id": draft_id,
        "created_by": "ConnectorActionDraft",
        "status": "draft_ready",
        "version": version,
        "connector_slug": parsed.connector_slug,
        "connection_slug": connection,
        "action": parsed.action,
        "input": action_input,
        "recipient_stable_id": recipient_stable_id,
        "message": message,
        "content_hash": content_hash,
        "session_id": state.session.id.to_string(),
        "turn_id": Value::Null,
        "created_at": created_at,
        "updated_at": created_at,
        "approved_message": Value::Null,
        "approved_by": Value::Null,
        "approved_at": Value::Null,
        "client_request_id": Value::Null,
        "send_attempt_id": Value::Null,
        "receipt": Value::Null,
        "error": Value::Null,
    });
    drafts.push(draft);
    write_outbound_action_draft_store(&store_path, &store)?;

    Ok(serde_json::to_string_pretty(&json!({
        "success": true,
        "draft": {
            "id": draft_id,
            "status": "draft_ready",
            "version": version,
            "connectorSlug": parsed.connector_slug,
            "connectionSlug": connection,
            "action": parsed.action,
            "recipientStableId": recipient_stable_id,
            "message": message,
            "contentHash": content_hash,
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

fn connector_action_requires_human_review_for_input(
    action: &ConnectorActionDefinition,
    _input: &Value,
) -> bool {
    connector_action_requires_human_review(action)
}

fn connector_action_requires_human_review(action: &ConnectorActionDefinition) -> bool {
    let category = action.permission.category.as_str();
    category == "external_message_send"
        || (action.permission.external_side_effect && send_like_action_slug(&action.slug))
}

fn outbound_action_drafts_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("outbound_action_drafts.json")
}

fn read_outbound_action_draft_store(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({ "drafts": [] }));
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read outbound draft store {}", path.display()))?;
    let mut store: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid outbound draft store {}", path.display()))?;
    if store.get("drafts").and_then(Value::as_array).is_none() {
        store["drafts"] = json!([]);
    }
    Ok(store)
}

fn write_outbound_action_draft_store(path: &Path, store: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(store)?)
        .with_context(|| format!("failed to write outbound draft store {}", path.display()))
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

fn draft_content_hash(recipient_stable_id: &str, text: &str) -> String {
    let canonical = json!({
        "recipient_stable_id": recipient_stable_id,
        "text": text,
        "media": [],
    });
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn send_like_action_slug(slug: &str) -> bool {
    let normalized = slug.replace('-', "_").to_ascii_lowercase();
    normalized == "send"
        || normalized == "reply"
        || normalized == "post"
        || normalized == "publish"
        || normalized == "send_message"
        || normalized == "forward_message"
        || normalized == "forward_messages"
        || normalized.starts_with("send_")
        || normalized.ends_with("_send")
        || normalized.starts_with("reply_")
        || normalized.ends_with("_reply")
        || normalized.starts_with("post_")
        || normalized.ends_with("_post")
        || normalized.starts_with("publish_")
        || normalized.ends_with("_publish")
        || normalized.starts_with("forward_")
        || normalized.ends_with("_forward")
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
    use puffer_config::{ensure_workspace_dirs, PufferConfig};
    use puffer_session_store::SessionStore;
    use puffer_subscriptions::{
        ActionResult, ConnectorPermissionDefinition, SubscriptionManagerBuilder,
    };
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
    fn interactive_connector_act_external_send_requires_draft_review_from_registry_category() {
        let template = puffer_subscriptions::builtin_connector_template("telegram-login").unwrap();
        let action = template.actions.get("send_message").unwrap();

        assert!(connector_action_requires_human_review(action));
    }

    #[test]
    fn interactive_connector_act_external_send_rejects_non_telegram_actions() {
        let template = puffer_subscriptions::builtin_connector_template("slack-login").unwrap();
        let action = template.actions.get("send_message").unwrap();

        assert!(
            connector_action_requires_human_review(action),
            "external send gate must not be Telegram-specific"
        );
    }

    #[test]
    fn interactive_connector_act_default_denies_future_send_like_side_effects() {
        for slug in [
            "send",
            "reply",
            "send_story",
            "forward_message",
            "forward-messages",
            "post_message",
            "publish_update",
        ] {
            let action = ConnectorActionDefinition {
                slug: slug.to_string(),
                description: String::new(),
                input_schema: json!({}),
                output_schema: json!({}),
                permission: ConnectorPermissionDefinition {
                    category: "custom_action".to_string(),
                    summary: String::new(),
                    external_side_effect: true,
                },
            };

            assert!(
                connector_action_requires_human_review(&action),
                "{slug} should require human review"
            );
        }
    }

    #[test]
    fn interactive_connector_act_ignores_agent_supplied_category_spoof() {
        let template = puffer_subscriptions::builtin_connector_template("telegram-login").unwrap();
        let action = template.actions.get("send_message").unwrap();
        let agent_input = json!({
            "chat_id": 42,
            "message": "hello",
            "category": "read"
        });

        assert!(
            connector_action_requires_human_review_for_input(action, &agent_input),
            "server-owned registry category must win over agent-supplied category spoof"
        );
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

        assert!(err.to_string().contains("requires human draft review"));
        assert!(
            dispatcher.calls.lock().unwrap().is_empty(),
            "direct ConnectorAct send must reject before deepest dispatch exit"
        );
    }

    #[test]
    fn connector_action_draft_saves_side_effect_free_external_send() {
        let (mut state, tmp) = make_state();

        let raw = execute_connector_action_draft(
            &mut state,
            tmp.path(),
            json!({
                "connector_slug": "telegram-login",
                "connection_slug": "telegram-user",
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
}
