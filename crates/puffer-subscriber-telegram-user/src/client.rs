//! Top-level entrypoint and main event loop.
//!
//! [`run`] is invoked by the hidden `puffer __subscriber telegram-user`
//! subcommand. It orchestrates session loading, the login flow, and the
//! infinite `next_update` loop that emits ndjson message events.

use anyhow::Context as _;
use grammers_client::{session::Session, Client, Config, InitParams, Update};
use puffer_subscriber_runtime::SubscriberCommand;
use serde_json::json;
use tracing::{error, info, warn};

use crate::commands::CommandStream;
use crate::events::{build_message_event, emit, emit_control};
use crate::login;
use crate::state::{
    resolve_api_hash, resolve_api_id, LoginState, PersistedCredentials, SkillEnv,
};

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
    info!(
        session = %env.session_path.display(),
        topic = %env.topic,
        "starting telegram-user subscriber"
    );

    if let Some(parent) = env.session_path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("create session parent dir {}", parent.display())
            })?;
        }
    }

    let mut commands = CommandStream::new();
    let mut login_state = LoginState::new();

    // Attempt to reuse a pre-authenticated session first. If the session file
    // holds a valid auth key we can go straight to the update loop without
    // prompting the agent for login credentials.
    let mut client = match try_resume_session(&env).await? {
        Some(c) => {
            emit_control(&env.topic, "ready", json!({ "resumed": true }))?;
            Some(c)
        }
        None => {
            emit_control(&env.topic, "login_required", json!({}))?;
            None
        }
    };

    // Login phase: if we don't have an authorized client yet, keep reading
    // stdin commands until login completes or stdin closes.
    while client.as_ref().map(|_| false).unwrap_or(true) {
        let cmd = tokio::select! {
            maybe = commands.next() => maybe?,
        };
        let Some(cmd) = cmd else {
            info!("stdin closed before login completed");
            return Ok(());
        };
        match cmd {
            SubscriberCommand::TelegramLoginStart { phone, api_id, api_hash } => {
                match login::start(&env, &mut login_state, phone, api_id, api_hash).await? {
                    Some(c) => client = Some(c),
                    None => continue,
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
            SubscriberCommand::Custom { op, .. } => {
                emit_control(
                    &env.topic,
                    "login_error",
                    json!({
                        "error": format!("command not understood: {op}")
                    }),
                )?;
            }
        }
    }

    let mut client = client.expect("client set after login phase");

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
            run_update_loop(&env, &mut commands, &mut client, &mut login_state).await?;
            return Ok(());
        }

        let cmd = commands.next().await?;
        let Some(cmd) = cmd else {
            info!("stdin closed before login finalized");
            return Ok(());
        };
        match cmd {
            SubscriberCommand::TelegramLoginSubmitCode { code } => {
                if login::submit_code(&env, &mut login_state, &client, code).await? {
                    authorized = true;
                }
            }
            SubscriberCommand::TelegramLoginSubmitPassword { password } => {
                if login::submit_password(&env, &mut login_state, &client, password).await? {
                    authorized = true;
                }
            }
            SubscriberCommand::TelegramLoginStart { phone, api_id, api_hash } => {
                // Operator restarted the flow; re-request the login code on
                // the same connection.
                login::start(&env, &mut login_state, phone, api_id, api_hash).await?;
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
            SubscriberCommand::Custom { op, .. } => {
                emit_control(
                    &env.topic,
                    "login_error",
                    json!({
                        "error": format!("command not understood: {op}")
                    }),
                )?;
            }
        }
    }
}

/// Tries to open and connect an already-authenticated session. Returns
/// `Ok(None)` when the session file is missing auth material or the client
/// is not currently authorized (e.g. the key was invalidated server-side).
async fn try_resume_session(env: &SkillEnv) -> anyhow::Result<Option<Client>> {
    let session = Session::load_file_or_create(&env.session_path)
        .with_context(|| format!("load session file {}", env.session_path.display()))?;
    if !session.signed_in() {
        return Ok(None);
    }

    // Resume needs api credentials for the `Config`. Resolution order:
    // persisted credentials (written on the last successful login), env
    // vars, then Telegram Desktop's published default. With the default
    // baked in the resume path always has *something* to try.
    let persisted = PersistedCredentials::load(&env.credentials_path()).unwrap_or_default();
    let api_id = resolve_api_id(None, &persisted);
    let api_hash = resolve_api_hash(None, &persisted);

    let config = Config {
        session,
        api_id,
        api_hash,
        params: InitParams {
            catch_up: true,
            ..Default::default()
        },
    };
    let client = match Client::connect(config).await {
        Ok(c) => c,
        Err(err) => {
            warn!(error = %err, "resume connect failed; falling back to login");
            return Ok(None);
        }
    };
    match client.is_authorized().await {
        Ok(true) => Ok(Some(client)),
        Ok(false) => Ok(None),
        Err(err) => {
            warn!(error = %err, "is_authorized probe failed; falling back to login");
            Ok(None)
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
) -> anyhow::Result<()> {
    emit_control(&env.topic, "ready", json!({}))?;
    info!("entering telegram update loop");

    loop {
        tokio::select! {
            update = client.next_update() => {
                match update {
                    Ok(Update::NewMessage(msg)) => {
                        let event = build_message_event(&env.topic, &msg);
                        if let Err(err) = emit(&event) {
                            error!(error = %err, "failed to emit message event");
                        }
                    }
                    Ok(_) => {
                        // Silently ignore other update kinds for now. They
                        // can be surfaced in later revisions (edits,
                        // deletions, callback queries, ...).
                    }
                    Err(err) => {
                        error!(error = %err, "next_update failed");
                        // Propagate so the supervisor can restart us with
                        // backoff; the session file preserves our auth key.
                        return Err(anyhow::anyhow!("next_update: {err}"));
                    }
                }
            }
            cmd = commands.next() => {
                let Some(cmd) = cmd? else {
                    info!("stdin closed; shutting down update loop");
                    return Ok(());
                };
                handle_runtime_command(env, client, login_state, cmd).await?;
            }
        }
    }
}

/// Handles a stdin command received while the update loop is running.
///
/// Most login-related commands are unexpected here (login already succeeded)
/// but we still accept them to support re-authentication without a restart.
async fn handle_runtime_command(
    env: &SkillEnv,
    client: &Client,
    login_state: &mut LoginState,
    cmd: SubscriberCommand,
) -> anyhow::Result<()> {
    match cmd {
        SubscriberCommand::TelegramLoginStart { phone, api_id, api_hash } => {
            // Re-request a code on the live client rather than reconnecting.
            // The effect is that the running session keeps serving updates
            // until sign_in succeeds and overwrites the auth.
            let persisted = PersistedCredentials::load(&env.credentials_path())
                .unwrap_or_default();
            let resolved_id = resolve_api_id(api_id, &persisted);
            let resolved_hash = resolve_api_hash(api_hash, &persisted);
            match client.request_login_code(&phone).await {
                Ok(token) => {
                    login_state.login_token = Some(token);
                    login_state.phone = Some(phone.clone());
                    login_state.api_id = Some(resolved_id);
                    login_state.api_hash = Some(resolved_hash);
                    emit_control(
                        &env.topic,
                        "login_awaiting_code",
                        json!({ "phone": phone }),
                    )?;
                }
                Err(err) => {
                    warn!(error = %err, "runtime request_login_code failed");
                    emit_control(
                        &env.topic,
                        "login_error",
                        json!({ "error": format!("request_login_code failed: {err}") }),
                    )?;
                }
            }
        }
        SubscriberCommand::TelegramLoginSubmitCode { code } => {
            login::submit_code(env, login_state, client, code).await?;
        }
        SubscriberCommand::TelegramLoginSubmitPassword { password } => {
            login::submit_password(env, login_state, client, password).await?;
        }
        SubscriberCommand::SendMessage { peer, text } => {
            handle_send_message(env, client, peer, text).await?;
        }
        SubscriberCommand::EmailConfigure { .. } => {
            emit_control(
                &env.topic,
                "command_ignored",
                json!({"error": "telegram-user does not handle email configuration"}),
            )?;
        }
        SubscriberCommand::Custom { op, .. } => {
            emit_control(
                &env.topic,
                "login_error",
                json!({ "error": format!("command not understood: {op}") }),
            )?;
        }
    }
    Ok(())
}

/// Resolves `peer` and sends `text` through the connected client.
///
/// Supported peer formats:
/// * `@username` — resolved through `Client::resolve_username`.
/// * Numeric chat id (e.g. `123456789` or `-100123456789`) — resolved by
///   walking the cached dialog list, since `send_message` needs a fully
///   packed chat (with access_hash) and grammers does not synthesize one
///   from a bare id.
///
/// On success a `send_complete` control event is emitted with the
/// resolved chat id and text byte count. On failure a `send_error` event
/// carries the underlying message.
async fn handle_send_message(
    env: &SkillEnv,
    client: &Client,
    peer: String,
    text: String,
) -> anyhow::Result<()> {
    let resolved = match resolve_peer(client, &peer).await {
        Ok(chat) => chat,
        Err(error) => {
            emit_control(
                &env.topic,
                "send_error",
                json!({ "peer": peer, "error": error.to_string() }),
            )?;
            return Ok(());
        }
    };
    match client.send_message(resolved.clone(), text.as_str()).await {
        Ok(_) => {
            emit_control(
                &env.topic,
                "send_complete",
                json!({
                    "peer": peer,
                    "chat_id": resolved.id(),
                    "bytes": text.len(),
                }),
            )?;
        }
        Err(error) => {
            emit_control(
                &env.topic,
                "send_error",
                json!({ "peer": peer, "error": format!("{error}") }),
            )?;
        }
    }
    Ok(())
}

async fn resolve_peer(
    client: &Client,
    peer: &str,
) -> anyhow::Result<grammers_client::types::Chat> {
    let trimmed = peer.trim();
    if let Some(handle) = trimmed.strip_prefix('@') {
        let chat = client
            .resolve_username(handle)
            .await
            .with_context(|| format!("resolve_username `@{handle}` failed"))?;
        return chat.ok_or_else(|| anyhow::anyhow!("no chat found for @{handle}"));
    }
    if let Ok(target_id) = trimmed.parse::<i64>() {
        let mut iter = client.iter_dialogs();
        while let Some(dialog) = iter
            .next()
            .await
            .with_context(|| "iter_dialogs failed while resolving numeric peer")?
        {
            if dialog.chat().id() == target_id {
                return Ok(dialog.chat().clone());
            }
        }
        return Err(anyhow::anyhow!(
            "numeric peer {target_id} not in cached dialogs; have the user open the chat in Telegram once and try again"
        ));
    }
    Err(anyhow::anyhow!(
        "peer `{peer}` is not a recognized format; use @username or a numeric chat id"
    ))
}
