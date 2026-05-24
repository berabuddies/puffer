//! Top-level entrypoint and main event loop.
//!
//! [`run`] is invoked by the hidden `puffer __subscriber telegram-user`
//! subcommand. It orchestrates session loading, the login flow, and the
//! infinite `next_update` loop that emits ndjson message events.

use anyhow::Context as _;
use grammers_client::{session::Session, Client, Config, Update};
use puffer_subscriber_runtime::SubscriberCommand;
use serde_json::json;
use tracing::{error, info, warn};

use crate::actions::handle_telegram_act;
use crate::commands::CommandStream;
use crate::delivery::{catch_up_recent_messages, emit_message_if_new, DeliveryCursor};
use crate::events::emit_control;
use crate::import::{import_tdata, TdataImportOptions, TdataImportOutcome};
use crate::login;
use crate::outbound::handle_send_message;
use crate::peers::{handle_list_peers, handle_search_messages};
use crate::qr_login;
use crate::state::{
    default_init_params, resolve_api_credentials, LoginState, PersistedCredentials, SkillEnv,
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
            SubscriberCommand::TelegramLoginStart {
                phone,
                api_id,
                api_hash,
            } => match login::start(&env, &mut login_state, phone, api_id, api_hash).await? {
                Some(c) => client = Some(c),
                None => continue,
            },
            SubscriberCommand::TelegramQrLoginStart { api_id, api_hash } => {
                if let Some(qr_client) =
                    qr_login::start(&env, &mut qr_state, api_id, api_hash).await?
                {
                    client = Some(qr_client);
                }
            }
            SubscriberCommand::TelegramQrLoginWait { timeout_seconds } => {
                if let Some(qr_client) =
                    qr_login::wait(&env, &mut qr_state, timeout_seconds).await?
                {
                    client = Some(qr_client);
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
                emit_control(
                    &env.topic,
                    "auth_ok",
                    json!({
                        "ok": false,
                        "authenticated": false,
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
            run_update_loop(
                &env,
                &mut commands,
                &mut client,
                &mut login_state,
                &mut qr_state,
                &mut delivery_cursor,
            )
            .await?;
            return Ok(());
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
                if let Some(qr_client) =
                    qr_login::start(&env, &mut qr_state, api_id, api_hash).await?
                {
                    client = qr_client;
                    authorized = true;
                }
            }
            SubscriberCommand::TelegramQrLoginWait { timeout_seconds } => {
                if let Some(qr_client) =
                    qr_login::wait(&env, &mut qr_state, timeout_seconds).await?
                {
                    client = qr_client;
                    authorized = true;
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

/// Tries to open and connect an already-authenticated session. Returns
/// `Ok(None)` when the session file is missing auth material or the client
/// is not currently authorized (e.g. the key was invalidated server-side).
async fn try_resume_session(env: &SkillEnv) -> anyhow::Result<Option<Client>> {
    let session = Session::load_file_or_create(&env.session_path)
        .with_context(|| format!("load session file {}", env.session_path.display()))?;
    if !session.signed_in() {
        return Ok(None);
    }

    // Resume needs the same app credentials used for the original login.
    // Missing credentials are not fatal here; the login phase can surface an
    // actionable error if the operator tries to start a fresh login.
    let persisted = PersistedCredentials::load(&env.credentials_path()).unwrap_or_default();
    let (api_id, api_hash) = match resolve_api_credentials(None, None, &persisted) {
        Ok(pair) => pair,
        Err(error) => {
            warn!(%error, "resume credentials unavailable; falling back to login");
            return Ok(None);
        }
    };

    let config = Config {
        session,
        api_id,
        api_hash,
        params: default_init_params(),
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
    qr_state: &mut Option<qr_login::QrLoginState>,
    delivery_cursor: &mut DeliveryCursor,
) -> anyhow::Result<()> {
    emit_control(&env.topic, "ready", json!({}))?;
    catch_up_recent_messages(env, client, delivery_cursor).await?;
    info!("entering telegram update loop");

    loop {
        tokio::select! {
            biased;
            cmd = commands.next() => {
                let Some(cmd) = cmd? else {
                    info!("stdin closed; shutting down update loop");
                    return Ok(());
                };
                handle_runtime_command(env, client, login_state, qr_state, cmd).await?;
            }
            update = client.next_update() => {
                match update {
                    Ok(Update::NewMessage(msg)) => {
                        if let Err(err) = emit_message_if_new(env, delivery_cursor, &msg) {
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
        }
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
) -> anyhow::Result<()> {
    match cmd {
        SubscriberCommand::TelegramLoginStart {
            phone,
            api_id,
            api_hash,
        } => {
            match emit_already_authorized(env, client).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    warn!(%error, "authorized status probe failed; re-requesting login code");
                }
            }
            let persisted = PersistedCredentials::load(&env.credentials_path()).unwrap_or_default();
            let (resolved_id, resolved_hash) =
                match resolve_api_credentials(api_id, api_hash, &persisted) {
                    Ok(pair) => pair,
                    Err(error) => {
                        emit_control(
                            &env.topic,
                            "login_error",
                            json!({ "error": error.to_string(), "phase": "credentials" }),
                        )?;
                        return Ok(());
                    }
                };
            match client.request_login_code(&phone).await {
                Ok(token) => {
                    login_state.login_token = Some(token);
                    login_state.phone = Some(phone.clone());
                    login_state.api_id = Some(resolved_id);
                    login_state.api_hash = Some(resolved_hash);
                    emit_control(&env.topic, "login_awaiting_code", json!({ "phone": phone }))?;
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
            submit_code_with_reconnect(env, client, login_state, code).await?;
        }
        SubscriberCommand::TelegramLoginSubmitPassword { password } => {
            login::submit_password(env, login_state, client, password).await?;
        }
        SubscriberCommand::TelegramQrLoginStart { api_id, api_hash } => {
            match emit_already_authorized(env, client).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    warn!(%error, "authorized status probe failed; starting qr login");
                }
            }
            if let Some(qr_client) = qr_login::start(env, qr_state, api_id, api_hash).await? {
                *client = qr_client;
            }
        }
        SubscriberCommand::TelegramQrLoginWait { timeout_seconds } => {
            match emit_already_authorized(env, client).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    warn!(%error, "authorized status probe failed; waiting for qr login");
                }
            }
            if let Some(qr_client) = qr_login::wait(env, qr_state, timeout_seconds).await? {
                *client = qr_client;
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
        SubscriberCommand::SendMessage {
            peer,
            text,
            reply_to,
            media,
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
    Ok(())
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
    if let Some(client) = try_resume_session(env).await? {
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
        if let Some(client) = try_resume_session(env).await? {
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

async fn emit_already_authorized(env: &SkillEnv, client: &Client) -> anyhow::Result<()> {
    let user = client.get_me().await?;
    emit_control(
        &env.topic,
        "login_complete",
        json!({
            "already_authorized": true,
            "user_id": user.id(),
            "first_name": user.first_name(),
        }),
    )?;
    Ok(())
}
