//! Connector and connection workflow tools.

use crate::runtime::subscription_manager;
use crate::AppState;
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::{Event, EventEnvelope};
use puffer_subscriptions::{
    connector_runtime_hints, connector_workflow_trigger_supported, suggested_connection_slug,
    ActionDispatcher, ActionSpec, BuiltinActionDispatcher, ConnectionAuthStatus, ConnectionRecord,
    ConnectionState, ConnectorActionRequest, ConnectorTemplate, SubscriberManifestRoots,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
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
    let dispatcher = BuiltinActionDispatcher::new();
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
}
