//! Natural-language Telegram login workflow actions.
//!
//! The consolidated internal `Telegram` tool drives the login state machine
//! inside a Telegram personal-account subscriber process with actions for
//! Telegram Desktop import, QR login, and phone-code login. The default
//! connection slug is `telegram-user`; callers can pass `connection_slug` to
//! select the Telegram account connection. `account`, `account_slug`, and
//! `connection` are accepted aliases. Multiple local Telegram accounts can run
//! in parallel, each with its own subscriber id, event topic, and session
//! state directory.
//!
//! The agent is responsible for the conversation: it asks the user for
//! their phone number, then for the code Telegram sent them, and (only if
//! Telegram requests it) for their 2FA password. Each tool call sends one
//! [`SubscriberCommand`] over the subscriber's stdin; progress is observed
//! by reading subsequent control events emitted by the subscriber on its
//! stdout (`login_qr`, `login_awaiting_code`, `login_awaiting_password`,
//! `login_complete`, `login_error`).
//!
//! Each tool ensures the selected subscriber is running before it sends its
//! command. If the base subscriber manifest is not on disk, an actionable
//! error is returned so the agent can ask the user to install it.

use crate::AppState;
use anyhow::{anyhow, Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriber_runtime::{Manifest, StateSpec, SubscriberCommand, TelegramPeerKind};
use puffer_subscriptions::ConnectionRecord;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::subscription_globals;

const TELEGRAM_USER_TOPIC: &str = "telegram-user";
const TELEGRAM_LOGIN_CONNECTOR_SLUG: &str = "telegram-login";
const TELEGRAM_LOGIN_EVENT_TIMEOUT: Duration = Duration::from_secs(45);
const TELEGRAM_PEER_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TelegramAction {
    ImportDesktop,
    ListPeers,
    LoginQr,
    LoginQrWait,
    LoginStart,
    LoginSubmitCode,
    LoginSubmitPassword,
    SearchMessages,
    SearchPeers,
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
        TelegramAction::ImportDesktop => execute_telegram_import_desktop(state, cwd, input),
        TelegramAction::ListPeers => execute_telegram_list_peers(state, cwd, input, false),
        TelegramAction::LoginQr => execute_telegram_login_qr(state, cwd, input),
        TelegramAction::LoginQrWait => execute_telegram_login_qr_wait(state, cwd, input),
        TelegramAction::LoginStart => execute_telegram_login_start(state, cwd, input),
        TelegramAction::LoginSubmitCode => execute_telegram_login_submit_code(state, cwd, input),
        TelegramAction::LoginSubmitPassword => {
            execute_telegram_login_submit_password(state, cwd, input)
        }
        TelegramAction::SearchMessages => execute_telegram_search_messages(state, cwd, input),
        TelegramAction::SearchPeers => execute_telegram_list_peers(state, cwd, input, true),
    }
}

#[derive(Debug, Deserialize)]
struct ImportDesktopInput {
    /// Optional Telegram Desktop `tdata` directory path.
    #[serde(default)]
    path: Option<String>,
    /// Optional Telegram Desktop local passcode.
    #[serde(default)]
    passcode: Option<String>,
    /// Optional zero-based Telegram Desktop account slot.
    #[serde(default)]
    account_index: Option<usize>,
    /// Optional Telegram Desktop tdata key file name.
    #[serde(default)]
    key_file: Option<String>,
}

/// Imports Telegram Desktop `tdata` authentication into the subscriber.
pub fn execute_telegram_import_desktop(
    _state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input)?;
    let parsed: ImportDesktopInput =
        serde_json::from_value(input).context("invalid Telegram import_desktop input")?;
    ensure_telegram_connection_subscriber(cwd, &connection_slug)?;
    let manager = subscription_globals::manager()?;
    let command = SubscriberCommand::TelegramImportTdata {
        path: parsed.path,
        passcode: parsed.passcode,
        account_index: parsed.account_index,
        key_file: parsed.key_file,
    };
    let event = manager.send_command_and_wait(
        &connection_slug,
        &connection_slug,
        &command,
        &["login_complete", "login_error"],
        TELEGRAM_LOGIN_EVENT_TIMEOUT,
    )?;
    if event.event.kind == "login_complete" {
        let (connection, created) =
            ensure_telegram_connection_record(&connection_slug, &event.event.payload)?;
        return Ok(json!({
            "status": "complete",
            "connection_slug": connection_slug,
            "registered_connection": created,
            "connection": connection,
            "imported": true,
            "payload": event.event.payload,
            "next": "Use this connection_slug in WorkflowCreate or ConnectorAct."
        })
        .to_string());
    }
    Err(anyhow!(
        "telegram Desktop import failed: {}",
        event_error_message(&event.event.payload)
    ))
}

#[derive(Debug, Deserialize)]
struct ListPeersInput {
    /// Optional case-insensitive query matched against title, username, or id.
    #[serde(default)]
    query: Option<String>,
    /// Optional peer kind filter.
    #[serde(default)]
    peer_kind: Option<TelegramPeerKind>,
    /// Optional maximum number of peers to return.
    #[serde(default)]
    limit: Option<usize>,
}

fn execute_telegram_list_peers(
    _state: &mut AppState,
    cwd: &Path,
    input: Value,
    require_query: bool,
) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input)?;
    let parsed: ListPeersInput =
        serde_json::from_value(input).context("invalid Telegram peer-list input")?;
    let missing_query = parsed
        .query
        .as_deref()
        .map(str::trim)
        .map(str::is_empty)
        .unwrap_or(true);
    if require_query && missing_query {
        return Err(anyhow!("telegram search_peers requires a non-empty query"));
    }
    ensure_telegram_connection_subscriber(cwd, &connection_slug)?;
    let manager = subscription_globals::manager()?;
    let command = SubscriberCommand::TelegramListPeers {
        query: parsed.query,
        peer_kind: parsed.peer_kind,
        limit: parsed.limit,
    };
    let event = manager.send_command_and_wait(
        &connection_slug,
        &connection_slug,
        &command,
        &["peer_list", "peer_list_error", "login_error"],
        TELEGRAM_PEER_EVENT_TIMEOUT,
    )?;
    if event.event.kind == "peer_list" {
        return Ok(json!({
            "status": "complete",
            "payload": event.event.payload,
            "next": "Use the returned peer `id` string as the Telegram send target or workflow filter chat id."
        })
        .to_string());
    }
    Err(anyhow!(
        "telegram peer lookup failed: {}",
        event_error_message(&event.event.payload)
    ))
}

#[derive(Debug, Deserialize)]
struct SearchMessagesInput {
    /// Telegram peer id from search-peers, or a public @username.
    peer: String,
    /// Message text query.
    query: String,
    /// Optional maximum number of message matches to return.
    #[serde(default)]
    limit: Option<usize>,
    /// Optional surrounding message count before and after each match.
    #[serde(default)]
    context: Option<usize>,
    /// Return compact message search payloads for LLM consumption.
    #[serde(default, alias = "succint")]
    succinct: bool,
}

fn execute_telegram_search_messages(
    _state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input)?;
    let parsed: SearchMessagesInput =
        serde_json::from_value(input).context("invalid Telegram message-search input")?;
    if parsed.peer.trim().is_empty() {
        return Err(anyhow!(
            "telegram search_messages requires a non-empty peer"
        ));
    }
    if parsed.query.trim().is_empty() {
        return Err(anyhow!(
            "telegram search_messages requires a non-empty query"
        ));
    }
    ensure_telegram_connection_subscriber(cwd, &connection_slug)?;
    let manager = subscription_globals::manager()?;
    let succinct = parsed.succinct;
    let command = SubscriberCommand::TelegramSearchMessages {
        peer: parsed.peer,
        query: parsed.query,
        limit: parsed.limit,
        context: parsed.context,
        succinct,
    };
    let event = manager.send_command_and_wait(
        &connection_slug,
        &connection_slug,
        &command,
        &["message_search", "message_search_error", "login_error"],
        TELEGRAM_PEER_EVENT_TIMEOUT,
    )?;
    if event.event.kind == "message_search" {
        if succinct {
            return Ok(format_succinct_message_search(&event.event.payload));
        }
        return Ok(json!({
            "status": "complete",
            "payload": event.event.payload,
            "next": "Inspect returned `results[].context`; each match is marked with `is_match: true`."
        })
        .to_string());
    }
    Err(anyhow!(
        "telegram message search failed: {}",
        event_error_message(&event.event.payload)
    ))
}

fn format_succinct_message_search(payload: &Value) -> String {
    let chat = payload.get("chat").unwrap_or(&Value::Null);
    let chat_title = value_string(chat, "title")
        .or_else(|| value_string(chat, "id"))
        .unwrap_or_else(|| "unknown chat".to_string());
    let chat_handle = value_string(chat, "handle")
        .filter(|value| !value.is_empty())
        .map(|value| format!(" ({value})"))
        .unwrap_or_default();
    let query = value_string(payload, "query").unwrap_or_else(|| "<empty query>".to_string());
    let count = payload.get("count").and_then(Value::as_u64).unwrap_or(0);
    let limit_reached = payload
        .get("limit_reached")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut output = String::new();
    let _ = writeln!(
        output,
        "Telegram search: \"{}\" in {}{}",
        one_line(&query),
        chat_title,
        chat_handle
    );
    let _ = writeln!(
        output,
        "{} match{} returned{}",
        count,
        if count == 1 { "" } else { "es" },
        if limit_reached {
            " (limit reached)"
        } else {
            ""
        }
    );

    let Some(results) = payload.get("results").and_then(Value::as_array) else {
        return trim_trailing_newline(output);
    };
    if results.is_empty() {
        return trim_trailing_newline(output);
    }

    for (index, result) in results.iter().enumerate() {
        let _ = writeln!(output);
        if results.len() > 1 {
            let _ = writeln!(output, "Match {}:", index + 1);
        } else {
            let _ = writeln!(output, "Match:");
        }

        match result.get("context").and_then(Value::as_array) {
            Some(context) if !context.is_empty() => {
                for message in context {
                    write_succinct_message_line(&mut output, message);
                }
            }
            _ => {
                if let Some(message) = result
                    .get("match")
                    .or_else(|| result.get("message"))
                    .filter(|value| value.is_object())
                {
                    write_succinct_message_line(&mut output, message);
                }
            }
        }

        if let Some(error) = value_string(result, "context_error").filter(|value| !value.is_empty())
        {
            let _ = writeln!(output, "context warning: {}", one_line(&error));
        }
    }

    trim_trailing_newline(output)
}

fn write_succinct_message_line(output: &mut String, message: &Value) {
    let offset = message.get("offset").and_then(Value::as_i64);
    let offset = match offset {
        Some(value) if value > 0 => format!("+{value}"),
        Some(value) => value.to_string(),
        None if message
            .get("is_match")
            .and_then(Value::as_bool)
            .unwrap_or(false) =>
        {
            "0".to_string()
        }
        None => "?".to_string(),
    };
    let from = value_string(message, "from")
        .or_else(|| {
            message
                .get("sender")
                .and_then(|sender| value_string(sender, "title"))
        })
        .unwrap_or_else(|| "unknown".to_string());
    let text = value_string(message, "text")
        .map(|value| one_line(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "[non-text message]".to_string());
    let _ = writeln!(output, "{offset:>3} {from}: {text}");
}

fn value_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn one_line(value: &str) -> String {
    value.replace('\r', "\\r").replace('\n', "\\n")
}

fn trim_trailing_newline(mut output: String) -> String {
    while output.ends_with('\n') {
        output.pop();
    }
    output
}

#[derive(Debug, Deserialize)]
struct LoginQrInput {
    /// Optional Telegram `api_id` from my.telegram.org.
    #[serde(default)]
    api_id: Option<i32>,
    /// Optional Telegram `api_hash` from my.telegram.org.
    #[serde(default)]
    api_hash: Option<String>,
}

/// Starts Telegram QR login and returns a short-lived `tg://login` URL.
pub fn execute_telegram_login_qr(
    _state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input)?;
    let parsed: LoginQrInput =
        serde_json::from_value(input).context("invalid Telegram login_qr input")?;
    ensure_telegram_connection_subscriber(cwd, &connection_slug)?;
    let manager = subscription_globals::manager()?;
    let command = SubscriberCommand::TelegramQrLoginStart {
        api_id: parsed.api_id,
        api_hash: parsed.api_hash,
    };
    let event = manager.send_command_and_wait(
        &connection_slug,
        &connection_slug,
        &command,
        &["login_qr", "login_complete", "login_error"],
        TELEGRAM_LOGIN_EVENT_TIMEOUT,
    )?;
    match event.event.kind.as_str() {
        "login_qr" => Ok(json!({
            "status": "qr_pending",
            "connection_slug": connection_slug,
            "payload": event.event.payload,
            "next": "Show the returned URL to the user. After they approve it from a logged-in Telegram app, run `telegram --connection <connection_slug> login-qr-wait`."
        })
        .to_string()),
        "login_complete" => {
            let (connection, created) =
                ensure_telegram_connection_record(&connection_slug, &event.event.payload)?;
            Ok(json!({
                "status": "complete",
                "connection_slug": connection_slug,
                "registered_connection": created,
                "connection": connection,
                "payload": event.event.payload,
            })
            .to_string())
        }
        _ => Err(anyhow!(
            "telegram QR login failed while creating token: {}",
            event_error_message(&event.event.payload)
        )),
    }
}

#[derive(Debug, Deserialize)]
struct LoginQrWaitInput {
    /// Optional wait timeout in seconds.
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

/// Waits for Telegram QR login approval and persists the authorized session.
pub fn execute_telegram_login_qr_wait(
    _state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input)?;
    let parsed: LoginQrWaitInput =
        serde_json::from_value(input).context("invalid Telegram login_qr_wait input")?;
    ensure_telegram_connection_subscriber(cwd, &connection_slug)?;
    let manager = subscription_globals::manager()?;
    let wait_seconds = parsed.timeout_seconds.unwrap_or(120);
    let command = SubscriberCommand::TelegramQrLoginWait {
        timeout_seconds: parsed.timeout_seconds,
    };
    let event = manager.send_command_and_wait(
        &connection_slug,
        &connection_slug,
        &command,
        &["login_complete", "login_qr", "login_error"],
        Duration::from_secs(wait_seconds.saturating_add(10)),
    )?;
    match event.event.kind.as_str() {
        "login_complete" => {
            let (connection, created) =
                ensure_telegram_connection_record(&connection_slug, &event.event.payload)?;
            Ok(json!({
                "status": "complete",
                "connection_slug": connection_slug,
                "registered_connection": created,
                "connection": connection,
                "payload": event.event.payload,
            })
            .to_string())
        }
        "login_qr" => Ok(json!({
            "status": "qr_pending",
            "connection_slug": connection_slug,
            "payload": event.event.payload,
            "next": "The previous QR token expired before approval. Show the refreshed URL to the user, then run `telegram --connection <connection_slug> login-qr-wait` again."
        })
        .to_string()),
        _ => Err(anyhow!(
            "telegram QR login failed while waiting for approval: {}",
            event_error_message(&event.event.payload)
        )),
    }
}

/// Starts the Telegram login flow. After a successful call, the subscriber
/// will emit a `login_awaiting_code` event and a code is texted to the
/// user's Telegram apps; the agent should then collect the code and run
/// `telegram login-submit-code`.
pub fn execute_telegram_login_start(
    _state: &mut AppState,
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input)?;
    let parsed: LoginStartInput =
        serde_json::from_value(input).context("invalid TelegramLoginStart input")?;
    ensure_telegram_connection_subscriber(cwd, &connection_slug)?;
    let manager = subscription_globals::manager()?;
    let phone = parsed.phone;
    let command = SubscriberCommand::TelegramLoginStart {
        phone: phone.clone(),
        api_id: parsed.api_id,
        api_hash: parsed.api_hash,
    };
    let event = manager.send_command_and_wait(
        &connection_slug,
        &connection_slug,
        &command,
        &["login_awaiting_code", "login_complete", "login_error"],
        TELEGRAM_LOGIN_EVENT_TIMEOUT,
    )?;
    match event.event.kind.as_str() {
        "login_awaiting_code" => Ok(json!({
            "status": "awaiting_code",
            "connection_slug": connection_slug,
            "phone": phone,
            "next": "Telegram accepted the login-code request. Ask the user for the code from Telegram, then run `telegram --connection <connection_slug> login-submit-code <code>`."
        })
        .to_string()),
        "login_complete" => {
            let (connection, created) =
                ensure_telegram_connection_record(&connection_slug, &event.event.payload)?;
            Ok(json!({
                "status": "complete",
                "connection_slug": connection_slug,
                "registered_connection": created,
                "connection": connection,
                "already_authorized": event
                    .event
                    .payload
                    .get("already_authorized")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                "payload": event.event.payload,
            })
            .to_string())
        }
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
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input)?;
    let parsed: SubmitCodeInput =
        serde_json::from_value(input).context("invalid TelegramLoginSubmitCode input")?;
    ensure_telegram_connection_subscriber(cwd, &connection_slug)?;
    let manager = subscription_globals::manager()?;
    let command = SubscriberCommand::TelegramLoginSubmitCode { code: parsed.code };
    let event = manager.send_command_and_wait(
        &connection_slug,
        &connection_slug,
        &command,
        &["login_complete", "login_awaiting_password", "login_error"],
        TELEGRAM_LOGIN_EVENT_TIMEOUT,
    )?;
    match event.event.kind.as_str() {
        "login_complete" => {
            let (connection, created) =
                ensure_telegram_connection_record(&connection_slug, &event.event.payload)?;
            Ok(json!({
                "status": "complete",
                "connection_slug": connection_slug,
                "registered_connection": created,
                "connection": connection,
                "payload": event.event.payload,
            })
            .to_string())
        }
        "login_awaiting_password" => Ok(json!({
            "status": "awaiting_password",
            "connection_slug": connection_slug,
            "next": "Telegram requires the user's 2FA cloud password. Ask the user for it, then run `telegram --connection <connection_slug> login-submit-password --password-stdin`.",
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
    cwd: &Path,
    input: Value,
) -> Result<String> {
    let connection_slug = connection_slug_from_input(&input)?;
    let parsed: SubmitPasswordInput =
        serde_json::from_value(input).context("invalid TelegramLoginSubmitPassword input")?;
    ensure_telegram_connection_subscriber(cwd, &connection_slug)?;
    let manager = subscription_globals::manager()?;
    let command = SubscriberCommand::TelegramLoginSubmitPassword {
        password: parsed.password,
    };
    let event = manager.send_command_and_wait(
        &connection_slug,
        &connection_slug,
        &command,
        &["login_complete", "login_error"],
        TELEGRAM_LOGIN_EVENT_TIMEOUT,
    )?;
    if event.event.kind == "login_complete" {
        let (connection, created) =
            ensure_telegram_connection_record(&connection_slug, &event.event.payload)?;
        return Ok(json!({
            "status": "complete",
            "connection_slug": connection_slug,
            "registered_connection": created,
            "connection": connection,
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

fn connection_slug_from_input(input: &Value) -> Result<String> {
    let slug = input
        .get("connection_slug")
        .or_else(|| input.get("account_slug"))
        .or_else(|| input.get("connection"))
        .or_else(|| input.get("account"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
        .unwrap_or(TELEGRAM_USER_TOPIC);
    let slug = if slug == TELEGRAM_LOGIN_CONNECTOR_SLUG {
        TELEGRAM_USER_TOPIC
    } else {
        slug
    };
    validate_connection_slug(slug)?;
    Ok(slug.to_string())
}

fn validate_connection_slug(slug: &str) -> Result<()> {
    if slug.is_empty()
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        anyhow::bail!("Telegram connection_slug must be non-empty kebab-case ASCII");
    }
    Ok(())
}

/// Ensures the Telegram subscriber for `connection_slug` is running.
pub(crate) fn ensure_telegram_connection_subscriber(
    cwd: &Path,
    connection_slug: &str,
) -> Result<()> {
    validate_connection_slug(connection_slug)?;
    let manager = subscription_globals::manager()?;
    if manager
        .subscriber_ids()
        .iter()
        .any(|id| id == connection_slug)
    {
        return Ok(());
    }
    let paths = ConfigPaths::discover(cwd);
    let manifest = telegram_connection_manifest(&paths, connection_slug)?;
    manager.start_subscriber(manifest)?;
    Ok(())
}

fn telegram_connection_manifest(paths: &ConfigPaths, connection_slug: &str) -> Result<Manifest> {
    let dir = find_subscriber_manifest(paths, TELEGRAM_USER_TOPIC).ok_or_else(|| {
        anyhow!("telegram-user subscriber manifest not found; install it before logging in")
    })?;
    let mut manifest = Manifest::load(&dir)?;
    if connection_slug != TELEGRAM_USER_TOPIC {
        manifest.spec.id = connection_slug.to_string();
        manifest.spec.topic = Some(connection_slug.to_string());
        manifest.spec.display_name = Some(format!("Telegram ({connection_slug})"));
        manifest.spec.state = Some(StateSpec {
            dir: paths
                .user_config_dir
                .join("telegram-accounts")
                .join(connection_slug)
                .to_string_lossy()
                .to_string(),
        });
    }
    Ok(manifest)
}

fn find_subscriber_manifest(paths: &ConfigPaths, topic: &str) -> Option<PathBuf> {
    let workspace = paths.workspace_config_dir.join("subscribers").join(topic);
    if workspace.join("manifest.toml").exists() {
        return Some(workspace);
    }
    let user = paths.user_config_dir.join("subscribers").join(topic);
    if user.join("manifest.toml").exists() {
        return Some(user);
    }
    let bundled = paths.builtin_resources_dir.join("subscribers").join(topic);
    if bundled.join("manifest.toml").exists() {
        return Some(bundled);
    }
    None
}

fn ensure_telegram_connection_record(
    connection_slug: &str,
    payload: &Value,
) -> Result<(ConnectionRecord, bool)> {
    let manager = subscription_globals::manager()?;
    if let Some(connection) = manager.connection_store().get(connection_slug) {
        if connection.connector_slug != TELEGRAM_LOGIN_CONNECTOR_SLUG {
            anyhow::bail!(
                "connection `{connection_slug}` already exists for connector `{}`",
                connection.connector_slug
            );
        }
        return Ok((connection, false));
    }
    let description = telegram_connection_description(payload);
    let record = ConnectionRecord::authenticated(
        connection_slug,
        TELEGRAM_LOGIN_CONNECTOR_SLUG,
        description,
    );
    manager.connection_store().create(record.clone())?;
    manager.refresh_connection_consumers()?;
    Ok((record, true))
}

fn telegram_connection_description(payload: &Value) -> String {
    let first_name = payload
        .get("first_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let user_id = payload.get("user_id").and_then(|value| {
        value
            .as_i64()
            .map(|id| id.to_string())
            .or_else(|| value.as_str().map(ToString::to_string))
    });
    match (first_name, user_id) {
        (Some(name), Some(id)) => format!("Telegram {name} ({id})"),
        (Some(name), None) => format!("Telegram {name}"),
        (None, Some(id)) => format!("Telegram account {id}"),
        (None, None) => "Telegram personal account".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        connection_slug_from_input, format_succinct_message_search, telegram_connection_description,
    };
    use serde_json::json;

    #[test]
    fn connection_slug_defaults_and_validates() {
        assert_eq!(
            connection_slug_from_input(&json!({})).unwrap(),
            "telegram-user"
        );
        assert_eq!(
            connection_slug_from_input(&json!({"connection_slug": "tg-alt"})).unwrap(),
            "tg-alt"
        );
        assert_eq!(
            connection_slug_from_input(&json!({"account": "tg-alt"})).unwrap(),
            "tg-alt"
        );
        assert_eq!(
            connection_slug_from_input(&json!({"connection_slug": "telegram-login"})).unwrap(),
            "telegram-user"
        );
        assert!(connection_slug_from_input(&json!({"connection_slug": "TG Alt"})).is_err());
    }

    #[test]
    fn telegram_connection_description_uses_login_payload() {
        let payload = json!({
            "user_id": 12345,
            "first_name": "Tony"
        });

        assert_eq!(
            telegram_connection_description(&payload),
            "Telegram Tony (12345)"
        );
    }

    #[test]
    fn succinct_message_search_formats_plain_text() {
        let payload = json!({
            "query": "karen",
            "count": 1,
            "limit_reached": true,
            "chat": {
                "id": "477843728",
                "kind": "user",
                "title": "Tony",
                "handle": "@tony"
            },
            "results": [{
                "match": {
                    "id": 3,
                    "offset": null,
                    "from": "Tony",
                    "outgoing": false,
                    "is_match": true,
                    "text": "karen"
                },
                "context": [
                    {
                        "id": 1,
                        "offset": -1,
                        "from": "Me",
                        "outgoing": true,
                        "is_match": false,
                        "text": "before"
                    },
                    {
                        "id": 3,
                        "offset": 0,
                        "from": "Tony",
                        "outgoing": false,
                        "is_match": true,
                        "text": "karen"
                    },
                    {
                        "id": 4,
                        "offset": 1,
                        "from": "Me",
                        "outgoing": true,
                        "is_match": false,
                        "text": "after"
                    }
                ],
                "context_error": null
            }]
        });

        let formatted = format_succinct_message_search(&payload);

        assert_eq!(
            formatted,
            concat!(
                "Telegram search: \"karen\" in Tony (@tony)\n",
                "1 match returned (limit reached)\n",
                "\n",
                "Match:\n",
                " -1 Me: before\n",
                "  0 Tony: karen\n",
                " +1 Me: after"
            )
        );
        assert!(!formatted.contains('{'));
        assert!(!formatted.contains("\"context\""));
    }
}
