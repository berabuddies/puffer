//! `subscribe` — the monitor loop. Read-only: it never sends input.
//!
//! Flow: parse the subscribe command (connection slug → instance), bring the
//! container up, confirm login, then poll. The DEFAULT read path is the decrypted
//! chat-DB reader (`poll_db`), which emits a [`ConnectorSubscribeFrame::Event`]
//! per NEW message; when the DB reader is disabled it falls back to the vision
//! path (`poll_once`: screenshot, skip if byte-identical to last time, else ask
//! the vision model to extract messages).
//!
//! Robustness:
//! - dedup keys are PERSISTED (`<slug>.seen.json`) so a daemon restart does not
//!   re-emit messages still on screen; oldest keys are evicted (not wholesale
//!   cleared). On restart (a non-empty persisted set) the DB path starts primed
//!   and EMITS rows not already in the set, so messages that arrived while the
//!   bridge was DOWN are backfilled (bounded by `seen`); a vision-only monitor
//!   still cannot backfill an off-screen down-window (it sees only the current
//!   framebuffer).
//! - login loss mid-session is detected each poll and surfaced (and vision is
//!   paused while logged out, instead of feeding QR screens to the model).
//! - a WeChat risk/limit banner trips the anti-ban [`Policy`] pause so the act
//!   path stops sending.
//! - shutdown follows the bridge contract: the host closes our stdin; we race
//!   every poll against that signal so a slow vision call can't blow the host's
//!   grace window (no SIGKILL).

use anyhow::Result;
use puffer_subscriptions::{ConnectorSubscribeCommand, ConnectorSubscribeFrame};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use super::config::InstanceConfig;
use super::dbread;
use super::docker::WechatInstance;
use super::policy::Policy;
use super::read::{read_messages_from_png, read_risk_warning};

/// Base interval between screen polls. Overridable via `WECHAT_POLL_SECS`.
const DEFAULT_POLL_SECS: u64 = 6;
/// Cap on the persisted dedup set so it cannot grow unbounded.
const MAX_SEEN: usize = 4000;
/// Hours to pause sends after a WeChat risk warning is seen.
const RISK_PAUSE_HOURS: u64 = 24;

/// Runs the WeChat monitor bridge end to end.
pub(crate) async fn run() -> Result<()> {
    let mut host_stdin = BufReader::new(tokio::io::stdin());

    // 1. Subscribe command → connection slug → instance. EOF before any command
    //    is a clean exit; a malformed/unexpected first line is a protocol error
    //    (don't silently fall back to the default instance and monitor the wrong
    //    container).
    let mut first_line = String::new();
    if host_stdin.read_line(&mut first_line).await? == 0 {
        return Ok(()); // host closed stdin before sending a subscribe command
    }
    let connection = match serde_json::from_str::<ConnectorSubscribeCommand>(first_line.trim()) {
        Ok(ConnectorSubscribeCommand::Subscribe { connection, .. }) => connection,
        _ => {
            write_frame(&health(
                "error",
                "expected a `subscribe` command as the first line".to_string(),
            ))
            .await?;
            drain_stdin(host_stdin).await;
            return Ok(());
        }
    };
    let instance = WechatInstance::for_connection(&connection);

    // 2. Bring the container up and verify login + tooling. Any failure is a
    //    health error, after which we idle until the host closes stdin.
    if let Some(detail) = prepare(&instance).await {
        write_frame(&health("error", detail)).await?;
        drain_stdin(host_stdin).await;
        return Ok(());
    }
    write_frame(&health("ok", format!("monitoring WeChat `{}`", instance.name()))).await?;

    // 3. Shutdown signal: a task drains host stdin and fires on EOF, so a slow
    //    vision call can be raced against it (M12) — no SIGKILL past the grace.
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        drain_stdin(host_stdin).await;
        let _ = shutdown_tx.send(());
    });

    // 4. Poll loop. Read path: direct chat-DB (cheaper/accurate) when enabled,
    //    else screenshot+vision.
    let db_mode = dbread::enabled();
    if db_mode {
        write_frame(&health("ok", "using direct chat-DB reader".to_string())).await.ok();
    }
    let mut seen = SeenStore::load(instance.name());
    // DB mode + a persisted `seen` means RESTART: start primed so the first poll
    // emits rows not in `seen` (messages missed while down) instead of re-seeding
    // them; a first run (empty `seen`) primes silently. Vision keeps silent priming
    // — the screen holds arbitrary read history, so "unseen" rows aren't new.
    let mut primed = db_mode && !seen.is_empty();
    let mut last_shot = String::new();
    let mut logged_in = true;
    let mut cursor = 0.0_f64;
    let mut ticker = tokio::time::interval(poll_interval());
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => break,
            _ = ticker.tick() => {
                // Build only ONE poll future (avoids a double &mut borrow), each
                // raced against shutdown so a slow read can't blow the grace.
                let result = if db_mode {
                    tokio::select! {
                        r = poll_db(&instance, &mut seen, &mut cursor, &mut primed, &mut logged_in) => r,
                        _ = &mut shutdown_rx => break,
                    }
                } else {
                    tokio::select! {
                        r = poll_once(&instance, &mut seen, &mut last_shot, &mut primed, &mut logged_in) => r,
                        _ = &mut shutdown_rx => break,
                    }
                };
                if let Err(error) = result {
                    write_frame(&health("degraded", format!("{error:#}"))).await.ok();
                }
            }
        }
    }
    seen.save();
    Ok(())
}

/// One DB poll: reads conversations at-or-after `cursor` from the decrypted
/// `session.db` and emits an event per NEW one. The cursor advances to the max
/// timestamp but the query is INCLUSIVE (`>= cursor`) and a persisted dedup set
/// (`seen`, keyed by chat+text+create_time) suppresses repeats — so a second
/// conversation that ties the cursor's whole-second timestamp is not dropped
/// (which a strict `>` cursor would do). Primes silently on the first read,
/// seeding `seen` so the existing backlog is not replayed. Mirrors
/// [`poll_once`]'s login handling.
async fn poll_db(
    instance: &WechatInstance,
    seen: &mut SeenStore,
    cursor: &mut f64,
    primed: &mut bool,
    logged_in: &mut bool,
) -> Result<()> {
    let now_logged_in = instance.is_logged_in().await.unwrap_or(false);
    if now_logged_in != *logged_in {
        *logged_in = now_logged_in;
        let detail = if now_logged_in {
            ("ok", format!("WeChat `{}` login restored", instance.name()))
        } else {
            ("error", format!("WeChat `{}` is logged out; reconnect it via the connector setup to re-show the QR and resume", instance.name()))
        };
        write_frame(&health(detail.0, detail.1)).await?;
    }
    if !now_logged_in {
        return Ok(());
    }

    let read = dbread::read_session(instance, *cursor).await?;
    if read.cursor > *cursor {
        *cursor = read.cursor;
    }
    let mut emitted = false;
    for message in read.messages {
        let key = hash_hex(
            format!("{}\u{1}{}\u{1}{}", message.chat, message.text, message.create_time).as_bytes(),
        );
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key.clone());
        emitted = true;
        // First read just SEEDS the dedup set — don't replay the existing backlog.
        if !*primed {
            continue;
        }
        let text = message.text.clone();
        let payload = json!({
            "kind": "message",
            "text": text,
            "message": text,
            "sender": message.sender,
            "chat": message.chat,
            "connection": instance.name(),
            "create_time": message.create_time,
        });
        write_frame(&ConnectorSubscribeFrame::Event {
            id: key.clone(),
            cursor: key,
            payload,
        })
        .await?;
    }
    *primed = true;
    if emitted {
        seen.save();
    }
    Ok(())
}

/// Brings the container up and checks login + screenshot tooling. Returns
/// `Some(detail)` describing the first problem, or `None` when ready.
async fn prepare(instance: &WechatInstance) -> Option<String> {
    let cfg = match InstanceConfig::load_or_create(instance.name()) {
        Ok(cfg) => cfg,
        Err(error) => return Some(format!("wechat config error: {error:#}")),
    };
    if let Err(error) = instance.ensure_container(&cfg).await {
        return Some(format!("wechat bringup failed: {error:#}"));
    }
    if !instance.is_logged_in().await.unwrap_or(false) {
        return Some(format!(
            "WeChat `{}` is not connected; reconnect it via the connector setup (it shows the QR to scan)",
            instance.name()
        ));
    }
    // The screenshot tool + capture sizing serve ONLY the vision poll path. In DB
    // mode (the default) the monitor reads the decrypted chat DB, so they are not
    // needed — and a failed install (e.g. restricted container network) must NOT
    // idle the whole monitor before it ever polls. Require them only when actually
    // polling via vision.
    if !dbread::enabled() {
        if let Err(error) = instance.ensure_screenshot_tool().await {
            return Some(format!("screenshot tool unavailable: {error:#}"));
        }
        // Enlarge the screen + WeChat window so captures are sharp enough to read.
        let _ = instance.prepare_capture().await;
    }
    None
}

/// One poll: re-check login, screenshot, skip if unchanged, else read + emit new
/// messages (and watch for a risk banner).
async fn poll_once(
    instance: &WechatInstance,
    seen: &mut SeenStore,
    last_shot: &mut String,
    primed: &mut bool,
    logged_in: &mut bool,
) -> Result<()> {
    // H3: detect login loss/restore each poll; while logged out, do not feed QR
    //     screens to the (paid) vision model.
    let now_logged_in = instance.is_logged_in().await.unwrap_or(false);
    if now_logged_in != *logged_in {
        *logged_in = now_logged_in;
        if now_logged_in {
            write_frame(&health("ok", format!("WeChat `{}` login restored", instance.name()))).await?;
        } else {
            write_frame(&health(
                "error",
                format!("WeChat `{}` is logged out; reconnect it via the connector setup to re-show the QR and resume", instance.name()),
            ))
            .await?;
        }
    }
    if !now_logged_in {
        return Ok(());
    }

    let png = instance.screenshot_png().await?;
    let shot_hash = hash_hex(&png);
    if shot_hash == *last_shot {
        return Ok(()); // screen unchanged — no vision call needed
    }

    // Vision call is blocking (reqwest::blocking); keep it off the async runtime.
    let png_for_read = png.clone();
    let messages =
        tokio::task::spawn_blocking(move || read_messages_from_png(&png_for_read)).await??;

    // Only advance the screen hash AFTER a successful read (M5) — otherwise a
    // failed read would permanently mask that screen's messages.
    *last_shot = shot_hash;

    // B1: when nothing message-like is found on a changed screen, check for a
    //     WeChat risk/limit banner and, if present, pause the act path.
    if messages.is_empty() {
        let png_for_risk = png.clone();
        if let Ok(Some(warning)) =
            tokio::task::spawn_blocking(move || read_risk_warning(&png_for_risk)).await?
        {
            let _ = Policy::load(instance.name()).note_risk_warning(RISK_PAUSE_HOURS);
            write_frame(&health(
                "error",
                format!("WeChat risk/limit warning detected; sends paused {RISK_PAUSE_HOURS}h: {warning}"),
            ))
            .await?;
        }
    }

    let mut emitted = false;
    for message in messages {
        let key = hash_hex(
            format!("{}\u{1}{}\u{1}{}", message.chat, message.sender, message.text).as_bytes(),
        );
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key.clone());
        emitted = true;
        // Seed silently on the first read so we don't replay the on-screen backlog.
        if !*primed {
            continue;
        }
        let text = message.text.clone();
        // `chat` is the CONVERSATION (the routable reply target), distinct from
        // `sender` (the author) — critical for group messages, where the author
        // is a member, not the group. The vision contract returns both; fall back
        // to sender only if the model omitted chat.
        let chat = if message.chat.trim().is_empty() {
            message.sender.clone()
        } else {
            message.chat.clone()
        };
        let payload = json!({
            "kind": "message",
            "text": text,
            "message": text,
            "sender": message.sender,
            "chat": chat,
            "connection": instance.name(),
        });
        write_frame(&ConnectorSubscribeFrame::Event {
            id: key.clone(),
            cursor: key,
            payload,
        })
        .await?;
    }
    *primed = true;
    if emitted {
        seen.save();
    }
    Ok(())
}

/// Persisted, bounded, insertion-ordered dedup set (`<slug>.seen.json`).
struct SeenStore {
    set: HashSet<String>,
    order: VecDeque<String>,
    path: Option<PathBuf>,
    dirty: bool,
}

impl SeenStore {
    fn load(instance: &str) -> Self {
        let path = seen_path(instance);
        let order: VecDeque<String> = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .map(VecDeque::from)
            .unwrap_or_default();
        let set: HashSet<String> = order.iter().cloned().collect();
        Self {
            set,
            order,
            path,
            dirty: false,
        }
    }

    fn contains(&self, key: &str) -> bool {
        self.set.contains(key)
    }

    /// Whether no keys are loaded — true only on a first-ever run (no persisted
    /// state), used to decide silent-prime vs emit-new-on-restart.
    fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Inserts a key, evicting the OLDEST (not wholesale clearing) at the cap (M4).
    fn insert(&mut self, key: String) {
        if self.set.insert(key.clone()) {
            self.order.push_back(key);
            while self.order.len() > MAX_SEEN {
                if let Some(old) = self.order.pop_front() {
                    self.set.remove(&old);
                }
            }
            self.dirty = true;
        }
    }

    fn save(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            if let Ok(raw) = serde_json::to_string(&self.order) {
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, raw).is_ok() {
                    let _ = std::fs::rename(&tmp, path);
                }
            }
        }
        self.dirty = false;
    }
}

/// `~/.puffer/wechat/<instance>.seen.json` (honors WECHAT_STATE_DIR/PUFFER_HOME/HOME).
fn seen_path(instance: &str) -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("WECHAT_STATE_DIR") {
        if !dir.trim().is_empty() {
            return Some(PathBuf::from(dir).join(format!("{instance}.seen.json")));
        }
    }
    let home = std::env::var("PUFFER_HOME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("HOME").ok())?;
    Some(
        PathBuf::from(home)
            .join(".puffer")
            .join("wechat")
            .join(format!("{instance}.seen.json")),
    )
}

/// Poll interval, honoring `WECHAT_POLL_SECS` (min 2s).
fn poll_interval() -> Duration {
    let secs = std::env::var("WECHAT_POLL_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_POLL_SECS)
        .max(2);
    Duration::from_secs(secs)
}

/// Hex-encoded SHA-256 of `bytes`.
fn hash_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Builds a health frame.
fn health(status: &str, detail: String) -> ConnectorSubscribeFrame {
    ConnectorSubscribeFrame::Health {
        status: status.to_string(),
        detail: Some(detail),
    }
}

/// Writes one frame as a single NDJSON line to stdout and flushes.
async fn write_frame(frame: &ConnectorSubscribeFrame) -> Result<()> {
    let mut line = serde_json::to_string(frame)?;
    line.push('\n');
    let mut out = tokio::io::stdout();
    out.write_all(line.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

/// Reads and discards host stdin until EOF (the graceful-shutdown signal).
async fn drain_stdin(mut host_stdin: BufReader<tokio::io::Stdin>) {
    let mut line = String::new();
    loop {
        line.clear();
        match host_stdin.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_distinct() {
        assert_eq!(hash_hex(b"a"), hash_hex(b"a"));
        assert_ne!(hash_hex(b"a"), hash_hex(b"b"));
        assert_eq!(hash_hex(b"a").len(), 64);
    }

    #[test]
    fn poll_interval_respects_floor() {
        // Save/restore the process-global env so parallel tests aren't affected.
        let prev = std::env::var("WECHAT_POLL_SECS").ok();
        std::env::remove_var("WECHAT_POLL_SECS");
        assert_eq!(poll_interval(), Duration::from_secs(DEFAULT_POLL_SECS));
        if let Some(v) = prev {
            std::env::set_var("WECHAT_POLL_SECS", v);
        }
    }

    #[test]
    fn seen_store_evicts_oldest_not_all() {
        let mut s = SeenStore {
            set: HashSet::new(),
            order: VecDeque::new(),
            path: None,
            dirty: false,
        };
        for i in 0..(MAX_SEEN + 5) {
            s.insert(format!("k{i}"));
        }
        assert_eq!(s.order.len(), MAX_SEEN);
        // Oldest evicted, newest retained.
        assert!(!s.contains("k0"));
        assert!(s.contains(&format!("k{}", MAX_SEEN + 4)));
    }
}
