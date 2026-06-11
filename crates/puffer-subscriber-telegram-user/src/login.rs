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
use grammers_client::{session::Session, Client, Config, SignInError};
use serde_json::json;
use tracing::{info, warn};

use crate::events::emit_control;
use crate::state::{
    default_init_params, resolve_api_credentials, LoginState, PersistedCredentials, SkillEnv,
};

/// Result of a Telegram login-code submission.
pub enum CodeSubmitOutcome {
    /// Login completed and the session is authorized.
    Complete,
    /// Telegram accepted the code but requires the user's 2FA password.
    AwaitingPassword,
    /// The submission failed and the subscriber emitted a terminal error.
    Failed,
    /// The submission hit a transient transport failure and can be retried.
    RetryableTransportError {
        /// Error text emitted by grammers for diagnostics.
        error: String,
    },
}

/// Starts a login attempt: connects to Telegram (creating a fresh session if
/// necessary), requests a login code for `phone`, stores the resulting
/// [`grammers_client::types::LoginToken`] in `state`, and emits
/// `login_awaiting_code`. Returns the connected [`Client`] so the caller can
/// reuse it for the subsequent sign-in step.
///
/// `api_id`/`api_hash` may be `None`; the subscriber resolves a complete
/// credential pair via [`resolve_api_credentials`] from explicit input,
/// persisted credentials, or environment variables.
pub async fn start(
    env: &SkillEnv,
    state: &mut LoginState,
    phone: String,
    api_id: Option<i32>,
    api_hash: Option<String>,
) -> anyhow::Result<Option<Client>> {
    let persisted = PersistedCredentials::load(&env.credentials_path()).unwrap_or_default();
    let (api_id, api_hash) = match resolve_api_credentials(api_id, api_hash, &persisted) {
        Ok(pair) => pair,
        Err(error) => {
            warn!(%error, "telegram api credential resolution failed");
            emit_control(
                &env.topic,
                "login_error",
                json!({ "error": error.to_string(), "phase": "credentials" }),
            )?;
            return Ok(None);
        }
    };

    for attempt in 0..2 {
        let client = match connect_fresh_login_client(api_id, api_hash.clone()).await {
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
                if let Err(error) = save_session(env, &client) {
                    warn!(error = %error, "failed to persist telegram pre-auth session");
                }
                emit_control(&env.topic, "login_awaiting_code", json!({ "phone": phone }))?;
                info!(phone = %phone, "login code requested");
                return Ok(Some(client));
            }
            Err(err) => {
                let error = err.to_string();
                if attempt == 0 && is_auth_restart_error_text(&error) {
                    warn!(
                        %error,
                        "telegram requested auth restart while sending login code; retrying with a fresh session"
                    );
                    continue;
                }
                warn!(%error, "request_login_code failed");
                emit_control(
                    &env.topic,
                    "login_error",
                    json!({ "error": format!("request_login_code failed: {error}"), "phase": "request_code" }),
                )?;
                return Ok(None);
            }
        }
    }

    Ok(None)
}

/// Handles `TelegramLoginSubmitCode`: completes sign-in with the cached
/// [`grammers_client::types::LoginToken`], persists the session on success,
/// and emits the appropriate control event.
pub async fn submit_code(
    env: &SkillEnv,
    state: &mut LoginState,
    client: &Client,
    code: String,
) -> anyhow::Result<CodeSubmitOutcome> {
    let Some(token) = state.login_token.take() else {
        emit_control(
            &env.topic,
            "login_error",
            json!({ "error": "no login in progress; send telegram_login_start first" }),
        )?;
        return Ok(CodeSubmitOutcome::Failed);
    };

    match client.sign_in(&token, &code).await {
        Ok(user) => {
            save_completed_session(env, client)?;
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
            Ok(CodeSubmitOutcome::Complete)
        }
        Err(SignInError::PasswordRequired(password_token)) => {
            state.password_token = Some(password_token);
            emit_control(
                &env.topic,
                "login_awaiting_password",
                json!({ "phone": state.phone.clone().unwrap_or_default() }),
            )?;
            info!("2FA password required");
            Ok(CodeSubmitOutcome::AwaitingPassword)
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
            Ok(CodeSubmitOutcome::Failed)
        }
        Err(err) => {
            let error = err.to_string();
            if is_retryable_sign_in_error_text(&error) {
                warn!(%error, "sign_in transport failed; preserving login token for retry");
                state.login_token = Some(token);
                return Ok(CodeSubmitOutcome::RetryableTransportError { error });
            }
            warn!(error = %error, "sign_in failed");
            state.clear_tokens();
            emit_control(
                &env.topic,
                "login_error",
                json!({ "error": format!("sign_in failed: {error}"), "phase": "sign_in" }),
            )?;
            Ok(CodeSubmitOutcome::Failed)
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

    match client
        .check_password(password_token, password.as_bytes())
        .await
    {
        Ok(user) => {
            save_completed_session(env, client)?;
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
    save_session_bytes(&env.session_path, client.session().save())
}

/// Persists a *fully authorized* session and promotes it onto the live
/// session path when the host staged the login (see
/// [`SkillEnv::live_session_path`]).
///
/// Completion events must only be emitted after this returns: parents treat
/// `login_complete` as terminal and may kill this process the moment they
/// read it, which would otherwise strand the staged session and leave the
/// account with credentials but no usable `telegram.session`
/// (agentenv/monorepo#551).
pub fn save_completed_session(env: &SkillEnv, client: &Client) -> anyhow::Result<()> {
    save_session(env, client)?;
    promote_completed_session(env)
}

/// Atomically renames the staged session at `env.session_path` onto
/// `env.live_session_path`. No-op when the host does not stage logins or
/// when nothing was staged.
pub(crate) fn promote_completed_session(env: &SkillEnv) -> anyhow::Result<()> {
    let Some(live) = env.live_session_path.as_ref() else {
        return Ok(());
    };
    if live == &env.session_path || !env.session_path.exists() {
        return Ok(());
    }
    if let Some(parent) = live.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create session parent dir {}", parent.display()))?;
    }
    std::fs::rename(&env.session_path, live).with_context(|| {
        format!(
            "promote session {} -> {}",
            env.session_path.display(),
            live.display()
        )
    })
}

fn save_session_bytes(path: &std::path::Path, bytes: Vec<u8>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create session parent dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

async fn connect_fresh_login_client(api_id: i32, api_hash: String) -> anyhow::Result<Client> {
    Client::connect(Config {
        session: Session::new(),
        api_id,
        api_hash,
        params: default_init_params(),
    })
    .await
    .context("connect telegram login client")
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

fn is_retryable_sign_in_error_text(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("read 0 bytes")
        || lower.contains("connection reset")
        || lower.contains("connection aborted")
        || lower.contains("broken pipe")
        || lower.contains("unexpected eof")
}

fn is_auth_restart_error_text(error: &str) -> bool {
    error.to_ascii_lowercase().contains("auth_restart")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_sign_in_error_text_matches_transport_disconnects() {
        assert!(is_retryable_sign_in_error_text(
            "request error: read error, IO failed: read 0 bytes"
        ));
        assert!(is_retryable_sign_in_error_text(
            "request error: read error, IO failed: connection reset by peer"
        ));
    }

    #[test]
    fn retryable_sign_in_error_text_rejects_auth_errors() {
        assert!(!is_retryable_sign_in_error_text("invalid code"));
        assert!(!is_retryable_sign_in_error_text("PHONE_CODE_INVALID"));
    }

    #[test]
    fn auth_restart_error_text_matches_telegram_restart() {
        assert!(is_auth_restart_error_text(
            "request error: rpc error 500: AUTH_RESTART caused by auth.sendCode"
        ));
        assert!(is_auth_restart_error_text("auth_restart"));
        assert!(!is_auth_restart_error_text("PHONE_NUMBER_INVALID"));
    }

    #[test]
    fn save_session_bytes_creates_missing_session_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state/telegram.session");

        save_session_bytes(&path, b"session-bytes".to_vec()).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"session-bytes");
        assert!(!path.with_extension("tmp").exists());
    }

    fn staging_env(temp: &tempfile::TempDir, live: Option<std::path::PathBuf>) -> SkillEnv {
        SkillEnv {
            state_dir: temp.path().to_path_buf(),
            session_path: temp.path().join("login-staging.session"),
            topic: "telegram-user".to_string(),
            workspace_config_dir: None,
            live_session_path: live,
        }
    }

    #[test]
    fn promote_completed_session_renames_staging_onto_live() {
        let temp = tempfile::tempdir().unwrap();
        let live = temp.path().join("telegram.session");
        let env = staging_env(&temp, Some(live.clone()));
        std::fs::write(&env.session_path, b"authorized-session").unwrap();

        promote_completed_session(&env).unwrap();

        assert_eq!(std::fs::read(&live).unwrap(), b"authorized-session");
        assert!(
            !env.session_path.exists(),
            "staging must be renamed onto live, not copied"
        );
    }

    #[test]
    fn promote_completed_session_noop_without_live_path() {
        let temp = tempfile::tempdir().unwrap();
        let env = staging_env(&temp, None);
        std::fs::write(&env.session_path, b"resident-session").unwrap();

        promote_completed_session(&env).unwrap();

        assert_eq!(
            std::fs::read(&env.session_path).unwrap(),
            b"resident-session"
        );
    }

    #[test]
    fn promote_completed_session_noop_when_nothing_staged() {
        let temp = tempfile::tempdir().unwrap();
        let live = temp.path().join("telegram.session");
        std::fs::write(&live, b"existing-live-session").unwrap();
        let env = staging_env(&temp, Some(live.clone()));

        promote_completed_session(&env).unwrap();

        assert_eq!(std::fs::read(&live).unwrap(), b"existing-live-session");
    }
}
