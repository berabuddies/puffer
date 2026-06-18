//! Lark/Feishu web connector backed by daemon-managed CEF sessions.

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

use crate::lark_browser_script::{parse_feed_rows, FeedRow, LARK_FEED_SCRIPT};

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
    pub(crate) fn web_url(&self) -> &'static str {
        match self {
            Brand::Lark => "https://web.larksuite.com/",
            Brand::Feishu => "https://web.feishu.cn/",
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
    #[serde(default)]
    pub(crate) brand: String,
    #[serde(default)]
    pub(crate) connection: String,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
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
    handshake: &'a mut Option<crate::daemon::Handshake>,
) -> Result<&'a crate::daemon::Handshake> {
    if handshake.is_none() {
        let paths = ConfigPaths::discover(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        );
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
            handle_command(command)
        }
    }
}

fn handle_command(command: SubscriberCommand) -> Result<()> {
    match command {
        SubscriberCommand::Custom { op, .. } => {
            eprintln!("lark-browser: ignored unknown custom op={op}");
            Ok(())
        }
        _ => {
            eprintln!("lark-browser: ignored unrecognized command");
            Ok(())
        }
    }
}

fn poll_once_feed(
    env: &SubscriberEnv,
    config: &LarkBrowserConfig,
    brand: Brand,
    seen: &mut SeenState,
    handshake: &mut Option<crate::daemon::Handshake>,
) -> Result<()> {
    let handshake_ref = ensure_browser_daemon(handshake)?;

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

    let rows = parse_feed_rows(&parsed);
    let seen_count_before = seen.seen.len();
    let initialized_before = seen.initialized;
    let mut newly_seen = BTreeSet::new();
    let mut emitted = 0usize;

    for row in &rows {
        let key = feed_dedup_key(&config.connection, row);
        newly_seen.insert(key.clone());
        if should_emit_feed(seen, &key) {
            emitted += 1;
            emit_event(build_message_event(
                brand.platform(),
                &config.brand,
                &row.chat_id,
                &row.name,
                &row.preview,
                row.is_outgoing,
                "feed",
                &key,
            ))?;
        }
    }

    seen.seen.extend(newly_seen);
    seen.initialized = true;

    eprintln!(
        "lark-browser: poll_complete topic={} observed_rows={} emitted_rows={emitted} initialized_before={initialized_before} initialized_after={} seen_count_before={seen_count_before} seen_count_after={}",
        env.topic,
        rows.len(),
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
            wait_or_handle_command(&mut commands, POLL_INTERVAL).await?;
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
            wait_or_handle_command(&mut commands, POLL_INTERVAL).await?;
            continue;
        };

        let result = poll_once_feed(&env, &config, brand, &mut seen, &mut handshake);
        match result {
            Ok(()) => {
                save_seen(&env.state_dir, &seen)?;
                wait_or_handle_command(&mut commands, POLL_INTERVAL).await?;
            }
            Err(error) => {
                handshake = None;
                eprintln!("lark-browser: poll_loop_error topic={} error={error:#}", env.topic);
                emit_control(
                    &env.topic,
                    "poll_error",
                    json!({ "error": format!("{error:#}") }),
                )?;
                wait_or_handle_command(&mut commands, ERROR_BACKOFF).await?;
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
        assert_eq!(Brand::Lark.web_url(), "https://web.larksuite.com/");
        assert_eq!(Brand::Feishu.web_url(), "https://web.feishu.cn/");
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
