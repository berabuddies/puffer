//! Workflow inspection, validation, listing, and toggle tools.

use crate::runtime::subscription_manager;
use crate::AppState;
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    connection_subscriber_manifest, connection_subscriber_manifest_exists, validate_action_spec,
    ActionSpec, ConnectionRecord, ConnectorTemplate, FilterSpec, SubscriberManifestRoots,
    TaggedFilterSpec, WorkflowBindingRun, WorkflowBindingSpec, WorkflowBindingStatus,
};
use puffer_workflow::{WorkflowDefinition, WorkflowRun, WorkflowStore};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Deserialize)]
struct WorkflowToggleInput {
    slug: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct WorkflowValidateInput {
    #[serde(default)]
    action: Option<Value>,
    #[serde(default)]
    yaml_action: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkflowCreateInput {
    slug: String,
    #[serde(default)]
    description: Option<String>,
    connection_slug: String,
    #[serde(default)]
    filter: Option<Value>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    classify_prompt: Option<String>,
    #[serde(default)]
    classify_model: Option<String>,
    #[serde(default)]
    action: Option<Value>,
    #[serde(default)]
    yaml_action: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct WorkflowInspectInput {
    slug: String,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    run_idx: Option<u64>,
}

/// Executes `WorkflowList`.
pub fn execute_workflow_list(_state: &mut AppState, cwd: &Path, _input: Value) -> Result<String> {
    let store = workflow_store(cwd);
    let workflows = store.list()?;
    let runs = store.list_runs()?;
    let mut summaries = workflows
        .iter()
        .map(|workflow| workflow_summary(workflow, &runs))
        .collect::<Vec<_>>();
    if let Ok(manager) = subscription_manager() {
        let native_slugs = workflows
            .iter()
            .map(|workflow| workflow.slug.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let native_binding_slugs = workflows
            .iter()
            .map(|workflow| format!("workflow-{}", workflow.slug))
            .collect::<std::collections::BTreeSet<_>>();
        for binding in manager.store().list() {
            if !native_slugs.contains(binding.slug.as_str())
                && !native_binding_slugs.contains(&binding.slug)
            {
                let runs = manager.history_store().list_for(&binding.slug);
                summaries.push(binding_summary(&binding, &runs));
            }
        }
    }
    Ok(serde_json::to_string_pretty(&json!({
        "workflows": summaries,
    }))?)
}

/// Executes `WorkflowToggle`.
pub fn execute_workflow_toggle(_state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: WorkflowToggleInput =
        serde_json::from_value(input).context("invalid WorkflowToggle input")?;
    let store = workflow_store(cwd);
    if let Some(mut workflow) = store.get(&parsed.slug)? {
        workflow.enabled = parsed.enabled;
        let workflow = store.upsert(workflow)?;
        if let Ok(manager) = subscription_manager() {
            let binding_id = format!("workflow-{}", workflow.slug);
            if manager.store().get(&binding_id).is_some() {
                let status = workflow_status(parsed.enabled);
                manager.store().set_status(&binding_id, status)?;
                manager.refresh_connection_consumers()?;
            }
        }
        return Ok(serde_json::to_string_pretty(&workflow)?);
    }
    if let Ok(manager) = subscription_manager() {
        if manager.store().get(&parsed.slug).is_some() {
            let binding = manager
                .store()
                .set_status(&parsed.slug, workflow_status(parsed.enabled))?;
            manager.refresh_connection_consumers()?;
            return Ok(serde_json::to_string_pretty(&binding)?);
        }
    }
    anyhow::bail!("workflow `{}` not found", parsed.slug)
}

/// Executes `WorkflowCreate`.
pub fn execute_workflow_create(_state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: WorkflowCreateInput =
        serde_json::from_value(input).context("invalid WorkflowCreate input")?;
    let action = parse_create_action(parsed.action, parsed.yaml_action)?;
    validate_action_spec(&action).map_err(anyhow::Error::msg)?;
    let filter = create_filter(parsed.filter, parsed.pattern)?;
    let manager = subscription_manager()?;
    let connection = manager
        .connection_store()
        .get(&parsed.connection_slug)
        .ok_or_else(|| anyhow::anyhow!("connection `{}` not found", parsed.connection_slug))?;
    let template = manager
        .connector_store()
        .get(&connection.connector_slug)
        .ok_or_else(|| anyhow::anyhow!("connector `{}` not found", connection.connector_slug))?;
    if !workflow_trigger_supported(cwd, &connection, &template) {
        anyhow::bail!(
            "connector `{}` cannot produce workflow trigger events",
            connection.connector_slug
        );
    }
    let should_start_subscriber = parsed.enabled.unwrap_or(true);
    let spec = WorkflowBindingSpec {
        slug: parsed.slug,
        description: parsed.description.unwrap_or_default(),
        connection_slug: parsed.connection_slug,
        connector_slug: Some(connection.connector_slug.clone()),
        status: workflow_status(parsed.enabled.unwrap_or(true)),
        filter,
        classify_prompt: parsed.classify_prompt,
        classify_model: parsed.classify_model,
        action,
        created_at_ms: puffer_subscriptions::now_ms(),
    };
    let created_slug = spec.slug.clone();
    manager.store().create(spec.clone())?;
    let setup_result = (|| -> Result<()> {
        if should_start_subscriber {
            ensure_workflow_subscriber_started(&manager, cwd, &connection, &template)?;
        }
        manager.refresh_connection_consumers()?;
        Ok(())
    })();
    if let Err(error) = setup_result {
        let cleanup_result = manager
            .store()
            .delete(&created_slug)
            .map_err(anyhow::Error::from)
            .and_then(|_| manager.refresh_connection_consumers());
        if let Err(cleanup_error) = cleanup_result {
            anyhow::bail!(
                "workflow `{created_slug}` setup failed after create: {error:#}; rollback failed: {cleanup_error:#}"
            );
        }
        return Err(error).with_context(|| {
            format!("workflow `{created_slug}` setup failed after create; rolled back")
        });
    }
    Ok(serde_json::to_string_pretty(&spec)?)
}

/// Executes `WorkflowValidate`.
pub fn execute_workflow_validate(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: WorkflowValidateInput =
        serde_json::from_value(input).context("invalid WorkflowValidate input")?;
    let action = match (parsed.action, parsed.yaml_action) {
        (Some(action), None) => {
            serde_json::from_value::<ActionSpec>(action).context("invalid workflow action JSON")?
        }
        (None, Some(yaml)) => {
            serde_yaml::from_str::<ActionSpec>(&yaml).context("invalid workflow action YAML")?
        }
        (Some(_), Some(_)) => anyhow::bail!("provide only one of `action` or `yaml_action`"),
        (None, None) => anyhow::bail!("provide `action` JSON or `yaml_action`"),
    };
    validate_action_spec(&action).map_err(anyhow::Error::msg)?;
    Ok(serde_json::to_string_pretty(&json!({
        "valid": true,
        "action": action,
    }))?)
}

/// Executes `WorkflowInspect`.
pub fn execute_workflow_inspect(_state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: WorkflowInspectInput =
        serde_json::from_value(input).context("invalid WorkflowInspect input")?;
    let store = workflow_store(cwd);
    if let Some(workflow) = store.get(&parsed.slug)? {
        let runs = store.list_runs_for(&parsed.slug)?;
        if let Some(run) = select_run(&runs, parsed.run_idx, parsed.run_id.as_deref()) {
            return Ok(serde_json::to_string_pretty(&json!({
                "workflow": workflow,
                "run": run_details(run),
            }))?);
        }
        return Ok(serde_json::to_string_pretty(&json!({
            "workflow": workflow,
            "historical_runs": runs.iter().map(run_summary).collect::<Vec<_>>(),
        }))?);
    }
    if let Ok(manager) = subscription_manager() {
        if let Some(binding) = manager.store().get(&parsed.slug) {
            let runs = manager.history_store().list_for(&parsed.slug);
            if let Some(run) = select_binding_run(&runs, parsed.run_idx, parsed.run_id.as_deref()) {
                return Ok(serde_json::to_string_pretty(&json!({
                    "workflow": binding,
                    "run": binding_run_details(run),
                }))?);
            }
            return Ok(serde_json::to_string_pretty(&json!({
                "workflow": binding,
                "historical_runs": runs.iter().map(binding_run_summary).collect::<Vec<_>>(),
            }))?);
        }
    }
    anyhow::bail!("workflow `{}` not found", parsed.slug)
}

fn workflow_store(cwd: &Path) -> WorkflowStore {
    let paths = ConfigPaths::discover(cwd);
    WorkflowStore::new(&paths.workspace_config_dir)
}

fn workflow_trigger_supported(
    cwd: &Path,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> bool {
    connector_stream_supported(template)
        || connection_subscriber_manifest_exists(
            &subscriber_manifest_roots(cwd),
            connection,
            template,
        )
}

fn connector_stream_supported(template: &ConnectorTemplate) -> bool {
    template.can_subscribe && template.command_argv().is_some()
}

fn ensure_workflow_subscriber_started(
    manager: &puffer_subscriptions::SubscriptionManager,
    cwd: &Path,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> Result<()> {
    if connector_stream_supported(template) {
        return Ok(());
    }
    let Some(manifest) =
        connection_subscriber_manifest(&subscriber_manifest_roots(cwd), connection, template)?
    else {
        return Ok(());
    };
    if manager
        .subscriber_ids()
        .iter()
        .any(|subscriber_id| subscriber_id == &manifest.spec.id)
    {
        return Ok(());
    }
    manager.start_subscriber(manifest)?;
    Ok(())
}

fn subscriber_manifest_roots(cwd: &Path) -> SubscriberManifestRoots {
    let paths = ConfigPaths::discover(cwd);
    SubscriberManifestRoots::new(
        paths.workspace_config_dir,
        paths.user_config_dir,
        paths.builtin_resources_dir,
    )
}

fn workflow_summary(workflow: &WorkflowDefinition, runs: &[WorkflowRun]) -> Value {
    let latest = runs
        .iter()
        .filter(|run| run.workflow_slug == workflow.slug)
        .max_by_key(|run| run.idx)
        .map(run_summary);
    json!({
        "slug": workflow.slug,
        "enabled": workflow.enabled,
        "trigger": workflow.trigger,
        "node_count": workflow.pipeline.nodes.len(),
        "latest_run": latest,
    })
}

fn binding_summary(binding: &WorkflowBindingSpec, runs: &[WorkflowBindingRun]) -> Value {
    let latest = runs
        .iter()
        .max_by_key(|run| run.idx)
        .map(binding_run_summary);
    json!({
        "slug": binding.slug,
        "description": binding.description,
        "enabled": binding.status == WorkflowBindingStatus::Enabled,
        "trigger": {
            "type": "connection",
            "connection_slug": binding.connection_slug,
            "connector_slug": binding.connector_slug,
            "filter": binding.filter,
            "classify_prompt": binding.classify_prompt,
        },
        "node_count": match &binding.action {
            ActionSpec::Graph { nodes } => nodes.len(),
            _ => 1,
        },
        "latest_run": latest,
    })
}

fn run_summary(run: &WorkflowRun) -> Value {
    json!({
        "run_idx": run.idx,
        "run_id": run.run_id,
        "trigger_info": run.trigger,
        "action_summary": {
            "status": run.status,
            "nodes": run.nodes.iter().map(|node| {
                json!({
                    "id": node.id,
                    "status": node.status,
                    "output": node.output,
                    "error": node.error,
                })
            }).collect::<Vec<_>>(),
            "error": run.error,
        },
    })
}

fn run_details(run: &WorkflowRun) -> Value {
    json!({
        "run_idx": run.idx,
        "run_id": run.run_id,
        "trigger_info": run.trigger,
        "action_log": run.nodes.iter().map(|node| {
            json!({
                "id": node.id,
                "status": node.status,
                "started_at_ms": node.started_at_ms,
                "ended_at_ms": node.ended_at_ms,
                "output": node.output,
                "error": node.error,
            })
        }).collect::<Vec<_>>(),
        "status": run.status,
        "error": run.error,
    })
}

fn binding_run_summary(run: &WorkflowBindingRun) -> Value {
    json!({
        "run_idx": run.idx,
        "run_id": run.run_id,
        "trigger_info": run.trigger_info,
        "action_summary": run.action_summary,
    })
}

fn binding_run_details(run: &WorkflowBindingRun) -> Value {
    json!({
        "run_idx": run.idx,
        "run_id": run.run_id,
        "trigger_info": run.trigger_info,
        "action_log": run.action_log,
        "status": run.status,
    })
}

fn select_run<'a>(
    runs: &'a [WorkflowRun],
    idx: Option<u64>,
    run_id: Option<&str>,
) -> Option<&'a WorkflowRun> {
    if let Some(idx) = idx {
        return runs.iter().find(|run| run.idx == idx);
    }
    run_id.and_then(|run_id| runs.iter().find(|run| run.run_id == run_id))
}

fn select_binding_run<'a>(
    runs: &'a [WorkflowBindingRun],
    idx: Option<u64>,
    run_id: Option<&str>,
) -> Option<&'a WorkflowBindingRun> {
    if let Some(idx) = idx {
        return runs.iter().find(|run| run.idx == idx);
    }
    run_id.and_then(|run_id| runs.iter().find(|run| run.run_id == run_id))
}

fn workflow_status(enabled: bool) -> WorkflowBindingStatus {
    if enabled {
        WorkflowBindingStatus::Enabled
    } else {
        WorkflowBindingStatus::Paused
    }
}

fn parse_create_action(action: Option<Value>, yaml_action: Option<String>) -> Result<ActionSpec> {
    match (action, yaml_action) {
        (Some(action), None) => {
            serde_json::from_value::<ActionSpec>(action).context("invalid workflow action JSON")
        }
        (None, Some(yaml)) => {
            serde_yaml::from_str::<ActionSpec>(&yaml).context("invalid workflow action YAML")
        }
        (Some(_), Some(_)) => anyhow::bail!("provide only one of `action` or `yaml_action`"),
        (None, None) => anyhow::bail!("provide `action` JSON or `yaml_action`"),
    }
}

fn create_filter(filter: Option<Value>, pattern: Option<String>) -> Result<Option<FilterSpec>> {
    match (filter, pattern) {
        (Some(filter), _) => serde_json::from_value::<FilterSpec>(filter)
            .context("invalid workflow filter")
            .map(Some),
        (None, Some(pattern)) => Ok(Some(FilterSpec::Tagged(TaggedFilterSpec::Regex {
            pattern,
            case_insensitive: true,
        }))),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{subscriber_manifest_roots, workflow_trigger_supported};
    use puffer_subscriptions::{
        find_subscriber_manifest, ConnectionRecord, ConnectorSubscriberTemplate, ConnectorTemplate,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn subscriber_manifest_dir_finds_workspace_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let manifest_dir = temp.path().join(".puffer/subscribers/email");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(manifest_dir.join("manifest.toml"), "").unwrap();

        assert_eq!(
            find_subscriber_manifest(&subscriber_manifest_roots(temp.path()), "email").unwrap(),
            manifest_dir
        );
    }

    #[test]
    fn subscriber_manifest_dir_finds_builtin_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let manifest_dir = temp.path().join("resources/subscribers/email");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(manifest_dir.join("manifest.toml"), "").unwrap();

        assert_eq!(
            find_subscriber_manifest(&subscriber_manifest_roots(temp.path()), "email").unwrap(),
            manifest_dir
        );
    }

    #[test]
    fn workflow_trigger_support_rejects_action_only_connectors() {
        let temp = tempfile::tempdir().unwrap();
        let connection = ConnectionRecord::authenticated("work-lark", "lark-login", "demo");
        let template = connector_template("lark-login", false);

        assert!(!workflow_trigger_supported(
            temp.path(),
            &connection,
            &template
        ));
    }

    #[test]
    fn workflow_trigger_support_rejects_commandless_subscribe_templates() {
        let temp = tempfile::tempdir().unwrap();
        let connection = ConnectionRecord::authenticated("work-bot", "telegram-bot", "demo");
        let template = connector_template("telegram-bot", true);

        assert!(!workflow_trigger_supported(
            temp.path(),
            &connection,
            &template
        ));
    }

    #[test]
    fn workflow_trigger_support_accepts_command_backed_subscribe_templates() {
        let temp = tempfile::tempdir().unwrap();
        let connection = ConnectionRecord::authenticated("custom", "custom-sub", "demo");
        let mut template = connector_template("custom-sub", true);
        template.command = vec!["custom-sub".into()];

        assert!(workflow_trigger_supported(
            temp.path(),
            &connection,
            &template
        ));
    }

    #[test]
    fn workflow_trigger_support_accepts_configured_shared_subscriber() {
        let temp = tempfile::tempdir().unwrap();
        let manifest_dir = temp.path().join("resources/subscribers/shared-login");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(manifest_dir.join("manifest.toml"), "").unwrap();
        let connection = ConnectionRecord::authenticated("personal", "shared", "demo");
        let mut template = connector_template("shared", true);
        template.subscriber = Some(ConnectorSubscriberTemplate {
            manifest_slug: "shared-login".to_string(),
            state_root: Some("shared-accounts".to_string()),
            display_name: Some("Shared".to_string()),
        });

        assert!(workflow_trigger_supported(
            temp.path(),
            &connection,
            &template
        ));
    }

    #[test]
    fn workflow_trigger_support_accepts_legacy_manifest_topic() {
        let temp = tempfile::tempdir().unwrap();
        let manifest_dir = temp.path().join("resources/subscribers/email");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(manifest_dir.join("manifest.toml"), "").unwrap();
        let connection = ConnectionRecord::authenticated("work-email", "email", "demo");
        let template = connector_template("email", false);

        assert!(workflow_trigger_supported(
            temp.path(),
            &connection,
            &template
        ));
    }

    fn connector_template(slug: &str, can_subscribe: bool) -> ConnectorTemplate {
        ConnectorTemplate {
            slug: slug.to_string(),
            description: String::new(),
            skill: String::new(),
            binary: String::new(),
            command: Vec::new(),
            requires_auth: true,
            can_subscribe,
            can_proxy_agent: false,
            subscriber: None,
            output_schema: json!({}),
            actions: BTreeMap::new(),
        }
    }
}
