use anyhow::{Context, Result};
use puffer_config::{ConfigPaths, PufferConfig};
use puffer_core::{
    execute_tool_action_once, execute_user_turn_streaming,
    execute_user_turn_streaming_without_tools, AppState,
};
use puffer_provider_registry::{canonical_provider_id, AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, BACKGROUND_SESSION_TAG};
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

const OPENAI_TASK_AGENT_MODEL: &str = "gpt-5.4-mini";
const ANTHROPIC_TASK_AGENT_MODEL: &str = "claude-haiku-4-5-20251001";

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
        self.run_task_agent_prompt(prompt, model)
    }

    fn ignore_analysis_agent(
        &self,
        prompt: &str,
        model: Option<&str>,
        trigger: serde_json::Value,
    ) -> Result<String> {
        let _guard = self.lock.lock().unwrap();
        let trigger = serde_json::to_string_pretty(&trigger)?;
        let prompt = format!("{prompt}\n\nWorkflow trigger:\n```json\n{trigger}\n```");
        self.run_task_agent_prompt_without_tools(prompt, model)
    }
}

impl ProcessWorkflowRunner {
    fn new_app_state(&self, cwd: PathBuf, model: Option<&str>) -> Result<AppState> {
        let session_store = SessionStore::from_paths(&self.paths)?;
        let session = session_store
            .create_session_with_tags(cwd.clone(), vec![BACKGROUND_SESSION_TAG.to_string()])?;
        let mut state = AppState::new(self.config.clone(), cwd, session);
        if let Some(model) = model.and_then(non_empty_trimmed) {
            apply_explicit_model(&mut state, model);
        } else {
            apply_authenticated_provider_fallback(&mut state, &self.providers, &self.auth_store);
        }
        Ok(state)
    }

    fn new_task_app_state(&self, cwd: PathBuf, model: Option<&str>) -> Result<AppState> {
        let mut state = self.new_app_state(cwd, model)?;
        if model.and_then(non_empty_trimmed).is_none() {
            apply_task_agent_model_default(&mut state, &self.providers);
        }
        Ok(state)
    }

    fn run_task_agent_prompt(&self, prompt: String, model: Option<&str>) -> Result<String> {
        let cwd = self.paths.workspace_root.clone();
        let mut state = self.new_task_app_state(cwd, model)?;
        let mut auth_store = self.auth_store.clone();
        let output = execute_user_turn_streaming(
            &mut state,
            &self.resources,
            &self.providers,
            &mut auth_store,
            &prompt,
            |_| {},
        )?;
        Ok(output.assistant_text)
    }

    fn run_task_agent_prompt_without_tools(
        &self,
        prompt: String,
        model: Option<&str>,
    ) -> Result<String> {
        let cwd = self.paths.workspace_root.clone();
        let mut state = self.new_task_app_state(cwd, model)?;
        let mut auth_store = self.auth_store.clone();
        let output = execute_user_turn_streaming_without_tools(
            &mut state,
            &self.resources,
            &self.providers,
            &mut auth_store,
            &prompt,
            |_| {},
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
        authenticated_fallback_provider, selected_provider_id_for_model, AuthStore,
        ProviderRegistry,
    };
    use puffer_config::PufferConfig;
    use puffer_provider_registry::{AuthMode, Modality, ModelDescriptor, ProviderDescriptor};
    use puffer_session_store::SessionMetadata;
    use std::path::PathBuf;
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
        registry.register(provider("minicpm5", "default_model"));
        let mut state = app_state("minicpm5", Some("minicpm5/default_model"));

        apply_task_agent_model_default(&mut state, &registry);

        assert_eq!(state.current_provider.as_deref(), Some("minicpm5"));
        assert_eq!(
            state.current_model.as_deref(),
            Some("minicpm5/default_model")
        );
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
}
