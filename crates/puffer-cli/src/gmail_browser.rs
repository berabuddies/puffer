//! Gmail web connector backed by daemon-managed Chrome sessions.

use crate::gmail_browser_log as diag;
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::{Event, SubscriberCommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader, Lines};

#[path = "gmail_browser_actions.rs"]
mod gmail_browser_actions;
#[path = "gmail_browser_script.rs"]
mod gmail_browser_script;
use gmail_browser_script::GMAIL_INBOX_SCRIPT;

/// Connector and default connection slug used by the Gmail browser connector.
pub(crate) const CONNECTOR_SLUG: &str = "gmail-browser";
/// Default connection slug used when no per-connection topic is supplied.
pub(crate) const DEFAULT_CONNECTION: &str = "gmail-browser";
/// User-config subdirectory used for instantiated Gmail browser subscribers.
pub(crate) const STATE_ROOT: &str = "gmail-browser-accounts";

const CONFIG_FILE: &str = "config.toml";
const SEEN_FILE: &str = "seen.json";
const AUTH_STATE_FILE: &str = "auth_state.json";
const POLL_INTERVAL: Duration = Duration::from_secs(30);
const ERROR_BACKOFF: Duration = Duration::from_secs(10);
const GMAIL_LOAD_TIMEOUT: Duration = Duration::from_secs(20);
const GMAIL_EVALUATE_INTERVAL: Duration = Duration::from_secs(1);
const BROWSER_WIDTH: u32 = 1280;
const BROWSER_HEIGHT: u32 = 900;
const INITIAL_ROW_EMIT_LIMIT: u64 = 1;

/// Persisted Gmail browser connector configuration.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct GmailBrowserConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workspace_root: Option<PathBuf>,
    #[serde(default)]
    pub(crate) accounts: Vec<String>,
}

impl GmailBrowserConfig {
    /// Returns true when the connector has enough information to poll Gmail.
    pub(crate) fn is_configured(&self) -> bool {
        !self.accounts.is_empty()
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SeenState {
    #[serde(default)]
    initialized: bool,
    #[serde(default)]
    seen: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
struct AuthState {
    #[serde(default)]
    auth_required_accounts: BTreeMap<String, AuthRequiredAccount>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct AuthRequiredAccount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default)]
    updated_at_ms: u128,
}

impl AuthState {
    fn has_auth_required_for(&self, accounts: &[String]) -> bool {
        accounts
            .iter()
            .any(|account| self.auth_required_accounts.contains_key(account))
    }

    fn retain_configured_accounts(&mut self, accounts: &[String]) {
        let configured = accounts.iter().map(String::as_str).collect::<BTreeSet<_>>();
        self.auth_required_accounts
            .retain(|account, _| configured.contains(account.as_str()));
    }
}

struct SubscriberEnv {
    state_dir: PathBuf,
    topic: String,
}

impl SubscriberEnv {
    fn from_env() -> Self {
        let state_dir = std::env::var_os("PUFFER_SKILL_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./state"));
        let topic = std::env::var("PUFFER_SKILL_TOPIC")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CONNECTION.to_string());
        Self { state_dir, topic }
    }
}

struct CommandStream {
    lines: Lines<BufReader<tokio::io::Stdin>>,
}

impl CommandStream {
    fn new() -> Self {
        Self {
            lines: BufReader::new(tokio::io::stdin()).lines(),
        }
    }

    async fn next(&mut self) -> Result<Option<SubscriberCommand>> {
        loop {
            let Some(line) = self.lines.next_line().await? else {
                return Ok(None);
            };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<SubscriberCommand>(&line) {
                Ok(command) => return Ok(Some(command)),
                Err(error) => eprintln!("gmail-browser: ignored malformed command: {error}"),
            }
        }
    }
}

/// Loads Gmail browser config for one connection, if present.
pub(crate) fn load_config(
    paths: &ConfigPaths,
    connection_slug: &str,
) -> Result<Option<GmailBrowserConfig>> {
    load_config_from_dir(&state_dir(paths, connection_slug))
}

/// Returns whether one Gmail browser connection is currently usable for polling.
pub(crate) fn connection_auth_ok(paths: &ConfigPaths, connection_slug: &str) -> Result<bool> {
    let Some(config) = load_config(paths, connection_slug)? else {
        return Ok(false);
    };
    if !config.is_configured() {
        return Ok(false);
    }
    let auth_state = load_auth_state(&state_dir(paths, connection_slug))?;
    Ok(!auth_state.has_auth_required_for(&config.accounts))
}

/// Saves Gmail browser config for one connection.
pub(crate) fn save_config(
    paths: &ConfigPaths,
    workspace_root: &Path,
    connection_slug: &str,
    accounts: Vec<String>,
) -> Result<GmailBrowserConfig> {
    let accounts = normalize_accounts(accounts);
    anyhow::ensure!(
        !accounts.is_empty(),
        "gmail-browser requires at least one Google account"
    );
    let config = GmailBrowserConfig {
        workspace_root: Some(workspace_root.to_path_buf()),
        accounts,
    };
    let dir = state_dir(paths, connection_slug);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let raw = toml::to_string_pretty(&config).context("serialize Gmail browser config")?;
    let path = dir.join(CONFIG_FILE);
    fs::write(&path, raw).with_context(|| format!("write {}", path.display()))?;
    clear_auth_state(&dir)?;
    Ok(config)
}

/// Returns the per-connection state directory used by the subscriber runtime.
pub(crate) fn state_dir(paths: &ConfigPaths, connection_slug: &str) -> PathBuf {
    paths.user_config_dir.join(STATE_ROOT).join(connection_slug)
}

/// Runs the Gmail browser subscriber until the supervisor stops the process.
pub(crate) async fn run_subscriber() -> Result<()> {
    let env = SubscriberEnv::from_env();
    tokio::fs::create_dir_all(&env.state_dir)
        .await
        .with_context(|| format!("create {}", env.state_dir.display()))?;

    let mut seen = load_seen(&env.state_dir)?;
    diag::subscriber_start(
        &env.topic,
        &env.state_dir,
        seen.initialized,
        seen.seen.len(),
    );
    let mut handshake = None;
    let mut last_config_key = String::new();
    let mut commands = CommandStream::new();
    loop {
        let Some(config) = load_config_from_dir(&env.state_dir)? else {
            diag::config_required(&env.topic, &env.state_dir, "missing");
            emit_control(&env.topic, "config_required", json!({}))?;
            wait_or_handle_command(&env, None, &mut handshake, &mut commands, POLL_INTERVAL)
                .await?;
            continue;
        };
        if !config.is_configured() {
            diag::config_required(&env.topic, &env.state_dir, "no_accounts");
            emit_control(&env.topic, "config_required", json!({}))?;
            wait_or_handle_command(
                &env,
                Some(&config),
                &mut handshake,
                &mut commands,
                POLL_INTERVAL,
            )
            .await?;
            continue;
        }
        let config_key = config.accounts.join(",");
        if config_key != last_config_key {
            prune_auth_state(&env.state_dir, &config.accounts)?;
            let auth_required_count = load_auth_state(&env.state_dir)?
                .auth_required_accounts
                .len();
            diag::config_ready(
                &env.topic,
                &env.state_dir,
                config.accounts.len(),
                auth_required_count,
                seen.initialized,
                seen.seen.len(),
            );
            emit_control(
                &env.topic,
                "ready",
                json!({
                    "accounts": &config.accounts,
                }),
            )?;
            handshake = None;
            last_config_key = config_key;
        }
        let result = poll_once(&env, &config, &mut seen, &mut handshake).await;
        match result {
            Ok(()) => {
                save_seen(&env.state_dir, &seen)?;
                wait_or_handle_command(
                    &env,
                    Some(&config),
                    &mut handshake,
                    &mut commands,
                    POLL_INTERVAL,
                )
                .await?;
            }
            Err(error) => {
                handshake = None;
                diag::poll_loop_error(&env.topic, &error);
                emit_control(
                    &env.topic,
                    "poll_error",
                    json!({ "error": format!("{error:#}") }),
                )?;
                wait_or_handle_command(
                    &env,
                    Some(&config),
                    &mut handshake,
                    &mut commands,
                    ERROR_BACKOFF,
                )
                .await?;
            }
        }
    }
}

async fn wait_or_handle_command(
    env: &SubscriberEnv,
    config: Option<&GmailBrowserConfig>,
    handshake: &mut Option<crate::daemon::Handshake>,
    commands: &mut CommandStream,
    delay: Duration,
) -> Result<()> {
    tokio::select! {
        _ = tokio::time::sleep(delay) => Ok(()),
        command = commands.next() => {
            let Some(command) = command? else {
                tokio::time::sleep(delay).await;
                return Ok(());
            };
            handle_command(env, config, handshake, command)
        }
    }
}

fn handle_command(
    env: &SubscriberEnv,
    config: Option<&GmailBrowserConfig>,
    handshake: &mut Option<crate::daemon::Handshake>,
    command: SubscriberCommand,
) -> Result<()> {
    match command {
        SubscriberCommand::Custom { op, args } if op == "gmail_browser_act" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let input = args.get("input").cloned().unwrap_or_else(|| json!({}));
            let Some(config) = config else {
                emit_control(
                    &env.topic,
                    "gmail_browser_action_error",
                    json!({
                        "op": op,
                        "action": action,
                        "error": "gmail-browser connector is not configured yet",
                    }),
                )?;
                return Ok(());
            };
            match gmail_browser_actions::handle_action(env, config, handshake, action, &input) {
                Ok(payload) => emit_control(&env.topic, "gmail_browser_action_complete", payload),
                Err(error) => emit_control(
                    &env.topic,
                    "gmail_browser_action_error",
                    json!({
                        "op": op,
                        "action": action,
                        "error": format!("{error:#}"),
                    }),
                ),
            }
        }
        SubscriberCommand::Custom { op, .. } => emit_control(
            &env.topic,
            "command_ignored",
            json!({ "op": op, "error": "unknown custom op" }),
        ),
        _ => emit_control(
            &env.topic,
            "command_ignored",
            json!({ "error": "gmail-browser subscriber only handles gmail_browser_act custom commands" }),
        ),
    }
}

fn load_config_from_dir(state_dir: &Path) -> Result<Option<GmailBrowserConfig>> {
    let path = state_dir.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut config: GmailBrowserConfig =
        toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    config.accounts = normalize_accounts(config.accounts);
    Ok(Some(config))
}

fn load_seen(state_dir: &Path) -> Result<SeenState> {
    let path = state_dir.join(SEEN_FILE);
    if !path.exists() {
        return Ok(SeenState::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

fn save_seen(state_dir: &Path, seen: &SeenState) -> Result<()> {
    fs::create_dir_all(state_dir).with_context(|| format!("create {}", state_dir.display()))?;
    let path = state_dir.join(SEEN_FILE);
    fs::write(&path, serde_json::to_vec_pretty(seen)?)
        .with_context(|| format!("write {}", path.display()))
}

fn load_auth_state(state_dir: &Path) -> Result<AuthState> {
    let path = state_dir.join(AUTH_STATE_FILE);
    if !path.exists() {
        return Ok(AuthState::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

fn save_auth_state(state_dir: &Path, auth_state: &AuthState) -> Result<()> {
    if auth_state.auth_required_accounts.is_empty() {
        return clear_auth_state(state_dir);
    }
    fs::create_dir_all(state_dir).with_context(|| format!("create {}", state_dir.display()))?;
    let path = state_dir.join(AUTH_STATE_FILE);
    fs::write(&path, serde_json::to_vec_pretty(auth_state)?)
        .with_context(|| format!("write {}", path.display()))
}

fn clear_auth_state(state_dir: &Path) -> Result<()> {
    let path = state_dir.join(AUTH_STATE_FILE);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn prune_auth_state(state_dir: &Path, accounts: &[String]) -> Result<()> {
    let mut auth_state = load_auth_state(state_dir)?;
    let initial_len = auth_state.auth_required_accounts.len();
    auth_state.retain_configured_accounts(accounts);
    if auth_state.auth_required_accounts.len() != initial_len {
        save_auth_state(state_dir, &auth_state)?;
    }
    Ok(())
}

fn mark_account_auth_required(state_dir: &Path, account: &str, result: &Value) -> Result<()> {
    let mut auth_state = load_auth_state(state_dir)?;
    auth_state.auth_required_accounts.insert(
        account.to_string(),
        AuthRequiredAccount {
            url: result
                .get("href")
                .and_then(Value::as_str)
                .map(str::to_string),
            title: result
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
            updated_at_ms: now_ms(),
        },
    );
    save_auth_state(state_dir, &auth_state)
}

fn clear_account_auth_required(state_dir: &Path, account: &str) -> Result<()> {
    let mut auth_state = load_auth_state(state_dir)?;
    if auth_state.auth_required_accounts.remove(account).is_some() {
        save_auth_state(state_dir, &auth_state)?;
    }
    Ok(())
}

async fn poll_once(
    env: &SubscriberEnv,
    config: &GmailBrowserConfig,
    seen: &mut SeenState,
    handshake: &mut Option<crate::daemon::Handshake>,
) -> Result<()> {
    let handshake_ref = ensure_browser_daemon(config, handshake)?;
    let seen_count_before = seen.seen.len();
    let initialized_before = seen.initialized;
    let mut newly_seen = BTreeSet::new();
    let mut successful_poll = false;
    let mut observed_rows = 0usize;
    let mut emitted_rows = 0usize;
    for account in &config.accounts {
        let result = poll_account(env, account, handshake_ref)?;
        let status = result.get("status").and_then(Value::as_str).unwrap_or("ok");
        match status {
            "ok" => {
                successful_poll = true;
                clear_account_auth_required(&env.state_dir, account)?;
            }
            "auth_required" => {
                mark_account_auth_required(&env.state_dir, account, &result)?;
                diag::account_auth_required(&env.topic, account, &result);
                emit_control(
                    &env.topic,
                    "auth_required",
                    json!({
                        "account": account,
                        "url": result.get("href").cloned().unwrap_or(Value::Null),
                        "title": result.get("title").cloned().unwrap_or(Value::Null),
                    }),
                )?;
                continue;
            }
            other => {
                diag::account_poll_error(&env.topic, account, other, &result);
                emit_control(
                    &env.topic,
                    "poll_error",
                    json!({
                        "account": account,
                        "status": other,
                        "url": result.get("href").cloned().unwrap_or(Value::Null),
                        "title": result.get("title").cloned().unwrap_or(Value::Null),
                        "bodyText": result.get("bodyText").cloned().unwrap_or(Value::Null),
                    }),
                )?;
                continue;
            }
        }
        let rows = result
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        observed_rows += rows.len();
        for row in rows {
            let Some(id) = row.get("id").and_then(Value::as_str) else {
                continue;
            };
            let key = format!("{account}:{id}");
            newly_seen.insert(key.clone());
            if should_emit_row(seen, &key, &row) {
                emitted_rows += 1;
                emit_message(env, account, &key, row)?;
            }
        }
    }
    if successful_poll {
        seen.seen.extend(newly_seen);
        seen.initialized = true;
    }
    diag::poll_complete(
        &env.topic,
        successful_poll,
        observed_rows,
        emitted_rows,
        initialized_before,
        seen.initialized,
        seen_count_before,
        seen.seen.len(),
    );
    Ok(())
}

fn ensure_browser_daemon<'a>(
    config: &GmailBrowserConfig,
    handshake: &'a mut Option<crate::daemon::Handshake>,
) -> Result<&'a crate::daemon::Handshake> {
    if handshake.is_none() {
        let workspace_root = config
            .workspace_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let paths = ConfigPaths::discover(workspace_root);
        diag::browser_daemon_connect(&paths);
        *handshake = Some(crate::daemon_browser::ensure_daemon(&paths)?);
    }
    Ok(handshake.as_ref().expect("handshake populated above"))
}

fn poll_account(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
) -> Result<Value> {
    poll_account_at_url(env, account, handshake, &gmail_inbox_url(account))
}

fn poll_account_at_url(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
    url: &str,
) -> Result<Value> {
    let root_session = format!("gmail-browser-{}", safe_session_part(&env.topic));
    let tab_id = safe_session_part(account);
    let started = Instant::now();
    crate::daemon_browser::send_daemon_request(
        handshake,
        "browser_agent",
        json!({
            "action": "open",
            "sessionId": root_session,
            "tabId": tab_id,
            "label": format!("Gmail {account}"),
            "url": url,
            "width": BROWSER_WIDTH,
            "height": BROWSER_HEIGHT,
            "activate": false,
            "background": true,
        }),
    )
    .context("open Gmail browser tab")?;
    let deadline = Instant::now() + GMAIL_LOAD_TIMEOUT;
    loop {
        let value = crate::daemon_browser::send_daemon_request(
            handshake,
            "browser_agent",
            json!({
                "action": "evaluate",
                "sessionId": root_session,
                "tabId": safe_session_part(account),
                "width": BROWSER_WIDTH,
                "height": BROWSER_HEIGHT,
                "background": true,
                "script": GMAIL_INBOX_SCRIPT,
            }),
        )
        .context("read Gmail browser tab")?;
        let result = value.get("value").cloned().unwrap_or(Value::Null);
        if gmail_poll_result_ready(&result) || Instant::now() >= deadline {
            diag::account_poll_result(
                &env.topic,
                account,
                &result,
                started.elapsed().as_millis(),
                Instant::now() >= deadline,
            );
            return Ok(result);
        }
        std::thread::sleep(GMAIL_EVALUATE_INTERVAL);
    }
}

fn emit_message(env: &SubscriberEnv, account: &str, dedup_key: &str, row: Value) -> Result<()> {
    let sender = row.get("sender").and_then(Value::as_str).unwrap_or("");
    let subject = row.get("subject").and_then(Value::as_str).unwrap_or("");
    let snippet = row.get("snippet").and_then(Value::as_str).unwrap_or("");
    let text = [sender, subject, snippet]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    diag::emit_message(
        &env.topic,
        account,
        dedup_key,
        &row,
        sender,
        subject,
        snippet,
        text.len(),
    );
    emit_event(Event {
        topic: env.topic.clone(),
        kind: "message".to_string(),
        control: false,
        dedup_key: Some(dedup_key.to_string()),
        text,
        payload: json!({
            "platform": "gmail-browser",
            "account": account,
            "receivedAtMs": now_ms(),
            "message": row,
        }),
    })
}

fn emit_control(topic: &str, kind: &str, payload: Value) -> Result<()> {
    emit_event(Event {
        topic: topic.to_string(),
        kind: kind.to_string(),
        control: true,
        dedup_key: None,
        text: String::new(),
        payload,
    })
}

fn emit_event(event: Event) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &event).context("encode subscriber event")?;
    stdout.write_all(b"\n").context("write subscriber event")?;
    stdout.flush().context("flush subscriber event")
}

fn normalize_accounts(accounts: Vec<String>) -> Vec<String> {
    let mut normalized = accounts
        .into_iter()
        .map(|account| account.trim().to_ascii_lowercase())
        .filter(|account| looks_like_email(account))
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.ends_with('.')
}

fn safe_session_part(value: &str) -> String {
    let mut output = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    while output.contains("--") {
        output = output.replace("--", "-");
    }
    output.trim_matches('-').to_string()
}

fn gmail_inbox_url(account: &str) -> String {
    let encoded = url::form_urlencoded::byte_serialize(account.as_bytes()).collect::<String>();
    format!("https://mail.google.com/mail/?authuser={encoded}#inbox")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn should_emit_row(seen: &SeenState, key: &str, row: &Value) -> bool {
    if seen.seen.contains(key) {
        return false;
    }
    if !seen.initialized || seen.seen.is_empty() {
        return row.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX)
            < INITIAL_ROW_EMIT_LIMIT;
    }
    true
}

fn gmail_poll_result_ready(result: &Value) -> bool {
    let status = result.get("status").and_then(Value::as_str).unwrap_or("ok");
    if status != "loading" && status != "ok" {
        return true;
    }
    if result
        .get("empty")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    result
        .get("rows")
        .and_then(Value::as_array)
        .is_some_and(|rows| !rows.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accounts_sorts_and_deduplicates() {
        assert_eq!(
            normalize_accounts(vec![
                "B@Example.COM".to_string(),
                "a@example.com".to_string(),
                "b@example.com".to_string(),
                "not-email".to_string(),
            ]),
            vec!["a@example.com".to_string(), "b@example.com".to_string()]
        );
    }

    #[test]
    fn gmail_url_uses_account_selector() {
        assert_eq!(
            gmail_inbox_url("me@example.com"),
            "https://mail.google.com/mail/?authuser=me%40example.com#inbox"
        );
    }

    #[test]
    fn connection_auth_ok_tracks_auth_required_accounts() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(&temp);
        save_config(
            &paths,
            &paths.workspace_root,
            "work",
            vec!["Me@Example.com".to_string()],
        )
        .unwrap();

        assert!(connection_auth_ok(&paths, "work").unwrap());

        let dir = state_dir(&paths, "work");
        mark_account_auth_required(
            &dir,
            "me@example.com",
            &json!({
                "href": "https://accounts.google.com/signin",
                "title": "Sign in - Google Accounts"
            }),
        )
        .unwrap();

        assert!(!connection_auth_ok(&paths, "work").unwrap());

        clear_account_auth_required(&dir, "me@example.com").unwrap();

        assert!(connection_auth_ok(&paths, "work").unwrap());
        assert!(!dir.join(AUTH_STATE_FILE).exists());
    }

    #[test]
    fn save_config_clears_prior_auth_required_state() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(&temp);
        save_config(
            &paths,
            &paths.workspace_root,
            "work",
            vec!["me@example.com".to_string()],
        )
        .unwrap();
        let dir = state_dir(&paths, "work");
        mark_account_auth_required(
            &dir,
            "me@example.com",
            &json!({
                "href": "https://accounts.google.com/signin",
                "title": "Sign in - Google Accounts"
            }),
        )
        .unwrap();

        assert!(!connection_auth_ok(&paths, "work").unwrap());

        save_config(
            &paths,
            &paths.workspace_root,
            "work",
            vec!["me@example.com".to_string()],
        )
        .unwrap();

        assert!(connection_auth_ok(&paths, "work").unwrap());
        assert!(!dir.join(AUTH_STATE_FILE).exists());
    }

    #[test]
    fn auth_state_prune_drops_unconfigured_accounts() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(&temp);
        save_config(
            &paths,
            &paths.workspace_root,
            "work",
            vec!["new@example.com".to_string()],
        )
        .unwrap();
        let dir = state_dir(&paths, "work");
        mark_account_auth_required(
            &dir,
            "old@example.com",
            &json!({
                "href": "https://accounts.google.com/signin",
                "title": "Sign in - Google Accounts"
            }),
        )
        .unwrap();

        assert!(connection_auth_ok(&paths, "work").unwrap());

        prune_auth_state(&dir, &["new@example.com".to_string()]).unwrap();

        assert!(!dir.join(AUTH_STATE_FILE).exists());
    }

    #[test]
    fn first_snapshot_only_emits_the_top_row() {
        let seen = SeenState::default();

        assert!(should_emit_row(
            &seen,
            "me@example.com:1",
            &json!({ "unread": false, "index": 0 })
        ));
        assert!(!should_emit_row(
            &seen,
            "me@example.com:2",
            &json!({ "unread": true, "index": 1 })
        ));
        assert!(!should_emit_row(
            &seen,
            "me@example.com:3",
            &json!({ "unread": false, "index": 2 })
        ));
    }

    #[test]
    fn initialized_seen_cache_emits_new_rows() {
        let mut seen = SeenState {
            initialized: true,
            seen: BTreeSet::new(),
        };
        seen.seen.insert("me@example.com:1".to_string());

        assert!(!should_emit_row(
            &seen,
            "me@example.com:1",
            &json!({ "unread": true, "index": 0 })
        ));
        assert!(should_emit_row(
            &seen,
            "me@example.com:2",
            &json!({ "unread": false, "index": 25 })
        ));
    }

    #[test]
    fn gmail_poll_result_waits_for_loading_rows() {
        assert!(!gmail_poll_result_ready(&json!({
            "status": "loading",
            "rows": []
        })));
        assert!(gmail_poll_result_ready(&json!({
            "status": "ok",
            "rows": [{ "id": "one" }]
        })));
        assert!(gmail_poll_result_ready(&json!({
            "status": "ok",
            "empty": true,
            "rows": []
        })));
        assert!(gmail_poll_result_ready(&json!({
            "status": "auth_required",
            "rows": []
        })));
    }

    #[test]
    fn gmail_inbox_script_emits_attachment_flag() {
        assert!(GMAIL_INBOX_SCRIPT.contains("hasAttachment"));
        assert!(GMAIL_INBOX_SCRIPT.contains("attachment"));
    }

    fn test_paths(temp: &tempfile::TempDir) -> ConfigPaths {
        let workspace_root = temp.path().join("workspace");
        ConfigPaths {
            workspace_config_dir: workspace_root.join(".puffer"),
            user_config_dir: temp.path().join("home").join(".puffer"),
            builtin_resources_dir: temp.path().join("resources"),
            workspace_root,
        }
    }
}
