use anyhow::{Context, Result};
use puffer_config::{ConfigPaths, PufferConfig};
use puffer_core::{execute_tool_action_once, execute_user_turn, AppState};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::SessionStore;
use puffer_subscriptions::{install_workflow_runner, WorkflowActionRunner};
use puffer_workflow::{
    AgentExecution, AgentExecutor, CronDeduper, CronExpression, DagRunner, ExecutionContext,
    TriggerSpec, WorkflowStore,
};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use time::{OffsetDateTime, UtcOffset};

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

impl WorkflowActionRunner for ProcessWorkflowRunner {
    fn run_workflow(&self, slug: &str, trigger: serde_json::Value) -> Result<String> {
        let _guard = self.lock.lock().unwrap();
        let store = WorkflowStore::new(&self.paths.workspace_config_dir);
        let definition = store
            .get(slug)?
            .ok_or_else(|| anyhow::anyhow!("workflow `{slug}` is not registered"))?;
        if !definition.enabled {
            anyhow::bail!("workflow `{slug}` is disabled");
        }
        let run = DagRunner::new(
            &store,
            PufferAgentExecutor {
                paths: self.paths.clone(),
                config: self.config.clone(),
                resources: self.resources.clone(),
                providers: self.providers.clone(),
                auth_store: self.auth_store.clone(),
            },
        )
        .run(&definition, trigger, None)?;
        Ok(format!(
            "workflow `{slug}` run #{} {:?}",
            run.idx, run.status
        ))
    }

    fn run_tool_action(
        &self,
        tool_id: &str,
        input: serde_json::Value,
        _trigger: serde_json::Value,
    ) -> Result<String> {
        let _guard = self.lock.lock().unwrap();
        let cwd = self.paths.workspace_root.clone();
        let mut state = self.new_app_state(cwd.clone(), None)?;
        let result = execute_tool_action_once(&mut state, &self.resources, &cwd, tool_id, input)?;
        if result.success {
            return Ok(result.output.stdout);
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
    ) -> Result<String> {
        let _guard = self.lock.lock().unwrap();
        let trigger = serde_json::to_string_pretty(&trigger)?;
        let prompt = format!("{prompt}\n\nWorkflow trigger:\n```json\n{trigger}\n```");
        self.run_agent_prompt(prompt, model)
    }
}

impl ProcessWorkflowRunner {
    fn new_app_state(&self, cwd: PathBuf, model: Option<&str>) -> Result<AppState> {
        let session_store = SessionStore::from_paths(&self.paths)?;
        let session = session_store.create_session(cwd.clone())?;
        let mut state = AppState::new(self.config.clone(), cwd, session);
        if let Some(model) = model {
            state.current_model = Some(model.to_string());
        }
        Ok(state)
    }

    fn run_agent_prompt(&self, prompt: String, model: Option<&str>) -> Result<String> {
        let cwd = self.paths.workspace_root.clone();
        let mut state = self.new_app_state(cwd, model)?;
        let mut auth_store = self.auth_store.clone();
        let output = execute_user_turn(
            &mut state,
            &self.resources,
            &self.providers,
            &mut auth_store,
            &prompt,
        )?;
        Ok(output.assistant_text)
    }
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
        let session = session_store.create_session(cwd.clone())?;
        let mut state = AppState::new(self.config.clone(), cwd, session);
        if let Some(model) = context.model {
            state.current_model = Some(model);
        }
        let prompt = if let Some(agent) = context.agent {
            format!(
                "Run as Puffer workflow agent `{agent}`.\n\n{}",
                context.prompt
            )
        } else {
            context.prompt
        };
        let output = execute_user_turn(
            &mut state,
            &self.resources,
            &self.providers,
            &mut self.auth_store,
            &prompt,
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
            let run = DagRunner::new(
                &store,
                PufferAgentExecutor {
                    paths: runner.paths.clone(),
                    config: runner.config.clone(),
                    resources: runner.resources.clone(),
                    providers: runner.providers.clone(),
                    auth_store: runner.auth_store.clone(),
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

fn local_now() -> OffsetDateTime {
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    OffsetDateTime::now_utc().to_offset(offset)
}
