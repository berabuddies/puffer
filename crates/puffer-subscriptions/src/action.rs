//! Action dispatchers — what happens to an event after it passes the
//! prefilter and classifier.

use crate::spec::{render_template, render_value_templates, ActionSpec, FileAppendFormat};
use anyhow::{Context, Result};
use puffer_subscriber_runtime::EventEnvelope;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

static GLOBAL_OUTBOUND: OnceLock<Arc<dyn Outbound>> = OnceLock::new();
static GLOBAL_WORKFLOW_RUNNER: OnceLock<Arc<dyn WorkflowActionRunner>> = OnceLock::new();
static GLOBAL_CONNECTOR_ACTION_EXECUTOR: OnceLock<Arc<dyn ConnectorActionExecutor>> =
    OnceLock::new();

/// Installs the process-wide outbound implementation. Returns
/// `Err(_)` if a different outbound has already been installed.
pub fn install_outbound(outbound: Arc<dyn Outbound>) -> Result<()> {
    GLOBAL_OUTBOUND
        .set(outbound)
        .map_err(|_| anyhow::anyhow!("outbound already installed"))
}

/// Installs the process-wide workflow action runner. Returns `Err(_)` if
/// a different runner has already been installed.
pub fn install_workflow_runner(runner: Arc<dyn WorkflowActionRunner>) -> Result<()> {
    GLOBAL_WORKFLOW_RUNNER
        .set(runner)
        .map_err(|_| anyhow::anyhow!("workflow runner already installed"))
}

/// Returns the process-wide workflow action runner, if one is installed.
pub fn installed_workflow_runner() -> Option<Arc<dyn WorkflowActionRunner>> {
    global_workflow_runner()
}

/// Installs the process-wide connector action executor. Returns `Err(_)`
/// if a different executor has already been installed.
pub fn install_connector_action_executor(executor: Arc<dyn ConnectorActionExecutor>) -> Result<()> {
    GLOBAL_CONNECTOR_ACTION_EXECUTOR
        .set(executor)
        .map_err(|_| anyhow::anyhow!("connector action executor already installed"))
}

fn global_outbound() -> Option<Arc<dyn Outbound>> {
    GLOBAL_OUTBOUND.get().cloned()
}

fn global_workflow_runner() -> Option<Arc<dyn WorkflowActionRunner>> {
    GLOBAL_WORKFLOW_RUNNER.get().cloned()
}

fn global_connector_action_executor() -> Option<Arc<dyn ConnectorActionExecutor>> {
    GLOBAL_CONNECTOR_ACTION_EXECUTOR.get().cloned()
}

/// Outcome of an action invocation. The router records the success bit
/// and a short message for diagnostics.
#[derive(Debug, Clone)]
pub struct ActionResult {
    /// Whether the action succeeded.
    pub success: bool,
    /// One-line summary suitable for logs and `/subscriptions status`.
    pub summary: String,
    /// Optional token usage for agent-backed actions.
    pub usage: Option<ActionUsage>,
    /// When the agent turn actually started executing (post any runner
    /// queueing/locking), for latency stats. `None` for non-agent actions.
    pub turn_started_at_ms: Option<i128>,
    /// When the agent turn finished executing.
    pub turn_ended_at_ms: Option<i128>,
}

impl ActionResult {
    /// Builds a successful action result without token usage.
    pub fn success(summary: impl Into<String>) -> Self {
        Self {
            success: true,
            summary: summary.into(),
            usage: None,
            turn_started_at_ms: None,
            turn_ended_at_ms: None,
        }
    }

    /// Builds a successful action result with optional token usage.
    pub fn success_with_usage(summary: impl Into<String>, usage: Option<ActionUsage>) -> Self {
        Self {
            success: true,
            summary: summary.into(),
            usage,
            turn_started_at_ms: None,
            turn_ended_at_ms: None,
        }
    }

    /// Builds a successful action result from a workflow runner output,
    /// carrying its usage and real turn-execution window.
    pub fn success_from_output(output: WorkflowActionOutput) -> Self {
        Self {
            success: true,
            summary: output.summary,
            usage: output.usage,
            turn_started_at_ms: output.turn_started_at_ms,
            turn_ended_at_ms: output.turn_ended_at_ms,
        }
    }

    /// Builds a failed action result without token usage.
    pub fn failure(summary: impl Into<String>) -> Self {
        Self {
            success: false,
            summary: summary.into(),
            usage: None,
            turn_started_at_ms: None,
            turn_ended_at_ms: None,
        }
    }
}

/// Token usage reported by an agent-backed workflow action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionUsage {
    /// Input tokens reported by the model provider.
    pub input_tokens: u64,
    /// Output tokens reported by the model provider.
    pub output_tokens: u64,
    /// Input tokens served from provider cache.
    pub cache_read_tokens: u64,
    /// Input tokens written into provider cache.
    pub cache_creation_tokens: u64,
}

impl ActionUsage {
    /// Returns the non-cached input plus output token total.
    pub fn spent_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_sub(self.cache_read_tokens)
            .saturating_add(self.output_tokens)
    }
}

/// Output returned by a workflow action runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowActionOutput {
    /// One-line or concise action summary.
    pub summary: String,
    /// Optional token usage for agent-backed actions.
    pub usage: Option<ActionUsage>,
    /// When the agent turn actually started executing (post any runner
    /// queueing/locking). Lets history separate queue wait from turn time.
    pub turn_started_at_ms: Option<i128>,
    /// When the agent turn finished executing.
    pub turn_ended_at_ms: Option<i128>,
}

impl WorkflowActionOutput {
    /// Builds workflow action output without token usage.
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            usage: None,
            turn_started_at_ms: None,
            turn_ended_at_ms: None,
        }
    }

    /// Builds workflow action output with token usage.
    pub fn with_usage(summary: impl Into<String>, usage: Option<ActionUsage>) -> Self {
        Self {
            summary: summary.into(),
            usage,
            turn_started_at_ms: None,
            turn_ended_at_ms: None,
        }
    }

    /// Attaches the real turn-execution window.
    pub fn with_turn_window(mut self, started_at_ms: i128, ended_at_ms: i128) -> Self {
        self.turn_started_at_ms = Some(started_at_ms);
        self.turn_ended_at_ms = Some(ended_at_ms);
        self
    }
}

/// Trait for delivering an outbound message through whichever subscriber
/// is responsible for the named platform. The MVP impl, installed by
/// `puffer-cli`, routes `("telegram", peer, text)` to the running
/// `telegram-user` subscriber via [`puffer_subscriber_runtime::SubscriberCommand::SendMessage`].
///
/// Implementations are synchronous from the dispatcher's view; if they
/// need to drive a Tokio runtime they own that internally.
pub trait Outbound: Send + Sync {
    /// Sends `text` to `target` on `platform`. Returns a one-line
    /// human-readable summary on success.
    fn send(&self, platform: &str, target: &str, text: &str) -> Result<String>;
}

/// Trait for triggering native Puffer workflows from subscription actions.
pub trait WorkflowActionRunner: Send + Sync {
    /// Runs `slug` with `trigger` as the interpolation payload.
    fn run_workflow(&self, slug: &str, trigger: serde_json::Value) -> Result<WorkflowActionOutput>;

    /// Executes a Puffer tool call from a workflow action.
    fn run_tool_action(
        &self,
        tool_id: &str,
        input: serde_json::Value,
        trigger: serde_json::Value,
    ) -> Result<WorkflowActionOutput> {
        let _ = (tool_id, input, trigger);
        anyhow::bail!("workflow tool actions are not installed in this runtime")
    }

    /// Sends an event to an agent for triage.
    fn triage_agent(
        &self,
        prompt: &str,
        model: Option<&str>,
        trigger: serde_json::Value,
    ) -> Result<WorkflowActionOutput> {
        let _ = (prompt, model, trigger);
        anyhow::bail!("workflow agent triage is not installed in this runtime")
    }

    /// Sends a batch of same-connection events to an agent for one triage turn.
    fn triage_agent_batch(
        &self,
        prompt: &str,
        model: Option<&str>,
        triggers: Vec<serde_json::Value>,
    ) -> Result<WorkflowActionOutput> {
        let mut triggers = triggers;
        if triggers.len() == 1 {
            return self.triage_agent(prompt, model, triggers.remove(0));
        }
        anyhow::bail!("workflow agent batch triage is not installed in this runtime")
    }

    /// Sends an ignored monitor task to a read-only agent for analysis.
    fn ignore_analysis_agent(
        &self,
        prompt: &str,
        model: Option<&str>,
        trigger: serde_json::Value,
    ) -> Result<WorkflowActionOutput> {
        let _ = (prompt, model, trigger);
        anyhow::bail!("workflow ignore analysis is not installed in this runtime")
    }
}

/// Trait for executing connector actions from background workflow dispatch.
pub trait ConnectorActionExecutor: Send + Sync {
    /// Runs one connector action and returns a human-readable summary.
    fn run_connector_action(
        &self,
        connector_slug: &str,
        action: &str,
        input: serde_json::Value,
        trigger: serde_json::Value,
    ) -> Result<String>;
}

/// Dispatcher trait — one method per invocation. Implementations may keep
/// connection pools, etc., behind interior mutability.
pub trait ActionDispatcher: Send + Sync {
    /// Executes `action` for `envelope` and returns the result.
    fn dispatch(&self, action: &ActionSpec, envelope: &EventEnvelope) -> ActionResult;

    /// Executes `action` for a same-binding envelope batch.
    fn dispatch_batch(&self, action: &ActionSpec, envelopes: &[EventEnvelope]) -> ActionResult {
        dispatch_batch_sequential(self, action, envelopes)
    }
}

/// Built-in dispatcher for the MVP action set.
///
/// `sqlite_insert` connects to the configured database (creating it on
/// demand), creates a default table schema if missing, and inserts one
/// row per matched event.
///
/// `forward_message` calls into the installed [`Outbound`] impl. When no
/// outbound has been installed (e.g. the binary is running without
/// connector wiring) the action returns a clear error so the agent can
/// surface it to the user.
pub struct BuiltinActionDispatcher {
    sqlite_pool: Mutex<Vec<(PathBuf, Connection)>>,
    storage_root: PathBuf,
    /// Per-instance outbound override, used by tests. Production code
    /// installs the outbound process-globally via [`install_outbound`]
    /// so any dispatcher instance picks it up.
    outbound: OnceLock<Arc<dyn Outbound>>,
    workflow_runner: OnceLock<Arc<dyn WorkflowActionRunner>>,
    connector_action_executor: OnceLock<Arc<dyn ConnectorActionExecutor>>,
}

impl Default for BuiltinActionDispatcher {
    fn default() -> Self {
        Self {
            sqlite_pool: Mutex::new(Vec::new()),
            storage_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            outbound: OnceLock::new(),
            workflow_runner: OnceLock::new(),
            connector_action_executor: OnceLock::new(),
        }
    }
}

impl BuiltinActionDispatcher {
    /// Constructs an empty dispatcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Constructs a dispatcher rooted at `storage_root` for relative file actions.
    pub fn with_storage_root(storage_root: impl Into<PathBuf>) -> Self {
        Self {
            storage_root: storage_root.into(),
            ..Self::default()
        }
    }

    /// Installs an outbound implementation on this specific dispatcher
    /// instance. Tests use this to inject a recording outbound. Production
    /// code should prefer the process-wide [`install_outbound`].
    pub fn set_outbound(&self, outbound: Arc<dyn Outbound>) {
        let _ = self.outbound.set(outbound);
    }

    /// Installs a workflow runner on this dispatcher instance.
    pub fn set_workflow_runner(&self, runner: Arc<dyn WorkflowActionRunner>) {
        let _ = self.workflow_runner.set(runner);
    }

    /// Installs a connector action executor on this dispatcher instance.
    pub fn set_connector_action_executor(&self, executor: Arc<dyn ConnectorActionExecutor>) {
        let _ = self.connector_action_executor.set(executor);
    }

    fn resolved_outbound(&self) -> Option<Arc<dyn Outbound>> {
        self.outbound.get().cloned().or_else(global_outbound)
    }

    fn resolved_workflow_runner(&self) -> Option<Arc<dyn WorkflowActionRunner>> {
        self.workflow_runner
            .get()
            .cloned()
            .or_else(global_workflow_runner)
    }

    fn resolved_connector_action_executor(&self) -> Option<Arc<dyn ConnectorActionExecutor>> {
        self.connector_action_executor
            .get()
            .cloned()
            .or_else(global_connector_action_executor)
    }

    fn sqlite_insert(
        &self,
        path: &str,
        table: &str,
        envelope: &EventEnvelope,
    ) -> Result<ActionResult> {
        let absolute = resolve_sqlite_path(&self.storage_root, path)?;
        let mut pool = self.sqlite_pool.lock().unwrap();
        if !pool.iter().any(|(p, _)| p == &absolute) {
            if let Some(parent) = absolute.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            let conn = Connection::open(&absolute)
                .with_context(|| format!("open sqlite database {}", absolute.display()))?;
            pool.push((absolute.clone(), conn));
        }
        let conn = &pool
            .iter()
            .find(|(p, _)| p == &absolute)
            .expect("just inserted")
            .1;
        // The CREATE/INSERT SQL inlines `table` — we validate it as a
        // strict identifier in spec::validate_spec, so this is safe.
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {table} (\
                envelope_id TEXT PRIMARY KEY, \
                received_at_ms INTEGER NOT NULL, \
                subscriber_id TEXT NOT NULL, \
                topic TEXT NOT NULL, \
                kind TEXT NOT NULL, \
                dedup_key TEXT, \
                text TEXT NOT NULL, \
                payload TEXT NOT NULL\
            );"
        ))
        .with_context(|| format!("create table {table}"))?;
        let payload =
            serde_json::to_string(&envelope.event.payload).unwrap_or_else(|_| "null".into());
        conn.execute(
            &format!(
                "INSERT OR IGNORE INTO {table} \
                  (envelope_id, received_at_ms, subscriber_id, topic, kind, dedup_key, text, payload) \
                  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            ),
            params![
                envelope.envelope_id,
                envelope.received_at_ms as i64,
                envelope.subscriber_id,
                envelope.event.topic,
                envelope.event.kind,
                envelope.event.dedup_key,
                envelope.event.text,
                payload,
            ],
        )
        .with_context(|| format!("insert into {table}"))?;
        Ok(ActionResult::success(format!(
            "inserted into {} ({})",
            absolute.display(),
            table
        )))
    }

    fn file_append(
        &self,
        path: &str,
        format: FileAppendFormat,
        envelope: &EventEnvelope,
    ) -> Result<ActionResult> {
        let absolute = resolve_file_append_path(&self.storage_root, path)?;
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create file_append parent {}", parent.display()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&absolute)
            .with_context(|| format!("open append file {}", absolute.display()))?;
        match format {
            FileAppendFormat::Text => {
                file.write_all(envelope.event.text.as_bytes())
                    .with_context(|| format!("append text to {}", absolute.display()))?;
                file.write_all(b"\n")
                    .with_context(|| format!("append newline to {}", absolute.display()))?;
            }
            FileAppendFormat::Jsonl => {
                let line = serde_json::to_vec(&trigger_payload(envelope))
                    .context("serialize file_append jsonl event")?;
                file.write_all(&line)
                    .with_context(|| format!("append jsonl to {}", absolute.display()))?;
                file.write_all(b"\n")
                    .with_context(|| format!("append newline to {}", absolute.display()))?;
            }
        }
        Ok(ActionResult::success(format!(
            "appended to {}",
            absolute.display()
        )))
    }

    fn forward_message(
        &self,
        platform: &str,
        target: &str,
        template: Option<&str>,
        envelope: &EventEnvelope,
    ) -> ActionResult {
        let rendered = template
            .map(|t| render_template(t, &envelope.event.text, &envelope.event.payload))
            .unwrap_or_else(|| envelope.event.text.clone());
        match self.resolved_outbound() {
            Some(outbound) => match outbound.send(platform, target, &rendered) {
                Ok(summary) => ActionResult::success(summary),
                Err(error) => ActionResult::failure(format!(
                    "forward_message to {platform}:{target} failed: {error:#}"
                )),
            },
            None => ActionResult::failure(format!(
                    "forward_message: no outbound is installed; spec needs {platform}:{target} but the running puffer process cannot deliver messages"
                )),
        }
    }

    fn run_workflow(&self, slug: &str, envelope: &EventEnvelope) -> ActionResult {
        let trigger = trigger_payload(envelope);
        match self.resolved_workflow_runner() {
            Some(runner) => match runner.run_workflow(slug, trigger) {
                Ok(output) => ActionResult::success_from_output(output),
                Err(error) => {
                    ActionResult::failure(format!("run_workflow `{slug}` failed: {error:#}"))
                }
            },
            None => ActionResult::failure(format!(
                "run_workflow: no workflow runner is installed; `{slug}` cannot be run"
            )),
        }
    }

    fn tool_call(
        &self,
        tool: &str,
        input: &serde_json::Value,
        envelope: &EventEnvelope,
    ) -> ActionResult {
        let rendered = render_value_templates(input, &envelope.event.text, &envelope.event.payload);
        let trigger = trigger_payload(envelope);
        match self.resolved_workflow_runner() {
            Some(runner) => match runner.run_tool_action(tool, rendered, trigger) {
                Ok(output) => ActionResult::success_from_output(output),
                Err(error) => {
                    ActionResult::failure(format!("tool_call `{tool}` failed: {error:#}"))
                }
            },
            None => ActionResult::failure("tool_call: no workflow action runner is installed"),
        }
    }

    fn triage_agent(
        &self,
        prompt: &str,
        model: Option<&str>,
        envelope: &EventEnvelope,
    ) -> ActionResult {
        let rendered = render_template(prompt, &envelope.event.text, &envelope.event.payload);
        let trigger = trigger_payload(envelope);
        tracing::info!(
            envelope = %envelope.envelope_id,
            topic = %envelope.event.topic,
            kind = %envelope.event.kind,
            model = %model.unwrap_or("default"),
            prompt_len = rendered.len(),
            text_len = envelope.event.text.len(),
            "triage_agent action starting"
        );
        match self.resolved_workflow_runner() {
            Some(runner) => match runner.triage_agent(&rendered, model, trigger) {
                Ok(output) => {
                    tracing::info!(
                        envelope = %envelope.envelope_id,
                        summary = %output.summary,
                        "triage_agent action completed"
                    );
                    ActionResult::success_from_output(output)
                }
                Err(error) => {
                    tracing::warn!(
                        envelope = %envelope.envelope_id,
                        %error,
                        "triage_agent action failed"
                    );
                    ActionResult::failure(format!("triage_agent failed: {error:#}"))
                }
            },
            None => {
                tracing::warn!(
                    envelope = %envelope.envelope_id,
                    "triage_agent action missing workflow runner"
                );
                ActionResult::failure("triage_agent: no workflow action runner is installed")
            }
        }
    }

    fn triage_agent_batch(
        &self,
        prompt: &str,
        model: Option<&str>,
        envelopes: &[EventEnvelope],
    ) -> ActionResult {
        let Some(first) = envelopes.first() else {
            return ActionResult::success("triage_agent batch skipped: no events");
        };
        let rendered = render_template(prompt, &first.event.text, &first.event.payload);
        let triggers: Vec<_> = envelopes.iter().map(trigger_payload).collect();
        tracing::info!(
            batch_size = envelopes.len(),
            topic = %first.event.topic,
            kind = %first.event.kind,
            model = %model.unwrap_or("default"),
            prompt_len = rendered.len(),
            "triage_agent batch action starting"
        );
        match self.resolved_workflow_runner() {
            Some(runner) => match runner.triage_agent_batch(&rendered, model, triggers) {
                Ok(output) => {
                    tracing::info!(
                        batch_size = envelopes.len(),
                        summary = %output.summary,
                        "triage_agent batch action completed"
                    );
                    ActionResult::success_from_output(output)
                }
                Err(error) => {
                    tracing::warn!(
                        batch_size = envelopes.len(),
                        %error,
                        "triage_agent batch action failed"
                    );
                    ActionResult::failure(format!("triage_agent failed: {error:#}"))
                }
            },
            None => {
                tracing::warn!(
                    batch_size = envelopes.len(),
                    "triage_agent batch action missing workflow runner"
                );
                ActionResult::failure("triage_agent: no workflow action runner is installed")
            }
        }
    }

    fn connector_act(
        &self,
        connector_slug: &str,
        action: &str,
        input: &serde_json::Value,
        envelope: &EventEnvelope,
    ) -> ActionResult {
        let rendered = render_value_templates(input, &envelope.event.text, &envelope.event.payload);
        let trigger = trigger_payload(envelope);
        match self.resolved_connector_action_executor() {
            Some(executor) => {
                match executor.run_connector_action(connector_slug, action, rendered, trigger) {
                    Ok(summary) => ActionResult::success(summary),
                    Err(error) => ActionResult::failure(format!(
                        "connector_act `{connector_slug}.{action}` failed: {error:#}"
                    )),
                }
            }
            None => {
                ActionResult::failure("connector_act: no connector action executor is installed")
            }
        }
    }

    fn graph(
        &self,
        nodes: &[crate::spec::ActionGraphNode],
        envelope: &EventEnvelope,
    ) -> ActionResult {
        let mut summaries = Vec::new();
        let mut completed = std::collections::BTreeSet::new();
        while completed.len() < nodes.len() {
            let Some(node) = nodes.iter().find(|node| {
                !completed.contains(&node.id)
                    && node.depends_on.iter().all(|dep| completed.contains(dep))
            }) else {
                return ActionResult::failure(
                    "graph has no executable node; validate the action graph first",
                );
            };
            let result = self.dispatch(&node.action, envelope);
            summaries.push(format!("{}: {}", node.id, result.summary));
            if !result.success {
                return ActionResult::failure(summaries.join("; "));
            }
            completed.insert(node.id.clone());
        }
        ActionResult::success(summaries.join("; "))
    }
}

impl ActionDispatcher for BuiltinActionDispatcher {
    fn dispatch(&self, action: &ActionSpec, envelope: &EventEnvelope) -> ActionResult {
        match action {
            ActionSpec::SqliteInsert { path, table } => {
                match self.sqlite_insert(path, table, envelope) {
                    Ok(result) => result,
                    Err(error) => ActionResult::failure(format!("sqlite_insert failed: {error:#}")),
                }
            }
            ActionSpec::FileAppend { path, format } => {
                match self.file_append(path, *format, envelope) {
                    Ok(result) => result,
                    Err(error) => ActionResult::failure(format!("file_append failed: {error:#}")),
                }
            }
            ActionSpec::ForwardMessage {
                platform,
                target,
                template,
            } => self.forward_message(platform, target, template.as_deref(), envelope),
            ActionSpec::RunWorkflow { slug } => self.run_workflow(slug, envelope),
            ActionSpec::ConnectorAct {
                connector_slug,
                action,
                input,
            } => self.connector_act(connector_slug, action, input, envelope),
            ActionSpec::ToolCall { tool, input } => self.tool_call(tool, input, envelope),
            ActionSpec::TriageAgent { prompt, model } => {
                self.triage_agent(prompt, model.as_deref(), envelope)
            }
            ActionSpec::Graph { nodes } => self.graph(nodes, envelope),
            ActionSpec::Unknown => ActionResult::failure(
                "action.type unknown — agent wrote a spec this Puffer build cannot run",
            ),
        }
    }

    fn dispatch_batch(&self, action: &ActionSpec, envelopes: &[EventEnvelope]) -> ActionResult {
        match action {
            ActionSpec::TriageAgent { prompt, model } => {
                self.triage_agent_batch(prompt, model.as_deref(), envelopes)
            }
            _ => dispatch_batch_sequential(self, action, envelopes),
        }
    }
}

fn dispatch_batch_sequential<D: ActionDispatcher + ?Sized>(
    dispatcher: &D,
    action: &ActionSpec,
    envelopes: &[EventEnvelope],
) -> ActionResult {
    if envelopes.is_empty() {
        return ActionResult::success("batch skipped: no events");
    }
    let mut summaries = Vec::with_capacity(envelopes.len());
    let mut usage = None;
    let mut failed = 0usize;
    for envelope in envelopes {
        let result = dispatcher.dispatch(action, envelope);
        if !result.success {
            failed += 1;
        }
        if let Some(next) = result.usage {
            merge_action_usage(&mut usage, next);
        }
        summaries.push(result.summary);
    }
    let summary = summaries.join("; ");
    if failed == 0 {
        ActionResult::success_with_usage(summary, usage)
    } else {
        ActionResult {
            success: false,
            summary,
            usage,
            turn_started_at_ms: None,
            turn_ended_at_ms: None,
        }
    }
}

fn merge_action_usage(total: &mut Option<ActionUsage>, next: ActionUsage) {
    let current = total.get_or_insert(ActionUsage {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    });
    current.input_tokens = current.input_tokens.saturating_add(next.input_tokens);
    current.output_tokens = current.output_tokens.saturating_add(next.output_tokens);
    current.cache_read_tokens = current
        .cache_read_tokens
        .saturating_add(next.cache_read_tokens);
    current.cache_creation_tokens = current
        .cache_creation_tokens
        .saturating_add(next.cache_creation_tokens);
}

fn trigger_payload(envelope: &EventEnvelope) -> serde_json::Value {
    json!({
        "type": "connection",
        "envelope_id": envelope.envelope_id,
        "connection_id": envelope.subscriber_id,
        "receivedAt": format_epoch_ms_rfc3339(envelope.received_at_ms),
        "topic": envelope.event.topic,
        "kind": envelope.event.kind,
        "dedup_key": envelope.event.dedup_key,
        "text": envelope.event.text,
        "payload": envelope.event.payload,
    })
}

fn format_epoch_ms_rfc3339(value: i128) -> Option<String> {
    if value < 0 || value > i64::MAX as i128 {
        return None;
    }
    let seconds = (value / 1_000) as i64;
    let nanos = ((value % 1_000) as u32) * 1_000_000;
    let time = OffsetDateTime::from_unix_timestamp(seconds)
        .ok()?
        .replace_nanosecond(nanos)
        .ok()?;
    time.format(&Rfc3339).ok()
}

fn resolve_sqlite_path(storage_root: &Path, path: &str) -> Result<PathBuf> {
    let raw = path.trim();
    if raw.starts_with("~/") {
        anyhow::bail!("sqlite_insert.path must be relative to subscription storage");
    }
    let candidate = Path::new(raw);
    if candidate.is_absolute() || has_parent_component(candidate) {
        anyhow::bail!("sqlite_insert.path must be a safe relative path");
    }
    Ok(storage_root.join(candidate))
}

fn resolve_file_append_path(storage_root: &Path, path: &str) -> Result<PathBuf> {
    let raw = path.trim();
    if raw.starts_with("~/") {
        anyhow::bail!("file_append.path must be relative or under /tmp");
    }
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        if candidate.starts_with("/tmp") && !has_parent_component(candidate) {
            return Ok(candidate.to_path_buf());
        }
        anyhow::bail!("file_append.path absolute paths must be under /tmp");
    }
    if has_parent_component(candidate) {
        anyhow::bail!("file_append.path must not contain parent traversal");
    }
    Ok(storage_root.join(candidate))
}

fn has_parent_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
}

#[cfg(test)]
#[path = "action_tests.rs"]
mod tests;
