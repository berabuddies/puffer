//! Top-level entrypoint and main event loop.
//!
//! [`run`] is invoked by the hidden `puffer __subscriber telegram-user`
//! subcommand. It orchestrates session loading, the login flow, and the
//! infinite `next_update` loop that emits ndjson message events.

use anyhow::Context as _;
use grammers_client::{session::Session, Client};
use puffer_subscriber_runtime::SubscriberCommand;
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};

use crate::actions::handle_telegram_act;
use crate::commands::CommandStream;
use crate::delivery::DeliveryCursor;
use crate::events::emit_control;
use crate::import::{import_tdata, TdataImportOptions, TdataImportOutcome};
use crate::login::LoginSession;
use crate::login_flow::LoginPhase;
use crate::notifications::NotificationMuteCache;
use crate::outbound::handle_send_message;
use crate::peers::{handle_list_messages, handle_list_peers, handle_search_messages};
use crate::qr_login;
use crate::session_resume::{recoverable_live_update_error, try_resume_session, SessionResume};
use crate::state::SkillEnv;
use crate::updates::{handle_live_update, spawn_live_update_task, LiveUpdateEvent};

const OFFLINE_RESUME_RETRY_INITIAL_DELAY: Duration = Duration::from_secs(5);
const OFFLINE_RESUME_RETRY_MAX_DELAY: Duration = Duration::from_secs(60);

enum UpdateLoopExit {
    StdinClosed,
    ReauthStarted,
    /// The live update stream was lost for a recoverable (network-class)
    /// reason. `run()` re-enters the shared offline-parking login phase with
    /// the bounded, jittered retry timer armed instead of killing the process.
    WentOffline(String),
}

enum RuntimeCommandOutcome {
    Continue,
    ReauthStarted,
    ClientReplaced,
}

/// Single-flight guard for the fire-and-forget contact hydration task. A
/// second `TelegramHydrateContacts` command is refused while one hydration is
/// still running; a finished (or absent) handle means the slot is free, so it
/// releases automatically when the spawned task completes.
#[derive(Default)]
pub(crate) struct HydrationSlot {
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl HydrationSlot {
    /// True when no hydration task is currently running.
    fn is_idle(&self) -> bool {
        self.handle
            .as_ref()
            .map_or(true, tokio::task::JoinHandle::is_finished)
    }

    /// Stores the spawned task handle; its completion frees the slot.
    fn attach(&mut self, handle: tokio::task::JoinHandle<()>) {
        self.handle = Some(handle);
    }
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

/// What the login phase produced once an authorized client exists.
struct LoginPhaseOutcome {
    /// Server-time boundary of a recovered offline gap, threaded into the
    /// update loop's monitoring boundary so post-gap messages still deliver.
    recovered_offline_since_ms: Option<i64>,
}

/// Parks the subscriber (bounded jittered offline retries) and/or drives the
/// login command flow until `session.client` is authorized, then returns.
///
/// Shared by startup and by the update loop's `WentOffline` re-entry, so a
/// network drop after months of uptime re-parks on the exact same state
/// machine that a cold start uses. Returns `Ok(None)` when stdin closed before
/// login completed (the caller should exit cleanly).
async fn login_phase(
    env: &SkillEnv,
    commands: &mut CommandStream,
    session: &mut LoginSession,
    mut offline_state: Option<OfflineResumeState>,
    login_required_since: &mut SystemTime,
) -> anyhow::Result<Option<LoginPhaseOutcome>> {
    let mut recovered_offline_since_ms: Option<i64> = None;
    while session.client.is_none() {
        let cmd = if let Some(retry_at) = offline_state.as_ref().map(|state| state.next_retry_at) {
            tokio::select! {
                maybe = commands.next() => maybe?,
                _ = tokio::time::sleep_until(retry_at) => {
                    let state = offline_state
                        .take()
                        .expect("offline state exists for retry timer");
                    match attempt_offline_resume(env, state).await? {
                        OfflineResumeAttempt::Resumed {
                            client: recovered,
                            offline_since_ms,
                        } => {
                            recovered_offline_since_ms = Some(offline_since_ms);
                            session.client = Some(recovered);
                            session.phase = LoginPhase::Authorized;
                        }
                        OfflineResumeAttempt::AuthRequired => {
                            // `login_required` was just emitted: restart the
                            // promoted-session watermark from this moment.
                            *login_required_since = SystemTime::now();
                        }
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
            return Ok(None);
        };
        // Business commands still trigger an immediate retry instead of
        // waiting for the next timer tick, then answer on their own channel if
        // Telegram remains offline.
        if offline_state.is_some() && is_resume_retry_command(&cmd) {
            let state = offline_state
                .take()
                .expect("offline state exists for retry command");
            match attempt_offline_resume(env, state).await? {
                OfflineResumeAttempt::Resumed {
                    client: resumed,
                    offline_since_ms,
                } => {
                    recovered_offline_since_ms = Some(offline_since_ms);
                    session.client = Some(resumed);
                    session.phase = LoginPhase::Authorized;
                    let _ = handle_runtime_command(env, session, cmd).await?;
                    continue;
                }
                OfflineResumeAttempt::AuthRequired => {}
                OfflineResumeAttempt::StillOffline(next_state) => {
                    emit_offline_command_error(env, &cmd, &next_state.detail)?;
                    offline_state = Some(next_state);
                    continue;
                }
            }
        }
        let Some(cmd) = handle_login_command(session, cmd).await? else {
            continue;
        };
        match cmd {
            SubscriberCommand::TelegramAuthOk => {
                // While parked offline the on-disk session is signed in and
                // only the network was unreachable — answering ok:false would
                // degrade the connection as if the login were lost. Probes
                // deliberately do NOT trigger a resume retry; the retry timer
                // is owned by this state machine.
                let offline = offline_state.is_some();
                // An external `puffer connect` login may have promoted a fresh
                // session while we were parked here (#752). Adopt it instead
                // of answering a stale ok:false forever.
                if !offline {
                    if let Some(mtime) =
                        session_file_changed_since(&env.session_path, *login_required_since)
                    {
                        // Advance the watermark first: if this resume fails
                        // (file invalid), don't re-dial on every probe tick.
                        *login_required_since = mtime;
                        if let SessionResume::Resumed(resumed) = try_resume_session(env).await? {
                            emit_control(
                                &env.topic,
                                "auth_ok",
                                json!({ "ok": true, "authenticated": true, "offline": false }),
                            )?;
                            session.client = Some(resumed);
                            session.phase = LoginPhase::Authorized;
                            continue; // exits the login loop; update loop emits `ready`
                        }
                    }
                }
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
            SubscriberCommand::TelegramHydrateContacts { .. } => {
                emit_contacts_auth_required(env)?;
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
            SubscriberCommand::Custom { op, args } => handle_login_custom(env, op, args)?,
            // Login commands were consumed by handle_login_command above.
            _ => {}
        }
    }
    Ok(Some(LoginPhaseOutcome {
        recovered_offline_since_ms,
    }))
}

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
    let mut session = LoginSession::new(env.clone());

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
    // Watermark for detecting a session promoted by an external `puffer
    // connect` login while we are parked in the login phase (#752).
    let mut login_required_since = SystemTime::now();
    match try_resume_session(&env).await? {
        SessionResume::Resumed(c) => {
            emit_control(&env.topic, "ready", json!({ "resumed": true }))?;
            session.client = Some(c);
            session.phase = LoginPhase::Authorized;
        }
        SessionResume::AuthRequired => {
            emit_control(&env.topic, "login_required", json!({}))?;
            login_required_since = SystemTime::now();
        }
        SessionResume::Transient(detail) => {
            let state = OfflineResumeState::new(detail);
            emit_resume_offline(&env, &state)?;
            offline_state = Some(state);
        }
    }

    // Login phase: park offline / drive login until an authorized client
    // exists, or stdin closes. Callable again after the update loop parks us.
    let Some(outcome) = login_phase(
        &env,
        &mut commands,
        &mut session,
        offline_state,
        &mut login_required_since,
    )
    .await?
    else {
        return Ok(());
    };
    let mut recovered_offline_since_ms = outcome.recovered_offline_since_ms;

    let mut delivery_cursor = DeliveryCursor::load(&env)?;
    let mut notification_mutes = NotificationMuteCache::default();

    // Drive the remaining login steps (code + optional 2FA) if needed, then
    // enter the update loop. The client may already be authorized when we
    // resumed a session; the resume path sets the client without an explicit
    // phase, so probe once and converge the phase.
    let mut authorized = session.is_authorized()
        || session
            .client
            .as_ref()
            .expect("client set after login phase")
            .is_authorized()
            .await
            .context("probe initial auth state")?;
    if authorized {
        session.phase = LoginPhase::Authorized;
    }

    loop {
        if authorized {
            match run_update_loop(
                &env,
                &mut commands,
                &mut session,
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
                UpdateLoopExit::WentOffline(detail) => {
                    // The live stream dropped for a network-class reason.
                    // Clear the dead client so the shared login phase re-parks
                    // on the bounded, jittered offline-resume timer instead of
                    // killing the process.
                    session.client = None;
                    let state = OfflineResumeState::new(detail);
                    emit_resume_offline(&env, &state)?;
                    let Some(outcome) = login_phase(
                        &env,
                        &mut commands,
                        &mut session,
                        Some(state),
                        &mut login_required_since,
                    )
                    .await?
                    else {
                        return Ok(());
                    };
                    recovered_offline_since_ms = outcome.recovered_offline_since_ms;
                    authorized = session.is_authorized();
                    continue;
                }
            }
        }

        let cmd = commands.next().await?;
        let Some(cmd) = cmd else {
            info!("stdin closed before login finalized");
            return Ok(());
        };
        let Some(cmd) = handle_login_command(&mut session, cmd).await? else {
            authorized = session.is_authorized();
            continue;
        };
        match cmd {
            SubscriberCommand::TelegramAuthOk => match session.client.as_ref() {
                Some(client) => emit_auth_ok(&env, client).await?,
                None => emit_control(
                    &env.topic,
                    "auth_ok",
                    json!({ "ok": false, "authenticated": false }),
                )?,
            },
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
            SubscriberCommand::TelegramHydrateContacts { .. } => {
                emit_contacts_auth_required(&env)?;
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
            // Login commands were consumed by handle_login_command above.
            _ => {}
        }
    }
}

/// Drives the main event loop: waits for either a Telegram update or an
/// inbound stdin command, handles it, and repeats. Returns when stdin
/// closes or a fatal error occurs.
async fn run_update_loop(
    env: &SkillEnv,
    commands: &mut CommandStream,
    session: &mut LoginSession,
    delivery_cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
    recovered_offline_since_ms: Option<i64>,
) -> anyhow::Result<UpdateLoopExit> {
    // grammers `Client` is a cheap handle — clones share the connection.
    let mut client = session
        .client
        .clone()
        .expect("update loop requires a client");
    emit_control(&env.topic, "ready", json!({}))?;
    // Monitoring is promised from this moment on: messages dated after this
    // boundary must reach the triage pipeline even when they arrive while
    // startup hydration is still running (see hydrate_dialog_state).
    let live_since_ms =
        startup_monitoring_boundary(monitoring_live_since_ms(), recovered_offline_since_ms);
    reset_delivery_cursor_for_current_account(&client, delivery_cursor).await?;
    if let Some(exit) = hydrate_startup_state_before_updates(
        env,
        commands,
        session,
        delivery_cursor,
        notification_mutes,
        live_since_ms,
    )
    .await?
    {
        return Ok(exit);
    }
    // Hydration may have replaced the client (e.g. a runtime tdata import).
    client = session
        .client
        .clone()
        .expect("update loop requires a client");
    persist_live_session_state(env, &client);
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
                        // #604: network-class loss. Park on the shared offline
                        // state machine (bounded jittered retries) instead of a
                        // single in-place resume that killed the process on
                        // failure.
                        warn!(%error, "telegram live update stream lost; parking offline");
                        return Ok(UpdateLoopExit::WentOffline(error.to_string()));
                    }
                    // Auth-class loss: drop the dead client and fall back to the
                    // login flow rather than terminating the subscriber.
                    error!(%error, "telegram live update stream lost; reauthenticating");
                    session.client = None;
                    return Ok(UpdateLoopExit::ReauthStarted);
                }
                handle_live_update(
                    env,
                    delivery_cursor,
                    notification_mutes,
                    update,
                )
                .await?;
                persist_live_session_state(env, &client);
            }
            cmd = commands.next() => {
                let Some(cmd) = cmd? else {
                    info!("stdin closed; shutting down update loop");
                    live_task.abort();
                    return Ok(UpdateLoopExit::StdinClosed);
                };
                match handle_runtime_command(env, session, cmd).await? {
                    RuntimeCommandOutcome::Continue => {}
                    RuntimeCommandOutcome::ReauthStarted => {
                        live_task.abort();
                        return Ok(UpdateLoopExit::ReauthStarted);
                    }
                    RuntimeCommandOutcome::ClientReplaced => {
                        live_task.abort();
                        client = session
                            .client
                            .clone()
                            .expect("update loop requires a client");
                        reset_delivery_cursor_for_current_account(&client, delivery_cursor).await?;
                        if let Some(exit) = hydrate_startup_state_before_updates(
                            env,
                            commands,
                            session,
                            delivery_cursor,
                            notification_mutes,
                            live_since_ms,
                        )
                        .await?
                        {
                            return Ok(exit);
                        }
                        client = session
                            .client
                            .clone()
                            .expect("update loop requires a client");
                        persist_live_session_state(env, &client);
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
    session: &mut LoginSession,
    delivery_cursor: &mut DeliveryCursor,
    notification_mutes: &mut NotificationMuteCache,
    live_since_ms: i64,
) -> anyhow::Result<Option<UpdateLoopExit>> {
    let client = session
        .client
        .clone()
        .expect("startup hydration requires a client");
    let mut hydration = spawn_startup_hydration(
        env,
        &client,
        delivery_cursor,
        notification_mutes,
        live_since_ms,
    );
    loop {
        tokio::select! {
            biased;
            cmd = commands.next() => match cmd? {
                Some(cmd) => {
                    match handle_runtime_command(env, session, cmd).await? {
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
                            let client = session
                                .client
                                .clone()
                                .expect("startup hydration requires a client");
                            reset_delivery_cursor_for_current_account(&client, delivery_cursor)
                                .await?;
                            info!("restarting telegram startup hydration for replaced client");
                            hydration = spawn_startup_hydration(
                                env,
                                &client,
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

/// Answers a `TelegramHydrateContacts` request received while not authorized:
/// hydration needs a live client, so the caller must complete login first.
fn emit_contacts_auth_required(env: &SkillEnv) -> anyhow::Result<()> {
    emit_control(
        &env.topic,
        "contacts_hydrated",
        json!({ "ok": false, "state": "auth_required" }),
    )
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

/// Next offline-resume backoff with full jitter: the delay doubles (capped at
/// [`OFFLINE_RESUME_RETRY_MAX_DELAY`]) then lands uniformly in `[doubled/2,
/// doubled]`, so many subscribers reconnecting after the same outage spread
/// their retries instead of stampeding. Uses `SystemTime` subsec nanos as a
/// cheap entropy source (no `rand` dependency).
fn next_offline_retry_delay(current: Duration) -> Duration {
    let doubled = current
        .saturating_mul(2)
        .min(OFFLINE_RESUME_RETRY_MAX_DELAY);
    let half = doubled / 2;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let span_ms = half.as_millis().max(1) as u64;
    half + Duration::from_millis(nanos % (span_ms + 1))
}

/// Stops an in-flight hydration task and waits for it to wind down so a
/// successor never runs concurrently against the same on-disk state.
async fn abort_startup_hydration(
    hydration: tokio::task::JoinHandle<anyhow::Result<StartupHydrationState>>,
) {
    hydration.abort();
    let _ = hydration.await;
}

/// Detects an externally promoted session: `puffer connect` renames its
/// staging file onto the live path after a successful login (#551), which
/// carries an mtime newer than the moment we parked in the login phase.
fn session_file_changed_since(path: &std::path::Path, since: SystemTime) -> Option<SystemTime> {
    let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
    (mtime > since).then_some(mtime)
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

/// Dispatches the six login-flow commands to the session; hands anything
/// else back to the caller. All three command loops share this, so a login
/// transition behaves identically regardless of which loop received it.
async fn handle_login_command(
    session: &mut LoginSession,
    cmd: SubscriberCommand,
) -> anyhow::Result<Option<SubscriberCommand>> {
    match cmd {
        SubscriberCommand::TelegramLoginStart {
            phone,
            api_id,
            api_hash,
        } => {
            session.start(phone, api_id, api_hash).await?;
        }
        SubscriberCommand::TelegramQrLoginStart { api_id, api_hash } => {
            qr_login::start(session, api_id, api_hash).await?;
        }
        SubscriberCommand::TelegramQrLoginWait { timeout_seconds } => {
            qr_login::wait(session, timeout_seconds).await?;
        }
        SubscriberCommand::TelegramLoginSubmitCode { code } => {
            session.submit_code(code).await?;
        }
        SubscriberCommand::TelegramLoginSubmitPassword { password } => {
            session.submit_password(password).await?;
        }
        SubscriberCommand::TelegramImportTdata {
            path,
            passcode,
            account_index,
            key_file,
        } => {
            let env = session.env.clone();
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
                session.client = Some(imported);
                session.phase = LoginPhase::Authorized;
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}

/// Which of the six login commands a runtime command was, for outcome mapping.
enum LoginCommandKind {
    Start,
    QrStart,
    QrWait,
    SubmitCode,
    SubmitPassword,
    Import,
}

impl LoginCommandKind {
    fn of(cmd: &SubscriberCommand) -> Option<Self> {
        Some(match cmd {
            SubscriberCommand::TelegramLoginStart { .. } => Self::Start,
            SubscriberCommand::TelegramQrLoginStart { .. } => Self::QrStart,
            SubscriberCommand::TelegramQrLoginWait { .. } => Self::QrWait,
            SubscriberCommand::TelegramLoginSubmitCode { .. } => Self::SubmitCode,
            SubscriberCommand::TelegramLoginSubmitPassword { .. } => Self::SubmitPassword,
            SubscriberCommand::TelegramImportTdata { .. } => Self::Import,
            _ => return None,
        })
    }
}

/// Maps a handled login command to the update loop's control-flow outcome,
/// mirroring the per-arm behavior the loops had before `LoginSession`.
fn runtime_login_outcome(
    kind: LoginCommandKind,
    pre_phase: &'static str,
    session: &LoginSession,
) -> RuntimeCommandOutcome {
    match kind {
        LoginCommandKind::Start if matches!(session.phase, LoginPhase::CodeSent { .. }) => {
            RuntimeCommandOutcome::ReauthStarted
        }
        LoginCommandKind::QrStart | LoginCommandKind::QrWait => match session.phase {
            // qr_login resets the phase on entry, so landing on `authorized`
            // means THIS call completed the QR flow — except the wrong-phase
            // reject in `wait`, which leaves the previous phase untouched.
            LoginPhase::Authorized if pre_phase != "authorized" => {
                RuntimeCommandOutcome::ClientReplaced
            }
            LoginPhase::PasswordPending { .. } => RuntimeCommandOutcome::ReauthStarted,
            _ => RuntimeCommandOutcome::Continue,
        },
        // Import success replaces the client. A failed import while already
        // authorized also lands here; the extra rehydration is harmless.
        LoginCommandKind::Import if session.is_authorized() => {
            RuntimeCommandOutcome::ClientReplaced
        }
        _ => RuntimeCommandOutcome::Continue,
    }
}

/// Handles a stdin command received while the update loop is running.
///
/// Most login-related commands are unexpected here (login already succeeded)
/// but we still accept them to support re-authentication without a restart.
async fn handle_runtime_command(
    env: &SkillEnv,
    session: &mut LoginSession,
    cmd: SubscriberCommand,
) -> anyhow::Result<RuntimeCommandOutcome> {
    let login_kind = LoginCommandKind::of(&cmd);
    let pre_phase = session.phase.name();
    let Some(cmd) = handle_login_command(session, cmd).await? else {
        let kind = login_kind.expect("commands consumed by handle_login_command are login kinds");
        return Ok(runtime_login_outcome(kind, pre_phase, session));
    };
    match cmd {
        SubscriberCommand::TelegramAuthOk => match session.client.as_ref() {
            Some(client) => emit_auth_ok(env, client).await?,
            None => emit_control(
                &env.topic,
                "auth_ok",
                json!({ "ok": false, "authenticated": false }),
            )?,
        },
        SubscriberCommand::TelegramListPeers {
            query,
            peer_kind,
            limit,
        } => {
            let client = runtime_client(session)?;
            handle_list_peers(env, &client, query, peer_kind, limit).await?;
        }
        SubscriberCommand::TelegramHydrateContacts { target } => {
            if session.hydration.is_idle() {
                // Acquire the client BEFORE spawning so a failure here leaves
                // the slot untouched (nothing was attached — still idle).
                let client = runtime_client(session)?;
                let handle = tokio::spawn(crate::peer_cache::run_contact_hydration(
                    env.clone(),
                    client,
                    target,
                ));
                session.hydration.attach(handle);
            } else {
                emit_control(
                    &env.topic,
                    "contacts_hydrated",
                    json!({ "ok": false, "state": "hydrating" }),
                )?;
            }
        }
        SubscriberCommand::TelegramSearchMessages {
            peer,
            query,
            limit,
            context,
            succinct,
        } => {
            let client = runtime_client(session)?;
            handle_search_messages(env, &client, peer, query, limit, context, succinct).await?;
        }
        SubscriberCommand::TelegramListMessages {
            peer,
            limit,
            before_id,
            sender,
            scan_limit,
            succinct,
        } => {
            let client = runtime_client(session)?;
            handle_list_messages(
                env, &client, peer, limit, before_id, sender, scan_limit, succinct,
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
            let client = runtime_client(session)?;
            handle_send_message(env, &client, peer, text, reply_to, media).await?;
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
                let client = runtime_client(session)?;
                handle_telegram_act(env, &client, args).await?;
            } else {
                emit_control(
                    &env.topic,
                    "login_error",
                    json!({ "error": format!("command not understood: {op}") }),
                )?;
            }
        }
        // Login commands were consumed by handle_login_command above.
        _ => {}
    }
    Ok(RuntimeCommandOutcome::Continue)
}

/// Business commands in the runtime loops always run against the session's
/// live client handle; the update loop only runs with a client present.
fn runtime_client(session: &LoginSession) -> anyhow::Result<Client> {
    session
        .client
        .clone()
        .context("runtime command requires a connected telegram client")
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
                    "phase": "import_desktop",
                    "class": "import",
                }),
            )?;
            return Ok(None);
        }
    };
    let client = match verify_imported_session(env, &mut outcome).await? {
        VerifyOutcome::Connected(client) => client,
        VerifyOutcome::Failed(failure) => {
            let (class, error) = verify_failure_login_error(&failure);
            emit_control(
                &env.topic,
                "login_error",
                json!({
                    "error": error,
                    "phase": "import_verify",
                    "class": class,
                    "import": import_payload(&outcome),
                }),
            )?;
            return Ok(None);
        }
    };
    emit_import_complete(env, &client, &mut outcome).await?;
    Ok(Some(client))
}

/// Why a post-import session verification did not connect.
enum VerifyFailure {
    /// Could not reach Telegram (network/proxy/DC unreachable) — retryable.
    Unreachable(String),
    /// Reached Telegram but the imported session was not accepted (re-login).
    Rejected,
}

/// Result of verifying an imported session against Telegram.
enum VerifyOutcome {
    Connected(Client),
    Failed(VerifyFailure),
}

/// Maps a verify failure to (`class`, user-facing message) for the
/// `login_error` event the connector-setup UI shows.
fn verify_failure_login_error(failure: &VerifyFailure) -> (&'static str, String) {
    match failure {
        VerifyFailure::Unreachable(detail) => (
            "network",
            format!(
                "Couldn't reach Telegram to verify the import (likely a network/proxy block): \
                 {detail}. The local session was imported; check your network/proxy and retry."
            ),
        ),
        VerifyFailure::Rejected => (
            "auth",
            "Telegram import wrote a session, but Telegram did not accept it; re-login may be required."
                .to_string(),
        ),
    }
}

async fn verify_imported_session(
    env: &SkillEnv,
    outcome: &mut TdataImportOutcome,
) -> anyhow::Result<VerifyOutcome> {
    // The first attempt against the imported DC is the only path that can fall
    // through to the candidate-DC loop, so it always seeds `last_unreachable`
    // with a real failure detail; no synthetic fallback is needed.
    let mut last_unreachable = match try_resume_session(env).await? {
        SessionResume::Resumed(client) => return Ok(VerifyOutcome::Connected(client)),
        SessionResume::AuthRequired => return Ok(VerifyOutcome::Failed(VerifyFailure::Rejected)),
        SessionResume::Transient(detail) => detail,
    };

    let mut tried = vec![outcome.dc_id];
    for dc_id in outcome.candidate_dc_ids.clone() {
        if tried.contains(&dc_id) {
            continue;
        }
        tried.push(dc_id);
        rewrite_imported_session_dc(env, dc_id)?;
        outcome.dc_id = dc_id;
        match try_resume_session(env).await? {
            SessionResume::Resumed(client) => return Ok(VerifyOutcome::Connected(client)),
            SessionResume::AuthRequired => {
                return Ok(VerifyOutcome::Failed(VerifyFailure::Rejected))
            }
            SessionResume::Transient(detail) => last_unreachable = detail,
        }
    }

    Ok(VerifyOutcome::Failed(VerifyFailure::Unreachable(
        last_unreachable,
    )))
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
        next_offline_retry_delay, runtime_login_outcome, session_file_changed_since,
        startup_monitoring_boundary, verify_failure_login_error, HydrationSlot, LoginCommandKind,
        RuntimeCommandOutcome, VerifyFailure, OFFLINE_RESUME_RETRY_INITIAL_DELAY,
        OFFLINE_RESUME_RETRY_MAX_DELAY,
    };

    #[tokio::test]
    async fn hydrate_request_is_single_flight() {
        let mut slot = HydrationSlot::default();
        assert!(slot.is_idle()); // an empty slot accepts the first request

        slot.attach(tokio::spawn(std::future::pending::<()>()));
        assert!(!slot.is_idle()); // refused while the task is running

        slot.handle.as_ref().expect("handle attached").abort();
        for _ in 0..200 {
            if slot.is_idle() {
                return; // task completion frees the slot automatically
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("hydration slot never freed after task completion");
    }
    use crate::login::LoginSession;
    use crate::login_flow::LoginPhase;
    use crate::state::SkillEnv;
    use std::time::Duration;

    fn session_with_phase(
        temp: &tempfile::TempDir,
        phase: crate::login::LivePhase,
    ) -> LoginSession {
        let mut session = LoginSession::new(SkillEnv {
            state_dir: temp.path().to_path_buf(),
            session_path: temp.path().join("telegram.session"),
            topic: "telegram-user".to_string(),
            workspace_config_dir: None,
            live_session_path: None,
        });
        session.phase = phase;
        session
    }

    /// Pins the post-`handle_login_command` outcome mapping to the per-arm
    /// behavior the command loops had before `LoginSession` (QR completion
    /// replaces the client, QR 2FA starts reauth, wrong-phase rejects and
    /// pending states continue). `CodeSent`/`PasswordPending` phases carry
    /// grammers tokens that cannot be constructed in tests; their arms are
    /// covered by the login_flow transition table instead.
    #[test]
    fn runtime_login_outcome_maps_phase_transitions() {
        let temp = tempfile::tempdir().unwrap();

        let authorized = session_with_phase(&temp, LoginPhase::Authorized);
        // QR flow completed within this call: the session client was replaced.
        assert!(matches!(
            runtime_login_outcome(LoginCommandKind::QrWait, "qr_pending", &authorized),
            RuntimeCommandOutcome::ClientReplaced
        ));
        // Wrong-phase reject in `wait` leaves an authorized phase untouched.
        assert!(matches!(
            runtime_login_outcome(LoginCommandKind::QrWait, "authorized", &authorized),
            RuntimeCommandOutcome::Continue
        ));
        // Successful tdata import always replaces the client.
        assert!(matches!(
            runtime_login_outcome(LoginCommandKind::Import, "authorized", &authorized),
            RuntimeCommandOutcome::ClientReplaced
        ));

        let qr_pending = session_with_phase(
            &temp,
            LoginPhase::QrPending {
                dc_id: 2,
                refreshes: 0,
            },
        );
        // QR token emitted, waiting for approval: stay in the current loop.
        assert!(matches!(
            runtime_login_outcome(LoginCommandKind::QrStart, "authorized", &qr_pending),
            RuntimeCommandOutcome::Continue
        ));

        let idle = session_with_phase(&temp, LoginPhase::Idle);
        // Failed login start / import keeps the update loop running.
        assert!(matches!(
            runtime_login_outcome(LoginCommandKind::Start, "authorized", &idle),
            RuntimeCommandOutcome::Continue
        ));
        assert!(matches!(
            runtime_login_outcome(LoginCommandKind::Import, "authorized", &idle),
            RuntimeCommandOutcome::Continue
        ));
        assert!(matches!(
            runtime_login_outcome(LoginCommandKind::SubmitCode, "code_sent", &idle),
            RuntimeCommandOutcome::Continue
        ));
        assert!(matches!(
            runtime_login_outcome(LoginCommandKind::SubmitPassword, "password_pending", &idle),
            RuntimeCommandOutcome::Continue
        ));
    }

    #[test]
    fn session_file_changed_since_detects_promotion() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("telegram.session");
        let before = std::time::SystemTime::now() - std::time::Duration::from_secs(60);

        assert!(
            session_file_changed_since(&path, before).is_none(),
            "missing file"
        );

        std::fs::write(&path, b"session").unwrap();
        let mtime = session_file_changed_since(&path, before).expect("newer file detected");
        assert!(
            session_file_changed_since(&path, mtime).is_none(),
            "not newer than its own mtime"
        );
    }

    #[test]
    fn verify_failure_message_distinguishes_network_from_rejection() {
        let (class, msg) =
            verify_failure_login_error(&VerifyFailure::Unreachable("io timeout".into()));
        assert_eq!(class, "network");
        assert!(msg.contains("network/proxy"), "network message: {msg}");

        let (class, msg) = verify_failure_login_error(&VerifyFailure::Rejected);
        assert_eq!(class, "auth");
        assert!(msg.contains("did not accept"), "rejected message: {msg}");
    }

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
        // 5s → doubled 10s, then full-jitter into [5s, 10s].
        let from_initial = next_offline_retry_delay(OFFLINE_RESUME_RETRY_INITIAL_DELAY);
        assert!(from_initial >= Duration::from_secs(5));
        assert!(from_initial <= Duration::from_secs(10));
        // At the cap the jitter stays within [30s, 60s] and never exceeds it.
        let from_max = next_offline_retry_delay(OFFLINE_RESUME_RETRY_MAX_DELAY);
        assert!(from_max >= OFFLINE_RESUME_RETRY_MAX_DELAY / 2);
        assert!(from_max <= OFFLINE_RESUME_RETRY_MAX_DELAY);
    }

    #[test]
    fn offline_retry_delay_jitter_stays_in_bounds() {
        for _ in 0..100 {
            let next = next_offline_retry_delay(Duration::from_secs(10));
            assert!(next >= Duration::from_secs(10)); // doubled lower bound = 20/2
            assert!(next <= Duration::from_secs(20));
        }
        // cap still enforced
        assert!(
            next_offline_retry_delay(OFFLINE_RESUME_RETRY_MAX_DELAY)
                <= OFFLINE_RESUME_RETRY_MAX_DELAY
        );
    }
}
