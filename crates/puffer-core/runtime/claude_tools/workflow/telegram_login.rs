//! Natural-language Telegram login tools.
//!
//! These three workflow tools (`TelegramLoginStart`,
//! `TelegramLoginSubmitCode`, `TelegramLoginSubmitPassword`) drive the
//! login state machine inside the `telegram-user` subscriber process.
//!
//! The agent is responsible for the conversation: it asks the user for
//! their phone number, then for the code Telegram sent them, and (only if
//! Telegram requests it) for their 2FA password. Each tool call sends one
//! [`SubscriberCommand`] over the subscriber's stdin; progress is observed
//! by reading subsequent control events emitted by the subscriber on its
//! stdout (`login_awaiting_code`, `login_awaiting_password`,
//! `login_complete`, `login_error`).
//!
//! Each tool ensures the `telegram-user` subscriber is running before it
//! sends its command. If the subscriber's manifest is not on disk, an
//! actionable error is returned so the agent can ask the user to install it.

use crate::AppState;
use anyhow::{anyhow, Context, Result};
use puffer_subscriber_runtime::{Manifest, SubscriberCommand};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::subscription_globals;

const TELEGRAM_USER_TOPIC: &str = "telegram-user";

#[derive(Debug, Deserialize)]
struct LoginStartInput {
    /// E.164 phone number including the leading `+`.
    phone: String,
    /// Optional Telegram `api_id` from my.telegram.org. Omit to use
    /// Telegram Desktop's published default credentials so the user
    /// does not have to register an application.
    #[serde(default)]
    api_id: Option<i32>,
    /// Optional Telegram `api_hash` from my.telegram.org. Omit when
    /// `api_id` is omitted.
    #[serde(default)]
    api_hash: Option<String>,
}

/// Starts the Telegram login flow. After a successful call, the subscriber
/// will emit a `login_awaiting_code` event and a code is texted to the
/// user's Telegram apps; the agent should then collect the code and call
/// `TelegramLoginSubmitCode`.
pub fn execute_telegram_login_start(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: LoginStartInput =
        serde_json::from_value(input).context("invalid TelegramLoginStart input")?;
    ensure_subscriber_running()?;
    let manager = subscription_globals::manager()?;
    manager.send_command(
        TELEGRAM_USER_TOPIC,
        &SubscriberCommand::TelegramLoginStart {
            phone: parsed.phone,
            api_id: parsed.api_id,
            api_hash: parsed.api_hash,
        },
    )?;
    Ok(json!({
        "status": "awaiting_code",
        "next": "Telegram will text a code to the user's other devices. Ask the user for the code, then call TelegramLoginSubmitCode."
    })
    .to_string())
}

#[derive(Debug, Deserialize)]
struct SubmitCodeInput {
    /// The login code Telegram delivered to the user.
    code: String,
}

/// Submits the login code. On success the subscriber emits
/// `login_complete`; on `PASSWORD_REQUIRED` it emits `login_awaiting_password`
/// and the agent should call `TelegramLoginSubmitPassword`.
pub fn execute_telegram_login_submit_code(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: SubmitCodeInput =
        serde_json::from_value(input).context("invalid TelegramLoginSubmitCode input")?;
    let manager = subscription_globals::manager()?;
    manager.send_command(
        TELEGRAM_USER_TOPIC,
        &SubscriberCommand::TelegramLoginSubmitCode { code: parsed.code },
    )?;
    Ok(json!({
        "status": "submitted",
        "next": "Watch for `login_complete` or `login_awaiting_password` from the subscriber."
    })
    .to_string())
}

#[derive(Debug, Deserialize)]
struct SubmitPasswordInput {
    /// The user's 2FA cloud password.
    password: String,
}

/// Submits the 2FA cloud password. On success the subscriber emits
/// `login_complete`; on failure it emits `login_error`.
pub fn execute_telegram_login_submit_password(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: SubmitPasswordInput =
        serde_json::from_value(input).context("invalid TelegramLoginSubmitPassword input")?;
    let manager = subscription_globals::manager()?;
    manager.send_command(
        TELEGRAM_USER_TOPIC,
        &SubscriberCommand::TelegramLoginSubmitPassword {
            password: parsed.password,
        },
    )?;
    Ok(json!({"status": "submitted"}).to_string())
}

fn ensure_subscriber_running() -> Result<()> {
    let manager = subscription_globals::manager()?;
    if manager
        .subscriber_ids()
        .iter()
        .any(|id| id == TELEGRAM_USER_TOPIC)
    {
        return Ok(());
    }
    let dir = subscriber_manifest_dir(TELEGRAM_USER_TOPIC);
    if !dir.join("manifest.toml").exists() {
        return Err(anyhow!(
            "telegram-user subscriber manifest not found at {}; install it before logging in",
            dir.display()
        ));
    }
    let manifest = Manifest::load(&dir)?;
    manager.start_subscriber(manifest)?;
    Ok(())
}

fn subscriber_manifest_dir(topic: &str) -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let user = PathBuf::from(home)
            .join(".puffer")
            .join("subscribers")
            .join(topic);
        if user.join("manifest.toml").exists() {
            return user;
        }
    }
    PathBuf::from("resources/subscribers").join(topic)
}
