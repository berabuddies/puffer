//! Persisted Telegram session resume and stream recovery helpers.

use anyhow::Context as _;
use grammers_client::{session::Session, Client, Config};
use serde_json::json;

use crate::state::{default_init_params, resolve_api_credentials, PersistedCredentials, SkillEnv};

/// Outcome of attempting to resume a persisted session. Callers MUST treat
/// `Transient` differently from `AuthRequired`: on a transient failure the
/// on-disk session is likely still valid and only the network/DC was
/// unreachable — claiming "not authenticated" parks a healthy account behind
/// a re-login it doesn't need (agentenv/monorepo#626).
pub(crate) enum SessionResume {
    /// Connected and authorized.
    Resumed(Client),
    /// The session genuinely lacks auth material (fresh login required).
    AuthRequired,
    /// Infrastructure failure (network/DC unreachable); retryable on demand.
    Transient(String),
}

/// Tries to open and connect an already-authenticated session.
pub(crate) async fn try_resume_session(env: &SkillEnv) -> anyhow::Result<SessionResume> {
    let session = Session::load_file_or_create(&env.session_path)
        .with_context(|| format!("load session file {}", env.session_path.display()))?;
    if !session.signed_in() {
        crate::health::report_resume_failed(env, "not_signed_in", false, "none", json!({}));
        return Ok(SessionResume::AuthRequired);
    }

    let persisted = PersistedCredentials::load(&env.credentials_path()).unwrap_or_default();
    let (api_id, api_hash) = match resolve_api_credentials(None, None, &persisted) {
        Ok(pair) => pair,
        Err(error) => {
            crate::health::report_resume_failed(
                env,
                "credentials_unavailable",
                true,
                "config",
                json!({ "error": error.to_string() }),
            );
            // Needs user intervention (a fresh login re-persists the API
            // credentials), so route it through the login path.
            return Ok(SessionResume::AuthRequired);
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
            let detail = err.to_string();
            let class = crate::health::classify_error(&detail);
            crate::health::report_resume_failed(
                env,
                "connect_failed",
                true,
                class,
                json!({ "error": detail }),
            );
            // connect runs initConnection, so a server-invalidated key can
            // surface here as an auth-class error (AUTH_KEY_UNREGISTERED) —
            // misreading it as Transient would trap the user in an offline
            // retry loop instead of routing them to re-login.
            return Ok(if class == "auth" {
                SessionResume::AuthRequired
            } else {
                SessionResume::Transient(detail)
            });
        }
    };
    match client.is_authorized().await {
        Ok(true) => Ok(SessionResume::Resumed(client)),
        Ok(false) => {
            crate::health::report_resume_failed(env, "key_invalidated", true, "auth", json!({}));
            Ok(SessionResume::AuthRequired)
        }
        Err(err) => {
            let detail = err.to_string();
            let class = crate::health::classify_error(&detail);
            crate::health::report_resume_failed(
                env,
                "probe_failed",
                true,
                class,
                json!({ "error": detail }),
            );
            if class == "auth" {
                Ok(SessionResume::AuthRequired)
            } else {
                Ok(SessionResume::Transient(detail))
            }
        }
    }
}

/// Returns whether a live update stream error should reconnect the client
/// instead of forcing the subscriber process to exit.
pub(crate) fn recoverable_live_update_error(error: &str) -> bool {
    let class = crate::health::classify_error(error);
    if class == "auth" {
        return false;
    }
    if class == "network" {
        return true;
    }
    let error = error.to_ascii_lowercase();
    error.contains("unexpected constructor")
        || error.contains("msgid_decrease_retry")
        || error.contains("bad response")
}

#[cfg(test)]
mod tests {
    use super::recoverable_live_update_error;

    #[test]
    fn live_update_decode_failures_are_recoverable() {
        assert!(recoverable_live_update_error(
            "request error: read error, bad response: unexpected constructor: 95ef6f2b"
        ));
        assert!(recoverable_live_update_error(
            "rpc error 500: MSGID_DECREASE_RETRY caused by updates.getState"
        ));
    }

    #[test]
    fn auth_failures_are_not_recovered_as_stream_flaps() {
        assert!(!recoverable_live_update_error(
            "rpc error: AUTH_KEY_UNREGISTERED"
        ));
        assert!(!recoverable_live_update_error("session_revoked"));
    }

    #[test]
    fn transport_failures_are_recoverable() {
        assert!(recoverable_live_update_error("connection reset by peer"));
        assert!(recoverable_live_update_error("request timed out"));
    }
}
