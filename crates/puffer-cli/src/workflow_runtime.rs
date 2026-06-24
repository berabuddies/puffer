use anyhow::{Context, Result};
use puffer_config::{load_config, ConfigPaths, PufferConfig};
use puffer_core::{
    execute_tool_action_once, execute_user_turn_streaming,
    execute_user_turn_streaming_excluding_tools, execute_user_turn_streaming_without_tools,
    AppState, MonitorSourceStampContext, MonitorTaskCreateGateContext, TurnStreamEvent,
};
use puffer_provider_registry::{canonical_provider_id, AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, BACKGROUND_SESSION_TAG};
use puffer_subscriber_telegram_user::TelegramHistoryCache;
use puffer_subscriptions::{
    install_workflow_runner, ActionUsage, TriageDecision, TriageDecisionOutcome,
    WorkflowActionOutput, WorkflowActionRunner,
};
use puffer_workflow::{
    AgentExecution, AgentExecutor, CronDeduper, CronExpression, DagRunner, ExecutionContext,
    TriggerSpec, WorkflowStore,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;
use time::{OffsetDateTime, UtcOffset};

const OPENAI_TASK_AGENT_MODEL: &str = "gpt-5.4-mini";
const ANTHROPIC_TASK_AGENT_MODEL: &str = "claude-haiku-4-5-20251001";
const TELEGRAM_CONVERSATION_CONTEXT_LIMIT: usize = 8;
const NO_TASK_DECISION_MARKER: &str = "NO_TASK_DECISIONS:";
const MAX_TRIAGE_REASON_CHARS: usize = 240;
const MAX_TRIAGE_POLICY_CHARS: usize = 80;

/// Owns native workflow trigger hooks for the current process.
pub(crate) struct WorkflowRuntime {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl WorkflowRuntime {
    fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WorkflowRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Installs workflow action dispatch and starts cron polling.
pub(crate) fn install(
    paths: &ConfigPaths,
    config: &PufferConfig,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> Result<WorkflowRuntime> {
    let runner = Arc::new(ProcessWorkflowRunner {
        paths: paths.clone(),
        config: config.clone(),
        resources: resources.clone(),
        providers: providers.clone(),
        auth_store: auth_store.clone(),
        lock: Mutex::new(()),
    });
    install_workflow_runner(runner.clone()).context("failed to install workflow runner")?;
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = thread::Builder::new()
        .name("puffer-workflow-cron".to_string())
        .spawn(move || cron_loop(runner, thread_stop))
        .context("failed to start workflow cron thread")?;
    Ok(WorkflowRuntime {
        stop,
        thread: Some(thread),
    })
}

struct ProcessWorkflowRunner {
    paths: ConfigPaths,
    config: PufferConfig,
    resources: LoadedResources,
    providers: ProviderRegistry,
    auth_store: AuthStore,
    lock: Mutex<()>,
}

struct WorkflowRuntimeSnapshot {
    config: PufferConfig,
    providers: ProviderRegistry,
    auth_store: AuthStore,
}

impl WorkflowActionRunner for ProcessWorkflowRunner {
    fn run_workflow(&self, slug: &str, trigger: serde_json::Value) -> Result<WorkflowActionOutput> {
        let _guard = workflow_runner_lock(&self.lock);
        let store = WorkflowStore::new(&self.paths.workspace_config_dir);
        let definition = store
            .get(slug)?
            .ok_or_else(|| anyhow::anyhow!("workflow `{slug}` is not registered"))?;
        if !definition.enabled {
            anyhow::bail!("workflow `{slug}` is disabled");
        }
        let snapshot = self.runtime_snapshot();
        let run = DagRunner::new(
            &store,
            PufferAgentExecutor {
                paths: self.paths.clone(),
                config: snapshot.config,
                resources: self.resources.clone(),
                providers: snapshot.providers,
                auth_store: snapshot.auth_store,
            },
        )
        .run(&definition, trigger, None)?;
        Ok(WorkflowActionOutput::new(format!(
            "workflow `{slug}` run #{} {:?}",
            run.idx, run.status
        )))
    }

    fn run_tool_action(
        &self,
        tool_id: &str,
        input: serde_json::Value,
        _trigger: serde_json::Value,
    ) -> Result<WorkflowActionOutput> {
        let _guard = workflow_runner_lock(&self.lock);
        let cwd = self.paths.workspace_root.clone();
        let snapshot = self.runtime_snapshot();
        let mut state = self.new_app_state_with_snapshot(cwd.clone(), None, &snapshot)?;
        let result = execute_tool_action_once(
            &mut state,
            &self.resources,
            &snapshot.providers,
            &snapshot.auth_store,
            &cwd,
            tool_id,
            input,
        )?;
        if result.success {
            return Ok(WorkflowActionOutput::new(result.output.stdout));
        }
        anyhow::bail!(
            "tool `{}` failed: stdout={} stderr={}",
            tool_id,
            result.output.stdout,
            result.output.stderr
        )
    }

    fn triage_agent(
        &self,
        prompt: &str,
        model: Option<&str>,
        trigger: serde_json::Value,
    ) -> Result<WorkflowActionOutput> {
        self.triage_agent_batch(prompt, model, vec![trigger])
    }

    fn triage_agent_batch(
        &self,
        prompt: &str,
        model: Option<&str>,
        triggers: Vec<serde_json::Value>,
    ) -> Result<WorkflowActionOutput> {
        let triggers = triggers
            .into_iter()
            .map(|trigger| enrich_monitor_trigger_context(&self.paths, trigger))
            .collect::<Result<Vec<_>>>()?;
        // Outgoing-only bursts run in completion mode (the session excludes
        // task-creation tools); any incoming message keeps creation enabled.
        let is_outgoing = triggers.iter().all(|trigger| {
            trigger
                .get("payload")
                .and_then(|p| p.get("is_outgoing"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        });
        let prompt = render_triage_batch_prompt(prompt, &triggers)?;
        let session_key = triage_session_key(model, &triggers);
        let task_snapshot_before = match load_monitor_task_trace_values(&self.paths) {
            Ok(tasks) => Some(tasks),
            Err(error) => {
                tracing::warn!(%error, "failed to read monitor tasks before triage");
                None
            }
        };
        let gate_contexts = monitor_task_create_gate_contexts(&self.paths, &triggers);
        let source_stamp_contexts = monitor_source_stamp_contexts(&triggers);
        let mut output = self.run_task_agent_prompt_for_session(
            prompt,
            model,
            &session_key,
            is_outgoing,
            gate_contexts,
            source_stamp_contexts,
        )?;
        let task_decisions = match task_snapshot_before {
            Some(before) => match load_monitor_task_trace_values(&self.paths) {
                Ok(after) => monitor_task_trace_decisions(&before, &after, &triggers),
                Err(error) => {
                    tracing::warn!(%error, "failed to read monitor tasks after triage");
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        for trigger in &triggers {
            // Server-owned grounding: stamp each trigger's verbatim event text
            // onto any monitor task the triage turn created for that envelope.
            if let Err(error) = record_monitor_source_text(&self.paths, trigger) {
                tracing::warn!(%error, "failed to record verbatim monitor source text");
            }
        }
        if !task_decisions.is_empty() {
            let task_envelopes: HashSet<&str> = task_decisions
                .iter()
                .map(|decision| decision.envelope_id.as_str())
                .collect();
            output.triage_decisions.retain(|decision| {
                decision.outcome != TriageDecisionOutcome::NoTask
                    || !task_envelopes.contains(decision.envelope_id.as_str())
            });
            output.triage_decisions.extend(task_decisions);
        }
        Ok(output)
    }

    fn ignore_analysis_agent(
        &self,
        prompt: &str,
        model: Option<&str>,
        trigger: serde_json::Value,
    ) -> Result<WorkflowActionOutput> {
        let trigger = serde_json::to_string_pretty(&trigger)?;
        let prompt = format!("{prompt}\n\nWorkflow trigger:\n```json\n{trigger}\n```");
        self.run_task_agent_prompt_without_tools(prompt, model)
    }
}

impl ProcessWorkflowRunner {
    fn new_app_state_with_snapshot(
        &self,
        cwd: PathBuf,
        model: Option<&str>,
        snapshot: &WorkflowRuntimeSnapshot,
    ) -> Result<AppState> {
        let session_store = SessionStore::from_paths(&self.paths)?;
        let session = session_store
            .create_session_with_tags(cwd.clone(), vec![BACKGROUND_SESSION_TAG.to_string()])?;
        let mut state = AppState::new(snapshot.config.clone(), cwd, session);
        if let Some(model) = model.and_then(non_empty_trimmed) {
            apply_explicit_model(&mut state, model);
        } else {
            apply_authenticated_provider_fallback(
                &mut state,
                &snapshot.providers,
                &snapshot.auth_store,
            );
        }
        Ok(state)
    }

    fn new_task_app_state_with_snapshot(
        &self,
        cwd: PathBuf,
        model: Option<&str>,
        snapshot: &WorkflowRuntimeSnapshot,
    ) -> Result<AppState> {
        let mut state = self.new_app_state_with_snapshot(cwd, model, snapshot)?;
        if model.and_then(non_empty_trimmed).is_none() {
            apply_task_agent_model_default(&mut state, &snapshot.providers);
        }
        Ok(state)
    }

    fn runtime_snapshot(&self) -> WorkflowRuntimeSnapshot {
        let config = match load_config(&self.paths) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "failed to refresh workflow config; using startup snapshot"
                );
                self.config.clone()
            }
        };
        let auth_path = self.paths.user_config_dir.join("auth.json");
        let auth_store = match AuthStore::load(&auth_path) {
            Ok(auth_store) => auth_store,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "failed to refresh workflow auth store; using startup snapshot"
                );
                self.auth_store.clone()
            }
        };
        let mut providers = self.providers.clone();
        apply_config_provider_overrides(&mut providers, &config);
        WorkflowRuntimeSnapshot {
            config,
            providers,
            auth_store,
        }
    }

    fn run_task_agent_prompt_for_session(
        &self,
        prompt: String,
        model: Option<&str>,
        session_key: &str,
        is_outgoing: bool,
        gate_contexts: Vec<MonitorTaskCreateGateContext>,
        source_stamp_contexts: Vec<MonitorSourceStampContext>,
    ) -> Result<WorkflowActionOutput> {
        let _guard = workflow_runner_lock(&self.lock);
        let cwd = self.paths.workspace_root.clone();
        // Fresh agent state per triage turn. A long-lived session shared by
        // every turn on the same connection let earlier messages contaminate
        // later ones — numbers/times were copied from sibling messages in the
        // accumulated history (agentenv/monorepo#619). Task dedup never needed
        // session memory (the triage protocol requires TaskList, which reads
        // the store from disk), and the stable per-connection cache key keeps
        // prompt-prefix caching effective across fresh sessions.
        let snapshot = self.runtime_snapshot();
        let mut state = self.new_task_app_state_with_snapshot(cwd, model, &snapshot)?;
        state.prompt_cache_key_override = Some(session_key.to_string());
        // This is the monitor triage turn: a conversational "done" signal in the
        // chat is itself the human action, and TaskUpdate only marks the task
        // complete (it never sends a reply). Let the triage agent complete
        // human-gated monitor tasks; the reply-approval gate stays enforced on the
        // separate reply-send path.
        state.monitor_triage_turn = true;
        state.set_monitor_task_create_gate_contexts(gate_contexts);
        state.set_monitor_source_stamp_contexts(source_stamp_contexts);
        let mut auth_store = snapshot.auth_store.clone();
        let mut usage = None;
        // Post-lock stamp: history's run window starts at router dispatch, so
        // this is what separates queue/lock wait from actual turn time.
        let turn_started_at_ms = now_unix_ms();
        // On outgoing turns the agent must not create new tasks — derive the
        // exclusion from the shared constant so test and runtime stay in sync.
        let excluded_tools = triage_excluded_tools_for_direction(is_outgoing);
        let output = if excluded_tools.is_empty() {
            execute_user_turn_streaming(
                &mut state,
                &self.resources,
                &snapshot.providers,
                &mut auth_store,
                &prompt,
                |event| {
                    if let TurnStreamEvent::Usage(report) = event {
                        merge_usage(
                            &mut usage,
                            ActionUsage {
                                input_tokens: report.input_tokens,
                                output_tokens: report.output_tokens,
                                cache_read_tokens: report.cache_read_tokens,
                                cache_creation_tokens: report.cache_creation_tokens,
                            },
                        );
                    }
                },
            )?
        } else {
            execute_user_turn_streaming_excluding_tools(
                &mut state,
                &self.resources,
                &snapshot.providers,
                &mut auth_store,
                &prompt,
                excluded_tools,
                |event| {
                    if let TurnStreamEvent::Usage(report) = event {
                        merge_usage(
                            &mut usage,
                            ActionUsage {
                                input_tokens: report.input_tokens,
                                output_tokens: report.output_tokens,
                                cache_read_tokens: report.cache_read_tokens,
                                cache_creation_tokens: report.cache_creation_tokens,
                            },
                        );
                    }
                },
            )?
        };
        let triage_decisions = extract_no_task_decisions(&output.assistant_text);
        Ok(
            WorkflowActionOutput::with_usage(output.assistant_text, usage)
                .with_triage_decisions(triage_decisions)
                .with_turn_window(turn_started_at_ms, now_unix_ms()),
        )
    }

    fn run_task_agent_prompt_without_tools(
        &self,
        prompt: String,
        model: Option<&str>,
    ) -> Result<WorkflowActionOutput> {
        let cwd = self.paths.workspace_root.clone();
        let snapshot = self.runtime_snapshot();
        let mut state = self.new_task_app_state_with_snapshot(cwd, model, &snapshot)?;
        let mut auth_store = snapshot.auth_store.clone();
        let mut usage = None;
        let output = execute_user_turn_streaming_without_tools(
            &mut state,
            &self.resources,
            &snapshot.providers,
            &mut auth_store,
            &prompt,
            |event| {
                if let TurnStreamEvent::Usage(report) = event {
                    merge_usage(
                        &mut usage,
                        ActionUsage {
                            input_tokens: report.input_tokens,
                            output_tokens: report.output_tokens,
                            cache_read_tokens: report.cache_read_tokens,
                            cache_creation_tokens: report.cache_creation_tokens,
                        },
                    );
                }
            },
        )?;
        Ok(WorkflowActionOutput::with_usage(
            output.assistant_text,
            usage,
        ))
    }
}

fn merge_usage(total: &mut Option<ActionUsage>, next: ActionUsage) {
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

fn workflow_runner_lock(lock: &Mutex<()>) -> MutexGuard<'_, ()> {
    match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("workflow runner lock was poisoned; recovering for next action");
            poisoned.into_inner()
        }
    }
}

fn render_triage_batch_prompt(prompt: &str, triggers: &[serde_json::Value]) -> Result<String> {
    let trigger_label = if triggers.len() == 1 {
        "Workflow trigger"
    } else {
        "Monitor digest triggers"
    };
    // Stamp each trigger with its direction (incoming/outgoing) so the triage
    // agent can apply the completion protocol to the right messages.
    let with_direction: Vec<serde_json::Value> = triggers
        .iter()
        .map(|trigger| {
            let mut trigger = trigger.clone();
            let is_outgoing = trigger
                .get("payload")
                .and_then(|p| p.get("is_outgoing"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if let Some(obj) = trigger.as_object_mut() {
                obj.insert(
                    "direction".to_string(),
                    serde_json::Value::String(
                        if is_outgoing { "outgoing" } else { "incoming" }.to_string(),
                    ),
                );
            }
            trigger
        })
        .collect();
    let trigger = if with_direction.len() == 1 {
        serde_json::to_string_pretty(&with_direction[0])?
    } else {
        serde_json::to_string_pretty(&with_direction)?
    };
    let digest_guidance = if triggers.len() == 1 {
        String::new()
    } else {
        "\n\nMonitor digest policy:\n- Treat the triggers below as one conversation/time-window, not as independent task requests.\n- First infer the conversation-level intent, then create only consolidated tasks that still require attention.\n- Treat greetings, acknowledgements, casual chatter, and informational messages as context/noise unless they change the next action.\n- Do not create a separate task for each short question or each individual message.\n- Task subjects must be imperative next actions, not topic summaries. Prefer \"Review PR #404 and decide the triage approach\" over \"PR #404 discussion\".\n- Task descriptions must explain why the action is needed, cite the source request/context, and state the concrete next step.\n- When creating a task, copy the most relevant trigger's `envelope_id` into `metadata.monitor_envelope_id` so source grounding can be attached.\n- If one consolidated task depends on multiple triggers, also copy every contributing trigger `envelope_id` into `metadata.monitor_envelope_ids` as a JSON array. Do not include unrelated/noise triggers in that array."
            .to_string()
    };
    let no_task_diagnostics = "\n\nNo-task diagnostic output:\n- Diagnostic only: do not change task creation, update, or completion decisions to satisfy this output.\n- After any task tool calls and your normal answer, append a compact block only for triggers that did not create, update, or complete a task.\n- The block must start with `NO_TASK_DECISIONS:` and contain a JSON array. Each item must use the trigger's exact `envelope_id`, `outcome: \"no_task\"`, a short `policy`, and a short `reason`.\n- Include only no_task items; do not include successful task-created/task-updated triggers.\n- Keep `reason` under 160 Chinese characters or 240 total characters. Do not copy the full source message.\nExample:\nNO_TASK_DECISIONS:\n```json\n[{\"envelope_id\":\"env-123\",\"outcome\":\"no_task\",\"policy\":\"status_update_no_action\",\"reason\":\"这是状态通知，没有请求你处理。\"}]\n```";
    Ok(format!(
        "{prompt}{digest_guidance}{no_task_diagnostics}\n\n{trigger_label}:\n```json\n{trigger}\n```"
    ))
}

fn extract_no_task_decisions(text: &str) -> Vec<TriageDecision> {
    let Some(json_text) = no_task_decision_json_text(text) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(json_text.trim()) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items.iter().filter_map(parse_no_task_decision).collect()
}

fn no_task_decision_json_text(text: &str) -> Option<&str> {
    let after_marker = text.split_once(NO_TASK_DECISION_MARKER)?.1.trim_start();
    let Some(fenced) = after_marker.strip_prefix("```") else {
        return Some(after_marker.trim());
    };
    let body_start = fenced.find('\n').map(|index| index + 1).unwrap_or(0);
    let body = &fenced[body_start..];
    let body = body
        .split_once("\n```")
        .map(|(body, _)| body)
        .or_else(|| body.split_once("```").map(|(body, _)| body))
        .unwrap_or(body);
    Some(body.trim())
}

fn parse_no_task_decision(value: &Value) -> Option<TriageDecision> {
    let outcome = value
        .get("outcome")
        .and_then(Value::as_str)
        .map(str::trim)?;
    if outcome != "no_task" {
        return None;
    }
    let envelope_id = value
        .get("envelope_id")
        .and_then(Value::as_str)
        .and_then(|value| normalize_capped(value, usize::MAX))?;
    let reason = value
        .get("reason")
        .and_then(Value::as_str)
        .and_then(|value| normalize_capped(value, MAX_TRIAGE_REASON_CHARS))?;
    let policy = value
        .get("policy")
        .and_then(Value::as_str)
        .and_then(|value| normalize_capped(value, MAX_TRIAGE_POLICY_CHARS));
    Some(TriageDecision {
        envelope_id,
        outcome: TriageDecisionOutcome::NoTask,
        policy,
        reason,
    })
}

fn load_monitor_task_trace_values(paths: &ConfigPaths) -> Result<Vec<Value>> {
    let path = monitor_task_store_path(paths);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()))
        }
    };
    let value: Value =
        serde_json::from_str(&raw).with_context(|| format!("invalid {}", path.display()))?;
    if let Some(tasks) = value.as_array() {
        return Ok(tasks.clone());
    }
    Ok(value
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn monitor_task_trace_decisions(
    before: &[Value],
    after: &[Value],
    triggers: &[Value],
) -> Vec<TriageDecision> {
    let trigger_envelopes: HashSet<String> = triggers
        .iter()
        .filter_map(|trigger| value_string(trigger, &["envelope_id"]))
        .collect();
    if trigger_envelopes.is_empty() {
        return Vec::new();
    }
    let before_by_id: HashMap<String, &Value> = before
        .iter()
        .filter_map(|task| task_identifier(task).map(|task_id| (task_id, task)))
        .collect();
    let mut decisions = Vec::new();
    let mut seen_envelopes = HashSet::new();
    for task in after {
        let envelope_ids: Vec<String> = task_envelope_ids(task)
            .into_iter()
            .filter(|envelope_id| trigger_envelopes.contains(envelope_id))
            .collect();
        if envelope_ids.is_empty() {
            continue;
        };
        let Some(task_id) = task_identifier(task) else {
            continue;
        };
        let outcome = match before_by_id.get(&task_id) {
            None => TriageDecisionOutcome::TaskCreated,
            Some(previous) if *previous != task => TriageDecisionOutcome::TaskUpdated,
            Some(_) => continue,
        };
        let verb = match outcome {
            TriageDecisionOutcome::TaskCreated => "Created",
            TriageDecisionOutcome::TaskUpdated => "Updated",
            TriageDecisionOutcome::NoTask => "Processed",
        };
        for envelope_id in envelope_ids {
            if !seen_envelopes.insert(envelope_id.clone()) {
                continue;
            }
            decisions.push(TriageDecision {
                envelope_id,
                outcome,
                policy: Some("monitor_task_store_diff".to_string()),
                reason: format!("{verb} monitor task {task_id}."),
            });
        }
    }
    decisions
}

fn task_identifier(task: &Value) -> Option<String> {
    value_string(task, &["task_id", "taskId", "id"])
}

fn task_envelope_ids(task: &Value) -> Vec<String> {
    let mut envelope_ids = Vec::new();
    for envelope_id in
        task_strings_in_metadata(task, &["monitor_envelope_ids", "monitorEnvelopeIds"])
    {
        push_unique_string(&mut envelope_ids, envelope_id);
    }
    if let Some(envelope_id) =
        task_string_in_metadata(task, &["monitor_envelope_id", "monitorEnvelopeId"])
    {
        push_unique_string(&mut envelope_ids, envelope_id);
    }
    envelope_ids
}

fn task_string_in_metadata(task: &Value, keys: &[&str]) -> Option<String> {
    value_string(task, keys).or_else(|| {
        task.get("metadata")
            .and_then(|metadata| value_string(metadata, keys))
    })
}

fn task_strings_in_metadata(task: &Value, keys: &[&str]) -> Vec<String> {
    let mut values = value_strings(task, keys);
    if let Some(metadata) = task.get("metadata") {
        values.extend(value_strings(metadata, keys));
    }
    values
}

fn value_string(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        let Some(value) = value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        return Some(value.to_string());
    }
    None
}

fn value_strings(value: &Value, keys: &[&str]) -> Vec<String> {
    let mut values = Vec::new();
    for key in keys {
        let Some(array) = value.get(*key).and_then(Value::as_array) else {
            continue;
        };
        for item in array {
            if let Some(value) = item
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                push_unique_string(&mut values, value.to_string());
            }
        }
    }
    values
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn normalize_capped(value: &str, max_chars: usize) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    Some(normalized.chars().take(max_chars).collect())
}

fn enrich_monitor_trigger_context(paths: &ConfigPaths, mut trigger: Value) -> Result<Value> {
    let Some(connection_id) = trigger
        .get("connection_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(trigger);
    };
    if !connection_id.contains("telegram") || !safe_path_component(connection_id) {
        return Ok(trigger);
    }
    let payload = match trigger.get("payload").and_then(Value::as_object) {
        Some(payload) => payload,
        None => return Ok(trigger),
    };
    let Some(chat_id) = payload.get("chat_id").and_then(Value::as_i64) else {
        return Ok(trigger);
    };
    if payload
        .get("chat_kind")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "user")
    {
        return Ok(trigger);
    }
    let current_message_id = payload.get("message_id").and_then(Value::as_i64);
    let current_date_ms = payload.get("date_ms").and_then(Value::as_i64);
    let history_path = paths
        .user_config_dir
        .join("telegram-accounts")
        .join(connection_id)
        .join("telegram-history-cache.json");
    let messages = match read_prior_telegram_history_cache_messages(
        &history_path,
        chat_id,
        current_message_id,
        current_date_ms,
        TELEGRAM_CONVERSATION_CONTEXT_LIMIT,
    ) {
        Ok(messages) => messages,
        Err(error) => {
            tracing::warn!(
                %error,
                connection_id,
                chat_id,
                "failed to read Telegram history cache for monitor context; continuing without cached context"
            );
            Vec::new()
        }
    };
    if !messages.is_empty() {
        let context = json!({
            "kind": "telegram_prior_messages",
            "source": "telegram_server_history_cache",
            "continuity": "bounded_server_history",
            "scope": "same_chat_before_current_message",
            "limit": TELEGRAM_CONVERSATION_CONTEXT_LIMIT,
            "note": "bounded recent Telegram server history in this same chat before the current trigger, maintained by the Telegram subscriber. Use it to disambiguate the current source message; the current trigger text remains authoritative for creating/updating the task.",
            "messages": messages,
        });
        if let Some(payload) = trigger.get_mut("payload").and_then(Value::as_object_mut) {
            payload.insert("conversation_context".to_string(), context);
        }
        return Ok(trigger);
    }
    let diagnostics_path = paths
        .user_config_dir
        .join("telegram-accounts")
        .join(connection_id)
        .join("message-diagnostics.ndjson");
    let messages = match read_prior_telegram_context_messages(
        &diagnostics_path,
        chat_id,
        current_message_id,
        current_date_ms,
        TELEGRAM_CONVERSATION_CONTEXT_LIMIT,
    ) {
        Ok(messages) => messages,
        Err(error) => {
            tracing::warn!(
                %error,
                connection_id,
                chat_id,
                "failed to read Telegram diagnostics for monitor context; continuing without diagnostics context"
            );
            Vec::new()
        }
    };
    if messages.is_empty() {
        return Ok(trigger);
    }
    let context = json!({
        "kind": "telegram_prior_messages",
        "source": "subscriber_diagnostics",
        "continuity": "unknown",
        "scope": "same_chat_before_current_message",
        "limit": TELEGRAM_CONVERSATION_CONTEXT_LIMIT,
        "note": "Recent Telegram messages in this same chat before the current trigger, as observed by the subscriber. This may be partial if the subscriber was offline; use it only to disambiguate context, while the current trigger text remains authoritative for creating/updating the task.",
        "messages": messages,
    });
    if let Some(payload) = trigger.get_mut("payload").and_then(Value::as_object_mut) {
        payload.insert("conversation_context".to_string(), context);
    }
    Ok(trigger)
}

fn safe_path_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn read_prior_telegram_history_cache_messages(
    path: &Path,
    chat_id: i64,
    current_message_id: Option<i64>,
    current_date_ms: Option<i64>,
    limit: usize,
) -> Result<Vec<Value>> {
    if limit == 0 || !path.exists() {
        return Ok(Vec::new());
    }
    let cache = TelegramHistoryCache::load_path(path)?;
    cache
        .prior_context_messages(chat_id, current_message_id, current_date_ms, limit)
        .into_iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("serialize Telegram history context messages")
}

fn read_prior_telegram_context_messages(
    path: &Path,
    chat_id: i64,
    current_message_id: Option<i64>,
    current_date_ms: Option<i64>,
    limit: usize,
) -> Result<Vec<Value>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut candidates = Vec::new();
    for path in telegram_context_diagnostic_paths(path) {
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(payload) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if !telegram_context_stage_is_relevant(&payload) {
                continue;
            }
            if payload.get("chat_id").and_then(Value::as_i64) != Some(chat_id) {
                continue;
            }
            if payload.get("chat_kind").and_then(Value::as_str) != Some("user") {
                continue;
            }
            let message_id = payload.get("message_id").and_then(Value::as_i64);
            if message_id.is_some() && message_id == current_message_id {
                continue;
            }
            let date_ms = payload.get("date_ms").and_then(Value::as_i64).unwrap_or(0);
            if !telegram_context_message_precedes_current(
                date_ms,
                message_id,
                current_date_ms,
                current_message_id,
            ) {
                continue;
            }
            let Some(text) = payload
                .get("text_prefix")
                .or_else(|| payload.get("text"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let is_outgoing = payload
                .get("is_outgoing")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| {
                    payload.get("stage").and_then(Value::as_str) == Some("suppressed_outgoing")
                });
            let direction = if is_outgoing { "outgoing" } else { "incoming" };
            let from = if is_outgoing { "me" } else { "them" };
            let sender_label = telegram_context_sender_label(&payload, is_outgoing);
            let chat_title = payload
                .get("chat_title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let sender_username = payload
                .get("sender_username")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            candidates.push((
                date_ms,
                message_id.unwrap_or_default(),
                json!({
                    "from": from,
                    "direction": direction,
                    "sender": {
                        "label": sender_label,
                        "username": sender_username,
                        "is_user": is_outgoing,
                    },
                    "chat": {
                        "id": chat_id,
                        "title": chat_title,
                    },
                    "message_id": message_id,
                    "date_ms": date_ms,
                    "ts": date_ms,
                    "text": text,
                }),
            ));
        }
    }
    candidates.sort_by_key(|(date_ms, message_id, _)| (*date_ms, *message_id));
    let start = candidates.len().saturating_sub(limit);
    Ok(candidates
        .into_iter()
        .skip(start)
        .map(|(_, _, value)| value)
        .collect())
}

fn telegram_context_diagnostic_paths(path: &Path) -> [PathBuf; 2] {
    [
        PathBuf::from(format!("{}.1", path.display())),
        path.to_path_buf(),
    ]
}

fn telegram_context_stage_is_relevant(payload: &Value) -> bool {
    matches!(
        payload.get("stage").and_then(Value::as_str),
        Some("emitted" | "suppressed_outgoing")
    )
}

fn telegram_context_message_precedes_current(
    date_ms: i64,
    message_id: Option<i64>,
    current_date_ms: Option<i64>,
    current_message_id: Option<i64>,
) -> bool {
    if let Some(current_date_ms) = current_date_ms {
        return date_ms < current_date_ms
            || (date_ms == current_date_ms
                && message_id
                    .zip(current_message_id)
                    .is_some_and(|(left, right)| left < right));
    }
    match message_id.zip(current_message_id) {
        Some((left, right)) => left < right,
        None => true,
    }
}

fn telegram_context_sender_label(payload: &Value, is_outgoing: bool) -> String {
    payload
        .get("sender_name")
        .or_else(|| payload.get("chat_title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            if is_outgoing {
                "me".to_string()
            } else {
                "sender".to_string()
            }
        })
}

/// Tools withheld from the model-visible list on outgoing triage turns.
/// On outgoing turns the agent must only complete or update existing tasks;
/// creating new tasks from the user's own messages is not permitted.
const TRIAGE_OUTGOING_EXCLUDED_TOOLS: &[&str] = &["TaskCreate"];

/// Returns the tool names that the triage agent would see for a given
/// message direction. Incoming turns expose the full tool set; outgoing
/// turns suppress `TaskCreate` via [`TRIAGE_OUTGOING_EXCLUDED_TOOLS`] so
/// the agent can only complete or update tasks, not create new ones.
///
/// This helper is the single source of truth for the exclusion logic — it
/// is called both from tests and from `run_task_agent_prompt_for_session`,
/// so the two cannot drift apart.
fn triage_excluded_tools_for_direction(is_outgoing: bool) -> Vec<String> {
    if is_outgoing {
        TRIAGE_OUTGOING_EXCLUDED_TOOLS
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    }
}

/// Returns the tool NAMES that the triage turn would expose for the given
/// direction. Derives the excluded set from the real exclusion logic
/// (`triage_excluded_tools_for_direction`) rather than a separate list.
/// Used by unit tests to assert direction-aware tool visibility without
/// requiring a live LLM call.
#[cfg(test)]
fn triage_turn_tool_names(is_outgoing: bool) -> Vec<String> {
    // The test workspace does not have bundled resources loaded, so we
    // use a representative base list that includes the task tools the
    // triage agent actually uses. This mirrors the real code path:
    // `TaskCreate`, `TaskList`, and `TaskUpdate` are always loaded from
    // resources/tools at runtime; the exclusion is applied on top.
    let base_tool_names: Vec<String> = vec![
        "TaskCreate".to_string(),
        "TaskList".to_string(),
        "TaskUpdate".to_string(),
    ];
    let excluded = triage_excluded_tools_for_direction(is_outgoing);
    base_tool_names
        .into_iter()
        .filter(|name| !excluded.contains(name))
        .collect()
}

/// Current Unix time in milliseconds (i128, matching workflow history).
fn now_unix_ms() -> i128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i128
}

fn triage_session_key(model: Option<&str>, triggers: &[serde_json::Value]) -> String {
    let connection = triggers
        .first()
        .and_then(|trigger| trigger.get("connection_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown");
    let model = model.and_then(non_empty_trimmed).unwrap_or("default");
    format!("monitor-triage:{connection}:{model}")
}

fn monitor_source_stamp_contexts(triggers: &[serde_json::Value]) -> Vec<MonitorSourceStampContext> {
    triggers
        .iter()
        .filter_map(monitor_source_stamp_context)
        .collect()
}

fn monitor_source_stamp_context(trigger: &serde_json::Value) -> Option<MonitorSourceStampContext> {
    let envelope_id = trigger
        .get("envelope_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let connection_slug = trigger
        .get("connection_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let payload = trigger.get("payload").cloned().unwrap_or_else(|| json!({}));
    let connector_slug = connector_slug_for_trigger(trigger, &payload, connection_slug);
    Some(MonitorSourceStampContext {
        envelope_id: envelope_id.to_string(),
        connection_slug: connection_slug.to_string(),
        connector_slug,
        received_at_ms: value_i64_field(trigger, "received_at_ms")
            .or_else(|| value_i64_field(trigger, "receivedAtMs"))
            .or_else(|| value_i64_field(&payload, "date_ms")),
        text: trigger
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        payload,
    })
}

fn connector_slug_for_trigger(
    trigger: &Value,
    payload: &Value,
    connection_slug: &str,
) -> Option<String> {
    value_string(
        trigger,
        &[
            "connector_id",
            "connector_slug",
            "connectorSlug",
            "platform",
        ],
    )
    .or_else(|| {
        value_string(
            payload,
            &[
                "connector_id",
                "connector_slug",
                "connectorSlug",
                "platform",
            ],
        )
    })
    .or_else(|| {
        if connection_slug.contains("telegram") {
            Some("telegram-login".to_string())
        } else if connection_slug.contains("gmail") {
            Some("gmail-browser".to_string())
        } else if connection_slug.contains("gcal") || connection_slug.contains("calendar") {
            Some("gcal-browser".to_string())
        } else {
            None
        }
    })
}

fn monitor_task_create_gate_contexts(
    paths: &ConfigPaths,
    triggers: &[serde_json::Value],
) -> Vec<MonitorTaskCreateGateContext> {
    triggers
        .iter()
        .filter_map(|trigger| monitor_task_create_gate_context(paths, trigger))
        .collect()
}

fn monitor_task_create_gate_context(
    paths: &ConfigPaths,
    trigger: &serde_json::Value,
) -> Option<MonitorTaskCreateGateContext> {
    let connection_slug = trigger
        .get("connection_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    if !connection_slug.contains("telegram") || !safe_path_component(connection_slug) {
        return None;
    }
    let envelope_id = trigger
        .get("envelope_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let payload = trigger.get("payload")?;
    let chat_id = value_i64_field(payload, "chat_id")?;
    let chat_kind = payload
        .get("chat_kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("user");
    if chat_kind != "user" {
        return None;
    }
    let source_message_id = value_i64_field(payload, "message_id")?;
    let connector_slug = trigger
        .get("connector_id")
        .or_else(|| trigger.get("connector_slug"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| Some("telegram-login".to_string()));
    Some(MonitorTaskCreateGateContext {
        envelope_id: envelope_id.to_string(),
        connection_slug: connection_slug.to_string(),
        connector_slug,
        chat_id,
        chat_kind: chat_kind.to_string(),
        source_message_id,
        source_date_ms: value_i64_field(payload, "date_ms"),
        activity_state_path: paths
            .user_config_dir
            .join("telegram-accounts")
            .join(connection_slug)
            .join("telegram-activity-state.json"),
        monitor_trace_path: Some(paths.workspace_config_dir.join("monitor_trace.json")),
    })
}

fn value_i64_field(value: &Value, key: &str) -> Option<i64> {
    match value.get(key)? {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok())),
        Value::String(value) => value.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Stamps the trigger's verbatim source grounding onto monitor tasks created
/// for that envelope: the exact event text (`metadata.source_text`, plus
/// `source_context.text`), the source message id (`metadata.source_message_id`,
/// plus `source_context.message_id`, used to send approved replies as a Telegram
/// reply to the triggering message — agentenv/monorepo#630), and any
/// server-built recent Telegram conversation context for reply-draft quality.
/// Server-owned — the agent never writes these, so paraphrase errors can always
/// be checked against the original wording and replies thread to the right
/// message.
fn record_monitor_source_text(paths: &ConfigPaths, trigger: &serde_json::Value) -> Result<()> {
    let Some(envelope_id) = trigger
        .get("envelope_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let Some(text) = trigger
        .get("text")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(());
    };
    let message_id = trigger
        .get("payload")
        .and_then(|payload| payload.get("message_id"))
        .and_then(serde_json::Value::as_i64);
    let conversation_context = trigger
        .get("payload")
        .and_then(|payload| payload.get("conversation_context"))
        .cloned();
    let derived_source_context = derive_monitor_source_context_from_trigger(trigger);
    let path = monitor_task_store_path(paths);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()))
        }
    };
    let mut store: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid monitor task store {}", path.display()))?;
    let Some(tasks) = store
        .get_mut("tasks")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(());
    };
    let mut changed = false;
    for task in tasks.iter_mut() {
        let Some(metadata) = task
            .get_mut("metadata")
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };
        let task_envelope = metadata
            .get("monitor_envelope_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim);
        if task_envelope != Some(envelope_id) {
            continue;
        }
        // `monitor_envelope_id` tracks the task's CURRENT source message —
        // triage's TaskUpdate re-points it when a follow-up supersedes the
        // original request (e.g. the same metric reported again with new
        // numbers). Keep the verbatim text aligned with that envelope:
        // overwriting is safe because we only ever stamp the text belonging
        // to the envelope the task currently references.
        if metadata
            .get("source_text")
            .and_then(serde_json::Value::as_str)
            != Some(text)
        {
            metadata.insert(
                "source_text".to_string(),
                serde_json::Value::String(text.to_string()),
            );
            changed = true;
        }
        if let Some(message_id) = message_id {
            if metadata
                .get("source_message_id")
                .and_then(serde_json::Value::as_i64)
                != Some(message_id)
            {
                metadata.insert(
                    "source_message_id".to_string(),
                    serde_json::Value::from(message_id),
                );
                changed = true;
            }
        }
        if metadata.get("source_context").is_none() {
            if let Some(context) = derived_source_context.as_ref() {
                metadata.insert("source_context".to_string(), context.clone());
                changed = true;
            }
        }
        if let Some(context) = metadata
            .get_mut("source_context")
            .and_then(serde_json::Value::as_object_mut)
        {
            if context.get("text").and_then(serde_json::Value::as_str) != Some(text) {
                context.insert(
                    "text".to_string(),
                    serde_json::Value::String(text.to_string()),
                );
                changed = true;
            }
            if let Some(message_id) = message_id {
                if context
                    .get("message_id")
                    .and_then(serde_json::Value::as_i64)
                    != Some(message_id)
                {
                    context.insert(
                        "message_id".to_string(),
                        serde_json::Value::from(message_id),
                    );
                    changed = true;
                }
            }
            match conversation_context.as_ref() {
                Some(conversation_context) => {
                    if context.get("conversation_context") != Some(conversation_context) {
                        context.insert(
                            "conversation_context".to_string(),
                            conversation_context.clone(),
                        );
                        changed = true;
                    }
                    if let Some(messages) = conversation_context.get("messages") {
                        if context.get("context_messages") != Some(messages) {
                            context.insert("context_messages".to_string(), messages.clone());
                            changed = true;
                        }
                    }
                }
                None => {
                    if context.remove("conversation_context").is_some() {
                        changed = true;
                    }
                    if context.remove("context_messages").is_some() {
                        changed = true;
                    }
                }
            }
        }
    }
    if changed {
        std::fs::write(&path, serde_json::to_string_pretty(&store)?)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

fn derive_monitor_source_context_from_trigger(trigger: &Value) -> Option<Value> {
    let payload = trigger.get("payload")?;
    let connector_slug = value_string(trigger, &["connector_slug", "connectorSlug"])
        .or_else(|| value_string(payload, &["connector_slug", "connectorSlug", "platform"]))?;
    if connector_slug != "gmail-browser" {
        return None;
    }
    let message = payload.get("message").unwrap_or(payload);
    let sender_email = value_string(
        message,
        &[
            "fromEmail",
            "from_email",
            "senderEmail",
            "sender_email",
            "email",
        ],
    )
    .or_else(|| {
        value_string(
            payload,
            &[
                "fromEmail",
                "from_email",
                "senderEmail",
                "sender_email",
                "email",
            ],
        )
    })?;
    let sender_name = value_string(message, &["sender", "senderName", "fromName", "name"])
        .or_else(|| value_string(payload, &["sender", "senderName", "fromName", "name"]));
    let account = value_string(payload, &["account", "account_email", "accountEmail"]);
    let subject = value_string(message, &["subject"]);
    let thread_id = value_string(
        message,
        &[
            "threadId",
            "thread_id",
            "legacyThreadId",
            "legacy_thread_id",
            "gmailThreadId",
            "gmail_thread_id",
        ],
    );
    let url = value_string(message, &["url", "href"]);
    let connection_slug = value_string(
        trigger,
        &["connection_id", "connection_slug", "connectionSlug"],
    )
    .or_else(|| {
        value_string(
            payload,
            &["connection_id", "connection_slug", "connectionSlug"],
        )
    });

    let mut sender = serde_json::Map::new();
    sender.insert("email".to_string(), Value::String(sender_email.clone()));
    if let Some(name) = sender_name {
        sender.insert("name".to_string(), Value::String(name));
    }

    let mut delivery_target = serde_json::Map::new();
    delivery_target.insert(
        "type".to_string(),
        Value::String("gmail_message".to_string()),
    );
    if let Some(account) = account {
        delivery_target.insert("account".to_string(), Value::String(account));
    }
    if let Some(thread_id) = thread_id {
        delivery_target.insert("thread_id".to_string(), Value::String(thread_id));
    }
    if let Some(url) = url {
        delivery_target.insert("url".to_string(), Value::String(url));
    }

    let mut context = serde_json::Map::new();
    context.insert(
        "kind".to_string(),
        Value::String("gmail_message".to_string()),
    );
    context.insert(
        "platform".to_string(),
        Value::String(connector_slug.clone()),
    );
    context.insert("connector_slug".to_string(), Value::String(connector_slug));
    if let Some(connection_slug) = connection_slug {
        context.insert(
            "connection_slug".to_string(),
            Value::String(connection_slug),
        );
    }
    context.insert(
        "summary".to_string(),
        Value::String(format!("Gmail message from {sender_email}")),
    );
    if let Some(subject) = subject {
        context.insert("subject".to_string(), Value::String(subject));
    }
    context.insert("sender".to_string(), Value::Object(sender));
    context.insert(
        "delivery_target".to_string(),
        Value::Object(delivery_target),
    );
    Some(Value::Object(context))
}

fn monitor_task_store_path(paths: &ConfigPaths) -> PathBuf {
    paths
        .workspace_config_dir
        .join("runtime")
        .join("claude_workflow")
        .join("monitor_tasks.json")
}

struct PufferAgentExecutor {
    paths: ConfigPaths,
    config: PufferConfig,
    resources: LoadedResources,
    providers: ProviderRegistry,
    auth_store: AuthStore,
}

impl AgentExecutor for PufferAgentExecutor {
    fn execute(&mut self, context: ExecutionContext) -> Result<AgentExecution> {
        let session_store = SessionStore::from_paths(&self.paths)?;
        let cwd = context
            .working_dir
            .as_ref()
            .map(PathBuf::from)
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    self.paths.workspace_root.join(path)
                }
            })
            .unwrap_or_else(|| self.paths.workspace_root.clone());
        let session = session_store
            .create_session_with_tags(cwd.clone(), vec![BACKGROUND_SESSION_TAG.to_string()])?;
        let mut state = AppState::new(self.config.clone(), cwd, session);
        if let Some(model) = context.model.as_deref().and_then(non_empty_trimmed) {
            apply_explicit_model(&mut state, model);
        } else {
            apply_authenticated_provider_fallback(&mut state, &self.providers, &self.auth_store);
        }
        let prompt = if let Some(agent) = context.agent {
            format!(
                "Run as Puffer workflow agent `{agent}`.\n\n{}",
                context.prompt
            )
        } else {
            context.prompt
        };
        let output = execute_user_turn_streaming(
            &mut state,
            &self.resources,
            &self.providers,
            &mut self.auth_store,
            &prompt,
            |_| {},
        )?;
        Ok(AgentExecution {
            output: output.assistant_text,
        })
    }
}

fn cron_loop(runner: Arc<ProcessWorkflowRunner>, stop: Arc<std::sync::atomic::AtomicBool>) {
    let mut deduper = CronDeduper::default();
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        if let Err(error) = poll_cron(&runner, &mut deduper) {
            eprintln!("workflow cron poll failed: {error:#}");
        }
        for _ in 0..30 {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }
}

fn poll_cron(runner: &ProcessWorkflowRunner, deduper: &mut CronDeduper) -> Result<()> {
    let now = local_now();
    let minute_epoch = now.unix_timestamp() / 60;
    let minute = now.minute() as u32;
    let hour = now.hour() as u32;
    let day = now.day() as u32;
    let month = u8::from(now.month()) as u32;
    let weekday = now.weekday().number_days_from_sunday() as u32;
    let store = WorkflowStore::new(&runner.paths.workspace_config_dir);
    for definition in store.list()? {
        if !definition.enabled {
            continue;
        }
        let TriggerSpec::Cron { cron } = &definition.trigger else {
            continue;
        };
        if CronExpression::parse(cron)?.matches(minute, hour, day, month, weekday)
            && deduper.mark_if_new(&definition.slug, minute_epoch)
        {
            let trigger_key = format!("cron:{}:{minute_epoch}", definition.slug);
            let trigger = json!({
                "type": "cron",
                "cron": cron,
                "scheduled_minute_epoch": minute_epoch,
            });
            let _guard = runner.lock.lock().unwrap();
            let snapshot = runner.runtime_snapshot();
            let run = DagRunner::new(
                &store,
                PufferAgentExecutor {
                    paths: runner.paths.clone(),
                    config: snapshot.config,
                    resources: runner.resources.clone(),
                    providers: snapshot.providers,
                    auth_store: snapshot.auth_store,
                },
            )
            .run(&definition, trigger, Some(trigger_key))?;
            eprintln!(
                "workflow cron fired `{}` as run #{} {:?}",
                definition.slug, run.idx, run.status
            );
        }
    }
    Ok(())
}

fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn apply_explicit_model(state: &mut AppState, model: &str) {
    state.current_model = Some(model.to_string());
    if let Some((provider_id, _)) = model.split_once('/') {
        state.current_provider = Some(canonical_provider_id(provider_id));
    }
}

fn apply_authenticated_provider_fallback(
    state: &mut AppState,
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) {
    let active_provider = selected_provider_id(state, providers);
    if active_provider
        .as_deref()
        .is_some_and(|provider_id| provider_has_auth(auth_store, provider_id))
    {
        return;
    }

    let Some(fallback) = authenticated_fallback_provider(auth_store, providers) else {
        return;
    };
    state.current_provider = Some(fallback.clone());
    state.config.default_provider = Some(fallback);
    state.current_model = None;
    state.config.default_model = None;
}

fn apply_task_agent_model_default(state: &mut AppState, providers: &ProviderRegistry) {
    let Some(provider_id) = selected_provider_id(state, providers) else {
        return;
    };
    let Some(model_id) = task_agent_model_default_for_provider(&provider_id) else {
        return;
    };
    let selector = format!("{provider_id}/{model_id}");
    if providers.resolve_model(&selector).is_some() {
        apply_explicit_model(state, &selector);
    }
}

fn apply_config_provider_overrides(providers: &mut ProviderRegistry, config: &PufferConfig) {
    providers.apply_openai_base_url_override(config.openai_base_url.as_deref());
    if let Some(display_name) = config.openai_display_name.as_deref() {
        providers.set_openai_display_name(display_name);
    }
    if !config.openai_headers.is_empty() {
        providers.set_openai_headers(
            config
                .openai_headers
                .clone()
                .into_iter()
                .collect::<indexmap::IndexMap<_, _>>(),
        );
    }
    if !config.openai_query_params.is_empty() {
        providers.set_openai_query_params(
            config
                .openai_query_params
                .clone()
                .into_iter()
                .collect::<indexmap::IndexMap<_, _>>(),
        );
    }
}

fn task_agent_model_default_for_provider(provider_id: &str) -> Option<&'static str> {
    match canonical_provider_id(provider_id).as_str() {
        "openai" => Some(OPENAI_TASK_AGENT_MODEL),
        "anthropic" => Some(ANTHROPIC_TASK_AGENT_MODEL),
        _ => None,
    }
}

fn selected_provider_id(state: &AppState, providers: &ProviderRegistry) -> Option<String> {
    state
        .current_model
        .as_deref()
        .and_then(|model| {
            selected_provider_id_for_model(model, state.current_provider.as_deref(), providers)
        })
        .or_else(|| state.current_provider.as_deref().map(canonical_provider_id))
}

fn selected_provider_id_for_model(
    model: &str,
    current_provider: Option<&str>,
    providers: &ProviderRegistry,
) -> Option<String> {
    if let Some(model) = providers.resolve_model(model) {
        return Some(canonical_provider_id(&model.provider));
    }
    if let Some(provider_id) = current_provider {
        if let Some(provider) = providers.provider(provider_id) {
            if provider
                .models
                .iter()
                .any(|descriptor| descriptor.id == model)
            {
                return Some(provider.id.clone());
            }
        }
    }
    providers
        .providers()
        .find(|provider| {
            provider
                .models
                .iter()
                .any(|descriptor| descriptor.id == model)
        })
        .map(|provider| provider.id.clone())
}

fn provider_has_auth(auth_store: &AuthStore, provider_id: &str) -> bool {
    let canonical = canonical_provider_id(provider_id);
    auth_store.has_auth(&canonical)
}

fn authenticated_fallback_provider(
    auth_store: &AuthStore,
    providers: &ProviderRegistry,
) -> Option<String> {
    ["openai", "anthropic"]
        .into_iter()
        .find_map(|provider_id| {
            (provider_has_auth(auth_store, provider_id))
                .then(|| {
                    providers
                        .provider(provider_id)
                        .map(|provider| provider.id.clone())
                })
                .flatten()
        })
        .or_else(|| {
            auth_store.provider_ids().find_map(|provider_id| {
                providers
                    .provider(provider_id)
                    .map(|provider| provider.id.clone())
            })
        })
}

fn local_now() -> OffsetDateTime {
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    OffsetDateTime::now_utc().to_offset(offset)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_authenticated_provider_fallback, apply_task_agent_model_default,
        authenticated_fallback_provider, extract_no_task_decisions, render_triage_batch_prompt,
        selected_provider_id_for_model, triage_session_key, AuthStore, ProviderRegistry,
    };
    use puffer_config::PufferConfig;
    use puffer_provider_registry::{AuthMode, Modality, ModelDescriptor, ProviderDescriptor};
    use puffer_resources::LoadedResources;
    use puffer_session_store::SessionMetadata;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use uuid::Uuid;

    fn provider(id: &str, model_id: &str) -> ProviderDescriptor {
        provider_with_models(id, &[model_id])
    }

    fn provider_with_models(id: &str, model_ids: &[&str]) -> ProviderDescriptor {
        ProviderDescriptor {
            id: id.to_string(),
            display_name: id.to_string(),
            base_url: "https://example.test".to_string(),
            default_api: if id == "anthropic" {
                "anthropic-messages".to_string()
            } else {
                "openai-responses".to_string()
            },
            auth_modes: vec![AuthMode::ApiKey],
            headers: Default::default(),
            query_params: Default::default(),
            chat_completions_path: None,
            discovery: None,
            media: None,
            models: model_ids
                .iter()
                .map(|model_id| ModelDescriptor {
                    id: (*model_id).to_string(),
                    display_name: (*model_id).to_string(),
                    provider: id.to_string(),
                    api: if id == "anthropic" {
                        "anthropic-messages".to_string()
                    } else {
                        "openai-responses".to_string()
                    },
                    context_window: 100_000,
                    max_output_tokens: 8_192,
                    supports_reasoning: false,
                    input: vec![Modality::Text],
                    cost: None,
                    compat: None,
                })
                .collect(),
        }
    }

    fn app_state(provider_id: &str, model: Option<&str>) -> puffer_core::AppState {
        let mut config = PufferConfig::default();
        config.default_provider = Some(provider_id.to_string());
        config.default_model = model.map(ToOwned::to_owned);
        puffer_core::AppState::new(
            config,
            PathBuf::from("/tmp/puffer-test"),
            SessionMetadata {
                id: Uuid::nil(),
                display_name: None,
                generated_title: None,
                cwd: PathBuf::from("/tmp/puffer-test"),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        )
    }

    #[test]
    fn task_agent_state_refreshes_disk_config_and_auth_after_runner_start() {
        let temp = tempfile::tempdir().unwrap();
        let _home = puffer_config::set_puffer_home_override(temp.path());
        let paths = puffer_config::ConfigPaths::discover(temp.path());
        puffer_config::ensure_workspace_dirs(&paths).unwrap();

        let mut providers = ProviderRegistry::new();
        providers.register(provider_with_models(
            "anthropic",
            &["claude-sonnet-4-5", "claude-haiku-4-5-20251001"],
        ));
        providers.register(provider_with_models("openai", &["gpt-5.5", "gpt-5.4-mini"]));

        let mut startup_config = PufferConfig::default();
        startup_config.default_provider = Some("anthropic".to_string());
        startup_config.default_model = Some("anthropic/claude-sonnet-4-5".to_string());
        let runner = super::ProcessWorkflowRunner {
            paths: paths.clone(),
            config: startup_config,
            resources: LoadedResources::default(),
            providers,
            auth_store: AuthStore::default(),
            lock: Mutex::new(()),
        };

        let mut latest_config = PufferConfig::default();
        latest_config.default_provider = Some("anthropic".to_string());
        latest_config.default_model = Some("anthropic/claude-sonnet-4-5".to_string());
        latest_config.openai_base_url = Some("https://worldrouter.test/v1".to_string());
        puffer_config::save_user_config(&paths, &latest_config).unwrap();
        let mut latest_auth = AuthStore::default();
        latest_auth.set_api_key("openai", "test-key");
        latest_auth
            .save(&paths.user_config_dir.join("auth.json"))
            .unwrap();

        let snapshot = runner.runtime_snapshot();
        let state = runner
            .new_task_app_state_with_snapshot(paths.workspace_root.clone(), None, &snapshot)
            .unwrap();

        assert!(snapshot.auth_store.get("openai").is_some());
        assert_eq!(
            state.config.openai_base_url.as_deref(),
            Some("https://worldrouter.test/v1")
        );
        assert_eq!(state.current_provider.as_deref(), Some("openai"));
        assert_eq!(state.current_model.as_deref(), Some("openai/gpt-5.4-mini"));
    }

    #[test]
    fn triage_session_key_uses_connection_and_model() {
        let triggers = vec![json!({
            "connection_id": "telegram-user",
            "text": "hello"
        })];

        assert_eq!(
            triage_session_key(Some("gpt-5.4-mini"), &triggers),
            "monitor-triage:telegram-user:gpt-5.4-mini"
        );
    }

    #[test]
    fn monitor_task_create_gate_contexts_build_for_direct_telegram_trigger() {
        let temp = tempfile::tempdir().unwrap();
        let _home = puffer_config::set_puffer_home_override(temp.path());
        let paths = puffer_config::ConfigPaths::discover(temp.path());
        let triggers = vec![json!({
            "connection_id": "telegram-user",
            "connector_id": "telegram-login",
            "envelope_id": "env-6836",
            "payload": {
                "chat_id": 42,
                "chat_kind": "user",
                "message_id": 6836,
                "date_ms": 1000
            }
        })];

        let contexts = super::monitor_task_create_gate_contexts(&paths, &triggers);

        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].envelope_id, "env-6836");
        assert_eq!(contexts[0].chat_id, 42);
        assert_eq!(contexts[0].source_message_id, 6836);
        assert_eq!(contexts[0].source_date_ms, Some(1000));
        assert!(contexts[0]
            .activity_state_path
            .ends_with("telegram-accounts/telegram-user/telegram-activity-state.json"));
    }

    #[test]
    fn monitor_task_create_gate_contexts_skip_group_telegram_trigger() {
        let temp = tempfile::tempdir().unwrap();
        let _home = puffer_config::set_puffer_home_override(temp.path());
        let paths = puffer_config::ConfigPaths::discover(temp.path());
        let triggers = vec![json!({
            "connection_id": "telegram-user",
            "envelope_id": "env-group",
            "payload": {
                "chat_id": -10042,
                "chat_kind": "group",
                "message_id": 6836
            }
        })];

        let contexts = super::monitor_task_create_gate_contexts(&paths, &triggers);

        assert!(contexts.is_empty());
    }

    #[test]
    fn render_triage_batch_prompt_renders_multiple_triggers_as_digest() {
        let triggers = vec![
            json!({"connection_id": "telegram-user", "text": "first"}),
            json!({"connection_id": "telegram-user", "text": "second"}),
        ];

        let prompt = render_triage_batch_prompt("Monitor prompt", &triggers).unwrap();

        assert!(prompt.contains("Monitor digest triggers:"));
        assert!(prompt.contains("Do not create a separate task for each short question"));
        assert!(prompt.contains("Task subjects must be imperative next actions"));
        assert!(prompt.contains("metadata.monitor_envelope_ids"));
        assert!(prompt.contains("\"first\""));
        assert!(prompt.contains("\"second\""));
    }

    #[test]
    fn render_triage_batch_prompt_renders_one_trigger() {
        let triggers = vec![json!({"connection_id": "telegram-user", "text": "first"})];

        let prompt = render_triage_batch_prompt("Monitor prompt", &triggers).unwrap();

        assert!(prompt.contains("Workflow trigger:"));
        assert!(prompt.contains("\"first\""));
        assert!(prompt.contains("\"connection_id\""));
        assert!(prompt.contains("NO_TASK_DECISIONS"));
        assert!(prompt.contains("envelope_id"));
        assert!(prompt.contains("Diagnostic only"));
        assert!(!prompt.contains("[\n  {"));
    }

    #[test]
    fn extract_no_task_decisions_parses_marked_json_block() {
        let text = r#"No task.

NO_TASK_DECISIONS:
```json
[
  {
    "envelope_id": "env-1",
    "outcome": "no_task",
    "policy": "status_update_no_action",
    "reason": "这是状态通知，没有请求你回复、决策或处理。"
  },
  {
    "envelope_id": "env-2",
    "outcome": "task_created",
    "policy": "action_required",
    "reason": "ignored because MVP only records no_task"
  }
]
```"#;

        let decisions = extract_no_task_decisions(text);

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].envelope_id, "env-1");
        assert_eq!(
            decisions[0].outcome,
            puffer_subscriptions::TriageDecisionOutcome::NoTask
        );
        assert_eq!(
            decisions[0].policy.as_deref(),
            Some("status_update_no_action")
        );
        assert_eq!(
            decisions[0].reason,
            "这是状态通知，没有请求你回复、决策或处理。"
        );
    }

    #[test]
    fn monitor_task_trace_decisions_detects_created_and_updated_tasks() {
        let before = vec![json!({
            "task_id": "monitor-2",
            "subject": "旧任务",
            "metadata": { "monitor_envelope_id": "env-old" }
        })];
        let after = vec![
            json!({
                "task_id": "monitor-7",
                "subject": "18:00 前给结论",
                "metadata": { "monitor_envelope_id": "env-1" }
            }),
            json!({
                "task_id": "monitor-2",
                "subject": "19:00 前给结论",
                "metadata": { "monitor_envelope_id": "env-2" }
            }),
            json!({
                "task_id": "monitor-9",
                "subject": "非本次触发",
                "metadata": { "monitor_envelope_id": "env-other" }
            }),
        ];
        let triggers = vec![
            json!({ "envelope_id": "env-1" }),
            json!({ "envelope_id": "env-2" }),
            json!({ "envelope_id": "env-3" }),
        ];

        let decisions = super::monitor_task_trace_decisions(&before, &after, &triggers);

        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].envelope_id, "env-1");
        assert_eq!(
            decisions[0].outcome,
            puffer_subscriptions::TriageDecisionOutcome::TaskCreated
        );
        assert_eq!(decisions[1].envelope_id, "env-2");
        assert_eq!(
            decisions[1].outcome,
            puffer_subscriptions::TriageDecisionOutcome::TaskUpdated
        );
    }

    #[test]
    fn monitor_task_trace_decisions_fans_out_digest_task_envelopes() {
        let before = Vec::new();
        let after = vec![json!({
            "task_id": "monitor-7",
            "subject": "18:00 前给结论",
            "metadata": {
                "monitor_envelope_id": "env-3",
                "monitor_envelope_ids": ["env-1", "env-2", "env-3", "env-other"]
            }
        })];
        let triggers = vec![
            json!({ "envelope_id": "env-1" }),
            json!({ "envelope_id": "env-2" }),
            json!({ "envelope_id": "env-3" }),
            json!({ "envelope_id": "env-noise" }),
        ];

        let decisions = super::monitor_task_trace_decisions(&before, &after, &triggers);

        let envelopes: Vec<&str> = decisions
            .iter()
            .map(|decision| decision.envelope_id.as_str())
            .collect();
        assert_eq!(envelopes, vec!["env-1", "env-2", "env-3"]);
        assert!(decisions.iter().all(|decision| {
            decision.outcome == puffer_subscriptions::TriageDecisionOutcome::TaskCreated
        }));
    }

    #[test]
    fn enrich_monitor_trigger_context_adds_prior_telegram_chat_messages() {
        let temp = tempfile::tempdir().unwrap();
        let _home = puffer_config::set_puffer_home_override(temp.path());
        let paths = puffer_config::ConfigPaths::discover(temp.path());
        let account_dir = paths
            .user_config_dir
            .join("telegram-accounts")
            .join("telegram-user");
        std::fs::create_dir_all(&account_dir).unwrap();
        std::fs::write(
            account_dir.join("message-diagnostics.ndjson.1"),
            json!({
                "stage": "emitted",
                "chat_id": 42,
                "chat_kind": "user",
                "chat_title": "Chaofan 10-23",
                "sender_name": "Chaofan",
                "message_id": 9,
                "date_ms": 500,
                "text_prefix": "报价那件事今天定",
                "is_outgoing": false
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            account_dir.join("message-diagnostics.ndjson"),
            [
                json!({
                    "stage": "emitted",
                    "chat_id": 42,
                    "chat_kind": "user",
                    "chat_title": "Chaofan 10-23",
                    "sender_name": "Chaofan",
                    "message_id": 10,
                    "date_ms": 1_000,
                    "text_prefix": "上次说的报价发你了",
                    "is_outgoing": false
                })
                .to_string(),
                json!({
                    "stage": "suppressed_outgoing",
                    "chat_id": 42,
                    "chat_kind": "user",
                    "chat_title": "Chaofan 10-23",
                    "sender_name": "Me",
                    "message_id": 11,
                    "date_ms": 2_000,
                    "text_prefix": "我晚点看，周一聊细节",
                    "is_outgoing": true
                })
                .to_string(),
                json!({
                    "stage": "emitted",
                    "chat_id": 42,
                    "chat_kind": "user",
                    "chat_title": "Chaofan 10-23",
                    "sender_name": "Chaofan",
                    "message_id": 12,
                    "date_ms": 3_000,
                    "text_prefix": "聊下？",
                    "is_outgoing": false
                })
                .to_string(),
                json!({
                    "stage": "emitted",
                    "chat_id": 99,
                    "chat_kind": "user",
                    "chat_title": "Other",
                    "sender_name": "Other",
                    "message_id": 4,
                    "date_ms": 2_500,
                    "text_prefix": "别的聊天",
                    "is_outgoing": false
                })
                .to_string(),
            ]
            .join("\n"),
        )
        .unwrap();
        let trigger = json!({
            "connection_id": "telegram-user",
            "text": "聊下？",
            "payload": {
                "chat_id": 42,
                "chat_kind": "user",
                "chat_title": "Chaofan 10-23",
                "message_id": 12,
                "date_ms": 3_000
            }
        });

        let enriched = super::enrich_monitor_trigger_context(&paths, trigger).unwrap();
        let context = enriched
            .pointer("/payload/conversation_context")
            .and_then(serde_json::Value::as_object)
            .unwrap();
        let messages = context
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .unwrap();

        assert_eq!(context["source"], "subscriber_diagnostics");
        assert_eq!(context["continuity"], "unknown");
        assert!(context["note"].as_str().unwrap().contains("may be partial"));

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["text"], "报价那件事今天定");
        assert_eq!(messages[0]["from"], "them");
        assert_eq!(messages[0]["direction"], "incoming");
        assert_eq!(messages[1]["text"], "上次说的报价发你了");
        assert_eq!(messages[1]["from"], "them");
        assert_eq!(messages[1]["direction"], "incoming");
        assert_eq!(messages[2]["text"], "我晚点看，周一聊细节");
        assert_eq!(messages[2]["from"], "me");
        assert_eq!(messages[2]["direction"], "outgoing");
        assert!(!serde_json::to_string(messages).unwrap().contains("聊下？"));
        assert!(!serde_json::to_string(messages)
            .unwrap()
            .contains("别的聊天"));
    }

    #[test]
    fn enrich_monitor_trigger_context_reads_bounded_telegram_history_cache() {
        let temp = tempfile::tempdir().unwrap();
        let _home = puffer_config::set_puffer_home_override(temp.path());
        let paths = puffer_config::ConfigPaths::discover(temp.path());
        let account_dir = paths
            .user_config_dir
            .join("telegram-accounts")
            .join("telegram-user");
        std::fs::create_dir_all(&account_dir).unwrap();
        std::fs::write(
            account_dir.join("telegram-history-cache.json"),
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "source": "telegram_server_history",
                "limit_per_chat": 64,
                "chats": [
                    {
                        "chat_id": 42,
                        "chat_kind": "user",
                        "chat_title": "Chaofan 10-23",
                        "messages": [
                            {
                                "message_id": 9,
                                "date_ms": 500,
                                "is_outgoing": false,
                                "sender_name": "Chaofan",
                                "text": "报价那件事今天定"
                            },
                            {
                                "message_id": 10,
                                "date_ms": 1_000,
                                "is_outgoing": true,
                                "sender_name": "Me",
                                "text": "我晚点看，周一聊细节"
                            },
                            {
                                "message_id": 11,
                                "date_ms": 3_000,
                                "is_outgoing": false,
                                "sender_name": "Chaofan",
                                "text": "聊下？"
                            }
                        ]
                    },
                    {
                        "chat_id": 99,
                        "chat_kind": "user",
                        "chat_title": "Other",
                        "messages": [
                            {
                                "message_id": 3,
                                "date_ms": 1_500,
                                "is_outgoing": false,
                                "sender_name": "Other",
                                "text": "别的聊天"
                            }
                        ]
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let trigger = json!({
            "connection_id": "telegram-user",
            "text": "聊下？",
            "payload": {
                "chat_id": 42,
                "chat_kind": "user",
                "chat_title": "Chaofan 10-23",
                "message_id": 11,
                "date_ms": 3_000
            }
        });

        let enriched = super::enrich_monitor_trigger_context(&paths, trigger).unwrap();
        let context = enriched
            .pointer("/payload/conversation_context")
            .and_then(serde_json::Value::as_object)
            .unwrap();
        let messages = context
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .unwrap();

        assert_eq!(context["source"], "telegram_server_history_cache");
        assert_eq!(context["continuity"], "bounded_server_history");
        assert!(context["note"]
            .as_str()
            .unwrap()
            .contains("bounded recent Telegram server history"));
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["text"], "报价那件事今天定");
        assert_eq!(messages[0]["from"], "them");
        assert_eq!(messages[1]["text"], "我晚点看，周一聊细节");
        assert_eq!(messages[1]["from"], "me");
        assert!(!serde_json::to_string(messages).unwrap().contains("聊下？"));
        assert!(!serde_json::to_string(messages)
            .unwrap()
            .contains("别的聊天"));
    }

    #[test]
    fn enrich_monitor_trigger_context_falls_back_when_history_cache_is_corrupt() {
        let temp = tempfile::tempdir().unwrap();
        let _home = puffer_config::set_puffer_home_override(temp.path());
        let paths = puffer_config::ConfigPaths::discover(temp.path());
        let account_dir = paths
            .user_config_dir
            .join("telegram-accounts")
            .join("telegram-user");
        std::fs::create_dir_all(&account_dir).unwrap();
        std::fs::write(account_dir.join("telegram-history-cache.json"), "{bad json").unwrap();
        std::fs::write(
            account_dir.join("message-diagnostics.ndjson"),
            json!({
                "stage": "emitted",
                "chat_id": 42,
                "chat_kind": "user",
                "chat_title": "Chaofan 10-23",
                "sender_name": "Chaofan",
                "message_id": 10,
                "date_ms": 1_000,
                "text_prefix": "诊断兜底上下文",
                "is_outgoing": false
            })
            .to_string(),
        )
        .unwrap();
        let trigger = json!({
            "connection_id": "telegram-user",
            "text": "聊下？",
            "payload": {
                "chat_id": 42,
                "chat_kind": "user",
                "message_id": 11,
                "date_ms": 2_000
            }
        });

        let enriched = super::enrich_monitor_trigger_context(&paths, trigger).unwrap();
        let context = enriched
            .pointer("/payload/conversation_context")
            .and_then(serde_json::Value::as_object)
            .unwrap();
        let messages = context
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .unwrap();

        assert_eq!(context["source"], "subscriber_diagnostics");
        assert_eq!(messages[0]["text"], "诊断兜底上下文");
    }

    #[test]
    fn enrich_monitor_trigger_context_skips_group_chats() {
        let temp = tempfile::tempdir().unwrap();
        let _home = puffer_config::set_puffer_home_override(temp.path());
        let paths = puffer_config::ConfigPaths::discover(temp.path());
        let account_dir = paths
            .user_config_dir
            .join("telegram-accounts")
            .join("telegram-user");
        std::fs::create_dir_all(&account_dir).unwrap();
        std::fs::write(
            account_dir.join("message-diagnostics.ndjson"),
            json!({
                "stage": "emitted",
                "chat_id": -10042,
                "chat_kind": "group",
                "chat_title": "Project Group",
                "sender_name": "Alice",
                "message_id": 10,
                "date_ms": 1_000,
                "text_prefix": "群里前文",
                "is_outgoing": false
            })
            .to_string(),
        )
        .unwrap();
        let trigger = json!({
            "connection_id": "telegram-user",
            "text": "@zooey 聊下？",
            "payload": {
                "chat_id": -10042,
                "chat_kind": "group",
                "message_id": 11,
                "date_ms": 2_000
            }
        });

        let enriched = super::enrich_monitor_trigger_context(&paths, trigger).unwrap();

        assert!(enriched.pointer("/payload/conversation_context").is_none());
    }

    #[test]
    fn fallback_prefers_authenticated_openai_when_default_provider_lacks_auth() {
        let mut registry = ProviderRegistry::new();
        registry.register(provider("anthropic", "claude-sonnet-4-5"));
        registry.register(provider("openai", "gpt-5"));
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("openai", "test-key");

        assert_eq!(
            authenticated_fallback_provider(&auth_store, &registry).as_deref(),
            Some("openai")
        );
    }

    #[test]
    fn task_agent_default_prefers_openai_mini_for_openai() {
        let mut registry = ProviderRegistry::new();
        registry.register(provider_with_models("openai", &["gpt-5.5", "gpt-5.4-mini"]));
        let mut state = app_state("openai", Some("openai/gpt-5.5"));

        apply_task_agent_model_default(&mut state, &registry);

        assert_eq!(state.current_provider.as_deref(), Some("openai"));
        assert_eq!(state.current_model.as_deref(), Some("openai/gpt-5.4-mini"));
    }

    #[test]
    fn task_agent_default_prefers_haiku_for_anthropic() {
        let mut registry = ProviderRegistry::new();
        registry.register(provider_with_models(
            "anthropic",
            &["claude-sonnet-4-5", "claude-haiku-4-5-20251001"],
        ));
        let mut state = app_state("anthropic", Some("claude-sonnet-4-5"));

        apply_task_agent_model_default(&mut state, &registry);

        assert_eq!(state.current_provider.as_deref(), Some("anthropic"));
        assert_eq!(
            state.current_model.as_deref(),
            Some("anthropic/claude-haiku-4-5-20251001")
        );
    }

    #[test]
    fn task_agent_default_falls_back_to_current_model_when_preferred_missing() {
        let mut registry = ProviderRegistry::new();
        registry.register(provider("openai", "gpt-5.5"));
        let mut state = app_state("openai", Some("openai/gpt-5.5"));

        apply_task_agent_model_default(&mut state, &registry);

        assert_eq!(state.current_provider.as_deref(), Some("openai"));
        assert_eq!(state.current_model.as_deref(), Some("openai/gpt-5.5"));
    }

    #[test]
    fn task_agent_default_runs_after_authenticated_provider_fallback() {
        let mut registry = ProviderRegistry::new();
        registry.register(provider("anthropic", "claude-sonnet-4-5"));
        registry.register(provider_with_models("openai", &["gpt-5.5", "gpt-5.4-mini"]));
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("openai", "test-key");
        let mut state = app_state("anthropic", Some("anthropic/claude-sonnet-4-5"));

        apply_authenticated_provider_fallback(&mut state, &registry, &auth_store);
        apply_task_agent_model_default(&mut state, &registry);

        assert_eq!(state.current_provider.as_deref(), Some("openai"));
        assert_eq!(state.current_model.as_deref(), Some("openai/gpt-5.4-mini"));
    }

    #[test]
    fn task_agent_default_leaves_non_family_provider_current_model() {
        let mut registry = ProviderRegistry::new();
        registry.register(provider("qwen35", "default_model"));
        let mut state = app_state("qwen35", Some("qwen35/default_model"));

        apply_task_agent_model_default(&mut state, &registry);

        assert_eq!(state.current_provider.as_deref(), Some("qwen35"));
        assert_eq!(state.current_model.as_deref(), Some("qwen35/default_model"));
    }

    #[test]
    fn unscoped_model_selection_uses_current_provider_before_global_match() {
        let mut registry = ProviderRegistry::new();
        registry.register(provider("anthropic", "shared-model"));
        registry.register(provider("openai", "shared-model"));

        assert_eq!(
            selected_provider_id_for_model("shared-model", Some("openai"), &registry).as_deref(),
            Some("openai")
        );
    }

    #[test]
    fn record_monitor_source_text_stamps_matching_tasks_verbatim() {
        let temp = tempfile::tempdir().unwrap();
        let paths = puffer_config::ConfigPaths::discover(temp.path());
        let store_path = paths
            .workspace_config_dir
            .join("runtime")
            .join("claude_workflow")
            .join("monitor_tasks.json");
        std::fs::create_dir_all(store_path.parent().unwrap()).unwrap();
        std::fs::write(
            &store_path,
            serde_json::to_string_pretty(&json!({
                "tasks": [
                    {
                        "task_id": "monitor-1",
                        "subject": "SLA report request",
                        "metadata": {
                            "monitor_envelope_id": "env-1",
                            "source_context": { "kind": "telegram_direct_message" }
                        }
                    },
                    {
                        "task_id": "monitor-2",
                        "subject": "Other task",
                        "metadata": { "monitor_envelope_id": "env-2" }
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let trigger = json!({
            "envelope_id": "env-1",
            "text": "线上支付回调失败率刚升到 18%，请在 16:00 前给结论。",
            "payload": {
                "message_id": 6836,
                "conversation_context": {
                    "kind": "telegram_prior_messages",
                    "scope": "same_chat_before_current_message",
                    "messages": [
                        {
                            "from": "them",
                            "direction": "incoming",
                            "text": "刚刚的异常还在继续",
                            "date_ms": 1_000
                        },
                        {
                            "from": "me",
                            "direction": "outgoing",
                            "text": "我看一下，16:00 前回复",
                            "date_ms": 2_000
                        }
                    ]
                }
            }
        });

        super::record_monitor_source_text(&paths, &trigger).unwrap();

        let store: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&store_path).unwrap()).unwrap();
        let tasks = store["tasks"].as_array().unwrap();
        // The matching task gets the verbatim text on both fields…
        assert_eq!(
            tasks[0]["metadata"]["source_text"],
            "线上支付回调失败率刚升到 18%，请在 16:00 前给结论。"
        );
        assert_eq!(
            tasks[0]["metadata"]["source_context"]["text"],
            "线上支付回调失败率刚升到 18%，请在 16:00 前给结论。"
        );
        // The source message id rides along for reply threading (#630).
        assert_eq!(tasks[0]["metadata"]["source_message_id"], 6836);
        assert_eq!(tasks[0]["metadata"]["source_context"]["message_id"], 6836);
        // Prior same-chat context rides along for reply-draft quality.
        assert_eq!(
            tasks[0]["metadata"]["source_context"]["conversation_context"]["scope"],
            "same_chat_before_current_message"
        );
        assert_eq!(
            tasks[0]["metadata"]["source_context"]["context_messages"][0]["text"],
            "刚刚的异常还在继续"
        );
        // …while a different envelope's task is untouched.
        assert!(tasks[1]["metadata"].get("source_text").is_none());

        // Re-stamping the same envelope is a no-op.
        super::record_monitor_source_text(&paths, &trigger).unwrap();
        let store: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&store_path).unwrap()).unwrap();
        assert_eq!(
            store["tasks"][0]["metadata"]["source_text"],
            "线上支付回调失败率刚升到 18%，请在 16:00 前给结论。"
        );
    }

    #[test]
    fn record_monitor_source_text_derives_gmail_sender_context() {
        let temp = tempfile::tempdir().unwrap();
        let paths = puffer_config::ConfigPaths::discover(temp.path());
        let store_path = paths
            .workspace_config_dir
            .join("runtime")
            .join("claude_workflow")
            .join("monitor_tasks.json");
        std::fs::create_dir_all(store_path.parent().unwrap()).unwrap();
        std::fs::write(
            &store_path,
            serde_json::to_string_pretty(&json!({
                "tasks": [
                    {
                        "task_id": "monitor-gmail",
                        "subject": "Confirm meeting",
                        "metadata": {
                            "monitor_connection": "gmail-browser",
                            "monitor_connector": "gmail-browser",
                            "monitor_envelope_id": "env-gmail"
                        }
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let trigger = json!({
            "connection_id": "gmail-browser",
            "connector_slug": "gmail-browser",
            "envelope_id": "env-gmail",
            "text": "Fu Xiangyu\nConfirm meeting\nWhat time works?",
            "payload": {
                "platform": "gmail-browser",
                "account": "winterfell0614@gmail.com",
                "message": {
                    "sender": "Fu Xiangyu",
                    "fromEmail": "fuxiangyu@example.com",
                    "subject": "Confirm meeting",
                    "threadId": "thread-123",
                    "url": "https://mail.google.com/mail/u/0/#inbox/thread-123"
                }
            }
        });

        super::record_monitor_source_text(&paths, &trigger).unwrap();

        let store: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&store_path).unwrap()).unwrap();
        let context = &store["tasks"][0]["metadata"]["source_context"];
        assert_eq!(context["kind"], "gmail_message");
        assert_eq!(context["platform"], "gmail-browser");
        assert_eq!(context["sender"]["name"], "Fu Xiangyu");
        assert_eq!(context["sender"]["email"], "fuxiangyu@example.com");
        assert_eq!(
            context["delivery_target"]["account"],
            "winterfell0614@gmail.com"
        );
        assert_eq!(context["delivery_target"]["thread_id"], "thread-123");
        assert_eq!(
            context["text"],
            "Fu Xiangyu\nConfirm meeting\nWhat time works?"
        );
    }

    #[test]
    fn record_monitor_source_text_follows_task_update_re_pointing() {
        let temp = tempfile::tempdir().unwrap();
        let paths = puffer_config::ConfigPaths::discover(temp.path());
        let store_path = paths
            .workspace_config_dir
            .join("runtime")
            .join("claude_workflow")
            .join("monitor_tasks.json");
        std::fs::create_dir_all(store_path.parent().unwrap()).unwrap();
        // A task originally created from env-1 (already stamped with msg1)
        // that triage's TaskUpdate has re-pointed to a follow-up message.
        std::fs::write(
            &store_path,
            serde_json::to_string_pretty(&json!({
                "tasks": [
                    {
                        "task_id": "monitor-1",
                        "subject": "失败率升到 15%，18点前给结论",
                        "metadata": {
                            "monitor_envelope_id": "env-3",
                            "source_text": "失败率刚升到 18%，16:00 前给我结论",
                            "source_context": {
                                "kind": "telegram_direct_message",
                                "text": "失败率刚升到 18%，16:00 前给我结论"
                            }
                        }
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let trigger = json!({
            "envelope_id": "env-3",
            "text": "失败率刚升到 15% 18点前给我结论"
        });

        super::record_monitor_source_text(&paths, &trigger).unwrap();

        // The verbatim anchor follows the envelope the task now references —
        // a stale msg1 anchor under a msg3 subject would defeat its purpose.
        let store: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&store_path).unwrap()).unwrap();
        assert_eq!(
            store["tasks"][0]["metadata"]["source_text"],
            "失败率刚升到 15% 18点前给我结论"
        );
        assert_eq!(
            store["tasks"][0]["metadata"]["source_context"]["text"],
            "失败率刚升到 15% 18点前给我结论"
        );
    }

    #[test]
    fn record_monitor_source_text_tolerates_missing_store() {
        let temp = tempfile::tempdir().unwrap();
        let paths = puffer_config::ConfigPaths::discover(temp.path());
        let trigger = json!({ "envelope_id": "env-1", "text": "hello" });

        super::record_monitor_source_text(&paths, &trigger).unwrap();
    }

    #[test]
    fn outgoing_turn_excludes_task_create_tool() {
        let tools = super::triage_turn_tool_names(true);
        assert!(!tools.iter().any(|t| t == "TaskCreate"));
        assert!(tools.iter().any(|t| t == "TaskUpdate"));
        assert!(tools.iter().any(|t| t == "TaskList"));
    }

    #[test]
    fn incoming_turn_includes_task_create_tool() {
        let tools = super::triage_turn_tool_names(false);
        assert!(tools.iter().any(|t| t == "TaskCreate"));
    }

    #[test]
    fn triage_prompt_includes_direction_from_payload() {
        let triggers = vec![json!({
            "connection_id": "telegram-user",
            "text": "done",
            "payload": { "is_outgoing": true, "chat_id": 42 }
        })];
        let prompt = render_triage_batch_prompt("Monitor prompt", &triggers).unwrap();
        assert!(prompt.contains("\"direction\""));
        assert!(prompt.contains("outgoing"));
    }

    #[test]
    fn triage_prompt_direction_defaults_incoming() {
        let triggers = vec![json!({
            "connection_id": "telegram-user", "text": "hi",
            "payload": { "is_outgoing": false, "chat_id": 7 }
        })];
        let prompt = render_triage_batch_prompt("Monitor prompt", &triggers).unwrap();
        assert!(prompt.contains("incoming"));
    }
}
