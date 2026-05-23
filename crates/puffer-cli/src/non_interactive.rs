use anyhow::{anyhow, Context, Result};
use clap::Args;
use indexmap::IndexMap;
use puffer_config::{ConfigPaths, PufferConfig};
use puffer_core::{
    command_surface, dispatch_command, execute_user_turn_streaming_with_permissions_and_cancel,
    AppState, CancelToken, MessageRole, PermissionPromptAction, TurnStreamEvent,
};
use puffer_provider_registry::{
    detect_import_candidates, AuthStore, ExternalImportFamily, Modality, ModelDescriptor,
    ProviderRegistry, StoredCredential,
};
use puffer_resources::LoadedResources;
use puffer_session_store::{SessionStore, TranscriptEvent};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Arguments for one non-interactive Puffer run.
#[derive(Debug, Clone, Args)]
pub(crate) struct NonInteractiveArgs {
    /// Message to submit to the agent.
    #[arg(long = "user-message")]
    pub(crate) user_message: Option<String>,
    /// Transcript JSONL or markdown file to preload before the turn.
    #[arg(long = "load-transcript")]
    pub(crate) load_transcript: Option<PathBuf>,
    /// Skill markdown file to prepend as guidance for the turn.
    #[arg(long = "load-skill")]
    pub(crate) load_skill: Vec<PathBuf>,
    /// Slash command to run instead of a user message, e.g. /genskill.
    #[arg(long = "run-command")]
    pub(crate) run_command: Option<String>,
    /// Write assistant text or generated skill markdown to this path.
    #[arg(long = "output")]
    pub(crate) output: Option<PathBuf>,
    /// Write a replay-compatible JSON artifact to this path.
    #[arg(long = "emit-artifact")]
    pub(crate) emit_artifact: Option<PathBuf>,
    /// Write the session transcript JSONL to this path.
    #[arg(long = "transcript-out", default_value = "/tmp/puffer-session.jsonl")]
    pub(crate) transcript_out: PathBuf,
    /// Maximum tool calls before the run is cancelled at the next boundary.
    #[arg(long = "max-tool-calls")]
    pub(crate) max_tool_calls: Option<u64>,
    /// Maximum total tokens before the artifact is marked token-budget.
    #[arg(long = "max-tokens")]
    pub(crate) max_tokens: Option<u64>,
    /// Provider id override.
    #[arg(long = "provider")]
    pub(crate) provider: Option<String>,
    /// Model id or provider/model selector override.
    #[arg(long = "model")]
    pub(crate) model: Option<String>,
    /// Reasoning effort override.
    #[arg(long = "effort")]
    pub(crate) effort: Option<String>,
    /// Enable fast mode for this run.
    #[arg(long = "fast", default_value_t = false)]
    pub(crate) fast: bool,
    /// PR id to record in replay artifacts.
    #[arg(long = "artifact-pr")]
    pub(crate) artifact_pr: Option<String>,
    /// Replay arm to record in artifacts.
    #[arg(long = "artifact-arm", default_value = "no-skill")]
    pub(crate) artifact_arm: ArtifactArm,
}

/// Runs one non-interactive command or prompt turn.
pub(crate) fn run_non_interactive_command(
    cwd: &Path,
    config: &PufferConfig,
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    paths: &ConfigPaths,
    args: NonInteractiveArgs,
) -> Result<()> {
    hydrate_env_auth(auth_store);
    apply_openai_base_url_override(providers);
    hydrate_codex_openai_auth(providers, auth_store)?;

    let session_store = SessionStore::from_paths(paths)?;
    let session = session_store.create_session(cwd.to_path_buf())?;
    let mut state =
        AppState::new(config.clone(), cwd.to_path_buf(), session.clone()).with_tool_runner(
            crate::runner_selection::select_tool_runner(config, resources, cwd.to_path_buf()),
        );
    apply_model_overrides(&mut state, providers, &args)?;
    state.grant_all_tools_for_session();
    state.sandbox_mode = "danger-full-access".to_string();

    let mut text_context = String::new();
    if let Some(path) = args.load_transcript.as_deref() {
        text_context = load_transcript(path, &mut state, &session_store)?;
    }

    let mut artifact = ReplayArtifact::new(
        args.artifact_pr.clone().unwrap_or_default(),
        args.artifact_arm,
    );
    let started = std::time::Instant::now();
    let turn_result = if let Some(command) = args.run_command.as_deref() {
        if let Some(message) = args
            .user_message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
        {
            append_user_message(&mut state, &session_store, message)?;
        }
        run_slash_command(
            command,
            &mut state,
            resources,
            providers,
            auth_store,
            &session_store,
            args.output.as_deref(),
        )
    } else {
        let prompt = compose_prompt(
            text_context.as_str(),
            &load_skill_text(&args.load_skill)?,
            args.user_message.as_deref(),
        )?;
        run_user_turn(
            &prompt,
            &mut state,
            resources,
            providers,
            auth_store,
            &session_store,
            &args,
            &mut artifact,
        )
    };
    let result_text = match turn_result {
        Ok(text) => text,
        Err(error) => {
            artifact.wall_seconds = started.elapsed().as_secs();
            artifact.final_diff = git_diff(cwd);
            if let Some(path) = args.emit_artifact.as_deref() {
                write_json(path, &artifact)?;
            }
            write_transcript(&session_store, session.id, &args.transcript_out)?;
            return Err(error);
        }
    };
    artifact.wall_seconds = started.elapsed().as_secs();
    artifact.final_diff = git_diff(cwd);
    if let Some(path) = args.emit_artifact.as_deref() {
        write_json(path, &artifact)?;
    }
    if let Some(path) = args.output.as_deref() {
        if args.run_command.is_none() {
            write_text(path, &result_text)?;
        }
    }
    write_transcript(&session_store, session.id, &args.transcript_out)?;
    Ok(())
}

fn append_user_message(
    state: &mut AppState,
    session_store: &SessionStore,
    message: &str,
) -> Result<()> {
    state.push_message(MessageRole::User, message.to_string());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: message.to_string(),
            actor: Some(state.user_actor()),
        },
    )?;
    Ok(())
}

fn run_user_turn(
    prompt: &str,
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    session_store: &SessionStore,
    args: &NonInteractiveArgs,
    artifact: &mut ReplayArtifact,
) -> Result<String> {
    state.push_message(MessageRole::User, prompt.to_string());
    session_store.append_event(
        state.session.id,
        TranscriptEvent::UserMessage {
            text: prompt.to_string(),
            actor: Some(state.user_actor()),
        },
    )?;

    let cancel = CancelToken::new();
    let cancel_for_callback = cancel.clone();
    let mut tool_count = 0_u64;
    let mut token_total = 0_u64;
    let mut output_tokens = 0_u64;
    let mut input_tokens = 0_u64;
    let turn = puffer_core::with_user_question_prompt_handler(
        |_| puffer_core::UserQuestionPromptResponse {
            answers: serde_json::Map::new(),
            annotations: serde_json::Map::new(),
        },
        || {
            execute_user_turn_streaming_with_permissions_and_cancel(
                state,
                resources,
                providers,
                auth_store,
                prompt,
                None,
                &cancel,
                |event| match event {
                    TurnStreamEvent::TextDelta(delta) => {
                        print!("{delta}");
                        let _ = std::io::stdout().flush();
                    }
                    TurnStreamEvent::ToolInvocations(invocations) => {
                        tool_count += invocations.len() as u64;
                        for invocation in invocations {
                            artifact.tool_call_log.push(ToolCallArtifact {
                                name: invocation.tool_id,
                                input: parse_tool_input(&invocation.input),
                                output_size: invocation.output.len() as u64,
                                ts: unix_time_ms().to_string(),
                            });
                        }
                        if args
                            .max_tool_calls
                            .is_some_and(|budget| tool_count > budget)
                        {
                            cancel_for_callback.cancel();
                        }
                    }
                    TurnStreamEvent::Usage(usage) => {
                        input_tokens = usage.input_tokens;
                        output_tokens = usage.output_tokens;
                        token_total = usage.input_tokens
                            + usage.output_tokens
                            + usage.cache_read_tokens
                            + usage.cache_creation_tokens;
                    }
                    _ => {}
                },
                |_| PermissionPromptAction::AllowAllSession,
            )
        },
    );

    artifact.tool_calls = tool_count;
    artifact.tokens = TokenArtifact {
        input: input_tokens,
        output: output_tokens,
        tool_results: 0,
        total: token_total,
    };
    match turn {
        Ok(turn) => {
            append_tool_invocations(state, session_store, &turn.tool_invocations)?;
            state.push_message(MessageRole::Assistant, turn.assistant_text.clone());
            session_store.append_event(
                state.session.id,
                TranscriptEvent::AssistantMessage {
                    text: turn.assistant_text.clone(),
                    actor: Some(state.assistant_actor()),
                },
            )?;
            artifact.outcome = if args.max_tokens.is_some_and(|budget| token_total > budget) {
                ArtifactOutcome::TokenBudget
            } else if args
                .max_tool_calls
                .is_some_and(|budget| tool_count > budget)
            {
                ArtifactOutcome::ToolBudget
            } else {
                ArtifactOutcome::Pass
            };
            Ok(turn.assistant_text)
        }
        Err(error) => {
            artifact.outcome = if args
                .max_tool_calls
                .is_some_and(|budget| tool_count > budget)
            {
                ArtifactOutcome::ToolBudget
            } else {
                ArtifactOutcome::GaveUp
            };
            Err(error)
        }
    }
}

fn append_tool_invocations(
    state: &mut AppState,
    session_store: &SessionStore,
    invocations: &[puffer_core::ToolInvocation],
) -> Result<()> {
    for invocation in invocations {
        state.push_tool_invocation(
            &invocation.call_id,
            &invocation.tool_id,
            &invocation.input,
            &invocation.output,
            invocation.success,
        );
        session_store.append_event(
            state.session.id,
            TranscriptEvent::ToolInvocation {
                call_id: invocation.call_id.clone(),
                tool_id: invocation.tool_id.clone(),
                input: invocation.input.clone(),
                output: invocation.output.clone(),
                success: invocation.success,
                actor: Some(state.assistant_actor()),
                subject: state.tool_subject_actor(&invocation.tool_id, &invocation.output),
            },
        )?;
    }
    Ok(())
}

fn run_slash_command(
    command: &str,
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
    session_store: &SessionStore,
    output: Option<&Path>,
) -> Result<String> {
    let commands = command_surface(resources);
    dispatch_command(
        state,
        &commands,
        resources,
        providers,
        auth_store,
        session_store,
        command,
    )?;
    let message = latest_message_text(state).unwrap_or_default();
    if let Some(output) = output {
        if let Some(path) = message.strip_prefix("Skill written to ") {
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::copy(path.trim(), output)
                .with_context(|| format!("copying generated skill to {}", output.display()))?;
        } else {
            write_text(output, &message)?;
        }
    }
    println!("{message}");
    Ok(message)
}

fn latest_message_text(state: &AppState) -> Option<String> {
    state.transcript.last().map(|message| message.text.clone())
}

fn apply_model_overrides(
    state: &mut AppState,
    providers: &mut ProviderRegistry,
    args: &NonInteractiveArgs,
) -> Result<()> {
    let env_provider = env_nonempty("PUFFER_PROVIDER");
    let env_model = env_nonempty("PUFFER_MODEL");
    let provider_override = args.provider.as_deref().or(env_provider.as_deref());
    let model_override = args.model.as_deref().or(env_model.as_deref());
    if let Some(provider) = provider_override {
        state.current_provider = Some(provider.to_string());
    }
    if let Some(model) = model_override {
        let selector = normalize_model_selector(state.current_provider.as_deref(), model);
        ensure_model_selector_registered(providers, &selector)?;
        state.current_model = Some(selector);
    } else if state.current_model.is_none() {
        state.current_model = default_model_selector(state.current_provider.as_deref(), providers);
    }
    if state.current_provider.is_none() {
        state.current_provider = state.current_model.as_deref().and_then(|model| {
            model
                .split_once('/')
                .map(|(provider, _)| provider.to_string())
        });
    }
    if let Some(effort) = args.effort.as_deref() {
        state.effort_level = effort.to_string();
    }
    if args.fast {
        state.fast_mode = true;
    }
    if state.current_model.is_none() {
        return Err(anyhow!(
            "no model selected; configure default_model or pass --model"
        ));
    }
    Ok(())
}

fn ensure_model_selector_registered(
    providers: &mut ProviderRegistry,
    selector: &str,
) -> Result<()> {
    if providers.resolve_model(selector).is_some() {
        return Ok(());
    }
    let Some((provider_id, model_id)) = selector.split_once('/') else {
        return Ok(());
    };
    if providers
        .provider(provider_id)
        .and_then(|provider| provider.models.iter().find(|model| model.id == model_id))
        .is_some()
    {
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
        input: vec![Modality::Text],
        cost: None,
    });
    providers.register_with_source(descriptor, entry.source);
    Ok(())
}

fn env_nonempty(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn normalize_model_selector(provider: Option<&str>, model: &str) -> String {
    if model.contains('/') {
        model.to_string()
    } else {
        format!("{}/{}", provider.unwrap_or("openai"), model)
    }
}

fn default_model_selector(provider: Option<&str>, providers: &ProviderRegistry) -> Option<String> {
    let provider_id = provider.unwrap_or("openai");
    providers
        .provider(provider_id)
        .and_then(|provider| provider.models.first())
        .map(|model| format!("{}/{}", model.provider, model.id))
}

fn load_transcript(
    path: &Path,
    state: &mut AppState,
    session_store: &SessionStore,
) -> Result<String> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading transcript {}", path.display()))?;
    if looks_like_jsonl_transcript(&content) {
        for (index, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event: TranscriptEvent = serde_json::from_str(line)
                .with_context(|| format!("parsing transcript line {}", index + 1))?;
            apply_transcript_event(state, session_store, event)?;
        }
        return Ok(String::new());
    }
    Ok(content)
}

fn looks_like_jsonl_transcript(content: &str) -> bool {
    content
        .lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.trim_start().starts_with('{') && line.contains("\"type\""))
}

fn apply_transcript_event(
    state: &mut AppState,
    session_store: &SessionStore,
    event: TranscriptEvent,
) -> Result<()> {
    match &event {
        TranscriptEvent::UserMessage { text, .. } => state.push_message(MessageRole::User, text),
        TranscriptEvent::AssistantMessage { text, .. } => {
            state.push_message(MessageRole::Assistant, text)
        }
        TranscriptEvent::SystemMessage { text, .. } => {
            state.push_message(MessageRole::System, text)
        }
        TranscriptEvent::ToolInvocation {
            call_id,
            tool_id,
            input,
            output,
            success,
            ..
        } => state.push_tool_invocation(call_id, tool_id, input, output, *success),
        TranscriptEvent::TranscriptRewritten { rewrite } => state.apply_transcript_rewrite(rewrite),
        _ => {}
    }
    session_store.append_event(state.session.id, event)?;
    Ok(())
}

fn load_skill_text(paths: &[PathBuf]) -> Result<Vec<String>> {
    paths
        .iter()
        .map(|path| {
            fs::read_to_string(path).with_context(|| format!("reading skill {}", path.display()))
        })
        .collect()
}

fn compose_prompt(
    transcript_context: &str,
    skills: &[String],
    user_message: Option<&str>,
) -> Result<String> {
    let message = user_message
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .ok_or_else(|| anyhow!("provide --user-message or --run-command"))?;
    let mut parts = Vec::new();
    if !transcript_context.trim().is_empty() {
        parts.push(format!(
            "Conversation transcript:\n\n{}",
            transcript_context.trim()
        ));
    }
    for skill in skills {
        parts.push(format!("Skill guidance:\n\n{}", skill.trim()));
    }
    parts.push(message.to_string());
    Ok(parts.join("\n\n---\n\n"))
}

fn hydrate_env_auth(auth_store: &mut AuthStore) {
    for (provider, env_name) in [
        ("openai", "OPENAI_API_KEY"),
        ("anthropic", "ANTHROPIC_API_KEY"),
    ] {
        if let Ok(value) = std::env::var(env_name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                auth_store.set_api_key(provider, trimmed.to_string());
            }
        }
    }
}

fn apply_openai_base_url_override(providers: &mut ProviderRegistry) {
    if let Ok(base_url) = std::env::var("OPENAI_BASE_URL") {
        let trimmed = base_url.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            providers.apply_openai_base_url_override(Some(trimmed));
        }
    }
}

fn hydrate_codex_openai_auth(
    providers: &mut ProviderRegistry,
    auth_store: &mut AuthStore,
) -> Result<()> {
    if auth_store.has_auth("openai") {
        return Ok(());
    }

    let Some(candidate) = detect_import_candidates(ExternalImportFamily::OpenAi)?
        .into_iter()
        .next()
    else {
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

fn parse_tool_input(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::json!({ "value": raw }))
}

fn git_diff(cwd: &Path) -> String {
    std::process::Command::new("git")
        .args(["diff", "--"])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(value)?)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn write_transcript(store: &SessionStore, session_id: Uuid, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let record = store.load_session(session_id)?;
    let mut file =
        fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    for event in record.events {
        writeln!(file, "{}", serde_json::to_string(&event)?)?;
    }
    Ok(())
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

#[derive(Debug, Clone, Copy, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ArtifactArm {
    NoSkill,
    Direct,
    Gepa,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ArtifactOutcome {
    Pass,
    GaveUp,
    ToolBudget,
    TokenBudget,
}

#[derive(Debug, Clone, Serialize)]
struct TokenArtifact {
    input: u64,
    output: u64,
    tool_results: u64,
    total: u64,
}

#[derive(Debug, Clone, Serialize)]
struct ToolCallArtifact {
    name: String,
    input: Value,
    output_size: u64,
    ts: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReplayArtifact {
    pr: String,
    arm: ArtifactArm,
    outcome: ArtifactOutcome,
    wall_seconds: u64,
    tool_calls: u64,
    tokens: TokenArtifact,
    tool_call_log: Vec<ToolCallArtifact>,
    final_diff: String,
    test_outcome: Option<Value>,
}

impl ReplayArtifact {
    fn new(pr: String, arm: ArtifactArm) -> Self {
        Self {
            pr,
            arm,
            outcome: ArtifactOutcome::GaveUp,
            wall_seconds: 0,
            tool_calls: 0,
            tokens: TokenArtifact {
                input: 0,
                output: 0,
                tool_results: 0,
                total: 0,
            },
            tool_call_log: Vec::new(),
            final_diff: String::new(),
            test_outcome: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_provider_registry::ProviderDescriptor;

    #[test]
    fn compose_prompt_includes_transcript_and_skill() {
        let prompt = compose_prompt("history", &["skill".to_string()], Some("fix it")).unwrap();
        assert!(prompt.contains("Conversation transcript"));
        assert!(prompt.contains("Skill guidance"));
        assert!(prompt.ends_with("fix it"));
    }

    #[test]
    fn compose_prompt_rejects_empty_message() {
        assert!(compose_prompt("", &[], Some(" ")).is_err());
        assert!(compose_prompt("", &[], None).is_err());
    }

    #[test]
    fn parse_tool_input_preserves_json_objects() {
        assert_eq!(
            parse_tool_input(r#"{"path":"foo.cpp"}"#),
            serde_json::json!({"path":"foo.cpp"})
        );
        assert_eq!(
            parse_tool_input("plain"),
            serde_json::json!({"value":"plain"})
        );
    }

    #[test]
    fn jsonl_detection_requires_type_field() {
        assert!(looks_like_jsonl_transcript(
            r#"{"type":"user_message","text":"hi"}"#
        ));
        assert!(!looks_like_jsonl_transcript(r#"{"role":"user"}"#));
    }

    #[test]
    fn custom_model_selector_registers_unknown_provider_model() {
        let mut providers = ProviderRegistry::new();
        providers.register(ProviderDescriptor {
            id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            base_url: "https://api.openai.com".to_string(),
            default_api: "openai-responses".to_string(),
            auth_modes: Vec::new(),
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            chat_completions_path: None,
            discovery: None,
            models: vec![ModelDescriptor {
                id: "gpt-5.4".to_string(),
                display_name: "GPT-5.4".to_string(),
                provider: "openai".to_string(),
                api: "openai-responses".to_string(),
                context_window: 272_000,
                max_output_tokens: 16_384,
                supports_reasoning: true,
                input: vec![Modality::Text],
                cost: None,
                compat: None,
            }],
        });

        ensure_model_selector_registered(&mut providers, "openai/gpt-5.3-codex").unwrap();
        let model = providers.resolve_model("openai/gpt-5.3-codex").unwrap();

        assert_eq!(model.api, "openai-responses");
        assert_eq!(model.context_window, 272_000);
        assert!(model.supports_reasoning);
    }
}
