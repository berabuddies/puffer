use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::{BrowserInputEvent, BrowserState};

/// Parses a browser input RPC payload into the CDP input event model.
pub(super) fn parse_input_event(value: &Value) -> Result<BrowserInputEvent> {
    match required_string(value, "kind")?.as_str() {
        "mouse" => Ok(BrowserInputEvent::Mouse {
            event_type: required_string(value, "eventType")?,
            x: required_f64(value, "x")?,
            y: required_f64(value, "y")?,
            button: value
                .get("button")
                .and_then(Value::as_str)
                .unwrap_or("none")
                .to_string(),
            buttons: optional_u32(value, "buttons"),
            click_count: optional_u32(value, "clickCount").unwrap_or(0),
        }),
        "wheel" => Ok(BrowserInputEvent::Wheel {
            x: required_f64(value, "x")?,
            y: required_f64(value, "y")?,
            delta_x: value.get("deltaX").and_then(Value::as_f64).unwrap_or(0.0),
            delta_y: value.get("deltaY").and_then(Value::as_f64).unwrap_or(0.0),
        }),
        "key" => Ok(BrowserInputEvent::Key {
            event_type: required_string(value, "eventType")?,
            key: required_string(value, "key")?,
            code: value
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            text: value
                .get("text")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            modifiers: optional_u32(value, "modifiers").unwrap_or(0),
        }),
        "text" => Ok(BrowserInputEvent::Text {
            text: required_string(value, "text")?,
        }),
        other => bail!("unsupported browser input kind `{other}`"),
    }
}

/// Serializes the current browser state for daemon RPC responses.
pub(super) fn state_json(state: &BrowserState) -> Value {
    json!({
        "url": state.url,
        "title": state.title,
        "loading": state.loading,
        "width": state.width,
        "height": state.height,
        "popOut": false
    })
}

/// Reads a required string field from an RPC payload.
pub(super) fn required_string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .with_context(|| format!("missing `{key}`"))
}

/// Reads an optional unsigned integer field from an RPC payload.
pub(super) fn optional_u32(value: &Value, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

/// Reads one required array of non-empty strings from an RPC payload.
pub(super) fn required_string_array(value: &Value, key: &str) -> Result<Vec<String>> {
    let values = value
        .get(key)
        .and_then(Value::as_array)
        .with_context(|| format!("missing `{key}`"))?;
    if values.is_empty() {
        bail!("`{key}` must include at least one path");
    }
    values
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(ToString::to_string)
                .with_context(|| format!("`{key}` entries must be non-empty strings"))
        })
        .collect()
}

fn required_f64(value: &Value, key: &str) -> Result<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("missing `{key}`"))
}
