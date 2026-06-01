//! Google Calendar web connector backed by daemon-managed Chrome sessions.

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::{Event, SubscriberCommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader, Lines};

#[path = "gcal_browser_actions.rs"]
mod gcal_browser_actions;

/// Connector and default connection slug used by the Google Calendar browser connector.
pub(crate) const CONNECTOR_SLUG: &str = "gcal-browser";
/// Default connection slug used when no per-connection topic is supplied.
pub(crate) const DEFAULT_CONNECTION: &str = "gcal-browser";
/// User-config subdirectory used for instantiated Google Calendar browser subscribers.
pub(crate) const STATE_ROOT: &str = "gcal-browser-accounts";

const CONFIG_FILE: &str = "config.toml";
const SEEN_FILE: &str = "seen.json";
const POLL_INTERVAL: Duration = Duration::from_secs(30);
const ERROR_BACKOFF: Duration = Duration::from_secs(10);
const GCAL_LOAD_TIMEOUT: Duration = Duration::from_secs(20);
const GCAL_EVALUATE_INTERVAL: Duration = Duration::from_secs(1);
const BROWSER_WIDTH: u32 = 1280;
const BROWSER_HEIGHT: u32 = 900;
const INITIAL_ROW_EMIT_LIMIT: u64 = 1;

/// Persisted Google Calendar browser connector configuration.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct GcalBrowserConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workspace_root: Option<PathBuf>,
    #[serde(default)]
    pub(crate) accounts: Vec<String>,
}

impl GcalBrowserConfig {
    /// Returns true when the connector has enough information to poll Calendar.
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
                Err(error) => eprintln!("gcal-browser: ignored malformed command: {error}"),
            }
        }
    }
}

/// Loads Google Calendar browser config for one connection, if present.
pub(crate) fn load_config(
    paths: &ConfigPaths,
    connection_slug: &str,
) -> Result<Option<GcalBrowserConfig>> {
    load_config_from_dir(&state_dir(paths, connection_slug))
}

/// Saves Google Calendar browser config for one connection.
pub(crate) fn save_config(
    paths: &ConfigPaths,
    workspace_root: &Path,
    connection_slug: &str,
    accounts: Vec<String>,
) -> Result<GcalBrowserConfig> {
    let accounts = normalize_accounts(accounts);
    anyhow::ensure!(
        !accounts.is_empty(),
        "gcal-browser requires at least one Google account"
    );
    let config = GcalBrowserConfig {
        workspace_root: Some(workspace_root.to_path_buf()),
        accounts,
    };
    let dir = state_dir(paths, connection_slug);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let raw = toml::to_string_pretty(&config).context("serialize Google Calendar config")?;
    let path = dir.join(CONFIG_FILE);
    fs::write(&path, raw).with_context(|| format!("write {}", path.display()))?;
    Ok(config)
}

/// Returns the per-connection state directory used by the subscriber runtime.
pub(crate) fn state_dir(paths: &ConfigPaths, connection_slug: &str) -> PathBuf {
    paths.user_config_dir.join(STATE_ROOT).join(connection_slug)
}

/// Runs the Google Calendar browser subscriber until the supervisor stops the process.
pub(crate) async fn run_subscriber() -> Result<()> {
    let env = SubscriberEnv::from_env();
    tokio::fs::create_dir_all(&env.state_dir)
        .await
        .with_context(|| format!("create {}", env.state_dir.display()))?;

    let mut seen = load_seen(&env.state_dir)?;
    let mut handshake = None;
    let mut last_config_key = String::new();
    let mut commands = CommandStream::new();
    loop {
        let Some(config) = load_config_from_dir(&env.state_dir)? else {
            emit_control(&env.topic, "config_required", json!({}))?;
            wait_or_handle_command(&env, None, &mut handshake, &mut commands, POLL_INTERVAL)
                .await?;
            continue;
        };
        if !config.is_configured() {
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
    config: Option<&GcalBrowserConfig>,
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
    config: Option<&GcalBrowserConfig>,
    handshake: &mut Option<crate::daemon::Handshake>,
    command: SubscriberCommand,
) -> Result<()> {
    match command {
        SubscriberCommand::Custom { op, args } if op == "gcal_browser_act" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let input = args.get("input").cloned().unwrap_or_else(|| json!({}));
            let Some(config) = config else {
                emit_control(
                    &env.topic,
                    "gcal_browser_action_error",
                    json!({
                        "op": op,
                        "action": action,
                        "error": "gcal-browser connector is not configured yet",
                    }),
                )?;
                return Ok(());
            };
            match gcal_browser_actions::handle_action(env, config, handshake, action, &input) {
                Ok(payload) => emit_control(&env.topic, "gcal_browser_action_complete", payload),
                Err(error) => emit_control(
                    &env.topic,
                    "gcal_browser_action_error",
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
            json!({ "error": "gcal-browser subscriber only handles gcal_browser_act custom commands" }),
        ),
    }
}

fn load_config_from_dir(state_dir: &Path) -> Result<Option<GcalBrowserConfig>> {
    let path = state_dir.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut config: GcalBrowserConfig =
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

async fn poll_once(
    env: &SubscriberEnv,
    config: &GcalBrowserConfig,
    seen: &mut SeenState,
    handshake: &mut Option<crate::daemon::Handshake>,
) -> Result<()> {
    let handshake_ref = ensure_browser_daemon(config, handshake)?;
    let mut newly_seen = BTreeSet::new();
    let mut successful_poll = false;
    for account in &config.accounts {
        let result = poll_account(env, account, handshake_ref)?;
        let status = result.get("status").and_then(Value::as_str).unwrap_or("ok");
        match status {
            "ok" => {
                successful_poll = true;
            }
            "auth_required" => {
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
        for row in result
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let Some(id) = row.get("id").and_then(Value::as_str) else {
                continue;
            };
            let key = format!("{account}:{id}");
            newly_seen.insert(key.clone());
            if should_emit_row(seen, &key, &row) {
                emit_calendar_event(env, account, &key, row)?;
            }
        }
    }
    if successful_poll {
        seen.seen.extend(newly_seen);
        seen.initialized = true;
    }
    Ok(())
}

fn ensure_browser_daemon<'a>(
    config: &GcalBrowserConfig,
    handshake: &'a mut Option<crate::daemon::Handshake>,
) -> Result<&'a crate::daemon::Handshake> {
    if handshake.is_none() {
        let workspace_root = config
            .workspace_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let paths = ConfigPaths::discover(workspace_root);
        *handshake = Some(crate::daemon_browser::ensure_daemon(&paths)?);
    }
    Ok(handshake.as_ref().expect("handshake populated above"))
}

fn poll_account(
    env: &SubscriberEnv,
    account: &str,
    handshake: &crate::daemon::Handshake,
) -> Result<Value> {
    let root_session = root_session_id(&env.topic);
    let tab_id = safe_session_part(account);
    crate::daemon_browser::send_daemon_request(
        handshake,
        "browser_agent",
        json!({
            "action": "open",
            "sessionId": root_session,
            "tabId": tab_id,
            "label": format!("Google Calendar {account}"),
            "url": gcal_agenda_url(account),
            "width": BROWSER_WIDTH,
            "height": BROWSER_HEIGHT,
            "activate": false,
        }),
    )
    .context("open Google Calendar browser tab")?;
    let deadline = Instant::now() + GCAL_LOAD_TIMEOUT;
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
                "script": GCAL_EVENTS_SCRIPT,
            }),
        )
        .context("read Google Calendar browser tab")?;
        let result = value.get("value").cloned().unwrap_or(Value::Null);
        if gcal_poll_result_ready(&result) || Instant::now() >= deadline {
            return Ok(result);
        }
        std::thread::sleep(GCAL_EVALUATE_INTERVAL);
    }
}

fn emit_calendar_event(
    env: &SubscriberEnv,
    account: &str,
    dedup_key: &str,
    row: Value,
) -> Result<()> {
    let title = row.get("title").and_then(Value::as_str).unwrap_or("");
    let when = row.get("when").and_then(Value::as_str).unwrap_or("");
    let location = row.get("location").and_then(Value::as_str).unwrap_or("");
    let text = [title, when, location]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    emit_event(Event {
        topic: env.topic.clone(),
        kind: "message".to_string(),
        control: false,
        dedup_key: Some(dedup_key.to_string()),
        text,
        payload: json!({
            "platform": "gcal-browser",
            "account": account,
            "receivedAtMs": now_ms(),
            "event": row,
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
    let trimmed = output.trim_matches('-');
    if trimmed.is_empty() {
        "gcal".to_string()
    } else {
        trimmed.to_string()
    }
}

fn root_session_id(topic: &str) -> String {
    format!("gcal-browser-{}", safe_session_part(topic))
}

fn gcal_agenda_url(account: &str) -> String {
    let encoded = url::form_urlencoded::byte_serialize(account.as_bytes()).collect::<String>();
    format!("https://calendar.google.com/calendar/u/0/r/agenda?authuser={encoded}")
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

fn gcal_poll_result_ready(result: &Value) -> bool {
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

const GCAL_EVENTS_SCRIPT: &str = r#"
(() => {
  const href = location.href;
  const title = document.title || "";
  const bodyText = document.body ? document.body.innerText || "" : "";
  const host = location.hostname || "";
  const signinLike =
    host.includes("accounts.google.com") ||
    /ServiceLogin|signin|identifier/.test(href) ||
    (/sign in/i.test(title) && !/calendar/i.test(title));
  if (signinLike) {
    return { status: "auth_required", href, title, rows: [] };
  }
  const visible = (node) => {
    if (!node) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const clean = (value) => String(value || "").trim().replace(/\s+/g, " ");
  const text = (node) => clean(node && node.textContent);
  const attr = (node, name) => node && node.getAttribute ? clean(node.getAttribute(name)) : "";
  const hash = (value) => {
    let h = 2166136261;
    for (let i = 0; i < value.length; i += 1) {
      h ^= value.charCodeAt(i);
      h = Math.imul(h, 16777619);
    }
    return `fallback-${(h >>> 0).toString(36)}`;
  };
  const blocked =
    /^(create|today|next|previous|settings|help|search|tasks|keep|calendar|main menu)$/i;
  const candidateNodes = Array.from(document.querySelectorAll([
    "[data-eventid]",
    "[data-event-id]",
    "[data-eid]",
    "a[href*='event']",
    "a[href*='eid=']",
    "[role='button'][aria-label]",
    "[role='link'][aria-label]"
  ].join(",")));
  const rows = [];
  const seen = new Set();
  for (const node of candidateNodes) {
    if (!visible(node)) continue;
    const label = clean([
      attr(node, "aria-label"),
      attr(node, "title"),
      text(node)
    ].filter(Boolean).join(" "));
    if (!label || label.length < 3 || blocked.test(label)) continue;
    if (!/\d|am|pm|event|meeting|invite|calendar|today|tomorrow|mon|tue|wed|thu|fri|sat|sun/i.test(label)) {
      continue;
    }
    const rawId =
      attr(node, "data-eventid") ||
      attr(node, "data-event-id") ||
      attr(node, "data-eid") ||
      attr(node, "data-id") ||
      attr(node, "href");
    const id = rawId || hash(label);
    if (seen.has(id)) continue;
    seen.add(id);
    const hrefValue = attr(node, "href");
    const lines = label.split(/\s*(?:,|\n)\s*/).filter(Boolean);
    rows.push({
      id,
      title: lines[0] || label,
      when: lines.slice(1, 4).join(", "),
      summary: label,
      url: hrefValue || href,
      index: rows.length
    });
    if (rows.length >= 50) break;
  }
  const empty = /no events|nothing planned|no meetings|no events found/i.test(bodyText);
  const status = rows.length > 0 || empty ? "ok" : "loading";
  return { status, href, title, bodyText: bodyText.slice(0, 300), empty, rows };
})()
"#;

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
    fn calendar_url_uses_account_selector() {
        assert_eq!(
            gcal_agenda_url("me@example.com"),
            "https://calendar.google.com/calendar/u/0/r/agenda?authuser=me%40example.com"
        );
    }

    #[test]
    fn first_snapshot_only_emits_the_top_row() {
        let seen = SeenState::default();

        assert!(should_emit_row(
            &seen,
            "me@example.com:1",
            &json!({ "index": 0 })
        ));
        assert!(!should_emit_row(
            &seen,
            "me@example.com:2",
            &json!({ "index": 1 })
        ));
    }

    #[test]
    fn calendar_poll_result_waits_for_loading_rows() {
        assert!(!gcal_poll_result_ready(&json!({
            "status": "loading",
            "rows": []
        })));
        assert!(gcal_poll_result_ready(&json!({
            "status": "ok",
            "rows": [{ "id": "one" }]
        })));
        assert!(gcal_poll_result_ready(&json!({
            "status": "auth_required",
            "rows": []
        })));
    }
}
