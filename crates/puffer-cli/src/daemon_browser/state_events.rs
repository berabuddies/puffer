//! Browser state event helpers shared by CDP session workers.

use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

use crate::daemon::ServerEnvelope;

use super::{BrowserState, DEFAULT_URL};

/// Updates cached browser state from a JavaScript evaluation result.
pub(super) fn update_state_from_eval(
    events: &broadcast::Sender<ServerEnvelope>,
    channel_state: &str,
    state: &Arc<Mutex<BrowserState>>,
    value: &Value,
) {
    let Some(result) = value.pointer("/result/result/value") else {
        return;
    };
    let mut state = state.lock().unwrap();
    if let Some(url) = result.get("url").and_then(Value::as_str) {
        state.url = url.to_string();
    }
    if let Some(title) = result.get("title").and_then(Value::as_str) {
        state.title = title.to_string();
    }
    state.loading = false;
    emit_state(events, channel_state, &state);
}

/// Emits the current browser state to daemon subscribers.
pub(super) fn emit_state(
    events: &broadcast::Sender<ServerEnvelope>,
    channel_state: &str,
    state: &BrowserState,
) {
    let _ = events.send(ServerEnvelope::Event {
        event: channel_state.to_string(),
        payload: json!({
            "url": state.url,
            "title": state.title,
            "loading": state.loading,
            "width": state.width,
            "height": state.height,
            "popOut": false
        }),
    });
}

/// Emits a browser state error while preserving the normal state payload shape.
pub(super) fn emit_state_error<E: std::fmt::Display>(
    events: &broadcast::Sender<ServerEnvelope>,
    channel_state: &str,
    error: E,
) {
    let _ = events.send(ServerEnvelope::Event {
        event: channel_state.to_string(),
        payload: json!({
            "url": DEFAULT_URL,
            "title": "",
            "loading": false,
            "error": error.to_string(),
            "popOut": false
        }),
    });
}
