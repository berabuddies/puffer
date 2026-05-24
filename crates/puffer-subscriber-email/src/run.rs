//! Top-level entrypoint and event loop for the email subscriber.
//!
//! [`run`] is invoked by the hidden `puffer __subscriber email` subcommand.
//! The caller is already inside a Tokio runtime; we do not build a new one.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use puffer_subscriber_runtime::SubscriberCommand;
use serde_json::json;
use tracing::{error, info, warn};

use crate::commands::CommandStream;
use crate::config::{self, EmailConfig};
use crate::events::emit_control;
use crate::imap_poll;
use crate::smtp_send;
use crate::state::{self, SeenState};

/// Base poll interval between successful IMAP rounds.
const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// Minimum backoff after a transient error.
const BACKOFF_MIN: Duration = Duration::from_secs(5);
/// Maximum backoff between polls while errors persist.
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Resolved environment settings for the email skill.
struct SkillEnv {
    state_dir: PathBuf,
    topic: String,
}

impl SkillEnv {
    /// Reads `PUFFER_SKILL_STATE_DIR` (defaulting to `./state`) and
    /// `PUFFER_SKILL_TOPIC` (defaulting to `"email"`).
    fn from_env() -> Self {
        let state_dir = match std::env::var("PUFFER_SKILL_STATE_DIR") {
            Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => PathBuf::from("./state"),
        };
        let topic = std::env::var("PUFFER_SKILL_TOPIC")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "email".to_string());
        Self { state_dir, topic }
    }
}

/// Runs the email subscriber until stdin closes or a fatal error occurs.
///
/// Behavior:
/// * Loads persisted config from `<state_dir>/config.toml` if present; if
///   absent, emits a `config_required` control event and waits on stdin for
///   an `EmailConfigure` command.
/// * Polls IMAP on a `POLL_INTERVAL` cadence, using exponential backoff on
///   transient errors. Races the poll timer against stdin commands with
///   `tokio::select!` so new `SendMessage` / `EmailConfigure` commands are
///   handled promptly.
/// * All events / control messages are written to stdout as ndjson; logs go
///   to stderr via `tracing`.
pub async fn run() -> anyhow::Result<()> {
    let env = SkillEnv::from_env();
    info!(
        state_dir = %env.state_dir.display(),
        topic = %env.topic,
        "starting email subscriber"
    );

    tokio::fs::create_dir_all(&env.state_dir)
        .await
        .with_context(|| format!("create state dir {}", env.state_dir.display()))?;

    let mut commands = CommandStream::new();

    let mut config = match config::load(&env.state_dir).await? {
        Some(cfg) => {
            emit_control(&env.topic, "ready", json!({ "resumed": true }))?;
            Some(cfg)
        }
        None => {
            emit_control(&env.topic, "config_required", json!({}))?;
            None
        }
    };

    // If no config on disk, block on stdin until one arrives (or stdin
    // closes). Other commands emit `command_ignored` in the meantime.
    while config.is_none() {
        let cmd = commands.next().await?;
        let Some(cmd) = cmd else {
            info!("stdin closed before email subscriber was configured");
            return Ok(());
        };
        match cmd {
            SubscriberCommand::EmailConfigure {
                imap_host,
                imap_port,
                smtp_host,
                smtp_port,
                username,
                password,
                from_address,
                allowed_senders,
            } => {
                let cfg = EmailConfig::from_command_fields(
                    imap_host,
                    imap_port,
                    smtp_host,
                    smtp_port,
                    username,
                    password,
                    from_address,
                    allowed_senders,
                );
                if !cfg.is_valid() {
                    emit_control(
                        &env.topic,
                        "config_error",
                        json!({
                            "error": "imap_host, smtp_host, username and from_address are required"
                        }),
                    )?;
                    continue;
                }
                config::save(&env.state_dir, &cfg).await?;
                emit_control(&env.topic, "config_updated", json!({}))?;
                config = Some(cfg);
            }
            SubscriberCommand::SendMessage { peer, .. } => {
                emit_control(
                    &env.topic,
                    "send_unsupported",
                    json!({
                        "error": "email subscriber is not configured yet",
                        "peer": peer,
                    }),
                )?;
            }
            SubscriberCommand::TelegramLoginStart { .. }
            | SubscriberCommand::TelegramLoginSubmitCode { .. }
            | SubscriberCommand::TelegramLoginSubmitPassword { .. }
            | SubscriberCommand::TelegramQrLoginStart { .. }
            | SubscriberCommand::TelegramQrLoginWait { .. }
            | SubscriberCommand::TelegramImportTdata { .. }
            | SubscriberCommand::TelegramAuthOk
            | SubscriberCommand::TelegramListPeers { .. }
            | SubscriberCommand::TelegramSearchMessages { .. } => {
                emit_control(
                    &env.topic,
                    "command_ignored",
                    json!({"error": "email subscriber does not handle telegram login"}),
                )?;
            }
            SubscriberCommand::Custom { op, .. } => {
                emit_control(
                    &env.topic,
                    "command_ignored",
                    json!({ "op": op, "error": "unknown custom op" }),
                )?;
            }
        }
    }

    let config = config.expect("config set above");
    drive_poll_loop(&env, &mut commands, config).await
}

/// Runs the infinite poll/select loop once we have a valid config.
///
/// Each iteration races the current sleep timer against stdin. On timer
/// fire, runs one IMAP poll and resets the backoff. On command arrival,
/// handles it synchronously and re-enters the select without necessarily
/// polling (so a burst of commands can't starve the poll timer forever —
/// the timer is preserved across select branches).
async fn drive_poll_loop(
    env: &SkillEnv,
    commands: &mut CommandStream,
    mut config: EmailConfig,
) -> anyhow::Result<()> {
    let mut seen = state::load(&env.state_dir).await?;
    let mut current_delay = POLL_INTERVAL;
    let mut backoff = BACKOFF_MIN;

    loop {
        let timer = tokio::time::sleep(current_delay);
        tokio::pin!(timer);

        tokio::select! {
            _ = &mut timer => {
                match imap_poll::poll_once(&env.topic, &config, &env.state_dir, &mut seen).await {
                    Ok(count) => {
                        if count > 0 {
                            info!(count, "emitted email events");
                        }
                        current_delay = POLL_INTERVAL;
                        backoff = BACKOFF_MIN;
                    }
                    Err(err) => {
                        warn!(error = %err, "email poll failed; backing off");
                        emit_control(
                            &env.topic,
                            "poll_error",
                            json!({ "error": format!("{err:#}") }),
                        )?;
                        current_delay = backoff;
                        backoff = (backoff * 2).min(BACKOFF_MAX);
                    }
                }
            }
            cmd = commands.next() => {
                let Some(cmd) = cmd? else {
                    info!("stdin closed; shutting down email subscriber");
                    return Ok(());
                };
                handle_command(env, &mut config, &mut seen, cmd).await?;
                // Preserve the current timer; don't reset it — otherwise
                // a chatty stdin could indefinitely defer polling.
                current_delay = remaining(&timer, current_delay);
            }
        }
    }
}

/// Returns the time left on `timer` as a [`Duration`].
///
/// `tokio::time::Sleep::deadline` gives us the firing instant; subtracting
/// `now` yields the residual wait. Clamped to zero for a timer that has
/// already expired (the next select branch will fire it immediately).
fn remaining(timer: &std::pin::Pin<&mut tokio::time::Sleep>, _current: Duration) -> Duration {
    let deadline = timer.deadline();
    let now = tokio::time::Instant::now();
    deadline.saturating_duration_since(now)
}

/// Dispatches one stdin command while the poll loop is live.
///
/// Config updates are persisted and take effect on the next poll. Sends
/// translate into one SMTP session per call — fine for the expected low
/// volume; the lettre transport is cheap to build.
async fn handle_command(
    env: &SkillEnv,
    config: &mut EmailConfig,
    seen: &mut SeenState,
    cmd: SubscriberCommand,
) -> anyhow::Result<()> {
    match cmd {
        SubscriberCommand::EmailConfigure {
            imap_host,
            imap_port,
            smtp_host,
            smtp_port,
            username,
            password,
            from_address,
            allowed_senders,
        } => {
            let new_cfg = EmailConfig::from_command_fields(
                imap_host,
                imap_port,
                smtp_host,
                smtp_port,
                username,
                password,
                from_address,
                allowed_senders,
            );
            if !new_cfg.is_valid() {
                emit_control(
                    &env.topic,
                    "config_error",
                    json!({
                        "error": "imap_host, smtp_host, username and from_address are required"
                    }),
                )?;
                return Ok(());
            }
            // If the account changed, drop the UID high-water mark: the new
            // mailbox is a different UIDVALIDITY namespace and its current
            // UIDs shouldn't inherit the old account's baseline.
            if new_cfg.username != config.username || new_cfg.imap_host != config.imap_host {
                *seen = SeenState::default();
                state::save(&env.state_dir, seen).await?;
            }
            config::save(&env.state_dir, &new_cfg).await?;
            *config = new_cfg;
            emit_control(&env.topic, "config_updated", json!({}))?;
        }
        SubscriberCommand::SendMessage { peer, text, .. } => {
            match smtp_send::send_message(config, &peer, &text).await {
                Ok(outcome) => {
                    emit_control(
                        &env.topic,
                        "send_complete",
                        json!({ "peer": peer, "bytes": outcome.bytes }),
                    )?;
                }
                Err(err) => {
                    error!(error = %err, peer = %peer, "email send failed");
                    emit_control(
                        &env.topic,
                        "send_error",
                        json!({ "peer": peer, "error": format!("{err:#}") }),
                    )?;
                }
            }
        }
        SubscriberCommand::TelegramLoginStart { .. }
        | SubscriberCommand::TelegramLoginSubmitCode { .. }
        | SubscriberCommand::TelegramLoginSubmitPassword { .. }
        | SubscriberCommand::TelegramQrLoginStart { .. }
        | SubscriberCommand::TelegramQrLoginWait { .. }
        | SubscriberCommand::TelegramImportTdata { .. }
        | SubscriberCommand::TelegramAuthOk
        | SubscriberCommand::TelegramListPeers { .. }
        | SubscriberCommand::TelegramSearchMessages { .. } => {
            emit_control(
                &env.topic,
                "command_ignored",
                json!({ "error": "email subscriber does not handle telegram login" }),
            )?;
        }
        SubscriberCommand::Custom { op, .. } => {
            emit_control(
                &env.topic,
                "command_ignored",
                json!({ "op": op, "error": "unknown custom op" }),
            )?;
        }
    }
    Ok(())
}
