//! Product-layer Automation daemon RPC helpers.

use anyhow::{Context, Result};
use puffer_automation::{
    AutomationRecord, AutomationRuntimeState, AutomationSpec, AutomationStatus, AutomationStore,
    AutomationStoreError,
};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    builtin_connector_templates, connector_workflow_trigger_supported, suggested_connection_slug,
    ConnectionState, ConnectorActionDefinition, ConnectorTemplate, SubscriberManifestRoots,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;

const AUTOMATION_ID_KEYS: &[&str] = &["id", "automation_id", "automationId"];

/// Returns the user-level Automation store path.
pub(crate) fn automation_store_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("automations.json")
}

/// Lists user-facing Automations without exposing internal runtime artifact ids.
pub(crate) fn handle_automation_list(store: &AutomationStore) -> Result<Value> {
    let automations = store
        .list()
        .into_iter()
        .map(AutomationDto::from)
        .collect::<Vec<_>>();
    Ok(json!({ "automations": automations }))
}

/// Fetches one user-facing Automation.
pub(crate) fn handle_automation_get(store: &AutomationStore, params: &Value) -> Result<Value> {
    let id = required_automation_id(params)?;
    let record = store.get(&id)?;
    Ok(serde_json::to_value(AutomationDto::from(record))?)
}

/// Creates or updates one user-facing Automation.
pub(crate) fn handle_automation_save(store: &AutomationStore, params: &Value) -> Result<Value> {
    validate_save_fields(params)?;
    let id = required_automation_id(params)?;
    let expected_revision = optional_expected_revision(params)?;
    let status = optional_status(params)?;
    if matches!(status, Some(AutomationStatus::Enabled)) {
        anyhow::bail!("automation_save cannot enable automations; use automation_compile_deploy");
    }
    let spec = params
        .get("spec")
        .cloned()
        .context("automation_save requires spec")?;
    let spec: AutomationSpec =
        serde_json::from_value(spec).context("automation_save spec must match AutomationSpec")?;

    let previous = match store.get(&id) {
        Ok(record) => Some(record),
        Err(AutomationStoreError::NotFound(_)) => None,
        Err(error) => return Err(error.into()),
    };
    let record = match store.get(&id) {
        Ok(_) => {
            let expected_revision =
                expected_revision.context("automation_save update requires expected_revision")?;
            let mut saved = store.save_spec(&id, expected_revision, spec)?;
            if saved.status == AutomationStatus::Enabled
                && saved.runtime.status == puffer_automation::AutomationRuntimeStatus::Stale
            {
                saved = store.set_status(&id, AutomationStatus::Paused)?;
            }
            if let Some(status) = status {
                store.set_status(&id, status)?
            } else {
                saved
            }
        }
        Err(AutomationStoreError::NotFound(_)) => {
            if expected_revision.is_some() {
                anyhow::bail!("automation_save create must not include expected_revision");
            }
            store.create(&id, spec, status.unwrap_or(AutomationStatus::Paused))?
        }
        Err(error) => return Err(error.into()),
    };

    crate::daemon_automation_runtime::sync_automation_bindings_after_save(
        previous.as_ref(),
        &record,
    )?;
    Ok(serde_json::to_value(AutomationDto::from(record))?)
}

/// Deletes one user-facing Automation.
pub(crate) fn handle_automation_delete(store: &AutomationStore, params: &Value) -> Result<Value> {
    let id = required_automation_id(params)?;
    let record = store.get(&id)?;
    crate::daemon_automation_runtime::remove_automation_bindings(&record)?;
    store.delete(&id)?;
    Ok(json!({ "id": id, "deleted": true }))
}

/// Returns the real trigger/action catalog used by the desktop Automation UI.
pub(crate) fn handle_automation_catalog(paths: &ConfigPaths) -> Result<Value> {
    let mut trigger_error = None::<String>;
    let mut triggers = Vec::new();
    let mut actions = Vec::new();

    triggers.push(json!({
        "id": "webhook",
        "kind": "webhook",
        "label": "Webhook",
        "summary": "Run when the Automation runtime receives an HTTP POST.",
        "icon": "bolt",
        "connection_state": "ready",
        "permission_state": "ready",
        "required_inputs": [
            {"id": "path", "label": "Path", "kind": "text", "required": true, "default": "puffer-automation-webhook"}
        ],
        "spec_template": {
            "type": "agent_env_node",
            "id": "trigger-1",
            "node": {
                "node_type": "webhook",
                "name": "Webhook",
                "trusted": false,
                "config": {
                    "path": "puffer-automation-webhook",
                    "methods": ["POST"],
                    "authentication": "none"
                }
            },
            "summary": "Webhook"
        }
    }));
    triggers.push(json!({
        "id": "schedule:daily",
        "kind": "schedule",
        "label": "Every day at",
        "summary": "Run once per day at the configured local time.",
        "icon": "clock",
        "connection_state": "ready",
        "permission_state": "ready",
        "required_inputs": [
            {"id": "mode", "label": "Mode", "kind": "select", "required": true, "default": "daily", "options": ["daily", "weekday", "cron"]},
            {"id": "time", "label": "Time", "kind": "time", "required": true, "default": "09:00"},
            {"id": "timezone", "label": "Timezone", "kind": "text", "required": true, "default": "local"}
        ],
        "spec_template": {
            "type": "agent_env_node",
            "id": "trigger-1",
            "node": {
                "node_type": "schedule",
                "name": "Schedule",
                "trusted": true,
                "config": {"mode": "daily", "time": "09:00", "timezone": "local"}
            },
            "summary": "Every day at 09:00"
        }
    }));
    triggers.push(json!({
        "id": "schedule:cron",
        "kind": "schedule",
        "label": "Custom schedule",
        "summary": "Run from a cron expression and timezone.",
        "icon": "clock",
        "connection_state": "ready",
        "permission_state": "ready",
        "required_inputs": [
            {"id": "mode", "label": "Mode", "kind": "select", "required": true, "default": "cron", "options": ["cron"]},
            {"id": "cron", "label": "Cron", "kind": "text", "required": true, "default": "0 9 * * 1-5"},
            {"id": "timezone", "label": "Timezone", "kind": "text", "required": true, "default": "local"}
        ],
        "spec_template": {
            "type": "agent_env_node",
            "id": "trigger-1",
            "node": {
                "node_type": "schedule",
                "name": "Cron schedule",
                "trusted": true,
                "config": {"mode": "cron", "cron": "0 9 * * 1-5", "timezone": "local"}
            },
            "summary": "Custom schedule 0 9 * * 1-5"
        }
    }));
    actions.push(local_transform_action_json());

    match puffer_core::subscription_manager() {
        Ok(manager) => {
            let roots = subscriber_manifest_roots(paths);
            let connections = manager.connection_store().list();
            let templates = manager.connector_store().list_with_builtins();
            for template in &templates {
                let can_trigger = connector_workflow_trigger_supported(&roots, template);
                let matching_connections = connections
                    .iter()
                    .filter(|connection| connection.connector_slug == template.slug)
                    .collect::<Vec<_>>();
                if can_trigger {
                    if matching_connections.is_empty() {
                        let connection_slug = suggested_connection_slug(&template.slug);
                        triggers.push(connector_trigger_json(
                            template,
                            &connection_slug,
                            "not_connected",
                        ));
                    } else {
                        for connection in &matching_connections {
                            triggers.push(connector_trigger_json(
                                template,
                                &connection.slug,
                                connection_state_slug(connection.state),
                            ));
                        }
                    }
                }
                if matching_connections.is_empty() {
                    let connection_slug = suggested_connection_slug(&template.slug);
                    actions.extend(connector_action_jsons(
                        template,
                        &connection_slug,
                        "not_connected",
                    ));
                } else {
                    for connection in &matching_connections {
                        actions.extend(connector_action_jsons(
                            template,
                            &connection.slug,
                            connection_state_slug(connection.state),
                        ));
                    }
                }
            }
        }
        Err(error) => {
            trigger_error = Some(error.to_string());
            for template in builtin_connector_templates() {
                let connection_slug = suggested_connection_slug(&template.slug);
                actions.extend(connector_action_jsons(
                    &template,
                    &connection_slug,
                    "not_connected",
                ));
            }
        }
    }

    Ok(json!({
        "triggers": triggers,
        "actions": actions,
        "trigger_error": trigger_error,
        "action_error": null,
        "agentenv_error": null,
    }))
}

fn local_transform_action_json() -> Value {
    json!({
        "id": "agentenv:transform_js:local-transform",
        "runtime_owner": "agentenv",
        "kind": "agentenv_node",
        "label": "Local JavaScript Transform",
        "summary": "Run JavaScript in the selected Automation runtime and return a structured result.",
        "icon": "bolt",
        "connection_state": "ready",
        "permission_state": "ready",
        "permission_summary": "Runs inside the configured Automation runtime.",
        "external_side_effect": false,
        "required_inputs": [],
        "input_schema": {
            "type": "object",
            "description": "Automation preview or trigger input."
        },
        "output_schema": {
            "type": "object",
            "properties": {
                "ok": {"type": "boolean"},
                "kind": {"type": "string"},
                "input": {"type": "object"}
            }
        },
        "node_ref": {
            "node_type": "transform_js",
            "name": "Local JavaScript Transform",
            "trusted": false,
            "config": {
                "code": local_transform_code()
            }
        }
    })
}

fn local_transform_code() -> &'static str {
    r#"const trigger = input && typeof input === "object" ? input.trigger ?? input : input;
return {
  kind: "local_transform_result",
  trigger,
  input,
  ok: true,
  message: "Local AgentEnv transform executed."
};"#
}

fn connector_trigger_json(
    template: &ConnectorTemplate,
    connection_slug: &str,
    connection_state: &str,
) -> Value {
    let label = connector_trigger_label(&template.slug);
    json!({
        "id": format!("connector:{}:{}", template.slug, connection_slug),
        "kind": "connector_event",
        "connector_slug": template.slug,
        "connection_slug": connection_slug,
        "label": label,
        "summary": template.description,
        "icon": icon_for_connector(&template.slug),
        "connection_state": connection_state,
        "permission_state": if connection_state == "authenticated" || connection_state == "active" { "ready" } else { "needs_connection" },
        "required_inputs": connector_required_inputs(&template.slug),
        "spec_template": {
            "type": "puffer_connection",
            "id": "trigger-1",
            "connection_slug": connection_slug,
            "connector_slug": template.slug,
            "summary": label,
        }
    })
}

fn connector_required_inputs(_connector_slug: &str) -> Vec<Value> {
    vec![json!({"id": "filter", "label": "Filter", "kind": "text", "required": false})]
}

fn connector_action_jsons(
    template: &ConnectorTemplate,
    connection_slug: &str,
    connection_state: &str,
) -> Vec<Value> {
    template
        .actions
        .values()
        .map(|action| connector_action_json(template, connection_slug, connection_state, action))
        .collect()
}

fn connector_action_json(
    template: &ConnectorTemplate,
    connection_slug: &str,
    connection_state: &str,
    action: &ConnectorActionDefinition,
) -> Value {
    let ready = matches!(connection_state, "authenticated" | "active" | "ready");
    json!({
        "id": format!("connector:{}:{}:{}", template.slug, connection_slug, action.slug),
        "runtime_owner": "puffer",
        "kind": "connector_action",
        "connector_slug": template.slug,
        "connection_slug": connection_slug,
        "action": action.slug,
        "label": field_label(&action.slug),
        "summary": action.description,
        "icon": icon_for_connector(&template.slug),
        "connection_state": connection_state,
        "permission_state": if ready { "ready" } else { "needs_connection" },
        "permission_summary": action.permission.summary,
        "external_side_effect": action.permission.external_side_effect,
        "required_inputs": connector_action_required_inputs(action),
        "input_schema": action.input_schema,
        "output_schema": action.output_schema,
        "node_ref": {
            "node_type": "puffer_connector_action",
            "name": field_label(&action.slug),
            "trusted": false,
            "config": {
                "connector_slug": template.slug,
                "connection_slug": connection_slug,
                "action": action.slug,
                "input": {},
                "external_side_effect": action.permission.external_side_effect,
                "human_approval_required": action.permission.external_side_effect,
                "draft_only": action.permission.external_side_effect
            }
        }
    })
}

fn connector_action_required_inputs(action: &ConnectorActionDefinition) -> Vec<Value> {
    let required = action
        .input_schema
        .get("required")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let properties = action
        .input_schema
        .get("properties")
        .and_then(Value::as_object);
    required
        .into_iter()
        .map(|id| {
            let description = properties
                .and_then(|props| props.get(id))
                .and_then(|property| property.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("");
            json!({
                "id": id,
                "label": field_label(id),
                "kind": "text",
                "required": true,
                "default": "",
                "description": description
            })
        })
        .collect()
}

fn icon_for_connector(slug: &str) -> &'static str {
    if slug.contains("github") || slug.contains("gitlab") {
        "git"
    } else if slug.contains("calendar") || slug.contains("gcal") {
        "clock"
    } else if slug.contains("mail") || slug.contains("gmail") || slug.contains("email") {
        "edit"
    } else if slug.contains("slack")
        || slug.contains("telegram")
        || slug.contains("lark")
        || slug.contains("wechat")
        || slug.contains("discord")
        || slug.contains("matrix")
    {
        "message"
    } else {
        "bolt"
    }
}

fn connector_trigger_label(slug: &str) -> String {
    if slug.contains("github") {
        "Pull request or issue event".to_string()
    } else if slug.contains("gcal") || slug.contains("calendar") {
        "Calendar event changes".to_string()
    } else if slug.contains("gmail") || slug.contains("email") {
        "Email arrives".to_string()
    } else {
        format!("{} event", field_label(slug))
    }
}

fn field_label(value: &str) -> String {
    value
        .split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn subscriber_manifest_roots(paths: &ConfigPaths) -> SubscriberManifestRoots {
    SubscriberManifestRoots {
        workspace_config_dir: paths.workspace_config_dir.clone(),
        user_config_dir: paths.user_config_dir.clone(),
        builtin_resources_dir: paths.builtin_resources_dir.clone(),
    }
}

fn connection_state_slug(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Created => "created",
        ConnectionState::Authenticating => "authenticating",
        ConnectionState::Authenticated => "authenticated",
        ConnectionState::Active => "active",
        ConnectionState::Degraded => "degraded",
        ConnectionState::Disabled => "disabled",
    }
}

#[derive(Debug, Serialize)]
struct AutomationDto {
    id: String,
    status: AutomationStatus,
    revision: u64,
    spec: AutomationSpec,
    runtime: AutomationRuntimeSummaryDto,
    created_at_ms: i128,
    updated_at_ms: i128,
}

impl From<AutomationRecord> for AutomationDto {
    fn from(record: AutomationRecord) -> Self {
        Self {
            id: record.id,
            status: record.status,
            revision: record.revision,
            spec: record.spec,
            runtime: AutomationRuntimeSummaryDto::from(record.runtime),
            created_at_ms: record.created_at_ms,
            updated_at_ms: record.updated_at_ms,
        }
    }
}

#[derive(Debug, Serialize)]
struct AutomationRuntimeSummaryDto {
    status: puffer_automation::AutomationRuntimeStatus,
    spec_hash: Option<String>,
    compiled_revision: Option<u64>,
    agentenv_workflow_count: usize,
    puffer_binding_count: usize,
    last_error: Option<String>,
}

impl From<AutomationRuntimeState> for AutomationRuntimeSummaryDto {
    fn from(runtime: AutomationRuntimeState) -> Self {
        Self {
            status: runtime.status,
            spec_hash: runtime.spec_hash,
            compiled_revision: runtime.compiled_revision,
            agentenv_workflow_count: runtime.agentenv_workflows.len(),
            puffer_binding_count: runtime.puffer_bindings.len(),
            last_error: runtime
                .last_error
                .as_deref()
                .map(crate::daemon_automation_runtime::public_automation_error_detail_message),
        }
    }
}

fn required_automation_id(params: &Value) -> Result<String> {
    for key in AUTOMATION_ID_KEYS {
        if let Some(value) = params
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(value.to_string());
        }
    }
    anyhow::bail!("missing automation id")
}

fn optional_expected_revision(params: &Value) -> Result<Option<u64>> {
    match (
        params.get("expected_revision"),
        params.get("expectedRevision"),
    ) {
        (Some(_), Some(_)) => anyhow::bail!(
            "automation_save accepts only one of expected_revision or expectedRevision"
        ),
        (Some(value), None) | (None, Some(value)) => value
            .as_u64()
            .context("automation_save expected_revision must be an unsigned integer")
            .map(Some),
        (None, None) => Ok(None),
    }
}

fn optional_status(params: &Value) -> Result<Option<AutomationStatus>> {
    params
        .get("status")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .context("automation_save status must match AutomationStatus")
}

fn validate_save_fields(params: &Value) -> Result<()> {
    let object = params
        .as_object()
        .context("automation_save params must be a JSON object")?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "id" | "automation_id"
                | "automationId"
                | "expected_revision"
                | "expectedRevision"
                | "status"
                | "spec"
        ) {
            anyhow::bail!("automation_save does not accept top-level field `{key}`");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_automation::{
        automation_spec_hash, AgentEnvNodeRef, AutomationFlowSpec, AutomationReviewSpec,
        AutomationRuntimeStatus, AutomationSource, AutomationStepSpec, AutomationTriggerSpec,
        CompiledAgentEnvWorkflow, CompiledPufferBinding, CompiledWorkflowRole,
        AUTOMATION_SPEC_VERSION,
    };
    use std::collections::{BTreeMap, BTreeSet};

    fn store() -> (tempfile::TempDir, AutomationStore) {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("automations.json");
        let store = AutomationStore::load(path).unwrap();
        (tempdir, store)
    }

    fn sample_spec(instructions: &str) -> AutomationSpec {
        AutomationSpec {
            spec_version: AUTOMATION_SPEC_VERSION,
            name: "Reply helper".into(),
            description: None,
            source: AutomationSource::Blank,
            instructions: instructions.into(),
            run_location: Default::default(),
            triggers: vec![AutomationTriggerSpec::PufferConnection {
                id: "incoming".into(),
                connection_slug: "telegram-user".into(),
                connector_slug: Some("telegram-login".into()),
                filter: None,
                ignore_filters: Vec::new(),
                contact_ids: Vec::new(),
                summary: None,
            }],
            flow: AutomationFlowSpec {
                steps: vec![AutomationStepSpec::AgentEnvNode {
                    id: "draft".into(),
                    node: AgentEnvNodeRef {
                        node_type: "transform_js".into(),
                        name: Some("Transform".into()),
                        trusted: Some(true),
                        config: BTreeMap::new(),
                    },
                    summary: None,
                }],
            },
            review: AutomationReviewSpec::default(),
        }
    }

    fn save_create_params() -> Value {
        json!({
            "id": "reply-helper",
            "status": "paused",
            "spec": sample_spec("Draft a reply for review."),
        })
    }

    #[test]
    fn automation_list_empty_returns_array() {
        let (_tempdir, store) = store();

        let value = handle_automation_list(&store).unwrap();

        assert_eq!(value["automations"], json!([]));
    }

    #[test]
    fn automation_save_create_initializes_record_runtime() {
        let (_tempdir, store) = store();

        let value = handle_automation_save(&store, &save_create_params()).unwrap();

        assert_eq!(value["id"], "reply-helper");
        assert_eq!(value["revision"], 1);
        assert_eq!(value["status"], "paused");
        assert_eq!(value["runtime"]["status"], "not_compiled");
        assert_eq!(value["runtime"]["compiled_revision"], Value::Null);
        assert_eq!(value["runtime"]["spec_hash"], Value::Null);
    }

    #[test]
    fn automation_dto_maps_runtime_last_error_for_display() {
        let (_tempdir, store) = store();
        let record = store
            .create(
                "reply-helper",
                sample_spec("Draft a reply for review."),
                AutomationStatus::Paused,
            )
            .unwrap();
        store
            .replace_runtime_error(
                "reply-helper",
                record.revision,
                "create workflow artifact: runtime unreachable: error sending request for url (http://127.0.0.1:3000/v1/workflows)".to_string(),
            )
            .unwrap();

        let value = handle_automation_get(&store, &json!({"id": "reply-helper"})).unwrap();

        assert_eq!(
            value["runtime"]["last_error"],
            "Automation runtime is unreachable. Check Docker or the selected runtime settings, then try again."
        );
        assert!(!value["runtime"]["last_error"]
            .as_str()
            .unwrap()
            .contains("127.0.0.1"));
        assert!(!value["runtime"]["last_error"]
            .as_str()
            .unwrap()
            .contains("/v1/"));
    }

    #[test]
    fn automation_save_rejects_enabled_status() {
        let (_tempdir, store) = store();
        let mut params = save_create_params();
        params
            .as_object_mut()
            .unwrap()
            .insert("status".into(), json!("enabled"));

        let error = handle_automation_save(&store, &params).unwrap_err();

        assert!(error.to_string().contains("cannot enable automations"));
    }

    #[test]
    fn automation_get_returns_saved_dto() {
        let (_tempdir, store) = store();
        handle_automation_save(&store, &save_create_params()).unwrap();

        let value =
            handle_automation_get(&store, &json!({"automationId": "reply-helper"})).unwrap();

        assert_eq!(value["id"], "reply-helper");
        assert_eq!(value["spec"]["name"], "Reply helper");
    }

    #[test]
    fn automation_save_update_accepts_expected_revision_camel_case() {
        let (_tempdir, store) = store();
        handle_automation_save(&store, &save_create_params()).unwrap();

        let value = handle_automation_save(
            &store,
            &json!({
                "id": "reply-helper",
                "expectedRevision": 1,
                "spec": sample_spec("Draft a shorter reply for review."),
            }),
        )
        .unwrap();

        assert_eq!(value["revision"], 2);
        assert_eq!(
            value["spec"]["instructions"],
            "Draft a shorter reply for review."
        );
    }

    #[test]
    fn automation_save_update_requires_current_revision() {
        let (_tempdir, store) = store();
        handle_automation_save(&store, &save_create_params()).unwrap();
        handle_automation_save(
            &store,
            &json!({
                "id": "reply-helper",
                "expectedRevision": 1,
                "spec": sample_spec("Draft a shorter reply for review."),
            }),
        )
        .unwrap();

        let error = handle_automation_save(
            &store,
            &json!({
                "id": "reply-helper",
                "expectedRevision": 1,
                "spec": sample_spec("Draft an even shorter reply for review."),
            }),
        )
        .unwrap_err();

        assert!(error.to_string().contains("revision conflict"));
    }

    #[test]
    fn automation_save_spec_change_pauses_deployed_enabled_record() {
        let (_tempdir, store) = store();
        let record = store
            .create(
                "reply-helper",
                sample_spec("Draft a reply for review."),
                AutomationStatus::Enabled,
            )
            .unwrap();
        store
            .replace_runtime(
                "reply-helper",
                record.revision,
                AutomationRuntimeState {
                    spec_hash: Some(automation_spec_hash(&record.spec).unwrap()),
                    compiled_revision: Some(record.revision),
                    status: AutomationRuntimeStatus::Deployed,
                    agentenv_workflows: vec![CompiledAgentEnvWorkflow {
                        role: CompiledWorkflowRole::Root,
                        workflow_id: Some("wf-root".into()),
                        definition_hash: None,
                        deployed: true,
                    }],
                    puffer_bindings: Vec::new(),
                    last_error: None,
                },
            )
            .unwrap();

        let value = handle_automation_save(
            &store,
            &json!({
                "id": "reply-helper",
                "expectedRevision": 1,
                "spec": sample_spec("Draft a shorter reply for review."),
            }),
        )
        .unwrap();

        assert_eq!(value["status"], "paused");
        assert_eq!(value["runtime"]["status"], "stale");
    }

    #[test]
    fn automation_catalog_connector_trigger_inputs_are_persisted_fields_only() {
        let github_inputs = connector_required_inputs("github");

        assert_eq!(
            github_inputs
                .iter()
                .filter_map(|input| input.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>(),
            vec!["filter"]
        );
    }

    #[test]
    fn automation_catalog_includes_webhook_trigger() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let catalog = handle_automation_catalog(&paths).unwrap();
        let triggers = catalog["triggers"].as_array().unwrap();
        let webhook = triggers
            .iter()
            .find(|trigger| trigger["id"] == "webhook")
            .expect("webhook trigger");

        assert_eq!(webhook["kind"], "webhook");
        assert_eq!(webhook["label"], "Webhook");
        assert_eq!(webhook["spec_template"]["type"], "agent_env_node");
        assert_eq!(webhook["spec_template"]["node"]["node_type"], "webhook");
    }

    #[test]
    fn automation_catalog_includes_local_transform_action() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let catalog = handle_automation_catalog(&paths).unwrap();
        let actions = catalog["actions"].as_array().unwrap();
        let action = actions
            .iter()
            .find(|action| action["id"] == "agentenv:transform_js:local-transform")
            .expect("local transform action");

        assert_eq!(action["kind"], "agentenv_node");
        assert_eq!(action["runtime_owner"], "agentenv");
        assert_eq!(action["label"], "Local JavaScript Transform");
        assert_eq!(action["node_ref"]["node_type"], "transform_js");
        assert!(action["node_ref"]["config"]["code"]
            .as_str()
            .unwrap()
            .contains("local_transform_result"));
    }

    #[test]
    fn automation_catalog_actions_include_connector_actions() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let catalog = handle_automation_catalog(&paths).unwrap();
        let actions = catalog["actions"].as_array().unwrap();

        assert!(actions.iter().any(|action| {
            action["kind"] == "connector_action"
                && action["runtime_owner"] == "puffer"
                && action["node_ref"]["node_type"] == "puffer_connector_action"
                && action["connector_slug"].as_str().is_some()
                && action["action"].as_str().is_some()
        }));
        assert!(actions
            .iter()
            .any(|action| action["label"] == "Local JavaScript Transform"));
    }

    #[test]
    fn automation_catalog_exposes_supported_telegram_connector_actions() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());
        let catalog = handle_automation_catalog(&paths).unwrap();
        let actions = catalog["actions"].as_array().unwrap();
        let telegram_actions = actions
            .iter()
            .filter(|action| action["connector_slug"] == "telegram-login")
            .filter_map(|action| {
                Some((
                    action["action"].as_str()?,
                    action["runtime_owner"].as_str()?,
                    action["node_ref"]["node_type"].as_str()?,
                    action["node_ref"]["config"]["action"].as_str()?,
                ))
            })
            .collect::<Vec<_>>();
        let slugs = telegram_actions
            .iter()
            .map(|(slug, _, _, _)| *slug)
            .collect::<BTreeSet<_>>();

        for expected in [
            "send_message",
            "vote_poll",
            "edit_message",
            "delete_messages",
            "forward_messages",
            "pin_message",
            "react",
            "mark_read",
            "update_group_title",
            "send_story",
        ] {
            assert!(
                slugs.contains(expected),
                "automation catalog should expose Telegram connector action `{expected}`"
            );
        }
        for unsupported in ["list_messages", "search_messages", "get_chat_info"] {
            assert!(
                !slugs.contains(unsupported),
                "internal Telegram read command `{unsupported}` must not be advertised as a connector action"
            );
        }
        for (slug, runtime_owner, node_type, config_action) in telegram_actions {
            assert_eq!(runtime_owner, "puffer");
            assert_eq!(node_type, "puffer_connector_action");
            assert_eq!(config_action, slug);
        }
    }

    #[test]
    fn automation_save_rejects_unknown_and_record_metadata_fields() {
        let (_tempdir, store) = store();
        for key in [
            "runtime",
            "revision",
            "created_at_ms",
            "updated_at_ms",
            "extra",
        ] {
            let mut params = save_create_params();
            params.as_object_mut().unwrap().insert(key.into(), json!(1));

            let error = handle_automation_save(&store, &params).unwrap_err();

            assert!(error.to_string().contains("does not accept"));
        }
    }

    #[test]
    fn automation_save_rejects_puffer_agent_runtime_config_fields() {
        let (_tempdir, store) = store();
        let mut spec = sample_spec("Draft a reply for review.");
        let mut config = BTreeMap::new();
        config.insert("content".into(), json!("runtime prompt"));
        spec.flow.steps = vec![AutomationStepSpec::AgentEnvNode {
            id: "agent".into(),
            node: AgentEnvNodeRef {
                node_type: "puffer_agent".into(),
                name: Some("Agent".into()),
                trusted: Some(true),
                config,
            },
            summary: Some("Run the Agent".into()),
        }];

        let error = handle_automation_save(
            &store,
            &json!({
                "id": "reply-helper",
                "status": "paused",
                "spec": spec,
            }),
        )
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("puffer_agent"));
        assert!(message.contains("content"));
        assert!(message.contains("persisted product semantics"));
    }

    #[test]
    fn automation_dto_strips_internal_runtime_artifact_ids() {
        let (_tempdir, store) = store();
        handle_automation_save(&store, &save_create_params()).unwrap();
        let record = store.get("reply-helper").unwrap();
        store
            .replace_runtime(
                "reply-helper",
                record.revision,
                AutomationRuntimeState {
                    spec_hash: Some(automation_spec_hash(&record.spec).unwrap()),
                    compiled_revision: Some(record.revision),
                    status: AutomationRuntimeStatus::Deployed,
                    agentenv_workflows: vec![CompiledAgentEnvWorkflow {
                        role: CompiledWorkflowRole::Root,
                        workflow_id: Some("Wf_01HX.Runtime:123".into()),
                        definition_hash: Some(
                            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                                .into(),
                        ),
                        deployed: true,
                    }],
                    puffer_bindings: vec![CompiledPufferBinding {
                        trigger_id: "incoming".into(),
                        binding_slug: "reply-helper-binding".into(),
                    }],
                    last_error: None,
                },
            )
            .unwrap();

        let value = handle_automation_get(&store, &json!({"id": "reply-helper"})).unwrap();
        let body = serde_json::to_string(&value).unwrap();

        assert_eq!(value["runtime"]["agentenv_workflow_count"], 1);
        assert_eq!(value["runtime"]["puffer_binding_count"], 1);
        assert!(!body.contains("Wf_01HX.Runtime:123"));
        assert!(!body.contains("reply-helper-binding"));
        assert!(!body.contains("definition_hash"));
    }

    #[test]
    fn automation_delete_removes_record() {
        let (_tempdir, store) = store();
        handle_automation_save(&store, &save_create_params()).unwrap();

        let deleted =
            handle_automation_delete(&store, &json!({"automation_id": "reply-helper"})).unwrap();

        assert_eq!(deleted, json!({"id": "reply-helper", "deleted": true}));
        assert!(handle_automation_get(&store, &json!({"id": "reply-helper"})).is_err());
    }
}
