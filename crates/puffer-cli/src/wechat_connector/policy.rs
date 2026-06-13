//! Anti-ban behaviour policy for the WeChat connector.
//!
//! `human.rs` jitters *individual* keystrokes/clicks; this module governs the
//! *aggregate* behaviour that actually gets personal WeChat accounts limited or
//! banned (per the 2026 research brief): how OFTEN we send, WHEN, whether every
//! message is answered, and whether content is repetitive. Every outbound action
//! must pass [`Policy::check_send`] (and honour [`Policy::wait_before_send`])
//! before it runs, and call [`Policy::record_send`] after.
//!
//! State is persisted per instance at `~/.puffer/wechat/<instance>.policy.json`
//! so caps survive restarts. All thresholds are env-tunable (so testing can
//! relax them) but default to conservative, community-observed ceilings.
//!
//! Defaults target an ESTABLISHED account (no warm-up). Set `WECHAT_WARMUP=1`
//! for a fresh account to ramp the daily cap over the first weeks.

use anyhow::{Context, Result};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Reason an outbound action was blocked. The agent surfaces this and does NOT
/// retry blindly — most blocks are "try later", not "try again now".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PolicyBlock {
    /// A risk/frequency warning was seen; all sends paused until this time.
    Paused { until_ms: u64 },
    /// Outside the allowed active-hours window.
    OutsideActiveHours { hour: u32 },
    /// Too many sends in the last minute.
    RateMinute { cap: u32 },
    /// Daily send ceiling reached (warm-up aware).
    RateDay { cap: u32 },
    /// Identical to a recently-sent message (templated-content flag risk).
    DuplicateContent,
    /// Contains a flagged sensitive word.
    SensitiveContent { word: String },
    /// Too many DISTINCT recipients in the recent window (fan-out / spam shape).
    DistinctRecipients { cap: u32 },
    /// Policy state is corrupt/unreadable; fail closed rather than reset caps.
    StateUnavailable,
}

impl fmt::Display for PolicyBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyBlock::Paused { until_ms } => write!(
                f,
                "sends are paused after a WeChat risk warning (until epoch_ms {until_ms}); not retrying"
            ),
            PolicyBlock::OutsideActiveHours { hour } => write!(
                f,
                "outside active hours (local hour {hour}); deferring to avoid 24/7 activity"
            ),
            PolicyBlock::RateMinute { cap } => {
                write!(f, "per-minute send cap reached ({cap}/min); slow down")
            }
            PolicyBlock::RateDay { cap } => {
                write!(f, "daily send cap reached ({cap}/day)")
            }
            PolicyBlock::DuplicateContent => {
                write!(f, "message is identical to a recent send; vary the wording")
            }
            PolicyBlock::SensitiveContent { word } => {
                write!(f, "message contains a flagged sensitive word `{word}`")
            }
            PolicyBlock::DistinctRecipients { cap } => write!(
                f,
                "too many distinct recipients recently (cap {cap}/hour); looks like fan-out"
            ),
            PolicyBlock::StateUnavailable => {
                write!(f, "anti-ban policy state is unreadable; refusing to send (fail-closed)")
            }
        }
    }
}

/// Persisted per-instance policy state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PolicyState {
    /// Epoch ms when automated sending was first enabled (for warm-up ramp).
    warm_up_start_ms: u64,
    /// Local day index (epoch_local_secs / 86400) the counters belong to.
    day_index: u64,
    /// Sends recorded on `day_index`.
    day_count: u32,
    /// Epoch ms of recent sends; pruned to the last 60s on each check/record
    /// (per-minute window), so between calls it may hold older entries.
    recent_sends_ms: Vec<u64>,
    /// Epoch ms of the most recent send.
    last_send_ms: u64,
    /// All sends paused until this epoch ms (risk-warning reflex).
    paused_until_ms: u64,
    /// Recently-sent message texts (normalized), for duplicate detection (bounded).
    recent_contents: Vec<String>,
    /// Recent (recipient, epoch_ms) pairs, for distinct-recipient burst limiting.
    #[serde(default)]
    recent_recipients: Vec<(String, u64)>,
}

/// Tunable thresholds, read once from the environment.
#[derive(Debug, Clone)]
struct PolicyConfig {
    tz_offset_hours: i64,
    /// Active-hours window `(start, end)`. `None` = NO time restriction (the
    /// default); only enforced when `WECHAT_ACTIVE_HOURS` is explicitly set.
    active_hours: Option<(u32, u32)>,
    per_minute_cap: u32,
    day_cap_steady: u32,
    min_gap_ms: (u64, u64),
    reply_probability: f64,
    warmup: bool,
    recent_content_window: usize,
    distinct_recipients_per_hour: u32,
    sensitive_words: Vec<String>,
}

impl PolicyConfig {
    fn from_env() -> Self {
        Self {
            tz_offset_hours: env_i64("WECHAT_TZ_OFFSET", 8),
            // No time restriction by default; opt in via WECHAT_ACTIVE_HOURS.
            active_hours: parse_hours_env("WECHAT_ACTIVE_HOURS"),
            // Relaxed, human-but-not-slow defaults: a few seconds between sends,
            // and a daily budget sized at ~8h x 1/min. No time-of-day window is
            // enforced (active_hours is opt-in above). All env-tunable.
            per_minute_cap: env_u32("WECHAT_MAX_PER_MIN", 20),
            day_cap_steady: env_u32("WECHAT_MAX_PER_DAY", 480),
            min_gap_ms: parse_range_env("WECHAT_MIN_GAP_MS").unwrap_or((3000, 8000)),
            reply_probability: env_f64("WECHAT_REPLY_PROB", 0.9),
            warmup: std::env::var("WECHAT_WARMUP").map(|v| v.trim() == "1").unwrap_or(false),
            recent_content_window: env_u32("WECHAT_CONTENT_WINDOW", 8) as usize,
            distinct_recipients_per_hour: env_u32("WECHAT_MAX_DISTINCT_RECIPIENTS", 20),
            sensitive_words: sensitive_words_from_env(),
        }
    }
}

/// The policy gate for one instance.
pub(crate) struct Policy {
    instance: String,
    state: PolicyState,
    cfg: PolicyConfig,
    /// True when the on-disk state existed but could not be parsed — we then
    /// fail closed (block sends) rather than silently resetting all caps.
    corrupt: bool,
}

impl Policy {
    /// Loads (or initializes) the policy state for `instance`. A missing file is
    /// a clean start; a present-but-corrupt file marks the policy `corrupt` so
    /// [`Self::check_send`] fails closed.
    pub(crate) fn load(instance: &str) -> Self {
        let cfg = PolicyConfig::from_env();
        let (state, corrupt) = match read_state(instance) {
            ReadState::Ok(state) => (state, false),
            ReadState::Missing => (PolicyState::default(), false),
            ReadState::Corrupt => (PolicyState::default(), true),
        };
        Self {
            instance: instance.to_string(),
            state,
            cfg,
            corrupt,
        }
    }

    /// Checks whether a send of `text` is permitted right now. Returns the first
    /// violated rule, or `Ok(())` if allowed. Prunes the per-minute window and
    /// rolls the daily counter as a side effect (call before each send).
    pub(crate) fn check_send(
        &mut self,
        recipient: &str,
        text: &str,
    ) -> std::result::Result<(), PolicyBlock> {
        if self.corrupt {
            return Err(PolicyBlock::StateUnavailable);
        }
        let now = now_ms();
        if self.state.paused_until_ms > now {
            return Err(PolicyBlock::Paused {
                until_ms: self.state.paused_until_ms,
            });
        }
        let hour = self.local_hour(now);
        if !self.in_active_hours(hour) {
            return Err(PolicyBlock::OutsideActiveHours { hour });
        }
        // Per-minute window.
        self.state
            .recent_sends_ms
            .retain(|&ts| now.saturating_sub(ts) < 60_000);
        if self.state.recent_sends_ms.len() as u32 >= self.cfg.per_minute_cap {
            return Err(PolicyBlock::RateMinute {
                cap: self.cfg.per_minute_cap,
            });
        }
        // Daily counter (rolls at local midnight).
        let today = self.local_day_index(now);
        if self.state.day_index != today {
            self.state.day_index = today;
            self.state.day_count = 0;
        }
        let cap = self.daily_cap(now);
        if self.state.day_count >= cap {
            return Err(PolicyBlock::RateDay { cap });
        }
        // Distinct-recipient burst (fan-out shape): count distinct recipients in
        // the last hour; a NEW recipient beyond the cap is blocked.
        self.state
            .recent_recipients
            .retain(|(_, ts)| now.saturating_sub(*ts) < 3_600_000);
        let r = recipient.trim();
        if !r.is_empty() && !self.state.recent_recipients.iter().any(|(name, _)| name == r) {
            let distinct = self
                .state
                .recent_recipients
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len() as u32;
            if distinct >= self.cfg.distinct_recipients_per_hour {
                return Err(PolicyBlock::DistinctRecipients {
                    cap: self.cfg.distinct_recipients_per_hour,
                });
            }
        }
        // Content checks.
        if let Some(word) = self.sensitive_hit(text) {
            return Err(PolicyBlock::SensitiveContent { word });
        }
        let normalized = normalize_content(text);
        if self
            .state
            .recent_contents
            .iter()
            .any(|prev| prev == &normalized)
        {
            return Err(PolicyBlock::DuplicateContent);
        }
        Ok(())
    }

    /// Milliseconds the caller should sleep before sending, to honour the
    /// randomized minimum gap between two outbound actions.
    pub(crate) fn wait_before_send(&self) -> u64 {
        let now = now_ms();
        let gap = rand_in(self.cfg.min_gap_ms);
        let earliest = self.state.last_send_ms.saturating_add(gap);
        earliest.saturating_sub(now)
    }

    /// Records a successful send and persists state. Done under an exclusive
    /// file lock with a fresh re-read so concurrent `act` processes merge their
    /// increments instead of clobbering each other's counts.
    pub(crate) fn record_send(&mut self, recipient: &str, text: &str) -> Result<()> {
        let now = now_ms();
        let _guard = FileLock::acquire(&self.instance);
        // Merge with the latest on-disk state (other processes may have sent).
        if let ReadState::Ok(fresh) = read_state(&self.instance) {
            self.state = fresh;
        }
        if self.state.warm_up_start_ms == 0 {
            self.state.warm_up_start_ms = now;
        }
        let today = self.local_day_index(now);
        if self.state.day_index != today {
            self.state.day_index = today;
            self.state.day_count = 0;
        }
        self.state.day_count += 1;
        self.state.last_send_ms = self.state.last_send_ms.max(now);
        self.state.recent_sends_ms.push(now);
        self.state
            .recent_sends_ms
            .retain(|&ts| now.saturating_sub(ts) < 60_000); // prune here too (record path)
        let r = recipient.trim();
        if !r.is_empty() {
            self.state.recent_recipients.push((r.to_string(), now));
        }
        self.state
            .recent_recipients
            .retain(|(_, ts)| now.saturating_sub(*ts) < 3_600_000);
        self.state.recent_contents.push(normalize_content(text));
        let window = self.cfg.recent_content_window.max(1);
        let len = self.state.recent_contents.len();
        if len > window {
            self.state.recent_contents.drain(0..len - window);
        }
        self.save()
    }

    /// Trips the risk-warning reflex: pause all sends for `hours` and persist.
    /// Locked + fresh-read so it cannot be lost to a concurrent writer.
    pub(crate) fn note_risk_warning(&mut self, hours: u64) -> Result<()> {
        let _guard = FileLock::acquire(&self.instance);
        if let ReadState::Ok(fresh) = read_state(&self.instance) {
            self.state = fresh;
        }
        self.state.paused_until_ms = now_ms().saturating_add(hours.saturating_mul(3_600_000));
        self.save()
    }

    /// Whether the agent should reply to a given inbound message — a bot that
    /// answers 100% of messages instantly is itself a flag.
    pub(crate) fn should_reply(&self) -> bool {
        rand::thread_rng().gen_bool(self.cfg.reply_probability.clamp(0.0, 1.0))
    }

    /// Daily cap, applying the warm-up ramp when `WECHAT_WARMUP=1`.
    fn daily_cap(&self, now: u64) -> u32 {
        if !self.cfg.warmup || self.state.warm_up_start_ms == 0 {
            return self.cfg.day_cap_steady;
        }
        let days = now.saturating_sub(self.state.warm_up_start_ms) / 86_400_000;
        let steady = self.cfg.day_cap_steady;
        match days {
            0..=6 => 20.min(steady),
            7..=13 => 50.min(steady),
            14..=20 => 100.min(steady),
            21..=27 => 200.min(steady),
            _ => steady,
        }
    }

    fn in_active_hours(&self, hour: u32) -> bool {
        let (start, end) = match self.cfg.active_hours {
            None => return true,        // no time restriction (default)
            Some(window) => window,
        };
        if start == end {
            return true; // disabled (equal bounds)
        }
        if start < end {
            hour >= start && hour < end
        } else {
            hour >= start || hour < end // wraps midnight (e.g. 22-6)
        }
    }

    fn local_hour(&self, now_ms: u64) -> u32 {
        let secs = (now_ms / 1000) as i64 + self.cfg.tz_offset_hours * 3600;
        (secs.rem_euclid(86_400) / 3600) as u32
    }

    fn local_day_index(&self, now_ms: u64) -> u64 {
        let secs = (now_ms / 1000) as i64 + self.cfg.tz_offset_hours * 3600;
        (secs.max(0) / 86_400) as u64
    }

    fn sensitive_hit(&self, text: &str) -> Option<String> {
        self.cfg
            .sensitive_words
            .iter()
            .find(|word| !word.is_empty() && text.contains(word.as_str()))
            .cloned()
    }

    /// Atomically persists state: write a temp file in the same dir, then rename
    /// over the target so a crash mid-write cannot truncate/corrupt it.
    fn save(&self) -> Result<()> {
        let path = state_path(&self.instance)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let raw = serde_json::to_string_pretty(&self.state)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, raw).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("rename into {}", path.display()))
    }
}

/// Result of reading the persisted state: cleanly distinguishes a missing file
/// (fresh start) from a corrupt one (fail closed).
enum ReadState {
    Ok(PolicyState),
    Missing,
    Corrupt,
}

/// RAII exclusive lock on the per-instance policy file (advisory `flock`).
/// Best-effort: if the lock can't be taken we proceed (single-writer is the
/// common case); the lock matters only under concurrent `act` processes.
struct FileLock {
    _file: Option<std::fs::File>,
}

impl FileLock {
    fn acquire(instance: &str) -> Self {
        let file = lock_path(instance).ok().and_then(|p| {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&p)
                .ok()
        });
        if let Some(f) = &file {
            // Blocking exclusive lock; released when the file is dropped/closed.
            unsafe {
                libc::flock(f.as_raw_fd(), libc::LOCK_EX);
            }
        }
        Self { _file: file }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if let Some(f) = &self._file {
            unsafe {
                libc::flock(f.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

/// RAII exclusive lock (advisory `flock` on `<instance>.ui.lock`) that serializes
/// ALL UI-driving `act` processes for one instance. The container has a single X
/// display, keyboard focus, and clipboard selection, so two concurrent acts would
/// race — pasting the wrong content or sending to the wrong chat. Held for the
/// WHOLE action. The read-only vision monitor reads only the framebuffer and does
/// NOT take this. Blocking; bounded by the host's per-action command timeout.
pub(crate) struct UiLock {
    _file: Option<std::fs::File>,
}

impl UiLock {
    pub(crate) fn acquire(instance: &str) -> Self {
        let file = ui_lock_path(instance).and_then(|p| {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&p)
                .ok()
        });
        if let Some(f) = &file {
            unsafe {
                libc::flock(f.as_raw_fd(), libc::LOCK_EX);
            }
        }
        Self { _file: file }
    }
}

impl Drop for UiLock {
    fn drop(&mut self) {
        if let Some(f) = &self._file {
            unsafe {
                libc::flock(f.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

/// `~/.puffer/wechat/<instance>.ui.lock` — the per-instance UI serialization lock.
fn ui_lock_path(instance: &str) -> Option<PathBuf> {
    state_path(instance).ok().map(|p| p.with_file_name(format!("{instance}.ui.lock")))
}

/// Normalizes message text for duplicate detection: collapse all whitespace,
/// strip surrounding punctuation/quotes, and casefold — so trivial variants of
/// the same message still count as duplicates.
fn normalize_content(text: &str) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .to_lowercase()
}

/// Default sensitive words (overridable/extendable via `WECHAT_SENSITIVE_WORDS`,
/// comma-separated).
fn sensitive_words_from_env() -> Vec<String> {
    let mut words: Vec<String> = ["转账", "收款", "红包", "返现", "加微信", "点击链接"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Ok(extra) = std::env::var("WECHAT_SENSITIVE_WORDS") {
        for w in extra.split(',').map(str::trim).filter(|w| !w.is_empty()) {
            words.push(w.to_string());
        }
    }
    words
}

fn read_state(instance: &str) -> ReadState {
    let Ok(path) = state_path(instance) else {
        return ReadState::Missing;
    };
    match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<PolicyState>(&raw) {
            Ok(state) => ReadState::Ok(state),
            Err(error) => {
                eprintln!("wechat policy: corrupt state {} ({error}); failing closed", path.display());
                ReadState::Corrupt
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ReadState::Missing,
        Err(error) => {
            eprintln!("wechat policy: cannot read {} ({error}); failing closed", path.display());
            ReadState::Corrupt
        }
    }
}

/// `~/.puffer/wechat/<instance>.policy.lock` — the advisory lock file.
fn lock_path(instance: &str) -> Result<PathBuf> {
    Ok(state_path(instance)?.with_extension("lock"))
}

/// `~/.puffer/wechat/<instance>.policy.json` (honors WECHAT_STATE_DIR/PUFFER_HOME/HOME).
fn state_path(instance: &str) -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("WECHAT_STATE_DIR") {
        if !dir.trim().is_empty() {
            return Ok(PathBuf::from(dir).join(format!("{instance}.policy.json")));
        }
    }
    let home = std::env::var("PUFFER_HOME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("HOME").ok())
        .context("neither PUFFER_HOME nor HOME is set")?;
    Ok(PathBuf::from(home)
        .join(".puffer")
        .join("wechat")
        .join(format!("{instance}.policy.json")))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn rand_in(range: (u64, u64)) -> u64 {
    let (lo, hi) = (range.0.min(range.1), range.0.max(range.1));
    if lo == hi {
        return lo;
    }
    rand::thread_rng().gen_range(lo..=hi)
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn env_i64(key: &str, default: i64) -> i64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn parse_range_env(key: &str) -> Option<(u64, u64)> {
    let raw = std::env::var(key).ok()?;
    let (a, b) = raw.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn parse_hours_env(key: &str) -> Option<(u32, u32)> {
    let raw = std::env::var(key).ok()?;
    let (a, b) = raw.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_policy(cfg: PolicyConfig) -> Policy {
        Policy {
            instance: "unit-test".to_string(),
            state: PolicyState::default(),
            cfg,
            corrupt: false,
        }
    }

    fn base_cfg() -> PolicyConfig {
        PolicyConfig {
            tz_offset_hours: 8,
            active_hours: None, // no time restriction (deterministic tests)
            per_minute_cap: 3,
            day_cap_steady: 300,
            min_gap_ms: (8000, 25000),
            reply_probability: 1.0,
            warmup: false,
            recent_content_window: 4,
            distinct_recipients_per_hour: 20,
            sensitive_words: vec!["transfer".to_string()],
        }
    }

    #[test]
    fn allows_normal_send_then_records() {
        let mut p = test_policy(base_cfg());
        assert!(p.check_send("alice", "hello").is_ok());
        // record without persisting to disk for the test
        let now = now_ms();
        p.state.day_count += 1;
        p.state.last_send_ms = now;
        p.state.recent_sends_ms.push(now);
        p.state.recent_contents.push(normalize_content("hello"));
        // duplicate now blocked, even with trivial variation (whitespace/case/punct)
        assert_eq!(
            p.check_send("alice", "  Hello! "),
            Err(PolicyBlock::DuplicateContent)
        );
    }

    #[test]
    fn blocks_sensitive_content() {
        let mut p = test_policy(base_cfg());
        match p.check_send("x", "please transfer me money") {
            Err(PolicyBlock::SensitiveContent { word }) => assert_eq!(word, "transfer"),
            other => panic!("expected sensitive block, got {other:?}"),
        }
    }

    #[test]
    fn per_minute_cap_enforced() {
        let mut p = test_policy(base_cfg());
        let now = now_ms();
        p.state.recent_sends_ms = vec![now, now, now];
        assert_eq!(p.check_send("r", "x"), Err(PolicyBlock::RateMinute { cap: 3 }));
    }

    #[test]
    fn daily_cap_enforced() {
        let mut cfg = base_cfg();
        cfg.day_cap_steady = 2;
        let mut p = test_policy(cfg);
        p.state.day_index = p.local_day_index(now_ms());
        p.state.day_count = 2;
        assert_eq!(p.check_send("r", "x"), Err(PolicyBlock::RateDay { cap: 2 }));
    }

    #[test]
    fn paused_blocks_everything() {
        let mut p = test_policy(base_cfg());
        p.state.paused_until_ms = now_ms() + 60_000;
        assert!(matches!(p.check_send("r", "x"), Err(PolicyBlock::Paused { .. })));
    }

    #[test]
    fn distinct_recipient_burst_blocked() {
        let mut cfg = base_cfg();
        cfg.distinct_recipients_per_hour = 2;
        let mut p = test_policy(cfg);
        let now = now_ms();
        p.state.recent_recipients = vec![("a".into(), now), ("b".into(), now)];
        // a NEW (3rd distinct) recipient is blocked; an existing one is fine.
        assert_eq!(
            p.check_send("c", "hi"),
            Err(PolicyBlock::DistinctRecipients { cap: 2 })
        );
        assert!(p.check_send("a", "hi").is_ok());
    }

    #[test]
    fn corrupt_state_fails_closed() {
        let mut p = test_policy(base_cfg());
        p.corrupt = true;
        assert_eq!(p.check_send("a", "hi"), Err(PolicyBlock::StateUnavailable));
    }

    #[test]
    fn normalize_collapses_variants() {
        // Whitespace, case, and surrounding punctuation are normalized away.
        assert_eq!(normalize_content("  Hello  World! "), normalize_content("hello world"));
        assert_eq!(normalize_content("OK."), normalize_content("ok"));
    }

    #[test]
    fn active_hours_unrestricted_by_default() {
        let mut cfg = base_cfg();
        cfg.active_hours = None; // default: no time restriction
        let p = test_policy(cfg);
        assert!(p.in_active_hours(0) && p.in_active_hours(3) && p.in_active_hours(22));
    }

    #[test]
    fn active_hours_window_when_set() {
        let mut cfg = base_cfg();
        cfg.active_hours = Some((8, 22));
        let p = test_policy(cfg);
        assert!(p.in_active_hours(8) && p.in_active_hours(21));
        assert!(!p.in_active_hours(7) && !p.in_active_hours(22) && !p.in_active_hours(3));
    }

    #[test]
    fn active_hours_wrapping_midnight() {
        let mut cfg = base_cfg();
        cfg.active_hours = Some((22, 6));
        let p = test_policy(cfg);
        assert!(p.in_active_hours(23) && p.in_active_hours(2));
        assert!(!p.in_active_hours(12));
    }

    #[test]
    fn warmup_ramps_daily_cap() {
        let mut cfg = base_cfg();
        cfg.warmup = true;
        let mut p = test_policy(cfg);
        let now = now_ms();
        p.state.warm_up_start_ms = now; // day 0
        assert_eq!(p.daily_cap(now), 20);
        assert_eq!(p.daily_cap(now + 8 * 86_400_000), 50);
        assert_eq!(p.daily_cap(now + 30 * 86_400_000), 300);
    }
}
