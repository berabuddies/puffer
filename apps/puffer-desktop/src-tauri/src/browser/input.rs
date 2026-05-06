//! CDP input serialization for the managed browser worker.

use anyhow::Result;
use serde_json::{json, Value};
use std::net::TcpStream;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::WebSocket;

use super::{send_cdp, BrowserInputEvent};

/// Dispatches one UI or agent input event to Chrome through CDP.
pub(super) fn send_input(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: &mut u64,
    event: BrowserInputEvent,
) -> Result<u64> {
    let (method, params): (&str, Value) = match event {
        BrowserInputEvent::Mouse {
            event_type,
            x,
            y,
            button,
            buttons,
            click_count,
        } => {
            let mut params = json!({
                "type": event_type,
                "x": x,
                "y": y,
                "button": button,
                "clickCount": click_count
            });
            if let Some(buttons) = buttons {
                params["buttons"] = json!(buttons);
            }
            ("Input.dispatchMouseEvent", params)
        }
        BrowserInputEvent::Wheel {
            x,
            y,
            delta_x,
            delta_y,
        } => (
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseWheel",
                "x": x,
                "y": y,
                "deltaX": delta_x,
                "deltaY": delta_y
            }),
        ),
        BrowserInputEvent::Key {
            event_type,
            key,
            code,
            text,
            modifiers,
        } => (
            "Input.dispatchKeyEvent",
            json!({
                "type": event_type,
                "key": key,
                "code": code,
                "text": text.unwrap_or_default(),
                "modifiers": modifiers
            }),
        ),
        BrowserInputEvent::Text { text } => ("Input.insertText", json!({ "text": text })),
    };
    Ok(send_cdp(socket, next_id, method, params))
}
