//! Process-wide mutable state for the Telegram subscriber.
//!
//! The subscriber holds at most one active login attempt at a time. While
//! waiting for a code or 2FA password, the in-flight [`LoginToken`] or
//! [`PasswordToken`] must be retained in memory so the corresponding
//! "submit" command can complete the flow.

use std::path::PathBuf;

use anyhow::Context;
use grammers_client::types::{LoginToken, PasswordToken};
use serde::{Deserialize, Serialize};

/// Telegram Desktop's publicly-published `api_id`. Used as the default
/// when the agent omits `api_id` in `TelegramLoginStart`. Lives in
/// Telegram Desktop's open-source repo and is in widespread use across
/// third-party MTProto clients. Trade-off: shared credentials can hit
/// `FLOOD_WAIT` under heavy load — supply your own pair only if that
/// happens.
pub const DEFAULT_API_ID: i32 = 2040;

/// Telegram Desktop's publicly-published `api_hash`. See
/// [`DEFAULT_API_ID`] for context.
pub const DEFAULT_API_HASH: &str = "b18441a1ff607e10a989891a5462e627";

/// Ambient configuration resolved once at startup from environment variables.
#[derive(Debug, Clone)]
pub struct SkillEnv {
    /// Absolute path to the directory the supervisor created for our
    /// persistent state.
    pub state_dir: PathBuf,
    /// Absolute path to the session file that persists MTProto auth keys.
    pub session_path: PathBuf,
    /// Event topic to stamp on outbound events. Defaults to `"telegram-user"`.
    pub topic: String,
}

impl SkillEnv {
    /// Resolves ambient configuration from `PUFFER_SKILL_STATE_DIR` and
    /// `PUFFER_SKILL_TOPIC`, falling back to sensible defaults when unset.
    pub fn from_env() -> Self {
        let state_dir = match std::env::var("PUFFER_SKILL_STATE_DIR") {
            Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => PathBuf::from("./state"),
        };
        let session_path = state_dir.join("telegram.session");
        let topic = std::env::var("PUFFER_SKILL_TOPIC")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "telegram-user".to_string());
        Self {
            state_dir,
            session_path,
            topic,
        }
    }

    /// Returns the path used to persist API credentials chosen for the
    /// active session, so reconnects do not need to re-ask the operator.
    pub fn credentials_path(&self) -> PathBuf {
        self.state_dir.join("credentials.json")
    }
}

/// Persisted Telegram API credentials + last-used phone, written after a
/// successful login so the subscriber can reconnect without prompting
/// the agent again.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistedCredentials {
    /// Telegram API id used for the current session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_id: Option<i32>,
    /// Telegram API hash used for the current session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_hash: Option<String>,
    /// Phone number that completed the login. Surfaced for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

impl PersistedCredentials {
    /// Loads persisted credentials. Missing files return an empty value.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read credentials file {}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(&raw)
            .with_context(|| format!("parse credentials file {}", path.display()))
    }

    /// Atomically saves credentials to `path` (tempfile + rename).
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create credentials parent {}", parent.display()))?;
        }
        let tmp = path.with_extension("tmp");
        let body = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
    }
}

/// Resolves the API id to use for a login or reconnect, in priority
/// order: explicit caller value > persisted credentials > env var
/// (`PUFFER_TELEGRAM_API_ID`) > [`DEFAULT_API_ID`].
pub fn resolve_api_id(explicit: Option<i32>, persisted: &PersistedCredentials) -> i32 {
    if let Some(value) = explicit {
        return value;
    }
    if let Some(value) = persisted.api_id {
        return value;
    }
    if let Ok(raw) = std::env::var("PUFFER_TELEGRAM_API_ID") {
        if let Ok(parsed) = raw.parse::<i32>() {
            return parsed;
        }
    }
    DEFAULT_API_ID
}

/// Resolves the API hash to use for a login or reconnect, in the same
/// priority order as [`resolve_api_id`].
pub fn resolve_api_hash(explicit: Option<String>, persisted: &PersistedCredentials) -> String {
    if let Some(value) = explicit {
        return value;
    }
    if let Some(value) = persisted.api_hash.clone() {
        return value;
    }
    if let Ok(value) = std::env::var("PUFFER_TELEGRAM_API_HASH") {
        if !value.is_empty() {
            return value;
        }
    }
    DEFAULT_API_HASH.to_string()
}

/// Transient state carried between login-flow commands.
///
/// Once a login has completed successfully both fields are cleared. While a
/// code request is pending, [`Self::login_token`] is populated. While a 2FA
/// password is pending, [`Self::password_token`] is populated.
#[derive(Default)]
pub struct LoginState {
    /// Token returned by `request_login_code`, consumed by `sign_in`.
    pub login_token: Option<LoginToken>,
    /// Token returned by `sign_in` when 2FA is required, consumed by
    /// `check_password`.
    pub password_token: Option<PasswordToken>,
    /// Phone number currently being signed in with, retained so outbound
    /// events can echo it back to the operator.
    pub phone: Option<String>,
    /// Telegram API id used for the current attempt. Needed because sign-in
    /// happens after `request_login_code` on a previously-connected client.
    pub api_id: Option<i32>,
    /// Telegram API hash used for the current attempt.
    pub api_hash: Option<String>,
}

impl LoginState {
    /// Constructs an empty [`LoginState`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears the login-token / password-token fields after a successful or
    /// terminally-failed login attempt. Credentials (api id/hash/phone) are
    /// preserved so a subsequent retry can reuse them without re-sending
    /// `TelegramLoginStart`.
    pub fn clear_tokens(&mut self) {
        self.login_token = None;
        self.password_token = None;
    }
}
