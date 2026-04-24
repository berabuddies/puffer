//! Login-flow handlers for the Telegram subscriber.
//!
//! The skill drives a three-step interactive login: the agent sends
//! `TelegramLoginStart { phone, api_id, api_hash }`, Telegram dispatches a
//! code out-of-band, the agent forwards that code as
//! `TelegramLoginSubmitCode`, and if 2FA is enabled the agent forwards the
//! cloud password as `TelegramLoginSubmitPassword`. Each state transition
//! emits a control event on the skill's topic so the agent can observe
//! progress without polling.

use anyhow::Context as _;
use grammers_client::{
    session::Session, Client, Config, InitParams, SignInError,
};
use serde_json::json;
use tracing::{info, warn};

use crate::events::emit_control;
use crate::state::{
    resolve_api_hash, resolve_api_id, LoginState, PersistedCredentials, SkillEnv,
};

/// Starts a login attempt: connects to Telegram (creating a fresh session if
/// necessary), requests a login code for `phone`, stores the resulting
/// [`grammers_client::types::LoginToken`] in `state`, and emits
/// `login_awaiting_code`. Returns the connected [`Client`] so the caller can
/// reuse it for the subsequent sign-in step.
///
/// `api_id`/`api_hash` may be `None`; the subscriber resolves a working
/// pair via [`resolve_api_id`] / [`resolve_api_hash`] (persisted creds,
/// env vars, then Telegram Desktop's published default).
pub async fn start(
    env: &SkillEnv,
    state: &mut LoginState,
    phone: String,
    api_id: Option<i32>,
    api_hash: Option<String>,
) -> anyhow::Result<Option<Client>> {
    let persisted = PersistedCredentials::load(&env.credentials_path()).unwrap_or_default();
    let api_id = resolve_api_id(api_id, &persisted);
    let api_hash = resolve_api_hash(api_hash, &persisted);
    let session = Session::load_file_or_create(&env.session_path)
        .with_context(|| format!("load session file {}", env.session_path.display()))?;

    let config = Config {
        session,
        api_id,
        api_hash: api_hash.clone(),
        params: InitParams {
            catch_up: true,
            ..Default::default()
        },
    };

    let client = match Client::connect(config).await {
        Ok(c) => c,
        Err(err) => {
            warn!(error = %err, "telegram connect failed");
            emit_control(
                &env.topic,
                "login_error",
                json!({ "error": format!("connect failed: {err}"), "phase": "connect" }),
            )?;
            return Ok(None);
        }
    };

    match client.request_login_code(&phone).await {
        Ok(token) => {
            state.login_token = Some(token);
            state.password_token = None;
            state.phone = Some(phone.clone());
            state.api_id = Some(api_id);
            state.api_hash = Some(api_hash);
            emit_control(
                &env.topic,
                "login_awaiting_code",
                json!({ "phone": phone }),
            )?;
            info!(phone = %phone, "login code requested");
            Ok(Some(client))
        }
        Err(err) => {
            warn!(error = %err, "request_login_code failed");
            emit_control(
                &env.topic,
                "login_error",
                json!({ "error": format!("request_login_code failed: {err}"), "phase": "request_code" }),
            )?;
            Ok(None)
        }
    }
}

/// Handles `TelegramLoginSubmitCode`: completes sign-in with the cached
/// [`grammers_client::types::LoginToken`], persists the session on success,
/// and emits the appropriate control event.
///
/// Returns `Ok(true)` if the login has fully completed (no 2FA needed),
/// `Ok(false)` otherwise (error, or 2FA password is now required).
pub async fn submit_code(
    env: &SkillEnv,
    state: &mut LoginState,
    client: &Client,
    code: String,
) -> anyhow::Result<bool> {
    let Some(token) = state.login_token.take() else {
        emit_control(
            &env.topic,
            "login_error",
            json!({ "error": "no login in progress; send telegram_login_start first" }),
        )?;
        return Ok(false);
    };

    match client.sign_in(&token, &code).await {
        Ok(user) => {
            save_session(env, client)?;
            persist_credentials_from_state(env, state);
            state.clear_tokens();
            emit_control(
                &env.topic,
                "login_complete",
                json!({
                    "user_id": user.id(),
                    "first_name": user.first_name(),
                }),
            )?;
            info!(user_id = user.id(), "telegram login complete");
            Ok(true)
        }
        Err(SignInError::PasswordRequired(password_token)) => {
            state.password_token = Some(password_token);
            emit_control(
                &env.topic,
                "login_awaiting_password",
                json!({ "phone": state.phone.clone().unwrap_or_default() }),
            )?;
            info!("2FA password required");
            Ok(false)
        }
        Err(SignInError::InvalidCode) => {
            // Re-arm the token so the operator can retry with a fresh code
            // without restarting the flow.
            state.login_token = Some(token);
            emit_control(
                &env.topic,
                "login_error",
                json!({ "error": "invalid code", "phase": "sign_in" }),
            )?;
            Ok(false)
        }
        Err(err) => {
            warn!(error = %err, "sign_in failed");
            state.clear_tokens();
            emit_control(
                &env.topic,
                "login_error",
                json!({ "error": format!("sign_in failed: {err}"), "phase": "sign_in" }),
            )?;
            Ok(false)
        }
    }
}

/// Handles `TelegramLoginSubmitPassword`: completes the 2FA step.
///
/// Returns `Ok(true)` if the login has fully completed, `Ok(false)` otherwise.
pub async fn submit_password(
    env: &SkillEnv,
    state: &mut LoginState,
    client: &Client,
    password: String,
) -> anyhow::Result<bool> {
    let Some(password_token) = state.password_token.take() else {
        emit_control(
            &env.topic,
            "login_error",
            json!({ "error": "no 2FA challenge pending" }),
        )?;
        return Ok(false);
    };

    match client.check_password(password_token, password.as_bytes()).await {
        Ok(user) => {
            save_session(env, client)?;
            persist_credentials_from_state(env, state);
            state.clear_tokens();
            emit_control(
                &env.topic,
                "login_complete",
                json!({
                    "user_id": user.id(),
                    "first_name": user.first_name(),
                }),
            )?;
            info!(user_id = user.id(), "telegram 2FA login complete");
            Ok(true)
        }
        Err(err) => {
            warn!(error = %err, "check_password failed");
            state.clear_tokens();
            emit_control(
                &env.topic,
                "login_error",
                json!({ "error": format!("check_password failed: {err}"), "phase": "check_password" }),
            )?;
            Ok(false)
        }
    }
}

/// Persists the current authenticated session to the configured path.
///
/// The session file holds the MTProto authorization key and per-DC address
/// book; without it the next start would force the operator through the full
/// login flow again.
pub fn save_session(env: &SkillEnv, client: &Client) -> anyhow::Result<()> {
    client
        .session()
        .save_to_file(&env.session_path)
        .with_context(|| format!("save session to {}", env.session_path.display()))
}

/// Best-effort: writes the api_id/api_hash/phone the active login used
/// to the credentials file so future reconnects can skip prompting the
/// agent. Errors are logged and ignored — the login itself already
/// succeeded and we don't want a write failure to roll that back.
fn persist_credentials_from_state(env: &SkillEnv, state: &LoginState) {
    let creds = PersistedCredentials {
        api_id: state.api_id,
        api_hash: state.api_hash.clone(),
        phone: state.phone.clone(),
    };
    if let Err(error) = creds.save(&env.credentials_path()) {
        warn!(error = %error, "failed to persist telegram credentials");
    }
}
