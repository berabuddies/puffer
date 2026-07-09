use anyhow::{Context, Result};
use puffer_config::{load_config, ConfigPaths};
use puffer_workflow::{
    WorkflowRuntimeCreateWorkflowRequest, WorkflowRuntimeInMemoryExecuteRequest,
    WorkflowRuntimeRecord, WorkflowRuntimeUpdateWorkflowRequest,
};
use serde_json::Value;

const WORKFLOW_ID_KEYS: &[&str] = &[
    "workflowId",
    "workflow_id",
    "workflowSlug",
    "workflow_slug",
    "id",
];

/// Creates one workflow in the configured runtime.
pub(crate) fn handle_workflow_create(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let client = workflow_runtime_client(paths)?;
    let request = typed_param_or_root::<WorkflowRuntimeCreateWorkflowRequest>(
        params,
        "workflow",
        "workflow_create",
    )?;
    Ok(serde_json::to_value(runtime_call(
        client.create_workflow(&request),
    )?)?)
}

/// Updates one workflow draft in the configured runtime.
pub(crate) fn handle_workflow_update(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let client = workflow_runtime_client(paths)?;
    let workflow_id = required_string(params, WORKFLOW_ID_KEYS, "workflow id")?;
    let request = typed_param_or_root::<WorkflowRuntimeUpdateWorkflowRequest>(
        params,
        "workflow",
        "workflow_update",
    )?;
    Ok(serde_json::to_value(runtime_call(
        client.update_workflow(&workflow_id, &request),
    )?)?)
}

/// Deploys one workflow in the configured runtime.
pub(crate) fn handle_workflow_deploy(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let client = workflow_runtime_client(paths)?;
    let workflow_id = required_string(params, WORKFLOW_ID_KEYS, "workflow id")?;
    Ok(serde_json::to_value(runtime_call(
        client.deploy_workflow(&workflow_id),
    )?)?)
}

/// Undeploys one workflow in the configured runtime.
pub(crate) fn handle_workflow_undeploy(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let client = workflow_runtime_client(paths)?;
    let workflow_id = required_string(params, WORKFLOW_ID_KEYS, "workflow id")?;
    Ok(serde_json::to_value(runtime_call(
        client.undeploy_workflow(&workflow_id),
    )?)?)
}

/// Lists AgentEnv node definitions from the configured runtime.
pub(crate) fn handle_workflow_node_definitions(paths: &ConfigPaths) -> Result<Value> {
    let client = workflow_runtime_client(paths)?;
    Ok(serde_json::to_value(runtime_call(
        client.list_node_definitions(),
    )?)?)
}

/// Fetches one AgentEnv node definition from the configured runtime.
pub(crate) fn handle_workflow_node_definition(
    paths: &ConfigPaths,
    params: &Value,
) -> Result<Value> {
    let client = workflow_runtime_client(paths)?;
    let node_type = required_string(params, &["type", "nodeType", "node_type"], "node type")?;
    Ok(serde_json::to_value(runtime_call(
        client.get_node_definition(&node_type),
    )?)?)
}

/// Executes one workflow in the configured runtime.
pub(crate) fn handle_workflow_execute(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let client = workflow_runtime_client(paths)?;
    let workflow_id = required_string(params, WORKFLOW_ID_KEYS, "workflow id")?;
    let request = workflow_execute_request(params)?;
    Ok(serde_json::to_value(runtime_call(
        client.execute_workflow(&workflow_id, &request),
    )?)?)
}

/// Executes an in-memory workflow definition in the configured runtime.
pub(crate) fn handle_workflow_execute_in_memory(
    paths: &ConfigPaths,
    params: &Value,
) -> Result<Value> {
    let client = workflow_runtime_client(paths)?;
    let request = typed_param_or_root::<WorkflowRuntimeInMemoryExecuteRequest>(
        params,
        "request",
        "workflow_execute_in_memory",
    )?;
    Ok(serde_json::to_value(runtime_call(
        client.execute_in_memory(&request),
    )?)?)
}

/// Lists executions for one workflow from the configured runtime.
pub(crate) fn handle_workflow_list_executions(
    paths: &ConfigPaths,
    params: &Value,
) -> Result<Value> {
    let client = workflow_runtime_client(paths)?;
    let workflow_id = required_string(params, WORKFLOW_ID_KEYS, "workflow id")?;
    Ok(serde_json::to_value(runtime_call(
        client.list_executions(&workflow_id),
    )?)?)
}

/// Fetches one workflow execution from the configured runtime.
pub(crate) fn handle_workflow_get_execution(paths: &ConfigPaths, params: &Value) -> Result<Value> {
    let client = workflow_runtime_client(paths)?;
    let workflow_id = required_string(params, WORKFLOW_ID_KEYS, "workflow id")?;
    let execution_id = required_string(
        params,
        &["executionId", "execution_id", "runId", "run_id"],
        "execution id",
    )?;
    Ok(serde_json::to_value(runtime_call(
        client.get_execution(&workflow_id, &execution_id),
    )?)?)
}

fn workflow_runtime_client(paths: &ConfigPaths) -> Result<puffer_workflow::WorkflowRuntimeClient> {
    let config = load_config(paths).context("load workflow backend config")?;
    crate::daemon_workflow_runtime::workflow_runtime_client(paths, &config).map_err(|error| {
        let detail = format!("{error:#}");
        tracing::warn!(error = %detail, "workflow runtime client setup failed");
        anyhow::anyhow!(
            crate::daemon_workflow_runtime::public_workflow_runtime_error_message(&error)
        )
    })
}

fn runtime_call<T>(result: puffer_workflow::WorkflowRuntimeResult<T>) -> Result<T> {
    result.map_err(|error| {
        let detail = error.to_string();
        tracing::warn!(error = %detail, "workflow runtime request failed");
        anyhow::anyhow!(crate::daemon_workflow_runtime::public_workflow_runtime_error(&error))
    })
}

fn workflow_execute_request(params: &Value) -> Result<WorkflowRuntimeRecord> {
    if params.get("request").is_some() {
        return record_param_or_root(params, "request", "workflow_execute request");
    }
    if params.get("execution").is_some() {
        return record_param_or_root(params, "execution", "workflow_execute request");
    }

    let mut object = serde_json::Map::new();
    if let Some(input) = params.get("input") {
        object.insert("input".to_string(), input.clone());
    }
    if let Some(trigger_node_id) = params
        .get("triggerNodeId")
        .or_else(|| params.get("trigger_node_id"))
    {
        object.insert("triggerNodeId".to_string(), trigger_node_id.clone());
    }
    WorkflowRuntimeRecord::try_from(Value::Object(object))
        .context("workflow_execute request must be a JSON object")
}

fn record_param_or_root(params: &Value, key: &str, label: &str) -> Result<WorkflowRuntimeRecord> {
    let value = params.get(key).unwrap_or(params).clone();
    WorkflowRuntimeRecord::try_from(value).with_context(|| format!("{label} must be a JSON object"))
}

fn typed_param_or_root<T>(params: &Value, key: &str, label: &str) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let value = params.get(key).unwrap_or(params).clone();
    serde_json::from_value(value)
        .with_context(|| format!("{label} must match AgentEnv workflow JSON"))
}

fn required_string(params: &Value, keys: &[&str], label: &str) -> Result<String> {
    for key in keys {
        if let Some(value) = params
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(value.to_string());
        }
    }
    anyhow::bail!("missing {label}")
}
