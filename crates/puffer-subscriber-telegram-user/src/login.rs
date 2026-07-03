//! Effect layer for the Telegram login flow.
//!
//! The pure transition logic lives in [`crate::login_flow`]; this module owns
//! the grammers [`Client`], runs every MTProto round-trip bounded at
//! [`LOGIN_NETWORK_TIMEOUT`], classifies failures into
//! [`ErrClass`](crate::login_flow::ErrClass), and applies the returned
//! decisions via [`LoginSession`]. Each state transition emits a control
//! event on the skill's topic so the agent can observe progress without
//! polling.

use anyhow::Context as _;
use grammers_client::types::{LoginToken, PasswordToken};
use grammers_client::{session::Session, Client, Config, SignInError};
use grammers_tl_types as tl;
use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

use crate::client::HydrationSlot;
use crate::events::emit_control;
use crate::login_flow::{
    self, AuthorizedUser, Decision, ErrClass, LoginPhase, PasswordStep, StartFailOp, StartStep,
};
use crate::state::{default_init_params, resolve_api_credentials, PersistedCredentials, SkillEnv};

pub(crate) type LivePhase = LoginPhase<LoginToken, PasswordToken>;
pub(crate) type LiveErr = ErrClass<PasswordToken>;

/// Bound on every MTProto round-trip in the login flow. Without it an
/// unreachable/half-dead Telegram connection blocks the sequential command
/// loop forever and queued retries are never read (#744, #705).
pub(crate) const LOGIN_NETWORK_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Runs one network effect with the login timeout, classifying failures.
pub(crate) async fn bounded<T, E, F>(fut: F) -> Result<T, LiveErr>
where
    E: std::fmt::Display,
    F: Future<Output = Result<T, E>>,
{
    match tokio::time::timeout(LOGIN_NETWORK_TIMEOUT, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(login_flow::classify_error_text(error.to_string())),
        Err(_elapsed) => Err(ErrClass::Timeout),
    }
}

/// `sign_in`/`check_password` return typed `SignInError`s that carry the
/// password token — classify those by variant, not by text.
fn classify_sign_in(error: SignInError) -> LiveErr {
    match error {
        SignInError::PasswordRequired(token) => ErrClass::PasswordRequired(token),
        SignInError::InvalidCode => ErrClass::InvalidCode,
        SignInError::InvalidPassword => ErrClass::InvalidPassword,
        other => login_flow::classify_error_text(other.to_string()),
    }
}

async fn bounded_sign_in(
    client: &Client,
    token: &LoginToken,
    code: &str,
) -> Result<grammers_client::types::User, LiveErr> {
    match tokio::time::timeout(LOGIN_NETWORK_TIMEOUT, client.sign_in(token, code)).await {
        Ok(Ok(user)) => Ok(user),
        Ok(Err(error)) => Err(classify_sign_in(error)),
        Err(_elapsed) => Err(ErrClass::Timeout),
    }
}

async fn bounded_check_password(
    client: &Client,
    token: PasswordToken,
    password: &str,
) -> Result<grammers_client::types::User, LiveErr> {
    match tokio::time::timeout(
        LOGIN_NETWORK_TIMEOUT,
        client.check_password(token, password.as_bytes()),
    )
    .await
    {
        Ok(Ok(user)) => Ok(user),
        Ok(Err(error)) => Err(classify_sign_in(error)),
        Err(_elapsed) => Err(ErrClass::Timeout),
    }
}

/// Fetches a fresh 2FA password challenge (`account.GetPassword`). Shared by
/// the QR 2FA branch and the password-retry refetch (#751): grammers consumes
/// the `PasswordToken` on every `check_password`, so each retry needs a new one.
pub(crate) async fn get_password_token(client: &Client) -> Result<PasswordToken, LiveErr> {
    let request = tl::functions::account::GetPassword {};
    bounded(client.invoke(&request)).await.map(|password| {
        let password: tl::types::account::Password = password.into();
        PasswordToken::new(password)
    })
}

fn user_data(user: &grammers_client::types::User) -> AuthorizedUser {
    AuthorizedUser {
        id: user.id(),
        first_name: Some(user.first_name().to_string()),
    }
}

/// Owns the login flow for one subscriber: the grammers client handle, the
/// explicit phase, and the cross-phase credentials (preserved across failed
/// attempts so a retry can reuse them without re-sending them).
pub struct LoginSession {
    pub(crate) env: SkillEnv,
    pub(crate) client: Option<Client>,
    pub(crate) phase: LivePhase,
    pub(crate) api_id: Option<i32>,
    pub(crate) api_hash: Option<String>,
    pub(crate) phone: Option<String>,
    /// Single-flight guard for the fire-and-forget contact hydration task.
    pub(crate) hydration: HydrationSlot,
}

impl LoginSession {
    pub fn new(env: SkillEnv) -> Self {
        Self {
            env,
            client: None,
            phase: LoginPhase::Idle,
            api_id: None,
            api_hash: None,
            phone: None,
            hydration: HydrationSlot::default(),
        }
    }

    pub fn is_authorized(&self) -> bool {
        self.phase.is_authorized()
    }

    /// Applies a decision. `Authorized` saves+promotes the session BEFORE
    /// any event goes out (#551: parents may kill us on `login_complete`).
    pub(crate) fn apply(
        &mut self,
        decision: Decision<LoginToken, PasswordToken>,
    ) -> anyhow::Result<()> {
        if decision.next.is_authorized() {
            if let Some(client) = self.client.as_ref() {
                save_completed_session(&self.env, client)?;
            }
            self.persist_credentials();
        }
        self.phase = decision.next;
        for event in decision.events {
            emit_control(&self.env.topic, event.kind, event.payload)?;
        }
        Ok(())
    }

    fn emit_one(&self, event: login_flow::ControlEventSpec) -> anyhow::Result<()> {
        emit_control(&self.env.topic, event.kind, event.payload)
    }

    pub async fn start(
        &mut self,
        phone: String,
        api_id: Option<i32>,
        api_hash: Option<String>,
    ) -> anyhow::Result<()> {
        let persisted =
            PersistedCredentials::load(&self.env.credentials_path()).unwrap_or_default();
        let (api_id, api_hash) = match resolve_api_credentials(api_id, api_hash, &persisted) {
            Ok(pair) => pair,
            Err(error) => {
                warn!(%error, "telegram api credential resolution failed");
                return self.emit_one(login_flow::login_error_event(
                    "credentials",
                    "credentials_unresolved",
                    false,
                    error.to_string(),
                ));
            }
        };
        self.api_id = Some(api_id);
        self.api_hash = Some(api_hash.clone());
        self.phone = Some(phone.clone());

        for attempt in 0..2u8 {
            let client = match bounded(connect_fresh_login_client(api_id, api_hash.clone())).await {
                Ok(client) => client,
                Err(err) => {
                    let StartStep::Decided(decision) =
                        login_flow::decide_start::<LoginToken, PasswordToken>(
                            &phone,
                            Err((StartFailOp::Connect, err)),
                            attempt,
                            now_ms(),
                        )
                    else {
                        continue;
                    };
                    return self.apply(decision);
                }
            };
            let result = bounded(client.request_login_code(&phone)).await;
            match login_flow::decide_start(
                &phone,
                result.map_err(|e| (StartFailOp::RequestCode, e)),
                attempt,
                now_ms(),
            ) {
                StartStep::RetryFreshSession => {
                    warn!("telegram requested auth restart while sending login code; retrying with a fresh session");
                    continue;
                }
                StartStep::Decided(decision) => {
                    if matches!(decision.next, LoginPhase::CodeSent { .. }) {
                        if let Err(error) = save_session(&self.env, &client) {
                            warn!(error = %error, "failed to persist telegram pre-auth session");
                        }
                        self.client = Some(client);
                        info!(phone = %phone, "login code requested");
                    }
                    return self.apply(decision);
                }
            }
        }
        Ok(())
    }

    pub async fn submit_code(&mut self, code: String) -> anyhow::Result<()> {
        let phase = std::mem::replace(&mut self.phase, LoginPhase::Idle);
        let LoginPhase::CodeSent {
            token,
            requested_at_ms,
        } = phase
        else {
            let name = phase.name();
            self.phase = phase;
            return self.emit_one(login_flow::wrong_phase_error("submit_code", name));
        };
        let Some(client) = self.client.clone() else {
            // Defensive: `CodeSent` implies a client. If the invariant broke,
            // the token was already consumed by the phase reset above, so the
            // only recovery is restarting the login flow.
            return self.emit_one(login_flow::wrong_phase_error("submit_code", "no_client"));
        };

        let mut result = bounded_sign_in(&client, &token, &code).await;
        // Reconnect-once on transport failure: a clean disconnect between
        // request_login_code and submit is common after idle minutes.
        if matches!(result, Err(ErrClass::Transport(_))) {
            match self.reconnect_login_client().await {
                Ok(fresh) => {
                    warn!("retrying telegram sign_in after reconnect");
                    self.client = Some(fresh.clone());
                    result = bounded_sign_in(&fresh, &token, &code).await;
                }
                Err(error) => warn!(%error, "telegram sign_in reconnect failed"),
            }
        }
        let decision = login_flow::decide_code_submit(
            token,
            requested_at_ms,
            self.phone.as_deref(),
            result.map(|u| user_data(&u)),
            now_ms(),
        );
        self.apply(decision)
    }

    pub async fn submit_password(&mut self, password: String) -> anyhow::Result<()> {
        let phase = std::mem::replace(&mut self.phase, LoginPhase::Idle);
        let LoginPhase::PasswordPending { token, hint } = phase else {
            let name = phase.name();
            self.phase = phase;
            return self.emit_one(login_flow::wrong_phase_error("submit_password", name));
        };
        let Some(client) = self.client.clone() else {
            // Defensive: `PasswordPending` implies a client. If the invariant
            // broke, the token was already consumed by the phase reset above,
            // so the only recovery is restarting the login flow.
            return self.emit_one(login_flow::wrong_phase_error(
                "submit_password",
                "no_client",
            ));
        };

        let result = bounded_check_password(&client, token, &password).await;
        match login_flow::decide_password_submit(hint, result.map(|u| user_data(&u))) {
            PasswordStep::Decided(decision) => self.apply(decision),
            PasswordStep::RefetchToken {
                hint,
                reason,
                error,
            } => {
                let refetched = get_password_token(&client).await.map_err(|e| match e {
                    ErrClass::Timeout => {
                        "timed out fetching a fresh password challenge".to_string()
                    }
                    ErrClass::Transport(t) | ErrClass::Fatal(t) => t,
                    _ => "password challenge refresh failed".to_string(),
                });
                let decision = login_flow::decide_password_refetch(hint, reason, error, refetched);
                self.apply(decision)
            }
        }
    }

    async fn reconnect_login_client(&self) -> anyhow::Result<Client> {
        let api_id = self.api_id.context("no api_id for reconnect")?;
        let api_hash = self.api_hash.clone().context("no api_hash for reconnect")?;
        let session = Session::load_file_or_create(&self.env.session_path)
            .with_context(|| format!("load session file {}", self.env.session_path.display()))?;
        bounded(Client::connect(Config {
            session,
            api_id,
            api_hash,
            params: default_init_params(),
        }))
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "reconnect telegram client for sign_in retry: {}",
                match e {
                    ErrClass::Timeout => "timeout".into(),
                    ErrClass::Transport(t) | ErrClass::Fatal(t) => t,
                    _ => "failed".into(),
                }
            )
        })
    }

    fn persist_credentials(&self) {
        let creds = PersistedCredentials {
            api_id: self.api_id,
            api_hash: self.api_hash.clone(),
            phone: self.phone.clone(),
        };
        if let Err(error) = creds.save(&self.env.credentials_path()) {
            warn!(error = %error, "failed to persist telegram credentials");
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

#[cfg(test)]
mod tests {
    use super::*;

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
