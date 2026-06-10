//! Connection-health observability for issue #570 ("Telegram connected, then
//! suddenly unauthenticated / stops receiving").
//!
//! These failures are intermittent and previously left only an unclassified
//! `warn!`/`error!` tracing line plus a generic `login_required` control event,
//! so an occurrence could not be root-caused from logs alone. This module
//! records each failure moment three ways:
//!   - a durable append-only ndjson record next to the account's session/cursor
//!     (`connection-diagnostics.ndjson`), so the evidence survives a restart;
//!   - a classified `tracing` event (network vs auth vs other);
//!   - a control event on the subscriber bus, so the daemon can later drive
//!     connection state / telemetry off it.
//!
//! Best-effort: recording a failure never blocks or alters the resume/login
//! fallback behaviour itself.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tracing::{info, warn};

use crate::diagnostics::append_ndjson;
use crate::events::emit_control;
use crate::state::SkillEnv;

/// Path of the append-only connection-health diagnostics file, kept alongside
/// the account's session/cursor so a failure is durably recorded on disk.
fn connection_diagnostics_path(env: &SkillEnv) -> PathBuf {
    env.state_dir.join("connection-diagnostics.ndjson")
}

/// Coarsely classifies a Telegram/transport error so the drop cause is
/// queryable: `auth` = a server-side deauthorization (the key was invalidated —
/// the #570 "unauthenticated" case), `network` = a transient transport failure,
/// or `other`. Best-effort, case-insensitive substring match; defaults to
/// `other` when nothing matches.
pub(crate) fn classify_error(error: &str) -> &'static str {
    let e = error.to_ascii_lowercase();
    const AUTH: [&str; 8] = [
        "auth_key",
        "authkeyunregistered",
        "unauthorized",
        "not authorized",
        "session_revoked",
        "session_expired",
        "user_deactivated",
        "deauthor",
    ];
    const NETWORK: [&str; 9] = [
        "connect",
        "network",
        "timed out",
        "timeout",
        "transport",
        "broken pipe",
        "connection reset",
        "dns",
        "io error",
    ];
    if AUTH.iter().any(|needle| e.contains(needle)) {
        "auth"
    } else if NETWORK.iter().any(|needle| e.contains(needle)) {
        "network"
    } else {
        "other"
    }
}

/// Records that an authenticated session could not be resumed and the
/// subscriber fell back to login (the user-visible "unauthenticated" moment).
///
/// `reason` is a stable enum (`not_signed_in` / `credentials_unavailable` /
/// `connect_failed` / `key_invalidated` / `probe_failed`). `anomaly` is `false`
/// for the normal fresh-login path (`not_signed_in`) and `true` for the
/// unexpected paths that indicate #570, so queries can ignore the benign case.
/// `class` is the same `auth`/`network`/`other` tag as [`classify_error`] (plus
/// `config`/`none` for the non-error reasons) on EVERY record, so a single
/// `select(.class=="auth")` catches every auth-class drop regardless of reason.
pub(crate) fn report_resume_failed(
    env: &SkillEnv,
    reason: &str,
    anomaly: bool,
    class: &str,
    detail: Value,
) {
    let record = json!({
        "at_ms": now_unix_millis(),
        "event": "resume_failed",
        "reason": reason,
        "anomaly": anomaly,
        "class": class,
        "detail": detail,
    });
    append_ndjson(&connection_diagnostics_path(env), &record);
    if anomaly {
        warn!(
            reason,
            "telegram session resume failed; falling back to login"
        );
    } else {
        info!(
            reason,
            "telegram session not resumable; fresh login required"
        );
    }
    let _ = emit_control(&env.topic, "resume_failed", record);
}

/// Records the live update loop terminating on a stream error (the "connected
/// but receives nothing" failure). Always an anomaly; classifies the error as
/// `auth` / `network` / `other` so the drop cause is queryable.
pub(crate) fn report_update_loop_error(env: &SkillEnv, error: &str) {
    let class = classify_error(error);
    let record = json!({
        "at_ms": now_unix_millis(),
        "event": "update_loop_error",
        "class": class,
        "fatal": true,
        "error": error,
    });
    append_ndjson(&connection_diagnostics_path(env), &record);
    warn!(
        class,
        error, "telegram live update loop terminated on stream error"
    );
    let _ = emit_control(&env.topic, "update_loop_error", record);
}

/// Subscriber-local Unix-epoch milliseconds (mirrors the other modules' local
/// helper; the value is monotonic enough for ordering diagnostic records).
fn now_unix_millis() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i128)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::classify_error;

    #[test]
    fn classifies_auth_invalidation() {
        assert_eq!(classify_error("rpc error: AUTH_KEY_UNREGISTERED"), "auth");
        assert_eq!(classify_error("SESSION_REVOKED"), "auth");
        assert_eq!(classify_error("user was deauthorized"), "auth");
        assert_eq!(classify_error("Unauthorized"), "auth");
    }

    #[test]
    fn classifies_network_transport() {
        assert_eq!(classify_error("failed to connect: timed out"), "network");
        assert_eq!(classify_error("transport closed"), "network");
        assert_eq!(classify_error("connection reset by peer"), "network");
    }

    #[test]
    fn defaults_to_other() {
        assert_eq!(classify_error("some unexpected message"), "other");
    }

    #[test]
    fn auth_takes_precedence_over_network() {
        // A message mentioning both should classify as auth (the actionable one).
        assert_eq!(
            classify_error("connection dropped: AUTH_KEY_UNREGISTERED"),
            "auth"
        );
    }
}
