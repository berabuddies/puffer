//! `puffer connect` — connector authentication without a parent REPL.
//!
//! GUI hosts (Tauri/Electron) need to authenticate Telegram personal
//! accounts and configure IMAP/SMTP credentials from outside an
//! interactive puffer agent session. The TUI does this via
//! `/connect telegram-login …` which routes through the internal-tool
//! permission broker — that path requires a long-running parent process
//! and is not reachable from a one-shot CLI invocation.
//!
//! This module exposes the same primitives as a top-level subcommand by
//! calling the subscriber crates' public APIs directly. Telegram runs as
//! a JSON-stdio REPL because grammers' `LoginToken` and `PasswordToken`
//! are opaque in-memory values that must survive between phone/code/2FA
//! steps. Email is a one-shot config-file write.
//!
//! Wire protocol for Telegram (newline-delimited JSON on stdin/stdout):
//!
//! Stdin:
//!   {"action":"login_qr","api_id":<i32?>,"api_hash":<str?>}
//!   {"action":"login_qr_wait","timeout_seconds":<u64?>}
//!   {"action":"login_start","phone":"+1...","api_id":<i32?>,"api_hash":<str?>}
//!   {"action":"submit_code","code":"12345"}
//!   {"action":"submit_password","password":"..."}
//!   {"action":"exit"}
//!
//! Stdout: control events emitted by `puffer-subscriber-telegram-user`
//! (kinds `login_qr`, `login_awaiting_code`, `login_awaiting_password`,
//! `login_complete`, `login_error`) plus any malformed-input errors as
//! `kind:"connect_error"` events.

use crate::cli_args::{ConnectCommand, ConnectEmailCommand};
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriber_email::{save_email_config, EmailConfig};
use puffer_subscriber_runtime::Event;
use puffer_subscriber_telegram_user::{
    login_start, login_submit_code, login_submit_password, qr_login_start, qr_login_wait, Client,
    CodeSubmitOutcome, LoginState, QrLoginOutcome, QrLoginState, SkillEnv,
};
use puffer_subscriptions::{ConnectionRecord, ConnectionStore};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

const TELEGRAM_CONNECTOR_SLUG: &str = "telegram-login";
const EMAIL_CONNECTOR_SLUG: &str = "email";
const DEFAULT_EMAIL_CONNECTION_SLUG: &str = "email";

/// Dispatches a `puffer connect …` invocation to the matching handler.
pub(crate) fn run_connect_command(paths: &ConfigPaths, command: ConnectCommand) -> Result<()> {
    match command {
        ConnectCommand::Telegram { connection_slug } => run_telegram_repl(paths, &connection_slug),
        ConnectCommand::Email { command } => run_email_command(paths, command),
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum TelegramAction {
    LoginQr {
        #[serde(default)]
        api_id: Option<i32>,
        #[serde(default)]
        api_hash: Option<String>,
    },
    LoginQrWait {
        #[serde(default)]
        timeout_seconds: Option<u64>,
    },
    LoginStart {
        phone: String,
        #[serde(default)]
        api_id: Option<i32>,
        #[serde(default)]
        api_hash: Option<String>,
    },
    SubmitCode {
        code: String,
    },
    SubmitPassword {
        password: String,
    },
    Exit,
}

fn run_telegram_repl(paths: &ConfigPaths, connection_slug: &str) -> Result<()> {
    let state_dir = telegram_state_dir(paths, connection_slug);
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("create telegram state dir {}", state_dir.display()))?;
    let env = SkillEnv {
        // Login writes to a STAGING session, never directly to the live session
        // the resident subscriber loads — see telegram_login_staging_path /
        // promote_staged_session (agentenv/monorepo#551).
        session_path: telegram_login_staging_path(&state_dir),
        state_dir,
        topic: connection_slug.to_string(),
        workspace_config_dir: Some(paths.workspace_config_dir.clone()),
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("puffer-connect-telegram")
        .build()
        .context("failed to build connect runtime")?;
    runtime.block_on(telegram_repl_loop(env, paths.clone()))
}

/// The durable session file the resident `__subscriber telegram-user` process
/// loads on startup/resume. `connect telegram` must NOT overwrite this with a
/// half-finished login: writing a pre-auth (or abandoned) session here makes
/// the subscriber resume as `not_signed_in` and flip the connection to
/// `Degraded` (agentenv/monorepo#551).
fn telegram_live_session_path(state_dir: &Path) -> PathBuf {
    state_dir.join("telegram.session")
}

/// Staging session the connect/login flow writes to. It is promoted onto the
/// live session path only once a login fully completes, so an abandoned login
/// (e.g. the user never enters the code, or SMS never arrives) leaves the
/// subscriber's authorized session untouched.
fn telegram_login_staging_path(state_dir: &Path) -> PathBuf {
    state_dir.join("login-staging.session")
}

/// Atomically promotes the staged login session onto the live session path
/// after a successful login. No-op if nothing was staged.
fn promote_staged_session(staging: &Path, live: &Path) -> Result<()> {
    if !staging.exists() {
        return Ok(());
    }
    if let Some(parent) = live.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create session parent dir {}", parent.display()))?;
    }
    std::fs::rename(staging, live).with_context(|| {
        format!(
            "promote session {} -> {}",
            staging.display(),
            live.display()
        )
    })
}

/// Finalizes a completed login: promote the staged session onto the live path
/// (so the subscriber picks up the freshly authorized session) and register
/// the connection record.
fn finalize_login(env: &SkillEnv, paths: &ConfigPaths) -> Result<()> {
    promote_staged_session(
        &env.session_path,
        &telegram_live_session_path(&env.state_dir),
    )?;
    register_telegram_connection(paths, &env.topic)
}

async fn telegram_repl_loop(env: SkillEnv, paths: ConfigPaths) -> Result<()> {
    let mut state = LoginState::new();
    let mut qr_state: Option<QrLoginState> = None;
    let mut client: Option<Client> = None;
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .context("read telegram connect command")?;
        if bytes == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: TelegramAction = match serde_json::from_str(trimmed) {
            Ok(parsed) => parsed,
            Err(err) => {
                emit_local_error(&env.topic, &format!("invalid command JSON: {err}"))?;
                continue;
            }
        };
        match parsed {
            TelegramAction::LoginQr { api_id, api_hash } => {
                client = None;
                match qr_login_start(&env, &mut state, &mut qr_state, api_id, api_hash).await {
                    Ok(QrLoginOutcome::Pending) => {}
                    Ok(QrLoginOutcome::AwaitingPassword(c)) => client = Some(c),
                    Ok(QrLoginOutcome::Complete(_c)) => {
                        finalize_login(&env, &paths)?;
                        break;
                    }
                    Err(err) => emit_local_error(&env.topic, &format!("login_qr error: {err}"))?,
                }
            }
            TelegramAction::LoginQrWait { timeout_seconds } => {
                match qr_login_wait(&env, &mut state, &mut qr_state, timeout_seconds).await {
                    Ok(QrLoginOutcome::Pending) => {}
                    Ok(QrLoginOutcome::AwaitingPassword(c)) => client = Some(c),
                    Ok(QrLoginOutcome::Complete(_c)) => {
                        finalize_login(&env, &paths)?;
                        break;
                    }
                    Err(err) => {
                        emit_local_error(&env.topic, &format!("login_qr_wait error: {err}"))?
                    }
                }
            }
            TelegramAction::LoginStart {
                phone,
                api_id,
                api_hash,
            } => {
                qr_state = None;
                match login_start(&env, &mut state, phone, api_id, api_hash).await {
                    Ok(Some(c)) => client = Some(c),
                    Ok(None) => client = None,
                    Err(err) => emit_local_error(&env.topic, &format!("login_start error: {err}"))?,
                }
            }
            TelegramAction::SubmitCode { code } => {
                let Some(c) = client.as_ref() else {
                    emit_local_error(&env.topic, "no active client; send login_start first")?;
                    continue;
                };
                match login_submit_code(&env, &mut state, c, code).await {
                    Ok(CodeSubmitOutcome::Complete) => {
                        finalize_login(&env, &paths)?;
                        break;
                    }
                    // The subscriber already emitted `login_awaiting_password`
                    // / `login_error` / a retry hint on stdout for these
                    // outcomes — keep the REPL alive so the caller can send
                    // the next command (submit_password, or another
                    // submit_code retry).
                    Ok(CodeSubmitOutcome::AwaitingPassword)
                    | Ok(CodeSubmitOutcome::Failed)
                    | Ok(CodeSubmitOutcome::RetryableTransportError { .. }) => {}
                    Err(err) => emit_local_error(&env.topic, &format!("submit_code error: {err}"))?,
                }
            }
            TelegramAction::SubmitPassword { password } => {
                let Some(c) = client.as_ref() else {
                    emit_local_error(&env.topic, "no active client; send login_start first")?;
                    continue;
                };
                match login_submit_password(&env, &mut state, c, password).await {
                    Ok(true) => {
                        finalize_login(&env, &paths)?;
                        break;
                    }
                    // Subscriber emitted the matching control event already.
                    Ok(false) => {}
                    Err(err) => {
                        emit_local_error(&env.topic, &format!("submit_password error: {err}"))?
                    }
                }
            }
            TelegramAction::Exit => break,
        }
    }
    // Abandoned/partial logins leave a staging session behind; drop it so it
    // can't be mistaken for a real session later. A completed login already
    // renamed staging onto the live path, so this is a no-op in that case.
    let _ = std::fs::remove_file(&env.session_path);
    Ok(())
}

fn emit_local_error(topic: &str, msg: &str) -> Result<()> {
    let event = Event {
        topic: topic.to_string(),
        kind: "connect_error".to_string(),
        control: true,
        dedup_key: None,
        text: String::new(),
        payload: json!({ "error": msg }),
    };
    let rendered = serde_json::to_string(&event).context("serialize connect error event")?;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle
        .write_all(rendered.as_bytes())
        .context("write connect error event")?;
    handle.write_all(b"\n").context("write event newline")?;
    handle.flush().context("flush connect error event")?;
    Ok(())
}

fn run_email_command(paths: &ConfigPaths, command: ConnectEmailCommand) -> Result<()> {
    match command {
        ConnectEmailCommand::Configure {
            imap_host,
            imap_port,
            smtp_host,
            smtp_port,
            username,
            password,
            password_stdin,
            from_address,
            allowed_senders,
        } => {
            let password = if password_stdin {
                read_secret_from_stdin("Email password")?
            } else {
                password.context("--password or --password-stdin is required")?
            };
            let config = EmailConfig::from_command_fields(
                imap_host,
                imap_port,
                smtp_host,
                smtp_port,
                username,
                password,
                from_address,
                allowed_senders,
            );
            let state_dir = email_state_dir(paths);
            std::fs::create_dir_all(&state_dir)
                .with_context(|| format!("create email state dir {}", state_dir.display()))?;

            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to build connect runtime")?;
            runtime
                .block_on(save_email_config(&state_dir, &config))
                .context("write email config")?;
            register_email_connection(paths, DEFAULT_EMAIL_CONNECTION_SLUG)?;

            let response: Value = json!({
                "status": "configured",
                "state_dir": state_dir.to_string_lossy(),
                "connection_slug": DEFAULT_EMAIL_CONNECTION_SLUG,
            });
            println!("{response}");
            Ok(())
        }
    }
}

fn email_state_dir(paths: &ConfigPaths) -> PathBuf {
    paths
        .user_config_dir
        .join("subscribers")
        .join("email")
        .join("state")
}

fn read_secret_from_stdin(label: &str) -> Result<String> {
    let mut value = String::new();
    std::io::stdin()
        .read_line(&mut value)
        .with_context(|| format!("read {label} from stdin"))?;
    Ok(value.trim().to_string())
}

fn connections_store_path(paths: &ConfigPaths) -> PathBuf {
    paths.user_config_dir.join("connections.json")
}

fn telegram_state_dir(paths: &ConfigPaths, connection_slug: &str) -> PathBuf {
    paths
        .user_config_dir
        .join("telegram-accounts")
        .join(connection_slug)
}

fn register_connection(
    paths: &ConfigPaths,
    slug: &str,
    connector_slug: &str,
    description: &str,
) -> Result<()> {
    let path = connections_store_path(paths);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create connections store parent {}", parent.display()))?;
    }
    let store = ConnectionStore::load(&path).with_context(|| format!("load {}", path.display()))?;
    if let Some(existing) = store.get(slug) {
        if existing.connector_slug == connector_slug {
            return Ok(());
        }
        anyhow::bail!(
            "connection `{slug}` already exists for connector `{}`",
            existing.connector_slug
        );
    }
    let record = ConnectionRecord::authenticated(slug, connector_slug, description);
    store
        .create(record)
        .with_context(|| format!("register connection `{slug}` in {}", path.display()))?;
    Ok(())
}

fn register_telegram_connection(paths: &ConfigPaths, slug: &str) -> Result<()> {
    register_connection(
        paths,
        slug,
        TELEGRAM_CONNECTOR_SLUG,
        "Telegram personal account",
    )
}

fn register_email_connection(paths: &ConfigPaths, slug: &str) -> Result<()> {
    register_connection(paths, slug, EMAIL_CONNECTOR_SLUG, "Email (IMAP/SMTP)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_staging_path_differs_from_live_session() {
        let dir = Path::new("/tmp/telegram-accounts/telegram-user");
        assert_ne!(
            telegram_login_staging_path(dir),
            telegram_live_session_path(dir),
            "login must stage to a separate file, never the live session (#551)"
        );
    }

    #[test]
    fn completed_login_promotes_staging_onto_live() {
        let temp = tempfile::tempdir().unwrap();
        let live = telegram_live_session_path(temp.path());
        let staging = telegram_login_staging_path(temp.path());
        std::fs::write(&live, b"stale-or-absent").unwrap();
        std::fs::write(&staging, b"freshly-authorized-session").unwrap();

        promote_staged_session(&staging, &live).unwrap();

        assert_eq!(std::fs::read(&live).unwrap(), b"freshly-authorized-session");
        assert!(
            !staging.exists(),
            "staging is renamed onto live, not left behind"
        );
    }

    #[test]
    fn promote_is_noop_when_nothing_staged() {
        // #551: an abandoned login never stages a completed session, so a later
        // promote must leave the subscriber's authorized live session intact.
        let temp = tempfile::tempdir().unwrap();
        let live = telegram_live_session_path(temp.path());
        std::fs::write(&live, b"authorized-subscriber-session").unwrap();
        let staging = telegram_login_staging_path(temp.path());

        promote_staged_session(&staging, &live).unwrap();

        assert_eq!(
            std::fs::read(&live).unwrap(),
            b"authorized-subscriber-session"
        );
    }

    #[test]
    fn telegram_default_state_dir_uses_account_root() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());

        assert_eq!(
            telegram_state_dir(&paths, "telegram-user"),
            paths
                .user_config_dir
                .join("telegram-accounts/telegram-user")
        );
    }

    #[test]
    fn telegram_alternate_state_dir_uses_account_root() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());

        assert_eq!(
            telegram_state_dir(&paths, "tg-alt"),
            paths.user_config_dir.join("telegram-accounts/tg-alt")
        );
    }

    #[test]
    fn telegram_repl_parses_qr_login_actions() {
        let start: TelegramAction =
            serde_json::from_str(r#"{"action":"login_qr","api_id":123,"api_hash":"hash"}"#)
                .unwrap();
        match start {
            TelegramAction::LoginQr { api_id, api_hash } => {
                assert_eq!(api_id, Some(123));
                assert_eq!(api_hash.as_deref(), Some("hash"));
            }
            other => panic!("unexpected action: {other:?}"),
        }

        let wait: TelegramAction =
            serde_json::from_str(r#"{"action":"login_qr_wait","timeout_seconds":10}"#).unwrap();
        match wait {
            TelegramAction::LoginQrWait { timeout_seconds } => {
                assert_eq!(timeout_seconds, Some(10));
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    fn paths(root: &std::path::Path) -> ConfigPaths {
        ConfigPaths {
            workspace_root: root.join("workspace"),
            workspace_config_dir: root.join("workspace/.puffer"),
            user_config_dir: root.join("home/.puffer"),
            builtin_resources_dir: root.join("resources"),
        }
    }
}
