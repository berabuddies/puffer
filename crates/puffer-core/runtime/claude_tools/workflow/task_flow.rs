use super::store::{ensure_safe_identifier, load_store, now_ms, save_store, workflow_root};
use crate::AppState;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaskFlowAction {
    FromToolContext,
    BindSession,
    CreateManaged,
    RunTask,
    SetWaiting,
    Resume,
    Finish,
    Fail,
    RequestCancel,
    CancelFlow,
    GetTaskSummary,
    AppendBuffer,
}

#[derive(Debug, Deserialize)]
struct TaskFlowInput {
    action: TaskFlowAction,
    #[serde(default)]
    flow: Option<Value>,
    #[serde(default)]
    ctx: Option<Value>,
    #[serde(default, rename = "sessionKey", alias = "session_key")]
    session_key: Option<String>,
    #[serde(default, rename = "requesterOrigin", alias = "requester_origin")]
    requester_origin: Option<String>,
    #[serde(default, rename = "controllerId", alias = "controller_id")]
    controller_id: Option<String>,
    #[serde(default)]
    goal: Option<String>,
    #[serde(default, rename = "currentStep", alias = "current_step")]
    current_step: Option<String>,
    #[serde(default, rename = "stateJson", alias = "state_json")]
    state_json: Option<Value>,
    #[serde(default, rename = "waitJson", alias = "wait_json")]
    wait_json: Option<Value>,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(
        default,
        rename = "childSessionKey",
        alias = "child_session_key",
        alias = "child_key"
    )]
    child_session_key: Option<String>,
    #[serde(default, rename = "runId", alias = "run_id")]
    run_id: Option<String>,
    #[serde(default, rename = "taskDesc", alias = "task_desc", alias = "task")]
    task_desc: Option<String>,
    #[serde(default, rename = "flowId", alias = "flow_id")]
    flow_id: Option<String>,
    #[serde(default, rename = "expectedRevision", alias = "expected_revision")]
    expected_revision: Option<Value>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    namespace: Option<String>,
    #[serde(default)]
    item: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ManagedFlow {
    id: String,
    revision: u64,
    controller_id: String,
    goal: String,
    current_step: String,
    state: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wait: Option<Value>,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    children: Vec<LinkedChildTask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    flow_binding: Option<Value>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct LinkedChildTask {
    run_id: String,
    runtime: String,
    child_session_key: String,
    task: String,
    status: String,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct BufferEntry {
    item: Value,
    created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct TaskFlowStore {
    flows: BTreeMap<String, ManagedFlow>,
    buffers: BTreeMap<String, Vec<BufferEntry>>,
}

/// Executes the typed TaskFlow bridge used by verified Lambda skill contracts.
pub fn execute_task_flow(_state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: TaskFlowInput = serde_json::from_value(input).context("invalid TaskFlow input")?;
    match parsed.action {
        TaskFlowAction::FromToolContext => from_tool_context(parsed),
        TaskFlowAction::BindSession => bind_session(parsed),
        TaskFlowAction::CreateManaged => create_managed(cwd, parsed),
        TaskFlowAction::RunTask => run_task(cwd, parsed),
        TaskFlowAction::SetWaiting => update_flow(cwd, parsed, FlowMutation::Waiting),
        TaskFlowAction::Resume => update_flow(cwd, parsed, FlowMutation::Resume),
        TaskFlowAction::Finish => update_flow(cwd, parsed, FlowMutation::Finish),
        TaskFlowAction::Fail => update_flow(cwd, parsed, FlowMutation::Fail),
        TaskFlowAction::RequestCancel => cancel_flow(cwd, parsed, "cancel_requested"),
        TaskFlowAction::CancelFlow => cancel_flow(cwd, parsed, "cancelled"),
        TaskFlowAction::GetTaskSummary => get_task_summary(cwd, parsed),
        TaskFlowAction::AppendBuffer => append_buffer(cwd, parsed),
    }
}

enum FlowMutation {
    Waiting,
    Resume,
    Finish,
    Fail,
}

fn from_tool_context(input: TaskFlowInput) -> Result<String> {
    let ctx = input.ctx.unwrap_or(Value::Null);
    let flow = json!({
        "kind": "taskflow",
        "source": "tool_context",
        "sessionKey": find_string_field(&ctx, "sessionKey")
            .or_else(|| find_string_field(&ctx, "session_key")),
        "requesterOrigin": find_string_field(&ctx, "requesterOrigin")
            .or_else(|| find_string_field(&ctx, "requester_origin")),
    });
    json_output(&json!({ "ok": true, "flow": flow }))
}

fn bind_session(input: TaskFlowInput) -> Result<String> {
    let session_key = required_string(input.session_key, "sessionKey")?;
    let requester_origin = required_string(input.requester_origin, "requesterOrigin")?;
    let flow = json!({
        "kind": "taskflow",
        "source": "session_binding",
        "sessionKey": session_key,
        "requesterOrigin": requester_origin,
    });
    json_output(&json!({ "ok": true, "flow": flow }))
}

fn create_managed(cwd: &Path, input: TaskFlowInput) -> Result<String> {
    let controller_id = required_string(input.controller_id, "controllerId")?;
    let goal = required_string(input.goal, "goal")?;
    let current_step = required_string(input.current_step, "currentStep")?;
    let state = required_json(input.state_json, "stateJson")?;
    let path = task_flow_path(cwd)?;
    let mut store = load_store::<TaskFlowStore>(&path)?;
    let id = next_flow_id(&store);
    let timestamp = now_ms();
    let flow = ManagedFlow {
        id: id.clone(),
        revision: 1,
        controller_id,
        goal,
        current_step,
        state,
        wait: None,
        status: "running".to_string(),
        reason: None,
        children: Vec::new(),
        flow_binding: input.flow,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    store.flows.insert(id.clone(), flow.clone());
    save_store(&path, &store)?;
    json_output(&json!({
        "ok": true,
        "applied": true,
        "flowId": id,
        "revision": flow.revision,
        "flow": flow_summary(&flow),
    }))
}

fn run_task(cwd: &Path, input: TaskFlowInput) -> Result<String> {
    let path = task_flow_path(cwd)?;
    let mut store = load_store::<TaskFlowStore>(&path)?;
    let flow_id = required_flow_id(&input)?;
    let Some(flow) = store.flows.get_mut(&flow_id) else {
        bail!("flowId `{flow_id}` not found");
    };
    let runtime = required_string(input.runtime, "runtime")?;
    let child_session_key = required_string(input.child_session_key, "childSessionKey")?;
    let task = required_string(input.task_desc, "taskDesc")?;
    let run_id = input
        .run_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| next_child_run_id(flow));
    let timestamp = now_ms();
    let child = LinkedChildTask {
        run_id,
        runtime,
        child_session_key,
        task,
        status: "running".to_string(),
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
    };
    flow.children.push(child.clone());
    flow.revision += 1;
    flow.updated_at_ms = timestamp;
    let revision = flow.revision;
    save_store(&path, &store)?;
    json_output(&json!({
        "ok": true,
        "created": true,
        "flowId": flow_id,
        "revision": revision,
        "child": child,
    }))
}

fn update_flow(cwd: &Path, input: TaskFlowInput, mutation: FlowMutation) -> Result<String> {
    let path = task_flow_path(cwd)?;
    let mut store = load_store::<TaskFlowStore>(&path)?;
    let flow_id = required_flow_id(&input)?;
    let expected_revision = required_revision(&input)?;
    let Some(flow) = store.flows.get_mut(&flow_id) else {
        bail!("flowId `{flow_id}` not found");
    };
    if flow.revision != expected_revision {
        return json_output(&json!({
            "ok": true,
            "applied": false,
            "code": "revision_mismatch",
            "flowId": flow_id,
            "expectedRevision": expected_revision,
            "currentRevision": flow.revision,
            "status": flow.status,
        }));
    }
    apply_mutation(flow, input, mutation)?;
    let revision = flow.revision;
    let status = flow.status.clone();
    save_store(&path, &store)?;
    json_output(&json!({
        "ok": true,
        "applied": true,
        "flowId": flow_id,
        "revision": revision,
        "status": status,
    }))
}

fn apply_mutation(
    flow: &mut ManagedFlow,
    input: TaskFlowInput,
    mutation: FlowMutation,
) -> Result<()> {
    match mutation {
        FlowMutation::Waiting => {
            flow.current_step = required_string(input.current_step, "currentStep")?;
            flow.state = required_json(input.state_json, "stateJson")?;
            flow.wait = Some(required_json(input.wait_json, "waitJson")?);
            flow.status = "waiting".to_string();
            flow.reason = None;
        }
        FlowMutation::Resume => {
            flow.current_step = required_string(input.current_step, "currentStep")?;
            flow.state = required_json(input.state_json, "stateJson")?;
            flow.wait = None;
            flow.status = input
                .status
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "running".to_string());
            flow.reason = None;
        }
        FlowMutation::Finish => {
            flow.state = required_json(input.state_json, "stateJson")?;
            flow.wait = None;
            flow.status = "finished".to_string();
            flow.reason = None;
        }
        FlowMutation::Fail => {
            flow.wait = None;
            flow.status = "failed".to_string();
            flow.reason = Some(required_string(input.reason, "reason")?);
        }
    }
    flow.revision += 1;
    flow.updated_at_ms = now_ms();
    Ok(())
}

fn cancel_flow(cwd: &Path, input: TaskFlowInput, status: &str) -> Result<String> {
    let path = task_flow_path(cwd)?;
    let mut store = load_store::<TaskFlowStore>(&path)?;
    let flow_id = required_flow_id(&input)?;
    let Some(flow) = store.flows.get_mut(&flow_id) else {
        bail!("flowId `{flow_id}` not found");
    };
    flow.status = status.to_string();
    flow.reason = input.reason.filter(|value| !value.trim().is_empty());
    flow.revision += 1;
    flow.updated_at_ms = now_ms();
    let revision = flow.revision;
    let status = flow.status.clone();
    save_store(&path, &store)?;
    json_output(&json!({
        "ok": true,
        "applied": true,
        "flowId": flow_id,
        "revision": revision,
        "status": status,
    }))
}

fn get_task_summary(cwd: &Path, input: TaskFlowInput) -> Result<String> {
    let store = load_store::<TaskFlowStore>(&task_flow_path(cwd)?)?;
    let flow_id = required_flow_id(&input)?;
    let Some(flow) = store.flows.get(&flow_id) else {
        bail!("flowId `{flow_id}` not found");
    };
    json_output(&json!({
        "ok": true,
        "flowId": flow_id,
        "summary": flow_summary(flow),
    }))
}

fn append_buffer(cwd: &Path, input: TaskFlowInput) -> Result<String> {
    let namespace = required_string(input.namespace, "namespace")?;
    ensure_safe_identifier(&namespace, "namespace")?;
    let item = input
        .item
        .ok_or_else(|| anyhow::anyhow!("item is required"))?;
    let path = task_flow_path(cwd)?;
    let mut store = load_store::<TaskFlowStore>(&path)?;
    let entries = store.buffers.entry(namespace.clone()).or_default();
    entries.push(BufferEntry {
        item,
        created_at_ms: now_ms(),
    });
    let count = entries.len();
    save_store(&path, &store)?;
    json_output(&json!({
        "ok": true,
        "applied": true,
        "namespace": namespace,
        "count": count,
    }))
}

fn required_flow_id(input: &TaskFlowInput) -> Result<String> {
    let flow_id = required_string(input.flow_id.clone(), "flowId")?;
    ensure_safe_identifier(&flow_id, "flowId")?;
    Ok(flow_id)
}

fn required_revision(input: &TaskFlowInput) -> Result<u64> {
    let Some(value) = input.expected_revision.as_ref() else {
        bail!("expectedRevision is required");
    };
    match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("expectedRevision must be a non-negative integer")),
        Value::String(text) => text
            .trim()
            .parse::<u64>()
            .context("expectedRevision must be a non-negative integer"),
        _ => bail!("expectedRevision must be a non-negative integer"),
    }
}

fn required_string(value: Option<String>, field: &str) -> Result<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{field} is required"))
}

fn required_json(value: Option<Value>, field: &str) -> Result<Value> {
    let Some(value) = value else {
        bail!("{field} is required");
    };
    match value {
        Value::String(text) => {
            serde_json::from_str(&text).with_context(|| format!("{field} must be valid JSON"))
        }
        other => Ok(other),
    }
}

fn find_string_field(value: &Value, field: &str) -> Option<String> {
    value
        .as_object()
        .and_then(|object| object.get(field))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn flow_summary(flow: &ManagedFlow) -> Value {
    json!({
        "flowId": flow.id,
        "revision": flow.revision,
        "controllerId": flow.controller_id,
        "goal": flow.goal,
        "currentStep": flow.current_step,
        "status": flow.status,
        "state": flow.state,
        "wait": flow.wait,
        "reason": flow.reason,
        "children": flow.children,
        "updatedAtMs": flow.updated_at_ms,
    })
}

fn task_flow_path(cwd: &Path) -> Result<PathBuf> {
    Ok(workflow_root(cwd)?.join("task_flows.json"))
}

fn next_flow_id(store: &TaskFlowStore) -> String {
    loop {
        let id = format!("flow-{}", Uuid::new_v4().simple());
        if !store.flows.contains_key(&id) {
            return id;
        }
    }
}

fn next_child_run_id(flow: &ManagedFlow) -> String {
    let mut index = flow.children.len() + 1;
    loop {
        let candidate = format!("{}-child-{index}", flow.id);
        if !flow.children.iter().any(|child| child.run_id == candidate) {
            return candidate;
        }
        index += 1;
    }
}

fn json_output(value: &Value) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}
