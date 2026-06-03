use crate::AppState;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

static HTTP_FALLBACK_SESSIONS: OnceLock<Mutex<HashSet<Uuid>>> = OnceLock::new();

fn http_fallback_sessions() -> &'static Mutex<HashSet<Uuid>> {
    HTTP_FALLBACK_SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Marks the current session as using HTTP fallback instead of WebSocket.
pub(super) fn activate_openai_websocket_http_fallback(state: &AppState) {
    http_fallback_sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(state.session.id);
}

/// Returns true when the current session should skip OpenAI WebSocket transport.
pub(super) fn openai_websocket_http_fallback_active(state: &AppState) -> bool {
    http_fallback_sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&state.session.id)
}

/// Clears remembered WebSocket HTTP fallback sessions for test isolation.
#[cfg(test)]
pub(in crate::runtime) fn reset_openai_websocket_http_fallbacks() {
    http_fallback_sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_is_scoped_by_session_id() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reset_openai_websocket_http_fallbacks();

        let mut first = crate::runtime::tests::state();
        first.session.id = Uuid::new_v4();
        let mut second = crate::runtime::tests::state();
        second.session.id = Uuid::new_v4();

        activate_openai_websocket_http_fallback(&first);

        assert!(openai_websocket_http_fallback_active(&first));
        assert!(!openai_websocket_http_fallback_active(&second));

        reset_openai_websocket_http_fallbacks();
    }
}
