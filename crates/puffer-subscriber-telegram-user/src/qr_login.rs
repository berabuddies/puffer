//! Telegram QR-login support.
//!
//! Grammers 0.7 does not expose a high-level QR login helper, but the pinned
//! TL schema includes the raw `auth.exportLoginToken` and
//! `auth.importLoginToken` calls. This module wraps those calls behind the
//! subscriber command protocol and persists the same session file used by the
//! phone-code login path.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use grammers_client::{
    session::Session,
    types::{PasswordToken, User},
    Client, Config, InvocationError,
};
use grammers_tl_types as tl;
use serde_json::json;
use tokio::time::{timeout_at, Instant};
use tracing::{info, warn};

use crate::events::emit_control;
use crate::login;
use crate::state::{
    default_init_params, resolve_api_credentials, LoginState, PersistedCredentials, SkillEnv,
};

const DEFAULT_QR_WAIT_SECONDS: u64 = 120;
const DEFAULT_DC_ID: i32 = 2;
const MAX_QR_MIGRATIONS: usize = 4;

/// In-memory state for a QR login attempt between `login-qr` and
/// `login-qr-wait`.
pub struct QrLoginState {
    client: Client,
    api_id: i32,
    api_hash: String,
    dc_id: i32,
}

/// Result produced by a QR-login command.
pub enum QrLoginOutcome {
    /// The QR flow is still pending, failed terminally with an emitted error,
    /// or refreshed the QR token for another wait attempt.
    Pending,
    /// Telegram accepted QR approval but requires the account's 2FA password.
    AwaitingPassword(Client),
    /// Telegram accepted QR approval and returned an authorized client.
    Complete(Client),
}

/// Starts QR login and emits either `login_qr`, `login_complete`, or
/// `login_error`.
pub async fn start(
    env: &SkillEnv,
    login_state: &mut LoginState,
    state: &mut Option<QrLoginState>,
    api_id: Option<i32>,
    api_hash: Option<String>,
) -> anyhow::Result<QrLoginOutcome> {
    *state = None;
    login_state.login_token = None;
    login_state.password_token = None;
    login_state.phone = None;
    login_state.api_id = None;
    login_state.api_hash = None;
    let persisted = PersistedCredentials::load(&env.credentials_path()).unwrap_or_default();
    let (api_id, api_hash) = match resolve_api_credentials(api_id, api_hash, &persisted) {
        Ok(pair) => pair,
        Err(error) => {
            warn!(%error, "telegram qr credential resolution failed");
            emit_control(
                &env.topic,
                "login_error",
                json!({ "error": error.to_string(), "phase": "qr_credentials" }),
            )?;
            return Ok(QrLoginOutcome::Pending);
        }
    };

    let client = match connect_qr_client(api_id, api_hash.clone(), None).await {
        Ok(client) => client,
        Err(error) => {
            warn!(%error, "telegram qr connect failed");
            emit_control(
                &env.topic,
                "login_error",
                json!({ "error": format!("connect failed: {error:#}"), "phase": "qr_connect" }),
            )?;
            return Ok(QrLoginOutcome::Pending);
        }
    };

    let token = match export_login_token(&client, api_id, &api_hash).await {
        Ok(token) => token,
        Err(error) => {
            warn!(%error, "telegram qr export token failed");
            emit_control(
                &env.topic,
                "login_error",
                json!({ "error": format!("export login token failed: {error:#}"), "phase": "qr_export" }),
            )?;
            return Ok(QrLoginOutcome::Pending);
        }
    };

    let qr = QrLoginState {
        client,
        api_id,
        api_hash,
        dc_id: DEFAULT_DC_ID,
    };
    handle_login_token(env, login_state, state, qr, token).await
}

/// Waits for approval of the active QR login. If the token expires before
/// approval, this emits a refreshed `login_qr` and keeps the QR state alive.
pub async fn wait(
    env: &SkillEnv,
    login_state: &mut LoginState,
    state: &mut Option<QrLoginState>,
    timeout_seconds: Option<u64>,
) -> anyhow::Result<QrLoginOutcome> {
    let Some(qr) = state.take() else {
        emit_control(
            &env.topic,
            "login_error",
            json!({
                "error": "no QR login in progress; run telegram login-qr first",
                "phase": "qr_wait"
            }),
        )?;
        return Ok(QrLoginOutcome::Pending);
    };

    let seconds = timeout_seconds.unwrap_or(DEFAULT_QR_WAIT_SECONDS).max(1);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let qr = qr;
    loop {
        match timeout_at(deadline, qr.client.next_raw_update()).await {
            Ok(Ok((tl::enums::Update::LoginToken, _))) => {
                let token = match export_login_token(&qr.client, qr.api_id, &qr.api_hash).await {
                    Ok(token) => token,
                    Err(error) => {
                        warn!(%error, "telegram qr export after update failed");
                        emit_control(
                            &env.topic,
                            "login_error",
                            json!({
                                "error": format!("export login token failed after approval update: {error:#}"),
                                "phase": "qr_export_after_update"
                            }),
                        )?;
                        return Ok(QrLoginOutcome::Pending);
                    }
                };
                return handle_login_token(env, login_state, state, qr, token).await;
            }
            Ok(Ok((_update, _))) => continue,
            Ok(Err(error)) => {
                warn!(%error, "telegram qr wait failed");
                emit_control(
                    &env.topic,
                    "login_error",
                    json!({ "error": format!("QR login wait failed: {error:#}"), "phase": "qr_wait" }),
                )?;
                return Ok(QrLoginOutcome::Pending);
            }
            Err(_) => {
                let token = match export_login_token(&qr.client, qr.api_id, &qr.api_hash).await {
                    Ok(token) => token,
                    Err(error) => {
                        warn!(%error, "telegram qr refresh after timeout failed");
                        emit_control(
                            &env.topic,
                            "login_error",
                            json!({
                                "error": format!("QR login timed out and refresh failed: {error:#}"),
                                "phase": "qr_timeout"
                            }),
                        )?;
                        return Ok(QrLoginOutcome::Pending);
                    }
                };
                return handle_login_token(env, login_state, state, qr, token).await;
            }
        }
    }
}

async fn handle_login_token(
    env: &SkillEnv,
    login_state: &mut LoginState,
    state: &mut Option<QrLoginState>,
    mut qr: QrLoginState,
    mut token: tl::enums::auth::LoginToken,
) -> anyhow::Result<QrLoginOutcome> {
    for _ in 0..MAX_QR_MIGRATIONS {
        match token {
            tl::enums::auth::LoginToken::Token(login_token) => {
                emit_qr_token(env, &login_token)?;
                *state = Some(qr);
                return Ok(QrLoginOutcome::Pending);
            }
            tl::enums::auth::LoginToken::Success(success) => {
                return complete_qr_login(
                    env,
                    qr.client,
                    qr.api_id,
                    qr.api_hash,
                    qr.dc_id,
                    success.authorization,
                )
                .await;
            }
            tl::enums::auth::LoginToken::MigrateTo(migration) => {
                let client = match connect_qr_client(
                    qr.api_id,
                    qr.api_hash.clone(),
                    Some(migration.dc_id),
                )
                .await
                {
                    Ok(client) => client,
                    Err(error) => {
                        warn!(%error, dc_id = migration.dc_id, "telegram qr dc migration connect failed");
                        emit_control(
                            &env.topic,
                            "login_error",
                            json!({
                                "error": format!("connect to Telegram DC {} failed: {error:#}", migration.dc_id),
                                "phase": "qr_migrate"
                            }),
                        )?;
                        return Ok(QrLoginOutcome::Pending);
                    }
                };
                token = match client
                    .invoke(&tl::functions::auth::ImportLoginToken {
                        token: migration.token,
                    })
                    .await
                {
                    Ok(token) => token,
                    Err(error) if error.is("SESSION_PASSWORD_NEEDED") => {
                        return prepare_qr_password_challenge(
                            env,
                            login_state,
                            client,
                            qr.api_id,
                            qr.api_hash,
                        )
                        .await;
                    }
                    Err(error) => {
                        warn!(%error, dc_id = migration.dc_id, "telegram qr import login token failed");
                        emit_control(
                            &env.topic,
                            "login_error",
                            json!({
                                "error": format!("import login token in Telegram DC {} failed: {error:#}", migration.dc_id),
                                "phase": "qr_import"
                            }),
                        )?;
                        return Ok(QrLoginOutcome::Pending);
                    }
                };
                qr = QrLoginState {
                    client,
                    api_id: qr.api_id,
                    api_hash: qr.api_hash,
                    dc_id: migration.dc_id,
                };
            }
        }
    }

    emit_control(
        &env.topic,
        "login_error",
        json!({
            "error": "Telegram QR login bounced through too many datacenters",
            "phase": "qr_migrate"
        }),
    )?;
    Ok(QrLoginOutcome::Pending)
}

async fn complete_qr_login(
    env: &SkillEnv,
    client: Client,
    api_id: i32,
    api_hash: String,
    dc_id: i32,
    authorization: tl::enums::auth::Authorization,
) -> anyhow::Result<QrLoginOutcome> {
    let user = match authorization {
        tl::enums::auth::Authorization::Authorization(auth) => User::from_raw(auth.user),
        tl::enums::auth::Authorization::SignUpRequired(_) => {
            emit_control(
                &env.topic,
                "login_error",
                json!({
                    "error": "Telegram QR login returned sign-up required; use an official Telegram app to create the account first",
                    "phase": "qr_complete"
                }),
            )?;
            return Ok(QrLoginOutcome::Pending);
        }
    };

    client.session().set_user(user.id(), dc_id, user.is_bot());
    login::save_session(env, &client)?;
    persist_qr_credentials(env, api_id, api_hash.clone());

    let verified = reconnect_authorized_client(env, api_id, api_hash).await?;
    let verified_user = verified.get_me().await?;
    emit_control(
        &env.topic,
        "login_complete",
        json!({
            "qr_login": true,
            "user_id": verified_user.id(),
            "first_name": verified_user.first_name(),
        }),
    )?;
    info!(user_id = verified_user.id(), "telegram qr login complete");
    Ok(QrLoginOutcome::Complete(verified))
}

async fn prepare_qr_password_challenge(
    env: &SkillEnv,
    login_state: &mut LoginState,
    client: Client,
    api_id: i32,
    api_hash: String,
) -> anyhow::Result<QrLoginOutcome> {
    let password_token = match get_password_token(&client).await {
        Ok(token) => token,
        Err(error) => {
            warn!(%error, "telegram qr password token fetch failed");
            emit_control(
                &env.topic,
                "login_error",
                json!({
                    "error": format!("Telegram QR login requires 2FA, but password challenge setup failed: {error:#}"),
                    "phase": "qr_password"
                }),
            )?;
            return Ok(QrLoginOutcome::Pending);
        }
    };
    let hint = password_token.hint().map(str::to_string);
    login_state.login_token = None;
    login_state.password_token = Some(password_token);
    login_state.phone = None;
    login_state.api_id = Some(api_id);
    login_state.api_hash = Some(api_hash);
    if let Err(error) = login::save_session(env, &client) {
        warn!(error = %error, "failed to persist telegram qr 2FA session");
    }
    emit_control(
        &env.topic,
        "login_awaiting_password",
        json!({
            "qr_login": true,
            "password_hint": hint,
        }),
    )?;
    info!("telegram qr login requires 2FA password");
    Ok(QrLoginOutcome::AwaitingPassword(client))
}

async fn get_password_token(client: &Client) -> Result<PasswordToken, InvocationError> {
    let request = tl::functions::account::GetPassword {};
    let password: tl::types::account::Password = client.invoke(&request).await?.into();
    Ok(PasswordToken::new(password))
}

async fn connect_qr_client(
    api_id: i32,
    api_hash: String,
    force_dc_id: Option<i32>,
) -> anyhow::Result<Client> {
    let session = Session::new();
    if let Some(dc_id) = force_dc_id {
        session.set_user(0, dc_id, false);
    }
    Client::connect(Config {
        session,
        api_id,
        api_hash,
        params: default_init_params(),
    })
    .await
    .context("connect Telegram QR login client")
}

async fn reconnect_authorized_client(
    env: &SkillEnv,
    api_id: i32,
    api_hash: String,
) -> anyhow::Result<Client> {
    let session = Session::load_file_or_create(&env.session_path)
        .with_context(|| format!("load session file {}", env.session_path.display()))?;
    let client = Client::connect(Config {
        session,
        api_id,
        api_hash,
        params: default_init_params(),
    })
    .await
    .context("reconnect authorized Telegram QR session")?;
    if !client
        .is_authorized()
        .await
        .context("verify Telegram QR session authorization")?
    {
        anyhow::bail!("Telegram did not accept the QR-authorized session");
    }
    Ok(client)
}

async fn export_login_token(
    client: &Client,
    api_id: i32,
    api_hash: &str,
) -> anyhow::Result<tl::enums::auth::LoginToken> {
    client
        .invoke(&tl::functions::auth::ExportLoginToken {
            api_id,
            api_hash: api_hash.to_string(),
            except_ids: Vec::new(),
        })
        .await
        .context("export Telegram QR login token")
}

fn emit_qr_token(env: &SkillEnv, login_token: &tl::types::auth::LoginToken) -> anyhow::Result<()> {
    let url = qr_login_url(&login_token.token);
    emit_control(
        &env.topic,
        "login_qr",
        json!({
            "url": url,
            "expires_at_unix": login_token.expires,
            "expires_in_seconds": seconds_until(login_token.expires),
            "next": "Open this URL from a logged-in Telegram app, approve the login, then run `telegram login-qr-wait`."
        }),
    )
}

fn qr_login_url(token: &[u8]) -> String {
    format!("tg://login?token={}", URL_SAFE_NO_PAD.encode(token))
}

fn seconds_until(expires_at_unix: i32) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    i64::from(expires_at_unix)
        .checked_sub(now)
        .unwrap_or(0)
        .max(0)
}

fn persist_qr_credentials(env: &SkillEnv, api_id: i32, api_hash: String) {
    let creds = PersistedCredentials {
        api_id: Some(api_id),
        api_hash: Some(api_hash),
        phone: None,
    };
    if let Err(error) = creds.save(&env.credentials_path()) {
        warn!(error = %error, "failed to persist telegram qr credentials");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_login_url_uses_url_safe_unpadded_base64() {
        assert_eq!(qr_login_url(&[251, 255, 16]), "tg://login?token=-_8Q");
    }

    #[test]
    fn seconds_until_saturates_for_past_expiration() {
        assert_eq!(seconds_until(1), 0);
    }
}
