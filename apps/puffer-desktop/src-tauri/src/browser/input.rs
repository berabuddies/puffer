//! CDP input serialization for the managed browser worker.

use anyhow::Result;
use serde_json::{json, Map, Value};
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
            key_event_params(event_type, key, code, text, modifiers),
        ),
        BrowserInputEvent::Text { text } => ("Input.insertText", json!({ "text": text })),
    };
    Ok(send_cdp(socket, next_id, method, params))
}

fn key_event_params(
    event_type: String,
    key: String,
    code: String,
    text: Option<String>,
    modifiers: u32,
) -> Value {
    let event_text = key_event_text(&event_type, &key, text);
    let mut params = Map::new();
    params.insert("type".to_string(), json!(event_type));
    params.insert("key".to_string(), json!(key));
    params.insert("code".to_string(), json!(code));
    params.insert("modifiers".to_string(), json!(modifiers));
    if let Some(key_code) = virtual_key_code(
        params
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        params
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    ) {
        params.insert("windowsVirtualKeyCode".to_string(), json!(key_code));
    }
    params.insert(
        "location".to_string(),
        json!(key_location(
            params
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )),
    );
    if let Some(text) = event_text.filter(|value| !value.is_empty()) {
        params.insert("text".to_string(), json!(text));
        params.insert("unmodifiedText".to_string(), json!(text));
    }
    Value::Object(params)
}

fn key_event_text(event_type: &str, key: &str, text: Option<String>) -> Option<String> {
    if event_type == "keyUp" {
        return None;
    }
    if key == "Enter" {
        return Some("\r".to_string());
    }
    text
}

fn virtual_key_code(key: &str, code: &str) -> Option<u32> {
    match key {
        "Backspace" => Some(8),
        "Tab" => Some(9),
        "Enter" => Some(13),
        "Escape" => Some(27),
        " " => Some(32),
        "ArrowLeft" => Some(37),
        "ArrowUp" => Some(38),
        "ArrowRight" => Some(39),
        "ArrowDown" => Some(40),
        "Delete" => Some(46),
        _ => virtual_key_code_from_code(code),
    }
}

fn virtual_key_code_from_code(code: &str) -> Option<u32> {
    if let Some(letter) = code.strip_prefix("Key") {
        return single_ascii(letter).filter(u8::is_ascii_uppercase).map(u32::from);
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        return single_ascii(digit).filter(u8::is_ascii_digit).map(u32::from);
    }
    None
}

fn key_location(code: &str) -> u32 {
    match code {
        "ShiftLeft" | "ControlLeft" | "AltLeft" | "MetaLeft" => 1,
        "ShiftRight" | "ControlRight" | "AltRight" | "MetaRight" => 2,
        code if code.starts_with("Numpad") => 3,
        _ => 0,
    }
}

fn single_ascii(value: &str) -> Option<u8> {
    let bytes = value.as_bytes();
    (bytes.len() == 1).then_some(bytes[0])
}

#[cfg(test)]
mod tests {
    use super::key_event_params;

    #[test]
    fn enter_key_event_includes_virtual_key_and_carriage_return() {
        let params = key_event_params(
            "rawKeyDown".to_string(),
            "Enter".to_string(),
            "Enter".to_string(),
            None,
            0,
        );

        assert_eq!(params["windowsVirtualKeyCode"], 13);
        assert_eq!(params["location"], 0);
        assert_eq!(params["text"], "\r");
        assert_eq!(params["unmodifiedText"], "\r");
    }

    #[test]
    fn backspace_key_event_includes_virtual_key_without_text() {
        let params = key_event_params(
            "rawKeyDown".to_string(),
            "Backspace".to_string(),
            "Backspace".to_string(),
            None,
            0,
        );

        assert_eq!(params["windowsVirtualKeyCode"], 8);
        assert_eq!(params["location"], 0);
        assert!(params.get("text").is_none());
        assert!(params.get("unmodifiedText").is_none());
    }

    #[test]
    fn printable_key_event_keeps_text_and_virtual_key() {
        let params = key_event_params(
            "keyDown".to_string(),
            "a".to_string(),
            "KeyA".to_string(),
            Some("a".to_string()),
            0,
        );

        assert_eq!(params["windowsVirtualKeyCode"], 65);
        assert_eq!(params["location"], 0);
        assert_eq!(params["text"], "a");
        assert_eq!(params["unmodifiedText"], "a");
    }

    #[test]
    fn key_up_event_omits_text() {
        let params = key_event_params(
            "keyUp".to_string(),
            "Enter".to_string(),
            "Enter".to_string(),
            None,
            0,
        );

        assert_eq!(params["windowsVirtualKeyCode"], 13);
        assert!(params.get("text").is_none());
        assert!(params.get("unmodifiedText").is_none());
    }
}
