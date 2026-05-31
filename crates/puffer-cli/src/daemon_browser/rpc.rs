//! JSON-RPC handlers for daemon browser operations.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::daemon::DaemonState;

use super::params::{optional_u32, parse_input_event, required_string, state_json};
use super::{BrowserHistoryDirection, INITIAL_HEIGHT, INITIAL_WIDTH};

/// Handles `browser_open`.
pub(crate) fn handle_browser_open(state: &Arc<DaemonState>, params: &Value) -> Result<Value> {
    let session_id = required_string(params, "sessionId")?;
    let url = params
        .get("url")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let width = optional_u32(params, "width").unwrap_or(INITIAL_WIDTH);
    let height = optional_u32(params, "height").unwrap_or(INITIAL_HEIGHT);
    let browser_state =
        state
            .browsers
            .open(state.event_sender(), session_id, url, width, height)?;
    Ok(state_json(&browser_state))
}

/// Handles `browser_backend_status`.
pub(crate) fn handle_browser_backend_status(
    state: &Arc<DaemonState>,
    params: &Value,
) -> Result<Value> {
    let preferred = params
        .get("preferredRenderer")
        .or_else(|| params.get("preferred_renderer"))
        .and_then(Value::as_str)
        .unwrap_or("screencast");
    Ok(state.browsers.backend_status(preferred))
}

/// Handles `browser_navigate`.
pub(crate) fn handle_browser_navigate(state: &Arc<DaemonState>, params: &Value) -> Result<Value> {
    let session_id = required_string(params, "sessionId")?;
    let url = required_string(params, "url")?;
    state.browsers.navigate(&session_id, url)?;
    Ok(json!({ "ok": true }))
}

/// Handles `browser_reload`.
pub(crate) fn handle_browser_reload(state: &Arc<DaemonState>, params: &Value) -> Result<Value> {
    let session_id = required_string(params, "sessionId")?;
    state.browsers.reload(&session_id)?;
    Ok(json!({ "ok": true }))
}

/// Handles `browser_history`.
pub(crate) fn handle_browser_history(state: &Arc<DaemonState>, params: &Value) -> Result<Value> {
    let session_id = required_string(params, "sessionId")?;
    let direction = match required_string(params, "direction")?.as_str() {
        "back" => BrowserHistoryDirection::Back,
        "forward" => BrowserHistoryDirection::Forward,
        other => bail!("unsupported browser history direction `{other}`"),
    };
    state.browsers.history(&session_id, direction)?;
    Ok(json!({ "ok": true }))
}

/// Handles `browser_resize`.
pub(crate) fn handle_browser_resize(state: &Arc<DaemonState>, params: &Value) -> Result<Value> {
    let session_id = required_string(params, "sessionId")?;
    let width = optional_u32(params, "width").unwrap_or(INITIAL_WIDTH);
    let height = optional_u32(params, "height").unwrap_or(INITIAL_HEIGHT);
    state.browsers.resize(&session_id, width, height)?;
    Ok(json!({ "ok": true }))
}

/// Handles `browser_input`.
pub(crate) fn handle_browser_input(state: &Arc<DaemonState>, params: &Value) -> Result<Value> {
    let session_id = required_string(params, "sessionId")?;
    let event = parse_input_event(
        params
            .get("event")
            .ok_or_else(|| anyhow!("browser_input requires event"))?,
    )?;
    state.browsers.input(&session_id, event)?;
    Ok(json!({ "ok": true }))
}

/// Handles `browser_copy_selection`.
pub(crate) fn handle_browser_copy_selection(
    state: &Arc<DaemonState>,
    params: &Value,
) -> Result<Value> {
    let session_id = required_string(params, "sessionId")?;
    let copied = state.browsers.copy_selection(&session_id)?;
    Ok(json!({
        "text": copied.text,
        "copiedFrom": copied.copied_from
    }))
}

/// Handles `browser_cursor`.
pub(crate) fn handle_browser_cursor(state: &Arc<DaemonState>, params: &Value) -> Result<Value> {
    let session_id = required_string(params, "sessionId")?;
    let x = params
        .get("x")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow!("browser_cursor requires x"))?;
    let y = params
        .get("y")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow!("browser_cursor requires y"))?;
    let cursor = state.browsers.cursor(&session_id, x, y)?;
    Ok(json!({ "cursor": cursor.cursor }))
}

/// Handles `browser_close`.
pub(crate) fn handle_browser_close(state: &Arc<DaemonState>, params: &Value) -> Result<Value> {
    let session_id = required_string(params, "sessionId")?;
    state.browsers.close(&session_id)?;
    Ok(json!({ "ok": true }))
}

/// Handles `browser_recording`.
pub(crate) fn handle_browser_recording(state: &Arc<DaemonState>, params: &Value) -> Result<Value> {
    let session_id = required_string(params, "sessionId")?;
    Ok(state.browsers.recording_frames(&session_id))
}

/// Handles `browser_current_tab`.
pub(crate) fn handle_browser_current_tab(
    state: &Arc<DaemonState>,
    params: &Value,
) -> Result<Value> {
    let session_id = required_string(params, "sessionId")?;
    Ok(serde_json::to_value(
        state.browsers.current_tab_context(&session_id),
    )?)
}
