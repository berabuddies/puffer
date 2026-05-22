//! Natural-language Telegram login workflow actions.
//!
//! The consolidated internal `Telegram` tool drives the login state machine
//! inside the `telegram-user` subscriber process with three actions:
//! `login_start`, `login_submit_code`, and `login_submit_password`.
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
use std::time::Duration;

use super::subscription_globals;

const TELEGRAM_USER_TOPIC: &str = "telegram-user";
const TELEGRAM_LOGIN_EVENT_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TelegramAction {
    LoginStart,
    LoginSubmitCode,
    LoginSubmitPassword,
}

#[derive(Debug, Deserialize)]
struct TelegramInput {
    action: TelegramAction,
}

#[derive(Debug, Deserialize)]
struct LoginStartInput {
    /// E.164 phone number including the leading `+`.
    phone: String,
    /// Optional Telegram `api_id` from my.telegram.org. Omit only when
    /// persisted credentials or `PUFFER_TELEGRAM_API_ID` are configured.
    #[serde(default)]
    api_id: Option<i32>,
    /// Optional Telegram `api_hash` from my.telegram.org. Must be provided
    /// together with `api_id` unless credentials are already configured.
    #[serde(default)]
    api_hash: Option<String>,
}

/// Executes the consolidated internal `Telegram` workflow action.
pub fn execute_telegram(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: TelegramInput =
        serde_json::from_value(input.clone()).context("invalid Telegram input")?;
    match parsed.action {
        TelegramAction::LoginStart => execute_telegram_login_start(state, cwd, input),
        TelegramAction::LoginSubmitCode => execute_telegram_login_submit_code(state, cwd, input),
        TelegramAction::LoginSubmitPassword => {
            execute_telegram_login_submit_password(state, cwd, input)
        }
    }
}

/// Starts the Telegram login flow. After a successful call, the subscriber
/// will emit a `login_awaiting_code` event and a code is texted to the
/// user's Telegram apps; the agent should then collect the code and run
/// `telegram login-submit-code`.
pub fn execute_telegram_login_start(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: LoginStartInput =
        serde_json::from_value(input).context("invalid TelegramLoginStart input")?;
    ensure_subscriber_running()?;
    let manager = subscription_globals::manager()?;
    let phone = parsed.phone;
    let command = SubscriberCommand::TelegramLoginStart {
        phone: phone.clone(),
        api_id: parsed.api_id,
        api_hash: parsed.api_hash,
    };
    let event = manager.send_command_and_wait(
        TELEGRAM_USER_TOPIC,
        TELEGRAM_USER_TOPIC,
        &command,
        &["login_awaiting_code", "login_complete", "login_error"],
        TELEGRAM_LOGIN_EVENT_TIMEOUT,
    )?;
    match event.event.kind.as_str() {
        "login_awaiting_code" => Ok(json!({
            "status": "awaiting_code",
            "phone": phone,
            "next": "Telegram accepted the login-code request. Ask the user for the code from Telegram, then run `telegram login-submit-code <code>`."
        })
        .to_string()),
        "login_complete" => Ok(json!({
            "status": "complete",
            "already_authorized": event
                .event
                .payload
                .get("already_authorized")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "payload": event.event.payload,
        })
        .to_string()),
        _ => Err(anyhow!(
            "telegram login failed while requesting code: {}",
            event_error_message(&event.event.payload)
        )),
    }
}

#[derive(Debug, Deserialize)]
struct SubmitCodeInput {
    /// The login code Telegram delivered to the user.
    code: String,
}

/// Submits the login code. On success the subscriber emits
/// `login_complete`; on `PASSWORD_REQUIRED` it emits `login_awaiting_password`
/// and the agent should run `telegram login-submit-password`.
pub fn execute_telegram_login_submit_code(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: SubmitCodeInput =
        serde_json::from_value(input).context("invalid TelegramLoginSubmitCode input")?;
    let manager = subscription_globals::manager()?;
    let command = SubscriberCommand::TelegramLoginSubmitCode { code: parsed.code };
    let event = manager.send_command_and_wait(
        TELEGRAM_USER_TOPIC,
        TELEGRAM_USER_TOPIC,
        &command,
        &["login_complete", "login_awaiting_password", "login_error"],
        TELEGRAM_LOGIN_EVENT_TIMEOUT,
    )?;
    match event.event.kind.as_str() {
        "login_complete" => Ok(json!({
            "status": "complete",
            "payload": event.event.payload,
        })
        .to_string()),
        "login_awaiting_password" => Ok(json!({
            "status": "awaiting_password",
            "next": "Telegram requires the user's 2FA cloud password. Ask the user for it, then run `telegram login-submit-password --password-stdin`.",
            "payload": event.event.payload,
        })
        .to_string()),
        _ => Err(anyhow!(
            "telegram login failed while submitting code: {}",
            event_error_message(&event.event.payload)
        )),
    }
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
    let command = SubscriberCommand::TelegramLoginSubmitPassword {
        password: parsed.password,
    };
    let event = manager.send_command_and_wait(
        TELEGRAM_USER_TOPIC,
        TELEGRAM_USER_TOPIC,
        &command,
        &["login_complete", "login_error"],
        TELEGRAM_LOGIN_EVENT_TIMEOUT,
    )?;
    if event.event.kind == "login_complete" {
        return Ok(json!({
            "status": "complete",
            "payload": event.event.payload,
        })
        .to_string());
    }
    Err(anyhow!(
        "telegram login failed while submitting password: {}",
        event_error_message(&event.event.payload)
    ))
}

fn event_error_message(payload: &Value) -> String {
    payload
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("unknown subscriber error")
        .to_string()
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
