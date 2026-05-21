use anyhow::{anyhow, Context, Result};
use indexmap::IndexMap;
use puffer_config::{ensure_workspace_dirs, ConfigPaths, PufferConfig};
use puffer_core::{execute_user_turn_streaming_with_reflection, AppState};
use puffer_provider_registry::{
    detect_import_candidates, AuthStore, ExternalImportFamily, ModelDescriptor, ProviderRegistry,
    StoredCredential,
};
use puffer_resources::{LoadedItem, LoadedResources, PromptTemplate, SourceInfo, SourceKind};
use puffer_session_store::{SessionMetadata, SessionStore, TranscriptEvent};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::benchmark_reflection::benchmark_reflection_config;
use crate::runner_selection::select_tool_runner;

const APP_NAME: &str = "puffer-code";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const BENCHMARK_WORKING_DIRS_ENV: &str = "PUFFER_BENCHMARK_WORKING_DIRS";
const BENCHMARK_SYSTEM_PROMPT_ID: &str = "system-base";
const BENCHMARK_SYSTEM_PROMPT_TEMPLATE: &str = r#"You are an autonomous coding agent solving a software engineering task in the current workspace.
Work directly on the files, commands, services, and output paths named by the task.
Read only the files you need, make only the changes the task requires, and run the smallest relevant verification before you stop.
Do not repeat the same search or reread the same unchanged file unless the earlier result was incomplete.
When the task names a file, read or write that exact path directly instead of searching for it.
Ignore benchmark harness files under /app/.puffer unless the task explicitly mentions them.
When the task names a required output file, start a draft as soon as you have enough information to make progress and iterate from there.
If the required output file does not exist, create a minimal valid draft immediately after your first relevant read or verifier inspection, then iterate on that file instead of continuing to explore.
When the task already names both the verifier file and the required output file, do not use Glob or Grep before writing the first draft.

Before opening or running a tool on a file, consider whether it may modify that file or its neighbors as a side effect. When in doubt, back up first.
When a file's contents look unexpected, inspect it with a low-level tool before passing it to a higher-level one that might alter it.

$USING_YOUR_TOOLS

$ENVIRONMENT"#;
const BENCHMARK_ALLOWED_TOOL_IDS: &[&str] = &[
    "Bash",
    "Edit",
    "Glob",
    "Grep",
    "Read",
    "Write",
    "WebSearch",
    "WebFetch",
];

/// Carries CLI inputs for one unattended benchmark execution.
#[derive(Debug, Clone)]
pub(crate) struct BenchmarkRunArgs {
    pub(crate) prompt: Option<String>,
    pub(crate) prompt_file: Option<String>,
    pub(crate) stdin: bool,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) effort: String,
    pub(crate) fast: bool,
    pub(crate) result_json: Option<String>,
    pub(crate) trajectory_json: Option<String>,
    pub(crate) deny_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct BenchmarkToolInvocation {
    call_id: String,
    tool_id: String,
    input: String,
    output: String,
    success: bool,
}

#[derive(Debug, Clone, Serialize)]
struct BenchmarkResult {
    success: bool,
    session_id: String,
    prompt_cache_key: String,
    provider: String,
    model: String,
    effort: String,
    fast_mode: bool,
    prompt: String,
    assistant_text: String,
    tool_invocations: Vec<BenchmarkToolInvocation>,
    error: Option<String>,
    /// Categorical failure tag. Populated when the failure is
    /// distinguishable from a generic exit-1 — currently only the
    /// quota family (`quota_rate_limit` / `quota_access_terminated`).
    /// `None` for success and for unclassified failures. Present in
    /// `result.json` so `puffer_harbor_agent.py` / `run_tb2.py` can
    /// decide whether to delay retry vs. fail-fast.
    #[serde(skip_serializing_if = "Option::is_none")]
    error_kind: Option<String>,
}

/// Executes one unattended benchmark turn and writes optional result artifacts.
pub(crate) fn run_benchmark_command(
    cwd: &Path,
    config: &PufferConfig,
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    args: BenchmarkRunArgs,
) -> Result<()> {
    let prompt = resolve_prompt(&args)?;
    let selected_provider = args.provider.trim().to_string();
    if selected_provider.is_empty() {
        return Err(anyhow!("provider cannot be empty"));
    }

    let model_selector = normalize_model_selector(&selected_provider, &args.model);
    let model_id = model_selector
        .split_once('/')
        .map(|(_, model)| model.to_string())
        .ok_or_else(|| anyhow!("invalid model selector {model_selector}"))?;

    hydrate_openai_auth(providers, auth_store, &selected_provider)?;
    // Honor OPENAI_BASE_URL for proxy/custom endpoints.
    if let Ok(base_url) = std::env::var("OPENAI_BASE_URL") {
        let trimmed = base_url.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            providers.apply_openai_base_url_override(Some(trimmed));
        }
    }
    let _ = providers.discover_and_merge_all(auth_store);
    ensure_model_registered(providers, &selected_provider, &model_id)?;

    let paths = ConfigPaths::discover(cwd);
    ensure_workspace_dirs(&paths)?;
    let benchmark_resources = benchmark_resources(resources);
    prepare_unattended_workspace(&paths, &benchmark_resources, &args.deny_tools)?;

    let now_ms = unix_time_ms();
    let session_id = Uuid::new_v4();
    let prompt_cache_key = benchmark_prompt_cache_key(
        cwd,
        &selected_provider,
        &model_selector,
        &args.effort,
        args.fast,
        &prompt,
        &args.deny_tools,
    );
    let mut state = AppState::new(
        config.clone(),
        cwd.to_path_buf(),
        SessionMetadata {
            id: session_id,
            display_name: Some("benchmark-run".to_string()),
            generated_title: None,
            cwd: cwd.to_path_buf(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            parent_session_id: None,
            slug: None,
            tags: vec!["benchmark".to_string()],
            note: None,
        },
    );
    state.prompt_cache_key_override = Some(prompt_cache_key.clone());
    state.current_provider = Some(selected_provider.clone());
    state.current_model = Some(model_selector.clone());
    state.effort_level = args.effort.clone();
    state.fast_mode = args.fast;
    state.sandbox_mode = "danger-full-access".to_string();
    state.working_dirs = benchmark_working_dirs(&state.cwd);

    // Hydrate the runner from `config.remote_runner` (if set) or from a
    // local `LocalToolRunner` populated with the workspace's MCP server
    // manifest. Without this the agent loop runs against the empty
    // default `LocalToolRunner::new()` and never sees `mcp__*` tools.
    let runner = select_tool_runner(config, &benchmark_resources, cwd.to_path_buf());
    state = state.with_tool_runner(runner);

    // Create a session store under ~/.puffer/sessions (not project dir).
    let home_puffer = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".puffer"))
        .unwrap_or_else(|_| cwd.join(".puffer"));
    let session_paths = ConfigPaths {
        workspace_root: cwd.to_path_buf(),
        workspace_config_dir: home_puffer.clone(),
        user_config_dir: home_puffer,
        builtin_resources_dir: paths.builtin_resources_dir.clone(),
    };
    let session_store =
        SessionStore::from_paths(&session_paths).expect("failed to create session store");
    let _ = session_store.append_event(
        session_id,
        TranscriptEvent::UserMessage {
            text: prompt.clone(),
            actor: Some(state.user_actor()),
        },
    );
    let benchmark_actor = state.assistant_actor();

    // Write streaming events to trajectory file incrementally so progress
    // is observable and partial results survive crashes.
    let trajectory_path = args.trajectory_json.as_deref().map(PathBuf::from);
    let incremental_path = trajectory_path
        .as_ref()
        .map(|p| p.with_extension("incremental.jsonl"));
    let incremental_file = incremental_path
        .as_ref()
        .and_then(|p| fs::File::create(p).ok())
        .map(std::sync::Mutex::new);
    let incremental_ref = &incremental_file;
    let emitted_text_delta = std::cell::Cell::new(false);

    match execute_user_turn_streaming_with_reflection(
        &mut state,
        &benchmark_resources,
        providers,
        auth_store,
        &prompt,
        benchmark_reflection_config(&model_selector),
        |event| {
            use std::io::Write;
            match &event {
                puffer_core::TurnStreamEvent::TextDelta(delta) => {
                    emitted_text_delta.set(true);
                    print!("{delta}");
                    let _ = std::io::stdout().flush();
                }
                puffer_core::TurnStreamEvent::ToolInvocations(invocations) => {
                    // Write to session store (same format as TUI)
                    for inv in invocations {
                        let status = if inv.success { "ok" } else { "error" };
                        let rendered = if inv.output.is_empty() {
                            format!("Tool {} [{}]\ninput: {}", inv.tool_id, status, inv.input)
                        } else {
                            format!(
                                "Tool {} [{}]\ninput: {}\n{}",
                                inv.tool_id, status, inv.input, inv.output
                            )
                        };
                        let _ = session_store.append_event(
                            session_id,
                            TranscriptEvent::SystemMessage {
                                text: rendered,
                                actor: Some(benchmark_actor.clone()),
                            },
                        );
                    }
                    // Write to incremental trajectory
                    if let Some(lock) = incremental_ref {
                        if let Ok(mut f) = lock.lock() {
                            for inv in invocations {
                                let output_preview: String =
                                    inv.output.chars().take(2000).collect();
                                let truncated = inv.output.chars().count() > 2000;
                                let line = serde_json::json!({
                                    "type": "tool_invocation",
                                    "tool_id": inv.tool_id,
                                    "input": inv.input,
                                    "output": output_preview,
                                    "output_truncated": truncated,
                                    "success": inv.success,
                                    "timestamp": unix_time_ms(),
                                });
                                let _ = writeln!(f, "{}", line);
                            }
                            let _ = f.flush();
                        }
                    }
                }
                puffer_core::TurnStreamEvent::ReflectionCheckpoint(summary) => {
                    let rendered = format!("Reflection checkpoint\n{summary}");
                    let _ = session_store.append_event(
                        session_id,
                        TranscriptEvent::SystemMessage {
                            text: rendered,
                            actor: Some(benchmark_actor.clone()),
                        },
                    );
                    if let Some(lock) = incremental_ref {
                        if let Ok(mut f) = lock.lock() {
                            let line = serde_json::json!({
                                "type": "reflection_checkpoint",
                                "summary": summary,
                                "timestamp": unix_time_ms(),
                            });
                            let _ = writeln!(f, "{}", line);
                            let _ = f.flush();
                        }
                    }
                }
                puffer_core::TurnStreamEvent::ReflectionTrace(trace) => {
                    let line = serde_json::json!({
                        "type": "reflection_trace",
                        "event": trace,
                        "timestamp": unix_time_ms(),
                    });
                    if let Err(error) = session_store.append_trace_event(
                        session_id,
                        puffer_session_store::TRACE_RUNTIME,
                        &line,
                    ) {
                        eprintln!("reflection trace persist failed: {error}");
                    }
                    if let Some(lock) = incremental_ref {
                        if let Ok(mut f) = lock.lock() {
                            let _ = writeln!(f, "{}", line);
                            let _ = f.flush();
                        }
                    }
                }
                puffer_core::TurnStreamEvent::Usage(report) => {
                    if let Some(lock) = incremental_ref {
                        if let Ok(mut f) = lock.lock() {
                            let line = serde_json::json!({
                                "type": "usage",
                                "input_tokens": report.input_tokens,
                                "output_tokens": report.output_tokens,
                                "cache_read_tokens": report.cache_read_tokens,
                                "cache_creation_tokens": report.cache_creation_tokens,
                                "timestamp": unix_time_ms(),
                            });
                            let _ = writeln!(f, "{}", line);
                            let _ = f.flush();
                        }
                    }
                }
                puffer_core::TurnStreamEvent::RetryAttempt {
                    attempt,
                    max_attempts,
                    error,
                } => {
                    if let Some(lock) = incremental_ref {
                        if let Ok(mut f) = lock.lock() {
                            let line = serde_json::json!({
                                "type": "retry_attempt",
                                "attempt": attempt,
                                "max_attempts": max_attempts,
                                "error": error,
                                "timestamp": unix_time_ms(),
                            });
                            let _ = writeln!(f, "{}", line);
                            let _ = f.flush();
                        }
                    }
                }
                _ => {}
            }
        },
    ) {
        Ok(turn) => {
            let tool_invocations = turn
                .tool_invocations
                .into_iter()
                .map(|tool| BenchmarkToolInvocation {
                    call_id: tool.call_id,
                    tool_id: tool.tool_id,
                    input: tool.input,
                    output: tool.output,
                    success: tool.success,
                })
                .collect::<Vec<_>>();
            let result = BenchmarkResult {
                success: true,
                session_id: session_id.to_string(),
                prompt_cache_key: prompt_cache_key.clone(),
                provider: selected_provider,
                model: model_selector.clone(),
                effort: args.effort,
                fast_mode: args.fast,
                prompt: prompt.clone(),
                assistant_text: turn.assistant_text.clone(),
                tool_invocations,
                error: None,
                error_kind: None,
            };
            let _ = session_store.append_event(
                session_id,
                TranscriptEvent::AssistantMessage {
                    text: turn.assistant_text.clone(),
                    actor: Some(benchmark_actor.clone()),
                },
            );
            write_benchmark_artifacts(
                &result,
                args.result_json.as_deref(),
                args.trajectory_json.as_deref(),
            )?;
            if emitted_text_delta.get() {
                if !turn.assistant_text.is_empty() && !turn.assistant_text.ends_with('\n') {
                    println!();
                }
            } else if !turn.assistant_text.is_empty() {
                println!("{}", turn.assistant_text);
            }
            Ok(())
        }
        Err(error) => {
            // Downcast to typed `QuotaError` so we can stamp
            // `error_kind` in `result.json` and exit with a distinct
            // code. Orchestration (`puffer_harbor_agent.py` /
            // `run_tb2.py`) reads the exit code to delay-retry
            // instead of burning the budget back-to-back. See
            // `runtime::quota` for design notes and the v16
            // trajectory-analysis finding (4/5 sampled "unsolved"
            // tasks were quota-cascade deaths).
            let quota_signal = error
                .downcast_ref::<puffer_core::QuotaError>()
                .map(|qe| qe.kind);
            let result = BenchmarkResult {
                success: false,
                session_id: session_id.to_string(),
                prompt_cache_key,
                provider: selected_provider,
                model: model_selector,
                effort: args.effort,
                fast_mode: args.fast,
                prompt,
                assistant_text: String::new(),
                tool_invocations: Vec::new(),
                error: Some(error.to_string()),
                error_kind: quota_signal.map(|kind| kind.slug().to_string()),
            };
            write_benchmark_artifacts(
                &result,
                args.result_json.as_deref(),
                args.trajectory_json.as_deref(),
            )?;
            if quota_signal.is_some() {
                // Print the error chain like anyhow's default handler
                // would, then exit with the quota-distinct code so
                // the orchestration layer can detect this without
                // parsing stderr. Skip returning Err — it would just
                // retrigger anyhow's exit-1 path.
                eprintln!("{error:?}");
                std::process::exit(puffer_core::QUOTA_EXIT_CODE);
            }
            Err(error)
        }
    }
}

fn benchmark_resources(resources: &LoadedResources) -> LoadedResources {
    let mut benchmark_resources = resources.clone();
    benchmark_resources
        .tools
        .retain(|tool| BENCHMARK_ALLOWED_TOOL_IDS.contains(&tool.value.id.as_str()));
    benchmark_resources
        .prompts
        .retain(|prompt| prompt.value.id != BENCHMARK_SYSTEM_PROMPT_ID);

    benchmark_resources.prompts.insert(
        0,
        LoadedItem {
            value: PromptTemplate {
                id: BENCHMARK_SYSTEM_PROMPT_ID.to_string(),
                description: "Benchmark-specific runtime system prompt".to_string(),
                template: BENCHMARK_SYSTEM_PROMPT_TEMPLATE.to_string(),
                variables: Vec::new(),
                allowed_tools: Vec::new(),
                provider_override: None,
                model_override: None,
                mode: None,
                chained_from: Vec::new(),
                for_provider: None,
                for_model: None,
            },
            source_info: SourceInfo {
                path: PathBuf::from("benchmark/prompts/system-base.yaml"),
                kind: SourceKind::Workspace,
            },
        },
    );
    benchmark_resources
}

fn resolve_prompt(args: &BenchmarkRunArgs) -> Result<String> {
    if args.stdin {
        let mut buffer = String::new();
        std::io::stdin()
            .read_to_string(&mut buffer)
            .context("failed to read prompt from stdin")?;
        let prompt = buffer.trim().to_string();
        if prompt.is_empty() {
            return Err(anyhow!("stdin prompt cannot be empty"));
        }
        return Ok(prompt);
    }
    if let Some(path) = args.prompt_file.as_deref() {
        let prompt = fs::read_to_string(path)
            .with_context(|| format!("failed to read prompt file {path}"))?;
        let trimmed = prompt.trim().to_string();
        if trimmed.is_empty() {
            return Err(anyhow!("prompt file {path} was empty"));
        }
        return Ok(trimmed);
    }
    let prompt = args
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("provide one of --prompt, --prompt-file, or --stdin"))?;
    Ok(prompt.to_string())
}

fn normalize_model_selector(provider_id: &str, model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.contains('/') {
        trimmed.to_string()
    } else {
        format!("{provider_id}/{trimmed}")
    }
}

fn hydrate_openai_auth(
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    provider_id: &str,
) -> Result<()> {
    if provider_id != "openai" {
        return Ok(());
    }
    // Env var takes priority over imported codex credentials.
    if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
        let trimmed = api_key.trim();
        if !trimmed.is_empty() {
            auth_store.set_api_key("openai", trimmed.to_string());
            return Ok(());
        }
    }

    if auth_store.has_auth("openai") {
        return Ok(());
    }
    let candidates = detect_import_candidates(ExternalImportFamily::OpenAi)?;
    let Some(candidate) = candidates.into_iter().next() else {
        return Ok(());
    };
    if let Some(base_url) = candidate.openai_base_url.as_deref() {
        providers.set_openai_base_url(base_url);
    }
    if !candidate.openai_headers.is_empty() {
        providers.set_openai_headers(
            candidate
                .openai_headers
                .clone()
                .into_iter()
                .collect::<IndexMap<_, _>>(),
        );
    }
    if !candidate.openai_query_params.is_empty() {
        providers.set_openai_query_params(
            candidate
                .openai_query_params
                .clone()
                .into_iter()
                .collect::<IndexMap<_, _>>(),
        );
    }
    match candidate.credential {
        StoredCredential::ApiKey { key } => auth_store.set_api_key("openai", key),
        StoredCredential::OAuth(credential) => auth_store.set_oauth("openai", credential),
    }
    Ok(())
}

fn ensure_model_registered(
    providers: &mut ProviderRegistry,
    provider_id: &str,
    model_id: &str,
) -> Result<()> {
    let selector = format!("{provider_id}/{model_id}");
    if providers.resolve_model(&selector).is_some() {
        return Ok(());
    }

    let entry = providers
        .provider_entry(provider_id)
        .cloned()
        .ok_or_else(|| anyhow!("provider {provider_id} is not registered"))?;
    let Some(prototype) = entry.descriptor.models.first().cloned() else {
        return Err(anyhow!("provider {provider_id} has no configured models"));
    };

    let mut descriptor = entry.descriptor.clone();
    descriptor.models.push(ModelDescriptor {
        id: model_id.to_string(),
        display_name: model_id.to_string(),
        provider: provider_id.to_string(),
        api: prototype.api,
        context_window: prototype.context_window,
        max_output_tokens: prototype.max_output_tokens,
        supports_reasoning: prototype.supports_reasoning,
        compat: None,
        input: vec![puffer_provider_registry::Modality::Text],
        cost: None,
    });
    providers.register_with_source(descriptor, entry.source);
    Ok(())
}

fn prepare_unattended_workspace(
    paths: &ConfigPaths,
    resources: &LoadedResources,
    deny_tools: &[String],
) -> Result<()> {
    fs::create_dir_all(&paths.workspace_config_dir).with_context(|| {
        format!(
            "failed to create benchmark workspace config dir {}",
            paths.workspace_config_dir.display()
        )
    })?;
    let mut tool_permissions = IndexMap::<String, String>::new();
    for tool in &resources.tools {
        tool_permissions.insert(
            tool.value.id.clone(),
            benchmark_tool_permission(&tool.value.id, deny_tools).to_string(),
        );
    }

    let mut permissions = String::from("[tools]\n");
    for (tool_id, policy) in tool_permissions {
        permissions.push_str(&format!("{tool_id} = \"{policy}\"\n"));
    }
    fs::write(
        paths.workspace_config_dir.join("permissions.toml"),
        permissions,
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            paths
                .workspace_config_dir
                .join("permissions.toml")
                .display()
        )
    })?;

    fs::write(
        paths.workspace_config_dir.join("sandbox.toml"),
        "mode = \"danger-full-access\"\nauto_allow = true\nallow_unsandboxed_fallback = false\nexcluded_commands = []\n",
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            paths.workspace_config_dir.join("sandbox.toml").display()
        )
    })?;
    Ok(())
}

fn write_benchmark_artifacts(
    result: &BenchmarkResult,
    result_json: Option<&str>,
    trajectory_json: Option<&str>,
) -> Result<()> {
    if let Some(path) = result_json {
        write_json_file(path, &serde_json::to_value(result)?)?;
    }
    if let Some(path) = trajectory_json {
        write_json_file(path, &build_trajectory_json(result))?;
    }
    Ok(())
}

fn write_json_file(path: &str, value: &Value) -> Result<()> {
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&path, serde_json::to_string_pretty(value)?)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn build_trajectory_json(result: &BenchmarkResult) -> Value {
    let mut steps = vec![json!({
        "step_id": 1,
        "source": "user",
        "message": result.prompt,
    })];

    for (index, tool) in result.tool_invocations.iter().enumerate() {
        let call_id = format!("tool-{}", index + 1);
        let arguments = parse_tool_arguments(&tool.input);
        steps.push(json!({
            "step_id": steps.len() + 1,
            "source": "agent",
            "model_name": result.model,
            "message": format!("Executed {}", tool.tool_id),
            "tool_calls": [{
                "tool_call_id": call_id,
                "function_name": tool.tool_id,
                "arguments": arguments,
            }],
            "observation": {
                "results": [{
                    "source_call_id": call_id,
                    "content": tool.output,
                }]
            },
            "extra": {
                "success": tool.success,
            }
        }));
    }

    let final_message = result
        .error
        .as_deref()
        .map(|error| format!("Benchmark run failed: {error}"))
        .unwrap_or_else(|| result.assistant_text.clone());
    steps.push(json!({
        "step_id": steps.len() + 1,
        "source": "agent",
        "model_name": result.model,
        "message": final_message,
    }));

    json!({
        "schema_version": "ATIF-v1.6",
        "session_id": result.session_id,
        "agent": {
            "name": APP_NAME,
            "version": APP_VERSION,
            "model_name": result.model,
            "extra": {
                "provider": result.provider,
                "effort": result.effort,
                "fast_mode": result.fast_mode,
                "prompt_cache_key": result.prompt_cache_key,
            }
        },
        "steps": steps,
        "extra": {
            "success": result.success,
        }
    })
}

fn parse_tool_arguments(raw: &str) -> Value {
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        if value.is_object() {
            return value;
        }
        return json!({ "value": value });
    }
    json!({ "value": raw })
}

fn benchmark_prompt_cache_key(
    cwd: &Path,
    provider: &str,
    model: &str,
    effort: &str,
    fast_mode: bool,
    prompt: &str,
    deny_tools: &[String],
) -> String {
    let tool_fingerprint = BENCHMARK_ALLOWED_TOOL_IDS.join(",");
    let deny_tool_fingerprint = deny_tools
        .iter()
        .map(|tool| tool.trim())
        .filter(|tool| !tool.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(",");
    let key = (
        "benchmark-run",
        provider,
        model,
        effort,
        fast_mode,
        tool_fingerprint,
        deny_tool_fingerprint,
        cwd,
        prompt.trim(),
    );
    let secondary_key = (
        "benchmark-run",
        provider,
        model,
        effort,
        fast_mode,
        BENCHMARK_ALLOWED_TOOL_IDS,
        deny_tools,
        cwd,
        prompt.trim(),
        "prompt-cache",
    );
    let primary = stable_hash64(&key) as u128;
    let secondary = stable_hash64(&secondary_key) as u128;
    Uuid::from_u128((primary << 64) | secondary).to_string()
}

fn stable_hash64<T: Hash>(value: &T) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

fn benchmark_working_dirs(cwd: &Path) -> Vec<PathBuf> {
    let Some(raw_paths) = std::env::var_os(BENCHMARK_WORKING_DIRS_ENV) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut working_dirs = Vec::new();
    for path in std::env::split_paths(&raw_paths) {
        if path.as_os_str().is_empty() || path == cwd || !path.is_dir() {
            continue;
        }
        if seen.insert(path.clone()) {
            working_dirs.push(path);
        }
    }
    working_dirs
}

fn benchmark_tool_permission<'a>(tool_id: &str, deny_tools: &'a [String]) -> &'a str {
    if deny_tools
        .iter()
        .map(|tool| tool.trim())
        .any(|tool| !tool.is_empty() && tool == tool_id)
    {
        return "deny";
    }
    if BENCHMARK_ALLOWED_TOOL_IDS.contains(&tool_id) {
        "allow"
    } else {
        "deny"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_core::ReflectionLanguage;
    use std::ffi::OsString;

    struct ScopedBenchmarkWorkingDirs {
        old_value: Option<OsString>,
    }

    impl ScopedBenchmarkWorkingDirs {
        fn set(paths: &[PathBuf]) -> Self {
            let old_value = std::env::var_os(BENCHMARK_WORKING_DIRS_ENV);
            std::env::set_var(
                BENCHMARK_WORKING_DIRS_ENV,
                std::env::join_paths(paths).expect("joined paths"),
            );
            Self { old_value }
        }
    }

    impl Drop for ScopedBenchmarkWorkingDirs {
        fn drop(&mut self) {
            if let Some(value) = self.old_value.take() {
                std::env::set_var(BENCHMARK_WORKING_DIRS_ENV, value);
            } else {
                std::env::remove_var(BENCHMARK_WORKING_DIRS_ENV);
            }
        }
    }

    #[test]
    fn normalize_model_selector_prefixes_provider() {
        assert_eq!(
            normalize_model_selector("openai", "gpt-5.4"),
            "openai/gpt-5.4"
        );
        assert_eq!(
            normalize_model_selector("openai", "other/model"),
            "other/model"
        );
        let config = benchmark_reflection_config("openai/gpt-5.3-codex-spark");
        assert_eq!(config.language, ReflectionLanguage::Chinese);
        assert_eq!(
            config.llm_judge.unwrap().model_selector.as_deref(),
            Some("openai/gpt-5.3-codex-spark")
        );
    }

    #[test]
    fn parse_tool_arguments_preserves_object_payloads() {
        assert_eq!(
            parse_tool_arguments(r#"{"path":"src/main.rs"}"#),
            json!({ "path": "src/main.rs" })
        );
        assert_eq!(
            parse_tool_arguments("plain text"),
            json!({ "value": "plain text" })
        );
    }

    #[test]
    fn build_trajectory_json_records_failure_message() {
        let value = build_trajectory_json(&BenchmarkResult {
            success: false,
            session_id: "session-123".to_string(),
            prompt_cache_key: "cache-123".to_string(),
            provider: "openai".to_string(),
            model: "openai/gpt-5.4".to_string(),
            effort: "high".to_string(),
            fast_mode: true,
            prompt: "hello".to_string(),
            assistant_text: String::new(),
            tool_invocations: Vec::new(),
            error: Some("boom".to_string()),
            error_kind: None,
        });
        assert_eq!(value["steps"][1]["message"], "Benchmark run failed: boom");
        assert_eq!(value["session_id"], "session-123");
        assert_eq!(value["agent"]["extra"]["prompt_cache_key"], "cache-123");
    }

    #[test]
    fn benchmark_working_dirs_reads_existing_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cwd = temp.path().join("cwd");
        let extra = temp.path().join("extra");
        fs::create_dir_all(&cwd).expect("create cwd");
        fs::create_dir_all(&extra).expect("create extra");
        let _guard = ScopedBenchmarkWorkingDirs::set(&[cwd.clone(), extra.clone()]);
        assert_eq!(benchmark_working_dirs(&cwd), vec![extra]);
    }

    #[test]
    fn benchmark_tool_permission_uses_curated_allowlist() {
        assert_eq!(benchmark_tool_permission("Bash", &[]), "allow");
        assert_eq!(benchmark_tool_permission("AskUserQuestion", &[]), "deny");
        assert_eq!(benchmark_tool_permission("WebSearch", &[]), "deny");
        assert_eq!(
            benchmark_tool_permission("Bash", &[String::from("Bash")]),
            "deny"
        );
    }

    #[test]
    fn benchmark_prompt_cache_key_is_stable_for_identical_inputs() {
        let cwd = Path::new("/tmp/bench");
        let first = benchmark_prompt_cache_key(
            cwd,
            "openai",
            "openai/gpt-5.4",
            "xhigh",
            true,
            "solve task a",
            &[],
        );
        let second = benchmark_prompt_cache_key(
            cwd,
            "openai",
            "openai/gpt-5.4",
            "xhigh",
            true,
            "solve task a",
            &[],
        );
        let changed_prompt = benchmark_prompt_cache_key(
            cwd,
            "openai",
            "openai/gpt-5.4",
            "xhigh",
            true,
            "solve task b",
            &[],
        );
        let changed_cwd = benchmark_prompt_cache_key(
            Path::new("/tmp/other"),
            "openai",
            "openai/gpt-5.4",
            "xhigh",
            true,
            "solve task a",
            &[],
        );

        assert_eq!(first, second);
        assert_ne!(first, changed_prompt);
        assert_ne!(first, changed_cwd);
    }

    #[test]
    fn benchmark_resources_overrides_system_prompt() {
        let resources = LoadedResources {
            prompts: vec![LoadedItem {
                value: PromptTemplate {
                    id: BENCHMARK_SYSTEM_PROMPT_ID.to_string(),
                    description: "Original".to_string(),
                    template: "original".to_string(),
                    variables: Vec::new(),
                    allowed_tools: Vec::new(),
                    provider_override: None,
                    model_override: None,
                    mode: None,
                    chained_from: Vec::new(),
                    for_provider: None,
                    for_model: None,
                },
                source_info: SourceInfo {
                    path: PathBuf::from("resources/prompts/system-base.yaml"),
                    kind: SourceKind::Builtin,
                },
            }],
            ..LoadedResources::default()
        };

        let benchmark = benchmark_resources(&resources);
        assert_eq!(benchmark.prompts.len(), 1);
        assert_eq!(benchmark.prompts[0].value.id, BENCHMARK_SYSTEM_PROMPT_ID);
        assert!(benchmark.prompts[0]
            .value
            .template
            .contains("autonomous coding agent"));
    }

    #[test]
    fn benchmark_resources_prunes_non_benchmark_tools() {
        let resources = LoadedResources {
            tools: vec![
                LoadedItem {
                    value: puffer_resources::ToolSpec {
                        id: "Read".to_string(),
                        ..Default::default()
                    },
                    source_info: SourceInfo {
                        path: PathBuf::from("resources/tools/read.yaml"),
                        kind: SourceKind::Builtin,
                    },
                },
                LoadedItem {
                    value: puffer_resources::ToolSpec {
                        id: "SendUserMessage".to_string(),
                        ..Default::default()
                    },
                    source_info: SourceInfo {
                        path: PathBuf::from("resources/tools/send_user_message.yaml"),
                        kind: SourceKind::Builtin,
                    },
                },
            ],
            ..LoadedResources::default()
        };

        let benchmark = benchmark_resources(&resources);
        assert_eq!(benchmark.tools.len(), 1);
        assert_eq!(benchmark.tools[0].value.id, "Read");
    }
}
