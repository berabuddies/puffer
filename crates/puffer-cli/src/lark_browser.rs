//! Lark/Feishu web connector backed by daemon-managed CEF sessions.

#[path = "lark_browser_actions.rs"]
mod lark_browser_actions;

use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::{Event, SubscriberCommand};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader, Lines};

use crate::lark_browser_script::{
    feed_loaded, parse_active_drain, parse_feed_rows, FeedRow, LARK_FEED_SCRIPT,
    LARK_OBSERVER_DRAIN_JS, LARK_OBSERVER_INSTALL_JS,
};

pub(crate) const CONNECTOR_SLUG_LARK: &str = "lark-browser";
pub(crate) const CONNECTOR_SLUG_FEISHU: &str = "feishu-browser";

const CONFIG_FILE: &str = "config.toml";
const SEEN_FILE: &str = "seen.json";
const POLL_INTERVAL: Duration = Duration::from_secs(30);
const ERROR_BACKOFF: Duration = Duration::from_secs(10);
const BROWSER_WIDTH: u32 = 1280;
const BROWSER_HEIGHT: u32 = 900;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Brand {
    Lark,
    Feishu,
}

impl Brand {
    pub(crate) fn from_slug(slug: &str) -> Option<Brand> {
        match slug {
            CONNECTOR_SLUG_LARK => Some(Brand::Lark),
            CONNECTOR_SLUG_FEISHU => Some(Brand::Feishu),
            _ => None,
        }
    }
    pub(crate) fn slug(&self) -> &'static str {
        match self {
            Brand::Lark => CONNECTOR_SLUG_LARK,
            Brand::Feishu => CONNECTOR_SLUG_FEISHU,
        }
    }
    pub(crate) fn platform(&self) -> &'static str {
        self.slug()
    }
    /// User-config subdirectory root for this brand's per-connection subscriber
    /// state. MUST match the catalog template's `subscriber.state_root` so the
    /// runtime's `PUFFER_SKILL_STATE_DIR` resolves to the same directory that
    /// `save_config` writes into.
    pub(crate) fn state_root(&self) -> &'static str {
        match self {
            Brand::Lark => "lark-browser-accounts",
            Brand::Feishu => "feishu-browser-accounts",
        }
    }
    /// The MESSENGER entry URL. The connector opens this every poll; it must land
    /// on the message feed, not the default app. Opening the web root lands on
    /// Drive (per-tenant), so the connector would re-open Drive each poll and
    /// fight the feed-script's navigation. The `/messenger/` entry redirects
    /// (post-login) straight to `<tenant>/next/messenger/` and stays there.
    /// Pre-login it redirects to the QR login, so it also works for setup.
    pub(crate) fn web_url(&self) -> &'static str {
        match self {
            Brand::Lark => "https://web.larksuite.com/messenger/",
            Brand::Feishu => "https://web.feishu.cn/messenger/",
        }
    }
}

/// Parses the `brand` field of the config into a `Brand`.
/// Accepts slug forms ("lark-browser", "feishu-browser") and short forms
/// ("lark", "feishu") to be tolerant of how the field might be written.
fn brand_from_config_str(s: &str) -> Option<Brand> {
    match s.trim().to_ascii_lowercase().as_str() {
        "lark-browser" | "lark" => Some(Brand::Lark),
        "feishu-browser" | "feishu" => Some(Brand::Feishu),
        _ => None,
    }
}

/// Persisted Lark/Feishu browser connector configuration.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct LarkBrowserConfig {
    /// The user workspace the connection was created in. The subscriber MUST
    /// connect to the browser daemon for THIS workspace (not the subscriber's
    /// cwd, which is the manifest dir) — otherwise it discovers the wrong
    /// workspace, fails to find the running daemon, and tries (and fails) to
    /// start its own. Mirrors `GmailBrowserConfig::workspace_root`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workspace_root: Option<PathBuf>,
    #[serde(default)]
    pub(crate) brand: String,
    #[serde(default)]
    pub(crate) connection: String,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct SeenState {
    #[serde(default)]
    pub(crate) initialized: bool,
    #[serde(default)]
    pub(crate) seen: BTreeSet<String>,
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
            .unwrap_or_else(|| CONNECTOR_SLUG_LARK.to_string());
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
                Err(error) => {
                    eprintln!("lark-browser: ignored malformed command: {error}")
                }
            }
        }
    }
}

/// Returns the per-connection state directory for a lark/feishu browser subscriber.
///
/// MUST match the path the subscriber runtime computes from the catalog
/// template's `subscriber.state_root` via `instantiated_state_dir`:
/// `user_config_dir/<brand-state-root>/<connection_slug>` (mirrors
/// `gmail_browser::state_dir`, but the root depends on brand).
pub(crate) fn state_dir(
    paths: &puffer_config::ConfigPaths,
    brand: Brand,
    connection_slug: &str,
) -> PathBuf {
    paths
        .user_config_dir
        .join(brand.state_root())
        .join(connection_slug)
}

/// Writes `LarkBrowserConfig` as `config.toml` into the per-connection state dir
/// so the subscriber's `load_config_from_dir` can find it on next poll.
pub(crate) fn save_config(
    paths: &puffer_config::ConfigPaths,
    workspace_root: &Path,
    brand: Brand,
    connection_slug: &str,
) -> Result<LarkBrowserConfig> {
    let config = LarkBrowserConfig {
        workspace_root: Some(workspace_root.to_path_buf()),
        brand: brand.slug().to_string(),
        connection: connection_slug.to_string(),
    };
    let dir = state_dir(paths, brand, connection_slug);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let raw = toml::to_string_pretty(&config).context("serialize Lark browser config")?;
    let path = dir.join(CONFIG_FILE);
    fs::write(&path, raw).with_context(|| format!("write {}", path.display()))?;
    Ok(config)
}

fn load_config_from_dir(state_dir: &Path) -> Result<Option<LarkBrowserConfig>> {
    let path = state_dir.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let config: LarkBrowserConfig =
        toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
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

fn feed_fingerprint(preview: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    preview.trim().hash(&mut h);
    format!("{:x}", h.finish())
}

fn feed_dedup_key(conn: &str, row: &FeedRow) -> String {
    format!("{}:{}:{}", conn, row.chat_id, feed_fingerprint(&row.preview))
}

fn should_emit_feed(seen: &SeenState, key: &str) -> bool {
    if seen.seen.contains(key) {
        return false;
    }
    seen.initialized // pre-init: seeds only, emits nothing
}

fn build_message_event(
    platform: &str,
    brand: &str,
    chat_id: &str,
    sender: &str,
    text: &str,
    is_outgoing: bool,
    source: &str,
    dedup_key: &str,
) -> Event {
    Event {
        topic: platform.to_string(),
        kind: "message".to_string(),
        control: false,
        dedup_key: Some(dedup_key.to_string()),
        text: format!("{sender}\n{text}").trim().to_string(),
        payload: json!({
            "platform": platform,
            "brand": brand,
            "chat_id": chat_id,
            "sender": sender,
            "is_outgoing": is_outgoing,
            "source": source,
            "receivedAtMs": now_ms(),
        }),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn emit_event(event: Event) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, &event).context("encode subscriber event")?;
    stdout.write_all(b"\n").context("write subscriber event")?;
    stdout.flush().context("flush subscriber event")
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

fn ensure_browser_daemon<'a>(
    config: &LarkBrowserConfig,
    handshake: &'a mut Option<crate::daemon::Handshake>,
) -> Result<&'a crate::daemon::Handshake> {
    if handshake.is_none() {
        // Connect to the browser daemon for the connection's WORKSPACE, not the
        // subscriber's cwd (the manifest dir) — otherwise we discover the wrong
        // workspace and fail to find the already-running daemon. Mirrors gmail.
        let workspace_root = config
            .workspace_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let paths = ConfigPaths::discover(workspace_root);
        eprintln!(
            "lark-browser: browser_daemon_connect workspace_root={} user_config_dir={}",
            paths.workspace_root.display(),
            paths.user_config_dir.display()
        );
        *handshake = Some(crate::daemon_browser::ensure_daemon(&paths)?);
    }
    Ok(handshake.as_ref().expect("handshake populated above"))
}

async fn wait_or_handle_command(
    env: &SubscriberEnv,
    config: Option<&LarkBrowserConfig>,
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
    config: Option<&LarkBrowserConfig>,
    handshake: &mut Option<crate::daemon::Handshake>,
    command: SubscriberCommand,
) -> Result<()> {
    match command {
        SubscriberCommand::Custom { op, args } if op == "lark_browser_act" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let input = args.get("input").cloned().unwrap_or_else(|| json!({}));
            let Some(config) = config else {
                emit_control(
                    &env.topic,
                    "lark_browser_action_error",
                    json!({
                        "op": op,
                        "action": action,
                        "error": "lark-browser connector is not configured yet",
                    }),
                )?;
                return Ok(());
            };
            match lark_browser_actions::handle_action(env, config, handshake, action, &input) {
                Ok(payload) => {
                    emit_control(&env.topic, "lark_browser_action_complete", payload)
                }
                Err(error) => emit_control(
                    &env.topic,
                    "lark_browser_action_error",
                    json!({
                        "op": op,
                        "action": action,
                        "error": format!("{error:#}"),
                    }),
                ),
            }
        }
        SubscriberCommand::Custom { op, .. } => {
            emit_control(
                &env.topic,
                "command_ignored",
                json!({ "op": op, "error": "unknown custom op" }),
            )
        }
        _ => {
            emit_control(
                &env.topic,
                "command_ignored",
                json!({ "error": "lark-browser subscriber only handles lark_browser_act custom commands" }),
            )
        }
    }
}

/// Pure, testable core of a single feed poll. Computes which events to emit and
/// mutates `seen` — but ONLY when the page reports `loaded: true`. When the
/// messenger shell is absent (unloaded page or logged-out), returns an empty
/// vec and leaves `seen` untouched so the next poll retries with a clean slate.
///
/// `active_chat_id`: if non-empty, feed rows for that chat_id are suppressed
/// (the active pass covers the open chat with higher fidelity).
pub(crate) fn process_feed_poll(
    parsed: &Value,
    conn: &str,
    platform: &str,
    brand_label: &str,
    seen: &mut SeenState,
    active_chat_id: &str,
) -> Vec<Event> {
    if !feed_loaded(parsed) {
        return Vec::new();
    }

    let rows = parse_feed_rows(parsed);
    let mut newly_seen = BTreeSet::new();
    let mut events = Vec::new();

    for row in &rows {
        let key = feed_dedup_key(conn, row);
        newly_seen.insert(key.clone());
        // Suppress feed rows for the open chat — the active layer covers it
        // with higher fidelity (full text, snowflake id, outgoing direction).
        if !active_chat_id.is_empty() && row.chat_id == active_chat_id {
            continue;
        }
        if should_emit_feed(seen, &key) {
            events.push(build_message_event(
                platform,
                brand_label,
                &row.chat_id,
                &row.name,
                &row.preview,
                row.is_outgoing,
                "feed",
                &key,
            ));
        }
    }

    // Only after a loaded poll: seed seen and mark initialized.
    seen.seen.extend(newly_seen);
    seen.initialized = true;

    events
}

/// Pure, testable core of a single active-conversation drain poll.
/// Evaluates `LARK_OBSERVER_DRAIN_JS` output (already parsed). For each
/// `ActiveMsg` with a snowflake id not yet in `seen`, emits an event with
/// `source:"active"`. Seeds (without emitting) on the very first loaded poll,
/// matching the feed-poll behaviour.
///
/// `parsed`: the parsed JSON returned from `LARK_OBSERVER_DRAIN_JS`.
/// Returns `(active_chat_id, events)` so the caller can pass `active_chat_id`
/// to `process_feed_poll` for suppression.
pub(crate) fn process_active_drain(
    parsed: &Value,
    conn: &str,
    platform: &str,
    brand_label: &str,
    seen: &mut SeenState,
) -> (String, Vec<Event>) {
    let (active_chat_id, msgs) = parse_active_drain(parsed);

    // Nothing open — return empty and don't mutate seen.
    if active_chat_id.is_empty() {
        return (String::new(), Vec::new());
    }

    let mut newly_seen = BTreeSet::new();
    let mut events = Vec::new();

    for msg in &msgs {
        let key = format!("{}:{}:{}", conn, active_chat_id, msg.id);
        newly_seen.insert(key.clone());
        if should_emit_feed(seen, &key) {
            events.push(build_message_event(
                platform,
                brand_label,
                &active_chat_id,
                "", // sender unknown at message level — text carries content
                &msg.text,
                msg.is_outgoing,
                "active",
                &key,
            ));
        }
    }

    // Seed seen regardless of init state (same gate as feed: only after a
    // loaded/non-empty drain do we update).
    if !newly_seen.is_empty() || seen.initialized {
        seen.seen.extend(newly_seen);
        // Mark initialized once we get a non-empty active drain on a loaded poll.
        // (The feed pass also sets this flag; whichever runs first wins.)
        seen.initialized = true;
    }

    (active_chat_id, events)
}

fn poll_once_feed(
    env: &SubscriberEnv,
    config: &LarkBrowserConfig,
    brand: Brand,
    seen: &mut SeenState,
    handshake: &mut Option<crate::daemon::Handshake>,
) -> Result<()> {
    let handshake_ref = ensure_browser_daemon(config, handshake)?;

    let session_id = format!("lark-browser-{}", safe_session_part(&env.topic));

    // Open (or reuse) the messenger tab for this brand.
    crate::daemon_browser::send_daemon_request(
        handshake_ref,
        "browser_agent",
        json!({
            "action": "open",
            "sessionId": session_id,
            "tabId": "messenger",
            "label": format!("{} messenger", config.brand),
            "url": brand.web_url(),
            "width": BROWSER_WIDTH,
            "height": BROWSER_HEIGHT,
            "activate": false,
            "background": true,
        }),
    )
    .context("open Lark browser tab")?;

    // Evaluate the feed script and parse results.
    let value = crate::daemon_browser::send_daemon_request(
        handshake_ref,
        "browser_agent",
        json!({
            "action": "evaluate",
            "sessionId": session_id,
            "tabId": "messenger",
            "width": BROWSER_WIDTH,
            "height": BROWSER_HEIGHT,
            "background": true,
            "script": LARK_FEED_SCRIPT,
        }),
    )
    .context("evaluate Lark feed script")?;

    // The evaluate response wraps the script's return value under "value".
    // The script returns a JSON string; parse it.
    let raw_value = value.get("value").cloned().unwrap_or(Value::Null);
    let parsed: Value = if let Some(s) = raw_value.as_str() {
        serde_json::from_str(s).unwrap_or(Value::Null)
    } else {
        raw_value
    };

    // ── Active pass (before feed so we know active_chat_id for suppression) ──
    // This pass is BEST-EFFORT: transient evaluate errors (page still loading,
    // tab not yet ready) must NOT abort the poll or block the feed pass below.
    // On any error or unparseable result we log a warning, default active_chat_id
    // to "" and active_events to [], and continue to the feed pass normally.

    // (1) Install observer (idempotent — re-installs after navigation).
    if let Err(e) = crate::daemon_browser::send_daemon_request(
        handshake_ref,
        "browser_agent",
        json!({
            "action": "evaluate",
            "sessionId": session_id,
            "tabId": "messenger",
            "width": BROWSER_WIDTH,
            "height": BROWSER_HEIGHT,
            "background": true,
            "script": LARK_OBSERVER_INSTALL_JS,
        }),
    ) {
        eprintln!("lark-browser: active_pass_install_warn topic={} error={e:#} (continuing to feed pass)", env.topic);
    }

    // (2) Drain captured messages.
    let drain_result = crate::daemon_browser::send_daemon_request(
        handshake_ref,
        "browser_agent",
        json!({
            "action": "evaluate",
            "sessionId": session_id,
            "tabId": "messenger",
            "width": BROWSER_WIDTH,
            "height": BROWSER_HEIGHT,
            "background": true,
            "script": LARK_OBSERVER_DRAIN_JS,
        }),
    );

    let (active_chat_id, active_events) = match drain_result {
        Err(e) => {
            eprintln!("lark-browser: active_pass_drain_warn topic={} error={e:#} (continuing to feed pass)", env.topic);
            (String::new(), Vec::new())
        }
        Ok(drain_value) => {
            let drain_raw = drain_value.get("value").cloned().unwrap_or(Value::Null);
            let drain_parsed: Value = if let Some(s) = drain_raw.as_str() {
                serde_json::from_str(s).unwrap_or(Value::Null)
            } else {
                drain_raw
            };
            process_active_drain(
                &drain_parsed,
                &config.connection,
                brand.platform(),
                &config.brand,
                seen,
            )
        }
    };

    let active_emitted = active_events.len();
    for event in active_events {
        emit_event(event)?;
    }

    // ── Feed pass ────────────────────────────────────────────────────────────
    let loaded = feed_loaded(&parsed);
    let seen_count_before = seen.seen.len();
    let initialized_before = seen.initialized;

    let events = process_feed_poll(
        &parsed,
        &config.connection,
        brand.platform(),
        &config.brand,
        seen,
        &active_chat_id,
    );
    let emitted = events.len();
    for event in events {
        emit_event(event)?;
    }

    eprintln!(
        "lark-browser: poll_complete topic={} loaded={loaded} active_chat_id={active_chat_id:?} observed_rows={} emitted_feed={emitted} emitted_active={active_emitted} initialized_before={initialized_before} initialized_after={} seen_count_before={seen_count_before} seen_count_after={}",
        env.topic,
        if loaded { parse_feed_rows(&parsed).len() } else { 0 },
        seen.initialized,
        seen.seen.len()
    );

    Ok(())
}

pub(crate) async fn run_subscriber() -> anyhow::Result<()> {
    let env = SubscriberEnv::from_env();
    tokio::fs::create_dir_all(&env.state_dir)
        .await
        .with_context(|| format!("create {}", env.state_dir.display()))?;

    let mut seen = load_seen(&env.state_dir)?;
    eprintln!(
        "lark-browser: subscriber_start topic={} state_dir={} seen_initialized={} seen_count={}",
        env.topic,
        env.state_dir.display(),
        seen.initialized,
        seen.seen.len()
    );

    let mut handshake = None;
    let mut commands = CommandStream::new();

    loop {
        let Some(config) = load_config_from_dir(&env.state_dir)? else {
            eprintln!(
                "lark-browser: config_required topic={} state_dir={} reason=missing",
                env.topic,
                env.state_dir.display()
            );
            emit_control(&env.topic, "config_required", json!({}))?;
            wait_or_handle_command(&env, None, &mut handshake, &mut commands, POLL_INTERVAL)
                .await?;
            continue;
        };

        let Some(brand) = brand_from_config_str(&config.brand) else {
            eprintln!(
                "lark-browser: config_required topic={} state_dir={} reason=unknown_brand brand={}",
                env.topic,
                env.state_dir.display(),
                config.brand
            );
            emit_control(
                &env.topic,
                "config_required",
                json!({ "reason": "unknown_brand", "brand": config.brand }),
            )?;
            wait_or_handle_command(
                &env,
                Some(&config),
                &mut handshake,
                &mut commands,
                POLL_INTERVAL,
            )
            .await?;
            continue;
        };

        let result = poll_once_feed(&env, &config, brand, &mut seen, &mut handshake);
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
                eprintln!("lark-browser: poll_loop_error topic={} error={error:#}", env.topic);
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

#[cfg(test)]
mod emit_tests {
    use super::*;
    use crate::lark_browser_script::FeedRow;

    fn row(chat: &str, preview: &str, out: bool) -> FeedRow {
        FeedRow {
            chat_id: chat.into(),
            name: "N".into(),
            preview: preview.into(),
            unread: true,
            is_outgoing: out,
        }
    }

    #[test]
    fn first_poll_seeds_without_emitting() {
        let mut seen = SeenState::default();
        let key = feed_dedup_key("c1", &row("123", "hi", false));
        assert!(!should_emit_feed(&seen, &key)); // pre-init: do not emit
        seen.seen.insert(key.clone());
        seen.initialized = true;
        let key2 = feed_dedup_key("c1", &row("123", "new msg", false));
        assert!(should_emit_feed(&seen, &key2)); // post-init: emit new
        assert!(!should_emit_feed(&seen, &key)); // already seen: skip
    }

    #[test]
    fn event_payload_has_monitor_keys() {
        let ev = build_message_event(
            "lark-browser",
            "lark",
            "123",
            "Alice",
            "hi",
            true,
            "feed",
            "c1:123:abc",
        );
        assert_eq!(ev.payload["chat_id"], "123");
        assert_eq!(ev.payload["is_outgoing"], true);
        assert_eq!(ev.payload["platform"], "lark-browser");
        assert_eq!(ev.payload["brand"], "lark");
        assert_eq!(ev.kind, "message");
        assert_eq!(ev.dedup_key.as_deref(), Some("c1:123:abc"));
    }

    // --- process_feed_poll loaded-gate tests ---

    #[test]
    fn process_feed_poll_not_loaded_leaves_seen_uninitialized() {
        // A poll result where `loaded` is false (page not yet ready).
        let parsed = serde_json::json!({
            "loaded": false,
            "rows": [{"chat_id": "42", "name": "Alice", "preview": "hi", "unread": true, "outgoing": false}]
        });
        let mut seen = SeenState::default();
        let events = process_feed_poll(&parsed, "conn1", "lark-browser", "lark", &mut seen, "");
        // No events emitted, and seen.initialized stays false.
        assert!(events.is_empty(), "expected no events when not loaded");
        assert!(!seen.initialized, "initialized must stay false when not loaded");
        assert!(seen.seen.is_empty(), "seen set must stay empty when not loaded");
    }

    #[test]
    fn process_feed_poll_no_loaded_key_leaves_seen_uninitialized() {
        // Missing `loaded` key (e.g. old script version or parse failure).
        let parsed = serde_json::json!({
            "rows": [{"chat_id": "42", "name": "Alice", "preview": "hi", "unread": true, "outgoing": false}]
        });
        let mut seen = SeenState::default();
        let events = process_feed_poll(&parsed, "conn1", "lark-browser", "lark", &mut seen, "");
        assert!(events.is_empty(), "expected no events when loaded key missing");
        assert!(!seen.initialized, "initialized must stay false when loaded key missing");
    }

    #[test]
    fn process_feed_poll_loaded_seeds_and_marks_initialized() {
        // A loaded poll: rows present → should seed seen, mark initialized, but NOT emit
        // (because initialized was false → should_emit_feed returns false for all).
        let parsed = serde_json::json!({
            "loaded": true,
            "rows": [{"chat_id": "42", "name": "Alice", "preview": "hi", "unread": true, "outgoing": false}]
        });
        let mut seen = SeenState::default();
        let events = process_feed_poll(&parsed, "conn1", "lark-browser", "lark", &mut seen, "");
        // First loaded poll: seeds but emits nothing (pre-init).
        assert!(events.is_empty(), "first loaded poll should seed only, not emit");
        assert!(seen.initialized, "initialized must be true after a loaded poll");
        assert_eq!(seen.seen.len(), 1, "seen must contain the seeded key");
    }

    #[test]
    fn process_feed_poll_loaded_emits_new_after_init() {
        // Second loaded poll with a NEW row that wasn't in the baseline.
        let parsed = serde_json::json!({
            "loaded": true,
            "rows": [{"chat_id": "99", "name": "Bob", "preview": "new msg", "unread": true, "outgoing": false}]
        });
        let mut seen = SeenState {
            initialized: true,
            seen: std::collections::BTreeSet::new(),
        };
        let events = process_feed_poll(&parsed, "conn1", "lark-browser", "lark", &mut seen, "");
        assert_eq!(events.len(), 1, "post-init poll with new row should emit one event");
        assert_eq!(events[0].payload["chat_id"], "99");
    }

    // --- feed suppression: active chat_id skips the matching feed row ---

    #[test]
    fn process_feed_poll_suppresses_active_chat_row() {
        // Feed has two rows. One chat_id matches the active (open) chat.
        // The active-chat row should NOT be emitted even post-init.
        let parsed = serde_json::json!({
            "loaded": true,
            "rows": [
                {"chat_id": "ACTIVE_CHAT", "name": "Alice", "preview": "active msg", "unread": true, "outgoing": false},
                {"chat_id": "OTHER_CHAT",  "name": "Bob",   "preview": "other msg",  "unread": true, "outgoing": false}
            ]
        });
        let mut seen = SeenState {
            initialized: true,
            seen: std::collections::BTreeSet::new(),
        };
        let events = process_feed_poll(
            &parsed,
            "conn1",
            "lark-browser",
            "lark",
            &mut seen,
            "ACTIVE_CHAT",
        );
        // Only the OTHER_CHAT row should be emitted; ACTIVE_CHAT is suppressed.
        assert_eq!(events.len(), 1, "only non-active feed row should emit");
        assert_eq!(events[0].payload["chat_id"], "OTHER_CHAT");
    }

    #[test]
    fn process_feed_poll_no_suppression_when_active_chat_empty() {
        // When active_chat_id is empty (no chat open), all feed rows emit normally.
        let parsed = serde_json::json!({
            "loaded": true,
            "rows": [
                {"chat_id": "CHAT_A", "name": "A", "preview": "msg a", "unread": true, "outgoing": false},
                {"chat_id": "CHAT_B", "name": "B", "preview": "msg b", "unread": true, "outgoing": false}
            ]
        });
        let mut seen = SeenState {
            initialized: true,
            seen: std::collections::BTreeSet::new(),
        };
        let events = process_feed_poll(&parsed, "conn1", "lark-browser", "lark", &mut seen, "");
        assert_eq!(events.len(), 2, "all feed rows should emit when active_chat_id is empty");
    }

    // --- process_active_drain pure helper tests ---

    #[test]
    fn process_active_drain_empty_drain_no_events() {
        // Empty items list → no events, no init flip.
        let parsed = serde_json::json!({"chat_id": "CHAT1", "items": []});
        let mut seen = SeenState::default();
        let (chat_id, events) = process_active_drain(
            &parsed, "conn1", "lark-browser", "lark", &mut seen,
        );
        assert_eq!(chat_id, "CHAT1");
        assert!(events.is_empty(), "no events for empty drain");
        // initialized stays false because no msgs to seed
        assert!(!seen.initialized, "initialized should not flip on empty drain");
    }

    #[test]
    fn process_active_drain_no_chat_open_no_events() {
        // chat_id is empty → no active chat; nothing happens.
        let parsed = serde_json::json!({"chat_id": "", "items": [
            {"id": "7652607780750119026", "dir": "out", "text": "hi"}
        ]});
        let mut seen = SeenState::default();
        let (chat_id, events) = process_active_drain(
            &parsed, "conn1", "lark-browser", "lark", &mut seen,
        );
        assert!(chat_id.is_empty());
        assert!(events.is_empty());
        assert!(!seen.initialized);
    }

    #[test]
    fn process_active_drain_seeds_on_first_poll_no_emit() {
        // First poll with a snowflake id: seeds seen but emits nothing (pre-init).
        let parsed = serde_json::json!({
            "chat_id": "CHAT1",
            "items": [{"id": "7652607780750119026", "dir": "in", "text": "hello"}]
        });
        let mut seen = SeenState::default();
        let (chat_id, events) = process_active_drain(
            &parsed, "conn1", "lark-browser", "lark", &mut seen,
        );
        assert_eq!(chat_id, "CHAT1");
        assert!(events.is_empty(), "first poll must seed only, not emit");
        assert!(seen.initialized, "initialized must flip after first drain with msgs");
        assert!(
            seen.seen.contains("conn1:CHAT1:7652607780750119026"),
            "key must be seeded"
        );
    }

    #[test]
    fn process_active_drain_emits_on_second_poll() {
        // Second poll: seen already initialized, new snowflake arrives → emits.
        let parsed = serde_json::json!({
            "chat_id": "CHAT1",
            "items": [{"id": "7652607883305029745", "dir": "out", "text": "reply"}]
        });
        let mut seen = SeenState {
            initialized: true,
            seen: std::collections::BTreeSet::new(),
        };
        let (chat_id, events) = process_active_drain(
            &parsed, "conn1", "lark-browser", "lark", &mut seen,
        );
        assert_eq!(chat_id, "CHAT1");
        assert_eq!(events.len(), 1, "new snowflake post-init must emit");
        assert_eq!(events[0].payload["chat_id"], "CHAT1");
        assert_eq!(events[0].payload["is_outgoing"], true);
        assert_eq!(events[0].payload["source"], "active");
    }

    #[test]
    fn process_active_drain_drops_optimistic_temp_ids() {
        // Optimistic ids (non-snowflake) are silently dropped.
        let parsed = serde_json::json!({
            "chat_id": "CHAT1",
            "items": [
                {"id": "gApEI0EY3S", "dir": "out", "text": "sending"},
                {"id": "7652607780750119026", "dir": "out", "text": "sent"}
            ]
        });
        let mut seen = SeenState {
            initialized: true,
            seen: std::collections::BTreeSet::new(),
        };
        let (_chat_id, events) = process_active_drain(
            &parsed, "conn1", "lark-browser", "lark", &mut seen,
        );
        assert_eq!(events.len(), 1, "only snowflake id should emit");
        assert_eq!(events[0].payload["source"], "active");
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use puffer_config::ConfigPaths;

    fn test_paths(tmp: &std::path::Path) -> ConfigPaths {
        ConfigPaths {
            workspace_root: tmp.join("workspace"),
            workspace_config_dir: tmp.join("workspace").join(".puffer"),
            user_config_dir: tmp.join("home").join(".puffer"),
            builtin_resources_dir: tmp.join("resources"),
        }
    }

    #[test]
    fn save_and_load_config_round_trip_lark() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        let ws = std::path::Path::new("/workspace/ws");
        save_config(&paths, ws, Brand::Lark, "myconn").unwrap();

        let dir = state_dir(&paths, Brand::Lark, "myconn");
        // The state dir MUST live under user_config_dir/lark-browser-accounts/<conn>,
        // exactly what the runtime's instantiated_state_dir computes from the
        // catalog state_root.
        assert!(
            dir.ends_with("lark-browser-accounts/myconn"),
            "lark state dir must end with lark-browser-accounts/myconn, got {}",
            dir.display()
        );
        assert!(
            dir.starts_with(&paths.user_config_dir),
            "lark state dir must live under user_config_dir, got {}",
            dir.display()
        );
        let loaded = load_config_from_dir(&dir).unwrap().expect("config must exist");
        assert_eq!(loaded.brand, "lark-browser");
        assert_eq!(loaded.connection, "myconn");
        // workspace_root must round-trip so the subscriber connects to the right
        // browser daemon (not the manifest-dir cwd).
        assert_eq!(loaded.workspace_root.as_deref(), Some(ws));
        assert_eq!(
            brand_from_config_str(&loaded.brand),
            Some(Brand::Lark),
            "brand_from_config_str must parse back to Brand::Lark"
        );
    }

    #[test]
    fn save_and_load_config_round_trip_feishu() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        save_config(&paths, std::path::Path::new("/workspace/ws"), Brand::Feishu, "myconn").unwrap();

        let dir = state_dir(&paths, Brand::Feishu, "myconn");
        assert!(
            dir.ends_with("feishu-browser-accounts/myconn"),
            "feishu state dir must end with feishu-browser-accounts/myconn, got {}",
            dir.display()
        );
        assert!(
            dir.starts_with(&paths.user_config_dir),
            "feishu state dir must live under user_config_dir, got {}",
            dir.display()
        );
        let loaded = load_config_from_dir(&dir).unwrap().expect("config must exist");
        assert_eq!(loaded.brand, "feishu-browser");
        assert_eq!(loaded.connection, "myconn");
        assert_eq!(
            brand_from_config_str(&loaded.brand),
            Some(Brand::Feishu),
            "brand_from_config_str must parse back to Brand::Feishu"
        );
    }

    #[test]
    fn load_config_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        let dir = state_dir(&paths, Brand::Lark, "myconn");
        // dir may not even exist — should return None without error
        let result = load_config_from_dir(&dir).unwrap();
        assert!(result.is_none(), "missing config.toml must return None");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_from_slug_maps_both_brands() {
        assert_eq!(Brand::from_slug("lark-browser"), Some(Brand::Lark));
        assert_eq!(Brand::from_slug("feishu-browser"), Some(Brand::Feishu));
        assert_eq!(Brand::from_slug("gmail-browser"), None);
    }

    #[test]
    fn brand_web_url_and_platform() {
        assert_eq!(Brand::Lark.web_url(), "https://web.larksuite.com/messenger/");
        assert_eq!(Brand::Feishu.web_url(), "https://web.feishu.cn/messenger/");
        assert_eq!(Brand::Lark.platform(), "lark-browser");
        assert_eq!(Brand::Feishu.platform(), "feishu-browser");
    }

    #[test]
    fn brand_from_config_str_accepts_slug_and_short_forms() {
        assert_eq!(brand_from_config_str("lark"), Some(Brand::Lark));
        assert_eq!(brand_from_config_str("lark-browser"), Some(Brand::Lark));
        assert_eq!(brand_from_config_str("feishu"), Some(Brand::Feishu));
        assert_eq!(brand_from_config_str("feishu-browser"), Some(Brand::Feishu));
        assert_eq!(brand_from_config_str(""), None);
        assert_eq!(brand_from_config_str("gmail"), None);
    }

    #[test]
    fn safe_session_part_sanitizes_special_chars() {
        assert_eq!(safe_session_part("lark-browser"), "lark-browser");
        // dots and @ become dashes; consecutive dashes collapse
        assert_eq!(safe_session_part("user@example.com"), "user-example-com");
        // colons/spaces become dashes, consecutive dashes collapse
        let result = safe_session_part("a::b  c");
        assert!(!result.contains("--"));
        assert!(result.starts_with('a'));
    }

    #[test]
    fn feed_dedup_key_includes_conn_and_chat_and_preview_hash() {
        use crate::lark_browser_script::FeedRow;
        let r = FeedRow {
            chat_id: "999".into(),
            name: "Bob".into(),
            preview: "hello".into(),
            unread: false,
            is_outgoing: false,
        };
        let key = feed_dedup_key("my-conn", &r);
        assert!(key.starts_with("my-conn:999:"));
        // same preview → same key
        let key2 = feed_dedup_key("my-conn", &r);
        assert_eq!(key, key2);
        // different preview → different key
        let r2 = FeedRow { preview: "world".into(), ..r };
        let key3 = feed_dedup_key("my-conn", &r2);
        assert_ne!(key, key3);
    }
}
