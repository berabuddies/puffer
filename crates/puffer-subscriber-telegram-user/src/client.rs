//! Top-level entrypoint and main event loop.
//!
//! [`run`] is invoked by the hidden `puffer __subscriber telegram-user`
//! subcommand. It orchestrates session loading, the login flow, and the
//! infinite `next_update` loop that emits ndjson message events.

use anyhow::Context as _;
use grammers_client::{session::Session, Client, Config};
use puffer_subscriber_runtime::SubscriberCommand;
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};

use crate::actions::handle_telegram_act;
use crate::commands::CommandStream;
use crate::delivery::DeliveryCursor;
use crate::events::emit_control;
use crate::import::{import_tdata, TdataImportOptions, TdataImportOutcome};
use crate::login;
use crate::notifications::NotificationMuteCache;
use crate::outbound::handle_send_message;
use crate::peers::{handle_list_messages, handle_list_peers, handle_search_messages};
use crate::qr_login;
use crate::session_resume::{recoverable_live_update_error, try_resume_session, SessionResume};
use crate::state::{default_init_params, LoginState, SkillEnv};
use crate::updates::{handle_live_update, spawn_live_update_task, LiveUpdateEvent};

const LIVE_UPDATE_RECOVERY_DELAY: Duration = Duration::from_secs(1);
const OFFLINE_RESUME_RETRY_INITIAL_DELAY: Duration = Duration::from_secs(5);
const OFFLINE_RESUME_RETRY_MAX_DELAY: Duration = Duration::from_secs(60);

enum UpdateLoopExit {
    StdinClosed,
    ReauthStarted,
}

enum RuntimeCommandOutcome {
    Continue,
    ReauthStarted,
    ClientReplaced,
}

enum OfflineResumeAttempt {
    Resumed {
        client: Client,
        offline_since_ms: i64,
    },
    AuthRequired,
    StillOffline(OfflineResumeState),
}

#[derive(Debug, Clone)]
struct OfflineResumeState {
    detail: String,
    offline_since_ms: i64,
    retry_delay: Duration,
    next_retry_at: tokio::time::Instant,
    next_retry_at_ms: i64,
}

impl OfflineResumeState {
    fn new(detail: String) -> Self {
        Self::scheduled(
            detail,
            monitoring_live_since_ms(),
            OFFLINE_RESUME_RETRY_INITIAL_DELAY,
        )
    }

    fn retry_failed(&self, detail: String) -> Self {
        Self::scheduled(
            detail,
            self.offline_since_ms,
            next_offline_retry_delay(self.retry_delay),
        )
    }

    fn scheduled(detail: String, offline_since_ms: i64, retry_delay: Duration) -> Self {
        Self {
            detail,
            offline_since_ms,
            retry_delay,
            next_retry_at: tokio::time::Instant::now() + retry_delay,
            next_retry_at_ms: unix_now_ms() + retry_delay.as_millis() as i64,
        }
    }
}

/// Result state handed back by the spawned startup hydration task.
type StartupHydrationState = (DeliveryCursor, NotificationMuteCache);

/// Runs the Telegram user subscriber until stdin closes or a fatal error
/// occurs. The caller is expected to already be inside a Tokio runtime
/// (the top-level `#[tokio::main]` in the puffer binary).
///
/// Behavior:
/// * Reads `$PUFFER_SKILL_STATE_DIR` / `$PUFFER_SKILL_TOPIC` from the env.
/// * Opens / creates the session file.
/// * If the session is already authenticated, connects immediately and
///   enters the update loop.
/// * Otherwise, waits on stdin for a `TelegramLoginStart` command before
///   requesting a login code.
/// * In the update loop, processes stdin commands and telegram updates
///   concurrently with `tokio::select!`.
pub async fn run() -> anyhow::Result<()> {
    let env = SkillEnv::from_env();
    // Install tracing first so every log below is durably written to
    // <state>/telegram.log. Held for the whole process: dropping the guard
    // flushes and stops the non-blocking file writer.
    let _log_guard = crate::logging::init(&env);
    info!(
        session = %env.session_path.display(),
        topic = %env.topic,
        log = %env.state_dir.join("telegram.log").display(),
        "starting telegram-user subscriber"
    );

    if let Some(parent) = env.session_path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create session parent dir {}", parent.display()))?;
        }
    }

    let mut commands = CommandStream::new();
    let mut login_state = LoginState::new();
    let mut qr_state = None;

    // Attempt to reuse a pre-authenticated session first. If the session file
    // holds a valid auth key we can go straight to the update loop without
    // prompting the agent for login credentials.
    //
    // `offline_state` parks the subscriber as "temporarily offline" when the
    // resume failed for infrastructure reasons (network/DC unreachable) with
    // the auth material intact. The same login-phase state machine owns both
    // stdin commands and the automatic retry timer so the Telegram client and
    // command stream still have a single owner.
    let mut offline_state: Option<OfflineResumeState> = None;
    let mut recovered_offline_since_ms: Option<i64> = None;
    let mut client = match try_resume_session(&env).await? {
        SessionResume::Resumed(c) => {
            emit_control(&env.topic, "ready", json!({ "resumed": true }))?;
            Some(c)
        }
        SessionResume::AuthRequired => {
            emit_control(&env.topic, "login_required", json!({}))?;
            None
        }
        SessionResume::Transient(detail) => {
            let state = OfflineResumeState::new(detail);
            emit_resume_offline(&env, &state)?;
            offline_state = Some(state);
            None
        }
    };

    // Login phase: if we don't have an authorized client yet, keep reading
    // stdin commands until login completes or stdin closes.
    while client.as_ref().map(|_| false).unwrap_or(true) {
        let cmd = if let Some(retry_at) = offline_state.as_ref().map(|state| state.next_retry_at) {
            tokio::select! {
                maybe = commands.next() => maybe?,
                _ = tokio::time::sleep_until(retry_at) => {
                    let state = offline_state
                        .take()
                        .expect("offline state exists for retry timer");
                    match attempt_offline_resume(&env, state).await? {
                        OfflineResumeAttempt::Resumed {
                            client: recovered,
                            offline_since_ms,
                        } => {
                            recovered_offline_since_ms = Some(offline_since_ms);
                            client = Some(recovered);
                        }
                        OfflineResumeAttempt::AuthRequired => {}
                        OfflineResumeAttempt::StillOffline(next_state) => {
                            offline_state = Some(next_state);
                        }
                    }
                    continue;
                }
            }
        } else {
            commands.next().await?
        };
        let Some(cmd) = cmd else {
            info!("stdin closed before login completed");
            return Ok(());
        };
        // Business commands still trigger an immediate retry instead of
        // waiting for the next timer tick, then answer on their own channel if
        // Telegram remains offline.
        if offline_state.is_some() && is_resume_retry_command(&cmd) {
            let state = offline_state
                .take()
                .expect("offline state exists for retry command");
            match attempt_offline_resume(&env, state).await? {
                OfflineResumeAttempt::Resumed {
                    client: mut resumed,
                    offline_since_ms,
                } => {
                    recovered_offline_since_ms = Some(offline_since_ms);
                    let _ = handle_runtime_command(
                        &env,
                        &mut resumed,
                        &mut login_state,
                        &mut qr_state,
                        cmd,
                    )
                    .await?;
                    client = Some(resumed);
                    continue;
                }
                OfflineResumeAttempt::AuthRequired => {}
                OfflineResumeAttempt::StillOffline(next_state) => {
                    emit_offline_command_error(&env, &cmd, &next_state.detail)?;
                    offline_state = Some(next_state);
                    continue;
                }
            }
        }
        match cmd {
            SubscriberCommand::TelegramLoginStart {
                phone,
                api_id,
                api_hash,
            } => match login::start(&env, &mut login_state, phone, api_id, api_hash).await? {
                Some(c) => client = Some(c),
                None => continue,
            },
            SubscriberCommand::TelegramQrLoginStart { api_id, api_hash } => {
                match qr_login::start(&env, &mut login_state, &mut qr_state, api_id, api_hash)
                    .await?
                {
                    qr_login::QrLoginOutcome::Pending => {}
                    qr_login::QrLoginOutcome::AwaitingPassword(qr_client)
                    | qr_login::QrLoginOutcome::Complete(qr_client) => {
                        client = Some(qr_client);
                    }
                }
            }
            SubscriberCommand::TelegramQrLoginWait { timeout_seconds } => {
                match qr_login::wait(&env, &mut login_state, &mut qr_state, timeout_seconds).await?
                {
                    qr_login::QrLoginOutcome::Pending => {}
                    qr_login::QrLoginOutcome::AwaitingPassword(qr_client)
                    | qr_login::QrLoginOutcome::Complete(qr_client) => {
                        client = Some(qr_client);
                    }
                }
            }
            SubscriberCommand::TelegramImportTdata {
                path,
                passcode,
                account_index,
                key_file,
            } => {
                if let Some(imported) = import_and_connect(
                    &env,
                    TdataImportOptions {
                        path,
                        passcode,
                        account_index,
                        key_file,
                    },
                )
                .await?
                {
                    client = Some(imported);
                }
            }
            SubscriberCommand::TelegramLoginSubmitCode { .. }
            | SubscriberCommand::TelegramLoginSubmitPassword { .. } => {
                emit_control(
                    &env.topic,
                    "login_error",
                    json!({
                        "error": "login not started; send telegram_login_start first"
                    }),
                )?;
            }
            SubscriberCommand::TelegramAuthOk => {
                // While parked offline the on-disk session is signed in and
                // only the network was unreachable — answering ok:false would
                // degrade the connection as if the login were lost. Probes
                // deliberately do NOT trigger a resume retry; the retry timer
                // is owned by this state machine.
                let offline = offline_state.is_some();
                emit_control(
                    &env.topic,
                    "auth_ok",
                    json!({
                        "ok": offline,
                        "authenticated": offline,
                        "offline": offline,
                    }),
                )?;
            }
            SubscriberCommand::TelegramListPeers { query, .. } => {
                emit_control(
                    &env.topic,
                    "peer_list_error",
                    json!({
                        "error": "not authenticated yet; complete login before listing Telegram peers",
                        "query": query,
                    }),
                )?;
            }
            SubscriberCommand::TelegramSearchMessages { peer, query, .. } => {
                emit_control(
                    &env.topic,
                    "message_search_error",
                    json!({
                        "error": "not authenticated yet; complete login before searching Telegram messages",
                        "peer": peer,
                        "query": query,
                    }),
                )?;
            }
            SubscriberCommand::TelegramListMessages { peer, .. } => {
                emit_control(
                    &env.topic,
                    "message_list_error",
                    json!({
                        "error": "not authenticated yet; complete login before listing Telegram messages",
                        "peer": peer,
                    }),
                )?;
            }
            SubscriberCommand::SendMessage { peer, .. } => {
                emit_control(
                    &env.topic,
                    "send_unsupported",
                    json!({
                        "error": "not authenticated yet; complete login before sending messages",
                        "peer": peer,
                    }),
                )?;
            }
            SubscriberCommand::EmailConfigure { .. } => {
                emit_control(
                    &env.topic,
                    "command_ignored",
                    json!({"error": "telegram-user does not handle email configuration"}),
                )?;
            }
            SubscriberCommand::Custom { op, args } => handle_login_custom(&env, op, args)?,
        }
    }

    let mut client = client.expect("client set after login phase");
    let mut delivery_cursor = DeliveryCursor::load(&env)?;
    let mut notification_mutes = NotificationMuteCache::default();

    // Drive the remaining login steps (code + optional 2FA) if needed, then
    // enter the update loop. `client` may already be authorized when we
    // resumed a session, in which case this loop is skipped on the first
    // iteration via `is_authorized`.
    let mut authorized = client
        .is_authorized()
        .await
        .context("probe initial auth state")?;

    loop {
        if authorized {
            match run_update_loop(
                &env,
                &mut commands,
                &mut client,
                &mut login_state,
                &mut qr_state,
                &mut delivery_cursor,
                &mut notification_mutes,
                recovered_offline_since_ms.take(),
            )
            .await?
            {
                UpdateLoopExit::StdinClosed => return Ok(()),
                UpdateLoopExit::ReauthStarted => {
                    authorized = false;
                    continue;
                }
            }
        }

        let cmd = commands.next().await?;
        let Some(cmd) = cmd else {
            info!("stdin closed before login finalized");
            return Ok(());
        };
        match cmd {
            SubscriberCommand::TelegramLoginSubmitCode { code } => {
                match submit_code_with_reconnect(&env, &mut client, &mut login_state, code).await? {
                    login::CodeSubmitOutcome::Complete => authorized = true,
                    login::CodeSubmitOutcome::AwaitingPassword
                    | login::CodeSubmitOutcome::Failed
                    | login::CodeSubmitOutcome::RetryableTransportError { .. } => {}
                }
            }
            SubscriberCommand::TelegramLoginSubmitPassword { password } => {
                if login::submit_password(&env, &mut login_state, &client, password).await? {
                    authorized = true;
                }
            }
            SubscriberCommand::TelegramLoginStart {
                phone,
                api_id,
                api_hash,
            } => {
                // Operator restarted the flow; re-request the login code on
                // the same connection.
                login::start(&env, &mut login_state, phone, api_id, api_hash).await?;
            }
            SubscriberCommand::TelegramQrLoginStart { api_id, api_hash } => {
                match qr_login::start(&env, &mut login_state, &mut qr_state, api_id, api_hash)
                    .await?
                {
                    qr_login::QrLoginOutcome::Pending => {}
                    qr_login::QrLoginOutcome::AwaitingPassword(qr_client) => {
                        client = qr_client;
                        authorized = false;
                    }
                    qr_login::QrLoginOutcome::Complete(qr_client) => {
                        client = qr_client;
                        authorized = true;
                    }
                }
            }
            SubscriberCommand::TelegramQrLoginWait { timeout_seconds } => {
                match qr_login::wait(&env, &mut login_state, &mut qr_state, timeout_seconds).await?
                {
                    qr_login::QrLoginOutcome::Pending => {}
                    qr_login::QrLoginOutcome::AwaitingPassword(qr_client) => {
                        client = qr_client;
                        authorized = false;
                    }
                    qr_login::QrLoginOutcome::Complete(qr_client) => {
                        client = qr_client;
                        authorized = true;
                    }
                }
            }
            SubscriberCommand::TelegramImportTdata {
                path,
                passcode,
                account_index,
                key_file,
            } => {
                if let Some(imported) = import_and_connect(
                    &env,
                    TdataImportOptions {
                        path,
                        passcode,
                        account_index,
                        key_file,
                    },
                )
                .await?
                {
                    client = imported;
                    authorized = true;
                }
            }
            SubscriberCommand::TelegramAuthOk => {
                emit_auth_ok(&env, &client).await?;
            }
            SubscriberCommand::TelegramListPeers { query, .. } => {
                emit_control(
                    &env.topic,
                    "peer_list_error",
                    json!({
                        "error": "complete login before listing Telegram peers",
                        "query": query,
                    }),
                )?;
            }
            SubscriberCommand::TelegramSearchMessages { peer, query, .. } => {
                emit_control(
                    &env.topic,
                    "message_search_error",
                    json!({
                        "error": "complete login before searching Telegram messages",
                        "peer": peer,
                        "query": query,
                    }),
                )?;
            }
            SubscriberCommand::TelegramListMessages { peer, .. } => {
                emit_control(
                    &env.topic,
                    "message_list_error",
                    json!({
                        "error": "complete login before listing Telegram messages",
                        "peer": peer,
                    }),
                )?;
            }
            SubscriberCommand::SendMessage { peer, .. } => {
                emit_control(
                    &env.topic,
                    "send_unsupported",
                    json!({
                        "error": "complete login before sending messages",
                        "peer": peer,
                    }),
                )?;
            }
            SubscriberCommand::EmailConfigure { .. } => {
                emit_control(
                    &env.topic,
                    "command_ignored",
                    json!({"error": "telegram-user does not handle email configuration"}),
                )?;
            }
            SubscriberCommand::Custom { op, args } => handle_login_custom(&env, op, args)?,
        }
    }
}

/// Drives the main event loop: waits for either a Telegram update or an
/// inbound stdin command, handles it, and repeats. Returns when stdin
/// closes or a fatal error occurs.
async fn run_update_loop(
    env: &SkillEnv,
    commands: &mut CommandStream,
    client: &mut Client,
    login_state: &mut LoginState,
    qr_state: &mut Option<qr_login::QrLoginState>,
    delivery_cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
    recovered_offline_since_ms: Option<i64>,
) -> anyhow::Result<UpdateLoopExit> {
    emit_control(&env.topic, "ready", json!({}))?;
    // Monitoring is promised from this moment on: messages dated after this
    // boundary must reach the triage pipeline even when they arrive while
    // startup hydration is still running (see hydrate_dialog_state).
    let live_since_ms =
        startup_monitoring_boundary(monitoring_live_since_ms(), recovered_offline_since_ms);
    reset_delivery_cursor_for_current_account(client, delivery_cursor).await?;
    if let Some(exit) = hydrate_startup_state_before_updates(
        env,
        commands,
        client,
        login_state,
        qr_state,
        delivery_cursor,
        notification_mutes,
        live_since_ms,
    )
    .await?
    {
        return Ok(exit);
    }
    persist_live_session_state(env, client);
    let (mut live_updates, mut live_task) = spawn_live_update_task(env.clone(), client.clone());
    info!("entering telegram update loop");

    loop {
        tokio::select! {
            biased;
            update = live_updates.recv() => {
                let Some(update) = update else {
                    live_task.abort();
                    return Err(anyhow::anyhow!("telegram live update task stopped"));
                };
                if let LiveUpdateEvent::Error(error) = update {
                    live_task.abort();
                    // #570: the live update stream died — "connected but stops
                    // receiving". Record it (classified) before tearing down so
                    // the drop is queryable instead of a bare tracing line.
                    let recoverable = recoverable_live_update_error(&error);
                    crate::health::report_update_loop_error(env, &error, !recoverable);
                    if recoverable {
                        warn!(%error, "recovering telegram live update stream");
                        tokio::time::sleep(LIVE_UPDATE_RECOVERY_DELAY).await;
                        if let SessionResume::Resumed(recovered) =
                            try_resume_session(env).await?
                        {
                            *client = recovered;
                            reset_delivery_cursor_for_current_account(client, delivery_cursor)
                                .await?;
                            if let Some(exit) = hydrate_startup_state_before_updates(
                                env,
                                commands,
                                client,
                                login_state,
                                qr_state,
                                delivery_cursor,
                                notification_mutes,
                                live_since_ms,
                            )
                            .await?
                            {
                                return Ok(exit);
                            }
                            persist_live_session_state(env, client);
                            emit_control(
                                &env.topic,
                                "ready",
                                json!({ "resumed": true, "recovered": true }),
                            )?;
                            (live_updates, live_task) =
                                spawn_live_update_task(env.clone(), client.clone());
                            continue;
                        }
                    }
                    error!(%error, "next_update failed");
                    return Err(anyhow::anyhow!("next_update: {error}"));
                }
                handle_live_update(
                    env,
                    delivery_cursor,
                    notification_mutes,
                    update,
                )
                .await?;
                persist_live_session_state(env, client);
            }
            cmd = commands.next() => {
                let Some(cmd) = cmd? else {
                    info!("stdin closed; shutting down update loop");
                    live_task.abort();
                    return Ok(UpdateLoopExit::StdinClosed);
                };
                match handle_runtime_command(env, client, login_state, qr_state, cmd).await? {
                    RuntimeCommandOutcome::Continue => {}
                    RuntimeCommandOutcome::ReauthStarted => {
                        live_task.abort();
                        return Ok(UpdateLoopExit::ReauthStarted);
                    }
                    RuntimeCommandOutcome::ClientReplaced => {
                        live_task.abort();
                        reset_delivery_cursor_for_current_account(client, delivery_cursor).await?;
                        if let Some(exit) = hydrate_startup_state_before_updates(
                            env,
                            commands,
                            client,
                            login_state,
                            qr_state,
                            delivery_cursor,
                            notification_mutes,
                            live_since_ms,
                        )
                        .await?
                        {
                            return Ok(exit);
                        }
                        persist_live_session_state(env, client);
                        (live_updates, live_task) =
                            spawn_live_update_task(env.clone(), client.clone());
                    }
                }
            }
        }
    }
}

async fn reset_delivery_cursor_for_current_account(
    client: &Client,
    delivery_cursor: &mut DeliveryCursor,
) -> anyhow::Result<()> {
    let user = client.get_me().await?;
    if delivery_cursor.reset_for_account(user.id()) {
        info!(
            user_id = user.id(),
            "reset Telegram delivery cursor for authenticated account"
        );
    }
    Ok(())
}

/// Runs startup hydration on its own task so inbound runtime commands (the
/// once-a-minute connection auth probe, message search/send, …) are answered
/// while hydration keeps making progress.
///
/// Hydration must NOT be recreated per command: dropping the in-flight future
/// on every command meant accounts whose hydration outlasts the probe
/// interval restarted from scratch forever, never reached the live update
/// loop, and silently stopped delivering messages while the connection still
/// reported healthy.
async fn hydrate_startup_state_before_updates(
    env: &SkillEnv,
    commands: &mut CommandStream,
    client: &mut Client,
    login_state: &mut LoginState,
    qr_state: &mut Option<qr_login::QrLoginState>,
    delivery_cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
    live_since_ms: i64,
) -> anyhow::Result<Option<UpdateLoopExit>> {
    let mut hydration = spawn_startup_hydration(
        env,
        client,
        delivery_cursor,
        notification_mutes,
        live_since_ms,
    );
    loop {
        tokio::select! {
            biased;
            cmd = commands.next() => match cmd? {
                Some(cmd) => {
                    match handle_runtime_command(env, client, login_state, qr_state, cmd).await? {
                        RuntimeCommandOutcome::Continue => {}
                        RuntimeCommandOutcome::ReauthStarted => {
                            abort_startup_hydration(hydration).await;
                            return Ok(Some(UpdateLoopExit::ReauthStarted));
                        }
                        RuntimeCommandOutcome::ClientReplaced => {
                            // The in-flight hydration belongs to the replaced
                            // client; restart it against the new one from the
                            // last persisted cursor state.
                            abort_startup_hydration(hydration).await;
                            *delivery_cursor = DeliveryCursor::load(env).unwrap_or_default();
                            *notification_mutes = NotificationMuteCache::default();
                            reset_delivery_cursor_for_current_account(client, delivery_cursor)
                                .await?;
                            info!("restarting telegram startup hydration for replaced client");
                            hydration = spawn_startup_hydration(
                                env,
                                client,
                                delivery_cursor,
                                notification_mutes,
                                live_since_ms,
                            );
                        }
                    }
                }
                None => {
                    abort_startup_hydration(hydration).await;
                    info!("stdin closed before telegram startup hydration completed");
                    return Ok(Some(UpdateLoopExit::StdinClosed));
                }
            },
            result = &mut hydration => {
                let (cursor, mutes) =
                    result.context("join telegram startup hydration task")??;
                *delivery_cursor = cursor;
                *notification_mutes = mutes;
                return Ok(None);
            }
        }
    }
}

/// Spawns `hydrate_dialog_state` on an owned task. The cursor and mute cache
/// are moved in (leaving defaults behind) and handed back on completion.
fn spawn_startup_hydration(
    env: &SkillEnv,
    client: &Client,
    delivery_cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
    live_since_ms: i64,
) -> tokio::task::JoinHandle<anyhow::Result<StartupHydrationState>> {
    let env = env.clone();
    let client = client.clone();
    let mut cursor = std::mem::take(delivery_cursor);
    let mut mutes = std::mem::take(notification_mutes);
    tokio::spawn(async move {
        let pending_avatar_chats = crate::startup::hydrate_dialog_state(
            &env,
            &client,
            &mut cursor,
            &mut mutes,
            live_since_ms,
        )
        .await?;
        if !pending_avatar_chats.is_empty() {
            // Avatars are contact-picker garnish; fetch them after the update
            // loop is live instead of delaying message delivery.
            tokio::spawn(async move {
                crate::peer_cache::hydrate_chat_avatars_deferred(
                    &env,
                    &client,
                    pending_avatar_chats,
                )
                .await;
            });
        }
        Ok((cursor, mutes))
    })
}

async fn attempt_offline_resume(
    env: &SkillEnv,
    state: OfflineResumeState,
) -> anyhow::Result<OfflineResumeAttempt> {
    match try_resume_session(env).await? {
        SessionResume::Resumed(client) => {
            emit_connection_health_ok(env)?;
            emit_control(
                &env.topic,
                "ready",
                json!({ "resumed": true, "recovered": true }),
            )?;
            Ok(OfflineResumeAttempt::Resumed {
                client,
                offline_since_ms: state.offline_since_ms,
            })
        }
        SessionResume::AuthRequired => {
            emit_connection_health_auth_required(env)?;
            emit_control(&env.topic, "login_required", json!({}))?;
            Ok(OfflineResumeAttempt::AuthRequired)
        }
        SessionResume::Transient(detail) => {
            let next_state = state.retry_failed(detail);
            emit_connection_health_retrying(env, &next_state)?;
            Ok(OfflineResumeAttempt::StillOffline(next_state))
        }
    }
}

fn emit_resume_offline(env: &SkillEnv, state: &OfflineResumeState) -> anyhow::Result<()> {
    emit_control(
        &env.topic,
        "resume_offline",
        json!({
            "error": state.detail,
            "retryable": true,
            "offline_since_ms": state.offline_since_ms,
            "next_retry_at_ms": state.next_retry_at_ms,
        }),
    )
}

fn emit_connection_health_retrying(
    env: &SkillEnv,
    state: &OfflineResumeState,
) -> anyhow::Result<()> {
    emit_control(
        &env.topic,
        "connection_health",
        json!({
            "status": "retrying",
            "reason": "connect_failed",
            "detail": state.detail,
            "offline_since_ms": state.offline_since_ms,
            "next_retry_at_ms": state.next_retry_at_ms,
        }),
    )
}

fn emit_connection_health_ok(env: &SkillEnv) -> anyhow::Result<()> {
    emit_control(
        &env.topic,
        "connection_health",
        json!({
            "status": "ok",
        }),
    )
}

fn emit_connection_health_auth_required(env: &SkillEnv) -> anyhow::Result<()> {
    emit_control(
        &env.topic,
        "connection_health",
        json!({
            "status": "auth_required",
            "reason": "login_required",
        }),
    )
}

/// Unix-millis boundary from which this session promises message delivery.
/// Message dates are Telegram server time; a small grace window absorbs local
/// clock skew so a message sent moments after startup is never misread as
/// pre-session history.
fn monitoring_live_since_ms() -> i64 {
    const CLOCK_SKEW_GRACE_MS: i64 = 30_000;
    unix_now_ms() - CLOCK_SKEW_GRACE_MS
}

fn unix_now_ms() -> i64 {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    now_ms
}

fn startup_monitoring_boundary(
    default_live_since_ms: i64,
    recovered_offline_since_ms: Option<i64>,
) -> i64 {
    recovered_offline_since_ms.unwrap_or(default_live_since_ms)
}

fn next_offline_retry_delay(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .min(OFFLINE_RESUME_RETRY_MAX_DELAY)
}

/// Stops an in-flight hydration task and waits for it to wind down so a
/// successor never runs concurrently against the same on-disk state.
async fn abort_startup_hydration(
    hydration: tokio::task::JoinHandle<anyhow::Result<StartupHydrationState>>,
) {
    hydration.abort();
    let _ = hydration.await;
}

fn persist_live_session_state(env: &SkillEnv, client: &Client) {
    client.sync_update_state();
    if let Err(error) = client.session().save_to_file(&env.session_path) {
        warn!(
            error = %error,
            path = %env.session_path.display(),
            "failed to persist Telegram live update session state"
        );
    }
}

async fn submit_code_with_reconnect(
    env: &SkillEnv,
    client: &mut Client,
    login_state: &mut LoginState,
    code: String,
) -> anyhow::Result<login::CodeSubmitOutcome> {
    let first = login::submit_code(env, login_state, client, code.clone()).await?;
    let login::CodeSubmitOutcome::RetryableTransportError { error } = first else {
        return Ok(first);
    };

    warn!(%error, "retrying telegram sign_in after reconnect");
    match reconnect_login_client(env, login_state).await {
        Ok(reconnected) => {
            *client = reconnected;
            login::submit_code(env, login_state, client, code).await
        }
        Err(reconnect_error) => {
            login_state.clear_tokens();
            emit_control(
                &env.topic,
                "login_error",
                json!({
                    "error": format!(
                        "sign_in transport failed and reconnect failed: {reconnect_error}"
                    ),
                    "phase": "sign_in_reconnect"
                }),
            )?;
            Ok(login::CodeSubmitOutcome::Failed)
        }
    }
}

async fn reconnect_login_client(
    env: &SkillEnv,
    login_state: &LoginState,
) -> anyhow::Result<Client> {
    let api_id = login_state
        .api_id
        .ok_or_else(|| anyhow::anyhow!("login attempt is missing api_id"))?;
    let api_hash = login_state
        .api_hash
        .clone()
        .ok_or_else(|| anyhow::anyhow!("login attempt is missing api_hash"))?;
    let session = Session::load_file_or_create(&env.session_path)
        .with_context(|| format!("load session file {}", env.session_path.display()))?;
    Client::connect(Config {
        session,
        api_id,
        api_hash,
        params: default_init_params(),
    })
    .await
    .context("reconnect telegram client for sign_in retry")
}

/// Handles a stdin command received while the update loop is running.
///
/// Most login-related commands are unexpected here (login already succeeded)
/// but we still accept them to support re-authentication without a restart.
async fn handle_runtime_command(
    env: &SkillEnv,
    client: &mut Client,
    login_state: &mut LoginState,
    qr_state: &mut Option<qr_login::QrLoginState>,
    cmd: SubscriberCommand,
) -> anyhow::Result<RuntimeCommandOutcome> {
    match cmd {
        SubscriberCommand::TelegramLoginStart {
            phone,
            api_id,
            api_hash,
        } => {
            *qr_state = None;
            if let Some(login_client) =
                login::start(env, login_state, phone, api_id, api_hash).await?
            {
                *client = login_client;
                return Ok(RuntimeCommandOutcome::ReauthStarted);
            }
        }
        SubscriberCommand::TelegramLoginSubmitCode { code } => {
            submit_code_with_reconnect(env, client, login_state, code).await?;
        }
        SubscriberCommand::TelegramLoginSubmitPassword { password } => {
            login::submit_password(env, login_state, client, password).await?;
        }
        SubscriberCommand::TelegramQrLoginStart { api_id, api_hash } => {
            login_state.clear_tokens();
            match qr_login::start(env, login_state, qr_state, api_id, api_hash).await? {
                qr_login::QrLoginOutcome::Pending => {}
                qr_login::QrLoginOutcome::AwaitingPassword(qr_client) => {
                    *client = qr_client;
                    return Ok(RuntimeCommandOutcome::ReauthStarted);
                }
                qr_login::QrLoginOutcome::Complete(qr_client) => {
                    *client = qr_client;
                    return Ok(RuntimeCommandOutcome::ClientReplaced);
                }
            }
        }
        SubscriberCommand::TelegramQrLoginWait { timeout_seconds } => {
            match qr_login::wait(env, login_state, qr_state, timeout_seconds).await? {
                qr_login::QrLoginOutcome::Pending => {}
                qr_login::QrLoginOutcome::AwaitingPassword(qr_client) => {
                    *client = qr_client;
                    return Ok(RuntimeCommandOutcome::ReauthStarted);
                }
                qr_login::QrLoginOutcome::Complete(qr_client) => {
                    *client = qr_client;
                    return Ok(RuntimeCommandOutcome::ClientReplaced);
                }
            }
        }
        SubscriberCommand::TelegramImportTdata {
            path,
            passcode,
            account_index,
            key_file,
        } => {
            if let Some(imported) = import_and_connect(
                env,
                TdataImportOptions {
                    path,
                    passcode,
                    account_index,
                    key_file,
                },
            )
            .await?
            {
                *client = imported;
                return Ok(RuntimeCommandOutcome::ClientReplaced);
            }
        }
        SubscriberCommand::TelegramAuthOk => {
            emit_auth_ok(env, client).await?;
        }
        SubscriberCommand::TelegramListPeers {
            query,
            peer_kind,
            limit,
        } => {
            handle_list_peers(env, client, query, peer_kind, limit).await?;
        }
        SubscriberCommand::TelegramSearchMessages {
            peer,
            query,
            limit,
            context,
            succinct,
        } => {
            handle_search_messages(env, client, peer, query, limit, context, succinct).await?;
        }
        SubscriberCommand::TelegramListMessages {
            peer,
            limit,
            before_id,
            sender,
            scan_limit,
            succinct,
        } => {
            handle_list_messages(
                env, client, peer, limit, before_id, sender, scan_limit, succinct,
            )
            .await?;
        }
        SubscriberCommand::SendMessage {
            peer,
            text,
            reply_to,
            media,
            ..
        } => {
            handle_send_message(env, client, peer, text, reply_to, media).await?;
        }
        SubscriberCommand::EmailConfigure { .. } => {
            emit_control(
                &env.topic,
                "command_ignored",
                json!({"error": "telegram-user does not handle email configuration"}),
            )?;
        }
        SubscriberCommand::Custom { op, args } => {
            if op == "telegram_act" {
                handle_telegram_act(env, client, args).await?;
            } else {
                emit_control(
                    &env.topic,
                    "login_error",
                    json!({ "error": format!("command not understood: {op}") }),
                )?;
            }
        }
    }
    Ok(RuntimeCommandOutcome::Continue)
}

async fn emit_auth_ok(env: &SkillEnv, client: &Client) -> anyhow::Result<()> {
    match client.is_authorized().await {
        Ok(ok) => emit_control(
            &env.topic,
            "auth_ok",
            json!({
                "ok": ok,
                "authenticated": ok,
            }),
        ),
        Err(error) => emit_control(
            &env.topic,
            "auth_ok",
            json!({
                "ok": false,
                "authenticated": false,
                "error": error.to_string(),
            }),
        ),
    }
}

fn handle_login_custom(env: &SkillEnv, op: String, args: serde_json::Value) -> anyhow::Result<()> {
    if op == "telegram_act" {
        let action = args
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let input = args.get("input").unwrap_or(&args);
        emit_control(
            &env.topic,
            "telegram_act_error",
            json!({
                "action": action,
                "peer": input
                    .get("to")
                    .or_else(|| input.get("target"))
                    .or_else(|| input.get("channel"))
                    .or_else(|| input.get("chat"))
                    .or_else(|| input.get("peer"))
                    .and_then(serde_json::Value::as_str),
                "error": "complete login before running Telegram connector actions"
            }),
        )?;
        return Ok(());
    }
    emit_control(
        &env.topic,
        "login_error",
        json!({ "error": format!("command not understood: {op}") }),
    )
}

/// Business commands that retry a parked-offline resume on demand. Login
/// commands run their own flow, and auth probes must stay passive because the
/// state machine owns automatic recovery.
fn is_resume_retry_command(cmd: &SubscriberCommand) -> bool {
    match cmd {
        SubscriberCommand::TelegramListPeers { .. }
        | SubscriberCommand::TelegramSearchMessages { .. }
        | SubscriberCommand::TelegramListMessages { .. }
        | SubscriberCommand::SendMessage { .. } => true,
        SubscriberCommand::Custom { op, .. } => op == "telegram_act",
        _ => false,
    }
}

/// Answers a business command while temporarily offline with a retryable
/// error on the command's own response channel — deliberately NOT the
/// "not authenticated" wording, which would send the user to a re-login the
/// account doesn't need.
fn emit_offline_command_error(
    env: &SkillEnv,
    cmd: &SubscriberCommand,
    detail: &str,
) -> anyhow::Result<()> {
    let error = format!(
        "Telegram connection is temporarily offline: {detail}. Check the network and retry."
    );
    match cmd {
        SubscriberCommand::TelegramListPeers { query, .. } => emit_control(
            &env.topic,
            "peer_list_error",
            json!({ "error": error, "query": query }),
        ),
        SubscriberCommand::TelegramSearchMessages { peer, query, .. } => emit_control(
            &env.topic,
            "message_search_error",
            json!({ "error": error, "peer": peer, "query": query }),
        ),
        SubscriberCommand::TelegramListMessages { peer, .. } => emit_control(
            &env.topic,
            "message_list_error",
            json!({ "error": error, "peer": peer }),
        ),
        SubscriberCommand::SendMessage { peer, .. } => emit_control(
            &env.topic,
            "send_unsupported",
            json!({ "error": error, "peer": peer }),
        ),
        SubscriberCommand::Custom { op, args } if op == "telegram_act" => {
            let action = args
                .get("action")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let input = args.get("input").unwrap_or(args);
            emit_control(
                &env.topic,
                "telegram_act_error",
                json!({
                    "action": action,
                    "peer": input
                        .get("to")
                        .or_else(|| input.get("target"))
                        .or_else(|| input.get("channel"))
                        .or_else(|| input.get("chat"))
                        .or_else(|| input.get("peer"))
                        .and_then(serde_json::Value::as_str),
                    "error": error,
                }),
            )
        }
        _ => Ok(()),
    }
}

async fn import_and_connect(
    env: &SkillEnv,
    options: TdataImportOptions,
) -> anyhow::Result<Option<Client>> {
    let mut outcome = match import_tdata(env, options) {
        Ok(outcome) => outcome,
        Err(error) => {
            emit_control(
                &env.topic,
                "login_error",
                json!({
                    "error": format!("Telegram Desktop import failed: {error}"),
                    "phase": "import_desktop"
                }),
            )?;
            return Ok(None);
        }
    };
    let Some(client) = verify_imported_session(env, &mut outcome).await? else {
        emit_control(
            &env.topic,
            "login_error",
            json!({
                "error": "Telegram import wrote a session, but Telegram did not accept it",
                "phase": "import_verify",
                "import": import_payload(&outcome),
            }),
        )?;
        return Ok(None);
    };
    emit_import_complete(env, &client, &mut outcome).await?;
    Ok(Some(client))
}

async fn verify_imported_session(
    env: &SkillEnv,
    outcome: &mut TdataImportOutcome,
) -> anyhow::Result<Option<Client>> {
    if let SessionResume::Resumed(client) = try_resume_session(env).await? {
        return Ok(Some(client));
    }

    let mut tried = vec![outcome.dc_id];
    for dc_id in outcome.candidate_dc_ids.clone() {
        if tried.contains(&dc_id) {
            continue;
        }
        tried.push(dc_id);
        rewrite_imported_session_dc(env, dc_id)?;
        outcome.dc_id = dc_id;
        if let SessionResume::Resumed(client) = try_resume_session(env).await? {
            return Ok(Some(client));
        }
    }
    Ok(None)
}

fn rewrite_imported_session_dc(env: &SkillEnv, dc_id: i32) -> anyhow::Result<()> {
    let session = Session::load_file(&env.session_path)
        .with_context(|| format!("load session file {}", env.session_path.display()))?;
    let user = session.get_user();
    session.set_user(
        user.as_ref().map(|user| user.id).unwrap_or(0),
        dc_id,
        user.as_ref().map(|user| user.bot).unwrap_or(false),
    );
    session
        .save_to_file(&env.session_path)
        .with_context(|| format!("save session file {}", env.session_path.display()))
}

async fn emit_import_complete(
    env: &SkillEnv,
    client: &Client,
    outcome: &mut TdataImportOutcome,
) -> anyhow::Result<()> {
    let user = client.get_me().await?;
    outcome.user_id = Some(user.id());
    client
        .session()
        .set_user(user.id(), outcome.dc_id, user.is_bot());
    client
        .session()
        .save_to_file(&env.session_path)
        .with_context(|| format!("save session file {}", env.session_path.display()))?;
    emit_control(
        &env.topic,
        "login_complete",
        json!({
            "imported": true,
            "user_id": user.id(),
            "first_name": user.first_name(),
            "import": import_payload(outcome),
        }),
    )?;
    Ok(())
}

fn import_payload(outcome: &TdataImportOutcome) -> serde_json::Value {
    json!({
        "source_kind": outcome.source_kind.as_str(),
        "source_path": outcome.source_path.display().to_string(),
        "account_index": outcome.account_index,
        "accounts_count": outcome.accounts_count,
        "user_id": outcome.user_id,
        "dc_id": outcome.dc_id,
        "session_path": outcome.session_path.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        next_offline_retry_delay, startup_monitoring_boundary, OFFLINE_RESUME_RETRY_INITIAL_DELAY,
        OFFLINE_RESUME_RETRY_MAX_DELAY,
    };
    use std::time::Duration;

    #[test]
    fn startup_monitoring_boundary_uses_offline_start_after_recovery() {
        assert_eq!(
            startup_monitoring_boundary(1_700_000_100_000, Some(1_700_000_000_000)),
            1_700_000_000_000
        );
    }

    #[test]
    fn startup_monitoring_boundary_uses_fresh_boundary_without_recovery() {
        assert_eq!(
            startup_monitoring_boundary(1_700_000_100_000, None),
            1_700_000_100_000
        );
    }

    #[test]
    fn offline_retry_delay_backs_off_until_cap() {
        assert_eq!(
            next_offline_retry_delay(OFFLINE_RESUME_RETRY_INITIAL_DELAY),
            Duration::from_secs(10)
        );
        assert_eq!(
            next_offline_retry_delay(OFFLINE_RESUME_RETRY_MAX_DELAY),
            OFFLINE_RESUME_RETRY_MAX_DELAY
        );
    }
}
