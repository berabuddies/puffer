use crate::codex_app_server::{self, CapturedTurnEvent, CodexTurnOptions, CodexTurnOutcome};
use crate::dtos::{
    AgentDiffDto, AgentDiffEntryDto, AgentDiffFileDto, AuthProviderStatusDto,
    BrowserCaptchaSettingsDto, BrowserCaptchaSolverDto, BrowserSettingsDto, DiffSummaryDto,
    DivergenceReportDto, ExternalCredentialDto, FolderGroupDto, ProviderSummaryDto,
    ResourceCountsDto, SecretSummaryDto, SecretsSettingsDto, SessionDetailDto, SessionListItemDto,
    SettingsConfigDto, SettingsSessionSummaryDto, SettingsSnapshotDto, TimelineItemDto,
};
use crate::events::EventEmitter;
use crate::repo_actions;
use crate::{browser, files, fs_watch, local_model, lsp, pty};
use anyhow::{anyhow, bail, Context, Result};
use base64::prelude::*;
use puffer_config::builtin_captcha_solvers;
use puffer_secrets::{SecretSummary, SecretUpsert, SecretVault};
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
const REMOTE_FILE_WRITE_MAX_BYTES: usize = 5 * 1024 * 1024;
const MAX_GIT_CLONE_DEPTH: u64 = 10_000;
const MAX_UNTRACKED_DIFF_FILES: usize = 128;
const MAX_UNTRACKED_DIFF_FILE_BYTES: u64 = 256 * 1024;
const MAX_UNTRACKED_DIFF_PATCH_BYTES: usize = 512 * 1024;
const DEFAULT_PTY_COLS: u16 = 100;
const DEFAULT_PTY_ROWS: u16 = 30;
const MAX_PTY_COLS: u16 = 500;
const MAX_PTY_ROWS: u16 = 200;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveSecretParams {
    id: Option<String>,
    label: String,
    value: String,
    description: Option<String>,
    username: Option<String>,
    origin: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteSecretParams {
    id: String,
}

pub(crate) struct BackendState {
    ptys: Arc<pty::PtyRegistry>,
    fs_watches: Arc<fs_watch::FsWatchRegistry>,
    browsers: browser::BrowserRegistry,
    local_models: local_model::LocalModelInstaller,
    turns: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl BackendState {
    /// Creates backend state shared by desktop command handlers.
    pub(crate) fn new() -> Self {
        let ptys = Arc::new(pty::PtyRegistry::new());
        ptys.spawn_idle_pruner();
        let browser_profile_root = app_home()
            .unwrap_or_else(|_| home_dir().join(".corbina"))
            .join("browser-profiles");
        Self {
            ptys,
            fs_watches: Arc::new(fs_watch::FsWatchRegistry::new()),
            browsers: browser::BrowserRegistry::new(browser_profile_root),
            local_models: local_model::LocalModelInstaller::new(),
            turns: Mutex::new(HashMap::new()),
        }
    }

    /// Dispatches one desktop backend request by method name.
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
            "refresh_repo_status" => {
                let session_id = string_param(&params, &["sessionId", "session_id"])?;
                let record = self.load_session(&session_id)?;
                serde_value(repo_actions::repo_status(
                    &session_id,
                    Path::new(&record.cwd),
                ))
            }
            "create_pull_request" => {
                let session_id = string_param(&params, &["sessionId", "session_id"])?;
                let record = self.load_session(&session_id)?;
                let title = optional_string_param(&params, &["title"]);
                let body = optional_string_param(&params, &["body"]);
                serde_value(repo_actions::create_pull_request(
                    &session_id,
                    Path::new(&record.cwd),
                    title,
                    body,
                ))
            }
            "merge_pull_request" => {
                let session_id = string_param(&params, &["sessionId", "session_id"])?;
                let record = self.load_session(&session_id)?;
                let number = params
                    .get("pullRequestNumber")
                    .or_else(|| params.get("pull_request_number"))
                    .and_then(Value::as_u64);
                let method = optional_string_param(&params, &["mergeMethod", "merge_method"]);
                serde_value(repo_actions::merge_pull_request(
                    &session_id,
                    Path::new(&record.cwd),
                    number,
                    method,
                ))
            }
            "load_settings_snapshot" => serde_value(self.load_settings_snapshot()?),
            "login_with_oauth" => serde_value(self.load_settings_snapshot()?),
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
            "list_external_credentials" => serde_value(self.list_external_credentials()?),
            "import_external_credential" => serde_value(self.load_settings_snapshot()?),
            "save_secret" => {
                self.save_secret(params)?;
                serde_value(self.load_settings_snapshot()?)
            }
            "delete_secret" => {
                self.delete_secret(params)?;
                serde_value(self.load_settings_snapshot()?)
            }
            "import_chrome_secrets" => {
                let report = self.secret_vault()?.import_chrome_saved_credentials()?;
                serde_value(json!({
                    "settings": self.load_settings_snapshot()?,
                    "report": report,
                }))
            }
            "run_remote_bash" => self.run_remote_bash(params),
            "read_remote_file" => self.read_remote_file(params),
            "write_remote_file" => self.write_remote_file(params),
            "create_session" => {
                let cwd = optional_string_param(&params, &["cwd"])
                    .map(PathBuf::from)
                    .unwrap_or(self.default_workspace()?);
                let provider =
                    optional_string_param(&params, &["providerId", "provider_id", "provider"]);
                let model = optional_string_param(&params, &["modelId", "model_id", "model"]);
                serde_value(self.create_session(cwd, provider, model)?)
            }
            "default_workspace" => {
                let cwd = self.default_workspace()?;
                serde_value(json!({
                    "cwd": cwd.display().to_string(),
                    "workspaceRoot": cwd.display().to_string(),
                }))
            }
            "git_clone" => self.git_clone(events.clone(), params),
            "load_desktop_pins" => serde_value(self.load_pins()?),
            "set_desktop_pin" => self.set_desktop_pin(params),
            "rename_session" => {
                let session_id = string_param(&params, &["sessionId", "session_id"])?;
                let title = string_param(&params, &["title"])?;
                self.rename_session(&session_id, title)?;
                serde_value(self.load_session_detail(&session_id)?)
            }
            "workflow_list" => serde_value(json!({"workflows": [], "runs": []})),
            "workflow_runs_list" => serde_value(Vec::<Value>::new()),
            "workflow_run_show" => Ok(Value::Null),
            "run_agent_turn" => self.run_agent_turn(events.clone(), params),
            "resolve_permission" | "resolve_user_question" => Ok(json!({})),
            "cancel_turn" => {
                let turn_id = string_param(&params, &["turnId", "turn_id"])?;
                if let Some(flag) = self.turns.lock().unwrap().get(&turn_id) {
                    flag.store(true, Ordering::SeqCst);
                }
                Ok(json!({}))
            }
            "list_dir" => files::list_dir(&params, &self.allowed_roots()?),
            "read_file" => files::read_file(&params, &self.allowed_roots()?),
            "write_file" => files::write_file(&params, &self.allowed_roots()?),
            "load_file_tabs" => self.load_file_tabs(params),
            "save_file_tabs" => self.save_file_tabs(params),
            "lsp_inspect" => lsp::inspect(&params, &self.allowed_roots()?),
            "fs_watch" => fs_watch::handle_fs_watch(
                &self.fs_watches,
                events.clone(),
                &params,
                &self.allowed_roots()?,
            ),
            "fs_unwatch" => fs_watch::handle_fs_unwatch(&self.fs_watches, &params),
            "pty_list" => self.pty_list(params),
            "pty_open" => self.pty_open(events.clone(), params),
            "pty_focus" => self.pty_focus(params),
            "pty_replay" => self.pty_replay(params),
            "pty_rename" => self.pty_rename(params),
            "pty_write" => self.pty_write(params),
            "pty_resize" => self.pty_resize(params),
            "pty_close" => self.pty_close(params),
            "browser_open" => self.browser_open(events.clone(), params),
            "browser_navigate" => self.browser_navigate(params),
            "browser_reload" => self.browser_reload(params),
            "browser_history" => self.browser_history(params),
            "browser_resize" => self.browser_resize(params),
            "browser_input" => self.browser_input(params),
            "browser_copy_selection" => self.browser_copy_selection(params),
            "browser_cursor" => self.browser_cursor(params),
            "browser_close" => self.browser_close(params),
            "browser_agent" => self.browser_agent(events.clone(), params),
            "browser_recording" => self.browser_recording(params),
            "list_mcp_servers" => serde_value(json!({"servers": self.list_mcp_servers()?})),
            "add_mcp_server" => serde_value(json!({"servers": self.list_mcp_servers()?})),
            "list_provider_models" => {
                let provider_id = string_param(&params, &["providerId", "provider_id"])?;
                serde_value(json!({
                    "providerId": provider_id,
                    "models": self.provider_models(&provider_id),
                }))
            }
            "list_permissions" => serde_value(json!({
                "path": permissions_file()?.display().to_string(),
                "tools": self.load_permissions()?,
            })),
            "save_permissions" => self.save_permissions(params),
            "update_config" => {
                self.update_config(params)?;
                serde_value(self.load_settings_snapshot()?)
            }
            "local_model_status" => {
                let model_id = optional_string_param(&params, &["modelId", "model_id"])
                    .unwrap_or_else(|| "minicpm5".to_string());
                serde_value(self.local_models.status(&model_id)?)
            }
            "install_local_model" => {
                let model_id = optional_string_param(&params, &["modelId", "model_id"])
                    .unwrap_or_else(|| "minicpm5".to_string());
                serde_value(
                    self.local_models
                        .install_or_start(events.clone(), &model_id)?,
                )
            }
            other => bail!("unsupported backend method `{other}`"),
        }
    }

    fn default_workspace(&self) -> Result<PathBuf> {
        Ok(env::current_dir().context("failed to read current directory")?)
    }

    fn allowed_roots(&self) -> Result<Vec<PathBuf>> {
        let mut roots = Vec::new();
        push_canonical_root(&mut roots, self.default_workspace()?);
        push_canonical_root(&mut roots, home_dir());
        for session in self.load_sessions()? {
            push_canonical_root(&mut roots, PathBuf::from(session.cwd));
        }
        roots.sort();
        roots.dedup();
        Ok(roots)
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
        let latest_diff = current_diff_snapshot(session_id, &record.cwd);
        let timeline = self.timeline_items(&record, latest_diff.clone());
        let repo_status = repo_actions::repo_status(session_id, Path::new(&record.cwd));
        let agent_diff = build_agent_diff(&record);
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
            latest_diff: latest_diff.clone(),
            diff_history: latest_diff.into_iter().collect(),
            repo_status,
            agent_diff: agent_diff.clone(),
            divergence: DivergenceReportDto {
                agent_only: Vec::new(),
                git_only: Vec::new(),
                agent_total: agent_diff.files.len(),
                git_total: 0,
            },
        })
    }

    fn timeline_items(
        &self,
        record: &SessionRecord,
        latest_diff: Option<DiffSummaryDto>,
    ) -> Vec<TimelineItemDto> {
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
        if let Some(snapshot) = latest_diff {
            items.push(TimelineItemDto::DiffSnapshot {
                id: "current-diff".to_string(),
                snapshot,
            });
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
                app_name: "Corbina".to_string(),
                default_provider: config.default_provider.clone(),
                default_model,
                openai_base_url: config.openai_base_url.clone(),
                theme: config.theme.clone().unwrap_or_else(|| "system".to_string()),
                mascot_id: "corbina".to_string(),
                mascot_display_name: "Corbina".to_string(),
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
                mcp_servers: 1,
                ides: 0,
            },
            sessions: SettingsSessionSummaryDto {
                total_sessions: sessions.len(),
                folder_groups: self.list_grouped_sessions()?.len(),
            },
            auth,
            providers,
            browser: legacy_browser_settings(&home),
            secrets: self.secret_settings(&home)?,
        })
    }

    fn save_secret(&self, params: Value) -> Result<()> {
        let input: SaveSecretParams =
            serde_json::from_value(params).context("invalid secret save params")?;
        self.secret_vault()?.put(SecretUpsert {
            id: input.id,
            label: input.label,
            description: input.description,
            value: input.value,
            username: input.username,
            origin: input.origin,
            source: "manual".to_string(),
        })?;
        Ok(())
    }

    fn delete_secret(&self, params: Value) -> Result<()> {
        let input: DeleteSecretParams =
            serde_json::from_value(params).context("invalid secret delete params")?;
        let _ = self.secret_vault()?.delete(&input.id)?;
        Ok(())
    }

    fn secret_settings(&self, home: &Path) -> Result<SecretsSettingsDto> {
        let store_file = home.join("secrets.json");
        let vault = SecretVault::open(&store_file)?;
        Ok(SecretsSettingsDto {
            store_file: store_file.display().to_string(),
            key_source: secret_key_source().to_string(),
            chrome_import_supported: cfg!(target_os = "macos"),
            items: vault.list()?.into_iter().map(secret_summary).collect(),
        })
    }

    fn secret_vault(&self) -> Result<SecretVault> {
        SecretVault::open(app_home()?.join("secrets.json"))
    }

    fn provider_models(&self, provider_id: &str) -> Vec<Value> {
        provider_models(provider_id)
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

    fn list_external_credentials(&self) -> Result<Vec<ExternalCredentialDto>> {
        let mut out = Vec::new();
        let codex = home_dir().join(".codex/auth.json");
        if codex.exists() {
            out.push(ExternalCredentialDto {
                provider_id: "codex".to_string(),
                source: "codex".to_string(),
                kind: "oauth".to_string(),
                description: "Codex CLI credentials".to_string(),
                source_path: codex.display().to_string(),
            });
        }
        let claude = home_dir().join(".claude");
        if claude.exists() {
            out.push(ExternalCredentialDto {
                provider_id: "claude".to_string(),
                source: "claude".to_string(),
                kind: "oauth".to_string(),
                description: "Claude Code credentials".to_string(),
                source_path: claude.display().to_string(),
            });
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

    fn run_remote_bash(&self, params: Value) -> Result<Value> {
        let command = string_param(&params, &["command"])?;
        let output = Command::new("bash")
            .arg("-lc")
            .arg(command)
            .current_dir(self.default_workspace()?)
            .output()
            .context("failed to execute bash")?;
        serde_value(json!({
            "success": output.status.success(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }))
    }

    fn read_remote_file(&self, params: Value) -> Result<Value> {
        let path = string_param(&params, &["path"])?;
        let path = files::validate_path(&self.allowed_roots()?, &path)?;
        let content = fs::read_to_string(&path).context("failed to read file")?;
        serde_value(json!({"success": true, "stdout": content, "stderr": ""}))
    }

    fn write_remote_file(&self, params: Value) -> Result<Value> {
        let path = string_param(&params, &["path"])?;
        let encoded = string_param(&params, &["contentsBase64", "contents_base64"])?;
        let bytes = BASE64_STANDARD.decode(encoded).context("invalid base64")?;
        if bytes.len() > REMOTE_FILE_WRITE_MAX_BYTES {
            bail!(
                "file is too large to write ({} bytes, hard limit {} bytes)",
                bytes.len(),
                REMOTE_FILE_WRITE_MAX_BYTES
            );
        }
        let path = files::validate_write_path(&self.allowed_roots()?, &path)?;
        fs::write(&path, bytes).context("failed to write file")?;
        serde_value(json!({"success": true, "stdout": "", "stderr": ""}))
    }

    fn git_clone(&self, events: EventEmitter, params: Value) -> Result<Value> {
        let url = string_param(&params, &["url"])?;
        let dest_raw = string_param(&params, &["dest"])?;
        let depth = parse_git_clone_depth(&params)?;
        let base = self.default_workspace()?;
        let dest = validate_git_clone_dest(&self.allowed_roots()?, &base, &dest_raw)?;
        let clone_id = Uuid::new_v4().to_string();
        let clone_id_thread = clone_id.clone();
        let dest_thread = dest.clone();
        thread::spawn(move || {
            let mut command = Command::new("git");
            command.arg("clone").arg("--progress");
            if let Some(depth) = depth {
                command.arg("--depth").arg(depth.to_string());
            }
            command.arg(&url).arg(&dest_thread);
            let output = command.output();
            let (ok, stdout, stderr, exit_code) = match output {
                Ok(output) => (
                    output.status.success(),
                    String::from_utf8_lossy(&output.stdout).to_string(),
                    String::from_utf8_lossy(&output.stderr).to_string(),
                    output.status.code(),
                ),
                Err(error) => (false, String::new(), error.to_string(), None),
            };
            for line in stderr.lines() {
                emit_backend_event(
                    &events,
                    &format!("clone:{clone_id_thread}:progress"),
                    json!({"cloneId": clone_id_thread, "line": line}),
                );
            }
            emit_backend_event(
                &events,
                &format!("clone:{clone_id_thread}:done"),
                json!({
                    "cloneId": clone_id_thread,
                    "ok": ok,
                    "dest": dest_thread.display().to_string(),
                    "stdout": stdout,
                    "stderr": stderr,
                    "exitCode": exit_code,
                }),
            );
        });
        serde_value(json!({"cloneId": clone_id, "dest": dest.display().to_string()}))
    }

    fn load_pins(&self) -> Result<Value> {
        read_json_or_default(&pins_file()?)
    }

    fn set_desktop_pin(&self, params: Value) -> Result<Value> {
        let kind = string_param(&params, &["kind"])?;
        let id = string_param(&params, &["id"])?;
        let pinned = params
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut pins = self.load_pins()?;
        let key = if kind == "workspace" {
            "pinnedWorkspacePaths"
        } else {
            "pinnedAgentIds"
        };
        let mut values = pins
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        values.retain(|value| value != &id);
        if pinned {
            values.push(id);
        }
        pins[key] = json!(values);
        write_json(&pins_file()?, &pins)?;
        Ok(pins)
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
        let browsers = self.browsers.clone();
        thread::spawn(move || {
            run_agent_turn_thread(
                events,
                browsers,
                session_id_thread,
                turn_id_thread,
                message,
                options,
                cancel,
            );
        });
        serde_value(json!({"turnId": turn_id}))
    }

    fn load_file_tabs(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        let tabs: Value = read_json_or_default(&file_tabs_file(&session_id)?)?;
        Ok(if tabs.is_null() {
            json!({"tabs": [], "activePath": null})
        } else {
            tabs
        })
    }

    fn save_file_tabs(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        let state = json!({
            "tabs": params.get("tabs").cloned().unwrap_or_else(|| json!([])),
            "activePath": params.get("activePath").cloned().unwrap_or(Value::Null),
        });
        write_json(&file_tabs_file(&session_id)?, &state)?;
        Ok(state)
    }

    fn pty_list(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        serde_value(self.ptys.list(&session_id))
    }

    fn pty_open(&self, events: EventEmitter, params: Value) -> Result<Value> {
        let session_id = optional_string_param(&params, &["sessionId", "session_id"])
            .unwrap_or_else(|| "default".to_string());
        let cwd = optional_string_param(&params, &["cwd"])
            .map(PathBuf::from)
            .unwrap_or(self.default_workspace()?);
        let cwd = validate_pty_cwd(&self.allowed_roots()?, &cwd)?;
        let cols = bounded_u16_param(&params, "cols", DEFAULT_PTY_COLS, MAX_PTY_COLS)?;
        let rows = bounded_u16_param(&params, "rows", DEFAULT_PTY_ROWS, MAX_PTY_ROWS)?;
        let title = optional_string_param(&params, &["title"]);
        let pty_id = self
            .ptys
            .open(events.clone(), session_id, cwd, cols, rows, title)?;
        serde_value(json!({"ptyId": pty_id}))
    }

    fn pty_focus(&self, params: Value) -> Result<Value> {
        let pty_id = string_param(&params, &["ptyId", "pty_id"])?;
        self.ptys.focus(&pty_id)?;
        Ok(json!({}))
    }

    fn pty_replay(&self, params: Value) -> Result<Value> {
        let pty_id = string_param(&params, &["ptyId", "pty_id"])?;
        let chunks = self.ptys.replay(&pty_id)?;
        serde_value(json!({"chunks": chunks}))
    }

    fn pty_rename(&self, params: Value) -> Result<Value> {
        let pty_id = string_param(&params, &["ptyId", "pty_id"])?;
        let title = string_param(&params, &["title"])?;
        serde_value(self.ptys.rename(&pty_id, title)?)
    }

    fn pty_write(&self, params: Value) -> Result<Value> {
        let pty_id = string_param(&params, &["ptyId", "pty_id"])?;
        let data = string_param(&params, &["data"])?;
        self.ptys.write(&pty_id, &data)?;
        Ok(json!({}))
    }

    fn pty_resize(&self, params: Value) -> Result<Value> {
        let pty_id = string_param(&params, &["ptyId", "pty_id"])?;
        let cols = bounded_u16_param(&params, "cols", DEFAULT_PTY_COLS, MAX_PTY_COLS)?;
        let rows = bounded_u16_param(&params, "rows", DEFAULT_PTY_ROWS, MAX_PTY_ROWS)?;
        self.ptys.resize(&pty_id, cols, rows)?;
        Ok(json!({}))
    }

    fn pty_close(&self, params: Value) -> Result<Value> {
        let pty_id = string_param(&params, &["ptyId", "pty_id"])?;
        self.ptys.close(&pty_id)?;
        Ok(json!({}))
    }

    fn browser_open(&self, events: EventEmitter, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        let url = optional_string_param(&params, &["url"]);
        let width = params.get("width").and_then(Value::as_u64).unwrap_or(960) as u32;
        let height = params.get("height").and_then(Value::as_u64).unwrap_or(720) as u32;
        let state = self
            .browsers
            .open(events.clone(), session_id, url, width, height)?;
        serde_value(json!({
            "url": state.url,
            "title": state.title,
            "loading": state.loading,
            "width": state.width,
            "height": state.height,
            "popOut": false
        }))
    }

    fn browser_navigate(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        let url = string_param(&params, &["url"])?;
        self.browsers.navigate(&session_id, url)?;
        Ok(json!({}))
    }

    fn browser_reload(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        self.browsers.reload(&session_id)?;
        Ok(json!({}))
    }

    fn browser_history(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        let direction = match string_param(&params, &["direction"])?.as_str() {
            "back" => browser::BrowserHistoryDirection::Back,
            "forward" => browser::BrowserHistoryDirection::Forward,
            other => bail!("unsupported browser history direction `{other}`"),
        };
        self.browsers.history(&session_id, direction)?;
        Ok(json!({}))
    }

    fn browser_resize(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        let width = params.get("width").and_then(Value::as_u64).unwrap_or(960) as u32;
        let height = params.get("height").and_then(Value::as_u64).unwrap_or(720) as u32;
        self.browsers.resize(&session_id, width, height)?;
        Ok(json!({}))
    }

    fn browser_input(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        let event = browser::params::parse_input_event(
            params
                .get("event")
                .ok_or_else(|| anyhow!("browser_input requires event"))?,
        )?;
        self.browsers.input(&session_id, event)?;
        Ok(json!({}))
    }

    fn browser_copy_selection(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        let selection = self.browsers.copy_selection(&session_id)?;
        serde_value(json!({"text": selection.text, "copiedFrom": selection.copied_from}))
    }

    fn browser_cursor(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        let x = params
            .get("x")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("browser_cursor requires x"))?;
        let y = params
            .get("y")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("browser_cursor requires y"))?;
        let cursor = self.browsers.cursor(&session_id, x, y)?;
        serde_value(json!({"cursor": cursor.cursor}))
    }

    fn browser_close(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        self.browsers.close(&session_id)?;
        Ok(json!({}))
    }

    fn browser_agent(&self, events: EventEmitter, params: Value) -> Result<Value> {
        browser::handle_browser_agent(&self.browsers, events.clone(), &params)
    }

    fn browser_recording(&self, params: Value) -> Result<Value> {
        let session_id = string_param(&params, &["sessionId", "session_id"])?;
        Ok(self.browsers.recording_frames(&session_id))
    }

    fn list_mcp_servers(&self) -> Result<Vec<Value>> {
        Ok(vec![json!({
            "id": "playwright",
            "displayName": "Playwright",
            "description": "Browser automation exposed to Codex and Claude through Playwright MCP",
            "transport": "stdio",
            "endpoint": "",
            "target": "npx --yes @playwright/mcp@latest --headless",
            "sourceKind": "builtin",
            "sourcePath": null,
        })])
    }

    fn load_permissions(&self) -> Result<HashMap<String, String>> {
        read_json_or_default(&permissions_file()?)
    }

    fn save_permissions(&self, params: Value) -> Result<Value> {
        let tools = params.get("tools").cloned().unwrap_or_else(|| json!({}));
        write_json(&permissions_file()?, &tools)?;
        serde_value(json!({"path": permissions_file()?.display().to_string(), "tools": tools}))
    }

    fn update_config(&self, params: Value) -> Result<()> {
        let mut config = self.load_config()?;
        if let Some(provider) = params.get("defaultProvider").and_then(Value::as_str) {
            config.default_provider = if provider.trim().is_empty() {
                None
            } else {
                Some(canonical_backend_provider_id(provider))
            };
            if params.get("defaultModel").is_none() {
                config.default_model = default_model_for(provider);
            }
        }
        if params.get("defaultProvider").is_some_and(Value::is_null) {
            config.default_provider = None;
        }
        if let Some(model) = params.get("defaultModel").and_then(Value::as_str) {
            config.default_model = if model.trim().is_empty() {
                None
            } else {
                Some(model.to_string())
            };
        }
        if params.get("defaultModel").is_some_and(Value::is_null) {
            config.default_model = None;
        }
        if let Some(theme) = params.get("theme").and_then(Value::as_str) {
            config.theme = Some(theme.to_string());
        }
        if let Some(base_url) = params.get("openaiBaseUrl").and_then(Value::as_str) {
            config.openai_base_url = if base_url.trim().is_empty() {
                None
            } else {
                Some(base_url.to_string())
            };
        }
        if params.get("openaiBaseUrl").is_some_and(Value::is_null) {
            config.openai_base_url = None;
        }
        self.save_config(&config)
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
    browsers: browser::BrowserRegistry,
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

    let outcome = run_agent_turn_inner(
        &events,
        &browsers,
        &session_id,
        &turn_id,
        &message,
        &options,
        &cancel,
    );
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
    browsers: &browser::BrowserRegistry,
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
        let playwright_cdp_endpoint = match browsers
            .cdp_endpoint_for_agent(events.clone(), session_id)
        {
            Ok(endpoint) => {
                emit_backend_event(
                    events,
                    &channel,
                    json!({
                        "type": "thinking-delta",
                        "turnId": turn_id,
                        "delta": format!("Connecting Playwright MCP to Corbina Browser at {endpoint}\n"),
                    }),
                );
                Some(endpoint)
            }
            Err(error) => {
                emit_backend_event(
                    events,
                    &channel,
                    json!({
                        "type": "thinking-delta",
                        "turnId": turn_id,
                        "delta": format!("Corbina Browser CDP unavailable; falling back to Playwright-managed browser: {error}\n"),
                    }),
                );
                None
            }
        };
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
                playwright_cdp_endpoint: playwright_cdp_endpoint.as_deref(),
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
                "-c".to_string(),
                "mcp_servers.playwright.command=\"npx\"".to_string(),
                "-c".to_string(),
                "mcp_servers.playwright.args=[\"--yes\",\"@playwright/mcp@latest\",\"--headless\"]"
                    .to_string(),
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
            let mcp_config = write_claude_mcp_config()?;
            let mut args = vec![
                "--print".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--include-partial-messages".to_string(),
                "--permission-mode".to_string(),
                "acceptEdits".to_string(),
                "--mcp-config".to_string(),
                mcp_config.display().to_string(),
                "--strict-mcp-config".to_string(),
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

fn write_claude_mcp_config() -> Result<PathBuf> {
    let path = app_home()?.join("playwright-mcp.json");
    let config = json!({
        "mcpServers": {
            "playwright": {
                "command": "npx",
                "args": ["--yes", "@playwright/mcp@latest", "--headless"]
            }
        }
    });
    write_json(&path, &config)?;
    Ok(path)
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

#[derive(Debug, Clone)]
struct AgentEditIntent {
    kind: String,
    path: String,
    summary: String,
}

fn build_agent_diff(record: &SessionRecord) -> AgentDiffDto {
    let mut entries = Vec::new();
    let mut by_path: BTreeMap<String, AgentDiffFileDto> = BTreeMap::new();

    for (idx, event) in record.events.iter().enumerate() {
        let StoredEvent::Tool {
            tool_id,
            input,
            output,
            success,
            ..
        } = event
        else {
            continue;
        };

        for intent in agent_edit_intents(tool_id, input, output) {
            let call_id = format!("event-{idx}");
            entries.push(AgentDiffEntryDto {
                call_id,
                tool_id: tool_id.clone(),
                kind: intent.kind.clone(),
                path: intent.path.clone(),
                success: *success,
                summary: intent.summary.clone(),
            });

            if *success {
                by_path
                    .entry(intent.path.clone())
                    .and_modify(|file| {
                        file.edit_count += 1;
                        file.latest_kind = intent.kind.clone();
                        file.latest_summary = intent.summary.clone();
                    })
                    .or_insert_with(|| AgentDiffFileDto {
                        path: intent.path,
                        latest_kind: intent.kind,
                        edit_count: 1,
                        latest_summary: intent.summary,
                    });
            }
        }
    }

    AgentDiffDto {
        files: by_path.into_values().collect(),
        entries,
    }
}

fn agent_edit_intents(tool_id: &str, input: &str, output: &str) -> Vec<AgentEditIntent> {
    let normalized = tool_id.to_lowercase();
    if !matches!(
        normalized.as_str(),
        "edit_file"
            | "edit"
            | "replace_in_file"
            | "write_file"
            | "write"
            | "apply_patch"
            | "apply_diff"
    ) {
        return Vec::new();
    }

    let input_value = serde_json::from_str::<Value>(input).ok();
    let output_value = serde_json::from_str::<Value>(output).ok();
    let mut intents = edit_intents_from_changes(output_value.as_ref());
    if intents.is_empty() {
        intents = edit_intents_from_changes(input_value.as_ref());
    }
    if !intents.is_empty() {
        return intents;
    }

    if let Some(obj) = input_value.as_ref().and_then(Value::as_object) {
        if normalized == "write_file" || normalized == "write" {
            if let Some(path) = obj
                .get("path")
                .or_else(|| obj.get("file_path"))
                .and_then(Value::as_str)
            {
                let content = obj
                    .get("contents")
                    .or_else(|| obj.get("content"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                intents.push(AgentEditIntent {
                    kind: "write".to_string(),
                    path: path.to_string(),
                    summary: summary_lines(content, 80),
                });
            }
        } else if normalized == "edit_file"
            || normalized == "edit"
            || normalized == "replace_in_file"
        {
            if let Some(path) = obj
                .get("path")
                .or_else(|| obj.get("file_path"))
                .and_then(Value::as_str)
            {
                let old = obj
                    .get("old")
                    .or_else(|| obj.get("old_string"))
                    .or_else(|| obj.get("oldText"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let new_text = obj
                    .get("new")
                    .or_else(|| obj.get("new_string"))
                    .or_else(|| obj.get("newText"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                intents.push(AgentEditIntent {
                    kind: "replace".to_string(),
                    path: path.to_string(),
                    summary: format!("- {old}\n+ {new_text}"),
                });
            }
        }
    }
    intents
}

fn edit_intents_from_changes(value: Option<&Value>) -> Vec<AgentEditIntent> {
    let Some(value) = value else {
        return Vec::new();
    };
    let changes = value
        .get("changes")
        .and_then(Value::as_array)
        .or_else(|| value.as_array());
    let Some(changes) = changes else {
        return Vec::new();
    };
    changes
        .iter()
        .filter_map(|change| {
            let path = change
                .get("path")
                .or_else(|| change.get("filePath"))
                .or_else(|| change.get("file_path"))
                .and_then(Value::as_str)?;
            let kind = change_kind(change);
            let summary = change
                .get("diff")
                .or_else(|| change.get("patch"))
                .and_then(Value::as_str)
                .map(|diff| summary_lines(diff, 80))
                .unwrap_or_else(|| kind.clone());
            Some(AgentEditIntent {
                kind,
                path: path.to_string(),
                summary,
            })
        })
        .collect()
}

fn change_kind(change: &Value) -> String {
    change
        .get("kind")
        .and_then(|kind| {
            kind.as_str().map(ToString::to_string).or_else(|| {
                kind.get("type")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
        })
        .unwrap_or_else(|| "edit".to_string())
}

fn summary_lines(text: &str, max_lines: usize) -> String {
    let mut out = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if text.lines().count() > max_lines {
        out.push_str("\n…");
    }
    out
}

fn current_diff_snapshot(session_id: &str, cwd: &str) -> Option<DiffSummaryDto> {
    let root = Path::new(cwd);
    let mut unstaged = git_output(root, &["diff", "--stat"]).unwrap_or_default();
    let staged = git_output(root, &["diff", "--cached", "--stat"]).unwrap_or_default();
    let mut patch = git_output(root, &["diff", "--", "."]).unwrap_or_default();
    let untracked =
        git_output(root, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
    let (untracked_stat, untracked_patch) = untracked_diff(root, &untracked);
    if !untracked_stat.trim().is_empty() {
        if !unstaged.ends_with('\n') && !unstaged.is_empty() {
            unstaged.push('\n');
        }
        unstaged.push_str(&untracked_stat);
    }
    if !untracked_patch.trim().is_empty() {
        if !patch.ends_with('\n') && !patch.is_empty() {
            patch.push('\n');
        }
        patch.push_str(&untracked_patch);
    }
    if unstaged.trim().is_empty() && staged.trim().is_empty() && patch.trim().is_empty() {
        return None;
    }
    Some(DiffSummaryDto {
        id: format!("{session_id}-current-diff"),
        source: "git".to_string(),
        command_label: "git diff".to_string(),
        status_text: "Working tree changes".to_string(),
        unstaged_diffstat: unstaged,
        staged_diffstat: staged,
        patch_excerpt: patch.chars().take(8000).collect(),
        patch,
    })
}

fn untracked_diff(root: &Path, files: &str) -> (String, String) {
    let mut stat = String::new();
    let mut patch = String::new();
    let mut skipped = 0usize;
    let mut processed = 0usize;
    for rel in files
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(MAX_UNTRACKED_DIFF_FILES)
    {
        processed += 1;
        let path = root.join(rel);
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if meta.len() > MAX_UNTRACKED_DIFF_FILE_BYTES {
            skipped += 1;
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let display_path = rel.replace('\\', "/");
        let line_count = content.lines().count();
        let marker_count = line_count.clamp(1, 60);
        stat.push_str(&format!(
            " {display_path} | {line_count} {}\n",
            "+".repeat(marker_count)
        ));
        patch.push_str(&format!(
            "diff --git a/{display_path} b/{display_path}\nnew file mode 100644\nindex 0000000..0000000\n--- /dev/null\n+++ b/{display_path}\n@@ -0,0 +1,{line_count} @@\n"
        ));
        for line in content.lines() {
            if patch.len() >= MAX_UNTRACKED_DIFF_PATCH_BYTES {
                skipped += 1;
                break;
            }
            patch.push('+');
            patch.push_str(line);
            patch.push('\n');
        }
        if patch.len() >= MAX_UNTRACKED_DIFF_PATCH_BYTES {
            break;
        }
    }
    let total = files.lines().filter(|line| !line.trim().is_empty()).count();
    if processed < total.min(MAX_UNTRACKED_DIFF_FILES) {
        skipped += total.min(MAX_UNTRACKED_DIFF_FILES) - processed;
    }
    if total > MAX_UNTRACKED_DIFF_FILES {
        skipped += total - MAX_UNTRACKED_DIFF_FILES;
    }
    if skipped > 0 {
        stat.push_str(&format!(
            " ... {skipped} untracked file(s) omitted by desktop diff limits\n"
        ));
        patch.push_str(&format!(
            "\n# {skipped} untracked file(s) omitted by desktop diff limits\n"
        ));
    }
    (stat, patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

    fn test_session_record(
        provider: &str,
        model: Option<&str>,
        events: Vec<StoredEvent>,
    ) -> SessionRecord {
        SessionRecord {
            id: "test-session".to_string(),
            display_name: None,
            generated_title: None,
            title: "Test session".to_string(),
            cwd: "/tmp/puffer-test".to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
            slug: None,
            tags: Vec::new(),
            note: None,
            parent_session_id: None,
            provider: provider.to_string(),
            model: model.map(ToOwned::to_owned),
            events,
        }
    }

    #[test]
    fn untracked_diff_omits_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("small.txt");
        let large = dir.path().join("large.txt");
        fs::write(&small, "hello\n").unwrap();
        fs::write(
            &large,
            vec![b'x'; (MAX_UNTRACKED_DIFF_FILE_BYTES as usize) + 1],
        )
        .unwrap();

        let (stat, patch) = untracked_diff(dir.path(), "small.txt\nlarge.txt\n");

        assert!(stat.contains("small.txt"));
        assert!(patch.contains("+hello"));
        assert!(stat.contains("omitted by desktop diff limits"));
        assert!(!patch.contains("large.txt"));
    }

    #[test]
    fn validate_api_key_login_rejects_empty_values() {
        let empty_provider = validate_api_key_login("  ", "sk-test").unwrap_err();
        assert!(empty_provider
            .to_string()
            .contains("provider id cannot be empty"));

        let empty_key = validate_api_key_login("anthropic", "  ").unwrap_err();
        assert!(empty_key.to_string().contains("api key cannot be empty"));
    }

    #[test]
    fn validate_api_key_login_trims_values() {
        let (provider, api_key) = validate_api_key_login("  anthropic  ", "  sk-test  ").unwrap();

        assert_eq!(provider, "claude");
        assert_eq!(api_key, "sk-test");
    }

    #[test]
    fn validate_api_key_login_canonicalizes_desktop_provider_aliases() {
        let (provider, _) = validate_api_key_login("openai", "sk-test").unwrap();
        assert_eq!(provider, "codex");

        let (provider, _) = validate_api_key_login("Claude", "sk-test").unwrap();
        assert_eq!(provider, "claude");

        let err = validate_api_key_login("unknown-provider", "sk-test").unwrap_err();
        assert!(err.to_string().contains("unknown provider"));
    }

    #[test]
    fn desktop_provider_aliases_map_to_tauri_cli_backends() {
        assert_eq!(canonical_backend_provider_id("openai"), "codex");
        assert_eq!(canonical_backend_provider_id("codex"), "codex");
        assert_eq!(canonical_backend_provider_id("anthropic"), "claude");
        assert_eq!(canonical_backend_provider_id("claude"), "claude");
        assert_eq!(canonical_backend_provider_id("puffer"), "puffer");
    }

    #[test]
    fn desktop_provider_aliases_match_for_default_routing() {
        assert!(backend_provider_ids_match("openai", "codex"));
        assert!(backend_provider_ids_match("anthropic", "claude"));
        assert!(backend_provider_ids_match("Codex", "OPENAI"));
        assert!(!backend_provider_ids_match("codex", "claude"));
    }

    #[test]
    fn desktop_provider_validation_accepts_frontend_canonical_ids() {
        validate_provider_id("openai").unwrap();
        validate_provider_id("anthropic").unwrap();

        let err = validate_provider_id("unknown-provider").unwrap_err();
        assert!(err.to_string().contains("unknown provider"));
    }

    #[test]
    fn desktop_default_models_accept_frontend_canonical_ids() {
        assert_eq!(
            default_model_for("anthropic"),
            Some(DEFAULT_CLAUDE_MODEL.to_string())
        );
        assert_eq!(
            default_model_for("puffer"),
            Some(DEFAULT_PUFFER_MODEL.to_string())
        );
    }

    #[test]
    fn first_turn_provider_override_does_not_reuse_previous_provider_model() {
        let record = test_session_record("claude", Some(DEFAULT_CLAUDE_MODEL), Vec::new());
        let config = StoredConfig {
            default_provider: Some("openai".to_string()),
            default_model: Some("openai/gpt-5.4".to_string()),
            ..StoredConfig::default()
        };
        let options = TurnLaunchOptions {
            provider_id: Some("openai".to_string()),
            ..TurnLaunchOptions::default()
        };

        let routing = resolve_turn_routing(&record, &config, &options, false);

        assert_eq!(routing.provider, "codex");
        assert_eq!(routing.model.as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn turn_routing_keeps_locked_session_provider_despite_provider_override() {
        let record = test_session_record(
            "claude",
            Some(DEFAULT_CLAUDE_MODEL),
            vec![StoredEvent::User {
                at_ms: 1,
                text: "hello".to_string(),
            }],
        );
        let config = StoredConfig {
            default_provider: Some("openai".to_string()),
            default_model: Some("openai/gpt-5.4".to_string()),
            ..StoredConfig::default()
        };
        let options = TurnLaunchOptions {
            provider_id: Some("openai".to_string()),
            ..TurnLaunchOptions::default()
        };

        let routing = resolve_turn_routing(&record, &config, &options, true);

        assert_eq!(routing.provider, "claude");
        assert_eq!(routing.model.as_deref(), Some(DEFAULT_CLAUDE_MODEL));
    }

    #[test]
    fn turn_routing_normalizes_explicit_matching_model_prefix() {
        let record = test_session_record("codex", Some("gpt-5.4"), Vec::new());
        let config = StoredConfig::default();
        let options = TurnLaunchOptions {
            provider_id: Some("anthropic".to_string()),
            model_id: Some("anthropic/claude-sonnet-4-5".to_string()),
            ..TurnLaunchOptions::default()
        };

        let routing = resolve_turn_routing(&record, &config, &options, false);

        assert_eq!(routing.provider, "claude");
        assert_eq!(routing.model.as_deref(), Some("claude-sonnet-4-5"));
    }

    #[test]
    fn session_cwd_initializer_creates_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("new-project").join("nested");

        ensure_session_cwd(&missing).unwrap();

        assert!(missing.is_dir());
    }

    #[test]
    fn session_cwd_initializer_rejects_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-directory");
        fs::write(&file, "not a directory").unwrap();

        let err = ensure_session_cwd(&file).unwrap_err();

        assert!(err.to_string().contains("not a directory"));
    }

    #[test]
    fn validate_remote_write_rejects_paths_outside_allowed_roots() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("secret.txt");
        let roots = vec![allowed.path().canonicalize().unwrap()];

        let err = files::validate_write_path(&roots, target.to_str().unwrap()).unwrap_err();

        assert!(err.to_string().contains("path escapes allowed roots"));
    }

    #[test]
    fn validate_remote_write_accepts_new_file_inside_allowed_root() {
        let allowed = tempfile::tempdir().unwrap();
        let target = allowed.path().join("created.txt");
        let roots = vec![allowed.path().canonicalize().unwrap()];

        let validated = files::validate_write_path(&roots, target.to_str().unwrap()).unwrap();
        fs::write(&validated, BASE64_STANDARD.decode("b2s=").unwrap()).unwrap();

        assert_eq!(fs::read_to_string(target).unwrap(), "ok");
    }

    #[test]
    fn validate_git_clone_dest_rejects_paths_outside_allowed_roots() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let roots = vec![allowed.path().canonicalize().unwrap()];

        let err = validate_git_clone_dest(
            &roots,
            allowed.path(),
            outside.path().join("repo").to_str().unwrap(),
        )
        .unwrap_err();

        assert!(err.to_string().contains("path escapes allowed roots"));
    }

    #[test]
    fn validate_git_clone_dest_rejects_relative_traversal() {
        let allowed = tempfile::tempdir().unwrap();
        let roots = vec![allowed.path().canonicalize().unwrap()];

        let err = validate_git_clone_dest(&roots, allowed.path(), "../repo").unwrap_err();

        assert!(err.to_string().contains("path escapes allowed roots"));
    }

    #[test]
    fn validate_pty_cwd_rejects_paths_outside_allowed_roots() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let roots = vec![allowed.path().canonicalize().unwrap()];

        let err = validate_pty_cwd(&roots, outside.path()).unwrap_err();

        assert!(err.to_string().contains("path escapes allowed roots"));
    }

    #[test]
    fn parse_git_clone_depth_rejects_zero_and_extreme_values() {
        assert!(parse_git_clone_depth(&json!({"depth": 0})).is_err());
        assert!(parse_git_clone_depth(&json!({"depth": MAX_GIT_CLONE_DEPTH + 1})).is_err());
        assert!(parse_git_clone_depth(&json!({"depth": "1"})).is_err());
        assert_eq!(
            parse_git_clone_depth(&json!({"depth": 1})).unwrap(),
            Some(1)
        );
    }

    #[test]
    fn bounded_u16_param_rejects_zero_and_overflow_values() {
        assert!(bounded_u16_param(&json!({"cols": 0}), "cols", 100, 500).is_err());
        assert!(bounded_u16_param(&json!({"cols": 65_536}), "cols", 100, 500).is_err());
        assert!(bounded_u16_param(&json!({"cols": "120"}), "cols", 100, 500).is_err());
        assert_eq!(
            bounded_u16_param(&json!({"cols": 120}), "cols", 100, 500).unwrap(),
            120
        );
    }

    #[test]
    fn file_tabs_file_rejects_path_components_in_session_id() {
        let err = file_tabs_file("../outside").unwrap_err();

        assert!(err.to_string().contains("simple identifier"));
    }
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

fn secret_key_source() -> &'static str {
    if env::var_os("PUFFER_SECRET_STORE_KEY").is_some() {
        "env"
    } else if cfg!(target_os = "macos") {
        "macos-keychain"
    } else {
        "local-key-file"
    }
}

fn secret_summary(summary: SecretSummary) -> SecretSummaryDto {
    SecretSummaryDto {
        id: summary.id,
        label: summary.label,
        description: summary.description,
        username: summary.username,
        origin: summary.origin,
        source: summary.source,
        created_at_ms: summary.created_at_ms,
        updated_at_ms: summary.updated_at_ms,
    }
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
        "claude" => "CORBINA_CLAUDE_BIN",
        "puffer" => "CORBINA_PUFFER_BIN",
        _ => "CORBINA_CODEX_BIN",
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
                "claude" => "CORBINA_CLAUDE_BIN",
                "puffer" => "CORBINA_PUFFER_BIN",
                _ => "CORBINA_CODEX_BIN",
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

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn validate_git_clone_dest(allowed_roots: &[PathBuf], base: &Path, raw: &str) -> Result<PathBuf> {
    let raw_path = Path::new(raw);
    let candidate = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        base.join(raw_path)
    };
    files::validate_write_path(allowed_roots, &candidate.display().to_string())
}

fn validate_pty_cwd(allowed_roots: &[PathBuf], cwd: &Path) -> Result<PathBuf> {
    files::validate_path(allowed_roots, &cwd.display().to_string())
}

fn parse_git_clone_depth(params: &Value) -> Result<Option<u64>> {
    let Some(value) = params.get("depth") else {
        return Ok(None);
    };
    let Some(depth) = value.as_u64() else {
        bail!("clone depth must be an unsigned integer");
    };
    if depth == 0 || depth > MAX_GIT_CLONE_DEPTH {
        bail!("clone depth must be between 1 and {MAX_GIT_CLONE_DEPTH}");
    }
    Ok(Some(depth))
}

fn bounded_u16_param(params: &Value, key: &str, default: u16, max: u16) -> Result<u16> {
    let Some(raw) = params.get(key) else {
        return Ok(default);
    };
    let Some(value) = raw.as_u64() else {
        bail!("{key} must be an unsigned integer");
    };
    if value == 0 || value > max as u64 {
        bail!("{key} must be between 1 and {max}");
    }
    Ok(value as u16)
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn push_canonical_root(roots: &mut Vec<PathBuf>, root: PathBuf) {
    let root = normalize_path(&root);
    if root.is_absolute() {
        roots.push(root);
    }
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
    if let Ok(path) = env::var("CORBINA_HOME") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir().join(".corbina"))
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

fn pins_file() -> Result<PathBuf> {
    Ok(app_home()?.join("pins.json"))
}

fn permissions_file() -> Result<PathBuf> {
    Ok(app_home()?.join("permissions.json"))
}

fn file_tabs_file(session_id: &str) -> Result<PathBuf> {
    validate_state_file_id(session_id, "session_id")?;
    Ok(app_home()?
        .join("file-tabs")
        .join(format!("{session_id}.json")))
}

fn validate_state_file_id(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("{field} must be a simple identifier");
    }
    Ok(())
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

fn legacy_browser_settings(home: &Path) -> BrowserSettingsDto {
    let resources_dir = home.join("resources");
    BrowserSettingsDto {
        extensions_enabled: true,
        extensions: Vec::new(),
        captcha: BrowserCaptchaSettingsDto {
            enabled: false,
            selected_solver: "nopecha".to_string(),
            solvers: builtin_captcha_solvers()
                .iter()
                .map(|solver| {
                    let extension_dir = resources_dir.join(solver.extension_path);
                    BrowserCaptchaSolverDto {
                        id: solver.id.to_string(),
                        display_name: solver.display_name.to_string(),
                        description: solver.description.to_string(),
                        enabled: solver.id == "nopecha",
                        base_url: solver.default_base_url.to_string(),
                        api_key_secret_id: None,
                        has_api_key: false,
                        version: solver.version.to_string(),
                        bundled: extension_dir.join("manifest.json").exists(),
                        extension_path: extension_dir.display().to_string(),
                        release_url: solver.release_url.to_string(),
                        download_url: solver.download_url.to_string(),
                        sha256: solver.sha256.to_string(),
                        license: solver.license.to_string(),
                    }
                })
                .collect(),
        },
    }
}

enum ProcessLine {
    Stdout(String),
    Stderr(String),
}
