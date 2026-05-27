use crate::codex_app_server::{self, CapturedTurnEvent, CodexTurnOptions, CodexTurnOutcome};
use crate::dtos::{
    AgentDiffDto, AuthProviderStatusDto, DivergenceReportDto, FolderGroupDto, ProviderSummaryDto,
    RepoStatusDto, ResourceCountsDto, SessionDetailDto, SessionListItemDto, SettingsConfigDto,
    SettingsSessionSummaryDto, SettingsSnapshotDto, TimelineItemDto,
};
use crate::events::EventEmitter;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const DEFAULT_PROVIDER: &str = "codex";
const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-4-6";
const DEFAULT_PUFFER_MODEL: &str = "default";

pub(crate) struct BackendState {
    turns: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl BackendState {
    pub(crate) fn new() -> Self {
        Self {
            turns: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn handle(
        &self,
        events: EventEmitter,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        match method {
            "list_grouped_sessions" => serde_value(self.list_grouped_sessions()?),
            "load_session_detail" => {
                let session_id = string_param(&params, &["sessionId", "session_id"])?;
                serde_value(self.load_session_detail(&session_id)?)
            }
            "login_with_api_key" => {
                let provider_id = string_param(&params, &["providerId", "provider_id"])?;
                let api_key = string_param(&params, &["apiKey", "api_key"])?;
                self.store_api_key(&provider_id, &api_key)?;
                serde_value(self.load_settings_snapshot()?)
            }
            "logout_provider" => {
                let provider_id = string_param(&params, &["providerId", "provider_id"])?;
                self.remove_api_key(&provider_id)?;
                serde_value(self.load_settings_snapshot()?)
            }
            "create_session" => {
                let cwd = optional_string_param(&params, &["cwd"])
                    .map(PathBuf::from)
                    .unwrap_or(self.default_workspace()?);
                let provider =
                    optional_string_param(&params, &["providerId", "provider_id", "provider"]);
                let model = optional_string_param(&params, &["modelId", "model_id", "model"]);
                serde_value(self.create_session(cwd, provider, model)?)
            }
            "rename_session" => {
                let session_id = string_param(&params, &["sessionId", "session_id"])?;
                let title = string_param(&params, &["title"])?;
                self.rename_session(&session_id, title)?;
                serde_value(self.load_session_detail(&session_id)?)
            }
            "run_agent_turn" => self.run_agent_turn(events.clone(), params),
            "cancel_turn" => {
                let turn_id = string_param(&params, &["turnId", "turn_id"])?;
                if let Some(flag) = self.turns.lock().unwrap().get(&turn_id) {
                    flag.store(true, Ordering::SeqCst);
                }
                Ok(json!({}))
            }
            other => bail!("unknown method: {other}"),
        }
    }

    fn default_workspace(&self) -> Result<PathBuf> {
        Ok(env::current_dir().context("failed to read current directory")?)
    }

    fn list_grouped_sessions(&self) -> Result<Vec<FolderGroupDto>> {
        let sessions = self.load_sessions()?;
        let mut groups: BTreeMap<String, Vec<SessionRecord>> = BTreeMap::new();
        for session in sessions {
            groups.entry(session.cwd.clone()).or_default().push(session);
        }

        let mut out = Vec::new();
        for (path, mut records) in groups {
            records.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
            let sessions = records
                .iter()
                .map(|record| self.session_list_item(record))
                .collect::<Vec<_>>();
            out.push(FolderGroupDto {
                folder_id: path.clone(),
                folder_label: folder_label(&path),
                folder_path: path,
                session_count: sessions.len(),
                sessions,
            });
        }
        Ok(out)
    }

    fn create_session(
        &self,
        cwd: PathBuf,
        provider_override: Option<String>,
        model_override: Option<String>,
    ) -> Result<Value> {
        let cwd = normalize_path(&cwd);
        ensure_session_cwd(&cwd)?;
        let mut config = self.load_config()?;
        if config.default_provider.is_none() {
            config.default_provider = Some(DEFAULT_PROVIDER.to_string());
        }
        if config.default_model.is_none() {
            config.default_model = default_model_for(
                config
                    .default_provider
                    .as_deref()
                    .unwrap_or(DEFAULT_PROVIDER),
            );
        }
        self.save_config(&config)?;
        let provider = provider_override
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                config
                    .default_provider
                    .clone()
                    .unwrap_or_else(|| DEFAULT_PROVIDER.to_string())
            });
        let provider = canonical_backend_provider_id(&provider);
        validate_provider_id(&provider)?;
        let model = model_override
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                if config
                    .default_provider
                    .as_deref()
                    .is_some_and(|default| backend_provider_ids_match(default, &provider))
                {
                    config.default_model.clone()
                } else {
                    None
                }
            })
            .or_else(|| default_model_for(&provider));

        let now = now_ms();
        let id = Uuid::new_v4().to_string();
        let record = SessionRecord {
            id: id.clone(),
            display_name: None,
            generated_title: None,
            title: "New agent".to_string(),
            cwd: cwd.display().to_string(),
            created_at_ms: now,
            updated_at_ms: now,
            slug: None,
            tags: vec![provider.clone()],
            note: None,
            parent_session_id: None,
            provider: provider.clone(),
            model: model.clone(),
            events: Vec::new(),
        };
        let mut sessions = self.load_sessions()?;
        sessions.push(record);
        self.save_sessions(&sessions)?;
        Ok(json!({
            "sessionId": id,
            "cwd": cwd.display().to_string(),
            "createdAtMs": now,
            "providerId": provider,
            "modelId": model,
        }))
    }

    fn load_session_detail(&self, session_id: &str) -> Result<SessionDetailDto> {
        let record = self.load_session(session_id)?;
        let timeline = self.timeline_items(&record);
        Ok(SessionDetailDto {
            session_id: record.id.clone(),
            display_name: record.display_name.clone(),
            generated_title: record.generated_title.clone(),
            title: record_title(&record),
            cwd: record.cwd.clone(),
            folder_path: record.cwd.clone(),
            updated_at_ms: record.updated_at_ms,
            created_at_ms: record.created_at_ms,
            event_count: record.events.len(),
            activity_status: stored_session_activity_status(&record.events).to_string(),
            slug: record.slug.clone(),
            tags: record.tags.clone(),
            note: record.note.clone(),
            parent_session_id: record.parent_session_id.clone(),
            provider_id: record.provider.clone(),
            model_id: record.model.clone(),
            timeline,
            latest_diff: None,
            diff_history: Vec::new(),
            repo_status: empty_repo_status(session_id, &record.cwd),
            agent_diff: AgentDiffDto {
                files: Vec::new(),
                entries: Vec::new(),
            },
            divergence: DivergenceReportDto {
                agent_only: Vec::new(),
                git_only: Vec::new(),
                agent_total: 0,
                git_total: 0,
            },
        })
    }

    fn timeline_items(&self, record: &SessionRecord) -> Vec<TimelineItemDto> {
        let mut items = Vec::new();
        for (idx, event) in record.events.iter().enumerate() {
            let id = format!("event-{idx}");
            match event {
                StoredEvent::User { text, .. } => items.push(TimelineItemDto::UserMessage {
                    id,
                    text: text.clone(),
                    actor: None,
                }),
                StoredEvent::Assistant { text, .. } => {
                    items.push(TimelineItemDto::AssistantMessage {
                        id,
                        text: text.clone(),
                        actor: None,
                    })
                }
                StoredEvent::System { text, .. } => items.push(TimelineItemDto::SystemMessage {
                    id,
                    text: text.clone(),
                    actor: None,
                }),
                StoredEvent::Tool {
                    tool_id,
                    input,
                    output,
                    success,
                    ..
                } => items.push(TimelineItemDto::ToolCall {
                    id,
                    tool_id: tool_id.clone(),
                    status: if *success { "completed" } else { "failed" }.to_string(),
                    summary: Some(tool_id.clone()),
                    input_text: input.clone(),
                    input_json: serde_json::from_str(input).ok(),
                    output_text: output.clone(),
                    actor: None,
                    subject: None,
                }),
            }
        }
        items
    }

    fn rename_session(&self, session_id: &str, title: String) -> Result<()> {
        let mut sessions = self.load_sessions()?;
        let record = sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or_else(|| anyhow!("unknown session `{session_id}`"))?;
        record.display_name = if title.trim().is_empty() {
            None
        } else {
            Some(title.trim().to_string())
        };
        record.updated_at_ms = now_ms();
        self.save_sessions(&sessions)
    }

    fn load_session(&self, session_id: &str) -> Result<SessionRecord> {
        self.load_sessions()?
            .into_iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| anyhow!("unknown session `{session_id}`"))
    }

    fn session_list_item(&self, record: &SessionRecord) -> SessionListItemDto {
        SessionListItemDto {
            session_id: record.id.clone(),
            display_name: record.display_name.clone(),
            generated_title: record.generated_title.clone(),
            title: record_title(record),
            cwd: record.cwd.clone(),
            folder_path: record.cwd.clone(),
            updated_at_ms: record.updated_at_ms,
            created_at_ms: record.created_at_ms,
            event_count: record.events.len(),
            activity_status: stored_session_activity_status(&record.events).to_string(),
            slug: record.slug.clone(),
            tags: record.tags.clone(),
            note: record.note.clone(),
            parent_session_id: record.parent_session_id.clone(),
            provider_id: record.provider.clone(),
            model_id: record.model.clone(),
        }
    }

    fn load_settings_snapshot(&self) -> Result<SettingsSnapshotDto> {
        let config = self.load_config()?;
        let sessions = self.load_sessions()?;
        let providers = provider_summaries();
        let auth = self.provider_auth_statuses()?;
        let home = app_home()?;
        let workspace = self.default_workspace()?;
        let default_model = normalized_default_model(&config);
        Ok(SettingsSnapshotDto {
            workspace_root: workspace.display().to_string(),
            workspace_config_file: config_file()?.display().to_string(),
            user_config_file: config_file()?.display().to_string(),
            auth_store_file: credentials_file()?.display().to_string(),
            builtin_resources_dir: home.join("resources").display().to_string(),
            config: SettingsConfigDto {
                app_name: "Momo".to_string(),
                default_provider: config.default_provider.clone(),
                default_model,
                openai_base_url: config.openai_base_url.clone(),
                theme: config.theme.clone().unwrap_or_else(|| "system".to_string()),
                mascot_id: "momo".to_string(),
                mascot_display_name: "Momo".to_string(),
                mascot_enabled: true,
                ui_no_alt_screen: true,
                ui_tmux_golden_mode: false,
            },
            resources: ResourceCountsDto {
                providers: providers.len(),
                tools: 0,
                agents: providers.len(),
                prompts: 0,
                hooks: 0,
                skills: 0,
                mascots: 1,
                plugins: 0,
                mcp_servers: 0,
                ides: 0,
            },
            sessions: SettingsSessionSummaryDto {
                total_sessions: sessions.len(),
                folder_groups: self.list_grouped_sessions()?.len(),
            },
            auth,
            providers,
        })
    }

    fn provider_auth_statuses(&self) -> Result<Vec<AuthProviderStatusDto>> {
        let credentials = self.load_credentials()?;
        let mut out = Vec::new();
        for provider in ["puffer", "codex", "claude"] {
            let command = provider_command(provider);
            let available = command_exists(&command);
            let has_stored_key = credentials.api_keys.contains_key(provider);
            let has_env = match provider {
                "codex" => env::var("OPENAI_API_KEY").is_ok(),
                "claude" => env::var("ANTHROPIC_API_KEY").is_ok(),
                "puffer" => env::var("PUFFER_API_KEY").is_ok(),
                _ => false,
            };
            let has_native_auth = match provider {
                "codex" => home_dir().join(".codex/auth.json").exists(),
                "claude" => home_dir().join(".claude").exists(),
                "puffer" => home_dir().join(".puffer/auth.json").exists(),
                _ => false,
            };
            if available || has_stored_key || has_env || has_native_auth {
                out.push(AuthProviderStatusDto {
                    provider_id: provider.to_string(),
                    kind: if has_stored_key {
                        "api_key".to_string()
                    } else if has_env {
                        "env".to_string()
                    } else if has_native_auth {
                        "native".to_string()
                    } else {
                        "cli".to_string()
                    },
                    email: None,
                    expires_at_ms: None,
                    scopes: Vec::new(),
                    plan_type: Some(if available {
                        "CLI available".to_string()
                    } else {
                        "Credentials found".to_string()
                    }),
                    organization_name: None,
                });
            }
        }
        Ok(out)
    }

    fn store_api_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        let (provider_id, api_key) = validate_api_key_login(provider_id, api_key)?;
        let mut credentials = self.load_credentials()?;
        credentials.api_keys.insert(provider_id, api_key);
        self.save_credentials(&credentials)
    }

    fn remove_api_key(&self, provider_id: &str) -> Result<()> {
        let provider_id = canonical_backend_provider_id(provider_id);
        validate_provider_id(&provider_id)?;
        let mut credentials = self.load_credentials()?;
        credentials.api_keys.remove(&provider_id);
        self.save_credentials(&credentials)
    }

    fn run_agent_turn(&self, events: EventEmitter, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        let message = string_param(&params, &["message"])?;
        let options = TurnLaunchOptions::from_params(&params);
        let turn_id = Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        self.turns
            .lock()
            .unwrap()
            .insert(turn_id.clone(), cancel.clone());
        let turn_id_thread = turn_id.clone();
        let session_id_thread = session_id.clone();
        thread::spawn(move || {
            run_agent_turn_thread(
                events,
                session_id_thread,
                turn_id_thread,
                message,
                options,
                cancel,
            );
        });
        serde_value(json!({"turnId": turn_id}))
    }

    fn load_config(&self) -> Result<StoredConfig> {
        let mut config: StoredConfig = read_json_or_default(&config_file()?)?;
        if config.default_provider.is_none() {
            config.default_provider = Some(DEFAULT_PROVIDER.to_string());
        }
        Ok(config)
    }

    fn save_config(&self, config: &StoredConfig) -> Result<()> {
        write_json(&config_file()?, config)
    }

    fn load_credentials(&self) -> Result<StoredCredentials> {
        read_json_or_default(&credentials_file()?)
    }

    fn save_credentials(&self, credentials: &StoredCredentials) -> Result<()> {
        write_json_private(&credentials_file()?, credentials)
    }

    fn load_sessions(&self) -> Result<Vec<SessionRecord>> {
        read_json_or_default(&sessions_file()?)
    }

    fn save_sessions(&self, sessions: &[SessionRecord]) -> Result<()> {
        write_json(&sessions_file()?, sessions)
    }
}

fn ensure_session_cwd(cwd: &Path) -> Result<()> {
    if cwd.exists() {
        if cwd.is_dir() {
            return Ok(());
        }
        bail!(
            "session cwd exists but is not a directory: {}",
            cwd.display()
        );
    }
    fs::create_dir_all(cwd)
        .with_context(|| format!("failed to create session cwd {}", cwd.display()))
}

fn run_agent_turn_thread(
    events: EventEmitter,
    session_id: String,
    turn_id: String,
    message: String,
    options: TurnLaunchOptions,
    cancel: Arc<AtomicBool>,
) {
    let channel = format!("session:{session_id}:event");
    emit_backend_event(
        &events,
        &channel,
        json!({"type": "turn-start", "turnId": turn_id}),
    );

    let outcome = run_agent_turn_inner(&events, &session_id, &turn_id, &message, &options, &cancel);
    match outcome {
        Ok(assistant_text) => {
            emit_backend_event(
                &events,
                &channel,
                json!({"type": "turn-complete", "turnId": turn_id, "assistantText": assistant_text}),
            );
            emit_backend_event(
                &events,
                "workspace:sessions:changed",
                json!({"sessionId": session_id, "reason": "turn_complete"}),
            );
        }
        Err(error) => {
            emit_backend_event(
                &events,
                &channel,
                json!({"type": "turn-error", "turnId": turn_id, "error": format!("{error:#}")}),
            );
        }
    }
}

fn persist_codex_outcome(session_id: &str, outcome: CodexTurnOutcome) -> Result<String> {
    let assistant_text = outcome.assistant_text.clone();
    if outcome.events.is_empty() {
        for tool in outcome.tools {
            append_codex_tool_event(session_id, tool)?;
        }
        let assistant_messages = if outcome.assistant_messages.is_empty() {
            vec![assistant_text.clone()]
        } else {
            outcome.assistant_messages
        };
        for text in assistant_messages {
            append_codex_assistant_event(session_id, text)?;
        }
        return Ok(assistant_text);
    }

    for event in outcome.events {
        match event {
            CapturedTurnEvent::Assistant(text) => append_codex_assistant_event(session_id, text)?,
            CapturedTurnEvent::Tool(tool) => append_codex_tool_event(session_id, tool)?,
        }
    }
    Ok(assistant_text)
}

fn append_codex_tool_event(
    session_id: &str,
    tool: codex_app_server::CapturedToolEvent,
) -> Result<()> {
    append_event(
        session_id,
        StoredEvent::Tool {
            at_ms: now_ms(),
            tool_id: tool.tool_id,
            input: tool.input,
            output: tool.output,
            success: tool.success,
        },
    )
}

fn append_codex_assistant_event(session_id: &str, text: String) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    append_event(
        session_id,
        StoredEvent::Assistant {
            at_ms: now_ms(),
            text,
        },
    )
}

fn run_agent_turn_inner(
    events: &EventEmitter,
    session_id: &str,
    turn_id: &str,
    message: &str,
    options: &TurnLaunchOptions,
    cancel: &AtomicBool,
) -> Result<String> {
    let channel = format!("session:{session_id}:event");
    let record = load_session_record(session_id)?;
    let provider_locked = !record.events.is_empty();
    append_event(
        session_id,
        StoredEvent::User {
            at_ms: now_ms(),
            text: message.to_string(),
        },
    )?;

    let config = read_config()?;
    let routing = resolve_turn_routing(&record, &config, options, provider_locked);
    let provider = routing.provider;
    let model = routing.model;
    update_session_routing(session_id, &provider, model.as_deref())?;
    let credentials: StoredCredentials = read_json_or_default(&credentials_file()?)?;
    if provider == "codex" {
        let command = ensure_provider_command("codex")?;
        emit_backend_event(
            events,
            &channel,
            json!({"type": "thinking-delta", "turnId": turn_id, "delta": "Starting Codex app-server\n"}),
        );
        let outcome = codex_app_server::run_turn(
            &command,
            events,
            &channel,
            turn_id,
            CodexTurnOptions {
                model: model.as_deref(),
                cwd: &record.cwd,
                message,
                thinking_option_id: options.thinking_option_id.as_deref(),
                fast_mode: options.fast_mode,
                permission_mode: options.permission_mode.as_deref(),
                api_key: credentials.api_keys.get("codex").map(String::as_str),
                playwright_cdp_endpoint: None,
                cancel,
            },
        )?;
        let assistant_text = persist_codex_outcome(session_id, outcome)?;
        return Ok(assistant_text);
    }
    let launch =
        build_provider_command(&provider, model.as_deref(), &record.cwd, message, options)?;

    emit_backend_event(
        events,
        &channel,
        json!({"type": "thinking-delta", "turnId": turn_id, "delta": format!("Starting {}", launch.label)}),
    );

    let mut command = Command::new(&launch.command);
    command
        .args(&launch.args)
        .current_dir(&record.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(api_key) = credentials.api_keys.get(&provider) {
        match provider.as_str() {
            "codex" => {
                command.env("OPENAI_API_KEY", api_key);
            }
            "claude" => {
                command.env("ANTHROPIC_API_KEY", api_key);
            }
            "puffer" => {
                command.env("PUFFER_API_KEY", api_key);
            }
            _ => {}
        }
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn {}", launch.command))?;
    let stdout = child.stdout.take().context("missing child stdout")?;
    let stderr = child.stderr.take().context("missing child stderr")?;
    let (tx, rx) = std::sync::mpsc::channel::<ProcessLine>();
    {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                let _ = tx.send(ProcessLine::Stdout(line));
            }
        });
    }
    {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr)
                .lines()
                .map_while(std::result::Result::ok)
            {
                let _ = tx.send(ProcessLine::Stderr(line));
            }
        });
    }
    drop(tx);

    let mut assistant_text = String::new();
    let mut raw_stdout = String::new();
    let mut stderr_text = String::new();
    while let Ok(line) = rx.recv() {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            bail!("turn canceled");
        }
        match line {
            ProcessLine::Stdout(line) => {
                raw_stdout.push_str(&line);
                raw_stdout.push('\n');
                if launch.json_stream {
                    if let Ok(value) = serde_json::from_str::<Value>(&line) {
                        if let Some(delta) = extract_text_delta(&value) {
                            assistant_text.push_str(&delta);
                            emit_backend_event(
                                events,
                                &channel,
                                json!({"type": "text-delta", "turnId": turn_id, "delta": delta}),
                            );
                        } else if is_tool_event(&value) {
                            emit_backend_event(
                                events,
                                &channel,
                                json!({
                                    "type": "tool-invocations",
                                    "turnId": turn_id,
                                    "invocations": [{
                                        "callId": Uuid::new_v4().to_string(),
                                        "toolId": value.get("type").and_then(Value::as_str).unwrap_or("tool"),
                                        "input": serde_json::to_string(&value).unwrap_or_default(),
                                        "output": "",
                                        "success": true,
                                    }]
                                }),
                            );
                        }
                    }
                } else {
                    assistant_text.push_str(&line);
                    assistant_text.push('\n');
                    emit_backend_event(
                        events,
                        &channel,
                        json!({"type": "text-delta", "turnId": turn_id, "delta": format!("{line}\n")}),
                    );
                }
            }
            ProcessLine::Stderr(line) => {
                stderr_text.push_str(&line);
                stderr_text.push('\n');
                emit_backend_event(
                    events,
                    &channel,
                    json!({"type": "thinking-delta", "turnId": turn_id, "delta": format!("{line}\n")}),
                );
            }
        }
    }

    let status = child.wait().context("failed to wait for provider")?;
    if assistant_text.trim().is_empty() && !raw_stdout.trim().is_empty() {
        assistant_text = raw_stdout;
    }

    if !status.success() {
        append_event(
            session_id,
            StoredEvent::System {
                at_ms: now_ms(),
                text: format!(
                    "{} exited with status {status}. {}",
                    launch.label,
                    stderr_text.trim()
                ),
            },
        )?;
        bail!(
            "{} exited with status {status}: {}",
            launch.label,
            stderr_text.trim()
        );
    }

    let assistant_text = assistant_text.trim().to_string();
    append_event(
        session_id,
        StoredEvent::Assistant {
            at_ms: now_ms(),
            text: assistant_text.clone(),
        },
    )?;
    Ok(assistant_text)
}

fn build_provider_command(
    provider: &str,
    model: Option<&str>,
    cwd: &str,
    message: &str,
    options: &TurnLaunchOptions,
) -> Result<ProviderLaunch> {
    match provider {
        "codex" => {
            let command = ensure_provider_command("codex")?;
            let mut args = vec![
                "exec".to_string(),
                "--json".to_string(),
                "--skip-git-repo-check".to_string(),
                "-C".to_string(),
                cwd.to_string(),
            ];
            apply_codex_permission_args(&mut args, options.permission_mode.as_deref());
            if options.fast_mode {
                args.push("-c".to_string());
                args.push("model_service_tier=\"fast\"".to_string());
            }
            if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
                args.push("--model".to_string());
                args.push(model.to_string());
            }
            if let Some(effort) = options
                .thinking_option_id
                .as_deref()
                .filter(|value| !value.trim().is_empty() && *value != "default")
            {
                args.push("--effort".to_string());
                args.push(effort.to_string());
            }
            args.push(message.to_string());
            Ok(ProviderLaunch {
                label: "Codex".to_string(),
                command,
                args,
                json_stream: true,
            })
        }
        "claude" => {
            let command = ensure_provider_command("claude")?;
            let mut args = vec![
                "--print".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--include-partial-messages".to_string(),
                "--permission-mode".to_string(),
                "acceptEdits".to_string(),
            ];
            if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
                args.push("--model".to_string());
                args.push(model.to_string());
            }
            args.push(message.to_string());
            Ok(ProviderLaunch {
                label: "Claude".to_string(),
                command,
                args,
                json_stream: true,
            })
        }
        "puffer" => {
            let command = ensure_provider_command("puffer")?;
            Ok(ProviderLaunch {
                label: "Puffer".to_string(),
                command,
                args: vec!["--no-alt-screen".to_string(), message.to_string()],
                json_stream: false,
            })
        }
        other => bail!("unknown provider `{other}`"),
    }
}

fn append_event(session_id: &str, event: StoredEvent) -> Result<()> {
    let mut sessions: Vec<SessionRecord> = read_json_or_default(&sessions_file()?)?;
    let record = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| anyhow!("unknown session `{session_id}`"))?;
    if matches!(event, StoredEvent::User { .. })
        && record.generated_title.is_none()
        && record.display_name.is_none()
    {
        record.generated_title = Some(title_from_message(match &event {
            StoredEvent::User { text, .. } => text,
            _ => "",
        }));
        record.title = record
            .generated_title
            .clone()
            .unwrap_or_else(|| record.title.clone());
    }
    record.events.push(event);
    record.updated_at_ms = now_ms();
    write_json(&sessions_file()?, &sessions)
}

fn load_session_record(session_id: &str) -> Result<SessionRecord> {
    let sessions: Vec<SessionRecord> = read_json_or_default(&sessions_file()?)?;
    sessions
        .into_iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| anyhow!("unknown session `{session_id}`"))
}

fn update_session_routing(session_id: &str, provider: &str, model: Option<&str>) -> Result<()> {
    let mut sessions: Vec<SessionRecord> = read_json_or_default(&sessions_file()?)?;
    let record = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| anyhow!("unknown session `{session_id}`"))?;
    record.provider = provider.to_string();
    record.model = model.map(str::to_string);
    if !record.tags.iter().any(|tag| tag == provider) {
        record.tags.push(provider.to_string());
    }
    record.updated_at_ms = now_ms();
    write_json(&sessions_file()?, &sessions)
}

fn read_config() -> Result<StoredConfig> {
    let mut config: StoredConfig = read_json_or_default(&config_file()?)?;
    if config.default_provider.is_none() {
        config.default_provider = Some(DEFAULT_PROVIDER.to_string());
    }
    Ok(config)
}

fn extract_text_delta(value: &Value) -> Option<String> {
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if event_type.contains("result") || event_type.contains("usage") {
        return None;
    }
    let mut out = String::new();
    collect_text(value, &mut out);
    let trimmed = out.trim_matches('\0').to_string();
    if trimmed.trim().is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn collect_text(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            for key in ["delta", "text", "content"] {
                if let Some(value) = map.get(key) {
                    match value {
                        Value::String(text) => out.push_str(text),
                        _ => collect_text(value, out),
                    }
                }
            }
            if out.is_empty() {
                for value in map.values() {
                    collect_text(value, out);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_text(value, out);
            }
        }
        _ => {}
    }
}

fn is_tool_event(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .map(|value| value.contains("tool") || value.contains("exec") || value.contains("patch"))
        .unwrap_or(false)
}

fn emit_backend_event(events: &EventEmitter, event: &str, payload: Value) {
    events.emit(event.to_string(), payload);
}

fn provider_summaries() -> Vec<ProviderSummaryDto> {
    vec![
        ProviderSummaryDto {
            id: "puffer".to_string(),
            display_name: "Puffer".to_string(),
            base_url: "local-cli://puffer".to_string(),
            default_api: "cli".to_string(),
            model_count: provider_models("puffer").len(),
            auth_modes: vec!["native".to_string(), "api_key".to_string()],
            source_kind: "builtin".to_string(),
            source_path: None,
        },
        ProviderSummaryDto {
            id: "codex".to_string(),
            display_name: "Codex".to_string(),
            base_url: "local-cli://codex".to_string(),
            default_api: "cli".to_string(),
            model_count: provider_models("codex").len(),
            auth_modes: vec!["native".to_string(), "api_key".to_string()],
            source_kind: "builtin".to_string(),
            source_path: None,
        },
        ProviderSummaryDto {
            id: "claude".to_string(),
            display_name: "Claude".to_string(),
            base_url: "local-cli://claude".to_string(),
            default_api: "cli".to_string(),
            model_count: provider_models("claude").len(),
            auth_modes: vec!["native".to_string(), "api_key".to_string()],
            source_kind: "builtin".to_string(),
            source_path: None,
        },
    ]
}

fn provider_models(provider_id: &str) -> Vec<Value> {
    match canonical_backend_provider_id(provider_id).as_str() {
        "puffer" => vec![model("default", "Default", "puffer", false)],
        "claude" => claude_models(),
        _ => codex_app_server_models().unwrap_or_default(),
    }
}

fn codex_app_server_models() -> Result<Vec<Value>> {
    Ok(codex_app_server_catalog()?.models)
}

fn codex_app_server_catalog() -> Result<codex_app_server::CodexModelCatalog> {
    let command = ensure_provider_command("codex")?;
    codex_app_server::list_model_catalog(&command)
}

fn normalized_default_model(config: &StoredConfig) -> Option<String> {
    let provider = config
        .default_provider
        .as_deref()
        .unwrap_or(DEFAULT_PROVIDER);
    if provider != "codex" {
        return config
            .default_model
            .clone()
            .or_else(|| default_model_for(provider));
    }
    let catalog = codex_app_server_catalog().ok();
    let models = catalog
        .as_ref()
        .map(|catalog| catalog.models.as_slice())
        .unwrap_or(&[]);
    if let Some(default_model) = config.default_model.as_deref() {
        if models
            .iter()
            .any(|model| model.get("id").and_then(Value::as_str) == Some(default_model))
        {
            return Some(default_model.to_string());
        }
    }
    catalog.and_then(|catalog| catalog.default_model)
}

fn model(id: &str, display_name: &str, provider: &str, supports_reasoning: bool) -> Value {
    json!({
        "id": id,
        "displayName": display_name,
        "provider": provider,
        "api": "cli",
        "contextWindow": 0,
        "maxOutputTokens": 0,
        "supportsReasoning": supports_reasoning,
    })
}

fn claude_models() -> Vec<Value> {
    vec![
        claude_model(
            "claude-opus-4-7[1m]",
            "Opus 4.7 1M",
            "Opus 4.7 with 1M context window",
            true,
            false,
        ),
        claude_model(
            "claude-opus-4-7",
            "Opus 4.7",
            "Opus 4.7 · Latest release",
            true,
            false,
        ),
        claude_model(
            "claude-opus-4-6[1m]",
            "Opus 4.6 1M",
            "Opus 4.6 with 1M context window",
            true,
            false,
        ),
        claude_model(
            "claude-opus-4-6",
            "Opus 4.6",
            "Opus 4.6 · Most capable for complex work",
            true,
            true,
        ),
        claude_model(
            "claude-sonnet-4-6",
            "Sonnet 4.6",
            "Sonnet 4.6 · Best for everyday tasks",
            true,
            false,
        ),
        claude_model(
            "claude-haiku-4-5",
            "Haiku 4.5",
            "Haiku 4.5 · Fastest for quick answers",
            false,
            false,
        ),
    ]
}

fn claude_model(
    id: &str,
    display_name: &str,
    description: &str,
    supports_reasoning: bool,
    is_default: bool,
) -> Value {
    let thinking_options = if supports_reasoning {
        let efforts = if id.starts_with("claude-opus-4-7") {
            vec![
                ("low", "Low"),
                ("medium", "Medium"),
                ("high", "High"),
                ("xhigh", "Extra High"),
                ("max", "Max"),
            ]
        } else {
            vec![
                ("low", "Low"),
                ("medium", "Medium"),
                ("high", "High"),
                ("max", "Max"),
            ]
        };
        efforts
            .into_iter()
            .map(|(id, label)| json!({"id": id, "label": label}))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    json!({
        "id": id,
        "displayName": display_name,
        "description": description,
        "provider": "claude",
        "api": "cli",
        "contextWindow": 0,
        "maxOutputTokens": 0,
        "supportsReasoning": supports_reasoning,
        "isDefault": is_default,
        "thinkingOptions": thinking_options,
    })
}

fn default_model_for(provider: &str) -> Option<String> {
    match canonical_backend_provider_id(provider).as_str() {
        "claude" => Some(DEFAULT_CLAUDE_MODEL.to_string()),
        "puffer" => Some(DEFAULT_PUFFER_MODEL.to_string()),
        _ => codex_app_server_catalog()
            .ok()
            .and_then(|catalog| catalog.default_model),
    }
}

fn validate_provider_id(provider: &str) -> Result<()> {
    match canonical_backend_provider_id(provider).as_str() {
        "puffer" | "codex" | "claude" => Ok(()),
        other => bail!("unknown provider `{other}`"),
    }
}

fn canonical_backend_provider_id(provider: &str) -> String {
    let trimmed = provider.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "openai" | "codex" => "codex".to_string(),
        "anthropic" | "claude" => "claude".to_string(),
        "puffer" => "puffer".to_string(),
        _ => trimmed.to_string(),
    }
}

fn backend_provider_ids_match(left: &str, right: &str) -> bool {
    canonical_backend_provider_id(left) == canonical_backend_provider_id(right)
}

fn resolve_turn_routing(
    record: &SessionRecord,
    config: &StoredConfig,
    options: &TurnLaunchOptions,
    provider_locked: bool,
) -> TurnRouting {
    let provider = if provider_locked && !record.provider.trim().is_empty() {
        record.provider.clone()
    } else if let Some(provider) = options.provider_id.as_deref() {
        provider.to_string()
    } else if record.provider.trim().is_empty() {
        config
            .default_provider
            .clone()
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_string())
    } else {
        record.provider.clone()
    };
    let provider = canonical_backend_provider_id(&provider);
    let model =
        options
            .model_id
            .as_deref()
            .and_then(|model| normalize_backend_model_id_for_provider(&provider, model))
            .or_else(|| {
                backend_provider_ids_match(&record.provider, &provider)
                    .then(|| {
                        record.model.as_deref().and_then(|model| {
                            normalize_backend_model_id_for_provider(&provider, model)
                        })
                    })
                    .flatten()
            })
            .or_else(|| {
                config
                    .default_provider
                    .as_deref()
                    .filter(|default| backend_provider_ids_match(default, &provider))
                    .and_then(|_| {
                        config.default_model.as_deref().and_then(|model| {
                            normalize_backend_model_id_for_provider(&provider, model)
                        })
                    })
            })
            .or_else(|| default_model_for(&provider));
    TurnRouting { provider, model }
}

fn normalize_backend_model_id_for_provider(provider_id: &str, model_id: &str) -> Option<String> {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((prefix, model)) = trimmed.split_once('/') {
        let prefix = canonical_backend_provider_id(prefix);
        let model = model.trim();
        if prefix == canonical_backend_provider_id(provider_id) && !model.is_empty() {
            return Some(model.to_string());
        }
        return None;
    }
    Some(trimmed.to_string())
}

fn validate_api_key_login(provider_id: &str, api_key: &str) -> Result<(String, String)> {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        bail!("provider id cannot be empty");
    }
    let provider_id = canonical_backend_provider_id(provider_id);
    validate_provider_id(&provider_id)?;
    let api_key = api_key.trim();
    if api_key.is_empty() {
        bail!("api key cannot be empty");
    }
    Ok((provider_id, api_key.to_string()))
}

fn provider_command(provider: &str) -> String {
    let env_key = match provider {
        "claude" => "MOMO_CLAUDE_BIN",
        "puffer" => "MOMO_PUFFER_BIN",
        _ => "MOMO_CODEX_BIN",
    };
    if let Ok(value) = env::var(env_key) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    match provider {
        "claude" => "claude".to_string(),
        "puffer" => "puffer".to_string(),
        _ => "codex".to_string(),
    }
}

fn ensure_provider_command(provider: &str) -> Result<String> {
    let command = provider_command(provider);
    if command_exists(&command) {
        Ok(command)
    } else {
        bail!(
            "`{command}` is not installed or not executable. Set {} to an explicit binary path.",
            match provider {
                "claude" => "MOMO_CLAUDE_BIN",
                "puffer" => "MOMO_PUFFER_BIN",
                _ => "MOMO_CODEX_BIN",
            }
        )
    }
}

fn command_exists(command: &str) -> bool {
    Command::new(command)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn folder_label(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn record_title(record: &SessionRecord) -> String {
    record
        .display_name
        .clone()
        .or(record.generated_title.clone())
        .unwrap_or_else(|| record.title.clone())
}

fn title_from_message(message: &str) -> String {
    let title = message
        .lines()
        .next()
        .unwrap_or("New agent")
        .trim()
        .chars()
        .take(80)
        .collect::<String>();
    if title.is_empty() {
        "New agent".to_string()
    } else {
        title
    }
}

fn string_param(params: &Value, names: &[&str]) -> Result<String> {
    for name in names {
        if let Some(value) = params.get(*name).and_then(Value::as_str) {
            return Ok(value.to_string());
        }
    }
    bail!("missing parameter `{}`", names[0])
}

fn optional_string_param(params: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        params
            .get(*name)
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn optional_trimmed_string_param(params: &Value, names: &[&str]) -> Option<String> {
    optional_string_param(params, names).and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn serde_value<T: Serialize>(value: T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

fn stored_session_activity_status(events: &[StoredEvent]) -> &'static str {
    if latest_stored_action_requires_permission(events) {
        return "awaiting";
    }
    if latest_stored_action_is_unanswered(events) {
        return "running";
    }
    "idle"
}

fn latest_stored_action_requires_permission(events: &[StoredEvent]) -> bool {
    for event in events.iter().rev() {
        match event {
            StoredEvent::System { text, .. } => return text_requires_permission(text),
            StoredEvent::Tool { output, .. } => return output_requires_permission(output),
            StoredEvent::User { .. } | StoredEvent::Assistant { .. } => return false,
        }
    }
    false
}

fn latest_stored_action_is_unanswered(events: &[StoredEvent]) -> bool {
    for event in events.iter().rev() {
        match event {
            StoredEvent::User { .. } => return true,
            StoredEvent::Assistant { .. }
            | StoredEvent::System { .. }
            | StoredEvent::Tool { .. } => {
                return false;
            }
        }
    }
    false
}

fn text_requires_permission(text: &str) -> bool {
    output_requires_permission(text)
        || text
            .split_once('\n')
            .and_then(|(_, rest)| rest.strip_prefix("input: "))
            .and_then(|input| {
                input
                    .split_once('\n')
                    .map(|(_, output)| output_requires_permission(output))
            })
            .unwrap_or(false)
}

fn output_requires_permission(output: &str) -> bool {
    output.trim().strip_prefix("Permission required:").is_some()
}

fn read_json_or_default<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de> + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let text = fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(T::default());
    }
    Ok(serde_json::from_str(&text)?)
}

fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn write_json_private<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_json(path, value)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).ok();
    }
    Ok(())
}

fn app_home() -> Result<PathBuf> {
    if let Ok(path) = env::var("MOMO_HOME") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir().join(".momo"))
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn sessions_file() -> Result<PathBuf> {
    Ok(app_home()?.join("sessions.json"))
}

fn config_file() -> Result<PathBuf> {
    Ok(app_home()?.join("config.json"))
}

fn credentials_file() -> Result<PathBuf> {
    Ok(app_home()?.join("credentials.json"))
}

fn empty_repo_status(session_id: &str, cwd: &str) -> RepoStatusDto {
    RepoStatusDto {
        session_id: session_id.to_string(),
        cwd: cwd.to_string(),
        repo_root: None,
        branch: None,
        head_sha: None,
        is_clean: true,
        status_lines: Vec::new(),
        has_gh: false,
        gh_authenticated: false,
        can_create_pull_request: false,
        can_merge_pull_request: false,
        create_pull_request_reason: None,
        merge_pull_request_reason: None,
        open_pull_request: None,
        warnings: Vec::new(),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredConfig {
    default_provider: Option<String>,
    default_model: Option<String>,
    openai_base_url: Option<String>,
    theme: Option<String>,
}

impl Default for StoredConfig {
    fn default() -> Self {
        Self {
            default_provider: Some(DEFAULT_PROVIDER.to_string()),
            default_model: None,
            openai_base_url: None,
            theme: Some("system".to_string()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCredentials {
    api_keys: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRecord {
    id: String,
    display_name: Option<String>,
    generated_title: Option<String>,
    title: String,
    cwd: String,
    created_at_ms: u64,
    updated_at_ms: u64,
    slug: Option<String>,
    tags: Vec<String>,
    note: Option<String>,
    parent_session_id: Option<String>,
    provider: String,
    model: Option<String>,
    events: Vec<StoredEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredEvent {
    User {
        at_ms: u64,
        text: String,
    },
    Assistant {
        at_ms: u64,
        text: String,
    },
    System {
        at_ms: u64,
        text: String,
    },
    Tool {
        at_ms: u64,
        tool_id: String,
        input: String,
        output: String,
        success: bool,
    },
}

#[derive(Debug)]
struct ProviderLaunch {
    label: String,
    command: String,
    args: Vec<String>,
    json_stream: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TurnRouting {
    provider: String,
    model: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct TurnLaunchOptions {
    provider_id: Option<String>,
    model_id: Option<String>,
    thinking_option_id: Option<String>,
    fast_mode: bool,
    permission_mode: Option<String>,
}

impl TurnLaunchOptions {
    fn from_params(params: &Value) -> Self {
        Self {
            provider_id: optional_trimmed_string_param(params, &["providerId", "provider_id"]),
            model_id: optional_trimmed_string_param(params, &["modelId", "model_id"]),
            thinking_option_id: optional_trimmed_string_param(
                params,
                &["thinkingOptionId", "thinking_option_id", "effort"],
            ),
            fast_mode: params
                .get("fastMode")
                .or_else(|| params.get("fast_mode"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            permission_mode: optional_trimmed_string_param(
                params,
                &["permissionMode", "permission_mode"],
            )
            .and_then(|mode| match mode.as_str() {
                "read-only" | "workspace-write" | "full-access" => Some(mode),
                _ => None,
            }),
        }
    }
}

fn apply_codex_permission_args(args: &mut Vec<String>, permission_mode: Option<&str>) {
    match permission_mode.unwrap_or("workspace-write") {
        "read-only" => {
            args.push("--sandbox".to_string());
            args.push("read-only".to_string());
        }
        "full-access" => {
            args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
        }
        _ => {
            args.push("--full-auto".to_string());
        }
    }
}

enum ProcessLine {
    Stdout(String),
    Stderr(String),
}
