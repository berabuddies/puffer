//! Internal Automation compile/deploy/run helpers.

use anyhow::{bail, Context, Result};
use puffer_automation::{
    compile_automation, AutomationAgentMode, AutomationAgentToolSpec, AutomationFlowSpec,
    AutomationLoopInput, AutomationLoopSpec, AutomationRecord, AutomationRunLocation,
    AutomationRuntimeState, AutomationRuntimeStatus, AutomationStatus, AutomationStepSpec,
    AutomationStore, CompiledAgentEnvWorkflow, CompiledPufferBinding, CompiledWorkflowDefinition,
    CompiledWorkflowRole,
};
use puffer_config::{load_config, ConfigPaths, PufferConfig, WorkflowBackendMode};
use puffer_provider_registry::{canonical_provider_id, AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, BACKGROUND_SESSION_TAG};
use puffer_subscriptions::{
    installed_connector_action_executor, ActionSpec, ConnectorActionExecutor, WorkflowActionOutput,
    WorkflowBindingSpec, WorkflowBindingStatus,
};
use puffer_workflow::{
    AgentEnvWorkflowDefinition, AgentEnvWorkflowEdge, AgentEnvWorkflowNode, WorkflowRuntimeClient,
    WorkflowRuntimeCreateWorkflowRequest, WorkflowRuntimeError, WorkflowRuntimeErrorKind,
    WorkflowRuntimeInMemoryExecuteRequest, WorkflowRuntimeRecord,
    WorkflowRuntimeUpdateWorkflowRequest, WorkflowRuntimeWorkflow,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::automation_runtime_errors::{
    public_automation_runtime_detail_message, AutomationRuntimeErrorContext,
};
use crate::daemon::DaemonState;
use crate::daemon_workflows::{
    create_automation_connector_action_draft, AutomationConnectorActionDraftParams,
    CreatedAutomationConnectorActionDraft,
};
use crate::workflow_runtime_helpers::workflow_execute_summary;

const WORKFLOW_ID_KEYS: &[&str] = &[
    "id",
    "workflowId",
    "workflow_id",
    "workflowSlug",
    "workflow_slug",
];
const AUTOMATION_ID_KEYS: &[&str] = &["id", "automation_id", "automationId"];

#[derive(Debug)]
struct AutomationRunOutput {
    compiled: bool,
    record: AutomationRecord,
    result: Value,
    summary: String,
    status: String,
    approval: Option<AutomationRunApprovalRecord>,
}

#[derive(Debug)]
struct AutomationExecutionOutput {
    result: Value,
    summary: String,
    status: String,
    approval: Option<AutomationRunApprovalRecord>,
}

#[derive(Debug, Clone, Copy)]
struct AutomationConnectorActionRunContext<'a> {
    paths: &'a ConfigPaths,
    run_id: &'a str,
}

#[derive(Debug, Clone, Copy)]
enum ConnectorActionPosition {
    TopLevelTerminal,
    /// A top-level gated action with steps after it. `resume_step_index` is the
    /// index of the first step to run once the drafted action is approved.
    TopLevelNonTerminal {
        resume_step_index: usize,
    },
    LoopBody,
}

/// Instructs the flow runner to resume a suspended run instead of starting from
/// the root output: it first runs the gated step's continuation with the
/// approved connector result, then continues from `resume_step_index`.
#[derive(Debug, Clone)]
struct FlowResume {
    step_id: String,
    resume_step_index: usize,
    seed_output: Value,
}

pub(crate) struct AutomationProviderContext<'a> {
    pub(crate) providers: &'a ProviderRegistry,
    pub(crate) auth_store: &'a AuthStore,
    pub(crate) resources: Option<&'a LoadedResources>,
}

pub(crate) fn handle_automation_compile_deploy(
    state: &DaemonState,
    params: &Value,
) -> Result<Value> {
    let automation_id = required_automation_id(params)?;
    let expected_revision = optional_expected_revision(params)?;
    user_facing_automation_result(compile_and_deploy_automation(
        state,
        &automation_id,
        expected_revision,
    ))
}

pub(crate) fn handle_automation_sync_preview(state: &DaemonState, params: &Value) -> Result<Value> {
    let automation_id = required_automation_id(params)?;
    let expected_revision = optional_expected_revision(params)?;
    user_facing_automation_result(sync_preview_automation(
        state,
        &automation_id,
        expected_revision,
    ))
}

pub(crate) fn handle_automation_run_preview(state: &DaemonState, params: &Value) -> Result<Value> {
    let automation_id = required_automation_id(params)?;
    let input = params.get("input").cloned().unwrap_or_else(|| json!({}));
    run_automation(state, &automation_id, input)
}

pub(crate) fn handle_automation_run_history(state: &DaemonState, params: &Value) -> Result<Value> {
    let automation_id = required_automation_id(params)?;
    let runs = load_run_history(&automation_run_history_path(state.config_paths()))?
        .runs
        .into_iter()
        .filter(|run| run.automation_id == automation_id)
        .collect::<Vec<_>>();
    Ok(json!({ "automation_id": automation_id, "runs": runs }))
}

fn compile_and_deploy_automation(
    state: &DaemonState,
    automation_id: &str,
    expected_revision: Option<u64>,
) -> Result<Value> {
    let inputs = state.build_runtime_inputs_without_discovery()?;
    let provider_context = AutomationProviderContext {
        providers: &inputs.providers,
        auth_store: &inputs.auth_store,
        resources: Some(&inputs.resources),
    };
    let record = compile_and_deploy_with_context(
        state.config_paths(),
        state.automation_store(),
        automation_id,
        expected_revision,
        Some(&provider_context),
    )?;
    Ok(json!({
        "id": record.id,
        "status": record.status,
        "revision": record.revision,
        "runtime": runtime_summary(&record.runtime),
    }))
}

fn sync_preview_automation(
    state: &DaemonState,
    automation_id: &str,
    expected_revision: Option<u64>,
) -> Result<Value> {
    let inputs = state.build_runtime_inputs_without_discovery()?;
    let provider_context = AutomationProviderContext {
        providers: &inputs.providers,
        auth_store: &inputs.auth_store,
        resources: Some(&inputs.resources),
    };
    let record = sync_preview_with_context(
        state.config_paths(),
        state.automation_store(),
        automation_id,
        expected_revision,
        Some(&provider_context),
    )?;
    Ok(json!({
        "id": record.id,
        "status": record.status,
        "revision": record.revision,
        "runtime": runtime_summary(&record.runtime),
    }))
}

fn run_automation(state: &DaemonState, automation_id: &str, input: Value) -> Result<Value> {
    let started_at_ms = puffer_subscriptions::now_ms();
    let run_id = format!("preview-{automation_id}-{started_at_ms}");
    let result = (|| {
        let inputs = state.build_runtime_inputs_without_discovery()?;
        let provider_context = AutomationProviderContext {
            providers: &inputs.providers,
            auth_store: &inputs.auth_store,
            resources: Some(&inputs.resources),
        };
        run_automation_preview_with_context(
            state.config_paths(),
            state.automation_store(),
            automation_id,
            input,
            &run_id,
            Some(&provider_context),
        )
    })();
    let ended_at_ms = puffer_subscriptions::now_ms();
    match result {
        Ok(output) => {
            let response = automation_preview_response(&output);
            append_run_history(
                &automation_run_history_path(state.config_paths()),
                AutomationRunHistoryRecord {
                    id: run_id,
                    automation_id: automation_id.to_string(),
                    title: "Test run".to_string(),
                    status: output.status.clone(),
                    started_at_ms,
                    duration_ms: (ended_at_ms - started_at_ms).max(0),
                    summary: output.summary.clone(),
                    source_event: Some("preview_run".to_string()),
                    compiled: output.compiled,
                    runtime_status: output.record.runtime.status,
                    result: Some(output.result.clone()),
                    error: None,
                    approval: output.approval.clone().or_else(|| {
                        Some(AutomationRunApprovalRecord {
                            required: output.record.spec.review.human_approval_required,
                            status: "not_required_for_preview".to_string(),
                        })
                    }),
                },
            )?;
            Ok(response)
        }
        Err(error) => {
            let detail = format!("{error:#}");
            tracing::warn!(error = %detail, automation_id, "automation preview failed");
            let message = public_automation_error_message(&error);
            let runtime_status = state
                .automation_store()
                .get(automation_id)
                .ok()
                .map(|record| record.runtime.status)
                .unwrap_or_default();
            append_run_history(
                &automation_run_history_path(state.config_paths()),
                AutomationRunHistoryRecord {
                    id: format!("preview-{automation_id}-{started_at_ms}"),
                    automation_id: automation_id.to_string(),
                    title: "Test run".to_string(),
                    status: "error".to_string(),
                    started_at_ms,
                    duration_ms: (ended_at_ms - started_at_ms).max(0),
                    summary: message.clone(),
                    source_event: Some("preview_run".to_string()),
                    compiled: false,
                    runtime_status,
                    result: None,
                    error: Some(message.clone()),
                    approval: Some(AutomationRunApprovalRecord {
                        required: true,
                        status: "not_created".to_string(),
                    }),
                },
            )?;
            Err(anyhow::anyhow!(message))
        }
    }
}

fn user_facing_automation_result<T>(result: Result<T>) -> Result<T> {
    result.map_err(|error| {
        let detail = format!("{error:#}");
        tracing::warn!(error = %detail, "automation runtime operation failed");
        anyhow::anyhow!(public_automation_error_message(&error))
    })
}

pub(crate) fn public_automation_error_message(error: &anyhow::Error) -> String {
    let detail = format!("{error:#}");
    public_automation_error_detail_message(&detail)
}

pub(crate) fn public_automation_error_detail_message(detail: &str) -> String {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("puffer_agent") && lower.contains("provider/model context") {
        return "Automation cannot run the Puffer agent because provider/model context is unavailable. Reload the daemon runtime inputs, then try again."
            .to_string();
    }
    if lower.contains("puffer_agent") && lower.contains("loaded puffer resources") {
        return "Automation cannot run the Puffer agent because Puffer resources are not loaded. Reload the daemon runtime inputs, then try again."
            .to_string();
    }
    if lower.contains("connect") && lower.contains("provider") && lower.contains("automation") {
        return "Connect credentials for the selected provider before activating this automation."
            .to_string();
    }
    if lower.contains("complete the required configuration")
        || lower.contains("validation_failed")
        || lower.contains("validation failed")
    {
        return "Automation runtime rejected this automation because one trigger or action is missing required configuration. Check the automation's trigger and tool settings, then try again."
            .to_string();
    }
    public_automation_runtime_detail_message(detail, AutomationRuntimeErrorContext::Automation)
}

fn automation_preview_response(output: &AutomationRunOutput) -> Value {
    json!({
        "id": output.record.id,
        "status": output.status,
        "summary": output.summary,
        "result": output.result,
        "compiled": output.compiled,
        "runtime": runtime_summary(&output.record.runtime),
    })
}

fn automation_run_history_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("automation_runs.json")
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AutomationRunHistoryFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    runs: Vec<AutomationRunHistoryRecord>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AutomationRunHistoryRecord {
    id: String,
    automation_id: String,
    title: String,
    status: String,
    started_at_ms: i128,
    duration_ms: i128,
    summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_event: Option<String>,
    compiled: bool,
    runtime_status: AutomationRuntimeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval: Option<AutomationRunApprovalRecord>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AutomationRunApprovalRecord {
    required: bool,
    status: String,
}

fn load_run_history(path: &Path) -> Result<AutomationRunHistoryFile> {
    if !path.exists() {
        return Ok(AutomationRunHistoryFile::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read automation run history `{}`", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(AutomationRunHistoryFile::default());
    }
    serde_json::from_str(&raw)
        .with_context(|| format!("parse automation run history `{}`", path.display()))
}

fn append_run_history(path: &Path, record: AutomationRunHistoryRecord) -> Result<()> {
    let mut history = load_run_history(path)?;
    history.version = 1;
    history.runs.insert(0, record);
    if history.runs.len() > 500 {
        history.runs.truncate(500);
    }
    write_run_history(path, &history)
}

fn write_run_history(path: &Path, history: &AutomationRunHistoryFile) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create automation run history dir `{}`", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let body = serde_json::to_vec_pretty(history).context("serialize automation run history")?;
    std::fs::write(&tmp, body)
        .with_context(|| format!("write automation run history temp `{}`", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("replace automation run history `{}`", path.display()))?;
    Ok(())
}

fn automation_suspensions_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("automation_suspensions.json")
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AutomationSuspensionFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    suspensions: Vec<AutomationRunSuspension>,
}

/// Persisted continuation context for a run that suspended at a human-gated
/// connector action. On approval the daemon loads this, runs the suspended
/// step's continuation with the connector result, then the remaining top-level
/// steps. See `puffer-automation` 02.md (Execution Model: Suspend At The Gated
/// Step). Loop-body and agent-step-tool gated actions do not suspend; they are
/// rejected at save/compile time instead.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct AutomationRunSuspension {
    automation_id: String,
    run_id: String,
    step_id: String,
    draft_id: String,
    connector_slug: String,
    connection_slug: String,
    action: String,
    /// Index into `spec.flow.steps` of the first step to run on resume (the
    /// step immediately after the gated connector action).
    resume_step_index: usize,
    trigger_input: Value,
    root_output: Value,
    previous_output: Value,
}

fn load_suspensions(path: &Path) -> Result<AutomationSuspensionFile> {
    if !path.exists() {
        return Ok(AutomationSuspensionFile::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read automation suspensions `{}`", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(AutomationSuspensionFile::default());
    }
    serde_json::from_str(&raw)
        .with_context(|| format!("parse automation suspensions `{}`", path.display()))
}

fn write_suspensions(path: &Path, file: &AutomationSuspensionFile) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create automation suspensions dir `{}`", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let body = serde_json::to_vec_pretty(file).context("serialize automation suspensions")?;
    std::fs::write(&tmp, body)
        .with_context(|| format!("write automation suspensions temp `{}`", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("replace automation suspensions `{}`", path.display()))?;
    Ok(())
}

/// Persists (or replaces) the suspension for a run/step. Keyed by run_id +
/// step_id so a re-suspend at the same step overwrites cleanly.
fn upsert_suspension(paths: &ConfigPaths, suspension: AutomationRunSuspension) -> Result<()> {
    let path = automation_suspensions_path(paths);
    let mut file = load_suspensions(&path)?;
    file.version = 1;
    file.suspensions.retain(|existing| {
        !(existing.run_id == suspension.run_id && existing.step_id == suspension.step_id)
    });
    file.suspensions.push(suspension);
    write_suspensions(&path, &file)
}

fn find_suspension_for_draft(
    paths: &ConfigPaths,
    draft_id: &str,
) -> Result<Option<AutomationRunSuspension>> {
    let path = automation_suspensions_path(paths);
    let file = load_suspensions(&path)?;
    Ok(file
        .suspensions
        .into_iter()
        .find(|suspension| suspension.draft_id == draft_id))
}

/// Removes any suspensions for a draft (called after a resume completes) or for
/// a whole run (called on reject). Idempotent.
fn remove_suspensions(
    paths: &ConfigPaths,
    draft_id: Option<&str>,
    run_id: Option<&str>,
) -> Result<()> {
    let path = automation_suspensions_path(paths);
    if !path.exists() {
        return Ok(());
    }
    let mut file = load_suspensions(&path)?;
    let before = file.suspensions.len();
    file.suspensions.retain(|suspension| {
        let draft_match = draft_id.is_some_and(|id| suspension.draft_id == id);
        let run_match = run_id.is_some_and(|id| suspension.run_id == id);
        !(draft_match || run_match)
    });
    if file.suspensions.len() != before {
        file.version = 1;
        write_suspensions(&path, &file)?;
    }
    Ok(())
}

pub(crate) fn mark_automation_run_rejected(
    paths: &ConfigPaths,
    automation_id: &str,
    run_id: &str,
    reason: &str,
) -> Result<()> {
    let path = automation_run_history_path(paths);
    let mut history = load_run_history(&path)?;
    let run = history
        .runs
        .iter_mut()
        .find(|run| run.automation_id == automation_id && run.id == run_id)
        .with_context(|| {
            format!("automation run `{run_id}` for automation `{automation_id}` not found")
        })?;
    let summary = format!(
        "Rejected by human reviewer: {}",
        compact_rejection_reason(reason)
    );
    run.status = "rejected".to_string();
    run.summary = summary.clone();
    run.error = Some(summary);
    match &mut run.approval {
        Some(approval) => {
            approval.required = true;
            approval.status = "rejected".to_string();
        }
        None => {
            run.approval = Some(AutomationRunApprovalRecord {
                required: true,
                status: "rejected".to_string(),
            });
        }
    }
    write_run_history(&path, &history)?;
    // A rejected run will never resume; drop its continuation context.
    remove_suspensions(paths, None, Some(run_id))
}

pub(crate) fn mark_automation_run_approved(
    paths: &ConfigPaths,
    automation_id: &str,
    run_id: &str,
    receipt: Value,
) -> Result<()> {
    let path = automation_run_history_path(paths);
    let mut history = load_run_history(&path)?;
    let run = history
        .runs
        .iter_mut()
        .find(|run| run.automation_id == automation_id && run.id == run_id)
        .with_context(|| {
            format!("automation run `{run_id}` for automation `{automation_id}` not found")
        })?;
    run.status = "completed".to_string();
    run.summary = "Approved by human reviewer and sent.".to_string();
    run.error = None;
    match &mut run.result {
        Some(Value::Object(result)) => {
            result.insert("receipt".to_string(), receipt);
        }
        Some(result) => {
            *result = json!({ "receipt": receipt });
        }
        None => {
            run.result = Some(json!({ "receipt": receipt }));
        }
    }
    match &mut run.approval {
        Some(approval) => {
            approval.required = true;
            approval.status = "approved".to_string();
        }
        None => {
            run.approval = Some(AutomationRunApprovalRecord {
                required: true,
                status: "approved".to_string(),
            });
        }
    }
    write_run_history(&path, &history)
}

fn compact_rejection_reason(reason: &str) -> String {
    let collapsed = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    let mut truncated = false;
    for (index, ch) in collapsed.chars().enumerate() {
        if index >= 160 {
            truncated = true;
            break;
        }
        out.push(ch);
    }
    if truncated {
        out.push_str("...");
    }
    out
}

pub(crate) fn run_automation_with_context(
    paths: &ConfigPaths,
    store: &AutomationStore,
    automation_id: &str,
    input: Value,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<WorkflowActionOutput> {
    let started_at_ms = puffer_subscriptions::now_ms();
    let run_id = format!("run-{automation_id}-{started_at_ms}");
    let mut record = store.get(automation_id)?;
    ensure_live_automation_can_run(&record)?;
    if runtime_needs_deploy(&record)? {
        record = compile_and_deploy_with_context(
            paths,
            store,
            automation_id,
            Some(record.revision),
            provider_context,
        )?;
    }
    ensure_live_automation_can_run(&record)?;
    let execution =
        execute_automation_with_run_id(paths, &record, input, &run_id, provider_context)?;
    if execution.status == "awaiting_approval" {
        let ended_at_ms = puffer_subscriptions::now_ms();
        append_run_history(
            &automation_run_history_path(paths),
            AutomationRunHistoryRecord {
                id: run_id,
                automation_id: automation_id.to_string(),
                title: "Live run".to_string(),
                status: execution.status.clone(),
                started_at_ms,
                duration_ms: (ended_at_ms - started_at_ms).max(0),
                summary: execution.summary.clone(),
                source_event: Some("connector_event".to_string()),
                compiled: !record.runtime.agentenv_workflows.is_empty(),
                runtime_status: record.runtime.status,
                result: Some(execution.result.clone()),
                error: None,
                approval: execution.approval.clone(),
            },
        )?;
    }
    let output = AutomationRunOutput {
        compiled: !record.runtime.agentenv_workflows.is_empty(),
        record,
        result: execution.result,
        summary: execution.summary,
        status: execution.status,
        approval: execution.approval,
    };
    Ok(WorkflowActionOutput::new(output.summary))
}

/// Resumes a run that suspended at a gated connector action, after the drafted
/// action has been executed for real. Runs the suspended step's continuation
/// with `receipt` as the connector result, then the remaining top-level steps,
/// and updates run history. Returns `false` when the draft has no suspension
/// (a terminal-position draft), so the caller marks the run completed as before.
pub(crate) fn resume_automation_run(
    state: &DaemonState,
    draft_id: &str,
    receipt: Value,
) -> Result<bool> {
    let paths = state.config_paths();
    let Some(suspension) = find_suspension_for_draft(paths, draft_id)? else {
        return Ok(false);
    };
    let inputs = state.build_runtime_inputs_without_discovery()?;
    let provider_context = AutomationProviderContext {
        providers: &inputs.providers,
        auth_store: &inputs.auth_store,
        resources: Some(&inputs.resources),
    };
    let record = state.automation_store().get(&suspension.automation_id)?;
    let execution = execute_resumed_automation(
        paths,
        &record,
        &suspension,
        receipt,
        Some(&provider_context),
    )?;
    update_automation_run_after_resume(paths, &suspension, &execution)?;
    // Resume finished (completed, or re-suspended at a later gated step which
    // wrote its own suspension): drop this step's suspension.
    remove_suspensions(paths, Some(draft_id), None)?;
    Ok(true)
}

/// The connector-result value the ungated path would have produced, rebuilt
/// from the persisted suspension plus the real send receipt, so the resumed
/// continuation sees a consistent `previous_output`.
fn resumed_seed_output(suspension: &AutomationRunSuspension, receipt: Value) -> Value {
    json!({
        "kind": "connector_action_result",
        "step_id": suspension.step_id,
        "connector_slug": suspension.connector_slug,
        "connection_slug": suspension.connection_slug,
        "action": suspension.action,
        "summary": receipt,
        "input": Value::Null,
        "previous_output": suspension.previous_output,
    })
}

/// Runs the remaining flow for a resumed run. Uses the in-memory execution path
/// (fresh-compiled definitions), mirroring how previews execute.
fn execute_resumed_automation(
    paths: &ConfigPaths,
    record: &AutomationRecord,
    suspension: &AutomationRunSuspension,
    receipt: Value,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<AutomationExecutionOutput> {
    let config = load_config(paths).context("load workflow backend config")?;
    let client = crate::daemon_workflow_runtime::workflow_runtime_client_for_mode(
        paths,
        &config,
        workflow_backend_mode_for_run_location(record.spec.run_location),
    )
    .context("create workflow runtime client")?;
    let mut plan = compile_automation(record).context("compile automation for resume")?;
    plan.spec_hash = puffer_automation::automation_spec_hash(&record.spec)
        .map_err(|error| anyhow::anyhow!("hash automation spec: {error}"))?;
    let resume = FlowResume {
        step_id: suspension.step_id.clone(),
        resume_step_index: suspension.resume_step_index,
        seed_output: resumed_seed_output(suspension, receipt),
    };
    let final_output = execute_ordered_flow_after_root_in_memory(
        &client,
        &plan.workflows,
        record,
        &suspension.trigger_input,
        &suspension.root_output,
        AutomationConnectorActionRunContext {
            paths,
            run_id: &suspension.run_id,
        },
        installed_connector_action_executor().as_deref(),
        provider_context,
        Some(&resume),
    )?;
    Ok(automation_execution_output_from_final(record, final_output))
}

fn update_automation_run_after_resume(
    paths: &ConfigPaths,
    suspension: &AutomationRunSuspension,
    execution: &AutomationExecutionOutput,
) -> Result<()> {
    let path = automation_run_history_path(paths);
    let mut history = load_run_history(&path)?;
    let run = history
        .runs
        .iter_mut()
        .find(|run| run.automation_id == suspension.automation_id && run.id == suspension.run_id)
        .with_context(|| {
            format!(
                "automation run `{}` for automation `{}` not found",
                suspension.run_id, suspension.automation_id
            )
        })?;
    run.status = execution.status.clone();
    run.summary = execution.summary.clone();
    run.result = Some(execution.result.clone());
    run.error = None;
    // A completed resume means a human approved and the flow finished; a
    // re-suspend keeps the awaiting_approval draft state from the flow output.
    run.approval = execution
        .approval
        .clone()
        .or(Some(AutomationRunApprovalRecord {
            required: true,
            status: "approved".to_string(),
        }));
    write_run_history(&path, &history)
}

#[cfg(test)]
fn run_automation_preview_with_store(
    paths: &ConfigPaths,
    store: &AutomationStore,
    automation_id: &str,
    input: Value,
) -> Result<AutomationRunOutput> {
    let run_id = format!("preview-{automation_id}-{}", puffer_subscriptions::now_ms());
    run_automation_preview_with_context(paths, store, automation_id, input, &run_id, None)
}

fn run_automation_preview_with_context(
    paths: &ConfigPaths,
    store: &AutomationStore,
    automation_id: &str,
    input: Value,
    run_id: &str,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<AutomationRunOutput> {
    let record = store.get(automation_id)?;
    ensure_preview_automation_can_run(&record)?;
    let execution = execute_automation_preview(paths, &record, input, run_id, provider_context)?;
    Ok(AutomationRunOutput {
        compiled: !record.runtime.agentenv_workflows.is_empty(),
        record,
        result: execution.result,
        summary: execution.summary,
        status: execution.status,
        approval: execution.approval,
    })
}

fn execute_automation_preview(
    paths: &ConfigPaths,
    record: &AutomationRecord,
    input: Value,
    run_id: &str,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<AutomationExecutionOutput> {
    let config = load_config(paths).context("load workflow backend config")?;
    let client = crate::daemon_workflow_runtime::workflow_runtime_client_for_mode(
        paths,
        &config,
        workflow_backend_mode_for_run_location(record.spec.run_location),
    )
    .context("create workflow runtime client")?;
    let mut plan = compile_automation(record).context("compile automation preview")?;
    plan.spec_hash = puffer_automation::automation_spec_hash(&record.spec)
        .map_err(|error| anyhow::anyhow!("hash automation spec: {error}"))?;
    let root = compiled_workflow_for_role(&plan.workflows, &CompiledWorkflowRole::Root)
        .context("compiled Automation has no root workflow definition")?;
    let root_definition = compiled_agentenv_definition(root)?;
    let root_trigger_id = root_trigger_node_id(record);
    let root_output = match execute_in_memory_value(
        &client,
        root_definition,
        json!({ "trigger": input }),
        root_trigger_id.as_deref(),
    ) {
        Ok(value) => value,
        Err(error) if preview_can_fallback_to_deployed_execution(&error, record) => {
            return execute_automation_with_run_id(paths, record, input, run_id, provider_context)
                .context("fall back to deployed workflow preview execution");
        }
        Err(error) => return Err(error),
    };

    let final_output = execute_ordered_flow_after_root_in_memory(
        &client,
        &plan.workflows,
        record,
        &input,
        &root_output,
        AutomationConnectorActionRunContext { paths, run_id },
        installed_connector_action_executor().as_deref(),
        provider_context,
        None,
    )?;
    Ok(automation_execution_output_from_final(record, final_output))
}

fn preview_can_fallback_to_deployed_execution(
    error: &anyhow::Error,
    record: &AutomationRecord,
) -> bool {
    if record.runtime.status != AutomationRuntimeStatus::Deployed {
        return false;
    }
    error.chain().any(|cause| {
        cause
            .downcast_ref::<WorkflowRuntimeError>()
            .is_some_and(|runtime_error| {
                matches!(
                    runtime_error.kind,
                    WorkflowRuntimeErrorKind::RuntimeUnreachable
                        | WorkflowRuntimeErrorKind::WorkspaceInaccessible
                        | WorkflowRuntimeErrorKind::IncompatibleRuntime
                )
            })
    })
}

#[cfg(test)]
fn compile_and_deploy_with_store(
    paths: &ConfigPaths,
    store: &AutomationStore,
    automation_id: &str,
    expected_revision: Option<u64>,
) -> Result<AutomationRecord> {
    compile_and_deploy_with_context(paths, store, automation_id, expected_revision, None)
}

fn compile_and_deploy_with_context(
    paths: &ConfigPaths,
    store: &AutomationStore,
    automation_id: &str,
    expected_revision: Option<u64>,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<AutomationRecord> {
    let record = compile_with_context(
        paths,
        store,
        automation_id,
        expected_revision,
        true,
        provider_context,
    )?;
    if record.status == AutomationStatus::Enabled {
        return Ok(record);
    }
    set_generated_binding_status(&record, WorkflowBindingStatus::Enabled, true)?;
    store
        .set_status(&record.id, AutomationStatus::Enabled)
        .context("mark automation enabled after deploy")
}

#[cfg(test)]
fn sync_preview_with_store(
    paths: &ConfigPaths,
    store: &AutomationStore,
    automation_id: &str,
    expected_revision: Option<u64>,
) -> Result<AutomationRecord> {
    sync_preview_with_context(paths, store, automation_id, expected_revision, None)
}

fn sync_preview_with_context(
    paths: &ConfigPaths,
    store: &AutomationStore,
    automation_id: &str,
    expected_revision: Option<u64>,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<AutomationRecord> {
    let record = store.get(automation_id)?;
    if let Some(expected_revision) = expected_revision {
        if record.revision != expected_revision {
            bail!(
                "automation `{automation_id}` revision conflict: expected {expected_revision}, found {}",
                record.revision
            );
        }
    }
    if record.runtime.status == AutomationRuntimeStatus::Deployed && !runtime_needs_deploy(&record)?
    {
        return Ok(record);
    }
    compile_with_context(
        paths,
        store,
        automation_id,
        expected_revision,
        false,
        provider_context,
    )
}

fn compile_with_context(
    paths: &ConfigPaths,
    store: &AutomationStore,
    automation_id: &str,
    expected_revision: Option<u64>,
    deploy_live: bool,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<AutomationRecord> {
    let record = store.get(automation_id)?;
    if let Some(expected_revision) = expected_revision {
        if record.revision != expected_revision {
            bail!(
                "automation `{automation_id}` revision conflict: expected {expected_revision}, found {}",
                record.revision
            );
        }
    }
    match compile_inner(paths, store, &record, deploy_live, provider_context) {
        Ok(record) => Ok(record),
        Err(error) => {
            let message = format!("{error:#}");
            if let Err(store_error) =
                store.replace_runtime_error(&record.id, record.revision, message)
            {
                return Err(error).with_context(|| {
                    format!("failed to record Automation runtime error: {store_error:#}")
                });
            }
            Err(error)
        }
    }
}

fn compile_inner(
    paths: &ConfigPaths,
    store: &AutomationStore,
    record: &AutomationRecord,
    deploy_live: bool,
    _provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<AutomationRecord> {
    // Reject unsupported human-gated action positions at compile/deploy time so
    // authors are warned when they save, not when a run later fails.
    validate_automation_gated_connector_actions(record)?;
    let config = load_config(paths).context("load workflow backend config")?;
    let mut plan = compile_automation(record).context("compile automation")?;
    plan.spec_hash = puffer_automation::automation_spec_hash(&record.spec)
        .map_err(|error| anyhow::anyhow!("hash automation spec: {error}"))?;
    let client = workflow_runtime_client_for_record_with_config(paths, &config, record)?;

    let mut compiled_workflows = Vec::new();
    for workflow in &plan.workflows {
        let workflow_id = deploy_workflow_definition(
            &client,
            &record,
            workflow,
            existing_workflow_id(&record, &workflow.role),
            deploy_live,
        )?;
        compiled_workflows.push(CompiledAgentEnvWorkflow {
            role: workflow.role.clone(),
            workflow_id: Some(workflow_id),
            definition_hash: Some(workflow.definition_hash.clone()),
            deployed: deploy_live,
        });
    }

    if deploy_live {
        deploy_puffer_bindings(&record)?;
    }

    let runtime = AutomationRuntimeState {
        spec_hash: Some(plan.spec_hash),
        compiled_revision: Some(record.revision),
        status: if deploy_live {
            AutomationRuntimeStatus::Deployed
        } else {
            AutomationRuntimeStatus::DraftSynced
        },
        agentenv_workflows: compiled_workflows,
        puffer_bindings: if deploy_live {
            plan.puffer_bindings
                .into_iter()
                .map(|binding| CompiledPufferBinding {
                    trigger_id: binding.trigger_id,
                    binding_slug: binding.binding_slug,
                })
                .collect()
        } else {
            Vec::new()
        },
        last_error: None,
    };
    store
        .replace_runtime(&record.id, record.revision, runtime)
        .context("save automation runtime state")
}

fn workflow_runtime_client_for_record_with_config(
    paths: &ConfigPaths,
    config: &PufferConfig,
    record: &AutomationRecord,
) -> Result<WorkflowRuntimeClient> {
    crate::daemon_workflow_runtime::workflow_runtime_client_for_mode(
        paths,
        config,
        workflow_backend_mode_for_run_location(record.spec.run_location),
    )
    .context("create workflow runtime client")
}

fn deploy_workflow_definition(
    client: &WorkflowRuntimeClient,
    record: &AutomationRecord,
    workflow: &CompiledWorkflowDefinition,
    existing_id: Option<String>,
    deploy: bool,
) -> Result<String> {
    let mut definition: AgentEnvWorkflowDefinition =
        serde_json::from_value(workflow.definition.clone())
            .context("compiled workflow definition must match AgentEnv schema")?;
    ensure_runtime_trigger(
        &mut definition,
        &runtime_trigger_path(record, &workflow.role),
    );
    let name = workflow_display_name(record, &workflow.role);
    let description = Some(format!(
        "Internal workflow artifact for Puffer Automation `{}` revision {}.",
        record.id, record.revision
    ));

    let artifact = if let Some(workflow_id) = existing_id {
        let request = WorkflowRuntimeUpdateWorkflowRequest {
            name: Some(name),
            description,
            definition: Some(definition),
            status: None,
        };
        client
            .update_workflow(&workflow_id, &request)
            .with_context(|| format!("update workflow `{workflow_id}`"))?
    } else {
        let request = WorkflowRuntimeCreateWorkflowRequest {
            name,
            description,
            definition,
        };
        client
            .create_workflow(&request)
            .context("create workflow artifact")?
    };

    let workflow_id = runtime_workflow_id(&artifact)?;
    if deploy {
        client
            .deploy_workflow(&workflow_id)
            .with_context(|| format!("deploy workflow `{workflow_id}`"))?;
    }
    Ok(workflow_id)
}

pub(crate) fn sync_automation_bindings_after_save(
    previous: Option<&AutomationRecord>,
    record: &AutomationRecord,
) -> Result<()> {
    let Ok(manager) = puffer_core::subscription_manager() else {
        return Ok(());
    };

    if let Some(previous) = previous {
        if previous.revision != record.revision
            && matches!(record.runtime.status, AutomationRuntimeStatus::Stale)
        {
            for slug in generated_binding_slugs(previous) {
                let _ = manager.store().delete(&slug);
            }
            manager.refresh_connection_consumers()?;
            return Ok(());
        }
        let current_slugs = generated_binding_slugs(record);
        for slug in generated_binding_slugs(previous) {
            if !current_slugs.iter().any(|current| current == &slug) {
                let _ = manager.store().delete(&slug);
            }
        }
    }

    for slug in generated_binding_slugs(record) {
        if manager.store().get(&slug).is_none() {
            continue;
        }
        let status = match record.status {
            AutomationStatus::Enabled => WorkflowBindingStatus::Enabled,
            AutomationStatus::Paused | AutomationStatus::Archived => WorkflowBindingStatus::Paused,
        };
        manager.store().set_status(&slug, status)?;
    }
    manager.refresh_connection_consumers()?;
    Ok(())
}

fn set_generated_binding_status(
    record: &AutomationRecord,
    status: WorkflowBindingStatus,
    require_existing: bool,
) -> Result<()> {
    let slugs = generated_binding_slugs(record);
    if slugs.is_empty() {
        return Ok(());
    }
    let manager = puffer_core::subscription_manager()
        .context("subscription manager is required to update Automation bindings")?;
    for slug in slugs {
        if manager.store().get(&slug).is_none() {
            if require_existing {
                anyhow::bail!("automation binding `{slug}` was not deployed");
            }
            continue;
        }
        manager.store().set_status(&slug, status)?;
    }
    manager.refresh_connection_consumers()?;
    Ok(())
}

pub(crate) fn remove_automation_bindings(record: &AutomationRecord) -> Result<()> {
    let Ok(manager) = puffer_core::subscription_manager() else {
        return Ok(());
    };
    for slug in generated_binding_slugs(record) {
        let _ = manager.store().delete(&slug);
    }
    manager.refresh_connection_consumers()?;
    Ok(())
}

fn generated_binding_slugs(record: &AutomationRecord) -> Vec<String> {
    let mut slugs = Vec::new();
    for binding in &record.runtime.puffer_bindings {
        slugs.push(binding.binding_slug.clone());
    }
    for trigger in &record.spec.triggers {
        if let puffer_automation::AutomationTriggerSpec::PufferConnection { id, .. } = trigger {
            slugs.push(format!("automation-{}-{id}", record.id));
        }
    }
    slugs.sort();
    slugs.dedup();
    slugs
}

fn deploy_puffer_bindings(record: &AutomationRecord) -> Result<()> {
    if !record.spec.triggers.iter().any(|trigger| {
        matches!(
            trigger,
            puffer_automation::AutomationTriggerSpec::PufferConnection { .. }
        )
    }) {
        return Ok(());
    }
    let manager = puffer_core::subscription_manager()
        .context("subscription manager is required to deploy Automation bindings")?;
    for trigger in &record.spec.triggers {
        let puffer_automation::AutomationTriggerSpec::PufferConnection {
            id,
            connection_slug,
            connector_slug,
            filter,
            ignore_filters,
            contact_ids,
            ..
        } = trigger
        else {
            continue;
        };
        let binding = WorkflowBindingSpec {
            slug: format!("automation-{}-{id}", record.id),
            description: format!("Run Automation {} from trigger {id}", record.id),
            connection_slug: connection_slug.clone(),
            connector_slug: connector_slug.clone(),
            status: match record.status {
                AutomationStatus::Enabled => WorkflowBindingStatus::Enabled,
                AutomationStatus::Paused | AutomationStatus::Archived => {
                    WorkflowBindingStatus::Paused
                }
            },
            filter: filter.clone(),
            ignore_filters: ignore_filters.clone(),
            contact_ids: contact_ids.clone(),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::RunAutomation {
                automation_id: record.id.clone(),
            },
            created_at_ms: puffer_subscriptions::now_ms(),
        };
        manager.store().upsert(binding)?;
    }
    manager.refresh_connection_consumers()?;
    Ok(())
}

fn execute_automation_with_run_id(
    paths: &ConfigPaths,
    record: &AutomationRecord,
    input: Value,
    run_id: &str,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<AutomationExecutionOutput> {
    let config = load_config(paths).context("load workflow backend config")?;
    let client = crate::daemon_workflow_runtime::workflow_runtime_client_for_mode(
        paths,
        &config,
        workflow_backend_mode_for_run_location(record.spec.run_location),
    )
    .context("create workflow runtime client")?;

    let root_id = workflow_id_for_role(record, &CompiledWorkflowRole::Root)
        .context("compiled Automation has no root workflow id")?;
    let root_trigger_id = root_trigger_node_id(record);
    let root_output = execute_workflow_value(
        &client,
        &root_id,
        json!({ "trigger": input }),
        root_trigger_id.as_deref(),
    )?;

    let final_output = execute_ordered_flow_after_root(
        &client,
        record,
        &input,
        &root_output,
        AutomationConnectorActionRunContext { paths, run_id },
        installed_connector_action_executor().as_deref(),
        provider_context,
        None,
    )?;
    Ok(automation_execution_output_from_final(record, final_output))
}

fn automation_execution_output_from_final(
    record: &AutomationRecord,
    final_output: Value,
) -> AutomationExecutionOutput {
    if final_output.get("kind").and_then(Value::as_str) == Some("connector_action_draft")
        && final_output.get("status").and_then(Value::as_str) == Some("draft_ready")
    {
        let step_id = final_output
            .get("step_id")
            .and_then(Value::as_str)
            .unwrap_or("connector action")
            .to_string();
        return AutomationExecutionOutput {
            result: final_output,
            summary: format!(
                "automation `{}` awaiting approval for connector action `{step_id}`",
                record.id
            ),
            status: "awaiting_approval".to_string(),
            approval: Some(AutomationRunApprovalRecord {
                required: true,
                status: "draft_ready".to_string(),
            }),
        };
    }

    AutomationExecutionOutput {
        summary: format!(
            "automation `{}` completed: {}",
            record.id,
            summarize_value(&final_output)
        ),
        result: final_output,
        status: "completed".to_string(),
        approval: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_ordered_flow_after_root(
    client: &WorkflowRuntimeClient,
    record: &AutomationRecord,
    trigger_input: &Value,
    root_output: &Value,
    run_context: AutomationConnectorActionRunContext<'_>,
    executor: Option<&dyn ConnectorActionExecutor>,
    provider_context: Option<&AutomationProviderContext<'_>>,
    resume: Option<&FlowResume>,
) -> Result<Value> {
    let mut output = root_output.clone();
    let mut start_index = 0;
    if let Some(resume) = resume {
        // The gated step already ran (on approval). Feed its connector result
        // into its own continuation, then continue from the next top-level step.
        output = resume.seed_output.clone();
        let role = CompiledWorkflowRole::Continuation {
            step_id: resume.step_id.clone(),
        };
        if let Some(workflow_id) = workflow_id_for_role(record, &role) {
            output = execute_workflow_value(
                client,
                &workflow_id,
                continuation_input(trigger_input, root_output, &output),
                None,
            )
            .with_context(|| {
                format!(
                    "run continuation after resumed connector action `{}`",
                    resume.step_id
                )
            })?;
        }
        start_index = resume.resume_step_index;
    }
    for (index, step) in record.spec.flow.steps.iter().enumerate() {
        if index < start_index {
            continue;
        }
        match step {
            AutomationStepSpec::AgentEnvNode { id, node, .. }
                if is_puffer_connector_action(node) =>
            {
                let role = CompiledWorkflowRole::Continuation {
                    step_id: id.clone(),
                };
                let has_continuation = workflow_id_for_role(record, &role).is_some();
                let is_terminal = index + 1 == record.spec.flow.steps.len() && !has_continuation;
                output = execute_puffer_connector_action_step(
                    record,
                    id,
                    node,
                    trigger_input,
                    root_output,
                    &output,
                    run_context,
                    if is_terminal {
                        ConnectorActionPosition::TopLevelTerminal
                    } else {
                        ConnectorActionPosition::TopLevelNonTerminal {
                            resume_step_index: index + 1,
                        }
                    },
                    executor,
                )?;
                // A gated action drafts and suspends: stop the flow here. Its
                // continuation and the remaining steps run on approval.
                if is_suspended_connector_action_draft(&output) {
                    break;
                }
                if let Some(workflow_id) = workflow_id_for_role(record, &role) {
                    output = execute_workflow_value(
                        client,
                        &workflow_id,
                        continuation_input(trigger_input, root_output, &output),
                        None,
                    )
                    .with_context(|| format!("run continuation after connector action `{id}`"))?;
                }
            }
            AutomationStepSpec::AgentEnvNode { id, node, .. } if is_puffer_agent(node) => {
                let role = CompiledWorkflowRole::Continuation {
                    step_id: id.clone(),
                };
                output = execute_puffer_agent_step(
                    run_context.paths,
                    record,
                    id,
                    node,
                    trigger_input,
                    root_output,
                    &output,
                    Value::Null,
                    None,
                    provider_context,
                )?;
                if let Some(workflow_id) = workflow_id_for_role(record, &role) {
                    output = execute_workflow_value(
                        client,
                        &workflow_id,
                        continuation_input(trigger_input, root_output, &output),
                        None,
                    )
                    .with_context(|| format!("run continuation after puffer_agent `{id}`"))?;
                }
            }
            AutomationStepSpec::Agent {
                id,
                instructions,
                mode,
                max_iterations,
                tools,
                ..
            } => {
                let role = CompiledWorkflowRole::Continuation {
                    step_id: id.clone(),
                };
                output = execute_agent_step(
                    record,
                    id,
                    instructions,
                    *mode,
                    *max_iterations,
                    tools,
                    trigger_input,
                    root_output,
                    &output,
                    run_context,
                    executor,
                    provider_context,
                )?;
                if let Some(workflow_id) = workflow_id_for_role(record, &role) {
                    output = execute_workflow_value(
                        client,
                        &workflow_id,
                        continuation_input(trigger_input, root_output, &output),
                        None,
                    )
                    .with_context(|| format!("run continuation after agent step `{id}`"))?;
                }
            }
            AutomationStepSpec::Loop {
                id,
                loop_spec,
                body,
                ..
            } => {
                let role = CompiledWorkflowRole::LoopBody {
                    step_id: id.clone(),
                };
                let loop_workflow_id = workflow_id_for_role(record, &role).with_context(|| {
                    format!("compiled Automation has no loop body workflow for `{id}`")
                })?;
                output = execute_loop(
                    client,
                    record,
                    &loop_workflow_id,
                    loop_spec,
                    body,
                    trigger_input,
                    &output,
                    run_context,
                    executor,
                    provider_context,
                )
                .with_context(|| format!("run loop `{id}`"))?;
            }
            _ => {}
        }
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn execute_ordered_flow_after_root_in_memory(
    client: &WorkflowRuntimeClient,
    workflows: &[CompiledWorkflowDefinition],
    record: &AutomationRecord,
    trigger_input: &Value,
    root_output: &Value,
    run_context: AutomationConnectorActionRunContext<'_>,
    executor: Option<&dyn ConnectorActionExecutor>,
    provider_context: Option<&AutomationProviderContext<'_>>,
    resume: Option<&FlowResume>,
) -> Result<Value> {
    let mut output = root_output.clone();
    let mut start_index = 0;
    if let Some(resume) = resume {
        // The gated step already ran (on approval). Feed its connector result
        // into its own continuation, then continue from the next top-level step.
        output = resume.seed_output.clone();
        let role = CompiledWorkflowRole::Continuation {
            step_id: resume.step_id.clone(),
        };
        if let Some(workflow) = compiled_workflow_for_role(workflows, &role) {
            output = execute_in_memory_value(
                client,
                compiled_agentenv_definition(workflow)?,
                continuation_input(trigger_input, root_output, &output),
                None,
            )
            .with_context(|| {
                format!(
                    "run continuation after resumed connector action `{}`",
                    resume.step_id
                )
            })?;
        }
        start_index = resume.resume_step_index;
    }
    for (index, step) in record.spec.flow.steps.iter().enumerate() {
        if index < start_index {
            continue;
        }
        match step {
            AutomationStepSpec::AgentEnvNode { id, node, .. }
                if is_puffer_connector_action(node) =>
            {
                let role = CompiledWorkflowRole::Continuation {
                    step_id: id.clone(),
                };
                let has_continuation = compiled_workflow_for_role(workflows, &role).is_some();
                let is_terminal = index + 1 == record.spec.flow.steps.len() && !has_continuation;
                output = execute_puffer_connector_action_step(
                    record,
                    id,
                    node,
                    trigger_input,
                    root_output,
                    &output,
                    run_context,
                    if is_terminal {
                        ConnectorActionPosition::TopLevelTerminal
                    } else {
                        ConnectorActionPosition::TopLevelNonTerminal {
                            resume_step_index: index + 1,
                        }
                    },
                    executor,
                )?;
                // A gated action drafts and suspends: stop the flow here. Its
                // continuation and the remaining steps run on approval.
                if is_suspended_connector_action_draft(&output) {
                    break;
                }
                if let Some(workflow) = compiled_workflow_for_role(workflows, &role) {
                    output = execute_in_memory_value(
                        client,
                        compiled_agentenv_definition(workflow)?,
                        continuation_input(trigger_input, root_output, &output),
                        None,
                    )
                    .with_context(|| format!("run continuation after connector action `{id}`"))?;
                }
            }
            AutomationStepSpec::AgentEnvNode { id, node, .. } if is_puffer_agent(node) => {
                let role = CompiledWorkflowRole::Continuation {
                    step_id: id.clone(),
                };
                output = execute_puffer_agent_step(
                    run_context.paths,
                    record,
                    id,
                    node,
                    trigger_input,
                    root_output,
                    &output,
                    Value::Null,
                    None,
                    provider_context,
                )?;
                if let Some(workflow) = compiled_workflow_for_role(workflows, &role) {
                    output = execute_in_memory_value(
                        client,
                        compiled_agentenv_definition(workflow)?,
                        continuation_input(trigger_input, root_output, &output),
                        None,
                    )
                    .with_context(|| format!("run continuation after puffer_agent `{id}`"))?;
                }
            }
            AutomationStepSpec::Agent {
                id,
                instructions,
                mode,
                max_iterations,
                tools,
                ..
            } => {
                let role = CompiledWorkflowRole::Continuation {
                    step_id: id.clone(),
                };
                output = execute_agent_step(
                    record,
                    id,
                    instructions,
                    *mode,
                    *max_iterations,
                    tools,
                    trigger_input,
                    root_output,
                    &output,
                    run_context,
                    executor,
                    provider_context,
                )?;
                if let Some(workflow) = compiled_workflow_for_role(workflows, &role) {
                    output = execute_in_memory_value(
                        client,
                        compiled_agentenv_definition(workflow)?,
                        continuation_input(trigger_input, root_output, &output),
                        None,
                    )
                    .with_context(|| format!("run continuation after agent step `{id}`"))?;
                }
            }
            AutomationStepSpec::Loop {
                id,
                loop_spec,
                body,
                ..
            } => {
                let role = CompiledWorkflowRole::LoopBody {
                    step_id: id.clone(),
                };
                let loop_workflow =
                    compiled_workflow_for_role(workflows, &role).with_context(|| {
                        format!("compiled Automation has no loop body workflow for `{id}`")
                    })?;
                output = execute_loop_in_memory(
                    client,
                    record,
                    compiled_agentenv_definition(loop_workflow)?,
                    loop_spec,
                    body,
                    trigger_input,
                    &output,
                    run_context,
                    executor,
                    provider_context,
                )
                .with_context(|| format!("run loop `{id}`"))?;
            }
            _ => {}
        }
    }
    Ok(output)
}

fn continuation_input(trigger: &Value, root_output: &Value, previous_output: &Value) -> Value {
    json!({
        "trigger": trigger,
        "root_output": root_output,
        "previous_output": previous_output,
    })
}

fn execute_puffer_connector_action_step(
    record: &AutomationRecord,
    id: &str,
    node: &puffer_automation::AgentEnvNodeRef,
    trigger_input: &Value,
    root_output: &Value,
    previous_output: &Value,
    run_context: AutomationConnectorActionRunContext<'_>,
    position: ConnectorActionPosition,
    executor: Option<&dyn ConnectorActionExecutor>,
) -> Result<Value> {
    let connector_slug = node_config_string(node, "connector_slug")
        .with_context(|| format!("connector action step `{id}` missing connector_slug"))?;
    let connection_slug = node_config_string(node, "connection_slug")
        .with_context(|| format!("connector action step `{id}` missing connection_slug"))?;
    let action = node_config_string(node, "action")
        .with_context(|| format!("connector action step `{id}` missing action"))?;
    let action_input = connector_action_input(node, trigger_input, root_output, previous_output);
    if connector_action_is_gated(node) {
        return execute_gated_connector_action_step(
            record,
            id,
            &connector_slug,
            &connection_slug,
            &action,
            action_input,
            trigger_input,
            root_output,
            previous_output,
            run_context,
            position,
        );
    }

    // Connector-backed UI tools are Puffer-owned runtime steps. They execute
    // through the daemon connector executor, then feed their structured bridge
    // result into any AgentEnv continuation workflow.
    let executor = executor.context("connector action executor is not installed")?;
    let action_trigger = json!({
        "type": "automation_connector_action",
        "envelope_id": format!("automation-{}-{id}", record.id),
        "connection_id": connection_slug,
        "receivedAt": time::OffsetDateTime::now_utc().to_string(),
        "topic": connection_slug,
        "kind": "connector_action",
        "dedup_key": format!("automation-{}-{id}", record.id),
        "text": connector_action_text(trigger_input, root_output, previous_output),
        "payload": {
            "automation_id": record.id,
            "step_id": id,
            "trigger": trigger_input,
            "root_output": root_output,
            "previous_output": previous_output,
        }
    });
    let summary = executor
        .run_connector_action(
            &connector_slug,
            &action,
            action_input.clone(),
            action_trigger,
        )
        .with_context(|| {
            format!("run connector action `{connector_slug}.{action}` for step `{id}`")
        })?;
    Ok(json!({
        "kind": "connector_action_result",
        "step_id": id,
        "connector_slug": connector_slug,
        "connection_slug": connection_slug,
        "action": action,
        "summary": summary,
        "input": action_input,
        "previous_output": previous_output,
    }))
}

#[allow(clippy::too_many_arguments)]
fn execute_gated_connector_action_step(
    record: &AutomationRecord,
    id: &str,
    connector_slug: &str,
    connection_slug: &str,
    action: &str,
    action_input: Value,
    trigger_input: &Value,
    root_output: &Value,
    previous_output: &Value,
    run_context: AutomationConnectorActionRunContext<'_>,
    position: ConnectorActionPosition,
) -> Result<Value> {
    // Loop-body and agent-step-tool gated actions are rejected at save/compile
    // time (see validate_flow_gated_connector_actions); this run-time guard is a
    // backstop. Top-level actions draft-and-suspend: terminal ones complete on
    // approval, non-terminal ones persist a resume point for the continuation.
    let resume_step_index = match position {
        ConnectorActionPosition::LoopBody => bail!(
            "Automation connector action step `{id}` requires human approval inside a loop; loop-body gated connector actions are not supported"
        ),
        ConnectorActionPosition::TopLevelNonTerminal { resume_step_index } => {
            Some(resume_step_index)
        }
        ConnectorActionPosition::TopLevelTerminal => None,
    };

    let draft = create_automation_connector_action_draft(
        run_context.paths,
        AutomationConnectorActionDraftParams {
            automation_id: record.id.clone(),
            automation_run_id: run_context.run_id.to_string(),
            step_id: id.to_string(),
            connector_slug: connector_slug.to_string(),
            connection_slug: connection_slug.to_string(),
            action: action.to_string(),
            input: action_input,
        },
    )
    .with_context(|| format!("create connector action draft for Automation step `{id}`"))?;

    if let Some(resume_step_index) = resume_step_index {
        upsert_suspension(
            run_context.paths,
            AutomationRunSuspension {
                automation_id: record.id.clone(),
                run_id: run_context.run_id.to_string(),
                step_id: id.to_string(),
                draft_id: draft.draft_id.clone(),
                connector_slug: connector_slug.to_string(),
                connection_slug: connection_slug.to_string(),
                action: action.to_string(),
                resume_step_index,
                trigger_input: trigger_input.clone(),
                root_output: root_output.clone(),
                previous_output: previous_output.clone(),
            },
        )
        .with_context(|| format!("persist suspension for Automation step `{id}`"))?;
    }

    Ok(automation_connector_action_draft_output(id, draft))
}

/// True when a step output is a gated connector-action draft that suspended the
/// run (as opposed to an executed `connector_action_result`).
fn is_suspended_connector_action_draft(output: &Value) -> bool {
    output.get("kind").and_then(Value::as_str) == Some("connector_action_draft")
        && output.get("status").and_then(Value::as_str) == Some("draft_ready")
}

fn automation_connector_action_draft_output(
    step_id: &str,
    draft: CreatedAutomationConnectorActionDraft,
) -> Value {
    let message_editable = draft.action == "send_message";
    let approval_kind = if message_editable {
        "editable_message"
    } else {
        "exact_action"
    };
    json!({
        "kind": "connector_action_draft",
        "status": draft.status,
        "step_id": step_id,
        "draft_id": draft.draft_id,
        "version": draft.version,
        "connector_slug": draft.connector_slug,
        "connection_slug": draft.connection_slug,
        "action": draft.action,
        "recipient_stable_id": draft.recipient_stable_id,
        "message": draft.message,
        "content_hash": draft.content_hash,
        "message_editable": message_editable,
        "approval_kind": approval_kind,
    })
}

fn execute_puffer_agent_step(
    paths: &ConfigPaths,
    record: &AutomationRecord,
    id: &str,
    node: &puffer_automation::AgentEnvNodeRef,
    trigger_input: &Value,
    root_output: &Value,
    previous_output: &Value,
    loop_input: Value,
    iteration: Option<u32>,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<Value> {
    let context = provider_context.context(
        "Automation uses puffer_agent but daemon resolver is missing provider/model context",
    )?;
    let step_instructions = node
        .config
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let prompt = puffer_agent_prompt(
        record,
        id,
        step_instructions,
        &[],
        trigger_input,
        root_output,
        previous_output,
        &loop_input,
        iteration,
    )?;
    let turn = run_agent_prompt_turn(paths, context, id, &prompt)?;
    let decision = parse_puffer_agent_decision(&turn.assistant_text);
    Ok(puffer_agent_result_value(id, &decision, &turn))
}

/// One executed agent turn.
struct AgentTurnResult {
    assistant_text: String,
    tool_use_count: usize,
}

/// Runs a single Puffer-managed agent turn for the given prompt. This is the
/// shared engine behind both the single-shot `puffer_agent` node and the
/// iterative [`AutomationStepSpec::Agent`] step.
fn run_agent_prompt_turn(
    paths: &ConfigPaths,
    context: &AutomationProviderContext<'_>,
    id: &str,
    prompt: &str,
) -> Result<AgentTurnResult> {
    let resources = context.resources.context(
        "Automation uses an agent step but daemon resolver is missing loaded Puffer resources",
    )?;
    let config = load_config(paths).context("load Puffer config for agent step")?;
    let session_store = SessionStore::from_paths(paths)?;
    let session = session_store
        .create_session_with_tags(
            paths.workspace_root.clone(),
            vec![BACKGROUND_SESSION_TAG.to_string()],
        )
        .context("create agent step session")?;
    let mut app_state =
        puffer_core::AppState::new(config.clone(), paths.workspace_root.clone(), session);
    if let Some(provider) = config
        .default_provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        app_state.current_provider = Some(canonical_provider_id(provider));
    }
    if let Some(model) = config
        .default_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        app_state.current_model = Some(model.to_string());
    }

    let mut auth_store = context.auth_store.clone();
    let turn = puffer_core::execute_user_turn(
        &mut app_state,
        resources,
        context.providers,
        &mut auth_store,
        prompt,
    )
    .with_context(|| format!("run agent step `{id}`"))?;
    Ok(AgentTurnResult {
        tool_use_count: turn.tool_invocations.len(),
        assistant_text: turn.assistant_text,
    })
}

fn puffer_agent_result_value(
    id: &str,
    decision: &PufferAgentDecision,
    turn: &AgentTurnResult,
) -> Value {
    json!({
        "kind": "puffer_agent_result",
        "step_id": id,
        "done": decision.done,
        "reason": decision.reason,
        "next_input": decision.next_input,
        "output": decision.output,
        "tool_calls": decision.tool_calls,
        "raw": {
            "assistant_text": turn.assistant_text,
            "tool_use_count": turn.tool_use_count,
        },
    })
}

struct PufferAgentDecision {
    done: bool,
    reason: String,
    next_input: Value,
    output: Value,
    /// Tools the agent requested for this turn, in call order.
    tool_calls: Vec<AgentToolCall>,
}

/// One tool invocation the agent asked the runner to perform.
#[derive(Debug, Clone, Serialize)]
struct AgentToolCall {
    /// Id of the tool declared on the agent step.
    tool_id: String,
    /// Agent-provided input for the call. Surfaced to the connector action as
    /// its `previous_output` so the tool can act on what the agent requested.
    input: Value,
}

fn parse_agent_tool_calls(value: &Value) -> Vec<AgentToolCall> {
    let Some(items) = value.get("tool_calls").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let tool_id = item
                .get("tool_id")
                .or_else(|| item.get("tool"))
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            Some(AgentToolCall {
                tool_id: tool_id.to_string(),
                input: item.get("input").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn parse_puffer_agent_decision(text: &str) -> PufferAgentDecision {
    if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        let done = value.get("done").and_then(Value::as_bool).unwrap_or(false);
        let reason = value
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let next_input = value.get("next_input").cloned().unwrap_or(Value::Null);
        let tool_calls = parse_agent_tool_calls(&value);
        return PufferAgentDecision {
            done,
            reason,
            next_input,
            tool_calls,
            output: value,
        };
    }
    PufferAgentDecision {
        done: false,
        reason: "agent step did not return structured JSON".to_string(),
        next_input: Value::Null,
        tool_calls: Vec::new(),
        output: json!({ "text": text }),
    }
}

#[allow(clippy::too_many_arguments)]
fn puffer_agent_prompt(
    record: &AutomationRecord,
    step_id: &str,
    step_instructions: Option<&str>,
    tools: &[AutomationAgentToolSpec],
    trigger_input: &Value,
    root_output: &Value,
    previous_output: &Value,
    loop_input: &Value,
    iteration: Option<u32>,
) -> Result<String> {
    let tool_catalog: Vec<Value> = tools
        .iter()
        .map(|tool| {
            json!({
                "tool_id": tool.id,
                "action": tool.node.config.get("action"),
                "connector_slug": tool.node.config.get("connector_slug"),
                "summary": tool.summary,
            })
        })
        .collect();
    let context = json!({
        "automation_id": record.id,
        "automation_instructions": record.spec.instructions,
        "step_id": step_id,
        "step_instructions": step_instructions,
        "available_tools": tool_catalog,
        "trigger": trigger_input,
        "root_output": root_output,
        "previous_output": previous_output,
        "loop_input": loop_input,
        "iteration": iteration,
    });
    let tools_line = if tools.is_empty() {
        String::new()
    } else {
        "To use a tool, include a `tool_calls` array of {tool_id, input} objects; the runner executes them and feeds the results back on the next turn.\n         ".to_string()
    };
    Ok(format!(
        "You are the Puffer-owned automation agent step `{step_id}`.\n\
         Decide whether the user's automation goal is complete after the latest state.\n\
         {tools_line}Return only strict JSON with keys: done (boolean), reason (string), next_input (object or null), output (object or string), tool_calls (array or omitted).\n\n\
         Context:\n```json\n{}\n```",
        serde_json::to_string_pretty(&context)?
    ))
}

/// Runs a first-class iterative agent step. The loop is the agent's run
/// strategy: each turn the agent observes the current state, optionally requests
/// tool calls (which the daemon executes and feeds back), and decides whether
/// the goal is done. The loop stops when the agent reports done, when the hard
/// iteration cap is reached, or immediately in `once` mode.
#[allow(clippy::too_many_arguments)]
fn execute_agent_step(
    record: &AutomationRecord,
    id: &str,
    instructions: &str,
    mode: AutomationAgentMode,
    max_iterations: Option<u32>,
    tools: &[AutomationAgentToolSpec],
    trigger_input: &Value,
    root_output: &Value,
    previous_output: &Value,
    run_context: AutomationConnectorActionRunContext<'_>,
    executor: Option<&dyn ConnectorActionExecutor>,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<Value> {
    let context = provider_context.context(
        "Automation uses an agent step but daemon resolver is missing provider/model context",
    )?;
    let iterations = match mode {
        AutomationAgentMode::Once => 1,
        AutomationAgentMode::UntilDone => max_iterations.unwrap_or(1).max(1),
    };

    let mut carried = previous_output.clone();
    let mut last = Value::Null;
    for index in 0..iterations {
        let prompt = puffer_agent_prompt(
            record,
            id,
            Some(instructions),
            tools,
            trigger_input,
            root_output,
            &carried,
            &Value::Null,
            Some(index),
        )?;
        let turn = run_agent_prompt_turn(run_context.paths, context, id, &prompt)?;
        let decision = parse_puffer_agent_decision(&turn.assistant_text);
        let agent_result = puffer_agent_result_value(id, &decision, &turn);

        let mut tool_results = Vec::new();
        for call in &decision.tool_calls {
            let tool = tools
                .iter()
                .find(|tool| tool.id == call.tool_id)
                .with_context(|| {
                    format!(
                        "agent step `{id}` requested unknown tool `{}`",
                        call.tool_id
                    )
                })?;
            // Agent tools run through the same connector-action executor as
            // loop-body actions. Gated (human-approval) actions are rejected
            // mid-loop by ConnectorActionPosition::LoopBody; the reviewed
            // outward effect stays a terminal connector action step.
            let result = execute_puffer_connector_action_step(
                record,
                &tool.id,
                &tool.node,
                trigger_input,
                root_output,
                &call.input,
                run_context,
                ConnectorActionPosition::LoopBody,
                executor,
            )
            .with_context(|| format!("run agent step `{id}` tool `{}`", tool.id))?;
            tool_results.push(json!({ "tool_id": tool.id, "result": result }));
        }

        carried = json!({
            "kind": "agent_step_iteration",
            "step_id": id,
            "iteration": index,
            "done": decision.done,
            "agent": agent_result,
            "tool_results": tool_results,
        });
        last = carried.clone();

        if matches!(mode, AutomationAgentMode::Once) || decision.done {
            break;
        }
    }

    Ok(last)
}

fn execute_loop_body_connector_actions(
    record: &AutomationRecord,
    body: &AutomationFlowSpec,
    trigger_input: &Value,
    root_output: &Value,
    previous_output: &Value,
    run_context: AutomationConnectorActionRunContext<'_>,
    executor: Option<&dyn ConnectorActionExecutor>,
) -> Result<Value> {
    let mut output = previous_output.clone();
    for step in &body.steps {
        let AutomationStepSpec::AgentEnvNode { id, node, .. } = step else {
            continue;
        };
        if !is_puffer_connector_action(node) {
            continue;
        }
        output = execute_puffer_connector_action_step(
            record,
            id,
            node,
            trigger_input,
            root_output,
            &output,
            run_context,
            ConnectorActionPosition::LoopBody,
            executor,
        )?;
    }
    Ok(output)
}

fn execute_loop_body_puffer_agents(
    paths: &ConfigPaths,
    record: &AutomationRecord,
    body: &AutomationFlowSpec,
    trigger_input: &Value,
    root_output: &Value,
    previous_output: &Value,
    loop_input: Value,
    iteration: u32,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<Value> {
    let mut output = previous_output.clone();
    for step in &body.steps {
        let AutomationStepSpec::AgentEnvNode { id, node, .. } = step else {
            continue;
        };
        if !is_puffer_agent(node) {
            continue;
        }
        output = execute_puffer_agent_step(
            paths,
            record,
            id,
            node,
            trigger_input,
            root_output,
            &output,
            loop_input.clone(),
            Some(iteration),
            provider_context,
        )?;
    }
    Ok(output)
}

fn is_puffer_connector_action(node: &puffer_automation::AgentEnvNodeRef) -> bool {
    node.node_type == "puffer_connector_action"
}

fn is_puffer_agent(node: &puffer_automation::AgentEnvNodeRef) -> bool {
    node.node_type == "puffer_agent"
}

fn connector_action_is_gated(node: &puffer_automation::AgentEnvNodeRef) -> bool {
    node.config_bool("draft_only")
        || node.config_bool("human_approval_required")
        || node.config_bool("external_side_effect")
}

fn node_config_string(node: &puffer_automation::AgentEnvNodeRef, key: &str) -> Option<String> {
    node.config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn connector_action_input(
    node: &puffer_automation::AgentEnvNodeRef,
    trigger_input: &Value,
    root_output: &Value,
    previous_output: &Value,
) -> Value {
    let mut input = node
        .config
        .get("input")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for key in ["connector_slug", "connection_slug", "action", "target"] {
        if let Some(value) = node
            .config
            .get(key)
            .filter(|value| config_value_is_present(value))
        {
            input
                .entry(key.to_string())
                .or_insert_with(|| value.clone());
        }
    }
    input
        .entry("trigger".to_string())
        .or_insert_with(|| trigger_input.clone());
    input
        .entry("root_output".to_string())
        .or_insert_with(|| root_output.clone());
    input
        .entry("previous_output".to_string())
        .or_insert_with(|| previous_output.clone());
    Value::Object(input)
}

fn connector_action_text(
    trigger_input: &Value,
    root_output: &Value,
    previous_output: &Value,
) -> String {
    for value in [previous_output, root_output, trigger_input] {
        if let Some(text) = value
            .get("text")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return text.to_string();
        }
    }
    summarize_value(previous_output)
}

fn workflow_backend_mode_for_run_location(
    run_location: AutomationRunLocation,
) -> WorkflowBackendMode {
    match run_location {
        AutomationRunLocation::Local => WorkflowBackendMode::Local,
        AutomationRunLocation::AgentEnvCloud => WorkflowBackendMode::AgentEnvCloud,
    }
}

fn execute_loop(
    client: &WorkflowRuntimeClient,
    record: &AutomationRecord,
    workflow_id: &str,
    loop_spec: &AutomationLoopSpec,
    body: &AutomationFlowSpec,
    trigger: &Value,
    root_output: &Value,
    run_context: AutomationConnectorActionRunContext<'_>,
    executor: Option<&dyn ConnectorActionExecutor>,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<Value> {
    match loop_spec {
        AutomationLoopSpec::ForEach {
            input,
            item_alias,
            max_iterations,
        } => {
            let collection = resolve_loop_input(input, trigger, root_output, &Value::Null)?;
            let items = collection
                .as_array()
                .context("foreach loop input must resolve to a JSON array")?;
            let limit = max_iterations
                .map(|value| value as usize)
                .unwrap_or(items.len())
                .min(items.len());
            let mut previous_output = Value::Null;
            for (index, item) in items.iter().take(limit).enumerate() {
                previous_output = execute_workflow_value(
                    client,
                    workflow_id,
                    json!({
                        "trigger": trigger,
                        "root_output": root_output,
                        "previous_output": previous_output,
                        "loop_input": item,
                        "item": item,
                        item_alias: item,
                        "iteration": index,
                    }),
                    None,
                )?;
                previous_output = execute_loop_body_puffer_agents(
                    run_context.paths,
                    record,
                    body,
                    trigger,
                    root_output,
                    &previous_output,
                    item.clone(),
                    index as u32,
                    provider_context,
                )?;
                previous_output = execute_loop_body_connector_actions(
                    record,
                    body,
                    trigger,
                    root_output,
                    &previous_output,
                    run_context,
                    executor,
                )?;
            }
            Ok(previous_output)
        }
    }
}

fn execute_loop_in_memory(
    client: &WorkflowRuntimeClient,
    record: &AutomationRecord,
    definition: AgentEnvWorkflowDefinition,
    loop_spec: &AutomationLoopSpec,
    body: &AutomationFlowSpec,
    trigger: &Value,
    root_output: &Value,
    run_context: AutomationConnectorActionRunContext<'_>,
    executor: Option<&dyn ConnectorActionExecutor>,
    provider_context: Option<&AutomationProviderContext<'_>>,
) -> Result<Value> {
    match loop_spec {
        AutomationLoopSpec::ForEach {
            input,
            item_alias,
            max_iterations,
        } => {
            let collection = resolve_loop_input(input, trigger, root_output, &Value::Null)?;
            let items = collection
                .as_array()
                .context("foreach loop input must resolve to a JSON array")?;
            let limit = max_iterations
                .map(|value| value as usize)
                .unwrap_or(items.len())
                .min(items.len());
            let mut previous_output = Value::Null;
            for (index, item) in items.iter().take(limit).enumerate() {
                previous_output = execute_in_memory_value(
                    client,
                    definition.clone(),
                    json!({
                        "trigger": trigger,
                        "root_output": root_output,
                        "previous_output": previous_output,
                        "loop_input": item,
                        "item": item,
                        item_alias: item,
                        "iteration": index,
                    }),
                    None,
                )?;
                previous_output = execute_loop_body_puffer_agents(
                    run_context.paths,
                    record,
                    body,
                    trigger,
                    root_output,
                    &previous_output,
                    item.clone(),
                    index as u32,
                    provider_context,
                )?;
                previous_output = execute_loop_body_connector_actions(
                    record,
                    body,
                    trigger,
                    root_output,
                    &previous_output,
                    run_context,
                    executor,
                )?;
            }
            Ok(previous_output)
        }
    }
}

fn execute_workflow_value(
    client: &WorkflowRuntimeClient,
    workflow_id: &str,
    input: Value,
    trigger_node_id: Option<&str>,
) -> Result<Value> {
    let mut fields = BTreeMap::new();
    fields.insert("input".to_string(), input);
    if let Some(trigger_node_id) = trigger_node_id {
        fields.insert("triggerNodeId".to_string(), json!(trigger_node_id));
    }
    let request = WorkflowRuntimeRecord::new(fields);
    let response = client
        .execute_workflow(workflow_id, &request)
        .with_context(|| format!("execute workflow `{workflow_id}`"))?;
    Ok(serde_json::to_value(response)?)
}

fn execute_in_memory_value(
    client: &WorkflowRuntimeClient,
    mut definition: AgentEnvWorkflowDefinition,
    input: Value,
    trigger_node_id: Option<&str>,
) -> Result<Value> {
    ensure_runtime_trigger(&mut definition, RUNTIME_TRIGGER_IN_MEMORY_PATH);
    let request = WorkflowRuntimeInMemoryExecuteRequest {
        definition,
        input: Some(input_fields(input)?),
        trigger_node_id: trigger_node_id.map(ToString::to_string),
    };
    let response = client
        .execute_in_memory(&request)
        .context("execute in-memory workflow")?;
    Ok(serde_json::to_value(response)?)
}

/// Synthetic entry node id injected into compiled workflow definitions that lack
/// an AgentEnv trigger node.
const RUNTIME_TRIGGER_NODE_ID: &str = "__puffer_runtime_trigger";
/// Placeholder webhook path used for in-memory execution, where the path is
/// never routed.
const RUNTIME_TRIGGER_IN_MEMORY_PATH: &str = "puffer/automation/in-memory";
/// AgentEnv node types that satisfy the runtime's "workflow must have a trigger
/// node" requirement.
const RUNTIME_TRIGGER_NODE_TYPES: &[&str] = &["webhook", "schedule"];

/// Ensures a compiled workflow definition carries an AgentEnv trigger node.
///
/// AgentEnv requires every workflow (including in-memory executions) to start
/// from a trigger node such as `webhook` or `schedule`. Puffer's compiler emits
/// trigger-less definitions for `puffer_connection` and loop automations because
/// the trigger and the loop are owned by the Puffer runner, not AgentEnv. Those
/// definitions are still invoked through the runtime's execute APIs, so before we
/// hand one to the runtime we prepend a synthetic `webhook` entry (a passthrough
/// whose output is the execution input) and wire it to the current entry nodes.
/// This keeps the Puffer product model trigger-less while satisfying the runtime
/// contract. No-op when a trigger node is already present.
fn ensure_runtime_trigger(definition: &mut AgentEnvWorkflowDefinition, path: &str) {
    let has_trigger = definition
        .nodes
        .iter()
        .any(|node| RUNTIME_TRIGGER_NODE_TYPES.contains(&node.node_type.as_str()));
    if has_trigger {
        return;
    }

    let entry_ids: Vec<String> = definition
        .nodes
        .iter()
        .filter(|node| !definition.edges.iter().any(|edge| edge.target == node.id))
        .map(|node| node.id.clone())
        .collect();

    let mut config = BTreeMap::new();
    config.insert("path".to_string(), json!(path));
    config.insert("methods".to_string(), json!(["POST"]));
    config.insert("authentication".to_string(), json!("none"));
    definition.nodes.insert(
        0,
        AgentEnvWorkflowNode {
            id: RUNTIME_TRIGGER_NODE_ID.to_string(),
            node_type: "webhook".to_string(),
            name: Some("Puffer entry".to_string()),
            config,
            trusted: Some(false),
            position: None,
        },
    );
    for entry_id in entry_ids {
        definition.edges.push(AgentEnvWorkflowEdge {
            source: RUNTIME_TRIGGER_NODE_ID.to_string(),
            target: entry_id,
            condition_script: None,
        });
    }
}

/// Deterministic, per-artifact webhook path so deployed synthetic triggers do
/// not collide across an automation's workflows.
fn runtime_trigger_path(record: &AutomationRecord, role: &CompiledWorkflowRole) -> String {
    let suffix = match role {
        CompiledWorkflowRole::Root => "root".to_string(),
        CompiledWorkflowRole::LoopBody { step_id } => format!("loop-{step_id}"),
        CompiledWorkflowRole::Continuation { step_id } => format!("continuation-{step_id}"),
        CompiledWorkflowRole::Helper { step_id } => format!("helper-{step_id}"),
    };
    format!("puffer/automation/{}/{}", record.id, suffix)
}

fn input_fields(input: Value) -> Result<BTreeMap<String, Value>> {
    match input {
        Value::Object(map) => Ok(map.into_iter().collect()),
        other => {
            let mut fields = BTreeMap::new();
            fields.insert("value".to_string(), other);
            Ok(fields)
        }
    }
}

fn compiled_workflow_for_role<'a>(
    workflows: &'a [CompiledWorkflowDefinition],
    role: &CompiledWorkflowRole,
) -> Option<&'a CompiledWorkflowDefinition> {
    workflows.iter().find(|workflow| &workflow.role == role)
}

fn compiled_agentenv_definition(
    workflow: &CompiledWorkflowDefinition,
) -> Result<AgentEnvWorkflowDefinition> {
    serde_json::from_value(workflow.definition.clone())
        .context("compiled workflow definition must match AgentEnv schema")
}

fn root_trigger_node_id(record: &AutomationRecord) -> Option<String> {
    match record.spec.triggers.as_slice() {
        [puffer_automation::AutomationTriggerSpec::AgentEnvNode { id, .. }] => Some(id.clone()),
        _ => None,
    }
}

fn runtime_needs_preview_sync(record: &AutomationRecord) -> Result<bool> {
    let current_hash = puffer_automation::automation_spec_hash(&record.spec)
        .map_err(|error| anyhow::anyhow!("hash automation spec: {error}"))?;
    Ok(!matches!(
        record.runtime.status,
        AutomationRuntimeStatus::DraftSynced | AutomationRuntimeStatus::Deployed
    ) || record.runtime.compiled_revision != Some(record.revision)
        || record.runtime.spec_hash.as_deref() != Some(current_hash.as_str())
        || record.runtime.agentenv_workflows.is_empty())
}

fn ensure_preview_automation_can_run(record: &AutomationRecord) -> Result<()> {
    if runtime_needs_preview_sync(record)? {
        bail!(
            "automation `{}` runtime is not deployed for revision {}; deploy before running a test preview",
            record.id,
            record.revision
        );
    }
    validate_automation_gated_connector_actions(record)?;
    Ok(())
}

fn runtime_needs_deploy(record: &AutomationRecord) -> Result<bool> {
    let current_hash = puffer_automation::automation_spec_hash(&record.spec)
        .map_err(|error| anyhow::anyhow!("hash automation spec: {error}"))?;
    Ok(record.runtime.status != AutomationRuntimeStatus::Deployed
        || record.runtime.compiled_revision != Some(record.revision)
        || record.runtime.spec_hash.as_deref() != Some(current_hash.as_str())
        || record.runtime.agentenv_workflows.is_empty())
}

fn ensure_live_automation_can_run(record: &AutomationRecord) -> Result<()> {
    if record.status != AutomationStatus::Enabled {
        bail!(
            "automation `{}` is {:?}; only enabled automations can run from connector events",
            record.id,
            record.status
        );
    }
    validate_automation_gated_connector_actions(record)?;
    Ok(())
}

/// Rejects human-gated connector actions in positions the runner cannot yet
/// suspend/resume. Top-level gated actions — terminal or mid-flow — are allowed
/// (they draft-and-suspend, then resume the continuation on approval). Loop-body
/// and agent-step-tool gated actions are not yet supported and are rejected here
/// so authors are warned at save/compile time rather than at run time.
fn validate_automation_gated_connector_actions(record: &AutomationRecord) -> Result<()> {
    validate_flow_gated_connector_actions(&record.spec.flow, false).with_context(|| {
        format!(
            "automation `{}` contains unsupported human-gated connector actions",
            record.id
        )
    })
}

fn validate_flow_gated_connector_actions(
    flow: &puffer_automation::AutomationFlowSpec,
    in_loop: bool,
) -> Result<()> {
    for step in flow.steps.iter() {
        match step {
            AutomationStepSpec::AgentEnvNode { id, node, .. }
                if is_puffer_connector_action(node) && connector_action_is_gated(node) =>
            {
                if in_loop {
                    bail!(
                        "Automation connector action step `{id}` requires human approval inside a loop; loop-body gated connector actions are not supported"
                    );
                }
                // Top-level gated actions (terminal or mid-flow) suspend and
                // resume; they are supported.
            }
            AutomationStepSpec::Agent { id, tools, .. } => {
                if let Some(tool) = tools.iter().find(|tool| {
                    is_puffer_connector_action(&tool.node) && connector_action_is_gated(&tool.node)
                }) {
                    bail!(
                        "Automation agent step `{id}` tool `{}` requires human approval; agent-step gated connector actions are not supported. Move the gated action to a top-level connector action step instead.",
                        tool.id
                    );
                }
            }
            AutomationStepSpec::Loop { body, .. } => {
                validate_flow_gated_connector_actions(body, true)?;
            }
            _ => {}
        }
    }
    Ok(())
}

trait AgentEnvNodeRefConfigExt {
    fn config_bool(&self, key: &str) -> bool;
}

impl AgentEnvNodeRefConfigExt for puffer_automation::AgentEnvNodeRef {
    fn config_bool(&self, key: &str) -> bool {
        self.config
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

fn config_value_is_present(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.trim().is_empty(),
        _ => true,
    }
}

fn resolve_loop_input(
    input: &AutomationLoopInput,
    trigger: &Value,
    root_output: &Value,
    previous_output: &Value,
) -> Result<Value> {
    match input {
        AutomationLoopInput::Trigger => Ok(trigger.clone()),
        AutomationLoopInput::Static { value } => Ok(value.clone()),
        AutomationLoopInput::StepOutput { path, .. } => {
            let value = if previous_output.is_null() {
                root_output
            } else {
                previous_output
            };
            Ok(path
                .as_deref()
                .and_then(|path| json_path_value(value, path))
                .cloned()
                .unwrap_or_else(|| value.clone()))
        }
    }
}

fn json_path_value<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let trimmed = path.trim();
    let trimmed = trimmed.strip_prefix('$').unwrap_or(trimmed);
    let trimmed = trimmed.strip_prefix('.').unwrap_or(trimmed);
    if trimmed.is_empty() {
        return Some(value);
    }
    let mut current = value;
    for part in trimmed.split('.') {
        current = match current {
            Value::Object(map) => map.get(part)?,
            Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn workflow_id_for_role(record: &AutomationRecord, role: &CompiledWorkflowRole) -> Option<String> {
    record
        .runtime
        .agentenv_workflows
        .iter()
        .find(|workflow| workflow.role == *role)
        .and_then(|workflow| workflow.workflow_id.clone())
}

fn existing_workflow_id(record: &AutomationRecord, role: &CompiledWorkflowRole) -> Option<String> {
    workflow_id_for_role(record, role)
}

fn runtime_workflow_id(record: &WorkflowRuntimeWorkflow) -> Result<String> {
    for key in WORKFLOW_ID_KEYS {
        if let Some(value) = record
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(value.to_string());
        }
    }
    bail!("workflow runtime response did not include a workflow id")
}

fn workflow_display_name(record: &AutomationRecord, role: &CompiledWorkflowRole) -> String {
    match role {
        CompiledWorkflowRole::Root => format!("Automation {} root", record.id),
        CompiledWorkflowRole::LoopBody { step_id } => {
            format!("Automation {} loop {step_id}", record.id)
        }
        CompiledWorkflowRole::Continuation { step_id } => {
            format!("Automation {} continuation {step_id}", record.id)
        }
        CompiledWorkflowRole::Helper { step_id } => {
            format!("Automation {} helper {step_id}", record.id)
        }
    }
}

fn summarize_value(value: &Value) -> String {
    let text = workflow_execute_summary(
        "automation",
        &WorkflowRuntimeRecord::try_from(value.clone()).unwrap_or_else(|_| {
            let mut fields = BTreeMap::new();
            fields.insert("output".into(), value.clone());
            WorkflowRuntimeRecord::new(fields)
        }),
    );
    if text.chars().count() > 240 {
        format!("{}...", text.chars().take(240).collect::<String>())
    } else {
        text
    }
}

fn runtime_summary(runtime: &AutomationRuntimeState) -> Value {
    json!({
        "status": runtime.status,
        "spec_hash": runtime.spec_hash.clone(),
        "compiled_revision": runtime.compiled_revision,
        "agentenv_workflow_count": runtime.agentenv_workflows.len(),
        "puffer_binding_count": runtime.puffer_bindings.len(),
        "last_error": runtime.last_error.clone(),
    })
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
    bail!("missing automation id")
}

fn optional_expected_revision(params: &Value) -> Result<Option<u64>> {
    match (
        params.get("expected_revision"),
        params.get("expectedRevision"),
    ) {
        (Some(_), Some(_)) => {
            bail!("accepts only one of expected_revision or expectedRevision")
        }
        (Some(value), None) | (None, Some(value)) => value
            .as_u64()
            .context("expected_revision must be an unsigned integer")
            .map(Some),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_workflow_backend_settings::save_workflow_backend_settings;
    use crate::daemon_workflow_backend_settings::test_support::{
        lock_secret_store, ScopedSecretStoreKey,
    };
    use crate::desktop_api_types::SaveWorkflowBackendSettingsParams;
    use puffer_automation::{
        automation_spec_hash, AgentEnvNodeRef, AutomationFlowSpec, AutomationLoopInput,
        AutomationReviewSpec, AutomationRunLocation, AutomationSource, AutomationSpec,
        AutomationTriggerSpec, AUTOMATION_SPEC_VERSION,
    };
    use puffer_config::{ensure_workspace_dirs, PufferConfig, WorkflowBackendMode};
    use puffer_core::{install_subscription_manager, subscription_manager};
    use puffer_subscriptions::{SubscriptionManager, SubscriptionManagerBuilder};
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    fn temp_paths(temp: &TempDir) -> ConfigPaths {
        let root = temp.path();
        ConfigPaths {
            workspace_root: root.join("workspace"),
            workspace_config_dir: root.join("workspace").join(".puffer"),
            user_config_dir: root.join("home").join(".puffer"),
            builtin_resources_dir: root.join("resources"),
        }
    }

    fn node(node_type: &str) -> AgentEnvNodeRef {
        AgentEnvNodeRef {
            node_type: node_type.to_string(),
            name: Some(node_type.to_string()),
            trusted: Some(true),
            config: BTreeMap::new(),
        }
    }

    fn puffer_connection_trigger() -> AutomationTriggerSpec {
        AutomationTriggerSpec::PufferConnection {
            id: "incoming".into(),
            connection_slug: "telegram-user".into(),
            connector_slug: Some("telegram-login".into()),
            filter: None,
            ignore_filters: Vec::new(),
            contact_ids: Vec::new(),
            summary: None,
        }
    }

    fn agentenv_trigger(node_type: &str) -> AutomationTriggerSpec {
        AutomationTriggerSpec::AgentEnvNode {
            id: "incoming".into(),
            node: node(node_type),
            summary: None,
        }
    }

    fn linear_spec(instructions: &str) -> AutomationSpec {
        AutomationSpec {
            spec_version: AUTOMATION_SPEC_VERSION,
            name: "Reply helper".into(),
            description: None,
            source: AutomationSource::Blank,
            instructions: instructions.into(),
            run_location: AutomationRunLocation::AgentEnvCloud,
            triggers: vec![agentenv_trigger("webhook")],
            flow: AutomationFlowSpec {
                steps: vec![AutomationStepSpec::AgentEnvNode {
                    id: "draft".into(),
                    node: node("llm"),
                    summary: None,
                }],
            },
            review: AutomationReviewSpec::default(),
        }
    }

    fn puffer_agent_spec() -> AutomationSpec {
        AutomationSpec {
            flow: AutomationFlowSpec {
                steps: vec![AutomationStepSpec::AgentEnvNode {
                    id: "agent".into(),
                    node: node("puffer_agent"),
                    summary: None,
                }],
            },
            ..linear_spec("Draft a reply.")
        }
    }

    fn agent_step_spec() -> AutomationSpec {
        let mut tool_node = node("puffer_connector_action");
        tool_node
            .config
            .insert("connector_slug".into(), json!("telegram-login"));
        tool_node
            .config
            .insert("connection_slug".into(), json!("telegram-user"));
        tool_node
            .config
            .insert("action".into(), json!("send_message"));
        AutomationSpec {
            flow: AutomationFlowSpec {
                steps: vec![AutomationStepSpec::Agent {
                    id: "agent".into(),
                    instructions: "Resolve the ticket until done.".into(),
                    mode: AutomationAgentMode::UntilDone,
                    max_iterations: Some(3),
                    tools: vec![AutomationAgentToolSpec {
                        id: "reply".into(),
                        node: tool_node,
                        summary: None,
                    }],
                    summary: None,
                }],
            },
            ..linear_spec("Resolve the ticket.")
        }
    }

    fn connector_trigger_spec(instructions: &str, connection_slug: &str) -> AutomationSpec {
        AutomationSpec {
            triggers: vec![AutomationTriggerSpec::PufferConnection {
                id: "incoming".into(),
                connection_slug: connection_slug.into(),
                connector_slug: Some("telegram-login".into()),
                filter: None,
                ignore_filters: Vec::new(),
                contact_ids: Vec::new(),
                summary: None,
            }],
            ..linear_spec(instructions)
        }
    }

    fn connector_action_spec(external_side_effect: bool) -> AutomationSpec {
        let mut config = BTreeMap::new();
        config.insert("connector_slug".into(), json!("demo-connector"));
        config.insert("connection_slug".into(), json!("demo-account"));
        config.insert("action".into(), json!("read_status"));
        config.insert("input".into(), json!({"query": "latest"}));
        config.insert("external_side_effect".into(), json!(external_side_effect));
        config.insert("draft_only".into(), json!(external_side_effect));
        AutomationSpec {
            flow: AutomationFlowSpec {
                steps: vec![AutomationStepSpec::AgentEnvNode {
                    id: "send".into(),
                    node: AgentEnvNodeRef {
                        node_type: "puffer_connector_action".into(),
                        name: Some("Send message".into()),
                        trusted: Some(true),
                        config,
                    },
                    summary: None,
                }],
            },
            ..linear_spec("Send a connector action.")
        }
    }

    fn connector_send_message_action_spec(gated: bool) -> AutomationSpec {
        let mut spec = connector_action_spec(gated);
        let AutomationStepSpec::AgentEnvNode { node, .. } = &mut spec.flow.steps[0] else {
            panic!("connector action step");
        };
        node.config.insert("action".into(), json!("send_message"));
        node.config.insert(
            "input".into(),
            json!({"chat_id": 42, "message": "hello from automation"}),
        );
        spec
    }

    #[derive(Default)]
    struct RecordingConnectorActionExecutor {
        calls: Mutex<Vec<(String, String, Value, Value)>>,
    }

    impl ConnectorActionExecutor for RecordingConnectorActionExecutor {
        fn run_connector_action(
            &self,
            connector_slug: &str,
            action: &str,
            input: Value,
            trigger: Value,
        ) -> Result<String> {
            self.calls.lock().unwrap().push((
                connector_slug.to_string(),
                action.to_string(),
                input,
                trigger,
            ));
            Ok(format!("called {connector_slug}.{action}"))
        }
    }

    fn loop_continuation_spec() -> AutomationSpec {
        AutomationSpec {
            flow: AutomationFlowSpec {
                steps: vec![
                    AutomationStepSpec::Loop {
                        id: "retry".into(),
                        loop_spec: AutomationLoopSpec::ForEach {
                            input: AutomationLoopInput::Trigger,
                            item_alias: "item".into(),
                            max_iterations: Some(2),
                        },
                        body: AutomationFlowSpec {
                            steps: vec![AutomationStepSpec::AgentEnvNode {
                                id: "attempt".into(),
                                node: node("attempt"),
                                summary: None,
                            }],
                        },
                        summary: None,
                    },
                    AutomationStepSpec::AgentEnvNode {
                        id: "after".into(),
                        node: node("after"),
                        summary: None,
                    },
                ],
            },
            triggers: vec![puffer_connection_trigger()],
            ..linear_spec("Try until complete.")
        }
    }

    fn foreach_loop_spec(max_iterations: Option<u32>) -> AutomationSpec {
        AutomationSpec {
            flow: AutomationFlowSpec {
                steps: vec![AutomationStepSpec::Loop {
                    id: "items".into(),
                    loop_spec: AutomationLoopSpec::ForEach {
                        input: AutomationLoopInput::StepOutput {
                            step_id: "root".into(),
                            path: Some("$.items".into()),
                        },
                        item_alias: "item".into(),
                        max_iterations,
                    },
                    body: AutomationFlowSpec {
                        steps: vec![AutomationStepSpec::AgentEnvNode {
                            id: "visit".into(),
                            node: node("transform_js"),
                            summary: None,
                        }],
                    },
                    summary: None,
                }],
            },
            triggers: vec![puffer_connection_trigger()],
            ..linear_spec("Visit items.")
        }
    }

    fn configure_runtime(paths: &ConfigPaths, api_url: String) {
        ensure_workspace_dirs(paths).expect("workspace dirs");
        let mut config = PufferConfig::default();
        save_workflow_backend_settings(
            paths,
            &mut config,
            SaveWorkflowBackendSettingsParams {
                mode: WorkflowBackendMode::AgentEnvCloud,
                api_url,
                ui_url: "http://localhost:5173".into(),
                workspace_id: "workspace-automation-test".into(),
                api_token: Some("runtime-token".into()),
                keep_token: false,
            },
        )
        .expect("save workflow backend settings");
    }

    struct TestSubscriptionManager {
        _runtime: tokio::runtime::Runtime,
        _tempdir: tempfile::TempDir,
        manager: Arc<SubscriptionManager>,
    }

    static TEST_SUBSCRIPTION_MANAGER: OnceLock<TestSubscriptionManager> = OnceLock::new();

    fn test_subscription_manager() -> Arc<SubscriptionManager> {
        if let Ok(manager) = subscription_manager() {
            return manager;
        }
        let state = TEST_SUBSCRIPTION_MANAGER.get_or_init(|| {
            let tempdir = tempfile::tempdir().unwrap();
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(1)
                .thread_name("puffer-automation-runtime-test")
                .build()
                .unwrap();
            let manager = Arc::new(
                SubscriptionManagerBuilder::new(tempdir.path().join("subscriptions.json"))
                    .build(runtime.handle().clone())
                    .unwrap(),
            );
            let _ = install_subscription_manager(manager.clone());
            TestSubscriptionManager {
                _runtime: runtime,
                _tempdir: tempdir,
                manager,
            }
        });
        subscription_manager().unwrap_or_else(|_| state.manager.clone())
    }

    fn unavailable_runtime_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind unused runtime port");
        let address = listener.local_addr().expect("unused runtime address");
        drop(listener);
        format!("http://{address}")
    }

    fn deployed_runtime(
        record: &AutomationRecord,
        workflows: Vec<CompiledAgentEnvWorkflow>,
    ) -> AutomationRuntimeState {
        AutomationRuntimeState {
            spec_hash: Some(automation_spec_hash(&record.spec).unwrap()),
            compiled_revision: Some(record.revision),
            status: AutomationRuntimeStatus::Deployed,
            agentenv_workflows: workflows,
            puffer_bindings: Vec::new(),
            last_error: None,
        }
    }

    fn root_workflow(id: &str) -> CompiledAgentEnvWorkflow {
        CompiledAgentEnvWorkflow {
            role: CompiledWorkflowRole::Root,
            workflow_id: Some(id.into()),
            definition_hash: None,
            deployed: true,
        }
    }

    fn loop_workflow(step_id: &str, id: &str) -> CompiledAgentEnvWorkflow {
        CompiledAgentEnvWorkflow {
            role: CompiledWorkflowRole::LoopBody {
                step_id: step_id.into(),
            },
            workflow_id: Some(id.into()),
            definition_hash: None,
            deployed: true,
        }
    }

    fn continuation_workflow(step_id: &str, id: &str) -> CompiledAgentEnvWorkflow {
        CompiledAgentEnvWorkflow {
            role: CompiledWorkflowRole::Continuation {
                step_id: step_id.into(),
            },
            workflow_id: Some(id.into()),
            definition_hash: None,
            deployed: true,
        }
    }

    struct MockRuntimeResponse {
        status: u16,
        body: Value,
    }

    impl MockRuntimeResponse {
        fn ok(body: Value) -> Self {
            Self { status: 200, body }
        }

        fn error(message: &str) -> Self {
            Self {
                status: 500,
                body: json!({ "error": { "message": message } }),
            }
        }

        fn not_found(message: &str) -> Self {
            Self {
                status: 404,
                body: json!({ "error": { "message": message } }),
            }
        }
    }

    fn spawn_runtime_server(
        responses: Vec<MockRuntimeResponse>,
    ) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock runtime");
        let address = listener.local_addr().expect("mock runtime address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept runtime request");
                let request = read_http_request(&mut stream);
                captured.lock().expect("requests lock").push(request);
                write_http_json(&mut stream, response.status, response.body);
            }
        });
        (format!("http://{address}"), requests, handle)
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set read timeout");
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_end = loop {
            let read = stream.read(&mut buffer).expect("read request");
            assert!(read > 0, "request ended before headers");
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let head = String::from_utf8(bytes[..header_end].to_vec()).expect("request head utf8");
        let content_length = content_length(&head);
        while bytes.len() < header_end + content_length {
            let read = stream.read(&mut buffer).expect("read request body");
            assert!(read > 0, "request ended before body");
            bytes.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8(bytes).expect("request utf8")
    }

    fn content_length(head: &str) -> usize {
        head.lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
            .unwrap_or(0)
    }

    fn write_http_json(stream: &mut TcpStream, status: u16, value: Value) {
        let body = value.to_string();
        let reason = if status == 200 {
            "OK"
        } else {
            "Internal Server Error"
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    }

    #[test]
    fn daemon_automation_runtime_compile_failure_writes_error_runtime_without_success_hash() {
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        store
            .create(
                "reply-helper",
                loop_continuation_spec(),
                AutomationStatus::Enabled,
            )
            .unwrap();

        let error =
            compile_and_deploy_with_store(&paths, &store, "reply-helper", Some(1)).unwrap_err();

        assert!(
            format!("{error:#}").contains("loop continuation compilation is not implemented yet")
        );
        let record = store.get("reply-helper").unwrap();
        assert_eq!(record.runtime.status, AutomationRuntimeStatus::Error);
        assert_eq!(record.runtime.spec_hash, None);
        assert_eq!(record.runtime.compiled_revision, None);
        assert!(record
            .runtime
            .last_error
            .as_deref()
            .unwrap()
            .contains("loop continuation compilation is not implemented yet"));
    }

    #[test]
    fn daemon_automation_runtime_agentenv_unavailable_writes_error_runtime() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, unavailable_runtime_url());
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        store
            .create(
                "reply-helper",
                linear_spec("Draft a reply."),
                AutomationStatus::Enabled,
            )
            .unwrap();

        let error =
            compile_and_deploy_with_store(&paths, &store, "reply-helper", Some(1)).unwrap_err();

        assert!(format!("{error:#}").contains("create workflow artifact"));
        let record = store.get("reply-helper").unwrap();
        assert_eq!(record.runtime.status, AutomationRuntimeStatus::Error);
        assert!(record
            .runtime
            .last_error
            .as_deref()
            .unwrap()
            .contains("create workflow artifact"));
    }

    #[test]
    fn daemon_automation_runtime_public_error_hides_runtime_url() {
        let error = anyhow::anyhow!(
            "create workflow artifact: runtime unreachable: error sending request for url (http://127.0.0.1:3000/v1/workflows)"
        );

        let message = public_automation_error_message(&error);

        assert_eq!(
            message,
            "Automation runtime is unreachable. Check Docker or the selected runtime settings, then try again."
        );
        assert!(!message.contains("127.0.0.1"));
        assert!(!message.contains("/v1/"));
        assert!(!message.contains("workflow artifact"));
    }

    #[test]
    fn daemon_automation_runtime_public_error_fallback_hides_unknown_detail() {
        let error = anyhow::anyhow!(
            "unexpected runtime failure at /app/node_modules/pkg/index.js:42 with secret stack frame"
        );

        let message = public_automation_error_message(&error);

        assert_eq!(
            message,
            "Automation runtime could not prepare this automation. Check the selected run location and try again."
        );
        assert!(!message.contains("node_modules"));
        assert!(!message.contains("secret stack frame"));
    }

    #[test]
    fn daemon_automation_runtime_public_error_hides_database_diagnostics() {
        let error = anyhow::anyhow!(
            "local workflow runtime is incompatible_runtime: Database migration failed: error: could not open file \"global/pg_filenode.map\": No such file or directory at Parser.parseErrorMessage (/app/node_modules/pg-protocol/dist/parser.js:285:98)"
        );

        let message = public_automation_error_message(&error);

        assert_eq!(
            message,
            "The Puffer-managed local automation runtime database could not be prepared. Puffer needs to rebuild the local runtime data before automations can run."
        );
        assert!(!message.contains("global/pg_filenode.map"));
        assert!(!message.contains("Parser.parseErrorMessage"));
        assert!(!message.contains("node_modules"));
    }

    #[test]
    fn daemon_automation_runtime_public_error_reports_missing_node_configuration() {
        let error = anyhow::anyhow!(
            "create workflow artifact: workflow runtime error (400): This workflow can't be created yet. Complete the required configuration first."
        );

        let message = public_automation_error_message(&error);

        assert_eq!(
            message,
            "Automation runtime rejected this automation because one trigger or action is missing required configuration. Check the automation's trigger and tool settings, then try again."
        );
        assert!(!message.contains("workflow artifact"));
    }

    #[test]
    fn daemon_automation_runtime_public_error_reports_missing_agent_provider_context() {
        let error = anyhow::anyhow!(
            "Automation uses puffer_agent but daemon resolver is missing provider/model context"
        );

        let message = public_automation_error_message(&error);

        assert_eq!(
            message,
            "Automation cannot run the Puffer agent because provider/model context is unavailable. Reload the daemon runtime inputs, then try again."
        );
        assert!(!message.contains("daemon resolver"));
    }

    #[test]
    fn daemon_automation_runtime_public_error_reports_missing_agent_resources() {
        let error = anyhow::anyhow!(
            "Automation uses puffer_agent but daemon resolver is missing loaded Puffer resources"
        );

        let message = public_automation_error_message(&error);

        assert_eq!(
            message,
            "Automation cannot run the Puffer agent because Puffer resources are not loaded. Reload the daemon runtime inputs, then try again."
        );
        assert!(!message.contains("daemon resolver"));
    }

    #[test]
    fn daemon_automation_runtime_public_error_reports_missing_provider_credentials() {
        let error =
            anyhow::anyhow!("connect selected provider `OpenAI` before activating automation");

        let message = public_automation_error_message(&error);

        assert_eq!(
            message,
            "Connect credentials for the selected provider before activating this automation."
        );
    }

    #[test]
    fn daemon_automation_runtime_puffer_agent_deploys_without_managed_agent_nodes() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) = spawn_runtime_server(vec![
            MockRuntimeResponse::ok(json!({ "data": { "id": "wf-agent" } })),
            MockRuntimeResponse::ok(json!({ "data": { "id": "wf-agent" } })),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        store
            .create(
                "agent-helper",
                puffer_agent_spec(),
                AutomationStatus::Paused,
            )
            .unwrap();
        let record =
            compile_and_deploy_with_store(&paths, &store, "agent-helper", Some(1)).unwrap();
        handle.join().expect("mock runtime joined");

        assert_eq!(record.runtime.status, AutomationRuntimeStatus::Deployed);
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert!(captured[0].starts_with("POST /v1/workflows "));
        assert!(captured[1].starts_with("POST /v1/workflows/wf-agent/deploy "));
        for request in captured.iter() {
            assert!(!request.contains("managed_agent_create"));
            assert!(!request.contains("managed_agent_call"));
        }
    }

    #[test]
    fn daemon_automation_runtime_agent_step_deploys_without_managed_agent_nodes() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) = spawn_runtime_server(vec![
            MockRuntimeResponse::ok(json!({ "data": { "id": "wf-agent" } })),
            MockRuntimeResponse::ok(json!({ "data": { "id": "wf-agent" } })),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        store
            .create("agent-helper", agent_step_spec(), AutomationStatus::Paused)
            .unwrap();
        let record =
            compile_and_deploy_with_store(&paths, &store, "agent-helper", Some(1)).unwrap();
        handle.join().expect("mock runtime joined");

        assert_eq!(record.runtime.status, AutomationRuntimeStatus::Deployed);
        let captured = requests.lock().unwrap();
        for request in captured.iter() {
            assert!(!request.contains("managed_agent_create"));
            assert!(!request.contains("managed_agent_call"));
            // The iterative agent and its tools are Puffer-owned runtime
            // boundaries; none of them are emitted as AgentEnv nodes.
            assert!(!request.contains("puffer_agent"));
            assert!(!request.contains("puffer_connector_action"));
        }
    }

    #[test]
    fn agent_step_decision_parses_tool_calls() {
        let decision = parse_puffer_agent_decision(
            r#"{"done": false, "reason": "need lookup",
                "tool_calls": [{"tool_id": "reply", "input": {"text": "hi"}}]}"#,
        );

        assert!(!decision.done);
        assert_eq!(decision.reason, "need lookup");
        assert_eq!(decision.tool_calls.len(), 1);
        assert_eq!(decision.tool_calls[0].tool_id, "reply");
        assert_eq!(decision.tool_calls[0].input, json!({ "text": "hi" }));
    }

    #[test]
    fn agent_step_decision_without_tool_calls_is_empty() {
        let decision = parse_puffer_agent_decision(r#"{"done": true, "reason": "resolved"}"#);

        assert!(decision.done);
        assert!(decision.tool_calls.is_empty());
    }

    #[test]
    fn daemon_automation_runtime_create_workflow_failure_writes_error_runtime() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) =
            spawn_runtime_server(vec![MockRuntimeResponse::error("create failed")]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        store
            .create(
                "reply-helper",
                linear_spec("Draft a reply."),
                AutomationStatus::Enabled,
            )
            .unwrap();

        let error =
            compile_and_deploy_with_store(&paths, &store, "reply-helper", Some(1)).unwrap_err();
        handle.join().expect("mock runtime joined");

        let message = format!("{error:#}");
        assert!(message.contains("create workflow artifact"));
        assert!(message.contains("create failed"));
        assert_eq!(requests.lock().unwrap().len(), 1);
        let record = store.get("reply-helper").unwrap();
        assert_eq!(record.runtime.status, AutomationRuntimeStatus::Error);
        assert!(record
            .runtime
            .last_error
            .as_deref()
            .unwrap()
            .contains("create failed"));
    }

    #[test]
    fn daemon_automation_runtime_deploy_workflow_failure_writes_error_runtime() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) = spawn_runtime_server(vec![
            MockRuntimeResponse::ok(json!({ "data": { "id": "wf-deploy-fail" } })),
            MockRuntimeResponse::error("deploy failed"),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        store
            .create(
                "reply-helper",
                linear_spec("Draft a reply."),
                AutomationStatus::Enabled,
            )
            .unwrap();

        let error =
            compile_and_deploy_with_store(&paths, &store, "reply-helper", Some(1)).unwrap_err();
        handle.join().expect("mock runtime joined");

        let message = format!("{error:#}");
        assert!(message.contains("deploy workflow `wf-deploy-fail`"));
        assert!(message.contains("deploy failed"));
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert!(captured[0].starts_with("POST /v1/workflows "));
        assert!(captured[1].starts_with("POST /v1/workflows/wf-deploy-fail/deploy "));
        let record = store.get("reply-helper").unwrap();
        assert_eq!(record.runtime.status, AutomationRuntimeStatus::Error);
        assert!(record
            .runtime
            .last_error
            .as_deref()
            .unwrap()
            .contains("deploy failed"));
    }

    #[test]
    fn daemon_automation_runtime_compile_deploy_enables_after_success() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) = spawn_runtime_server(vec![
            MockRuntimeResponse::ok(json!({ "data": { "id": "wf-active" } })),
            MockRuntimeResponse::ok(json!({ "data": { "id": "wf-active" } })),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        store
            .create(
                "reply-helper",
                linear_spec("Draft a reply."),
                AutomationStatus::Paused,
            )
            .unwrap();

        let record =
            compile_and_deploy_with_store(&paths, &store, "reply-helper", Some(1)).unwrap();
        handle.join().expect("mock runtime joined");

        assert_eq!(record.status, AutomationStatus::Enabled);
        assert_eq!(record.runtime.status, AutomationRuntimeStatus::Deployed);
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert!(captured[0].starts_with("POST /v1/workflows "));
        assert!(captured[1].starts_with("POST /v1/workflows/wf-active/deploy "));
    }

    #[test]
    fn daemon_automation_runtime_preview_sync_does_not_deploy_live_bindings() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let manager = test_subscription_manager();
        let slug = "automation-preview-helper-incoming";
        let _ = manager.store().delete(slug);
        let (api_url, requests, handle) =
            spawn_runtime_server(vec![MockRuntimeResponse::ok(json!({
                "data": { "id": "wf-preview-sync" }
            }))]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        store
            .create(
                "preview-helper",
                connector_trigger_spec("Draft a reply.", "telegram-user"),
                AutomationStatus::Enabled,
            )
            .unwrap();

        let record = sync_preview_with_store(&paths, &store, "preview-helper", Some(1)).unwrap();
        handle.join().expect("mock runtime joined");

        assert_eq!(record.runtime.status, AutomationRuntimeStatus::DraftSynced);
        assert!(record.runtime.puffer_bindings.is_empty());
        assert!(manager.store().get(slug).is_none());
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].starts_with("POST /v1/workflows "));
        assert!(!captured[0].contains("/deploy "));
    }

    #[test]
    fn daemon_automation_runtime_preview_sync_keeps_current_live_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let record = store
            .create(
                "preview-live-helper",
                connector_trigger_spec("Draft a reply.", "telegram-user"),
                AutomationStatus::Enabled,
            )
            .unwrap();
        store
            .replace_runtime(
                "preview-live-helper",
                record.revision,
                AutomationRuntimeState {
                    spec_hash: Some(automation_spec_hash(&record.spec).unwrap()),
                    compiled_revision: Some(record.revision),
                    status: AutomationRuntimeStatus::Deployed,
                    agentenv_workflows: vec![root_workflow("wf-live")],
                    puffer_bindings: vec![CompiledPufferBinding {
                        trigger_id: "incoming".into(),
                        binding_slug: "automation-preview-live-helper-incoming".into(),
                    }],
                    last_error: None,
                },
            )
            .unwrap();

        let synced =
            sync_preview_with_store(&paths, &store, "preview-live-helper", Some(record.revision))
                .unwrap();

        assert_eq!(synced.runtime.status, AutomationRuntimeStatus::Deployed);
        assert_eq!(synced.runtime.puffer_bindings.len(), 1);
        assert_eq!(
            synced.runtime.puffer_bindings[0].binding_slug,
            "automation-preview-live-helper-incoming"
        );
    }

    #[test]
    fn daemon_automation_runtime_preview_execute_failure_keeps_deployed_runtime() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) =
            spawn_runtime_server(vec![MockRuntimeResponse::error("execute failed")]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let record = store
            .create(
                "reply-helper",
                linear_spec("Draft a reply."),
                AutomationStatus::Enabled,
            )
            .unwrap();
        store
            .replace_runtime(
                "reply-helper",
                record.revision,
                deployed_runtime(&record, vec![root_workflow("wf-root")]),
            )
            .unwrap();

        let error = run_automation_preview_with_store(
            &paths,
            &store,
            "reply-helper",
            json!({ "text": "hello" }),
        )
        .unwrap_err();
        handle.join().expect("mock runtime joined");

        let message = format!("{error:#}");
        assert!(message.contains("execute in-memory workflow"));
        assert!(message.contains("execute failed"));
        assert_eq!(requests.lock().unwrap().len(), 1);
        let record = store.get("reply-helper").unwrap();
        assert_eq!(record.runtime.status, AutomationRuntimeStatus::Deployed);
        assert_eq!(record.runtime.last_error, None);
    }

    #[test]
    fn daemon_automation_runtime_preview_falls_back_when_in_memory_endpoint_is_missing() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) = spawn_runtime_server(vec![
            MockRuntimeResponse::not_found("not found"),
            MockRuntimeResponse::ok(json!({
                "data": { "status": "completed", "output": { "fallback": true } }
            })),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let mut spec = connector_trigger_spec("Draft a reply.", "telegram-user");
        spec.run_location = AutomationRunLocation::AgentEnvCloud;
        let record = store
            .create("reply-helper", spec, AutomationStatus::Enabled)
            .unwrap();
        store
            .replace_runtime(
                "reply-helper",
                record.revision,
                deployed_runtime(&record, vec![root_workflow("wf-root")]),
            )
            .unwrap();

        let output = run_automation_preview_with_store(
            &paths,
            &store,
            "reply-helper",
            json!({ "text": "hello" }),
        )
        .unwrap();
        handle.join().expect("mock runtime joined");

        assert_eq!(output.result["status"], "completed");
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert!(captured[0].starts_with("POST /v1/workflows/execute-in-memory "));
        assert!(captured[1].starts_with("POST /v1/workflows/wf-root/execute "));
    }

    #[test]
    fn daemon_automation_runtime_preview_run_uses_in_memory_execution() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) =
            spawn_runtime_server(vec![MockRuntimeResponse::ok(json!({
                "data": { "status": "completed", "output": { "ok": true } }
            }))]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let mut spec = connector_trigger_spec("Draft a reply.", "telegram-user");
        spec.run_location = AutomationRunLocation::AgentEnvCloud;
        let record = store
            .create("reply-helper", spec, AutomationStatus::Enabled)
            .unwrap();
        store
            .replace_runtime(
                "reply-helper",
                record.revision,
                AutomationRuntimeState {
                    spec_hash: Some(automation_spec_hash(&record.spec).unwrap()),
                    compiled_revision: Some(record.revision),
                    status: AutomationRuntimeStatus::DraftSynced,
                    agentenv_workflows: vec![root_workflow("wf-draft")],
                    puffer_bindings: Vec::new(),
                    last_error: None,
                },
            )
            .unwrap();

        run_automation_preview_with_store(
            &paths,
            &store,
            "reply-helper",
            json!({ "text": "hello" }),
        )
        .unwrap();
        handle.join().expect("mock runtime joined");

        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].starts_with("POST /v1/workflows/execute-in-memory "));
        assert!(!captured[0].contains("triggerNodeId"));
        assert!(captured[0].contains(r#""trigger":{"text":"hello"}"#));
    }

    #[test]
    fn daemon_automation_runtime_save_spec_change_marks_runtime_stale() {
        let temp = tempfile::tempdir().unwrap();
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let record = store
            .create(
                "reply-helper",
                linear_spec("Draft a reply."),
                AutomationStatus::Enabled,
            )
            .unwrap();
        let spec_hash = automation_spec_hash(&record.spec).unwrap();
        store
            .replace_runtime(
                "reply-helper",
                record.revision,
                AutomationRuntimeState {
                    spec_hash: Some(spec_hash),
                    compiled_revision: Some(record.revision),
                    status: AutomationRuntimeStatus::Deployed,
                    agentenv_workflows: vec![CompiledAgentEnvWorkflow {
                        role: CompiledWorkflowRole::Root,
                        workflow_id: Some("automation-reply-helper-root".into()),
                        definition_hash: None,
                        deployed: true,
                    }],
                    puffer_bindings: Vec::new(),
                    last_error: None,
                },
            )
            .unwrap();

        let updated = store
            .save_spec(
                "reply-helper",
                record.revision,
                linear_spec("Draft a reply with context."),
            )
            .unwrap();

        assert_eq!(updated.revision, record.revision + 1);
        assert_eq!(updated.runtime.status, AutomationRuntimeStatus::Stale);
        assert!(updated.runtime.agentenv_workflows.is_empty());
        assert!(updated.runtime.puffer_bindings.is_empty());
        assert_eq!(updated.runtime.last_error, None);
    }

    #[test]
    fn daemon_automation_runtime_stale_spec_preview_refuses_remote_sync() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) = spawn_runtime_server(vec![]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let record = store
            .create(
                "reply-helper",
                linear_spec("Draft a reply."),
                AutomationStatus::Enabled,
            )
            .unwrap();
        store
            .replace_runtime(
                "reply-helper",
                record.revision,
                deployed_runtime(&record, vec![root_workflow("wf-old")]),
            )
            .unwrap();
        let updated = store
            .save_spec(
                "reply-helper",
                record.revision,
                linear_spec("Draft a reply with context."),
            )
            .unwrap();

        let error = run_automation_preview_with_store(
            &paths,
            &store,
            "reply-helper",
            json!({ "text": "hello" }),
        )
        .unwrap_err();
        handle.join().expect("mock runtime joined");

        assert!(format!("{error:#}").contains("deploy before running a test preview"));
        let record = store.get("reply-helper").unwrap();
        assert_eq!(record.revision, updated.revision);
        assert_eq!(record.runtime.status, AutomationRuntimeStatus::Stale);
        let captured = requests.lock().unwrap();
        assert!(captured.is_empty());
    }

    #[test]
    fn daemon_automation_runtime_preview_gated_terminal_non_send_action_returns_awaiting_approval()
    {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) =
            spawn_runtime_server(vec![MockRuntimeResponse::ok(json!({
                "data": { "root": "ok" }
            }))]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let record = store
            .create(
                "reply-helper",
                connector_action_spec(true),
                AutomationStatus::Enabled,
            )
            .unwrap();
        store
            .replace_runtime(
                "reply-helper",
                record.revision,
                deployed_runtime(&record, vec![root_workflow("wf-root")]),
            )
            .unwrap();

        let output = run_automation_preview_with_store(
            &paths,
            &store,
            "reply-helper",
            json!({ "text": "hello" }),
        )
        .unwrap();
        handle.join().expect("mock runtime joined");

        assert_eq!(output.status, "awaiting_approval");
        assert_eq!(output.result["kind"], "connector_action_draft");
        assert_eq!(output.result["action"], "read_status");
        assert_eq!(output.result["approval_kind"], "exact_action");
        assert_eq!(output.result["message_editable"], false);
        assert_eq!(requests.lock().unwrap().len(), 1);

        let store: Value = serde_json::from_str(
            &std::fs::read_to_string(paths.user_config_dir.join("outbound_actions.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(store["actions"].as_array().unwrap().len(), 1);
        assert_eq!(store["actions"][0]["action"], "read_status");
        assert_eq!(
            store["actions"][0]["input"]["__automation"]["automation_id"],
            "reply-helper"
        );
    }

    #[test]
    fn daemon_automation_runtime_connector_action_calls_executor() {
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        let record = AutomationRecord {
            id: "reply-helper".into(),
            status: AutomationStatus::Enabled,
            revision: 1,
            spec: connector_action_spec(false),
            runtime: AutomationRuntimeState::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        let executor = RecordingConnectorActionExecutor::default();

        let AutomationStepSpec::AgentEnvNode { id, node, .. } = &record.spec.flow.steps[0] else {
            panic!("connector action step");
        };
        let output = execute_puffer_connector_action_step(
            &record,
            id,
            node,
            &json!({"text": "trigger text"}),
            &json!({"agent": "ok"}),
            &json!({"previous": "ok"}),
            AutomationConnectorActionRunContext {
                paths: &paths,
                run_id: "run-1",
            },
            ConnectorActionPosition::TopLevelTerminal,
            Some(&executor),
        )
        .unwrap();

        assert_eq!(output["kind"], "connector_action_result");
        assert_eq!(output["summary"], "called demo-connector.read_status");
        let calls = executor.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "demo-connector");
        assert_eq!(calls[0].1, "read_status");
        assert_eq!(calls[0].2["connection_slug"], "demo-account");
        assert_eq!(calls[0].2["query"], "latest");
        assert_eq!(calls[0].2["previous_output"]["previous"], "ok");
        assert_eq!(calls[0].3["connection_id"], "demo-account");
        assert_eq!(calls[0].3["payload"]["automation_id"], "reply-helper");
    }

    #[test]
    fn daemon_automation_runtime_gated_terminal_send_message_writes_draft_without_executor() {
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        let record = AutomationRecord {
            id: "reply-helper".into(),
            status: AutomationStatus::Enabled,
            revision: 1,
            spec: connector_send_message_action_spec(true),
            runtime: AutomationRuntimeState::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        let executor = RecordingConnectorActionExecutor::default();

        let AutomationStepSpec::AgentEnvNode { id, node, .. } = &record.spec.flow.steps[0] else {
            panic!("connector action step");
        };
        let output = execute_puffer_connector_action_step(
            &record,
            id,
            node,
            &json!({"text": "trigger text"}),
            &json!({"agent": "ok"}),
            &json!({"previous": "ok"}),
            AutomationConnectorActionRunContext {
                paths: &paths,
                run_id: "run-42",
            },
            ConnectorActionPosition::TopLevelTerminal,
            Some(&executor),
        )
        .unwrap();

        assert_eq!(output["kind"], "connector_action_draft");
        assert_eq!(output["status"], "draft_ready");
        assert_eq!(output["step_id"], "send");
        assert_eq!(output["recipient_stable_id"], "42");
        assert_eq!(output["message"], "hello from automation");
        assert!(executor.calls.lock().unwrap().is_empty());

        let store: Value = serde_json::from_str(
            &std::fs::read_to_string(paths.user_config_dir.join("outbound_actions.json")).unwrap(),
        )
        .unwrap();
        let draft = &store["actions"][0];
        assert_eq!(draft["status"], "draft_ready");
        assert_eq!(draft["action"], "send_message");
        assert_eq!(draft["origin"]["session_id"], "automation:reply-helper");
        assert_eq!(draft["origin"]["turn_id"], "run-42");
        assert_eq!(
            draft["input"]["__automation"]["automation_id"],
            "reply-helper"
        );
        assert_eq!(
            draft["input"]["__automation"]["automation_run_id"],
            "run-42"
        );
        assert_eq!(draft["input"]["__automation"]["step_id"], "send");
        assert_eq!(draft["input"]["chat_id"], 42);
    }

    #[test]
    fn daemon_automation_runtime_preview_gated_terminal_send_message_returns_awaiting_approval() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) =
            spawn_runtime_server(vec![MockRuntimeResponse::ok(json!({
                "data": { "root": "ok" }
            }))]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let record = store
            .create(
                "reply-helper",
                connector_send_message_action_spec(true),
                AutomationStatus::Enabled,
            )
            .unwrap();
        store
            .replace_runtime(
                "reply-helper",
                record.revision,
                deployed_runtime(&record, vec![root_workflow("wf-root")]),
            )
            .unwrap();

        let output = run_automation_preview_with_store(
            &paths,
            &store,
            "reply-helper",
            json!({ "text": "hello" }),
        )
        .unwrap();
        handle.join().expect("mock runtime joined");

        assert_eq!(output.status, "awaiting_approval");
        assert_eq!(output.approval.unwrap().status, "draft_ready");
        assert_eq!(output.result["kind"], "connector_action_draft");
        assert_eq!(output.result["action"], "send_message");
        assert_eq!(output.result["message"], "hello from automation");
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].starts_with("POST /v1/workflows/execute-in-memory "));

        let store: Value = serde_json::from_str(
            &std::fs::read_to_string(paths.user_config_dir.join("outbound_actions.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(store["actions"].as_array().unwrap().len(), 1);
        assert_eq!(
            store["actions"][0]["input"]["__automation"]["automation_id"],
            "reply-helper"
        );
        assert!(
            store["actions"][0]["input"]["__automation"]["automation_run_id"]
                .as_str()
                .unwrap()
                .starts_with("preview-reply-helper-")
        );
    }

    #[test]
    fn daemon_automation_runtime_preview_gated_midflow_suspends_with_resume_context() {
        // A top-level gated action followed by another step suspends: it drafts,
        // returns awaiting_approval, and persists a suspension recording where to
        // resume (the step after the gated action) so approval can continue.
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) =
            spawn_runtime_server(vec![MockRuntimeResponse::ok(json!({
                "data": { "root": "ok" }
            }))]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let mut spec = connector_send_message_action_spec(true);
        spec.flow.steps.push(AutomationStepSpec::AgentEnvNode {
            id: "after".into(),
            node: node("transform_js"),
            summary: None,
        });
        let record = store
            .create("reply-helper", spec, AutomationStatus::Enabled)
            .unwrap();
        store
            .replace_runtime(
                "reply-helper",
                record.revision,
                deployed_runtime(&record, vec![root_workflow("wf-root")]),
            )
            .unwrap();

        let output = run_automation_preview_with_store(
            &paths,
            &store,
            "reply-helper",
            json!({ "text": "hello" }),
        )
        .unwrap();
        handle.join().expect("mock runtime joined");

        assert_eq!(output.status, "awaiting_approval");
        assert_eq!(output.result["kind"], "connector_action_draft");

        // The suspension records the resume point and the draft that unblocks it.
        let suspensions: Value = serde_json::from_str(
            &std::fs::read_to_string(paths.user_config_dir.join("automation_suspensions.json"))
                .unwrap(),
        )
        .unwrap();
        let entries = suspensions["suspensions"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["step_id"], "send");
        assert_eq!(entries[0]["resume_step_index"], 1);
        assert_eq!(entries[0]["automation_id"], "reply-helper");
        assert_eq!(entries[0]["draft_id"], output.result["draft_id"]);
        assert!(entries[0]["run_id"]
            .as_str()
            .unwrap()
            .starts_with("preview-reply-helper-"));
    }

    #[test]
    fn daemon_automation_runtime_gated_send_with_continuation_is_supported() {
        // A top-level gated action followed by more steps now suspends and
        // resumes on approval, so it must pass validation (no longer rejected).
        let mut spec = connector_send_message_action_spec(true);
        spec.flow.steps.push(AutomationStepSpec::AgentEnvNode {
            id: "after".into(),
            node: node("transform_js"),
            summary: None,
        });
        let record = AutomationRecord {
            id: "reply-helper".into(),
            status: AutomationStatus::Enabled,
            revision: 1,
            spec,
            runtime: AutomationRuntimeState::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };

        assert!(validate_automation_gated_connector_actions(&record).is_ok());
    }

    #[test]
    fn daemon_automation_runtime_loop_body_gated_send_is_rejected() {
        let mut send_spec = connector_send_message_action_spec(true);
        let send_step = send_spec.flow.steps.remove(0);
        let spec = AutomationSpec {
            flow: AutomationFlowSpec {
                steps: vec![AutomationStepSpec::Loop {
                    id: "items".into(),
                    loop_spec: AutomationLoopSpec::ForEach {
                        input: AutomationLoopInput::Static { value: json!([1]) },
                        item_alias: "item".into(),
                        max_iterations: None,
                    },
                    body: AutomationFlowSpec {
                        steps: vec![send_step],
                    },
                    summary: None,
                }],
            },
            ..linear_spec("Send from a loop.")
        };
        let record = AutomationRecord {
            id: "reply-helper".into(),
            status: AutomationStatus::Enabled,
            revision: 1,
            spec,
            runtime: AutomationRuntimeState::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };

        let error = ensure_live_automation_can_run(&record).unwrap_err();

        assert!(
            format!("{error:#}").contains("loop-body gated connector actions are not supported")
        );
    }

    #[test]
    fn daemon_automation_runtime_loop_body_connector_action_calls_executor() {
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        let record = AutomationRecord {
            id: "reply-helper".into(),
            status: AutomationStatus::Enabled,
            revision: 1,
            spec: connector_action_spec(false),
            runtime: AutomationRuntimeState::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        let executor = RecordingConnectorActionExecutor::default();

        let output = execute_loop_body_connector_actions(
            &record,
            &record.spec.flow,
            &json!({"text": "trigger text"}),
            &json!({"root": "ok"}),
            &json!({"iteration": "ok"}),
            AutomationConnectorActionRunContext {
                paths: &paths,
                run_id: "run-1",
            },
            Some(&executor),
        )
        .unwrap();

        assert_eq!(output["kind"], "connector_action_result");
        assert_eq!(output["summary"], "called demo-connector.read_status");
        assert_eq!(output["previous_output"]["iteration"], "ok");
        let calls = executor.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "demo-connector");
        assert_eq!(calls[0].1, "read_status");
        assert_eq!(calls[0].2["previous_output"]["iteration"], "ok");
    }

    #[test]
    fn daemon_automation_runtime_connector_action_runs_before_continuation() {
        let (api_url, requests, handle) =
            spawn_runtime_server(vec![MockRuntimeResponse::ok(json!({
                "data": { "continued": true }
            }))]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let config = load_config(&paths).unwrap();
        let client = crate::daemon_workflow_runtime::workflow_runtime_client_for_mode(
            &paths,
            &config,
            WorkflowBackendMode::AgentEnvCloud,
        )
        .unwrap();
        let mut spec = connector_action_spec(false);
        spec.flow.steps.push(AutomationStepSpec::AgentEnvNode {
            id: "after".into(),
            node: node("transform_js"),
            summary: None,
        });
        let mut record = AutomationRecord {
            id: "reply-helper".into(),
            status: AutomationStatus::Enabled,
            revision: 1,
            spec,
            runtime: AutomationRuntimeState::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        record.runtime = deployed_runtime(
            &record,
            vec![
                root_workflow("wf-root"),
                continuation_workflow("send", "wf-after-send"),
            ],
        );
        let executor = RecordingConnectorActionExecutor::default();

        let output = execute_ordered_flow_after_root(
            &client,
            &record,
            &json!({"text": "trigger text"}),
            &json!({"root": "ok"}),
            AutomationConnectorActionRunContext {
                paths: &paths,
                run_id: "run-1",
            },
            Some(&executor),
            None,
            None,
        )
        .unwrap();
        handle.join().expect("mock runtime joined");

        assert_eq!(output["continued"], true);
        let calls = executor.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        drop(calls);
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].starts_with("POST /v1/workflows/wf-after-send/execute "));
        assert!(captured[0].contains(r#""previous_output":"#));
        assert!(captured[0].contains(r#""action":"read_status""#));
        assert!(captured[0].contains(r#""kind":"connector_action_result""#));
    }

    #[test]
    fn daemon_automation_runtime_stale_enabled_save_removes_old_binding() {
        let manager = test_subscription_manager();
        let slug = "automation-stale-binding-helper-incoming";
        let _ = manager.store().delete(slug);
        let temp = tempfile::tempdir().unwrap();
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let record = store
            .create(
                "stale-binding-helper",
                connector_trigger_spec("Draft a reply.", "telegram-old"),
                AutomationStatus::Enabled,
            )
            .unwrap();
        store
            .replace_runtime(
                "stale-binding-helper",
                record.revision,
                AutomationRuntimeState {
                    spec_hash: Some(automation_spec_hash(&record.spec).unwrap()),
                    compiled_revision: Some(record.revision),
                    status: AutomationRuntimeStatus::Deployed,
                    agentenv_workflows: Vec::new(),
                    puffer_bindings: vec![CompiledPufferBinding {
                        trigger_id: "incoming".into(),
                        binding_slug: slug.into(),
                    }],
                    last_error: None,
                },
            )
            .unwrap();
        manager
            .store()
            .upsert(WorkflowBindingSpec {
                slug: slug.into(),
                description: "old generated automation binding".into(),
                connection_slug: "telegram-old".into(),
                connector_slug: Some("telegram-login".into()),
                status: WorkflowBindingStatus::Enabled,
                filter: None,
                ignore_filters: Vec::new(),
                contact_ids: Vec::new(),
                classify_prompt: None,
                classify_model: None,
                action: ActionSpec::RunAutomation {
                    automation_id: "stale-binding-helper".into(),
                },
                created_at_ms: puffer_subscriptions::now_ms(),
            })
            .unwrap();
        let previous = store.get("stale-binding-helper").unwrap();
        let updated = store
            .save_spec(
                "stale-binding-helper",
                previous.revision,
                connector_trigger_spec("Draft a reply.", "telegram-new"),
            )
            .unwrap();

        sync_automation_bindings_after_save(Some(&previous), &updated).unwrap();

        assert_eq!(updated.status, AutomationStatus::Enabled);
        assert_eq!(updated.runtime.status, AutomationRuntimeStatus::Stale);
        assert!(manager.store().get(slug).is_none());
    }

    #[test]
    fn daemon_automation_runtime_agentenv_trigger_sets_trigger_node_id() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) =
            spawn_runtime_server(vec![MockRuntimeResponse::ok(json!({
                "data": { "status": "completed" }
            }))]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let spec = AutomationSpec {
            triggers: vec![agentenv_trigger("webhook")],
            flow: AutomationFlowSpec {
                steps: vec![AutomationStepSpec::AgentEnvNode {
                    id: "transform".into(),
                    node: node("transform_js"),
                    summary: None,
                }],
            },
            ..linear_spec("Transform webhook input.")
        };
        let record = store
            .create("webhook-helper", spec, AutomationStatus::Enabled)
            .unwrap();
        store
            .replace_runtime(
                "webhook-helper",
                record.revision,
                deployed_runtime(&record, vec![root_workflow("wf-webhook")]),
            )
            .unwrap();

        run_automation_preview_with_store(
            &paths,
            &store,
            "webhook-helper",
            json!({ "text": "hello" }),
        )
        .unwrap();
        handle.join().expect("mock runtime joined");

        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].contains(r#""triggerNodeId":"incoming""#));
        assert!(captured[0].contains(r#""trigger":{"text":"hello"}"#));
    }

    #[test]
    fn daemon_automation_runtime_foreach_loop_respects_max_iterations() {
        let _guard = lock_secret_store();
        let _secret_store_key = ScopedSecretStoreKey::set();
        let (api_url, requests, handle) = spawn_runtime_server(vec![
            MockRuntimeResponse::ok(json!({ "data": { "items": [1, 2, 3] } })),
            MockRuntimeResponse::ok(json!({ "data": { "seen": 1 } })),
            MockRuntimeResponse::ok(json!({ "data": { "seen": 2 } })),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let paths = temp_paths(&temp);
        configure_runtime(&paths, api_url);
        let store = AutomationStore::load(temp.path().join("automations.json")).unwrap();
        let record = store
            .create(
                "loop-helper",
                foreach_loop_spec(Some(2)),
                AutomationStatus::Enabled,
            )
            .unwrap();
        store
            .replace_runtime(
                "loop-helper",
                record.revision,
                deployed_runtime(
                    &record,
                    vec![root_workflow("wf-root"), loop_workflow("items", "wf-loop")],
                ),
            )
            .unwrap();

        let output = run_automation_preview_with_store(
            &paths,
            &store,
            "loop-helper",
            json!({ "text": "hello" }),
        )
        .unwrap();
        handle.join().expect("mock runtime joined");

        assert_eq!(output.result, json!({ "seen": 2 }));
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 3);
        assert!(captured[1].contains(r#""iteration":0"#));
        assert!(captured[1].contains(r#""item":1"#));
        assert!(captured[2].contains(r#""iteration":1"#));
        assert!(captured[2].contains(r#""item":2"#));
        assert!(!captured
            .iter()
            .any(|request| request.contains(r#""item":3"#)));
    }

    #[test]
    fn daemon_automation_runtime_preview_response_shape_hides_runtime_artifacts() {
        let record = AutomationRecord {
            id: "reply-helper".into(),
            status: AutomationStatus::Enabled,
            revision: 3,
            spec: linear_spec("Draft a reply."),
            runtime: AutomationRuntimeState {
                status: AutomationRuntimeStatus::Deployed,
                spec_hash: Some(
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                        .into(),
                ),
                compiled_revision: Some(3),
                agentenv_workflows: vec![CompiledAgentEnvWorkflow {
                    role: CompiledWorkflowRole::Root,
                    workflow_id: Some("internal-root".into()),
                    definition_hash: None,
                    deployed: true,
                }],
                puffer_bindings: vec![CompiledPufferBinding {
                    trigger_id: "incoming".into(),
                    binding_slug: "automation-reply-helper-incoming".into(),
                }],
                last_error: None,
            },
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        let output = AutomationRunOutput {
            compiled: true,
            record,
            result: json!({
                "ok": true,
                "workflowId": "customer-workflow-123",
                "nested": {
                    "binding_slug": "customer-binding"
                }
            }),
            summary: "completed".into(),
            status: "completed".into(),
            approval: None,
        };

        let value = automation_preview_response(&output);

        assert_eq!(value["id"], "reply-helper");
        assert_eq!(value["status"], "completed");
        assert_eq!(
            value["result"],
            json!({
                "ok": true,
                "workflowId": "customer-workflow-123",
                "nested": {
                    "binding_slug": "customer-binding"
                }
            })
        );
        assert_eq!(value["compiled"], true);
        assert_eq!(value["runtime"]["status"], "deployed");
        assert_eq!(value["runtime"]["agentenv_workflow_count"], 1);
        assert_eq!(value["runtime"]["puffer_binding_count"], 1);
        assert!(serde_json::to_string(&value)
            .unwrap()
            .find("internal-root")
            .is_none());
        assert!(serde_json::to_string(&value)
            .unwrap()
            .find("automation-reply-helper-incoming")
            .is_none());
    }
}
