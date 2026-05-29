//! Workflow inspection, validation, listing, and toggle tools.

use crate::runtime::subscription_manager;
use crate::AppState;
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    connection_subscriber_manifest, connection_workflow_trigger_supported,
    connector_workflow_trigger_supported, suggested_connection_slug, validate_action_spec,
    ActionGraphNode, ActionSpec, ConnectionRecord, ConnectorTemplate, FilterSpec,
    SubscriberManifestRoots, TaggedFilterSpec, WorkflowBindingRun, WorkflowBindingSpec,
    WorkflowBindingStatus,
};
use puffer_workflow::{WorkflowDefinition, WorkflowRun, WorkflowStore};
use serde::Deserialize;
use serde_json::{json, Map, Value};
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
    connector_slug: Option<String>,
    #[serde(default)]
    filter: Option<Value>,
    #[serde(default)]
    pattern: Option<String>,
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
    let roots = subscriber_manifest_roots(cwd);
    let connectors = manager.connector_store().list_with_builtins();
    let connections = manager.connection_store().list();
    let (connection, connector_slug, template) = resolve_create_trigger_from_context(
        &roots,
        &connectors,
        &connections,
        &parsed.connection_slug,
        parsed.connector_slug.as_deref(),
    )?;
    let should_start_subscriber = parsed.enabled.unwrap_or(true) && connection.is_some();
    let spec = WorkflowBindingSpec {
        slug: parsed.slug,
        description: parsed.description.unwrap_or_default(),
        connection_slug: parsed.connection_slug,
        connector_slug: Some(connector_slug),
        status: workflow_status(parsed.enabled.unwrap_or(true)),
        filter,
        ignore_filters: Vec::new(),
        classify_prompt: parsed.classify_prompt,
        classify_model: parsed.classify_model,
        action,
        created_at_ms: puffer_subscriptions::now_ms(),
    };
    let created_slug = spec.slug.clone();
    manager.store().create(spec.clone())?;
    let setup_result = (|| -> Result<()> {
        if should_start_subscriber {
            if let Some(connection) = connection.as_ref() {
                ensure_workflow_subscriber_started(&manager, cwd, connection, &template)?;
            }
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

fn resolve_create_trigger_from_context(
    roots: &SubscriberManifestRoots,
    connectors: &[ConnectorTemplate],
    connections: &[ConnectionRecord],
    connection_slug: &str,
    connector_slug: Option<&str>,
) -> Result<(Option<ConnectionRecord>, String, ConnectorTemplate)> {
    if let Some(connection) = connections
        .iter()
        .find(|connection| connection.slug == connection_slug)
    {
        if let Some(connector_slug) = connector_slug {
            if connector_slug != connection.connector_slug {
                anyhow::bail!(
                    "connection `{}` uses connector `{}`, not `{connector_slug}`",
                    connection.slug,
                    connection.connector_slug
                );
            }
        }
        let template = connectors
            .iter()
            .find(|template| template.slug == connection.connector_slug)
            .ok_or_else(|| {
                anyhow::anyhow!("connector `{}` not found", connection.connector_slug)
            })?;
        if !connection_workflow_trigger_supported(roots, connection, template) {
            anyhow::bail!(
                "connector `{}` cannot produce workflow trigger events",
                connection.connector_slug
            );
        }
        return Ok((
            Some(connection.clone()),
            connection.connector_slug.clone(),
            template.clone(),
        ));
    }

    resolve_planned_create_trigger(roots, connectors, connection_slug, connector_slug)
        .map(|(connector_slug, template)| (None, connector_slug, template))
}

fn resolve_planned_create_trigger(
    roots: &SubscriberManifestRoots,
    connectors: &[ConnectorTemplate],
    connection_slug: &str,
    connector_slug: Option<&str>,
) -> Result<(String, ConnectorTemplate)> {
    if let Some(connector_slug) = connector_slug
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let template = connectors
            .iter()
            .find(|template| template.slug == connector_slug)
            .ok_or_else(|| anyhow::anyhow!("connector `{connector_slug}` not found"))?;
        if !connector_workflow_trigger_supported(roots, template) {
            anyhow::bail!("connector `{connector_slug}` cannot produce workflow trigger events");
        }
        return Ok((connector_slug.to_string(), template.clone()));
    }

    let matches = connectors
        .iter()
        .filter(|template| {
            suggested_connection_slug(&template.slug) == connection_slug
                && connector_workflow_trigger_supported(roots, template)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [template] => Ok((template.slug.clone(), (*template).clone())),
        [] => anyhow::bail!(
            "connection `{connection_slug}` not found; provide `connector_slug` for a planned workflow connection or run `/connect` first"
        ),
        _ => anyhow::bail!(
            "connection `{connection_slug}` is ambiguous; provide `connector_slug` for the planned workflow connection"
        ),
    }
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
            parse_action_json(action).context("invalid workflow action JSON")?
        }
        (None, Some(yaml)) => parse_action_yaml(&yaml).context("invalid workflow action YAML")?,
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
    connection_workflow_trigger_supported(&subscriber_manifest_roots(cwd), connection, template)
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
        (Some(action), None) => parse_action_json(action).context("invalid workflow action JSON"),
        (None, Some(yaml)) => parse_action_yaml(&yaml).context("invalid workflow action YAML"),
        (Some(_), Some(_)) => anyhow::bail!("provide only one of `action` or `yaml_action`"),
        (None, None) => anyhow::bail!("provide `action` JSON or `yaml_action`"),
    }
}

fn parse_action_json(value: Value) -> Result<ActionSpec> {
    match serde_json::from_value::<ActionSpec>(value.clone()) {
        Ok(action) => Ok(action),
        Err(strict_error) => parse_action_shorthand(value).map_err(|shorthand_error| {
            anyhow::anyhow!("{shorthand_error}; strict action parse failed: {strict_error}")
        }),
    }
}

fn parse_action_yaml(yaml: &str) -> Result<ActionSpec> {
    match serde_yaml::from_str::<ActionSpec>(yaml) {
        Ok(action) => Ok(action),
        Err(strict_error) => {
            let value = serde_yaml::from_str::<Value>(yaml)
                .with_context(|| format!("strict action parse failed: {strict_error}"))?;
            parse_action_shorthand(value).map_err(|shorthand_error| {
                anyhow::anyhow!("{shorthand_error}; strict action parse failed: {strict_error}")
            })
        }
    }
}

fn parse_action_shorthand(value: Value) -> Result<ActionSpec> {
    match value {
        Value::Array(items) => parse_step_graph(items),
        Value::Object(mut map) => {
            if let Some(steps) = map.remove("steps") {
                return parse_step_graph_value(steps);
            }
            if let Some(nodes) = map.remove("nodes") {
                if map.is_empty() {
                    return parse_step_graph_value(nodes);
                }
                map.insert("nodes".to_string(), nodes);
            }
            parse_named_action_object(map)
        }
        _ => anyhow::bail!("workflow action shorthand must be an object or list"),
    }
}

fn parse_step_graph_value(value: Value) -> Result<ActionSpec> {
    let Value::Array(items) = value else {
        anyhow::bail!("workflow action steps must be a list");
    };
    parse_step_graph(items)
}

fn parse_step_graph(items: Vec<Value>) -> Result<ActionSpec> {
    if items.is_empty() {
        anyhow::bail!("workflow action steps must not be empty");
    }
    let mut nodes = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        nodes.push(parse_step_node(index, item)?);
    }
    Ok(ActionSpec::Graph { nodes })
}

fn parse_step_node(index: usize, value: Value) -> Result<ActionGraphNode> {
    let mut map = value_object(value, "workflow action step")?;
    let id = optional_string(&mut map, "id")?.unwrap_or_else(|| format!("step-{}", index + 1));
    let depends_on = optional_string_list(&mut map, "depends_on")?
        .or(optional_string_list(&mut map, "dependsOn")?)
        .unwrap_or_default();
    let action = if map.contains_key("type") {
        parse_action_json(Value::Object(map))?
    } else {
        parse_named_action_object(map)?
    };
    Ok(ActionGraphNode {
        id,
        depends_on,
        action,
    })
}

fn parse_named_action_object(map: Map<String, Value>) -> Result<ActionSpec> {
    if map.len() != 1 {
        anyhow::bail!(
            "workflow action shorthand must contain exactly one action key, such as `file_append`"
        );
    }
    let (name, params) = map.into_iter().next().expect("map length checked");
    parse_named_action(&name, params)
}

fn parse_named_action(name: &str, params: Value) -> Result<ActionSpec> {
    match name {
        "file_append" | "append_file" | "write_file" | "write" => parse_file_append_action(params),
        _ => anyhow::bail!("unsupported workflow action shorthand `{name}`"),
    }
}

fn parse_file_append_action(params: Value) -> Result<ActionSpec> {
    let mut map = value_object(params, "file_append action")?;
    let path = required_string(&mut map, "path")?;
    if let Some(content) = optional_string(&mut map, "content")? {
        validate_event_text_content_template(&content)?;
    }
    let mut action = json!({
        "type": "file_append",
        "path": path,
    });
    if let Some(format) = optional_string(&mut map, "format")? {
        validate_file_append_format(&format)?;
        action["format"] = json!(format);
    }
    serde_json::from_value::<ActionSpec>(action).context("normalized file_append action")
}

fn validate_file_append_format(format: &str) -> Result<()> {
    match format {
        "text" | "jsonl" => Ok(()),
        _ => anyhow::bail!("file_append.format must be `text` or `jsonl`"),
    }
}

fn validate_event_text_content_template(content: &str) -> Result<()> {
    let normalized = content
        .trim()
        .replace(' ', "")
        .replace('\n', "")
        .replace('\r', "");
    if matches!(normalized.as_str(), "{{text}}" | "{{event.text}}") {
        return Ok(());
    }
    anyhow::bail!(
        "write_file shorthand only supports event text content; use `{{{{ text }}}}` or omit content"
    )
}

fn value_object(value: Value, label: &str) -> Result<Map<String, Value>> {
    match value {
        Value::Object(map) => Ok(map),
        _ => anyhow::bail!("{label} must be an object"),
    }
}

fn required_string(map: &mut Map<String, Value>, key: &str) -> Result<String> {
    optional_string(map, key)?.ok_or_else(|| anyhow::anyhow!("{key} is required"))
}

fn optional_string(map: &mut Map<String, Value>, key: &str) -> Result<Option<String>> {
    let Some(value) = map.remove(key) else {
        return Ok(None);
    };
    match value {
        Value::String(value) => {
            let value = value.trim();
            if value.is_empty() {
                anyhow::bail!("{key} must not be empty");
            }
            Ok(Some(value.to_string()))
        }
        _ => anyhow::bail!("{key} must be a string"),
    }
}

fn optional_string_list(map: &mut Map<String, Value>, key: &str) -> Result<Option<Vec<String>>> {
    let Some(value) = map.remove(key) else {
        return Ok(None);
    };
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Value::String(value) if !value.trim().is_empty() => Ok(value.trim().to_string()),
                _ => anyhow::bail!("{key} must be a list of non-empty strings"),
            })
            .collect::<Result<Vec<_>>>()
            .map(Some),
        Value::String(value) if !value.trim().is_empty() => {
            Ok(Some(vec![value.trim().to_string()]))
        }
        _ => anyhow::bail!("{key} must be a string or list of strings"),
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
    use super::{
        parse_action_yaml, resolve_create_trigger_from_context, subscriber_manifest_roots,
        workflow_trigger_supported,
    };
    use puffer_subscriptions::{
        find_subscriber_manifest, ActionSpec, ConnectionRecord, ConnectorSubscriberTemplate,
        ConnectorTemplate,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn workflow_action_yaml_accepts_step_style_write_file_event_text() {
        let action = parse_action_yaml(
            r#"
steps:
  - write_file:
      path: /tmp/hi
      content: "{{ event.text }}\n"
"#,
        )
        .unwrap();

        let ActionSpec::Graph { nodes } = action else {
            panic!("expected action graph");
        };
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "step-1");
        assert!(nodes[0].depends_on.is_empty());
        assert!(matches!(
            &nodes[0].action,
            ActionSpec::FileAppend {
                path,
                ..
            } if path == "/tmp/hi"
        ));
    }

    #[test]
    fn workflow_action_yaml_accepts_named_file_append_shorthand() {
        let action = parse_action_yaml(
            r#"
file_append:
  path: /tmp/hi
  format: jsonl
"#,
        )
        .unwrap();

        assert!(matches!(
            &action,
            ActionSpec::FileAppend {
                path,
                ..
            } if path == "/tmp/hi"
        ));
        assert_eq!(
            serde_json::to_value(action).unwrap()["format"],
            json!("jsonl")
        );
    }

    #[test]
    fn workflow_action_yaml_rejects_write_file_static_content() {
        let error = parse_action_yaml(
            r#"
write_file:
  path: /tmp/hi
  content: static
"#,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("only supports event text content"));
    }

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

    #[test]
    fn workflow_create_trigger_uses_existing_connection() {
        let temp = tempfile::tempdir().unwrap();
        let roots = subscriber_manifest_roots(temp.path());
        let mut template = connector_template("email", true);
        template.command = vec!["puffer-subscriber-email".to_string()];
        let connection = ConnectionRecord::authenticated("personal-email", "email", "demo");

        let (connection, connector_slug, _) = resolve_create_trigger_from_context(
            &roots,
            &[template],
            &[connection],
            "personal-email",
            None,
        )
        .unwrap();

        assert!(connection.is_some());
        assert_eq!(connector_slug, "email");
    }

    #[test]
    fn workflow_create_trigger_infers_planned_suggested_connection() {
        let temp = tempfile::tempdir().unwrap();
        let roots = subscriber_manifest_roots(temp.path());
        let mut template = connector_template("email", true);
        template.command = vec!["puffer-subscriber-email".to_string()];

        let (connection, connector_slug, _) =
            resolve_create_trigger_from_context(&roots, &[template], &[], "email", None).unwrap();

        assert!(connection.is_none());
        assert_eq!(connector_slug, "email");
    }

    #[test]
    fn workflow_create_trigger_accepts_explicit_planned_connector() {
        let temp = tempfile::tempdir().unwrap();
        let roots = subscriber_manifest_roots(temp.path());
        let mut template = connector_template("email", true);
        template.command = vec!["puffer-subscriber-email".to_string()];

        let (connection, connector_slug, _) = resolve_create_trigger_from_context(
            &roots,
            &[template],
            &[],
            "team-mail",
            Some("email"),
        )
        .unwrap();

        assert!(connection.is_none());
        assert_eq!(connector_slug, "email");
    }

    #[test]
    fn workflow_create_trigger_rejects_unknown_planned_connector() {
        let temp = tempfile::tempdir().unwrap();
        let roots = subscriber_manifest_roots(temp.path());

        let error =
            resolve_create_trigger_from_context(&roots, &[], &[], "team-mail", None).unwrap_err();

        assert!(error.to_string().contains("provide `connector_slug`"));
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
